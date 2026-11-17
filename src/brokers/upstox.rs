//! Upstox API v2/v3 — **experimental, untested against a live account.**
//!
//! Built from the public docs at upstox.com/developer/api-documentation:
//! OAuth login (`/v2/login/authorization/*`), holdings, positions, the
//! order book, v3 order placement on the HFT host, and exact instrument
//! lookup in the public NSE instrument file. GTT orders are not supported:
//! Upstox GTT rules have no limit price, so they can't express StockWise's
//! "LIMIT at this price once triggered".

use super::app::{self, AppConfig, Needs};
use super::http::{self, unknown_state};
use super::masters::MasterCache;
use super::secrets::Secret;
use super::{
    Broker, BrokerKind, Endpoint, Holding, Instrument, Order, OrderAck, OrderKind, OrderRequest,
    Position, Profile, Shape, Trade,
};
use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub const CONFIG_FILE: &str = "upstox_config.json";
pub const SECRET_KEYS: &[&str] = app::SECRET_KEYS;
const KIND: BrokerKind = BrokerKind::Upstox;
const LABEL: &str = "Upstox";
const NEEDS: Needs = Needs {
    api_secret: true,
    redirect_uri: true,
};
const API: &str = "https://api.upstox.com";
/// Order placement lives on a separate host.
const HFT: &str = "https://api-hft.upstox.com";
const INSTRUMENTS: &str = "https://assets.upstox.com/market-quote/instruments/exchange/NSE.json.gz";

pub const SETUP: &[&str] = &[
    "Create an app at account.upstox.com/developer/apps; note its API key and API secret",
    "Set the app's redirect URL, e.g. http://127.0.0.1/ (any URL you can copy from the address bar)",
    "Upstox only accepts orders from the static IP registered on the app; reads work from anywhere",
    "Put api_key, api_secret and the same redirect_uri in the file `trade config --broker upstox` creates",
    "Log in daily with `trade login --broker upstox` (sessions end at 03:30 IST)",
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

/// Browser login → authorization code → access token.
pub async fn login() -> Result<()> {
    let (config, storage) = app::for_login(KIND, NEEDS)?;
    let state = http::oauth_state();
    let url = reqwest::Url::parse_with_params(
        &format!("{API}/v2/login/authorization/dialog"),
        &[
            ("response_type", "code"),
            ("client_id", config.api_key.as_str()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("state", state.as_str()),
        ],
    )?;
    let code = app::ask_for_redirect(KIND, url.as_str(), "code", Some(&state))?;
    let (token, user) = exchange_code(API, &config, &code).await?;
    app::save_session(KIND, config, &storage, token, user)
}

/// POST /v2/login/authorization/token (form-encoded).
async fn exchange_code(
    api: &str,
    config: &AppConfig,
    code: &str,
) -> Result<(Secret, Option<String>)> {
    let resp = http::client()?
        .post(format!("{api}/v2/login/authorization/token"))
        .header("Accept", "application/json")
        .form(&[
            ("code", code),
            ("client_id", config.api_key.as_str()),
            ("client_secret", config.api_secret.expose()),
            ("redirect_uri", config.redirect_uri.as_str()),
            ("grant_type", "authorization_code"),
        ])
        .send()
        .await
        .context("Could not reach Upstox")?;
    let (status, text) = http::body(resp).await?;
    let v: Value = http::parse_login(status, &text, LABEL)?;
    if v.get("status").and_then(Value::as_str) == Some("error") || status >= 400 {
        bail!("Upstox login failed: {}", error_message(&v));
    }
    // The token response is flat (no `data` envelope); accept either.
    let body = v.get("data").filter(|d| d.is_object()).unwrap_or(&v);
    let token = http::text(body.get("access_token")).context("Upstox returned no access token")?;
    Ok((Secret::new(token), http::text(body.get("user_id"))))
}

fn error_message(v: &Value) -> String {
    v.get("errors")
        .and_then(Value::as_array)
        .and_then(|e| e.first())
        .map(|e| {
            let code = http::text(e.get("error_code").or_else(|| e.get("errorCode")));
            let msg = http::text(e.get("message")).unwrap_or_else(|| "unknown error".into());
            match code {
                Some(c) => format!("{msg} ({c})"),
                None => msg,
            }
        })
        .unwrap_or_else(|| http::excerpt(&v.to_string()))
}

pub struct UpstoxClient {
    http: reqwest::Client,
    token: Secret,
    api: String,
    hft: String,
    instruments: String,
    masters: Arc<MasterCache>,
}

/// One entry of the NSE instrument file (other fields ignored).
#[derive(Debug, Deserialize)]
struct InstrumentRow {
    #[serde(default)]
    segment: String,
    #[serde(default)]
    instrument_type: String,
    #[serde(default)]
    trading_symbol: String,
    #[serde(default)]
    instrument_key: String,
}

/// The exact NSE cash-equity (`EQ` series) entry for `symbol`.
fn pick_equity(rows: &[InstrumentRow], symbol: &str) -> Result<Instrument> {
    let mut hits = rows.iter().filter(|r| {
        r.segment == "NSE_EQ" && r.instrument_type == "EQ" && r.trading_symbol == symbol
    });
    let Some(hit) = hits.next() else {
        bail!(
            "No exact NSE equity match for {symbol} in the Upstox instrument list. Check the symbol with `stockwise search`."
        );
    };
    if hits.next().is_some() {
        bail!("Several Upstox instruments match {symbol} on NSE; refusing to guess.");
    }
    if hit.instrument_key.is_empty() {
        bail!("Upstox instrument {symbol} has no instrument key");
    }
    Ok(Instrument {
        symbol: symbol.to_string(),
        tradingsymbol: hit.trading_symbol.clone(),
        token: hit.instrument_key.clone(),
        exchange: "NSE".into(),
    })
}

/// Decode the gzipped instrument file. The docs don't show the top-level
/// shape; accept a plain array or an object holding one.
fn parse_instruments(gz: &[u8]) -> Result<Vec<InstrumentRow>> {
    use std::io::Read;
    let mut raw = Vec::new();
    flate2::read::GzDecoder::new(gz)
        .read_to_end(&mut raw)
        .context("The Upstox instrument file is not valid gzip")?;
    if let Ok(rows) = serde_json::from_slice::<Vec<InstrumentRow>>(&raw) {
        return Ok(rows);
    }
    let v: Value =
        serde_json::from_slice(&raw).context("The Upstox instrument file is not JSON")?;
    let arr = v
        .as_object()
        .and_then(|o| o.values().find(|x| x.is_array()))
        .context("Unexpected Upstox instrument file layout")?;
    Ok(serde_json::from_value(arr.clone())?)
}

fn order_type(kind: &OrderKind) -> &'static str {
    match kind {
        OrderKind::Market => "MARKET",
        OrderKind::Limit { .. } => "LIMIT",
        OrderKind::StopLimit { .. } => "SL",
        OrderKind::StopMarket { .. } => "SL-M",
    }
}

/// v3 place-order body. Delivery product, DAY validity, no slicing.
/// MARKET and SL-M carry `market_protection: -1` (Upstox's automatic
/// protection band) because unprotected market orders may be rejected.
fn order_body(o: &OrderRequest) -> Value {
    let mut body = json!({
        "quantity": o.qty,
        "product": "D",
        "validity": "DAY",
        "price": o.kind.price().unwrap_or(0.0),
        "tag": "stockwise",
        "instrument_token": o.instrument.token,
        "order_type": order_type(&o.kind),
        "transaction_type": o.side.as_str(),
        "disclosed_quantity": 0,
        "trigger_price": o.kind.trigger().unwrap_or(0.0),
        "is_amo": false,
        "slice": false,
    });
    if matches!(o.kind, OrderKind::Market | OrderKind::StopMarket { .. }) {
        body["market_protection"] = json!(-1);
    }
    body
}

fn holding(h: &Value) -> Option<Holding> {
    let sym = http::text(h.get("trading_symbol").or_else(|| h.get("tradingsymbol")))?;
    Some(Holding {
        symbol: sym,
        exchange: http::text(h.get("exchange")),
        isin: http::text(h.get("isin")),
        qty: http::num(h.get("quantity")).unwrap_or(0.0),
        avg_price: http::price(h.get("average_price")),
        ltp: http::price(h.get("last_price")),
        pnl: http::num(h.get("pnl")),
    })
}

fn position(p: &Value) -> Option<Position> {
    let sym = http::text(p.get("trading_symbol").or_else(|| p.get("tradingsymbol")))?;
    Some(Position {
        symbol: sym,
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
        symbol: http::text(o.get("trading_symbol").or_else(|| o.get("tradingsymbol")))
            .unwrap_or_default(),
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
    let sym = http::text(t.get("trading_symbol").or_else(|| t.get("tradingsymbol")))?;
    let stamp = http::text(
        t.get("exchange_timestamp")
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

/// Fields read from each endpoint, and the ones Upstox's docs list.
fn shape(endpoint: Endpoint) -> Shape {
    match endpoint {
        Endpoint::Profile => Shape {
            required: "user_id exchanges products",
            documented: "email broker user_name order_types user_type poa ddpi is_active",
        },
        Endpoint::Holdings => Shape {
            required: "trading_symbol|tradingsymbol quantity average_price last_price",
            documented: "isin cnc_used_quantity collateral_type company_name haircut product \
                close_price pnl day_change day_change_percentage instrument_token \
                collateral_quantity collateral_update_quantity t1_quantity exchange",
        },
        Endpoint::Positions => Shape {
            required: "trading_symbol|tradingsymbol quantity",
            documented: "exchange multiplier value pnl product instrument_token average_price \
                buy_value overnight_quantity day_buy_value day_buy_price overnight_buy_amount \
                overnight_buy_quantity day_buy_quantity day_sell_value day_sell_price \
                overnight_sell_amount overnight_sell_quantity day_sell_quantity last_price \
                unrealised realised sell_value close_price buy_price sell_price",
        },
        Endpoint::Orders => Shape {
            required: "order_id status transaction_type quantity",
            documented: "exchange product price guid tag instrument_token placed_by \
                trading_symbol tradingsymbol order_type validity trigger_price disclosed_quantity \
                average_price filled_quantity pending_quantity status_message status_message_raw \
                exchange_order_id parent_order_id variety order_timestamp exchange_timestamp \
                is_amo order_request_id order_ref_id",
        },
        Endpoint::Trades => Shape {
            required: "trade_id order_id exchange product trading_symbol|tradingsymbol \
                transaction_type quantity average_price exchange_timestamp|order_timestamp",
            documented: "instrument_token order_type exchange_order_id order_ref_id \
                order_timestamp exchange_timestamp",
        },
        Endpoint::Gtts => Shape {
            required: "",
            documented: "",
        },
    }
}

impl UpstoxClient {
    /// Use the session saved by `trade login --broker upstox`.
    pub fn connect() -> Result<Self> {
        let config = app::session(KIND, NEEDS)?;
        let mut c = Self::with_urls(config.access_token, API, HFT, INSTRUMENTS)?;
        c.masters = MasterCache::shared();
        Ok(c)
    }

    fn with_urls(token: Secret, api: &str, hft: &str, instruments: &str) -> Result<Self> {
        Ok(Self {
            http: http::client()?,
            token,
            api: api.into(),
            hft: hft.into(),
            instruments: instruments.into(),
            masters: MasterCache::off(),
        })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let mut v =
            reqwest::header::HeaderValue::from_str(&format!("Bearer {}", self.token.expose()))
                .context("The saved Upstox token has invalid characters; log in again")?;
        v.set_sensitive(true);
        Ok(req
            .header(reqwest::header::AUTHORIZATION, v)
            .header("Accept", "application/json"))
    }

    /// GET a `{status, data}` endpoint (or a bare array) and return `data`.
    async fn get_data(&self, path: &str, what: &str) -> Result<Value> {
        let resp = self
            .auth(self.http.get(format!("{}{}", self.api, path)))?
            .send()
            .await
            .with_context(|| format!("Could not reach Upstox for the {what}"))?;
        let (status, text) = http::body(resp).await?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = http::parse(status, &text, LABEL, what)?;
        if v.is_array() {
            return Ok(v);
        }
        if v.get("status").and_then(Value::as_str) != Some("success") {
            bail!("Upstox {what} failed: {}", error_message(&v));
        }
        Ok(v.get("data").cloned().unwrap_or(Value::Null))
    }

    async fn list(&self, path: &str, what: &str) -> Result<Vec<Value>> {
        Ok(match self.get_data(path, what).await? {
            Value::Array(a) => a,
            Value::Null => vec![],
            other => bail!(
                "Unexpected Upstox {what} response: {}",
                http::excerpt(&other.to_string())
            ),
        })
    }
}

#[async_trait]
impl Broker for UpstoxClient {
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

    /// Today's fills (`get-trades-for-day`).
    async fn trades(&self, _since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
        let raw = self.rows(Endpoint::Trades).await?;
        Ok(raw.iter().filter_map(trade).collect())
    }

    async fn rows(&self, endpoint: Endpoint) -> Result<Vec<Value>> {
        let what = endpoint.label();
        match endpoint {
            Endpoint::Profile => Ok(vec![self.get_data("/v2/user/profile", what).await?]),
            Endpoint::Holdings => self.list("/v2/portfolio/long-term-holdings", what).await,
            Endpoint::Positions => self.list("/v2/portfolio/short-term-positions", what).await,
            Endpoint::Orders => self.list("/v2/order/retrieve-all", what).await,
            Endpoint::Trades => self.list("/v2/order/trades/get-trades-for-day", what).await,
            Endpoint::Gtts => bail!("Upstox GTT orders are not supported yet"),
        }
    }

    fn shape(&self, endpoint: Endpoint) -> Option<Shape> {
        (endpoint != Endpoint::Gtts).then(|| shape(endpoint))
    }

    async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
        if exchange != "NSE" {
            bail!("Upstox orders support NSE only");
        }
        let fetch = || async {
            let resp = self
                .http
                .get(&self.instruments)
                .send()
                .await
                .context("Could not download the Upstox instrument list")?;
            if !resp.status().is_success() {
                bail!("Upstox instrument list: HTTP {}", resp.status());
            }
            Ok(resp.bytes().await?.to_vec())
        };
        let rows = self
            .masters
            .load(KIND.id(), "NSE", fetch, parse_instruments)
            .await?;
        pick_equity(&rows, symbol)
    }

    async fn place_order(&self, o: &OrderRequest) -> Result<OrderAck> {
        let resp = self
            .auth(self.http.post(format!("{}/v3/order/place", self.hft)))?
            .json(&order_body(o))
            .send()
            .await
            .with_context(|| format!("Could not reach Upstox. {}", unknown_state(LABEL)))?;
        let (status, text) = http::body(resp)
            .await
            .with_context(|| unknown_state(LABEL))?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "Unreadable Upstox response (HTTP {status}). {}",
                unknown_state(LABEL)
            )
        })?;
        if v.get("status").and_then(Value::as_str) != Some("success") {
            if status >= 500 {
                bail!(
                    "Upstox error (HTTP {status}): {}. {}",
                    error_message(&v),
                    unknown_state(LABEL)
                );
            }
            bail!("Order rejected by Upstox: {}", error_message(&v));
        }
        let data = v.get("data");
        let id = data
            .and_then(|d| d.get("order_ids"))
            .and_then(Value::as_array)
            .and_then(|ids| ids.first())
            .and_then(|id| http::text(Some(id)))
            .or_else(|| http::text(data.and_then(|d| d.get("order_id"))));
        Ok(OrderAck { order_id: id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::Side;
    use crate::brokers::mock::{MockServer, Route};

    fn gz(json: &str) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        e.write_all(json.as_bytes()).unwrap();
        e.finish().unwrap()
    }

    const INSTRUMENTS_JSON: &str = include_str!("fixtures/upstox_instruments.json");

    async fn client(routes: Vec<Route>) -> (MockServer, UpstoxClient) {
        let server = MockServer::start(routes).await;
        let url = server.url();
        let c = UpstoxClient::with_urls(
            Secret::new("up-token"),
            &url,
            &url,
            &format!("{url}/instruments/NSE.json.gz"),
        )
        .unwrap();
        (server, c)
    }

    #[tokio::test]
    async fn holdings_parse_the_docs_fixture() {
        let (server, c) = client(vec![Route::json(
            "GET",
            "/v2/portfolio/long-term-holdings",
            include_str!("fixtures/upstox_holdings.json"),
        )])
        .await;
        let h = c.holdings().await.unwrap();
        assert_eq!(h.len(), 2);
        assert_eq!(h[0].symbol, "YESBANK");
        assert_eq!(h[0].qty, 36.0);
        assert_eq!(h[0].avg_price, Some(18.75));
        assert_eq!(h[0].isin.as_deref(), Some("INE528G01035"));
        let req = server.last("GET", "/v2/portfolio/long-term-holdings");
        assert_eq!(req.header("Authorization"), Some("Bearer up-token"));
    }

    #[tokio::test]
    async fn order_book_accepts_envelope_and_bare_array() {
        let fixture = include_str!("fixtures/upstox_orders.json");
        let wrapped = format!(r#"{{"status":"success","data":{fixture}}}"#);
        for body in [fixture.to_string(), wrapped] {
            let (_s, c) = client(vec![Route::json("GET", "/v2/order/retrieve-all", &body)]).await;
            let orders = c.orders().await.unwrap();
            assert_eq!(orders.len(), 2);
            assert_eq!(orders[1].status.as_deref(), Some("rejected"));
            assert_eq!(orders[1].trigger, Some(570.0));
            assert!(orders[1].message.as_deref().unwrap().contains("138385"));
        }
    }

    #[tokio::test]
    async fn expired_session_says_log_in_again() {
        let (_s, c) = client(vec![
            Route::json(
                "GET",
                "/v2/portfolio/long-term-holdings",
                r#"{"status":"error","errors":[{"error_code":"UDAPI100050","message":"Invalid token used to access API"}]}"#,
            )
            .status(401),
        ])
        .await;
        let err = c.holdings().await.unwrap_err().to_string();
        assert!(err.contains("trade login --broker upstox"), "{err}");
    }

    #[tokio::test]
    async fn resolves_only_the_exact_eq_instrument() {
        let (_s, c) = client(vec![Route::bytes(
            "GET",
            "/instruments/NSE.json.gz",
            gz(INSTRUMENTS_JSON),
            "application/gzip",
        )])
        .await;
        let inst = c.resolve_instrument("RELIANCE", "NSE").await.unwrap();
        assert_eq!(inst.token, "NSE_EQ|INE002A01018");
        // RELINFRA exists, "RELI" must not match it; a BE-series row is not EQ.
        assert!(c.resolve_instrument("RELI", "NSE").await.is_err());
        assert!(c.resolve_instrument("SUZLONBE", "NSE").await.is_err());
        // Index entries never resolve.
        assert!(c.resolve_instrument("Nifty 50", "NSE").await.is_err());
    }

    #[tokio::test]
    async fn stop_loss_market_order_body() {
        let (server, c) = client(vec![Route::json(
            "POST",
            "/v3/order/place",
            r#"{"status":"success","data":{"order_ids":["1644490272000"]},"metadata":{"latency":30}}"#,
        )])
        .await;
        let ack = c
            .place_order(&OrderRequest {
                instrument: Instrument {
                    symbol: "RELIANCE".into(),
                    tradingsymbol: "RELIANCE".into(),
                    token: "NSE_EQ|INE002A01018".into(),
                    exchange: "NSE".into(),
                },
                side: Side::Sell,
                qty: 3,
                kind: OrderKind::StopMarket { trigger: 1250.0 },
            })
            .await
            .unwrap();
        assert_eq!(ack.order_id.as_deref(), Some("1644490272000"));
        let body = server.last("POST", "/v3/order/place").json();
        assert_eq!(body["order_type"], "SL-M");
        assert_eq!(body["transaction_type"], "SELL");
        assert_eq!(body["trigger_price"], 1250.0);
        assert_eq!(body["price"], 0.0);
        assert_eq!(body["product"], "D");
        assert_eq!(body["market_protection"], -1);
        assert_eq!(body["instrument_token"], "NSE_EQ|INE002A01018");
    }

    #[tokio::test]
    async fn rejected_order_is_an_error_and_5xx_is_unknown() {
        let (_s, c) = client(vec![
            Route::json(
                "POST",
                "/v3/order/place",
                r#"{"status":"error","errors":[{"error_code":"UDAPI1154","message":"Order rejected: not from a registered static IP"}]}"#,
            )
            .status(400),
        ])
        .await;
        let req = OrderRequest {
            instrument: Instrument {
                symbol: "RELIANCE".into(),
                tradingsymbol: "RELIANCE".into(),
                token: "NSE_EQ|INE002A01018".into(),
                exchange: "NSE".into(),
            },
            side: Side::Buy,
            qty: 1,
            kind: OrderKind::Limit { price: 1300.0 },
        };
        let err = c.place_order(&req).await.unwrap_err().to_string();
        assert!(
            err.contains("static IP") && !err.contains("UNKNOWN"),
            "{err}"
        );

        let (_s, c) = client(vec![
            Route::json("POST", "/v3/order/place", "<html>bad gateway</html>").status(502),
        ])
        .await;
        let err = format!("{:#}", c.place_order(&req).await.unwrap_err());
        assert!(err.contains("UNKNOWN"), "{err}");
    }

    #[tokio::test]
    async fn token_exchange_reads_the_flat_response() {
        let server = MockServer::start(vec![Route::json(
            "POST",
            "/v2/login/authorization/token",
            include_str!("fixtures/upstox_token.json"),
        )])
        .await;
        let config = AppConfig {
            api_key: "key-1".into(),
            api_secret: Secret::new("sec-1"),
            redirect_uri: "http://127.0.0.1/".into(),
            ..Default::default()
        };
        let (token, user) = exchange_code(&server.url(), &config, "mk404x")
            .await
            .unwrap();
        assert_eq!(token.expose(), "eyJ0eXAiOiJKV1Qi.test");
        assert_eq!(user.as_deref(), Some("AB1234"));
        let req = server.last("POST", "/v2/login/authorization/token");
        assert_eq!(req.form("code").as_deref(), Some("mk404x"));
        assert_eq!(req.form("client_secret").as_deref(), Some("sec-1"));
        assert_eq!(
            req.form("redirect_uri").as_deref(),
            Some("http://127.0.0.1/")
        );
        assert_eq!(
            req.form("grant_type").as_deref(),
            Some("authorization_code")
        );
    }

    #[tokio::test]
    async fn gtt_is_not_supported() {
        let (_s, c) = client(vec![]).await;
        assert!(c.gtt_list().await.is_err());
    }

    fn read_routes() -> Vec<Route> {
        vec![
            Route::json(
                "GET",
                "/v2/user/profile",
                include_str!("fixtures/upstox_profile.json"),
            ),
            Route::json(
                "GET",
                "/v2/portfolio/long-term-holdings",
                include_str!("fixtures/upstox_holdings.json"),
            ),
            Route::json(
                "GET",
                "/v2/order/retrieve-all",
                include_str!("fixtures/upstox_orders.json"),
            ),
            Route::json(
                "GET",
                "/v2/order/trades/get-trades-for-day",
                include_str!("fixtures/upstox_trades.json"),
            ),
        ]
    }

    #[tokio::test]
    async fn trade_book_and_profile_parse() {
        let (_s, c) = client(read_routes()).await;
        let t = c.trades(None).await.unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].symbol, "GMRINFRA");
        assert_eq!((t[0].qty, t[0].price), (100.0, Some(82.35)));
        assert_eq!(t[0].trade_id.as_deref(), Some("50091502"));
        assert_eq!(t[0].date, chrono::NaiveDate::from_ymd_opt(2026, 10, 20));
        assert_eq!(t[0].time.as_deref(), Some("09:15:42"));
        assert!(t[0].is_delivery_equity());
        assert!(!t[1].is_delivery_equity());
        let p = c.profile().await.unwrap();
        assert_eq!(p.user_id.as_deref(), Some("TEST42"));
        assert!(c.rows(Endpoint::Gtts).await.is_err());
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
            ],
        )
        .await;
    }
}
