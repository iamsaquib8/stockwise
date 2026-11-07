//! Broker integrations behind one [`Broker`] trait.
//!
//! Angel One is the original integration. Zerodha Kite, Upstox, Dhan and
//! Fyers are **experimental**: built from each broker's public API docs and
//! tested against handwritten fixtures and a local mock server, never against
//! a live account.
//!
//! Every order — any broker, any order type, GTT included — goes through
//! `commands::trade::place_live_order` (preview, `max_order_value`, typed
//! `yes` / `--yes`, `--dry-run`). Nothing else calls [`Broker::place_order`]
//! or [`Broker::gtt_create`].

// `#[async_trait]` marks the boxed futures it generates `#[must_use]`, which
// newer clippy reports as `double_must_use` on every trait method.
#![allow(clippy::double_must_use)]

pub mod angel;
mod app;
pub mod dhan;
pub mod fyers;
pub mod gtt_watch;
mod http;
#[cfg(test)]
pub mod mock;
pub mod readonly;
pub mod secrets;
pub mod upstox;
pub mod zerodha;

use anyhow::{Result, bail};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// The brokers `trade --broker` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum BrokerKind {
    /// Angel One SmartAPI
    Angel,
    /// Zerodha Kite Connect (experimental)
    #[value(alias = "kite")]
    Zerodha,
    /// Upstox API v2 (experimental)
    Upstox,
    /// Dhan API v2 (experimental)
    Dhan,
    /// Fyers API v3 (experimental)
    Fyers,
}

impl BrokerKind {
    pub const ALL: [BrokerKind; 5] = [
        BrokerKind::Angel,
        BrokerKind::Zerodha,
        BrokerKind::Upstox,
        BrokerKind::Dhan,
        BrokerKind::Fyers,
    ];

    /// The `--broker` spelling.
    pub fn id(self) -> &'static str {
        match self {
            BrokerKind::Angel => "angel",
            BrokerKind::Zerodha => "zerodha",
            BrokerKind::Upstox => "upstox",
            BrokerKind::Dhan => "dhan",
            BrokerKind::Fyers => "fyers",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BrokerKind::Angel => "Angel One",
            BrokerKind::Zerodha => "Zerodha Kite",
            BrokerKind::Upstox => "Upstox",
            BrokerKind::Dhan => "Dhan",
            BrokerKind::Fyers => "Fyers",
        }
    }

    /// Built from public docs only — never run against a live account.
    pub fn experimental(self) -> bool {
        self != BrokerKind::Angel
    }

    /// Credentials file in the data directory.
    pub fn config_file(self) -> &'static str {
        match self {
            BrokerKind::Angel => angel::CONFIG_FILE,
            BrokerKind::Zerodha => zerodha::CONFIG_FILE,
            BrokerKind::Upstox => upstox::CONFIG_FILE,
            BrokerKind::Dhan => dhan::CONFIG_FILE,
            BrokerKind::Fyers => fyers::CONFIG_FILE,
        }
    }

    /// Config fields that are secrets (kept in the OS keyring when possible).
    pub fn secret_keys(self) -> &'static [&'static str] {
        match self {
            BrokerKind::Angel => angel::SECRET_KEYS,
            BrokerKind::Zerodha => zerodha::SECRET_KEYS,
            BrokerKind::Upstox => upstox::SECRET_KEYS,
            BrokerKind::Dhan => dhan::SECRET_KEYS,
            BrokerKind::Fyers => fyers::SECRET_KEYS,
        }
    }

    /// "Angel One" or "Zerodha Kite (experimental)".
    pub fn display(self) -> String {
        if self.experimental() {
            format!("{} (experimental)", self.label())
        } else {
            self.label().to_string()
        }
    }
}

/// The one-line warning shown wherever an experimental broker is used.
pub const EXPERIMENTAL_NOTE: &str = "experimental — untested against a live account";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Side {
    Buy,
    Sell,
}

impl Side {
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Buy => "BUY",
            Side::Sell => "SELL",
        }
    }
}

/// What kind of order to place. Prices are per share in the instrument's
/// currency (₹ for NSE).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OrderKind {
    Market,
    Limit {
        price: f64,
    },
    /// SL: once `trigger` trades, a limit order at `price` is placed.
    StopLimit {
        trigger: f64,
        price: f64,
    },
    /// SL-M: once `trigger` trades, a market order is placed.
    StopMarket {
        trigger: f64,
    },
}

impl OrderKind {
    /// Build from the `--limit` / `--trigger` flags.
    pub fn from_prices(limit: Option<f64>, trigger: Option<f64>) -> Self {
        match (limit, trigger) {
            (None, None) => OrderKind::Market,
            (Some(price), None) => OrderKind::Limit { price },
            (Some(price), Some(trigger)) => OrderKind::StopLimit { trigger, price },
            (None, Some(trigger)) => OrderKind::StopMarket { trigger },
        }
    }

    /// The conventional Indian order-type code.
    pub fn code(&self) -> &'static str {
        match self {
            OrderKind::Market => "MARKET",
            OrderKind::Limit { .. } => "LIMIT",
            OrderKind::StopLimit { .. } => "SL",
            OrderKind::StopMarket { .. } => "SL-M",
        }
    }

    pub fn price(&self) -> Option<f64> {
        match *self {
            OrderKind::Limit { price } | OrderKind::StopLimit { price, .. } => Some(price),
            _ => None,
        }
    }

    pub fn trigger(&self) -> Option<f64> {
        match *self {
            OrderKind::StopLimit { trigger, .. } | OrderKind::StopMarket { trigger } => {
                Some(trigger)
            }
            _ => None,
        }
    }

    pub fn is_stop(&self) -> bool {
        self.trigger().is_some()
    }
}

/// A broker instrument resolved from a user-typed symbol. Always an exact
/// match — never a fuzzy first hit.
#[derive(Debug, Clone, PartialEq)]
pub struct Instrument {
    /// Plain exchange symbol, e.g. `RELIANCE`.
    pub symbol: String,
    /// The broker's trading symbol, e.g. `RELIANCE-EQ` (Angel, Fyers) or
    /// `RELIANCE` (Kite).
    pub tradingsymbol: String,
    /// The broker's instrument id: Angel symbol token, Kite instrument token,
    /// Upstox instrument key, Dhan security id, Fyers symbol.
    pub token: String,
    pub exchange: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OrderRequest {
    pub instrument: Instrument,
    pub side: Side,
    pub qty: u32,
    pub kind: OrderKind,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct OrderAck {
    pub order_id: Option<String>,
}

/// A single-leg GTT: when `trigger` trades, a LIMIT order at `limit` is
/// placed for `qty` shares. Delivery (CNC) product.
#[derive(Debug, Clone, PartialEq)]
pub struct GttRequest {
    pub instrument: Instrument,
    pub side: Side,
    pub qty: u32,
    pub trigger: f64,
    pub limit: f64,
    /// Last traded price when the GTT was created (some brokers require it).
    pub last_price: f64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Gtt {
    pub id: String,
    pub symbol: String,
    pub side: Option<String>,
    pub qty: Option<f64>,
    pub trigger: Option<f64>,
    pub limit: Option<f64>,
    pub status: Option<String>,
}

/// A delivery holding. Missing prices are `None`, never 0.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Holding {
    /// Plain exchange symbol (series suffix removed), e.g. `RELIANCE`.
    pub symbol: String,
    pub exchange: Option<String>,
    pub isin: Option<String>,
    pub qty: f64,
    pub avg_price: Option<f64>,
    pub ltp: Option<f64>,
    pub pnl: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Position {
    pub symbol: String,
    pub exchange: Option<String>,
    pub product: Option<String>,
    /// Net quantity: negative for a short position.
    pub qty: f64,
    pub avg_price: Option<f64>,
    pub ltp: Option<f64>,
    pub pnl: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub symbol: String,
    pub side: Option<String>,
    pub order_type: Option<String>,
    pub qty: Option<f64>,
    pub price: Option<f64>,
    pub trigger: Option<f64>,
    pub status: Option<String>,
    pub message: Option<String>,
    /// Shares filled so far (partial fills show `filled_qty < qty`).
    #[serde(default)]
    pub filled_qty: Option<f64>,
    /// Average fill price.
    #[serde(default)]
    pub avg_price: Option<f64>,
    /// The broker's product code: CNC / DELIVERY / D, MIS / INTRADAY / I, ...
    #[serde(default)]
    pub product: Option<String>,
}

/// The account behind a session. `user_id` is never shown in full.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Profile {
    pub user_id: Option<String>,
    /// Exchanges or segments the account is enabled for.
    pub exchanges: Vec<String>,
    /// Product types the account may use.
    pub products: Vec<String>,
}

/// One execution (fill) from the broker's trade book. An order filled in
/// parts has one trade per part.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Trade {
    /// The exchange's or broker's id for this fill, when it has one.
    pub trade_id: Option<String>,
    pub order_id: String,
    /// Plain exchange symbol (series suffix removed), e.g. `RELIANCE`.
    pub symbol: String,
    /// `NSE` / `BSE` for cash equity; anything else (`NFO`, `NSE_FNO`,
    /// `MCX`, ...) is not a delivery holding.
    pub exchange: Option<String>,
    /// The broker's product code, as for [`Order::product`].
    pub product: Option<String>,
    pub side: Option<Side>,
    pub qty: f64,
    pub price: Option<f64>,
    /// Fill date. `None` when the broker gives a time only (Angel One's
    /// trade book is today's).
    pub date: Option<chrono::NaiveDate>,
    /// Fill time, `HH:MM:SS`, when given.
    pub time: Option<String>,
}

impl Trade {
    /// A delivery (CNC) fill in NSE/BSE cash equity: the only kind
    /// `trade sync` turns into lots. Intraday, margin (MTF) and F&O fills
    /// are left out.
    pub fn is_delivery_equity(&self) -> bool {
        let exchange_ok = matches!(self.exchange.as_deref(), Some("NSE" | "BSE"));
        let product_ok = self
            .product
            .as_deref()
            .is_some_and(|p| matches!(p.to_ascii_uppercase().as_str(), "CNC" | "DELIVERY" | "D"));
        exchange_ok && product_ok
    }
}

/// `BUY` / `SELL` (any case, or `B` / `S`) as a [`Side`].
pub fn side_from(text: Option<&str>) -> Option<Side> {
    match text?.trim().to_ascii_uppercase().as_str() {
        "BUY" | "B" => Some(Side::Buy),
        "SELL" | "S" => Some(Side::Sell),
        _ => None,
    }
}

/// Split a broker timestamp (`2024-03-15 09:30:00`, `15-Mar-2024 09:30:00`,
/// `2024-03-15T09:30:00`, or a bare `09:30:00`) into a date and a time.
pub fn split_timestamp(raw: Option<&str>) -> (Option<chrono::NaiveDate>, Option<String>) {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return (None, None);
    };
    let time = raw
        .split(['T', ' '])
        .find(|p| p.len() >= 5 && p.as_bytes().get(2) == Some(&b':'))
        .map(|t| t.chars().take(8).collect::<String>());
    let date = crate::portfolio::parse_trade_date(raw, false);
    (date, time)
}

/// A read-only endpoint, for `trade selftest` response-shape checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Endpoint {
    Profile,
    Holdings,
    Positions,
    Orders,
    Trades,
    Gtts,
}

impl Endpoint {
    pub const ALL: [Endpoint; 6] = [
        Endpoint::Profile,
        Endpoint::Holdings,
        Endpoint::Positions,
        Endpoint::Orders,
        Endpoint::Trades,
        Endpoint::Gtts,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Endpoint::Profile => "profile",
            Endpoint::Holdings => "holdings",
            Endpoint::Positions => "positions",
            Endpoint::Orders => "order book",
            Endpoint::Trades => "trade book",
            Endpoint::Gtts => "GTT list",
        }
    }
}

/// The fields StockWise expects in one row of an endpoint's response, as
/// space-separated names.
#[derive(Debug, Clone, Copy)]
pub struct Shape {
    /// Fields the parser reads; `a|b` means either. A missing one means
    /// StockWise will misread the response.
    pub required: &'static str,
    /// Other fields the broker's docs list. A field in neither list is
    /// reported as new — harmless, but worth knowing.
    pub documented: &'static str,
}

/// What a shape check found in the rows of one endpoint. Only field
/// names are kept — never values.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ShapeReport {
    pub rows: usize,
    /// Required fields absent from at least one row.
    pub missing: Vec<String>,
    /// Fields not in the documented list.
    pub unknown: Vec<String>,
    /// Rows that were not JSON objects.
    pub not_objects: usize,
}

/// Compare the field names of `rows` with `shape`. Names that don't look
/// like plain identifiers are counted, not echoed, so an odd response
/// can't smuggle a value into a report.
pub fn check_shape(rows: &[serde_json::Value], shape: &Shape) -> ShapeReport {
    let plain = |k: &str| {
        !k.is_empty()
            && k.len() <= 40
            && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
            && !k.starts_with(|c: char| c.is_ascii_digit())
    };
    let mut report = ShapeReport {
        rows: rows.len(),
        ..Default::default()
    };
    let mut odd = 0usize;
    for row in rows {
        let Some(obj) = row.as_object() else {
            report.not_objects += 1;
            continue;
        };
        for req in shape.required.split_whitespace() {
            if !req.split('|').any(|k| obj.contains_key(k))
                && !report.missing.iter().any(|m| m == req)
            {
                report.missing.push(req.to_string());
            }
        }
        for key in obj.keys() {
            let known = shape
                .documented
                .split_whitespace()
                .chain(shape.required.split_whitespace())
                .any(|d| d.split('|').any(|k| k == key));
            if known || report.unknown.iter().any(|u| u == key) {
                continue;
            }
            if plain(key) {
                report.unknown.push(key.clone());
            } else {
                odd += 1;
            }
        }
    }
    if odd > 0 {
        report
            .unknown
            .push(format!("({odd} unprintable field names)"));
    }
    report.missing.sort();
    report.unknown.sort();
    report
}

/// One logged-in broker session.
#[async_trait]
pub trait Broker: Send + Sync {
    fn kind(&self) -> BrokerKind;
    async fn holdings(&self) -> Result<Vec<Holding>>;
    async fn positions(&self) -> Result<Vec<Position>>;
    /// Today's order book.
    async fn orders(&self) -> Result<Vec<Order>>;
    /// The account's profile (a cheap call that proves the session works).
    async fn profile(&self) -> Result<Profile> {
        bail!("{} profile is not supported yet", self.kind().label())
    }
    /// Executed trades (fills). Most brokers return today's only; `since`
    /// asks for older ones where the broker keeps them (Dhan).
    async fn trades(&self, _since: Option<chrono::NaiveDate>) -> Result<Vec<Trade>> {
        bail!("{} trade book is not supported yet", self.kind().label())
    }
    /// The raw rows of a read-only endpoint, for shape checks. Never an
    /// order-changing request.
    async fn rows(&self, endpoint: Endpoint) -> Result<Vec<serde_json::Value>> {
        bail!(
            "{} raw {} is not supported yet",
            self.kind().label(),
            endpoint.label()
        )
    }
    /// The fields StockWise reads from `endpoint`, when known.
    fn shape(&self, _endpoint: Endpoint) -> Option<Shape> {
        None
    }
    /// The exact cash-equity instrument for `symbol` on `exchange` (`NSE`),
    /// or an error. Never a partial or first-of-many match.
    async fn resolve_instrument(&self, symbol: &str, exchange: &str) -> Result<Instrument>;
    /// Send one order. Only `place_live_order` may call this.
    async fn place_order(&self, order: &OrderRequest) -> Result<OrderAck>;
    /// Whether the GTT methods below are implemented for this broker.
    fn supports_gtt(&self) -> bool {
        false
    }
    /// Create a GTT; returns its id. Only `place_live_order` may call this.
    async fn gtt_create(&self, _gtt: &GttRequest) -> Result<String> {
        bail!("{} GTT orders are not supported yet", self.kind().label())
    }
    async fn gtt_list(&self) -> Result<Vec<Gtt>> {
        bail!("{} GTT orders are not supported yet", self.kind().label())
    }
    async fn gtt_cancel(&self, _id: &str) -> Result<()> {
        bail!("{} GTT orders are not supported yet", self.kind().label())
    }
}

/// Log in (Angel One) or load the saved session (the others) for `kind`.
pub async fn connect(kind: BrokerKind) -> Result<Box<dyn Broker>> {
    Ok(match kind {
        BrokerKind::Angel => Box::new(angel::AngelClient::new().await?),
        BrokerKind::Zerodha => Box::new(zerodha::KiteClient::connect()?),
        BrokerKind::Upstox => Box::new(upstox::UpstoxClient::connect()?),
        BrokerKind::Dhan => Box::new(dhan::DhanClient::connect()?),
        BrokerKind::Fyers => Box::new(fyers::FyersClient::connect()?),
    })
}

/// Interactive login for brokers whose sessions expire daily.
pub async fn login(kind: BrokerKind) -> Result<()> {
    match kind {
        BrokerKind::Angel => {
            // Angel One logs in with password + TOTP on every command.
            angel::AngelClient::new().await?;
            println!(
                "\n  Angel One logs in automatically on every command (PIN + TOTP); nothing to save.\n"
            );
            Ok(())
        }
        BrokerKind::Zerodha => zerodha::login().await,
        BrokerKind::Upstox => upstox::login().await,
        BrokerKind::Dhan => dhan::login().await,
        BrokerKind::Fyers => fyers::login().await,
    }
}

/// What `trade config` and `doctor` show about a broker's credentials.
#[derive(Debug, Clone)]
pub struct ConfigStatus {
    pub path: std::path::PathBuf,
    pub exists: bool,
    /// All required credentials are filled in.
    pub configured: bool,
    /// Non-secret identity, e.g. "client A123" or "api key kitefront".
    pub summary: String,
    pub storage: secrets::Storage,
    /// For brokers with daily sessions: when `trade login` last succeeded.
    pub session: Option<String>,
    /// The IST date of that login.
    pub session_date: Option<chrono::NaiveDate>,
    /// Account and app ids (not secrets, but personal): `trade selftest`
    /// scrubs them from anything it prints.
    pub identifiers: Vec<String>,
}

/// Read a broker's config. With `migrate`, plaintext secrets move into the
/// OS keyring when one is available.
pub fn config_status(kind: BrokerKind, migrate: bool) -> Result<ConfigStatus> {
    match kind {
        BrokerKind::Angel => angel::status(migrate),
        BrokerKind::Zerodha => zerodha::status(migrate),
        BrokerKind::Upstox => upstox::status(migrate),
        BrokerKind::Dhan => dhan::status(migrate),
        BrokerKind::Fyers => fyers::status(migrate),
    }
}

/// Write a credentials template with placeholder values (owner-only).
pub fn write_template(kind: BrokerKind) -> Result<()> {
    match kind {
        BrokerKind::Angel => angel::AngelConfig::template().save(),
        BrokerKind::Zerodha => zerodha::write_template(),
        BrokerKind::Upstox => upstox::write_template(),
        BrokerKind::Dhan => dhan::write_template(),
        BrokerKind::Fyers => fyers::write_template(),
    }
}

/// Numbered setup steps for `trade setup`.
pub fn setup_steps(kind: BrokerKind) -> &'static [&'static str] {
    match kind {
        BrokerKind::Angel => angel::SETUP,
        BrokerKind::Zerodha => zerodha::SETUP,
        BrokerKind::Upstox => upstox::SETUP,
        BrokerKind::Dhan => dhan::SETUP,
        BrokerKind::Fyers => fyers::SETUP,
    }
}

/// Today's date in India (sessions of Indian brokers roll over in IST).
pub fn ist_now() -> chrono::DateTime<chrono::FixedOffset> {
    let ist = chrono::FixedOffset::east_opt(5 * 3600 + 30 * 60).expect("valid offset");
    chrono::Utc::now().with_timezone(&ist)
}

/// Build a [`ConfigStatus`] from a loaded config.
fn status_of(
    kind: BrokerKind,
    configured: bool,
    summary: String,
    storage: secrets::Storage,
    session: Option<String>,
) -> Result<ConfigStatus> {
    let path = crate::store::path(kind.config_file())?;
    Ok(ConfigStatus {
        exists: path.exists(),
        path,
        configured,
        summary,
        storage,
        session,
        session_date: None,
        identifiers: Vec::new(),
    })
}

/// Error for a broker that has no saved credentials yet.
fn not_configured(kind: BrokerKind) -> anyhow::Error {
    let path = crate::store::path(kind.config_file())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| kind.config_file().to_string());
    anyhow::anyhow!(
        "{} is not configured. Run `stockwise trade setup --broker {}` for instructions, then edit {}",
        kind.label(),
        kind.id(),
        path
    )
}

/// Error for a broker whose daily session has not been created yet.
fn not_logged_in(kind: BrokerKind) -> anyhow::Error {
    anyhow::anyhow!(
        "No {} session. Log in with `stockwise trade login --broker {}` (sessions expire daily).",
        kind.label(),
        kind.id()
    )
}

/// Order guards shared by every broker, read from its config file.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct OrderLimits {
    /// Refuse any single order whose estimated value exceeds this (₹).
    pub max_order_value: Option<f64>,
    /// `"enable_orders": true` in the config file: the user opted in to
    /// order paths that were never run against a live account (every order
    /// on an experimental broker; stop-loss and GTT orders on Angel One).
    pub enable_orders: bool,
}

/// Fields every broker config file may carry, besides its credentials.
#[derive(Debug, Clone, Default, Deserialize)]
struct CommonSettings {
    #[serde(default)]
    max_order_value: Option<f64>,
    #[serde(default)]
    enable_orders: bool,
}

/// Read the order guards for `kind` without touching its secrets.
pub fn order_limits(kind: BrokerKind) -> Result<OrderLimits> {
    let path = crate::store::path(kind.config_file())?;
    let common: CommonSettings = crate::store::load_from(&path)?;
    Ok(OrderLimits {
        max_order_value: common.max_order_value,
        enable_orders: common.enable_orders,
    })
}

// ── Order validation (pure; tested below) ──

fn check_price(name: &str, v: f64) -> Result<()> {
    if !(v.is_finite() && v > 0.0) {
        bail!("{name} must be a positive number");
    }
    Ok(())
}

/// A limit more than this far from its trigger is treated as a typo.
const MAX_LIMIT_TRIGGER_GAP: f64 = 0.20;

fn check_limit_near_trigger(limit: f64, trigger: f64) -> Result<()> {
    let gap = (limit - trigger).abs() / trigger;
    if gap > MAX_LIMIT_TRIGGER_GAP {
        bail!(
            "Limit ₹{limit:.2} is {:.0}% away from the trigger ₹{trigger:.2} — check for a typo \
             (StockWise refuses gaps above {:.0}%)",
            gap * 100.0,
            MAX_LIMIT_TRIGGER_GAP * 100.0
        );
    }
    Ok(())
}

/// Check prices and, for stop-loss orders, the trigger against the limit,
/// the side and the last traded price. Stop orders need a last price: a
/// trigger on the wrong side of the market would fire immediately.
pub fn validate_order(side: Side, kind: &OrderKind, last: Option<f64>) -> Result<()> {
    if let Some(p) = kind.price() {
        check_price("Limit price", p)?;
    }
    let Some(trigger) = kind.trigger() else {
        return Ok(());
    };
    check_price("Trigger price", trigger)?;
    if let Some(price) = kind.price() {
        match side {
            Side::Buy if price < trigger => bail!(
                "For a stop-loss BUY the limit (₹{price:.2}) must be at or above the trigger (₹{trigger:.2})"
            ),
            Side::Sell if price > trigger => bail!(
                "For a stop-loss SELL the limit (₹{price:.2}) must be at or below the trigger (₹{trigger:.2})"
            ),
            _ => {}
        }
        check_limit_near_trigger(price, trigger)?;
    }
    let Some(last) = last else {
        bail!(
            "Cannot fetch the last price, so the trigger can't be checked against the market. \
             Try again in a moment."
        );
    };
    match side {
        Side::Buy if trigger <= last => bail!(
            "A stop-loss BUY fires when the price rises to the trigger, so the trigger (₹{trigger:.2}) \
             must be above the last price (₹{last:.2}). To buy at or below the market, use --limit."
        ),
        Side::Sell if trigger >= last => bail!(
            "A stop-loss SELL fires when the price falls to the trigger, so the trigger (₹{trigger:.2}) \
             must be below the last price (₹{last:.2}). To sell at or above the market, use --limit."
        ),
        _ => Ok(()),
    }
}

/// Check a single-leg GTT. Either direction is valid (a BUY above the market
/// is a breakout entry, below it a dip buy; a SELL above is a target, below
/// a stop-loss), but the trigger must not sit on the last price and the
/// limit must be near the trigger. A stop GTT (SELL below the market, BUY
/// above it) fires into a moving price, so its limit must be marketable at
/// the trigger: at or below it for a SELL, at or above it for a BUY —
/// otherwise the order could rest unfilled and leave the position open.
pub fn validate_gtt(side: Side, trigger: f64, limit: f64, last: Option<f64>) -> Result<()> {
    check_price("Trigger price", trigger)?;
    check_price("Limit price", limit)?;
    check_limit_near_trigger(limit, trigger)?;
    let Some(last) = last else {
        bail!("Cannot fetch the last price, which a GTT needs. Try again in a moment.");
    };
    if ((trigger - last) / last).abs() < 0.0025 {
        bail!(
            "The trigger (₹{trigger:.2}) is within 0.25% of the last price (₹{last:.2}); \
             it would fire immediately. Place a regular order instead."
        );
    }
    match side {
        Side::Sell if trigger < last && limit > trigger => bail!(
            "A stop-loss SELL GTT (trigger ₹{trigger:.2} below the market) needs its limit at or \
             below the trigger, or the sell may never fill; got ₹{limit:.2}"
        ),
        Side::Buy if trigger > last && limit < trigger => bail!(
            "A stop BUY GTT (trigger ₹{trigger:.2} above the market) needs its limit at or above \
             the trigger, or the buy may never fill; got ₹{limit:.2}"
        ),
        _ => Ok(()),
    }
}

/// Prices with more than 2 decimals: refused rather than silently rounded,
/// so what the preview shows is what is sent.
pub fn check_paise(name: &str, v: f64) -> Result<()> {
    let paise = v * 100.0;
    if (paise - paise.round()).abs() > 1e-6 {
        bail!("{name} ₹{v} has more than 2 decimals");
    }
    Ok(())
}

/// Estimated order value for the preview and the `max_order_value` cap.
/// Uses the limit price when there is one; SL-M uses the higher of trigger
/// and last price, since it fills at market after triggering.
pub fn estimate_value(kind: &OrderKind, qty: u32, last: Option<f64>) -> Option<f64> {
    let per_share = match *kind {
        // A zero, negative or non-finite last price is no estimate: a cap
        // must not pass a market order as worth nothing.
        OrderKind::Market => last.filter(|p| p.is_finite() && *p > 0.0)?,
        OrderKind::Limit { price } | OrderKind::StopLimit { price, .. } => price,
        OrderKind::StopMarket { trigger } => last
            .filter(|l| l.is_finite())
            .map_or(trigger, |l| l.max(trigger)),
    };
    Some(per_share * qty as f64)
}

/// Drop an NSE/BSE series suffix from a broker trading symbol:
/// `RELIANCE-EQ` / `CDSL-BE` → `RELIANCE` / `CDSL`. Hyphenated names such
/// as `BAJAJ-AUTO` are left alone.
pub fn strip_series(tradingsymbol: &str) -> &str {
    const SERIES: [&str; 9] = ["EQ", "BE", "BZ", "BL", "SM", "ST", "IL", "IQ", "RL"];
    match tradingsymbol.rsplit_once('-') {
        Some((base, series)) if !base.is_empty() && SERIES.contains(&series) => base,
        _ => tradingsymbol,
    }
}

/// `RELIANCE.NS` / `reliance-eq` / ` Reliance ` → `RELIANCE`. Other
/// suffixes (`.BO`) are kept.
pub fn plain_symbol(symbol: &str) -> String {
    let s = symbol.trim().to_uppercase();
    let s = s.strip_suffix(".NS").unwrap_or(&s);
    s.strip_suffix("-EQ").unwrap_or(s).to_string()
}

/// The NSE symbol to trade. Orders go to NSE only, so a BSE (`.BO`) or other
/// suffixed ticker is refused instead of quietly trading the NSE listing.
pub fn nse_symbol(symbol: &str) -> Result<String> {
    let s = plain_symbol(symbol);
    if s.is_empty() {
        bail!("Symbol required");
    }
    if s.contains('.') {
        bail!("StockWise places orders on NSE only; use the NSE symbol (e.g. RELIANCE), not {s}");
    }
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn order_kind_from_flags() {
        assert_eq!(OrderKind::from_prices(None, None), OrderKind::Market);
        assert_eq!(OrderKind::from_prices(Some(10.0), None).code(), "LIMIT");
        assert_eq!(OrderKind::from_prices(Some(10.0), Some(9.0)).code(), "SL");
        assert_eq!(OrderKind::from_prices(None, Some(9.0)).code(), "SL-M");
    }

    #[test]
    fn stop_loss_sell_needs_trigger_below_market_and_limit_below_trigger() {
        let ok = OrderKind::StopLimit {
            trigger: 95.0,
            price: 94.5,
        };
        assert!(validate_order(Side::Sell, &ok, Some(100.0)).is_ok());
        // Trigger above the market would fire at once.
        let above = OrderKind::StopMarket { trigger: 101.0 };
        assert!(validate_order(Side::Sell, &above, Some(100.0)).is_err());
        // Limit above the trigger is not a stop-loss sell.
        let inverted = OrderKind::StopLimit {
            trigger: 95.0,
            price: 96.0,
        };
        assert!(validate_order(Side::Sell, &inverted, Some(100.0)).is_err());
    }

    #[test]
    fn stop_loss_buy_needs_trigger_above_market_and_limit_above_trigger() {
        let ok = OrderKind::StopLimit {
            trigger: 105.0,
            price: 105.5,
        };
        assert!(validate_order(Side::Buy, &ok, Some(100.0)).is_ok());
        let below = OrderKind::StopMarket { trigger: 99.0 };
        assert!(validate_order(Side::Buy, &below, Some(100.0)).is_err());
        let inverted = OrderKind::StopLimit {
            trigger: 105.0,
            price: 104.0,
        };
        assert!(validate_order(Side::Buy, &inverted, Some(100.0)).is_err());
    }

    #[test]
    fn stop_orders_need_a_last_price() {
        let sl = OrderKind::StopMarket { trigger: 95.0 };
        assert!(validate_order(Side::Sell, &sl, None).is_err());
        // Plain limit/market orders don't.
        assert!(validate_order(Side::Buy, &OrderKind::Limit { price: 10.0 }, None).is_ok());
        assert!(validate_order(Side::Buy, &OrderKind::Market, None).is_ok());
    }

    #[test]
    fn typo_sized_gap_between_limit_and_trigger_is_refused() {
        let typo = OrderKind::StopLimit {
            trigger: 950.0,
            price: 95.0,
        };
        assert!(validate_order(Side::Sell, &typo, Some(1000.0)).is_err());
        assert!(validate_gtt(Side::Sell, 1200.0, 120.0, Some(1300.0)).is_err());
    }

    #[test]
    fn bad_numbers_are_refused() {
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert!(validate_order(Side::Buy, &OrderKind::Limit { price: bad }, None).is_err());
            assert!(validate_gtt(Side::Buy, bad, 100.0, Some(120.0)).is_err());
        }
    }

    #[test]
    fn gtt_trigger_must_not_sit_on_the_market() {
        assert!(validate_gtt(Side::Sell, 1200.0, 1195.0, Some(1300.0)).is_ok());
        assert!(validate_gtt(Side::Buy, 1400.0, 1405.0, Some(1300.0)).is_ok());
        assert!(validate_gtt(Side::Sell, 1300.5, 1300.0, Some(1300.0)).is_err());
        assert!(validate_gtt(Side::Sell, 1200.0, 1195.0, None).is_err());
    }

    #[test]
    fn stop_gtts_need_a_marketable_limit() {
        // Stop-loss SELL below the market with the limit above the trigger:
        // the sell could rest unfilled as the price falls.
        assert!(validate_gtt(Side::Sell, 1200.0, 1230.0, Some(1300.0)).is_err());
        // Stop BUY above the market with the limit below the trigger.
        assert!(validate_gtt(Side::Buy, 1400.0, 1390.0, Some(1300.0)).is_err());
        // Targets and dip buys may rest beyond the trigger (Kite's own examples do).
        assert!(validate_gtt(Side::Sell, 1400.0, 1405.0, Some(1300.0)).is_ok());
        assert!(validate_gtt(Side::Buy, 1200.0, 1195.0, Some(1300.0)).is_ok());
    }

    #[test]
    fn prices_with_more_than_two_decimals_are_refused() {
        assert!(check_paise("Limit price", 1300.45).is_ok());
        assert!(check_paise("Limit price", 0.05).is_ok());
        assert!(check_paise("Limit price", 1300.456).is_err());
    }

    #[test]
    fn value_estimates() {
        assert_eq!(
            estimate_value(&OrderKind::Market, 10, Some(100.0)),
            Some(1000.0)
        );
        assert_eq!(estimate_value(&OrderKind::Market, 10, None), None);
        // A halted stock (price 0), a negative or a NaN price is no estimate.
        for bad in [0.0, -5.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                estimate_value(&OrderKind::Market, 10, Some(bad)),
                None,
                "{bad}"
            );
        }
        assert_eq!(
            estimate_value(&OrderKind::Limit { price: 50.0 }, 2, None),
            Some(100.0)
        );
        // SL-M buy above market: trigger is the higher price.
        assert_eq!(
            estimate_value(&OrderKind::StopMarket { trigger: 110.0 }, 1, Some(100.0)),
            Some(110.0)
        );
        // SL-M sell below market: last price is the higher one.
        assert_eq!(
            estimate_value(&OrderKind::StopMarket { trigger: 90.0 }, 1, Some(100.0)),
            Some(100.0)
        );
    }

    #[test]
    fn strip_series_keeps_hyphenated_names() {
        assert_eq!(strip_series("RELIANCE-EQ"), "RELIANCE");
        assert_eq!(strip_series("CDSL-BE"), "CDSL");
        assert_eq!(strip_series("BAJAJ-AUTO"), "BAJAJ-AUTO");
        assert_eq!(strip_series("BAJAJ-AUTO-EQ"), "BAJAJ-AUTO");
        assert_eq!(strip_series("INFY"), "INFY");
    }

    #[test]
    fn plain_symbol_strips_suffixes() {
        assert_eq!(plain_symbol(" reliance.ns "), "RELIANCE");
        assert_eq!(plain_symbol("TCS-EQ"), "TCS");
        assert_eq!(plain_symbol("M&M"), "M&M");
        assert_eq!(plain_symbol("INFY.BO"), "INFY.BO");
        assert_eq!(nse_symbol("infy.ns").unwrap(), "INFY");
        assert!(nse_symbol("INFY.BO").is_err());
        assert!(nse_symbol(" ").is_err());
    }

    #[test]
    fn only_angel_is_not_experimental() {
        for k in BrokerKind::ALL {
            assert_eq!(k.experimental(), k != BrokerKind::Angel);
            assert!(k.config_file().ends_with("_config.json"));
        }
    }
}
