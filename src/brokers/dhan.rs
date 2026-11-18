//! Dhan API v2 — **experimental, untested against a live account.**
//!
//! Built from dhanhq.co/docs/v2 and its OpenAPI file: a pasted access token
//! (generated on web.dhan.co, valid 24 hours), holdings, positions, the
//! order book, orders (MARKET, LIMIT, STOP_LOSS, STOP_LOSS_MARKET; CNC),
//! exact instrument lookup in the NSE_EQ scrip master, and single Forever
//! Orders (Dhan's GTT).

use super::app::{self, Needs};
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

pub const CONFIG_FILE: &str = "dhan_config.json";
pub const SECRET_KEYS: &[&str] = app::SECRET_KEYS;
const KIND: BrokerKind = BrokerKind::Dhan;
const LABEL: &str = "Dhan";
const NEEDS: Needs = Needs {
    api_secret: false,
    redirect_uri: false,
};
const API: &str = "https://api.dhan.co/v2";
/// Segment-wise scrip master (redirects to a CSV download).
const INSTRUMENTS: &str = "https://api.dhan.co/v2/instrument/NSE_EQ";

pub const SETUP: &[&str] = &[
    "On web.dhan.co open My Profile → Access DhanHQ APIs and generate an access token (valid 24 hours)",
    "Dhan only accepts orders from the static IP set on that page; reads work from anywhere",
    "Put your Dhan client id as api_key in the file `trade config --broker dhan` creates",
    "Paste each new token with `trade login --broker dhan` (stored in the OS keyring when available)",
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

/// Paste a token generated on web.dhan.co; it is checked against the
/// account's profile before it is saved.
pub async fn login() -> Result<()> {
    let (config, storage) = app::for_login(KIND, NEEDS)?;
    println!();
    println!("  Generate an access token on web.dhan.co → My Profile → Access DhanHQ APIs.");
    let token = Secret::new(http::prompt_secret(
        "  Paste the access token (input hidden): ",
    )?);
    if !token.is_real() {
        bail!("No token pasted");
    }
    let client = DhanClient::with_urls(config.api_key.clone(), token.clone(), API, INSTRUMENTS)?;
    let who = client.profile_client_id().await?;
    if who.as_deref().is_some_and(|id| id != config.api_key) {
        bail!(
            "That token belongs to Dhan client {}, but {} is configured for {}",
            who.unwrap_or_default(),
            CONFIG_FILE,
            config.api_key
        );
    }
    app::save_session(KIND, config, &storage, token, who)
}

pub struct DhanClient {
    http: reqwest::Client,
    client_id: String,
    token: Secret,
    api: String,
    instruments: String,
    masters: Arc<MasterCache>,
}

/// `{errorType, errorCode, errorMessage}` → one line.
fn error_message(v: &Value, text: &str) -> String {
    let msg = http::text(v.get("errorMessage"));
    let code = http::text(v.get("errorCode"));
    match (msg, code) {
        (Some(m), Some(c)) => format!("{m} ({c})"),
        (Some(m), None) => m,
        (None, Some(c)) => c,
        (None, None) => http::excerpt(text),
    }
}

fn is_auth_error(v: &Value) -> bool {
    http::text(v.get("errorCode")).as_deref() == Some("DH-901")
}

/// Column names in the detailed and the compact scrip master layouts.
const COLUMNS: [[&str; 2]; 6] = [
    ["EXCH_ID", "SEM_EXM_EXCH_ID"],
    ["SEGMENT", "SEM_SEGMENT"],
    ["SECURITY_ID", "SEM_SMST_SECURITY_ID"],
    ["INSTRUMENT", "SEM_INSTRUMENT_NAME"],
    ["UNDERLYING_SYMBOL", "SEM_TRADING_SYMBOL"],
    ["SERIES", "SEM_SERIES"],
];

/// The scrip master as text, refused unless it has (in either layout) the
/// columns [`pick_equity`] reads.
fn parse_master(bytes: &[u8]) -> Result<String> {
    masters::csv_checked(bytes, "Dhan scrip master", |h| {
        COLUMNS
            .iter()
            .all(|names| names.iter().any(|n| h.iter().any(|f| f == n)))
    })
}

/// The exact NSE cash-equity (`EQ` series) row for `symbol` in the scrip
/// master, by column name. Accepts the detailed / segment-wise layout
/// (symbol in `UNDERLYING_SYMBOL`) and the compact one (`SEM_*`).
fn pick_equity(csv: &str, symbol: &str) -> Result<Instrument> {
    let mut lines = csv.lines();
    let header: Vec<String> = http::csv_fields(lines.next().unwrap_or_default())
        .into_iter()
        .map(|h| h.trim().to_string())
        .collect();
    let find = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| header.iter().position(|h| h == n))
    };
    let need = |names: &[&str]| {
        find(names).with_context(|| format!("Dhan scrip master has no {} column", names[0]))
    };
    let exch = need(&["EXCH_ID", "SEM_EXM_EXCH_ID"])?;
    let seg = need(&["SEGMENT", "SEM_SEGMENT"])?;
    let id = need(&["SECURITY_ID", "SEM_SMST_SECURITY_ID"])?;
    let inst = need(&["INSTRUMENT", "SEM_INSTRUMENT_NAME"])?;
    let sym = need(&["UNDERLYING_SYMBOL", "SEM_TRADING_SYMBOL"])?;
    let series = need(&["SERIES", "SEM_SERIES"])?;

    let mut hits = lines.map(http::csv_fields).filter(|f| {
        let get = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
        get(exch) == "NSE"
            && get(seg) == "E"
            && get(inst) == "EQUITY"
            && get(series) == "EQ"
            && get(sym) == symbol
    });
    let Some(hit) = hits.next() else {
        bail!(
            "No exact NSE equity match for {symbol} in the Dhan scrip master. Check the symbol with `stockwise search`."
        );
    };
    if hits.next().is_some() {
        bail!("Several Dhan instruments match {symbol} on NSE; refusing to guess.");
    }
    Ok(Instrument {
        symbol: symbol.to_string(),
        tradingsymbol: symbol.to_string(),
        token: hit[id].trim().to_string(),
        exchange: "NSE".into(),
    })
}

fn order_type(kind: &OrderKind) -> &'static str {
    match kind {
        OrderKind::Market => "MARKET",
        OrderKind::Limit { .. } => "LIMIT",
        OrderKind::StopLimit { .. } => "STOP_LOSS",
        OrderKind::StopMarket { .. } => "STOP_LOSS_MARKET",
    }
}

fn order_body(client_id: &str, o: &OrderRequest) -> Value {
    json!({
        "dhanClientId": client_id,
        "correlationId": "stockwise",
        "transactionType": o.side.as_str(),
        "exchangeSegment": "NSE_EQ",
        "productType": "CNC",
        "orderType": order_type(&o.kind),
        "validity": "DAY",
        "securityId": o.instrument.token,
        "quantity": o.qty,
        "disclosedQuantity": 0,
        "price": o.kind.price().unwrap_or(0.0),
        "triggerPrice": o.kind.trigger().unwrap_or(0.0),
        "afterMarketOrder": false,
    })
}

fn gtt_body(client_id: &str, g: &GttRequest) -> Value {
    json!({
        "dhanClientId": client_id,
        "correlationId": "stockwise",
        "orderFlag": "SINGLE",
        "transactionType": g.side.as_str(),
        "exchangeSegment": "NSE_EQ",
        "productType": "CNC",
        "orderType": "LIMIT",
        "validity": "DAY",
        "securityId": g.instrument.token,
        "quantity": g.qty,
        "disclosedQuantity": 0,
        "price": g.limit,
        "triggerPrice": g.trigger,
    })
}

fn holding(h: &Value) -> Option<Holding> {
    let sym = http::text(h.get("tradingSymbol"))?;
    Some(Holding {
        symbol: super::strip_series(&sym).to_string(),
        exchange: http::text(h.get("exchange")).filter(|e| e != "ALL"),
        isin: http::text(h.get("isin")),
        qty: http::num(h.get("totalQty")).unwrap_or(0.0),
        avg_price: http::price(h.get("avgCostPrice")),
        ltp: http::price(h.get("lastTradedPrice")),
        pnl: None,
    })
}

fn position(p: &Value) -> Option<Position> {
    let pnl = match (
        http::num(p.get("realizedProfit")),
        http::num(p.get("unrealizedProfit")),
    ) {
        (None, None) => None,
        (r, u) => Some(r.unwrap_or(0.0) + u.unwrap_or(0.0)),
    };
    Some(Position {
        symbol: http::text(p.get("tradingSymbol"))?,
        exchange: http::text(p.get("exchangeSegment")),
        product: http::text(p.get("productType")),
        qty: http::num(p.get("netQty")).unwrap_or(0.0),
        avg_price: http::price(p.get("costPrice")).or_else(|| http::price(p.get("buyAvg"))),
        ltp: None,
        pnl,
    })
}

fn order(o: &Value) -> Option<Order> {
    Some(Order {
        id: http::text(o.get("orderId"))?,
        symbol: http::text(o.get("tradingSymbol"))
            .or_else(|| http::text(o.get("securityId")))
            .unwrap_or_default(),
        side: http::text(o.get("transactionType")),
        order_type: http::text(o.get("orderType")),
        qty: http::num(o.get("quantity")),
        price: http::price(o.get("price")),
        trigger: http::price(o.get("triggerPrice")),
        status: http::text(o.get("orderStatus")),
        message: http::text(o.get("omsErrorDescription")),
        filled_qty: http::num(o.get("filledQty")),
        avg_price: http::price(o.get("averageTradedPrice")),
        product: http::text(o.get("productType")),
    })
}

/// `NSE_EQ` → `NSE`, `BSE_EQ` → `BSE`; other segments stay as they are
/// (and are not cash equity).
fn plain_exchange(segment: Option<String>) -> Option<String> {
    segment.map(|s| match s.as_str() {
        "NSE_EQ" => "NSE".to_string(),
        "BSE_EQ" => "BSE".to_string(),
        _ => s,
    })
}

/// The NSE equity trading symbol for a Dhan security id, from the scrip
/// master. Used for trade-history rows, which carry a security id and a
/// display name (`customSymbol`, never a ticker) but no trading symbol.
fn symbol_for_id(csv: &str, security_id: &str) -> Option<String> {
    let mut lines = csv.lines();
    let header: Vec<String> = http::csv_fields(lines.next().unwrap_or_default())
        .into_iter()
        .map(|h| h.trim().to_string())
        .collect();
    let find = |names: &[&str]| {
        names
            .iter()
            .find_map(|n| header.iter().position(|h| h == n))
    };
    let exch = find(&["EXCH_ID", "SEM_EXM_EXCH_ID"])?;
    let seg = find(&["SEGMENT", "SEM_SEGMENT"])?;
    let id = find(&["SECURITY_ID", "SEM_SMST_SECURITY_ID"])?;
    let inst = find(&["INSTRUMENT", "SEM_INSTRUMENT_NAME"])?;
    let sym = find(&["UNDERLYING_SYMBOL", "SEM_TRADING_SYMBOL"])?;
    let mut hits = lines.map(http::csv_fields).filter(|f| {
        let get = |i: usize| f.get(i).map(|s| s.trim()).unwrap_or("");
        get(exch) == "NSE" && get(seg) == "E" && get(inst) == "EQUITY" && get(id) == security_id
    });
    let hit = hits.next()?;
    if hits.next().is_some() {
        return None;
    }
    Some(hit.get(sym)?.trim().to_string()).filter(|s| !s.is_empty())
}

/// A trade row without a trading symbol that the scrip master can fill in.
fn needs_symbol(t: &Value) -> Option<String> {
    let missing = http::text(t.get("tradingSymbol")).is_none_or(|s| s.trim().is_empty());
    let nse = http::text(t.get("exchangeSegment")).as_deref() == Some("NSE_EQ");
    (missing && nse)
        .then(|| http::text(t.get("securityId")))
        .flatten()
}

fn trade(t: &Value) -> Option<Trade> {
    // Never `customSymbol`: it is a display name ("Tata Motors"), not a ticker.
    let sym = http::text(t.get("tradingSymbol")).unwrap_or_default();
    let stamp = http::text(
        t.get("exchangeTime")
            .filter(|v| !v.is_null())
            .or_else(|| t.get("createTime")),
    )
    .filter(|s| !s.starts_with("NA"));
    let (date, time) = super::split_timestamp(stamp.as_deref());
    Some(Trade {
        trade_id: http::text(t.get("exchangeTradeId")),
        order_id: http::text(t.get("orderId"))?,
        symbol: super::strip_series(&sym).to_string(),
        exchange: plain_exchange(http::text(t.get("exchangeSegment"))),
        product: http::text(t.get("productType")),
        side: super::side_from(http::text(t.get("transactionType")).as_deref()),
        qty: http::num(t.get("tradedQuantity")).unwrap_or(0.0),
        price: http::price(t.get("tradedPrice")),
        date,
        time,
    })
}

fn profile(p: &Value) -> Profile {
    let segments = http::text(p.get("activeSegment"))
        .map(|s| {
            s.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default();
    Profile {
        user_id: http::text(p.get("dhanClientId")),
        exchanges: segments,
        products: Vec::new(),
    }
}

/// Fields read from each endpoint, and the ones Dhan's docs list.
fn shape(endpoint: Endpoint) -> Shape {
    match endpoint {
        Endpoint::Profile => Shape {
            required: "dhanClientId",
            documented: "tokenValidity activeSegment ddpi mtf dataPlan dataValidity",
        },
        Endpoint::Holdings => Shape {
            required: "tradingSymbol totalQty avgCostPrice",
            documented: "exchange securityId isin dpQty t1Qty mtf_t1_qty mtf_qty availableQty \
                collateralQty lastTradedPrice",
        },
        Endpoint::Positions => Shape {
            required: "tradingSymbol netQty",
            documented: "dhanClientId securityId positionType exchangeSegment productType buyAvg \
                costPrice buyQty sellAvg sellQty realizedProfit unrealizedProfit rbiReferenceRate \
                multiplier carryForwardBuyQty carryForwardSellQty carryForwardBuyValue \
                carryForwardSellValue dayBuyQty daySellQty dayBuyValue daySellValue drvExpiryDate \
                drvOptionType drvStrikePrice crossCurrency",
        },
        Endpoint::Orders => Shape {
            required: "orderId orderStatus transactionType quantity",
            documented: "dhanClientId correlationId exchangeSegment productType orderType \
                validity tradingSymbol securityId disclosedQuantity price triggerPrice \
                afterMarketOrder boProfitValue boStopLossValue legName createTime updateTime \
                exchangeTime drvExpiryDate drvOptionType drvStrikePrice omsErrorCode \
                omsErrorDescription algoId remainingQuantity averageTradedPrice filledQty",
        },
        Endpoint::Trades => Shape {
            required: "orderId exchangeTradeId transactionType exchangeSegment productType \
                tradingSymbol tradedQuantity tradedPrice exchangeTime|createTime",
            documented: "dhanClientId exchangeOrderId orderType securityId isin instrument \
                sebiTax stt brokerageCharges serviceTax exchangeTransactionCharges stampDuty \
                createTime updateTime exchangeTime drvExpiryDate drvOptionType drvStrikePrice",
        },
        Endpoint::Gtts => Shape {
            required: "orderId orderStatus transactionType quantity price triggerPrice",
            documented: "dhanClientId exchangeSegment productType orderType tradingSymbol \
                securityId legName createTime updateTime exchangeTime drvExpiryDate drvOptionType \
                drvStrikePrice",
        },
    }
}

fn gtt(g: &Value) -> Option<Gtt> {
    Some(Gtt {
        id: http::text(g.get("orderId"))?,
        symbol: http::text(g.get("tradingSymbol")).unwrap_or_default(),
        side: http::text(g.get("transactionType")),
        qty: http::num(g.get("quantity")),
        trigger: http::price(g.get("triggerPrice")),
        limit: http::price(g.get("price")),
        status: http::text(g.get("orderStatus")),
    })
}

impl DhanClient {
    /// Use the token saved by `trade login --broker dhan`.
    pub fn connect() -> Result<Self> {
        let config = app::session(KIND, NEEDS)?;
        let mut c = Self::with_urls(config.api_key, config.access_token, API, INSTRUMENTS)?;
        c.masters = MasterCache::shared();
        Ok(c)
    }

    fn with_urls(client_id: String, token: Secret, api: &str, instruments: &str) -> Result<Self> {
        Ok(Self {
            http: http::client()?,
            client_id,
            token,
            api: api.into(),
            instruments: instruments.into(),
            masters: MasterCache::off(),
        })
    }

    fn auth(&self, req: reqwest::RequestBuilder) -> Result<reqwest::RequestBuilder> {
        let mut v = reqwest::header::HeaderValue::from_str(self.token.expose())
            .context("The saved Dhan token has invalid characters; log in again")?;
        v.set_sensitive(true);
        Ok(req
            .header("access-token", v)
            .header("client-id", &self.client_id)
            .header("Accept", "application/json"))
    }

    async fn get(&self, path: &str, what: &str) -> Result<Value> {
        let resp = self
            .auth(self.http.get(format!("{}{}", self.api, path)))?
            .send()
            .await
            .with_context(|| format!("Could not reach Dhan for the {what}"))?;
        let (status, text) = http::body(resp).await?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = http::parse(status, &text, LABEL, what)?;
        if is_auth_error(&v) {
            http::check_session(401, LABEL, KIND.id())?;
        }
        if status >= 400 || v.get("errorCode").is_some() {
            bail!("Dhan {what} failed: {}", error_message(&v, &text));
        }
        Ok(v)
    }

    async fn list(&self, path: &str, what: &str) -> Result<Vec<Value>> {
        Ok(match self.get(path, what).await? {
            Value::Array(a) => a,
            Value::Null => vec![],
            other => bail!(
                "Unexpected Dhan {what} response: {}",
                http::excerpt(&other.to_string())
            ),
        })
    }

    /// Today's fills, plus the history from `since` up to the day before
    /// `today`. History rows have no trading symbol; NSE ones get it from
    /// the scrip master by security id (others are left without one, and
    /// `trade sync` skips them).
    async fn trades_until(
        &self,
        since: Option<chrono::NaiveDate>,
        today: chrono::NaiveDate,
    ) -> Result<Vec<Trade>> {
        let mut raw = self.rows(Endpoint::Trades).await?;
        if let (Some(from), Some(to)) = (since, today.pred_opt())
            && from <= to
        {
            raw.extend(self.trade_history(from, to).await?);
        }
        if raw.iter().any(|t| needs_symbol(t).is_some())
            && let Ok(csv) = self.scrip_master().await
        {
            for t in raw.iter_mut() {
                if let Some(sym) = needs_symbol(t).and_then(|id| symbol_for_id(&csv, &id)) {
                    t["tradingSymbol"] = Value::String(sym);
                }
            }
        }
        Ok(raw.iter().filter_map(trade).collect())
    }

    /// Trades from `from` to `to` (inclusive), page by page (from 0) until
    /// an empty page. Refuses rather than stop short after 50 pages: a
    /// partial history would leave buys out.
    async fn trade_history(
        &self,
        from: chrono::NaiveDate,
        to: chrono::NaiveDate,
    ) -> Result<Vec<Value>> {
        const MAX_PAGES: u32 = 50;
        let mut all = Vec::new();
        for page in 0..MAX_PAGES {
            let path = format!("/trades/{from}/{to}/{page}");
            let rows = self.list(&path, "trade history").await?;
            if rows.is_empty() {
                return Ok(all);
            }
            all.extend(rows);
        }
        bail!(
            "Dhan's trade history from {from} has more than {MAX_PAGES} pages; \
             use a later --since"
        )
    }

    /// The NSE_EQ scrip master (CSV, gunzipped if needed), cached per IST day.
    async fn scrip_master(&self) -> Result<Arc<String>> {
        let fetch = || async {
            let resp = self
                .http
                .get(&self.instruments)
                .send()
                .await
                .context("Could not download the Dhan scrip master")?;
            if !resp.status().is_success() {
                bail!("Dhan scrip master: HTTP {}", resp.status());
            }
            Ok(resp.bytes().await?.to_vec())
        };
        self.masters
            .load(KIND.id(), "NSE", fetch, parse_master)
            .await
    }

    async fn profile_client_id(&self) -> Result<Option<String>> {
        let v = self.get("/profile", "token check").await?;
        Ok(http::text(v.get("dhanClientId")))
    }

    /// Send an order-changing request; transport and parse failures leave
    /// the state unknown and say so.
    async fn send_order(&self, req: reqwest::RequestBuilder, what: &str) -> Result<Value> {
        let resp = self
            .auth(req)?
            .send()
            .await
            .with_context(|| format!("Could not reach Dhan. {}", unknown_state(LABEL)))?;
        let (status, text) = http::body(resp)
            .await
            .with_context(|| unknown_state(LABEL))?;
        http::check_session(status, LABEL, KIND.id())?;
        let v: Value = serde_json::from_str(&text).with_context(|| {
            format!(
                "Unreadable Dhan response (HTTP {status}). {}",
                unknown_state(LABEL)
            )
        })?;
        if is_auth_error(&v) {
            http::check_session(401, LABEL, KIND.id())?;
        }
        if status >= 500 {
            bail!(
                "Dhan error (HTTP {status}): {}. {}",
                error_message(&v, &text),
                unknown_state(LABEL)
            );
        }
        if status >= 400 || v.get("errorCode").is_some() {
            bail!("Dhan rejected the {what}: {}", error_message(&v, &text));
        }
        if http::text(v.get("orderStatus")).as_deref() == Some("REJECTED") {
            bail!(
                "Dhan rejected the {what} (id {}). See `stockwise trade orders --broker dhan` for the reason.",
                http::text(v.get("orderId")).unwrap_or_default()
            );
        }
        Ok(v)
    }
}

#[async_trait]
impl Broker for DhanClient {
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

    /// Today's fills, plus Dhan's trade history from `since` when it is
    /// before today.
    async fn trades(&self, since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
        self.trades_until(since, super::ist_now().date_naive())
            .await
    }

    async fn rows(&self, endpoint: Endpoint) -> Result<Vec<Value>> {
        let what = endpoint.label();
        match endpoint {
            Endpoint::Profile => Ok(vec![self.get("/profile", what).await?]),
            Endpoint::Holdings => self.list("/holdings", what).await,
            Endpoint::Positions => self.list("/positions", what).await,
            Endpoint::Orders => self.list("/orders", what).await,
            Endpoint::Trades => self.list("/trades", what).await,
            Endpoint::Gtts => self.list("/forever/orders", what).await,
        }
    }

    fn shape(&self, endpoint: Endpoint) -> Option<Shape> {
        Some(shape(endpoint))
    }

    async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument> {
        if exchange != "NSE" {
            bail!("Dhan orders support NSE only");
        }
        pick_equity(&self.scrip_master().await?, symbol)
    }

    async fn place_order(&self, o: &OrderRequest) -> Result<OrderAck> {
        let req = self
            .http
            .post(format!("{}/orders", self.api))
            .json(&order_body(&self.client_id, o));
        let v = self.send_order(req, "order").await?;
        Ok(OrderAck {
            order_id: http::text(v.get("orderId")),
        })
    }

    fn supports_gtt(&self) -> bool {
        true
    }

    async fn gtt_create(&self, g: &GttRequest) -> Result<String> {
        let req = self
            .http
            .post(format!("{}/forever/orders", self.api))
            .json(&gtt_body(&self.client_id, g));
        let v = self.send_order(req, "Forever Order").await?;
        http::text(v.get("orderId")).context(
            "Dhan accepted the Forever Order but returned no id — check `stockwise trade gtt list --broker dhan`",
        )
    }

    async fn gtt_list(&self) -> Result<Vec<Gtt>> {
        let raw = self.rows(Endpoint::Gtts).await?;
        Ok(raw.iter().filter_map(gtt).collect())
    }

    async fn gtt_cancel(&self, id: &str) -> Result<()> {
        if !id.chars().all(|c| c.is_ascii_alphanumeric()) {
            bail!("Unexpected Dhan order id {id:?}");
        }
        let req = self
            .http
            .delete(format!("{}/forever/orders/{}", self.api, id));
        self.send_order(req, "Forever Order cancel").await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brokers::Side;
    use crate::brokers::mock::{MockServer, Route};

    async fn client(routes: Vec<Route>) -> (MockServer, DhanClient) {
        let server = MockServer::start(routes).await;
        let url = server.url();
        let c = DhanClient::with_urls(
            "1000000003".into(),
            Secret::new("dhan-jwt"),
            &url,
            &format!("{url}/instrument/NSE_EQ"),
        )
        .unwrap();
        (server, c)
    }

    fn hdfc() -> Instrument {
        Instrument {
            symbol: "HDFCBANK".into(),
            tradingsymbol: "HDFCBANK".into(),
            token: "1333".into(),
            exchange: "NSE".into(),
        }
    }

    #[tokio::test]
    async fn holdings_send_the_access_token_header() {
        let (server, c) = client(vec![Route::json(
            "GET",
            "/holdings",
            include_str!("fixtures/dhan_holdings.json"),
        )])
        .await;
        let h = c.holdings().await.unwrap();
        let req = server.last("GET", "/holdings");
        assert_eq!(req.header("access-token"), Some("dhan-jwt"));
        assert_eq!(h[0].symbol, "HDFC");
        assert_eq!(h[0].qty, 1000.0);
        assert_eq!(h[0].avg_price, Some(2655.0));
        assert_eq!(h[0].exchange, None);
        assert_eq!(h[1].ltp, Some(3560.0));
    }

    #[tokio::test]
    async fn positions_and_orders_parse() {
        let (_s, c) = client(vec![
            Route::json(
                "GET",
                "/positions",
                include_str!("fixtures/dhan_positions.json"),
            ),
            Route::json("GET", "/orders", include_str!("fixtures/dhan_orders.json")),
        ])
        .await;
        let p = c.positions().await.unwrap();
        assert_eq!((p[0].symbol.as_str(), p[0].qty), ("TCS", 40.0));
        assert_eq!(p[0].pnl, Some(6122.0));
        let o = c.orders().await.unwrap();
        assert_eq!(o[0].id, "112111182198");
        assert_eq!(o[0].status.as_deref(), Some("PENDING"));
        // Empty trading symbol falls back to the security id.
        assert_eq!(o[0].symbol, "11536");
    }

    #[tokio::test]
    async fn dh_901_means_log_in_again() {
        let (_s, c) = client(vec![
            Route::json(
                "GET",
                "/holdings",
                r#"{"errorType":"Invalid_Authentication","errorCode":"DH-901","errorMessage":"Client ID or user generated access token is invalid or expired."}"#,
            )
            .status(400),
        ])
        .await;
        let err = c.holdings().await.unwrap_err().to_string();
        assert!(err.contains("trade login --broker dhan"), "{err}");
    }

    #[test]
    fn scrip_master_lookup_is_exact_in_both_layouts() {
        let detailed = include_str!("fixtures/dhan_scrip_master.csv");
        assert_eq!(pick_equity(detailed, "HDFCBANK").unwrap(), hdfc());
        assert_eq!(pick_equity(detailed, "RELIANCE").unwrap().token, "2885");
        assert!(pick_equity(detailed, "HDFC").is_err());
        // A BE-series row and a state-loan row with the same symbol never match.
        assert!(pick_equity(detailed, "ZEEL").is_err());
        assert!(pick_equity(detailed, "656MH32").is_err());

        let compact = "SEM_EXM_EXCH_ID,SEM_SEGMENT,SEM_SMST_SECURITY_ID,SEM_INSTRUMENT_NAME,SEM_EXPIRY_CODE,SEM_TRADING_SYMBOL,SEM_LOT_UNITS,SEM_CUSTOM_SYMBOL,SEM_EXPIRY_DATE,SEM_STRIKE_PRICE,SEM_OPTION_TYPE,SEM_TICK_SIZE,SEM_EXPIRY_FLAG,SEM_EXCH_INSTRUMENT_TYPE,SEM_SERIES,SM_SYMBOL_NAME\n\
NSE,E,1333,EQUITY,0,HDFCBANK,1.0,HDFC Bank,,,,5.0000,NA,ES,EQ,HDFC BANK LTD\n\
BSE,E,500180,EQUITY,0,HDFCBANK,1.0,HDFC Bank,,,,5.0000,NA,ES,A,HDFC BANK LTD\n";
        assert_eq!(pick_equity(compact, "HDFCBANK").unwrap().token, "1333");
    }

    #[tokio::test]
    async fn stop_loss_market_order_body() {
        let (server, c) = client(vec![Route::json(
            "POST",
            "/orders",
            r#"{"orderId":"112111182198","orderStatus":"PENDING"}"#,
        )])
        .await;
        let ack = c
            .place_order(&OrderRequest {
                instrument: hdfc(),
                side: Side::Sell,
                qty: 10,
                kind: OrderKind::StopMarket { trigger: 1500.0 },
            })
            .await
            .unwrap();
        assert_eq!(ack.order_id.as_deref(), Some("112111182198"));
        let body = server.last("POST", "/orders").json();
        assert_eq!(body["orderType"], "STOP_LOSS_MARKET");
        assert_eq!(body["triggerPrice"], 1500.0);
        assert_eq!(body["price"], 0.0);
        assert_eq!(body["productType"], "CNC");
        assert_eq!(body["securityId"], "1333");
        assert_eq!(body["dhanClientId"], "1000000003");
        assert_eq!(body["quantity"], 10);
    }

    #[tokio::test]
    async fn rejected_status_is_an_error() {
        let (_s, c) = client(vec![Route::json(
            "POST",
            "/orders",
            r#"{"orderId":"9","orderStatus":"REJECTED"}"#,
        )])
        .await;
        let err = c
            .place_order(&OrderRequest {
                instrument: hdfc(),
                side: Side::Buy,
                qty: 1,
                kind: OrderKind::Limit { price: 1600.0 },
            })
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("rejected"), "{err}");
    }

    #[tokio::test]
    async fn forever_order_create_list_cancel() {
        let (server, c) = client(vec![
            Route::json(
                "POST",
                "/forever/orders",
                r#"{"orderId":"5132208051112","orderStatus":"PENDING"}"#,
            ),
            Route::json(
                "GET",
                "/forever/orders",
                include_str!("fixtures/dhan_forever_list.json"),
            ),
            Route::json(
                "DELETE",
                "/forever/orders/1132208051115",
                r#"{"orderId":"1132208051115","orderStatus":"CANCELLED"}"#,
            ),
        ])
        .await;
        let id = c
            .gtt_create(&GttRequest {
                instrument: hdfc(),
                side: Side::Buy,
                qty: 10,
                trigger: 1427.0,
                limit: 1428.0,
                last_price: 1500.0,
            })
            .await
            .unwrap();
        assert_eq!(id, "5132208051112");
        let body = server.last("POST", "/forever/orders").json();
        assert_eq!(body["orderFlag"], "SINGLE");
        assert_eq!(body["orderType"], "LIMIT");
        assert_eq!(body["price"], 1428.0);
        assert_eq!(body["triggerPrice"], 1427.0);

        let list = c.gtt_list().await.unwrap();
        assert_eq!(list[0].id, "1132208051115");
        assert_eq!(list[0].status.as_deref(), Some("CONFIRM"));
        c.gtt_cancel("1132208051115").await.unwrap();
        assert_eq!(server.count("DELETE", "/forever/orders/1132208051115"), 1);
    }

    fn read_routes() -> Vec<Route> {
        vec![
            Route::json(
                "GET",
                "/profile",
                include_str!("fixtures/dhan_profile.json"),
            ),
            Route::json(
                "GET",
                "/holdings",
                include_str!("fixtures/dhan_holdings.json"),
            ),
            Route::json(
                "GET",
                "/positions",
                include_str!("fixtures/dhan_positions.json"),
            ),
            Route::json("GET", "/orders", include_str!("fixtures/dhan_orders.json")),
            Route::json("GET", "/trades", include_str!("fixtures/dhan_trades.json")),
            Route::json(
                "GET",
                "/forever/orders",
                include_str!("fixtures/dhan_forever_list.json"),
            ),
            Route::json(
                "GET",
                "/trades/2026-10-15/2026-10-15/0",
                include_str!("fixtures/dhan_trade_history.json"),
            ),
            Route::json("GET", "/trades/2026-10-15/2026-10-15/1", "[]"),
            Route::bytes(
                "GET",
                "/instrument/NSE_EQ",
                include_str!("fixtures/dhan_scrip_master.csv")
                    .as_bytes()
                    .to_vec(),
                "text/csv",
            ),
        ]
    }

    #[tokio::test]
    async fn trade_book_maps_segments_and_profile_parses() {
        let (_s, c) = client(read_routes()).await;
        let t = c.trades(None).await.unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].symbol, "TCS");
        assert_eq!(t[0].exchange.as_deref(), Some("NSE"));
        assert_eq!(t[0].trade_id.as_deref(), Some("15112111182046"));
        assert_eq!((t[0].qty, t[0].price), (2.0, Some(3345.8)));
        assert_eq!(t[0].time.as_deref(), Some("11:20:06"));
        assert!(t[0].is_delivery_equity());
        assert_eq!(t[1].exchange.as_deref(), Some("NSE_FNO"));
        assert!(!t[1].is_delivery_equity());
        let p = c.profile().await.unwrap();
        assert_eq!(p.user_id.as_deref(), Some("1000000042"));
        assert!(p.exchanges.contains(&"Equity".to_string()));
    }

    #[tokio::test]
    async fn trade_history_maps_security_ids_never_display_names() {
        let (server, c) = client(read_routes()).await;
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let t = c
            .trades_until(Some(d("2026-10-15")), d("2026-10-16"))
            .await
            .unwrap();
        assert_eq!(server.count("GET", "/trades/2026-10-15/2026-10-15/1"), 1);
        assert_eq!(t.len(), 3);
        let h = &t[2];
        // tradingSymbol is null; customSymbol "HDFC Bank" must not become a ticker.
        assert_eq!(h.symbol, "HDFCBANK");
        assert_eq!(h.date, Some(d("2026-10-15")));
        assert!(h.is_delivery_equity());
        // Without the scrip master the row keeps no symbol (sync skips it).
        let raw: Vec<Value> =
            serde_json::from_str(include_str!("fixtures/dhan_trade_history.json")).unwrap();
        assert_eq!(trade(&raw[0]).unwrap().symbol, "");
        let csv = include_str!("fixtures/dhan_scrip_master.csv");
        assert_eq!(symbol_for_id(csv, "1333").as_deref(), Some("HDFCBANK"));
        assert_eq!(symbol_for_id(csv, "424242"), None);
    }

    #[tokio::test]
    async fn trade_history_refuses_to_stop_short() {
        let full = include_str!("fixtures/dhan_trade_history.json");
        let routes: Vec<Route> = (0..50)
            .map(|p| Route::json("GET", &format!("/trades/2026-10-01/2026-10-15/{p}"), full))
            .collect();
        let (_s, c) = client(routes).await;
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let err = c
            .trade_history(d("2026-10-01"), d("2026-10-15"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("more than 50 pages"), "{err}");
    }

    #[tokio::test]
    async fn fixtures_match_the_documented_shape() {
        let (_s, c) = client(read_routes()).await;
        crate::brokers::mock::assert_fixture_shapes(&c, &Endpoint::ALL).await;
    }

    #[test]
    fn a_master_without_the_lookup_columns_is_not_cached() {
        assert!(parse_master(b"{\"status\":\"error\",\"message\":\"Too many requests\",\"error_type\":\"NetworkException\"}").is_err());
        assert!(parse_master(include_str!("fixtures/dhan_scrip_master.csv").as_bytes()).is_ok());
        let compact = "SEM_EXM_EXCH_ID,SEM_SEGMENT,SEM_SMST_SECURITY_ID,SEM_INSTRUMENT_NAME,SEM_TRADING_SYMBOL,SEM_SERIES\n";
        assert!(parse_master(compact.as_bytes()).is_ok());
    }
}
