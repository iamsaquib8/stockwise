//! Zerodha Kite Connect v3 — **experimental, untested against a live account.**
//!
//! Built from kite.trade/docs/connect/v3: browser login (`request_token`)
//! exchanged for a daily `access_token`, holdings, positions, the order
//! book, regular orders (MARKET, LIMIT, SL, SL-M; CNC), exact instrument
//! lookup in the NSE instrument dump, and single-leg GTT triggers.

use super::app::{self, AppConfig, Needs};
use super::http::{self, unknown_state};
use super::masters::{self, MasterCache};
use super::secrets::Secret;
use super::{
    Broker, BrokerKind, Endpoint, Gtt, GttRequest, Holding, Instrument, Order, OrderAck, OrderKind,
    OrderRequest, Position, Profile, Shape, Trade,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

pub const CONFIG_FILE: &str = "zerodha_config.json";
pub const SECRET_KEYS: &[&str] = app::SECRET_KEYS;
const KIND: BrokerKind = BrokerKind::Zerodha;
const LABEL: &str = "Zerodha";
const NEEDS: Needs = Needs {
    api_secret: true,
    redirect_uri: false,
};
const API: &str = "https://api.kite.trade";
const LOGIN_URL: &str = "https://kite.zerodha.com/connect/login";

pub const SETUP: &[&str] = &[
    "Create a Kite Connect app at developers.kite.trade; note its API key and API secret",
    "Set the app's redirect URL to one you can copy from the address bar, e.g. http://127.0.0.1/",
    "Kite only accepts orders from the static IP registered on the app; reads work from anywhere",
    "Put api_key and api_secret in the file `trade config --broker zerodha` creates",
    "Log in daily with `trade login --broker zerodha` (sessions end at 06:00 IST)",
];

pub fn status(migrate: bool) -> Result<super::ConfigStatus> {
    app::status(KIND, NEEDS, migrate)
}

pub fn write_template() -> Result<()> {
    app::save(
        KIND,
        &app::template(NEEDS, ""),
        &super::secrets::Storage::File,
    )
}

/// Browser login → `request_token` → daily access token.
pub async fn login() -> Result<()> {
    let (config, storage) = app::for_login(KIND, NEEDS)?;
    let url = reqwest::Url::parse_with_params(
        LOGIN_URL,
        &[("v", "3"), ("api_key", config.api_key.as_str())],
    )?;
    let request_token = app::ask_for_redirect(KIND, url.as_str(), "request_token", None)?;
    let (token, user) = exchange_token(API, &config, &request_token).await?;
    app::save_session(KIND, config, &storage, token, user)
}

/// `checksum = sha256(api_key + request_token + api_secret)`, lowercase hex.
fn checksum(config: &AppConfig, request_token: &str) -> String {
    http::sha256_hex(&format!(
        "{}{}{}",
        config.api_key,
        request_token,
        config.api_secret.expose()
    ))
}

async fn exchange_token(
    api: &str,
    config: &AppConfig,
    request_token: &str,
) -> Result<(Secret, Option<String>)> {
    let resp = http::client()?
        .post(format!("{api}/session/token"))
        .header("X-Kite-Version", "3")
        .form(&[
            ("api_key", config.api_key.as_str()),
            ("request_token", request_token),
            ("checksum", checksum(config, request_token).as_str()),
        ])
        .send()
        .await
        .context("Could not reach Kite")?;
    let (status, text) = http::body(resp).await?;
    let v: Value = http::parse_login(status, &text, LABEL)?;
    let data = envelope(&v, "login")?;
    let token = http::text(data.get("access_token")).context("Kite returned no access token")?;
    Ok((Secret::new(token), http::text(data.get("user_id"))))
}

/// `{status: "success", data}` → data; `{status: "error", message}` → Err.
fn envelope<'a>(v: &'a Value, what: &str) -> Result<&'a Value> {
    if v.get("status").and_then(Value::as_str) == Some("success") {
        return Ok(v.get("data").unwrap_or(&Value::Null));
    }
    let msg = http::text(v.get("message")).unwrap_or_else(|| http::excerpt(&v.to_string()));
    match http::text(v.get("error_type")) {
        Some(t) => bail!("Kite {what} failed: {msg} ({t})"),
        None => bail!("Kite {what} failed: {msg}"),
    }
}

pub struct KiteClient {
    http: reqwest::Client,
    api_key: String,
    token: Secret,
    api: String,
    masters: Arc<MasterCache>,
}

/// The exact NSE cash-equity row for `symbol` in the instrument dump.
/// Columns are found by name; the base EQ series has no suffix.
fn pick_equity(csv: &str, symbol: &str) -> Result<Instrument> {
    let mut lines = csv.lines();
    let header = http::csv_fields(lines.next().unwrap_or_default());
    let col = |name: &str| {
        header
            .iter()
            .position(|h| h.trim() == name)
            .with_context(|| format!("Kite instrument dump has no `{name}` column"))
    };
    let (tok, sym, itype, seg, exch) = (
        col("instrument_token")?,
        col("tradingsymbol")?,
        col("instrument_type")?,
        col("segment")?,
        col("exchange")?,
    );
    let mut hits = lines.map(http::csv_fields).filter(|f| {
        let get = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
        get(sym) == symbol && get(itype) == "EQ" && get(seg) == "NSE" && get(exch) == "NSE"
    });
    let Some(hit) = hits.next() else {
        bail!(
            "No exact NSE equity match for {symbol} in the Kite instrument list. Check the symbol with `stockwise search`."
        );
    };
    if hits.next().is_some() {
        bail!("Several Kite instruments match {symbol} on NSE; refusing to guess.");
    }
    Ok(Instrument {
        symbol: symbol.to_string(),
        tradingsymbol: symbol.to_string(),
        token: hit[tok].trim().to_string(),
        exchange: "NSE".into(),
    })
}

/// The instrument dump as text, refused unless it has the columns
/// [`pick_equity`] reads.
fn parse_master(bytes: &[u8]) -> Result<String> {
    masters::csv_checked(bytes, "Kite instrument dump", true, |h| {
        masters::has_columns(
            h,
            &[
                "instrument_token",
                "tradingsymbol",
                "instrument_type",
                "segment",
                "exchange",
            ],
        )
    })
}

/// Form fields for POST /orders/regular. Delivery (CNC), DAY validity.
/// MARKET and SL-M carry `market_protection=-1`: Kite rejects them
/// without market protection.
fn order_form(o: &OrderRequest) -> Vec<(&'static str, String)> {
    let px = |p: f64| format!("{p:.2}");
    let mut f = vec![
        ("exchange", o.instrument.exchange.clone()),
        ("tradingsymbol", o.instrument.tradingsymbol.clone()),
        ("transaction_type", o.side.as_str().to_string()),
        ("quantity", o.qty.to_string()),
        ("product", "CNC".into()),
        ("validity", "DAY".into()),
        ("tag", "stockwise".into()),
    ];
    match o.kind {
        OrderKind::Market => {
            f.push(("order_type", "MARKET".into()));
            f.push(("market_protection", "-1".into()));
        }
        OrderKind::Limit { price } => {
            f.push(("order_type", "LIMIT".into()));
            f.push(("price", px(price)));
        }
        OrderKind::StopLimit { trigger, price } => {
            f.push(("order_type", "SL".into()));
            f.push(("price", px(price)));
            f.push(("trigger_price", px(trigger)));
        }
        OrderKind::StopMarket { trigger } => {
            f.push(("order_type", "SL-M".into()));
            f.push(("trigger_price", px(trigger)));
            f.push(("market_protection", "-1".into()));
        }
    }
    f
}

/// Form fields for a single-leg GTT: two of them are JSON strings.
fn gtt_form(g: &GttRequest) -> Vec<(&'static str, String)> {
    let condition = json!({
        "exchange": g.instrument.exchange,
        "tradingsymbol": g.instrument.tradingsymbol,
        "trigger_values": [g.trigger],
        "last_price": g.last_price,
    });
    let orders = json!([{
        "exchange": g.instrument.exchange,
        "tradingsymbol": g.instrument.tradingsymbol,
        "transaction_type": g.side.as_str(),
        "quantity": g.qty,
        "order_type": "LIMIT",
        "product": "CNC",
        "price": g.limit,
    }]);
    vec![
        ("type", "single".into()),
        ("condition", condition.to_string()),
        ("orders", orders.to_string()),
    ]
}

fn holding(h: &Value) -> Option<Holding> {
    let sym = http::text(h.get("tradingsymbol"))?;
    let qty = http::num(h.get("quantity")).unwrap_or(0.0)
        + http::num(h.get("t1_quantity")).unwrap_or(0.0);
    Some(Holding {
        symbol: super::strip_series(&sym).to_string(),
        exchange: http::text(h.get("exchange")),
        isin: http::text(h.get("isin")),
        qty,
        avg_price: http::price(h.get("average_price")),
        ltp: http::price(h.get("last_price")),
        pnl: http::num(h.get("pnl")),
    })
}

fn position(p: &Value) -> Option<Position> {
    Some(Position {
        symbol: http::text(p.get("tradingsymbol"))?,
        exchange: http::text(p.get("exchange")),
        product: http::text(p.get("product")),
        qty: http::num(p.get("quantity")).unwrap_or(0.0),
        avg_price: http::price(p.get("average_price")),
        ltp: http::price(p.get("last_price")),
        pnl: http::num(p.get("pnl")),
    })
}

fn order(o: &Value) -> Option<Order> {
    Some(Order {
        id: http::text(o.get("order_id"))?,
        symbol: http::text(o.get("tradingsymbol")).unwrap_or_default(),
        side: http::text(o.get("transaction_type")),
        order_type: http::text(o.get("order_type")),
        qty: http::num(o.get("quantity")),
        price: http::price(o.get("price")),
        trigger: http::price(o.get("trigger_price")),
        status: http::text(o.get("status")),
        message: http::text(o.get("status_message")),
        filled_qty: http::num(o.get("filled_quantity")),
        avg_price: http::price(o.get("average_price")),
        product: http::text(o.get("product")),
    })
}

fn trade(t: &Value) -> Option<Trade> {
    let sym = http::text(t.get("tradingsymbol"))?;
    let stamp = http::text(
        t.get("fill_timestamp")
            .or_else(|| t.get("exchange_timestamp"))
            .or_else(|| t.get("order_timestamp")),
    );
    let (date, time) = super::split_timestamp(stamp.as_deref());
    Some(Trade {
        trade_id: http::text(t.get("trade_id")),
        order_id: http::text(t.get("order_id"))?,
        symbol: super::strip_series(&sym).to_string(),
        exchange: http::text(t.get("exchange")),
        product: http::text(t.get("product")),
        side: super::side_from(http::text(t.get("transaction_type")).as_deref()),
        qty: http::num(t.get("quantity")).unwrap_or(0.0),
        price: http::price(t.get("average_price")),
        date,
        time,
    })
}

fn profile(p: &Value) -> Profile {
    let list = |k: &str| {
        p.get(k)
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|v| http::text(Some(v))).collect())
            .unwrap_or_default()
    };
    Profile {
        user_id: http::text(p.get("user_id")),
        exchanges: list("exchanges"),
        products: list("products"),
    }
}

/// Fields read from each endpoint, and the ones Kite's docs list.
fn shape(endpoint: Endpoint) -> Shape {
    match endpoint {
        Endpoint::Profile => Shape {
            required: "user_id exchanges products",
            documented: "user_type email user_name user_shortname broker order_types avatar_url \
                meta",
        },
        Endpoint::Holdings => Shape {
            required: "tradingsymbol quantity average_price last_price",
            documented: "exchange instrument_token isin product price used_quantity t1_quantity \
                realised_quantity authorised_quantity authorised_date authorisation \
                opening_quantity short_quantity collateral_quantity collateral_type discrepancy \
                close_price pnl day_change day_change_percentage mtf",
        },
        Endpoint::Positions => Shape {
            required: "tradingsymbol quantity",
            documented: "exchange instrument_token product overnight_quantity multiplier \
                average_price close_price last_price value pnl m2m unrealised realised \
                buy_quantity buy_price buy_value buy_m2m sell_quantity sell_price sell_value \
                sell_m2m day_buy_quantity day_buy_price day_buy_value day_sell_quantity \
                day_sell_price day_sell_value",
        },
        Endpoint::Orders => Shape {
            required: "order_id tradingsymbol transaction_type status quantity",
            documented: "placed_by account_id exchange_order_id parent_order_id status_message \
                status_message_raw order_timestamp exchange_update_timestamp exchange_timestamp \
                variety modified exchange instrument_token order_type validity validity_ttl \
                product disclosed_quantity price trigger_price average_price filled_quantity \
                pending_quantity cancelled_quantity market_protection meta tag tags guid \
                auction_number",
        },
        Endpoint::Trades => Shape {
            required: "trade_id order_id exchange tradingsymbol product average_price quantity \
                transaction_type fill_timestamp",
            documented: "instrument_token exchange_order_id order_timestamp exchange_timestamp",
        },
        Endpoint::Gtts => Shape {
            required: "id status condition orders",
            documented: "user_id parent_trigger type created_at updated_at expires_at meta",
        },
    }
}

fn gtt(t: &Value) -> Option<Gtt> {
    let cond = t.get("condition");
    let first = t
        .get("orders")
        .and_then(Value::as_array)
        .and_then(|o| o.first());
    Some(Gtt {
        id: http::text(t.get("id"))?,
        symbol: http::text(cond.and_then(|c| c.get("tradingsymbol"))).unwrap_or_default(),
        side: http::text(first.and_then(|o| o.get("transaction_type"))),
        qty: http::num(first.and_then(|o| o.get("quantity"))),
        trigger: http::price(
            cond.and_then(|c| c.get("trigger_values"))
                .and_then(Value::as_array)
                .and_then(|v| v.first()),
        ),
        limit: http::price(first.and_then(|o| o.get("price"))),
        status: http::text(t.get("status")),
    })
}

impl KiteClient {
    /// Use the session saved by `trade login --broker zerodha`.
    pub fn connect() -> Result<Self> {
        let config = app::session(KIND, NEEDS)?;
        let mut c = Self::with_url(config.api_key, config.access_token, API)?;
        c.masters = MasterCache::shared();
        Ok(c)
    }

    fn with_url(api_key: String, token: Secret, api: &str) -> Result<Self> {
        Ok(Self {
            http: http::client()?,
            api_key,
            token,
            api: api.into(),
            masters: MasterCache::off(),
        })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let mut v = reqwest::header::HeaderValue::from_str(&format!(
            "token {}:{}",
            self.api_key,
            self.token.expose()
        ))
        .context("The saved Kite session has invalid characters; log in again")?;
        v.set_sensitive(true);
        Ok(req
            .header(reqwest::header::AUTHORIZATION, v)
            .header("X-Kite-Version", "3"))
    }

    async fn get(&self, path: &str, what: &str) -> Result<Value> {
        let resp = self
            .auth(self.http.get(format!("{}{}", self.api, path)))?
            .send()
            .await
            .with_context(|| format!("Could not reach Kite for the {what}"))?;
        let (status, text) = http::body(resp).await?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = http::parse(status, &text, LABEL, what)?;
        Ok(envelope(&v, what)?.clone())
    }

    /// Send an order-changing request; transport and parse failures leave
    /// the state unknown and say so.
    async fn send_order(&self, req: reqwest::RequestBuilder, what: &str) -> Result<Value> {
        let resp = self
            .auth(req)?
            .send()
            .await
            .with_context(|| format!("Could not reach Kite. {}", unknown_state(LABEL)))?;
        let (status, text) = http::body(resp)
            .await
            .with_context(|| unknown_state(LABEL))?;
        http::check_session(status, LABEL, KIND.id())?;
        if status == 428 {
            bail!(
                "Kite needs you to authorise this sale at the depository (CDSL TPIN) first — \
                 do it in the Kite app, then retry"
            );
        }
        let v: Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "Unreadable Kite response (HTTP {status}). {}",
                unknown_state(LABEL)
            )
        })?;
        if status >= 500 {
            bail!(
                "Kite error (HTTP {status}): {}. {}",
                http::text(v.get("message")).unwrap_or_default(),
                unknown_state(LABEL)
            );
        }
        Ok(envelope(&v, what)?.clone())
    }

    fn list(v: Value) -> Vec<Value> {
        match v {
            Value::Array(a) => a,
            _ => vec![],
        }
    }
}

#[async_trait]
impl Broker for KiteClient {
    fn kind(&self) -> BrokerKind {
        KIND
    }

    async fn holdings(&self) -> Result<Vec<Holding>> {
        let rows = self.rows(Endpoint::Holdings).await?;
        Ok(rows.iter().filter_map(holding).collect())
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let rows = self.rows(Endpoint::Positions).await?;
        Ok(rows.iter().filter_map(position).collect())
    }

    async fn orders(&self) -> Result<Vec<Order>> {
        let rows = self.rows(Endpoint::Orders).await?;
        Ok(rows.iter().filter_map(order).collect())
    }

    async fn profile(&self) -> Result<Profile> {
        let v = self.get("/user/profile", "profile").await?;
        Ok(profile(&v))
    }

    /// Today's fills; Kite's trade book has no history.
    async fn trades(&self, _since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
        let rows = self.rows(Endpoint::Trades).await?;
        Ok(rows.iter().filter_map(trade).collect())
    }

    async fn rows(&self, endpoint: Endpoint) -> Result<Vec<Value>> {
        let what = endpoint.label();
        Ok(match endpoint {
            Endpoint::Profile => vec![self.get("/user/profile", what).await?],
            Endpoint::Holdings => Self::list(self.get("/portfolio/holdings", what).await?),
            Endpoint::Positions => {
                let v = self.get("/portfolio/positions", what).await?;
                Self::list(v.get("net").cloned().unwrap_or(Value::Null))
            }
            Endpoint::Orders => Self::list(self.get("/orders", what).await?),
            Endpoint::Trades => Self::list(self.get("/trades", what).await?),
            Endpoint::Gtts => Self::list(self.get("/gtt/triggers", what).await?),
        })
    }

    fn shape(&self, endpoint: Endpoint) -> Option<Shape> {
        Some(shape(endpoint))
    }

    async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
        if exchange != "NSE" {
            bail!("Kite orders support NSE only");
        }
        let fetch = || async {
            let resp = self
                .auth(self.http.get(format!("{}/instruments/NSE", self.api)))?
                .send()
                .await
                .context("Could not download the Kite instrument list")?;
            let status = resp.status().as_u16();
            http::check_session(status, LABEL, KIND.id())?;
            if status != 200 {
                bail!("Kite instrument list: HTTP {status}");
            }
            Ok(resp.bytes().await?.to_vec())
        };
        let csv = self
            .masters
            .load(KIND.id(), "NSE", fetch, parse_master)
            .await?;
        pick_equity(&csv, symbol)
    }

    async fn place_order(&self, o: &OrderRequest) -> Result<OrderAck> {
        let req = self
            .http
            .post(format!("{}/orders/regular", self.api))
            .form(&order_form(o));
        let data = self.send_order(req, "order").await?;
        Ok(OrderAck {
            order_id: http::text(data.get("order_id")),
        })
    }

    fn supports_gtt(&self) -> bool {
        true
    }

    async fn gtt_create(&self, g: &GttRequest) -> Result<String> {
        let req = self
            .http
            .post(format!("{}/gtt/triggers", self.api))
            .form(&gtt_form(g));
        let data = self.send_order(req, "GTT").await?;
        http::text(data.get("trigger_id")).context(
            "Kite accepted the GTT but returned no id — check `stockwise trade gtt list --broker zerodha`",
        )
    }

    async fn gtt_list(&self) -> Result<Vec<Gtt>> {
        let rows = self.rows(Endpoint::Gtts).await?;
        Ok(rows.iter().filter_map(gtt).collect())
    }

    async fn gtt_cancel(&self, id: &str) -> Result<()> {
        if !id.chars().all(|c| c.is_ascii_digit()) {
            bail!("Kite GTT ids are numbers; got {id:?}");
        }
        let req = self
            .http
            .delete(format!("{}/gtt/triggers/{}", self.api, id));
        self.send_order(req, "GTT cancel").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::Side;
    use crate::brokers::mock::{MockServer, Route};

    async fn client(routes: Vec<Route>) -> (MockServer, KiteClient) {
        let server = MockServer::start(routes).await;
        let c = KiteClient::with_url("kitekey".into(), Secret::new("kitetoken"), &server.url())
            .unwrap();
        (server, c)
    }

    fn infy() -> Instrument {
        Instrument {
            symbol: "INFY".into(),
            tradingsymbol: "INFY".into(),
            token: "408065".into(),
            exchange: "NSE".into(),
        }
    }

    #[test]
    fn checksum_is_sha256_of_key_token_secret() {
        let config = AppConfig {
            api_key: "key".into(),
            api_secret: Secret::new("secret"),
            ..Default::default()
        };
        assert_eq!(checksum(&config, "tok"), http::sha256_hex("keytoksecret"));
        assert_eq!(checksum(&config, "tok").len(), 64);
    }

    #[tokio::test]
    async fn holdings_use_token_auth_and_include_t1() {
        let (server, c) = client(vec![Route::json(
            "GET",
            "/portfolio/holdings",
            include_str!("fixtures/kite_holdings.json"),
        )])
        .await;
        let h = c.holdings().await.unwrap();
        let req = server.last("GET", "/portfolio/holdings");
        assert_eq!(req.header("Authorization"), Some("token kitekey:kitetoken"));
        assert_eq!(req.header("X-Kite-Version"), Some("3"));
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].symbol, "AARON");
        assert_eq!(h[0].avg_price, Some(161.0));
        // 10 settled + 2 bought yesterday; series suffix removed.
        assert_eq!(h[2].symbol, "RELINFRA");
        assert_eq!(h[2].qty, 12.0);
    }

    #[tokio::test]
    async fn positions_use_the_net_list() {
        let (_s, c) = client(vec![Route::json(
            "GET",
            "/portfolio/positions",
            include_str!("fixtures/kite_positions.json"),
        )])
        .await;
        let p = c.positions().await.unwrap();
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].symbol, "SBIN");
        assert_eq!(p[0].qty, -5.0);
    }

    #[tokio::test]
    async fn token_exception_says_log_in_again() {
        let (_s, c) = client(vec![
            Route::json(
                "GET",
                "/orders",
                r#"{"status":"error","message":"Incorrect `api_key` or `access_token`.","data":null,"error_type":"TokenException"}"#,
            )
            .status(403),
        ])
        .await;
        let err = c.orders().await.unwrap_err().to_string();
        assert!(err.contains("trade login --broker zerodha"), "{err}");
    }

    #[tokio::test]
    async fn order_book_parses_rejections() {
        let (_s, c) = client(vec![Route::json(
            "GET",
            "/orders",
            include_str!("fixtures/kite_orders.json"),
        )])
        .await;
        let o = c.orders().await.unwrap();
        assert_eq!(o.len(), 2);
        assert_eq!(o[1].status.as_deref(), Some("REJECTED"));
        assert!(o[1].message.as_deref().unwrap().contains("Insufficient"));
        // Large ids stay strings.
        assert_eq!(o[1].id, "1953341975595868160");
    }

    #[test]
    fn instrument_lookup_is_exact() {
        let csv = include_str!("fixtures/kite_instruments.csv");
        let i = pick_equity(csv, "INFY").unwrap();
        assert_eq!(i.token, "408065");
        assert_eq!(i.tradingsymbol, "INFY");
        // CDSL-BE is a different series; "CDSL" is the EQ row.
        assert_eq!(pick_equity(csv, "CDSL").unwrap().token, "5420545");
        assert!(pick_equity(csv, "CDS").is_err());
        // An ETF whose name contains RELIANCE never matches RELIANCE.
        assert!(pick_equity(csv, "RELIANCE").is_err());
        // F&O rows never match.
        assert!(pick_equity(csv, "BANKNIFTY18JAN23500CE").is_err());
    }

    /// Lookups on one IST day download the dump once; a new day, or a
    /// damaged cache file, downloads it again. Only the mock server is called.
    #[tokio::test]
    async fn instrument_dump_is_cached_per_ist_day() {
        let (server, mut c) = client(vec![Route::bytes(
            "GET",
            "/instruments/NSE",
            include_str!("fixtures/kite_instruments.csv")
                .as_bytes()
                .to_vec(),
            "text/csv",
        )])
        .await;
        let dir = std::env::temp_dir().join(format!("sw-kite-masters-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let date = Arc::new(std::sync::Mutex::new(
            chrono::NaiveDate::from_ymd_opt(2026, 11, 17).unwrap(),
        ));
        let cache = |date: &Arc<std::sync::Mutex<chrono::NaiveDate>>| {
            let d = date.clone();
            Arc::new(MasterCache::at(&dir, move || *d.lock().unwrap()))
        };
        let downloads = || server.count("GET", "/instruments/NSE");

        c.masters = cache(&date);
        assert_eq!(c.resolve_instrument("INFY", "NSE").await.unwrap(), infy());
        assert_eq!(c.resolve_instrument("INFY", "NSE").await.unwrap(), infy());
        assert_eq!(downloads(), 1, "second lookup is served from memory");

        // A new process on the same day reads the file.
        c.masters = cache(&date);
        assert_eq!(c.resolve_instrument("INFY", "NSE").await.unwrap(), infy());
        assert_eq!(downloads(), 1, "same-day lookup is served from the file");

        // A damaged file is downloaded again.
        let file = dir.join("zerodha-NSE-2026-11-17.master");
        std::fs::write(&file, b"garbage").unwrap();
        c.masters = cache(&date);
        assert_eq!(c.resolve_instrument("INFY", "NSE").await.unwrap(), infy());
        assert_eq!(downloads(), 2);

        // The next IST day downloads again.
        *date.lock().unwrap() = chrono::NaiveDate::from_ymd_opt(2026, 11, 18).unwrap();
        assert_eq!(c.resolve_instrument("INFY", "NSE").await.unwrap(), infy());
        assert_eq!(downloads(), 3);
        assert!(!file.exists(), "the old day's file is removed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn instrument_dump_may_be_gzipped() {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(include_str!("fixtures/kite_instruments.csv").as_bytes())
            .unwrap();
        let (_s, c) = client(vec![Route::bytes(
            "GET",
            "/instruments/NSE",
            e.finish().unwrap(),
            "text/csv",
        )])
        .await;
        assert_eq!(c.resolve_instrument("INFY", "NSE").await.unwrap(), infy());
    }

    #[tokio::test]
    async fn stop_loss_limit_and_market_order_forms() {
        let (server, c) = client(vec![Route::json(
            "POST",
            "/orders/regular",
            r#"{"status":"success","data":{"order_id":"151220000000000"}}"#,
        )])
        .await;
        let ack = c
            .place_order(&OrderRequest {
                instrument: infy(),
                side: Side::Sell,
                qty: 1,
                kind: OrderKind::StopLimit {
                    trigger: 1495.0,
                    price: 1494.9,
                },
            })
            .await
            .unwrap();
        assert_eq!(ack.order_id.as_deref(), Some("151220000000000"));
        let req = server.last("POST", "/orders/regular");
        assert_eq!(req.form("order_type").as_deref(), Some("SL"));
        assert_eq!(req.form("price").as_deref(), Some("1494.90"));
        assert_eq!(req.form("trigger_price").as_deref(), Some("1495.00"));
        assert_eq!(req.form("product").as_deref(), Some("CNC"));
        assert_eq!(req.form("transaction_type").as_deref(), Some("SELL"));
        assert_eq!(req.form("market_protection"), None);

        c.place_order(&OrderRequest {
            instrument: infy(),
            side: Side::Buy,
            qty: 2,
            kind: OrderKind::Market,
        })
        .await
        .unwrap();
        let req = server.last("POST", "/orders/regular");
        assert_eq!(req.form("order_type").as_deref(), Some("MARKET"));
        assert_eq!(req.form("market_protection").as_deref(), Some("-1"));
        assert_eq!(req.form("price"), None);
    }

    #[tokio::test]
    async fn rejected_order_vs_unknown_state() {
        let req = OrderRequest {
            instrument: infy(),
            side: Side::Buy,
            qty: 1,
            kind: OrderKind::Limit { price: 1500.0 },
        };
        let (_s, c) = client(vec![
            Route::json(
                "POST",
                "/orders/regular",
                r#"{"status":"error","message":"Insufficient funds.","data":null,"error_type":"MarginException"}"#,
            )
            .status(400),
        ])
        .await;
        let err = format!("{:#}", c.place_order(&req).await.unwrap_err());
        assert!(
            err.contains("Insufficient funds") && !err.contains("UNKNOWN"),
            "{err}"
        );

        let (_s, c) = client(vec![
            Route::json("POST", "/orders/regular", "upstream timeout").status(504),
        ])
        .await;
        let err = format!("{:#}", c.place_order(&req).await.unwrap_err());
        assert!(err.contains("UNKNOWN"), "{err}");
    }

    #[tokio::test]
    async fn gtt_create_list_cancel() {
        let (server, c) = client(vec![
            Route::json(
                "POST",
                "/gtt/triggers",
                r#"{"status":"success","data":{"trigger_id":123}}"#,
            ),
            Route::json(
                "GET",
                "/gtt/triggers",
                include_str!("fixtures/kite_gtt_list.json"),
            ),
            Route::json(
                "DELETE",
                "/gtt/triggers/112127",
                r#"{"status":"success","data":{"trigger_id":112127}}"#,
            ),
        ])
        .await;
        let id = c
            .gtt_create(&GttRequest {
                instrument: infy(),
                side: Side::Buy,
                qty: 1,
                trigger: 702.0,
                limit: 702.5,
                last_price: 798.0,
            })
            .await
            .unwrap();
        assert_eq!(id, "123");
        let req = server.last("POST", "/gtt/triggers");
        assert_eq!(req.form("type").as_deref(), Some("single"));
        let cond: Value = serde_json::from_str(&req.form("condition").unwrap()).unwrap();
        assert_eq!(cond["trigger_values"], json!([702.0]));
        assert_eq!(cond["last_price"], 798.0);
        let orders: Value = serde_json::from_str(&req.form("orders").unwrap()).unwrap();
        assert_eq!(orders[0]["order_type"], "LIMIT");
        assert_eq!(orders[0]["price"], 702.5);
        assert_eq!(orders[0]["product"], "CNC");

        let list = c.gtt_list().await.unwrap();
        assert_eq!(list[0].id, "112127");
        assert_eq!(list[0].trigger, Some(702.0));
        assert_eq!(list[0].limit, Some(702.5));
        assert_eq!(list[1].status.as_deref(), Some("triggered"));

        c.gtt_cancel("112127").await.unwrap();
        assert_eq!(server.count("DELETE", "/gtt/triggers/112127"), 1);
        assert!(c.gtt_cancel("../orders").await.is_err());
    }

    #[tokio::test]
    async fn login_exchange_sends_checksum() {
        let server = MockServer::start(vec![Route::json(
            "POST",
            "/session/token",
            include_str!("fixtures/kite_session.json"),
        )])
        .await;
        let config = AppConfig {
            api_key: "kitekey".into(),
            api_secret: Secret::new("kitesecret"),
            ..Default::default()
        };
        let (token, user) = exchange_token(&server.url(), &config, "reqtok")
            .await
            .unwrap();
        assert_eq!(token.expose(), "kite-access-token");
        assert_eq!(user.as_deref(), Some("XX0000"));
        let req = server.last("POST", "/session/token");
        assert_eq!(req.form("request_token").as_deref(), Some("reqtok"));
        assert_eq!(
            req.form("checksum").unwrap(),
            http::sha256_hex("kitekeyreqtokkitesecret")
        );
        // The secret itself never goes over the wire.
        assert!(!req.body.contains("kitesecret"));
    }

    fn all_fixtures() -> Vec<Route> {
        vec![
            Route::json(
                "GET",
                "/user/profile",
                include_str!("fixtures/kite_profile.json"),
            ),
            Route::json(
                "GET",
                "/portfolio/holdings",
                include_str!("fixtures/kite_holdings.json"),
            ),
            Route::json(
                "GET",
                "/portfolio/positions",
                include_str!("fixtures/kite_positions.json"),
            ),
            Route::json("GET", "/orders", include_str!("fixtures/kite_orders.json")),
            Route::json("GET", "/trades", include_str!("fixtures/kite_trades.json")),
            Route::json(
                "GET",
                "/gtt/triggers",
                include_str!("fixtures/kite_gtt_list.json"),
            ),
        ]
    }

    #[tokio::test]
    async fn trade_book_parses_partial_fills_and_products() {
        let (_s, c) = client(all_fixtures()).await;
        let t = c.trades(None).await.unwrap();
        assert_eq!(t.len(), 5);
        // One order filled in two parts.
        assert_eq!((t[0].order_id.as_str(), t[0].qty), ("200000000000000", 4.0));
        assert_eq!((t[1].order_id.as_str(), t[1].qty), ("200000000000000", 6.0));
        assert_eq!(t[1].trade_id.as_deref(), Some("10000001"));
        assert_eq!(t[1].price, Some(1510.0));
        assert_eq!(t[0].date, chrono::NaiveDate::from_ymd_opt(2026, 10, 20));
        assert_eq!(t[0].time.as_deref(), Some("09:16:39"));
        assert_eq!(t[2].side, Some(Side::Sell));
        // CNC cash equity only: not MIS, not F&O.
        let delivery: Vec<bool> = t.iter().map(Trade::is_delivery_equity).collect();
        assert_eq!(delivery, [true, true, true, false, false]);
    }

    #[tokio::test]
    async fn profile_and_order_fill_fields() {
        let (_s, c) = client(all_fixtures()).await;
        let p = c.profile().await.unwrap();
        assert_eq!(p.user_id.as_deref(), Some("XX0042"));
        assert!(p.exchanges.contains(&"NSE".to_string()));
        assert!(p.products.contains(&"CNC".to_string()));
        let o = c.orders().await.unwrap();
        assert_eq!((o[0].filled_qty, o[0].avg_price), (Some(1.0), Some(109.4)));
        assert_eq!(o[0].product.as_deref(), Some("CNC"));
    }

    #[tokio::test]
    async fn fixtures_match_the_documented_shape() {
        let (_s, c) = client(all_fixtures()).await;
        crate::brokers::mock::assert_fixture_shapes(&c, &Endpoint::ALL).await;
    }

    #[test]
    fn a_master_without_the_lookup_columns_is_not_cached() {
        assert!(parse_master(b"{\"status\":\"error\",\"message\":\"Too many requests\",\"error_type\":\"NetworkException\"}").is_err());
        assert!(parse_master(b"a,b,c\n1,2,3\n").is_err());
        let header = b"instrument_token,tradingsymbol,instrument_type,segment,exchange\n";
        assert!(
            parse_master(header).is_err(),
            "a header with no rows is refused"
        );
        assert!(parse_master(include_str!("fixtures/kite_instruments.csv").as_bytes()).is_ok());
    }
}
