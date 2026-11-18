//! Fyers API v3 — **experimental, untested against a live account.**
//!
//! Built from myapi.fyers.in/docsv3: browser login (`auth_code`) validated
//! into a daily access token, holdings, positions, the order book, orders
//! (Limit, Market, Stop, Stop-limit; CNC), exact symbol lookup in the
//! public NSE_CM symbol master, and single-leg GTT orders.

use super::app::{self, AppConfig, Needs};
use super::http::{self, unknown_state};
use super::masters::{self, MasterCache};
use super::secrets::Secret;
use super::{
    Broker, BrokerKind, Endpoint, Gtt, GttRequest, Holding, Instrument, Order, OrderAck, OrderKind,
    OrderRequest, Position, Profile, Shape, Side, Trade,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};
use std::sync::Arc;

pub const CONFIG_FILE: &str = "fyers_config.json";
pub const SECRET_KEYS: &[&str] = app::SECRET_KEYS;
const KIND: BrokerKind = BrokerKind::Fyers;
const LABEL: &str = "Fyers";
const NEEDS: Needs = Needs {
    api_secret: true,
    redirect_uri: true,
};
const API: &str = "https://api-t1.fyers.in/api/v3";
/// No header row is documented; the ticker is column 9 (`NSE:SBIN-EQ`).
const SYMBOLS: &str = "https://public.fyers.in/sym_details/NSE_CM.csv";
const TICKER_COL: usize = 9;

pub const SETUP: &[&str] = &[
    "Create an app at myapi.fyers.in/dashboard; note its App ID (e.g. XXXX-100) and Secret ID",
    "Only apps whose App ID ends in -200 can place orders, and only from the app's static IP",
    "Set the app's redirect URL to one you can copy from the address bar, e.g. http://127.0.0.1/",
    "Put the App ID as api_key, the Secret ID as api_secret, and the redirect_uri in the file `trade config --broker fyers` creates",
    "Log in daily with `trade login --broker fyers` (sessions end around 06:30 IST)",
];

pub fn status(migrate: bool) -> Result<super::ConfigStatus> {
    app::status(KIND, NEEDS, migrate)
}

pub fn write_template() -> Result<()> {
    app::save(
        KIND,
        &app::template(NEEDS, "http://127.0.0.1/"),
        &super::secrets::Storage::File,
    )
}

/// Browser login → `auth_code` → daily access token.
pub async fn login() -> Result<()> {
    let (config, storage) = app::for_login(KIND, NEEDS)?;
    let state = http::oauth_state();
    let url = reqwest::Url::parse_with_params(
        &format!("{API}/generate-authcode"),
        &[
            ("client_id", config.api_key.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("response_type", "code"),
            ("state", state.as_str()),
        ],
    )?;
    let code = app::ask_for_redirect(KIND, url.as_str(), "auth_code", Some(&state))?;
    let token = validate_code(API, &config, &code).await?;
    app::save_session(KIND, config, &storage, token, None)
}

/// `appIdHash = sha256("app_id:app_secret")`, lowercase hex.
fn app_id_hash(config: &AppConfig) -> String {
    http::sha256_hex(&format!(
        "{}:{}",
        config.api_key,
        config.api_secret.expose()
    ))
}

async fn validate_code(api: &str, config: &AppConfig, code: &str) -> Result<Secret> {
    let resp = http::client()?
        .post(format!("{api}/validate-authcode"))
        .json(&json!({
            "grant_type": "authorization_code",
            "appIdHash": app_id_hash(config),
            "code": code,
        }))
        .send()
        .await
        .context("Could not reach Fyers")?;
    let (status, text) = http::body(resp).await?;
    let v: Value = http::parse_login(status, &text, LABEL)?;
    envelope(&v, "login")?;
    let token = http::text(v.get("access_token")).context("Fyers returned no access token")?;
    Ok(Secret::new(token))
}

/// `{s: "ok", ...}` passes; `{s: "error", code, message}` → Err.
fn envelope(v: &Value, what: &str) -> Result<()> {
    if v.get("s").and_then(Value::as_str) == Some("ok") {
        return Ok(());
    }
    let msg = http::text(v.get("message")).unwrap_or_else(|| http::excerpt(&v.to_string()));
    match http::text(v.get("code")) {
        Some(c) => bail!("Fyers {what} failed: {msg} (code {c})"),
        None => bail!("Fyers {what} failed: {msg}"),
    }
}

/// Fyers token errors: -8 expired, -15 invalid, -16 / -17 cannot authenticate.
fn is_auth_error(v: &Value) -> bool {
    matches!(
        http::num(v.get("code")).map(|c| c as i64),
        Some(-8 | -15 | -16 | -17)
    )
}

pub struct FyersClient {
    http: reqwest::Client,
    app_id: String,
    token: Secret,
    api: String,
    symbols: String,
    masters: Arc<MasterCache>,
}

/// The symbol master as text, refused unless its first row has an
/// `NSE:` ticker in [`TICKER_COL`] (the file has no header line).
fn parse_master(bytes: &[u8]) -> Result<String> {
    masters::csv_checked(bytes, "Fyers symbol master", |row| {
        row.get(TICKER_COL).is_some_and(|t| t.starts_with("NSE:"))
    })
}

/// The exact `NSE:<SYMBOL>-EQ` row in the NSE_CM symbol master.
fn pick_equity(csv: &str, symbol: &str) -> Result<Instrument> {
    let wanted = format!("NSE:{symbol}-EQ");
    let mut hits = csv
        .lines()
        .map(http::csv_fields)
        .filter(|f| f.get(TICKER_COL).map(|s| s.trim()) == Some(wanted.as_str()));
    let Some(_) = hits.next() else {
        bail!(
            "No exact {wanted} instrument in the Fyers symbol master. Check the symbol with `stockwise search`."
        );
    };
    if hits.next().is_some() {
        bail!("Several Fyers instruments match {wanted}; refusing to guess.");
    }
    Ok(Instrument {
        symbol: symbol.to_string(),
        tradingsymbol: wanted.clone(),
        token: wanted,
        exchange: "NSE".into(),
    })
}

/// `NSE:IDEA-EQ` → (`IDEA`, `NSE`).
fn split_ticker(ticker: &str) -> (String, Option<String>) {
    match ticker.split_once(':') {
        Some((ex, rest)) => (super::strip_series(rest).to_string(), Some(ex.to_string())),
        None => (super::strip_series(ticker).to_string(), None),
    }
}

fn side_code(side: Side) -> i64 {
    match side {
        Side::Buy => 1,
        Side::Sell => -1,
    }
}

fn side_name(v: Option<&Value>) -> Option<String> {
    match http::num(v)? as i64 {
        1 => Some("BUY".into()),
        -1 => Some("SELL".into()),
        _ => None,
    }
}

/// Order type codes: 1 Limit, 2 Market, 3 Stop (SL-M), 4 Stop-limit (SL).
fn type_code(kind: &OrderKind) -> i64 {
    match kind {
        OrderKind::Limit { .. } => 1,
        OrderKind::Market => 2,
        OrderKind::StopMarket { .. } => 3,
        OrderKind::StopLimit { .. } => 4,
    }
}

fn type_name(code: Option<f64>) -> Option<String> {
    Some(
        match code? as i64 {
            1 => "LIMIT",
            2 => "MARKET",
            3 => "SL-M",
            4 => "SL",
            _ => return None,
        }
        .into(),
    )
}

fn status_name(code: Option<f64>) -> Option<String> {
    Some(
        match code? as i64 {
            1 => "cancelled",
            2 => "traded",
            4 => "transit",
            5 => "rejected",
            6 => "pending",
            7 => "expired",
            _ => return None,
        }
        .into(),
    )
}

fn order_body(o: &OrderRequest) -> Value {
    json!({
        "symbol": o.instrument.token,
        "qty": o.qty,
        "type": type_code(&o.kind),
        "side": side_code(o.side),
        "productType": "CNC",
        "limitPrice": o.kind.price().unwrap_or(0.0),
        "stopPrice": o.kind.trigger().unwrap_or(0.0),
        "validity": "DAY",
        "disclosedQty": 0,
        "offlineOrder": false,
        "stopLoss": 0,
        "takeProfit": 0,
        "orderTag": "stockwise",
    })
}

fn gtt_body(g: &GttRequest) -> Value {
    json!({
        "side": side_code(g.side),
        "symbol": g.instrument.token,
        "productType": "CNC",
        "orderInfo": {
            "leg1": {"price": g.limit, "triggerPrice": g.trigger, "qty": g.qty}
        },
    })
}

fn holding(h: &Value) -> Option<Holding> {
    let (symbol, exchange) = split_ticker(&http::text(h.get("symbol"))?);
    Some(Holding {
        symbol,
        exchange,
        isin: http::text(h.get("isin")),
        qty: http::num(h.get("remainingQuantity"))
            .or_else(|| http::num(h.get("quantity")))
            .unwrap_or(0.0),
        avg_price: http::price(h.get("costPrice")),
        ltp: http::price(h.get("ltp")),
        pnl: http::num(h.get("pl")),
    })
}

fn position(p: &Value) -> Option<Position> {
    let (symbol, exchange) = split_ticker(&http::text(p.get("symbol"))?);
    Some(Position {
        symbol,
        exchange,
        product: http::text(p.get("productType")),
        qty: http::num(p.get("netQty")).unwrap_or(0.0),
        avg_price: http::price(p.get("netAvg")),
        ltp: http::price(p.get("ltp")),
        pnl: http::num(p.get("pl")),
    })
}

fn order(o: &Value) -> Option<Order> {
    Some(Order {
        id: http::text(o.get("id"))?,
        symbol: http::text(o.get("symbol")).unwrap_or_default(),
        side: side_name(o.get("side")),
        order_type: type_name(http::num(o.get("type"))),
        qty: http::num(o.get("qty")),
        price: http::price(o.get("limitPrice")),
        trigger: http::price(o.get("stopPrice")),
        status: status_name(http::num(o.get("status"))),
        message: http::text(o.get("message")),
        filled_qty: http::num(o.get("filledQty")),
        avg_price: http::price(o.get("tradedPrice")),
        product: http::text(o.get("productType")),
    })
}

/// Segment code of the capital market (cash equity).
const SEGMENT_CM: i64 = 10;

fn trade(t: &Value) -> Option<Trade> {
    let (symbol, exchange) = split_ticker(&http::text(t.get("symbol"))?);
    // Only capital-market fills keep a plain NSE/BSE exchange.
    let exchange = match http::num(t.get("segment")).map(|s| s as i64) {
        Some(SEGMENT_CM) | None => exchange,
        Some(seg) => Some(format!("{}:{seg}", exchange.unwrap_or_default())),
    };
    let (date, time) = super::split_timestamp(http::text(t.get("orderDateTime")).as_deref());
    Some(Trade {
        trade_id: http::text(t.get("tradeNumber")),
        order_id: http::text(t.get("orderNumber"))?,
        symbol,
        exchange,
        product: http::text(t.get("productType")),
        side: side_name(t.get("side")).and_then(|s| super::side_from(Some(&s))),
        qty: http::num(t.get("tradedQty")).unwrap_or(0.0),
        price: http::price(t.get("tradePrice")),
        date,
        time,
    })
}

fn profile(p: &Value) -> Profile {
    Profile {
        user_id: http::text(p.get("fy_id")),
        exchanges: Vec::new(),
        products: Vec::new(),
    }
}

/// Fields read from each endpoint, and the ones Fyers's docs list.
fn shape(endpoint: Endpoint) -> Shape {
    match endpoint {
        Endpoint::Profile => Shape {
            required: "fy_id",
            documented: "name display_name image email_id pan PAN pin_change_date pwd_change_date \
                mobile_number totp pwd_to_expire ddpi_enabled mtf_enabled",
        },
        Endpoint::Holdings => Shape {
            required: "symbol remainingQuantity|quantity costPrice ltp",
            documented: "holdingType marketVal pl id fyToken exchange segment isin qty_t1 \
                remainingPledgeQuantity collateralQuantity",
        },
        Endpoint::Positions => Shape {
            required: "symbol netQty",
            documented: "qty avgPrice netAvg side productType realized_profit unrealized_profit \
                pl ltp buyQty buyAvg buyVal sellQty sellAvg sellVal slNo fyToken crossCurrency \
                rbiRefRate qtyMulti_com segment id cfBuyQty cfSellQty dayBuyQty daySellQty \
                exchange",
        },
        Endpoint::Orders => Shape {
            required: "id symbol side type status qty",
            documented: "clientId exchange fyToken id_fyers offlineOrder source pan limitPrice \
                productType disclosedQty remainingQuantity segment description ex_sym \
                orderDateTime orderValidity stopPrice tradedPrice filledQty exchOrdId message ch \
                chp lp slNo orderNumStatus instrument orderTag parentId takeProfit stopLoss",
        },
        Endpoint::Trades => Shape {
            required: "tradeNumber orderNumber symbol segment side productType tradedQty \
                tradePrice orderDateTime",
            documented: "clientId exchangeOrderNo exchange orderType fyToken tradeValue row \
                orderTag",
        },
        Endpoint::Gtts => Shape {
            required: "id symbol tran_side qty price_trigger price_limit",
            documented: "clientId exchange fy_token id_fyers instrument lot_size multiplier \
                ord_status precision price2_limit price2_trigger product_type qty2 report_type \
                segment symbol_desc symbol_exch tick_size gtt_oco_ind create_time \
                create_time_epoch oms_msg ltp ltp_ch ltp_chp",
        },
    }
}

fn gtt(g: &Value) -> Option<Gtt> {
    Some(Gtt {
        id: http::text(g.get("id"))?,
        symbol: http::text(g.get("symbol")).unwrap_or_default(),
        side: side_name(g.get("tran_side")),
        qty: http::num(g.get("qty")),
        trigger: http::price(g.get("price_trigger")),
        limit: http::price(g.get("price_limit")),
        status: http::text(g.get("report_type"))
            .or_else(|| status_name(http::num(g.get("ord_status")))),
    })
}

impl FyersClient {
    /// Use the session saved by `trade login --broker fyers`.
    pub fn connect() -> Result<Self> {
        let config = app::session(KIND, NEEDS)?;
        let mut c = Self::with_urls(config.api_key, config.access_token, API, SYMBOLS)?;
        c.masters = MasterCache::shared();
        Ok(c)
    }

    fn with_urls(app_id: String, token: Secret, api: &str, symbols: &str) -> Result<Self> {
        Ok(Self {
            http: http::client()?,
            app_id,
            token,
            api: api.into(),
            symbols: symbols.into(),
            masters: MasterCache::off(),
        })
    }

    /// `Authorization: <app_id>:<access_token>` (no "Bearer").
    fn auth(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let mut v = reqwest::header::HeaderValue::from_str(&format!(
            "{}:{}",
            self.app_id,
            self.token.expose()
        ))
        .context("The saved Fyers session has invalid characters; log in again")?;
        v.set_sensitive(true);
        Ok(req.header(reqwest::header::AUTHORIZATION, v))
    }

    async fn get(&self, path: &str, what: &str) -> Result<Value> {
        let resp = self
            .auth(self.http.get(format!("{}{}", self.api, path)))?
            .send()
            .await
            .with_context(|| format!("Could not reach Fyers for the {what}"))?;
        let (status, text) = http::body(resp).await?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = http::parse(status, &text, LABEL, what)?;
        if is_auth_error(&v) {
            http::check_session(401, LABEL, KIND.id())?;
        }
        envelope(&v, what)?;
        Ok(v)
    }

    fn list(v: &Value, key: &str) -> Vec<Value> {
        v.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// Send an order-changing request; transport and parse failures leave
    /// the state unknown and say so. Returns the id, or None when Fyers
    /// answered "request made but not acknowledged" (code 201).
    async fn send_order(&self, req: reqwest::RequestBuilder, what: &str) -> Result<Option<String>> {
        let resp = self
            .auth(req)?
            .send()
            .await
            .with_context(|| format!("Could not reach Fyers. {}", unknown_state(LABEL)))?;
        let (status, text) = http::body(resp)
            .await
            .with_context(|| unknown_state(LABEL))?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "Unreadable Fyers response (HTTP {status}). {}",
                unknown_state(LABEL)
            )
        })?;
        if is_auth_error(&v) {
            http::check_session(401, LABEL, KIND.id())?;
        }
        if status >= 500 {
            bail!(
                "Fyers error (HTTP {status}): {}. {}",
                http::text(v.get("message")).unwrap_or_default(),
                unknown_state(LABEL)
            );
        }
        envelope(&v, what)?;
        if http::num(v.get("code")).map(|c| c as i64) == Some(201) {
            return Ok(None);
        }
        Ok(http::text(v.get("id")))
    }
}

#[async_trait]
impl Broker for FyersClient {
    fn kind(&self) -> BrokerKind {
        KIND
    }

    async fn holdings(&self) -> Result<Vec<Holding>> {
        let raw = self.rows(Endpoint::Holdings).await?;
        Ok(raw.iter().filter_map(holding).collect())
    }

    async fn positions(&self) -> Result<Vec<Position>> {
        let raw = self.rows(Endpoint::Positions).await?;
        Ok(raw.iter().filter_map(position).collect())
    }

    async fn orders(&self) -> Result<Vec<Order>> {
        let raw = self.rows(Endpoint::Orders).await?;
        Ok(raw.iter().filter_map(order).collect())
    }

    async fn profile(&self) -> Result<Profile> {
        let raw = self.rows(Endpoint::Profile).await?;
        Ok(raw.first().map(profile).unwrap_or_default())
    }

    /// Today's fills; Fyers's trade book has no history.
    async fn trades(&self, _since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
        let raw = self.rows(Endpoint::Trades).await?;
        Ok(raw.iter().filter_map(trade).collect())
    }

    async fn rows(&self, endpoint: Endpoint) -> Result<Vec<Value>> {
        let what = endpoint.label();
        let (path, key) = match endpoint {
            Endpoint::Profile => {
                let v = self.get("/profile", what).await?;
                return Ok(v.get("data").cloned().into_iter().collect());
            }
            Endpoint::Holdings => ("/holdings", "holdings"),
            Endpoint::Positions => ("/positions", "netPositions"),
            Endpoint::Orders => ("/orders", "orderBook"),
            Endpoint::Trades => ("/tradebook", "tradeBook"),
            Endpoint::Gtts => ("/gtt/orders", "orderBook"),
        };
        Ok(Self::list(&self.get(path, what).await?, key))
    }

    fn shape(&self, endpoint: Endpoint) -> Option<Shape> {
        Some(shape(endpoint))
    }

    async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
        if exchange != "NSE" {
            bail!("Fyers orders support NSE only");
        }
        let fetch = || async {
            let resp = self
                .http
                .get(&self.symbols)
                .send()
                .await
                .context("Could not download the Fyers symbol master")?;
            if !resp.status().is_success() {
                bail!("Fyers symbol master: HTTP {}", resp.status());
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
            .post(format!("{}/orders/sync", self.api))
            .json(&order_body(o));
        Ok(OrderAck {
            order_id: self.send_order(req, "order").await?,
        })
    }

    fn supports_gtt(&self) -> bool {
        true
    }

    async fn gtt_create(&self, g: &GttRequest) -> Result<String> {
        let req = self
            .http
            .post(format!("{}/gtt/orders/sync", self.api))
            .json(&gtt_body(g));
        self.send_order(req, "GTT").await?.context(
            "Fyers accepted the GTT but returned no id — check `stockwise trade gtt list --broker fyers`",
        )
    }

    async fn gtt_list(&self) -> Result<Vec<Gtt>> {
        let raw = self.rows(Endpoint::Gtts).await?;
        Ok(raw.iter().filter_map(gtt).collect())
    }

    async fn gtt_cancel(&self, id: &str) -> Result<()> {
        let req = self
            .http
            .delete(format!("{}/gtt/orders/sync", self.api))
            .json(&json!({ "id": id }));
        self.send_order(req, "GTT cancel").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::mock::{MockServer, Route};

    async fn client(routes: Vec<Route>) -> (MockServer, FyersClient) {
        let server = MockServer::start(routes).await;
        let url = server.url();
        let c = FyersClient::with_urls(
            "APPID-200".into(),
            Secret::new("fy-token"),
            &url,
            &format!("{url}/sym_details/NSE_CM.csv"),
        )
        .unwrap();
        (server, c)
    }

    fn sbin() -> Instrument {
        Instrument {
            symbol: "SBIN".into(),
            tradingsymbol: "NSE:SBIN-EQ".into(),
            token: "NSE:SBIN-EQ".into(),
            exchange: "NSE".into(),
        }
    }

    #[test]
    fn app_id_hash_matches_the_docs_vector() {
        // The docs' example digest is sha256 of the literal "app_id:app_secret".
        let config = AppConfig {
            api_key: "app_id".into(),
            api_secret: Secret::new("app_secret"),
            ..Default::default()
        };
        assert_eq!(app_id_hash(&config), http::sha256_hex("app_id:app_secret"));
        assert!(app_id_hash(&config).starts_with("7c7120d2"));
    }

    #[tokio::test]
    async fn holdings_use_app_id_token_auth() {
        let (server, c) = client(vec![Route::json(
            "GET",
            "/holdings",
            include_str!("fixtures/fyers_holdings.json"),
        )])
        .await;
        let h = c.holdings().await.unwrap();
        assert_eq!(
            server.last("GET", "/holdings").header("Authorization"),
            Some("APPID-200:fy-token")
        );
        assert_eq!(h[0].symbol, "IDEA");
        assert_eq!(h[0].exchange.as_deref(), Some("NSE"));
        assert_eq!(h[0].qty, 3.0);
        assert_eq!(h[0].avg_price, Some(6.48));
    }

    #[tokio::test]
    async fn order_book_maps_codes_to_names() {
        let (_s, c) = client(vec![Route::json(
            "GET",
            "/orders",
            include_str!("fixtures/fyers_orders.json"),
        )])
        .await;
        let o = c.orders().await.unwrap();
        assert_eq!(o[0].side.as_deref(), Some("BUY"));
        assert_eq!(o[0].order_type.as_deref(), Some("LIMIT"));
        assert_eq!(o[0].status.as_deref(), Some("traded"));
        assert_eq!(o[0].price, Some(8.1));
    }

    #[tokio::test]
    async fn expired_token_says_log_in_again() {
        let (_s, c) = client(vec![
            Route::json(
                "GET",
                "/positions",
                r#"{"s":"error","code":-8,"message":"Could not authenticate the user"}"#,
            )
            .status(401),
        ])
        .await;
        let err = c.positions().await.unwrap_err().to_string();
        assert!(err.contains("trade login --broker fyers"), "{err}");
    }

    #[test]
    fn symbol_master_lookup_is_exact() {
        let csv = include_str!("fixtures/fyers_nse_cm.csv");
        assert_eq!(pick_equity(csv, "SBIN").unwrap(), sbin());
        assert!(pick_equity(csv, "SBI").is_err());
        // The BE series and an index never match a plain symbol.
        assert!(pick_equity(csv, "MODIRUBBER").is_err());
        assert!(pick_equity(csv, "NIFTY50").is_err());
    }

    #[tokio::test]
    async fn stop_limit_order_body_and_unacknowledged_201() {
        let (server, c) = client(vec![Route::json(
            "POST",
            "/orders/sync",
            r#"{"s":"ok","code":1101,"message":"Order submitted successfully. Your Order Ref. No.808058117761","id":"808058117761"}"#,
        )])
        .await;
        let req = OrderRequest {
            instrument: sbin(),
            side: Side::Sell,
            qty: 2,
            kind: OrderKind::StopLimit {
                trigger: 790.0,
                price: 789.5,
            },
        };
        let ack = c.place_order(&req).await.unwrap();
        assert_eq!(ack.order_id.as_deref(), Some("808058117761"));
        let body = server.last("POST", "/orders/sync").json();
        assert_eq!(body["type"], 4);
        assert_eq!(body["side"], -1);
        assert_eq!(body["stopPrice"], 790.0);
        assert_eq!(body["limitPrice"], 789.5);
        assert_eq!(body["productType"], "CNC");
        assert_eq!(body["symbol"], "NSE:SBIN-EQ");

        let (_s, c) = client(vec![Route::json(
            "POST",
            "/orders/sync",
            r#"{"s":"ok","code":201,"message":"Order request made but not acknowledged"}"#,
        )])
        .await;
        assert_eq!(c.place_order(&req).await.unwrap().order_id, None);
    }

    #[tokio::test]
    async fn gtt_create_list_cancel() {
        let (server, c) = client(vec![
            Route::json(
                "POST",
                "/gtt/orders/sync",
                r#"{"code":1101,"message":"Successfully placed order","s":"ok","id":"25012400002099"}"#,
            ),
            Route::json("GET", "/gtt/orders", include_str!("fixtures/fyers_gtt_list.json")),
            Route::json(
                "DELETE",
                "/gtt/orders/sync",
                r#"{"code":1103,"message":"Successfully cancelled order","s":"ok","id":"25012400002099"}"#,
            ),
        ])
        .await;
        let id = c
            .gtt_create(&GttRequest {
                instrument: sbin(),
                side: Side::Sell,
                qty: 5,
                trigger: 760.0,
                limit: 759.0,
                last_price: 800.0,
            })
            .await
            .unwrap();
        assert_eq!(id, "25012400002099");
        let body = server.last("POST", "/gtt/orders/sync").json();
        assert_eq!(body["side"], -1);
        assert_eq!(body["orderInfo"]["leg1"]["triggerPrice"], 760.0);
        assert_eq!(body["orderInfo"]["leg1"]["price"], 759.0);
        assert!(body["orderInfo"].get("leg2").is_none());

        let list = c.gtt_list().await.unwrap();
        assert_eq!(list[0].id, "25012400002074");
        assert_eq!(list[0].side.as_deref(), Some("BUY"));
        assert_eq!(list[0].status.as_deref(), Some("CANCELLED"));

        c.gtt_cancel("25012400002099").await.unwrap();
        assert_eq!(
            server.last("DELETE", "/gtt/orders/sync").json()["id"],
            "25012400002099"
        );
    }

    #[tokio::test]
    async fn validate_authcode_posts_the_hash_not_the_secret() {
        let server = MockServer::start(vec![Route::json(
            "POST",
            "/validate-authcode",
            r#"{"s":"ok","code":200,"message":"","access_token":"fy-new-token","refresh_token":"r"}"#,
        )])
        .await;
        let config = AppConfig {
            api_key: "APPID-200".into(),
            api_secret: Secret::new("very-secret"),
            ..Default::default()
        };
        let token = validate_code(&server.url(), &config, "authcode-1")
            .await
            .unwrap();
        assert_eq!(token.expose(), "fy-new-token");
        let req = server.last("POST", "/validate-authcode");
        assert_eq!(req.json()["appIdHash"], app_id_hash(&config));
        assert!(!req.body.contains("very-secret"));
    }

    fn read_routes() -> Vec<Route> {
        vec![
            Route::json(
                "GET",
                "/profile",
                include_str!("fixtures/fyers_profile.json"),
            ),
            Route::json(
                "GET",
                "/holdings",
                include_str!("fixtures/fyers_holdings.json"),
            ),
            Route::json("GET", "/orders", include_str!("fixtures/fyers_orders.json")),
            Route::json(
                "GET",
                "/tradebook",
                include_str!("fixtures/fyers_trades.json"),
            ),
            Route::json(
                "GET",
                "/gtt/orders",
                include_str!("fixtures/fyers_gtt_list.json"),
            ),
        ]
    }

    #[tokio::test]
    async fn trade_book_maps_codes_and_segments() {
        let (_s, c) = client(read_routes()).await;
        let t = c.trades(None).await.unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].symbol, "SBIN");
        assert_eq!(t[0].exchange.as_deref(), Some("NSE"));
        assert_eq!(t[0].side, Some(Side::Buy));
        assert_eq!((t[0].qty, t[0].price), (3.0, Some(540.6)));
        assert_eq!(t[0].trade_id.as_deref(), Some("1000001"));
        assert_eq!(t[0].date, chrono::NaiveDate::from_ymd_opt(2026, 10, 20));
        assert!(t[0].is_delivery_equity());
        // Segment 11 is F&O.
        assert_eq!(t[1].exchange.as_deref(), Some("NSE:11"));
        assert_eq!(t[1].side, Some(Side::Sell));
        assert!(!t[1].is_delivery_equity());
        let p = c.profile().await.unwrap();
        assert_eq!(p.user_id.as_deref(), Some("XX00042"));
    }

    #[tokio::test]
    async fn fixtures_match_the_documented_shape() {
        let (_s, c) = client(read_routes()).await;
        crate::brokers::mock::assert_fixture_shapes(
            &c,
            &[
                Endpoint::Profile,
                Endpoint::Holdings,
                Endpoint::Orders,
                Endpoint::Trades,
                Endpoint::Gtts,
            ],
        )
        .await;
    }

    #[test]
    fn a_master_without_tickers_is_not_cached() {
        assert!(parse_master(b"{\"status\":\"error\",\"message\":\"Too many requests\",\"error_type\":\"NetworkException\"}").is_err());
        assert!(parse_master(include_str!("fixtures/fyers_nse_cm.csv").as_bytes()).is_ok());
    }
}
