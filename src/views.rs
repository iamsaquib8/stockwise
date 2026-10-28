//! What the core commands show, gathered into plain structs.
//!
//! The CLI printers (`commands/`), the TUI and the HTTP server all build on
//! these functions, so `stockwise quote --json` and `GET /api/quote` return
//! the same shape. Nothing here prints. Market data and local files come
//! through [`DataSource`], which tests replace with fixtures.

use crate::alerts::{Alert, AlertStore, MarketData};
use crate::api::{ChartData, Quote, SearchResult, YahooClient};
use crate::daemon_status::DaemonStatus;
use crate::market::{self, Market};
use crate::portfolio::Portfolio;
use crate::watchlist::Watchlist;
use anyhow::{Result, bail};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;

/// Where market data and the local data files come from: Yahoo Finance and
/// the data directory in production, fixtures in tests.
pub trait DataSource: Send + Sync {
    fn quotes(&self, symbols: &[&str]) -> impl Future<Output = Result<Vec<Quote>>> + Send;
    fn chart(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> impl Future<Output = Result<ChartData>> + Send;
    fn search(&self, query: &str) -> impl Future<Output = Result<Vec<SearchResult>>> + Send;
    /// The next results date for `symbol` on or after `today`, for
    /// `results-in` alerts. None unless the source has a calendar.
    fn next_results(
        &self,
        _symbol: &str,
        _today: NaiveDate,
    ) -> impl Future<Output = Option<NaiveDate>> + Send {
        async { None }
    }
    /// The India 10-year G-sec benchmark yield for `markets`. None unless
    /// the source reads CCIL / RBI.
    fn india_10y(&self) -> impl Future<Output = Option<crate::gsec::YieldRow>> + Send {
        async { None }
    }
    /// AMFI's latest NAVs, to value `MF:` holdings. An error unless the
    /// source reads AMFI.
    fn fund_navs(
        &self,
    ) -> impl Future<Output = Result<std::sync::Arc<Vec<crate::funds::Scheme>>>> + Send {
        async { bail!("no mutual-fund NAVs in this data source") }
    }
    /// NSE's list of traded Sovereign Gold Bonds, to value SGB holdings. An
    /// error unless the source reads NSE.
    fn sgb_list(&self) -> impl Future<Output = Result<crate::commodity::SgbList>> + Send {
        async { bail!("no SGB prices in this data source") }
    }

    /// The Yahoo client behind this source, for data the methods above don't
    /// cover (the results calendar). Fixtures have none.
    fn yahoo(&self) -> impl Future<Output = Result<&YahooClient>> + Send {
        async { bail!("No Yahoo Finance client for this data source") }
    }

    fn portfolio(&self) -> Result<Portfolio> {
        Portfolio::load()
    }
    fn watchlist(&self) -> Result<Watchlist> {
        Watchlist::load()
    }
    fn alerts(&self) -> Result<AlertStore> {
        AlertStore::load()
    }
    /// Each intraday daemon's last saved state (one per market that ran).
    fn daemon_status(&self) -> Vec<DaemonStatus> {
        crate::daemon_status::load()
    }
}

impl DataSource for YahooClient {
    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        self.get_quote(symbols).await
    }
    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        self.get_chart(symbol, range, interval).await
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        YahooClient::search(self, query).await
    }
    async fn next_results(&self, symbol: &str, today: NaiveDate) -> Option<NaiveDate> {
        crate::calendar::next_results(self, symbol, today).await
    }
    async fn india_10y(&self) -> Option<crate::gsec::YieldRow> {
        live_india_10y().await
    }
    async fn fund_navs(&self) -> Result<std::sync::Arc<Vec<crate::funds::Scheme>>> {
        crate::funds::load_amfi_shared().await
    }
    async fn sgb_list(&self) -> Result<crate::commodity::SgbList> {
        crate::commodity::nse_sgb_list().await
    }
    async fn yahoo(&self) -> Result<&YahooClient> {
        Ok(self)
    }
}

/// The 10-year benchmark from CCIL (RBI fallback), cached for hours; a
/// short timeout so a slow site never holds up `markets`.
async fn live_india_10y() -> Option<crate::gsec::YieldRow> {
    let set = crate::gsec::yields(std::time::Duration::from_secs(6))
        .await
        .ok()?;
    crate::gsec::ten_year(&set.rows).cloned()
}

/// A [`YahooClient`] opened on first use. Long-running modes (`tui`,
/// `serve`) start without the network and retry the Yahoo session on the
/// next request if opening it fails. All requests share one client, so they
/// share its rate limiter and cache.
#[derive(Default)]
pub struct LiveSource {
    client: tokio::sync::OnceCell<YahooClient>,
}

impl LiveSource {
    pub fn new() -> Self {
        Self::default()
    }

    async fn client(&self) -> Result<&YahooClient> {
        self.client.get_or_try_init(YahooClient::new).await
    }
}

impl DataSource for LiveSource {
    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        self.client().await?.get_quote(symbols).await
    }
    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        self.client()
            .await?
            .get_chart(symbol, range, interval)
            .await
    }
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.client().await?.search(query).await
    }
    async fn next_results(&self, symbol: &str, today: NaiveDate) -> Option<NaiveDate> {
        let client = self.client().await.ok()?;
        crate::calendar::next_results(client, symbol, today).await
    }
    async fn india_10y(&self) -> Option<crate::gsec::YieldRow> {
        live_india_10y().await
    }
    async fn fund_navs(&self) -> Result<std::sync::Arc<Vec<crate::funds::Scheme>>> {
        crate::funds::load_amfi_shared().await
    }
    async fn sgb_list(&self) -> Result<crate::commodity::SgbList> {
        crate::commodity::nse_sgb_list().await
    }
    async fn yahoo(&self) -> Result<&YahooClient> {
        self.client().await
    }
}

/// "This symbol has no data" — as opposed to Yahoo being unreachable. The
/// server answers 404 for it instead of 502.
#[derive(Debug)]
pub struct NotFound(pub String);

impl std::fmt::Display for NotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NotFound {}

fn find_quote<'a>(quotes: &'a [Quote], symbol: &str) -> Option<&'a Quote> {
    quotes.iter().find(|q| q.symbol.as_deref() == Some(symbol))
}

// ── quote ──

/// Quotes for already-resolved symbols, plus the symbols Yahoo silently
/// dropped (unknown tickers).
pub struct QuoteSet {
    pub quotes: Vec<Quote>,
    pub missing: Vec<String>,
}

pub async fn quotes(src: &impl DataSource, resolved: &[String]) -> Result<QuoteSet> {
    let refs: Vec<&str> = resolved.iter().map(String::as_str).collect();
    let quotes = src.quotes(&refs).await?;
    let missing = resolved
        .iter()
        .filter(|r| {
            !quotes.iter().any(|q| {
                q.symbol
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(r))
            })
        })
        .cloned()
        .collect();
    Ok(QuoteSet { quotes, missing })
}

// ── markets ──

/// One index in `stockwise markets --json` (field order = key order).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct IndexRow {
    pub region: String,
    pub symbol: String,
    pub name: String,
    pub price: Option<f64>,
    pub change: Option<f64>,
    pub change_percent: Option<f64>,
    pub currency: Option<String>,
}

/// What one block of `markets` lists. `region` is the stable English key
/// used in JSON; `title` is what people read, in their language.
pub struct SectionSpec {
    pub region: &'static str,
    pub title: &'static str,
    /// (symbol, display name)
    pub items: Vec<(String, String)>,
}

pub fn section_spec(kind: Market) -> SectionSpec {
    use crate::i18n::{Msg, t};
    let owned = |list: &[(&str, &str)]| -> Vec<(String, String)> {
        list.iter()
            .map(|(s, n)| (s.to_string(), n.to_string()))
            .collect()
    };
    match kind {
        Market::In => SectionSpec {
            region: "Indian Markets",
            title: t(Msg::SectionIndianMarkets),
            items: owned(market::INDIA_INDICES),
        },
        Market::Us => SectionSpec {
            region: "US Markets",
            title: t(Msg::SectionUsMarkets),
            items: owned(market::US_INDICES),
        },
        Market::Crypto => SectionSpec {
            region: "Crypto",
            title: t(Msg::SectionCrypto),
            items: market::crypto_majors()
                .into_iter()
                .map(|(s, n)| (s, n.to_string()))
                .collect(),
        },
        Market::Fx => SectionSpec {
            region: "FX",
            title: t(Msg::SectionFx),
            items: owned(market::FX_MAJORS),
        },
        Market::Commodity => SectionSpec {
            region: "Commodities",
            title: t(Msg::SectionCommodities),
            items: owned(market::COMMODITY_UNIVERSE),
        },
        Market::Global(x) => {
            let info = x.info();
            SectionSpec {
                region: info.country,
                title: t(info.title),
                items: owned(info.indices),
            }
        }
    }
}

/// Symbol of the India 10-year G-sec yield row in `markets` (not a Yahoo
/// symbol; its `price` is a yield in percent and it has no currency).
pub const INDIA_10Y: &str = "IN10Y";

/// The `markets` row for the India 10-year yield.
pub fn india_10y_row(region: &str, y: &crate::gsec::YieldRow) -> IndexRow {
    IndexRow {
        region: region.to_string(),
        symbol: INDIA_10Y.to_string(),
        name: crate::i18n::t(crate::i18n::Msg::India10yName).to_string(),
        price: y.ytm,
        change: None,
        change_percent: None,
        currency: None,
    }
}

/// `markets --global`: each exchange's main index, India and the US first.
pub fn global_section_spec() -> SectionSpec {
    use crate::i18n::{Msg, t};
    let mut items = vec![
        ("^NSEI".to_string(), "India — NIFTY 50".to_string()),
        ("^GSPC".to_string(), "US — S&P 500".to_string()),
    ];
    items.extend(market::Exchange::ALL.iter().map(|x| {
        let info = x.info();
        let (symbol, name) = info.benchmark();
        (
            symbol.to_string(),
            format!("{} — {}", info.code.to_uppercase(), name),
        )
    }));
    SectionSpec {
        region: "Global",
        title: t(Msg::SectionGlobal),
        items,
    }
}

/// The market a `markets --global` row's index belongs to, for its
/// open/closed column.
pub fn benchmark_market(symbol: &str) -> Option<Market> {
    match symbol {
        "^NSEI" => Some(Market::In),
        "^GSPC" => Some(Market::Us),
        _ => market::Exchange::ALL
            .into_iter()
            .find(|x| x.info().benchmark().0.eq_ignore_ascii_case(symbol))
            .map(Market::Global),
    }
}

/// Sections for `markets`: the active market first, then the other stock
/// market; crypto and FX are added on request (or lead when selected).
pub fn section_order(market: Market, crypto: bool, fx: bool) -> Vec<Market> {
    let mut order = match market {
        Market::Us => vec![Market::Us, Market::In],
        Market::In => vec![Market::In, Market::Us],
        Market::Crypto | Market::Fx | Market::Commodity | Market::Global(_) => {
            vec![market, Market::In, Market::Us]
        }
    };
    for (wanted, kind) in [(crypto, Market::Crypto), (fx, Market::Fx)] {
        if wanted && !order.contains(&kind) {
            order.push(kind);
        }
    }
    order
}

fn label_for<'a>(items: &'a [(String, String)], symbol: &'a str) -> &'a str {
    items
        .iter()
        .find(|(sym, _)| sym.eq_ignore_ascii_case(symbol))
        .map_or(symbol, |(_, name)| name.as_str())
}

/// Rows for one section, in the order Yahoo returned them. Labels are matched
/// by symbol — Yahoo may reorder or drop symbols, so pairing by position
/// could mislabel an index.
pub fn index_rows(region: &str, items: &[(String, String)], quotes: &[Quote]) -> Vec<IndexRow> {
    quotes
        .iter()
        .map(|q| {
            let q_sym = q.symbol.as_deref().unwrap_or("");
            IndexRow {
                region: region.to_string(),
                symbol: q_sym.to_string(),
                name: label_for(items, q_sym).to_string(),
                price: q.regular_market_price,
                change: q.regular_market_change,
                change_percent: q.regular_market_change_percent,
                currency: q.currency.clone(),
            }
        })
        .collect()
}

pub struct MarketSection {
    pub region: &'static str,
    pub title: &'static str,
    pub rows: Vec<IndexRow>,
}

/// The `markets` sections, one quote request each.
pub async fn market_sections(
    src: &impl DataSource,
    market: Market,
    crypto: bool,
    fx: bool,
    global: bool,
) -> Result<Vec<MarketSection>> {
    let mut out = Vec::new();
    let specs = section_order(market, crypto, fx)
        .into_iter()
        .map(section_spec)
        .chain(global.then(global_section_spec));
    for spec in specs {
        let symbols: Vec<&str> = spec.items.iter().map(|(sym, _)| sym.as_str()).collect();
        let quotes = src.quotes(&symbols).await?;
        let mut rows = index_rows(spec.region, &spec.items, &quotes);
        if spec.region == "Indian Markets"
            && let Some(y) = src.india_10y().await
        {
            rows.push(india_10y_row(spec.region, &y));
        }
        out.push(MarketSection {
            region: spec.region,
            title: spec.title,
            rows,
        });
    }
    Ok(out)
}

/// `markets --json`: every section's rows.
pub async fn markets(
    src: &impl DataSource,
    market: Market,
    crypto: bool,
    fx: bool,
    global: bool,
) -> Result<Vec<IndexRow>> {
    Ok(market_sections(src, market, crypto, fx, global)
        .await?
        .into_iter()
        .flat_map(|s| s.rows)
        .collect())
}

// ── history ──

/// `stockwise history --json` (field order = key order).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct History {
    pub symbol: String,
    pub currency: Option<String>,
    pub period: String,
    pub interval: String,
    pub bars: Vec<Bar>,
}

/// One OHLCV bar of `history --json/--csv`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Bar {
    /// RFC 3339 UTC timestamp of the bar.
    pub time: Option<String>,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: Option<f64>,
    pub volume: Option<u64>,
    /// Unix seconds, for printers; not part of the JSON.
    #[serde(skip)]
    pub ts: i64,
}

/// Yahoo range and bar interval for a `history` period.
pub fn history_range(period: &str) -> (&str, &'static str) {
    match period {
        "1d" => ("1d", "5m"),
        "5d" | "1w" => ("5d", "15m"),
        "1mo" => ("1mo", "1d"),
        "3mo" | "6mo" => (period, "1d"),
        "1y" | "2y" => (period, "1wk"),
        "5y" | "10y" | "max" => (period, "1mo"),
        _ => (period, "1d"),
    }
}

/// Bars from a chart response: one per timestamp, missing values as None.
pub fn history_from_chart(chart: ChartData, symbol: &str, period: &str, interval: &str) -> History {
    let ind = chart.indicators.quote.into_iter().next();
    let (open, high, low, close, volume) = match ind {
        Some(q) => (
            q.open.unwrap_or_default(),
            q.high.unwrap_or_default(),
            q.low.unwrap_or_default(),
            q.close.unwrap_or_default(),
            q.volume.unwrap_or_default(),
        ),
        None => Default::default(),
    };
    let at = |v: &[Option<f64>], i: usize| v.get(i).copied().flatten();
    let bars = chart
        .timestamp
        .unwrap_or_default()
        .into_iter()
        .enumerate()
        .map(|(i, ts)| Bar {
            time: chrono::DateTime::from_timestamp(ts, 0).map(|d| d.to_rfc3339()),
            open: at(&open, i),
            high: at(&high, i),
            low: at(&low, i),
            close: at(&close, i),
            volume: volume.get(i).copied().flatten(),
            ts,
        })
        .collect();
    History {
        symbol: symbol.to_string(),
        currency: chart.meta.and_then(|m| m.currency),
        period: period.to_string(),
        interval: interval.to_string(),
        bars,
    }
}

/// Price history for a resolved symbol. No bars is a [`NotFound`] error.
pub async fn history(
    src: &impl DataSource,
    resolved: &str,
    period: &str,
    market: Market,
) -> Result<History> {
    let (range, interval) = history_range(period);
    let chart = src.chart(resolved, range, interval).await?;
    let hist = history_from_chart(chart, resolved, period, interval);
    if hist.bars.is_empty() {
        return Err(NotFound(format!(
            "No price history for {}{}",
            resolved,
            market::not_found_hint(market)
        ))
        .into());
    }
    Ok(hist)
}

// ── portfolio, watchlist, alerts ──
//
// The row builders live with the `portfolio`, `watch` and `alert` printers
// (`commands::holdings`); these wrap them so `/api/*` and the TUI return
// exactly what `--json` prints.

pub(crate) use crate::commands::{AlertCheckRow, WatchRow};

/// `portfolio --json`: holdings (lots, FIFO cost), totals in one base
/// currency, and the symbols left out of the totals (never valued at 0).
pub(crate) type PortfolioView = crate::commands::PortfolioReport;

/// Value `portfolio` from `quotes` (which may hold other symbols too),
/// fetching exchange rates only when holdings mix currencies. `off` holds
/// the fund and SGB prices already looked up (it may cover more symbols).
pub async fn portfolio_view(
    src: &impl DataSource,
    portfolio: &Portfolio,
    quotes: &[Quote],
    off: &crate::offmarket::OffMarket,
    market: Market,
) -> PortfolioView {
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let own: Vec<Quote> = quotes
        .iter()
        .filter(|q| q.symbol.as_ref().is_some_and(|s| symbols.contains(s)))
        .cloned()
        .collect();
    let mut val = crate::commands::Valuation::new(own, &symbols, market, None);
    val.off = off.restrict(&symbols);
    val.off.merge_into(&mut val.quotes);
    let currencies = val.currencies(&symbols);
    val.add_rates(src, &currencies).await;
    crate::commands::portfolio_report(portfolio, &val, chrono::Local::now().date_naive())
}

/// A local data file (portfolio, watchlist, alerts) could not be read — as
/// opposed to market data. The server answers 500 for it instead of 502.
#[derive(Debug)]
pub struct LocalData;

impl std::fmt::Display for LocalData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Could not read local data")
    }
}

fn local<T>(r: Result<T>) -> Result<T> {
    r.map_err(|e| e.context(LocalData))
}

fn unique_symbols<'a>(groups: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::new();
    for s in groups {
        if !out.contains(&s) {
            out.push(s);
        }
    }
    out
}

/// Fetch quotes only when there is something to price.
async fn quotes_for(src: &impl DataSource, symbols: &[&str]) -> Result<Vec<Quote>> {
    if symbols.is_empty() {
        Ok(Vec::new())
    } else {
        src.quotes(symbols).await
    }
}

/// Market quotes for `symbols`, plus fund NAVs and SGB prices for the ones
/// Yahoo does not quote (merged into the quotes, and returned with how
/// each was priced). `MF:` symbols are never sent to Yahoo.
pub async fn quotes_with_off_market(
    src: &impl DataSource,
    symbols: &[&str],
) -> Result<(Vec<Quote>, crate::offmarket::OffMarket)> {
    let yahoo: Vec<&str> = symbols
        .iter()
        .copied()
        .filter(|s| !market::is_namespaced(s))
        .collect();
    let mut quotes = quotes_for(src, &yahoo).await?;
    let owned: Vec<String> = symbols.iter().map(|s| s.to_string()).collect();
    let off = crate::offmarket::price(src, &owned, &quotes).await;
    off.merge_into(&mut quotes);
    Ok((quotes, off))
}

/// `portfolio --json`, with totals in `base` (or the holdings' currency).
pub async fn portfolio(
    src: &impl DataSource,
    market: Market,
    base: Option<&str>,
) -> Result<PortfolioView> {
    crate::commands::load_portfolio_report(src, market, base).await
}

/// `watch --json`: one row per watched symbol, in watchlist order.
pub async fn watchlist(src: &impl DataSource) -> Result<Vec<WatchRow>> {
    let wl = local(src.watchlist())?;
    let symbols = unique_symbols(wl.symbols.iter().map(String::as_str));
    let quotes = quotes_with_off_market(src, &symbols).await?.0;
    Ok(crate::commands::watch_rows(&wl.symbols, &quotes))
}

/// `alert check --json`: every alert with its live price and a `status` of
/// `triggered`, `waiting`, `no_price` or `no_data`.
pub async fn alerts(src: &impl DataSource) -> Result<Vec<AlertCheckRow>> {
    let store = local(src.alerts())?;
    let symbols = unique_symbols(store.alerts.iter().map(|a| a.symbol.as_str()));
    let quotes = quotes_with_off_market(src, &symbols).await?.0;
    Ok(alert_rows(src, &store.alerts, &quotes, Utc::now()).await)
}

/// Every alert evaluated: price alerts against `quotes`, technical ones
/// against daily bars, `results-in` against the results calendar.
pub async fn alert_rows(
    src: &impl DataSource,
    alerts: &[Alert],
    quotes: &[Quote],
    now: DateTime<Utc>,
) -> Vec<AlertCheckRow> {
    let data = alert_data(src, alerts, quotes, now).await;
    crate::commands::alert_check_rows(alerts, &data)
}

/// What `alerts` need, by symbol: the quote; daily bars (one chart request
/// per symbol, long enough for every alert on it and brought up to the live
/// quote) for technical conditions; the next results date for `results-in`.
/// A chart that fails leaves the symbol without bars, so its technical
/// alerts read `no_data` rather than a guess.
pub async fn alert_data(
    src: &impl DataSource,
    alerts: &[Alert],
    quotes: &[Quote],
    now: DateTime<Utc>,
) -> HashMap<String, MarketData> {
    let mut data = HashMap::new();
    for alert in alerts {
        if data.contains_key(&alert.symbol) {
            continue;
        }
        let symbol = alert.symbol.as_str();
        let on_symbol = || alerts.iter().filter(|a| a.symbol == symbol);
        let quote = find_quote(quotes, symbol);
        let today = crate::calendar::exchange_today(symbol, now);
        let mut d = MarketData {
            price: quote.and_then(|q| q.regular_market_price),
            currency: quote.and_then(|q| q.currency.clone()),
            change_percent: quote.and_then(|q| q.regular_market_change_percent),
            today: Some(today),
            ..Default::default()
        };
        let bars_needed = on_symbol().map(Alert::bars_needed).max().unwrap_or(0);
        if bars_needed > 0
            && let Ok(chart) = src
                .chart(symbol, crate::alerts::chart_range(bars_needed), "1d")
                .await
        {
            let mut bars = crate::backtest::Bars::from_chart(&chart);
            if let Some(q) = quote {
                let traded = q
                    .regular_market_time
                    .and_then(|t| DateTime::from_timestamp(t, 0))
                    .map(|t| crate::calendar::exchange_today(symbol, t));
                crate::alerts::patch_live_bar(&mut bars, q, today, traded);
            }
            d.bars = Some(bars);
        }
        if on_symbol().any(Alert::needs_results) {
            d.next_results = src.next_results(symbol, today).await;
        }
        data.insert(alert.symbol.clone(), d);
    }
    data
}

// ── dashboard snapshot ──

/// Everything the dashboard shows, priced from a single quote request.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub indices: Vec<IndexRow>,
    pub watchlist: Vec<WatchRow>,
    pub portfolio: PortfolioView,
    pub alerts: Vec<AlertCheckRow>,
}

/// Home-market indices (in list order, unpriced ones kept), watchlist,
/// portfolio and alerts in one batched quote request.
pub async fn snapshot(src: &impl DataSource, market: Market) -> Result<Snapshot> {
    let spec = section_spec(market);
    let p = local(src.portfolio())?;
    let wl = local(src.watchlist())?;
    let store = local(src.alerts())?;
    let symbols = unique_symbols(
        spec.items
            .iter()
            .map(|(s, _)| s.as_str())
            .chain(wl.symbols.iter().map(String::as_str))
            .chain(p.holdings.iter().map(|h| h.symbol.as_str()))
            .chain(store.alerts.iter().map(|a| a.symbol.as_str())),
    );
    let (quotes, off) = quotes_with_off_market(src, &symbols).await?;
    let indices = spec
        .items
        .iter()
        .map(|(sym, name)| {
            let q = quotes.iter().find(|q| {
                q.symbol
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(sym))
            });
            IndexRow {
                region: spec.region.to_string(),
                symbol: sym.to_string(),
                name: name.to_string(),
                price: q.and_then(|q| q.regular_market_price),
                change: q.and_then(|q| q.regular_market_change),
                change_percent: q.and_then(|q| q.regular_market_change_percent),
                currency: q.and_then(|q| q.currency.clone()),
            }
        })
        .collect();
    Ok(Snapshot {
        indices,
        watchlist: crate::commands::watch_rows(&wl.symbols, &quotes),
        portfolio: portfolio_view(src, &p, &quotes, &off, market).await,
        alerts: alert_rows(src, &store.alerts, &quotes, Utc::now()).await,
    })
}

/// Validate a `history` period the way the CLI does.
pub fn check_period(period: &str) -> Result<()> {
    if crate::PERIODS.contains(&period) {
        Ok(())
    } else {
        bail!(
            "Unknown period '{}' (use one of {})",
            period,
            crate::PERIODS.join(", ")
        )
    }
}

#[cfg(test)]
pub mod fixtures {
    //! Canned market data for tests of the views, the server and the TUI.

    use super::*;
    use crate::alerts::AlertCondition;
    use crate::api::{ChartMeta, Indicators, QuoteIndicator};
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub fn quote(symbol: &str, price: Option<f64>, change_pct: f64, currency: &str) -> Quote {
        Quote {
            symbol: Some(symbol.to_string()),
            short_name: Some(format!("{} Ltd", symbol.trim_end_matches(".NS"))),
            regular_market_price: price,
            regular_market_change: price.map(|p| p * change_pct / 100.0),
            regular_market_change_percent: Some(change_pct),
            regular_market_volume: Some(1_234_567),
            currency: Some(currency.to_string()),
            ..Default::default()
        }
    }

    pub fn chart(closes: &[f64], start_ts: i64, step: i64, currency: &str) -> ChartData {
        let some = |v: &[f64]| Some(v.iter().map(|c| Some(*c)).collect::<Vec<_>>());
        ChartData {
            meta: Some(ChartMeta {
                regular_market_price: closes.last().copied(),
                chart_previous_close: None,
                symbol: None,
                long_name: None,
                short_name: None,
                regular_market_day_high: None,
                regular_market_day_low: None,
                regular_market_volume: None,
                fifty_two_week_high: None,
                fifty_two_week_low: None,
                currency: Some(currency.to_string()),
                exchange_name: None,
            }),
            timestamp: Some(
                (0..closes.len() as i64)
                    .map(|i| start_ts + i * step)
                    .collect(),
            ),
            indicators: Indicators {
                quote: vec![QuoteIndicator {
                    open: some(closes),
                    high: some(&closes.iter().map(|c| c + 1.0).collect::<Vec<_>>()),
                    low: some(&closes.iter().map(|c| c - 1.0).collect::<Vec<_>>()),
                    close: some(closes),
                    volume: Some(closes.iter().map(|_| Some(1000)).collect()),
                }],
            },
            events: None,
        }
    }

    /// A market with a few Indian stocks and the index symbols. Unknown
    /// symbols are dropped, like Yahoo does.
    pub struct Fixture {
        pub quotes: Vec<Quote>,
        pub portfolio: Portfolio,
        pub watchlist: Watchlist,
        pub alerts: AlertStore,
        pub daemon: Vec<DaemonStatus>,
        /// Number of quote requests served.
        pub quote_calls: AtomicUsize,
        /// When set, every network call fails with this message.
        pub fail: Mutex<Option<String>>,
        /// The India 10Y benchmark `markets` adds, if any.
        pub ten_year: Option<crate::gsec::YieldRow>,
        /// AMFI's NAV file (`NAVAll.txt` text); None = AMFI unreachable.
        pub navall: Option<String>,
        /// NSE's SGB list; None = NSE refused.
        pub sgbs: Option<Vec<crate::commodity::NseSgb>>,
        /// Every symbol asked of the quote source, in order.
        pub requested: Mutex<Vec<String>>,
    }

    impl Default for Fixture {
        fn default() -> Self {
            let mut portfolio = Portfolio::default();
            portfolio.add("TCS.NS", 10.0, 3500.0);
            portfolio.add("INFY.NS", 5.0, 1600.0);
            portfolio.add("DELISTED.NS", 3.0, 100.0);
            let mut watchlist = Watchlist::default();
            watchlist.add("RELIANCE.NS");
            watchlist.add("TCS.NS");
            let mut alerts = AlertStore::default();
            alerts.add("TCS.NS", AlertCondition::Above, 4000.0);
            alerts.add("RELIANCE.NS", AlertCondition::Below, 1000.0);
            alerts.add("DELISTED.NS", AlertCondition::Below, 1_000_000.0);
            Self {
                quotes: vec![
                    quote("^NSEI", Some(24800.0), 0.5, "INR"),
                    quote("^BSESN", Some(81200.0), 0.4, "INR"),
                    quote("^NSEBANK", Some(52000.0), -0.3, "INR"),
                    quote("^CNXIT", Some(41000.0), 1.1, "INR"),
                    quote("^GSPC", Some(5800.0), 0.2, "USD"),
                    quote("TCS.NS", Some(4100.0), 1.5, "INR"),
                    quote("INFY.NS", Some(1500.0), -2.0, "INR"),
                    quote("RELIANCE.NS", Some(1300.0), 0.8, "INR"),
                ],
                portfolio,
                watchlist,
                alerts,
                daemon: Vec::new(),
                quote_calls: AtomicUsize::new(0),
                fail: Mutex::new(None),
                ten_year: None,
                navall: None,
                sgbs: None,
                requested: Mutex::new(Vec::new()),
            }
        }
    }

    impl Fixture {
        fn check(&self) -> Result<()> {
            match self.fail.lock().unwrap().as_ref() {
                Some(msg) => bail!("{}", msg),
                None => Ok(()),
            }
        }
    }

    impl DataSource for Fixture {
        async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
            self.check()?;
            self.quote_calls.fetch_add(1, Ordering::SeqCst);
            self.requested
                .lock()
                .unwrap()
                .extend(symbols.iter().map(|s| s.to_string()));
            Ok(symbols
                .iter()
                .filter_map(|s| find_quote(&self.quotes, s).cloned())
                .collect())
        }
        async fn chart(&self, symbol: &str, _range: &str, interval: &str) -> Result<ChartData> {
            self.check()?;
            if find_quote(&self.quotes, symbol).is_none() {
                bail!("Yahoo Finance returned HTTP 404 Not Found");
            }
            let step = if interval == "1d" { 86_400 } else { 300 };
            Ok(chart(
                &[100.0, 102.0, 101.0, 105.0, 107.5],
                1_700_000_000,
                step,
                "INR",
            ))
        }
        async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
            self.check()?;
            Ok(self
                .quotes
                .iter()
                .filter(|q| {
                    q.symbol
                        .as_deref()
                        .is_some_and(|s| s.to_lowercase().contains(&query.to_lowercase()))
                })
                .map(|q| SearchResult {
                    symbol: q.symbol.clone(),
                    short_name: q.short_name.clone(),
                    long_name: None,
                    quote_type: Some("EQUITY".into()),
                    exchange: Some("NSI".into()),
                })
                .collect())
        }
        async fn india_10y(&self) -> Option<crate::gsec::YieldRow> {
            self.ten_year.clone()
        }
        async fn fund_navs(&self) -> Result<std::sync::Arc<Vec<crate::funds::Scheme>>> {
            match &self.navall {
                Some(text) => Ok(std::sync::Arc::new(crate::funds::parse_navall(text))),
                None => bail!("AMFI returned HTTP 503"),
            }
        }
        async fn sgb_list(&self) -> Result<crate::commodity::SgbList> {
            match &self.sgbs {
                Some(rows) => Ok(crate::commodity::SgbList {
                    rows: rows.clone(),
                    saved_at: None,
                }),
                None => bail!("NSE answered HTTP 403"),
            }
        }
        fn portfolio(&self) -> Result<Portfolio> {
            Ok(Portfolio {
                holdings: self.portfolio.holdings.clone(),
                ..Default::default()
            })
        }
        fn watchlist(&self) -> Result<Watchlist> {
            Ok(Watchlist {
                symbols: self.watchlist.symbols.clone(),
            })
        }
        fn alerts(&self) -> Result<AlertStore> {
            Ok(AlertStore {
                alerts: self.alerts.alerts.clone(),
            })
        }
        fn daemon_status(&self) -> Vec<DaemonStatus> {
            self.daemon.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn block<F: Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn quote_json_keeps_yahoo_field_names() {
        // `quote --json` prints Yahoo's own camelCase quote objects.
        let v = serde_json::to_value(quote("TCS.NS", Some(4100.0), 1.5, "INR")).unwrap();
        for key in [
            "symbol",
            "shortName",
            "longName",
            "regularMarketPrice",
            "regularMarketChange",
            "regularMarketChangePercent",
            "regularMarketVolume",
            "fiftyTwoWeekHigh",
            "marketCap",
            "trailingPE",
            "currency",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(v["regularMarketPrice"], 4100.0);
        assert!(v["longName"].is_null());
    }

    #[test]
    fn technical_alerts_read_daily_bars() {
        use crate::alerts::AlertCondition as C;
        let mut f = Fixture::default();
        f.alerts = AlertStore::default();
        // The fixture's daily closes: 100, 102, 101, 105, 107.5 (highs +1).
        f.alerts.add("TCS.NS", C::BreakoutHigh, 3.0);
        f.alerts.add("TCS.NS", C::CrossAboveSma, 3.0);
        f.alerts.add("TCS.NS", C::RsiBelow, 30.0);
        f.alerts.add("TCS.NS", C::ChangeAbove, 1.0);
        f.alerts.add("TCS.NS", C::ResultsIn, 7.0);
        f.alerts.add("DELISTED.NS", C::CrossAboveEma, 2.0);
        // Saturday: NSE closed, so the bars are used as they are.
        let saturday = DateTime::parse_from_rfc3339("2026-10-24T05:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let quotes = block(f.quotes(&["TCS.NS"])).unwrap();
        let rows = block(alert_rows(&f, &f.alerts.alerts, &quotes, saturday));
        let status: Vec<&str> = rows.iter().map(|r| r.status).collect();
        assert_eq!(
            status,
            [
                // 107.5 closes above the previous 3 highs (max 106).
                "triggered",
                // Above its 3-day SMA, but it was already above yesterday.
                "waiting",
                // Five bars are too few for RSI(14).
                "no_data",
                // Quote: +1.5% today.
                "triggered",
                // No calendar in the fixture.
                "no_data",
                // No chart for an unknown symbol.
                "no_data",
            ]
        );
        assert_eq!(rows[0].value, Some(107.5));
        assert_eq!(rows[0].reference, Some(106.0));
        assert_eq!(rows[0].price, Some(4100.0));
    }

    #[test]
    fn quotes_reports_missing_symbols() {
        let f = Fixture::default();
        let set = block(quotes(&f, &["TCS.NS".into(), "NOPE.NS".into()])).unwrap();
        assert_eq!(set.quotes.len(), 1);
        assert_eq!(set.missing, vec!["NOPE.NS".to_string()]);
    }

    #[test]
    fn markets_add_the_india_10y_and_global_benchmarks() {
        let mut quotes = Fixture::default().quotes;
        quotes.push(quote("^FTSE", Some(10_690.0), -0.3, "GBP"));
        quotes.push(quote("^N225", Some(62_693.0), -0.3, "JPY"));
        let f = Fixture {
            quotes,
            ten_year: Some(crate::gsec::YieldRow {
                date: NaiveDate::from_ymd_opt(2026, 10, 23),
                tenor: "9Y-10Y".into(),
                kind: crate::gsec::Kind::Gsec,
                security: "6.94% GS 2036".into(),
                ytm: Some(7.4713),
                source: "CCIL",
            }),
            ..Default::default()
        };
        let rows = block(markets(&f, Market::In, false, false, true)).unwrap();
        let ten = rows.iter().find(|r| r.symbol == INDIA_10Y).unwrap();
        assert_eq!(ten.region, "Indian Markets");
        assert_eq!(ten.price, Some(7.4713));
        assert_eq!(ten.currency, None);
        assert_eq!(ten.change, None);
        let global: Vec<&IndexRow> = rows.iter().filter(|r| r.region == "Global").collect();
        let names: Vec<&str> = global.iter().map(|r| r.name.as_str()).collect();
        // Rows without a quote are dropped, as in every section.
        assert_eq!(
            names,
            [
                "India — NIFTY 50",
                "US — S&P 500",
                "UK — FTSE 100",
                "JP — Nikkei 225"
            ]
        );
        assert_eq!(
            benchmark_market("^FTSE"),
            Some(Market::Global(market::Exchange::Uk))
        );
        assert_eq!(benchmark_market("^NSEI"), Some(Market::In));
        assert_eq!(benchmark_market("TCS.NS"), None);
        // A global market leads its own section, then India and the US.
        let uk = block(markets(
            &f,
            Market::Global(market::Exchange::Uk),
            false,
            false,
            false,
        ))
        .unwrap();
        assert_eq!(uk[0].region, "United Kingdom");
        assert_eq!(uk[0].symbol, "^FTSE");
    }

    #[test]
    fn markets_json_shape_is_pinned() {
        let rows = block(markets(
            &Fixture::default(),
            Market::In,
            false,
            false,
            false,
        ))
        .unwrap();
        let first = serde_json::to_string(&rows[0]).unwrap();
        assert_eq!(
            first,
            r#"{"region":"Indian Markets","symbol":"^NSEI","name":"NIFTY 50","price":24800.0,"change":124.0,"change_percent":0.5,"currency":"INR"}"#
        );
        // India first for the default market; US indices the fixture lacks are dropped.
        assert_eq!(
            rows.iter().filter(|r| r.region == "Indian Markets").count(),
            4
        );
        assert_eq!(rows.last().unwrap().symbol, "^GSPC");
        assert_eq!(rows.last().unwrap().region, "US Markets");
    }

    #[test]
    fn history_json_shape_is_pinned() {
        let f = Fixture::default();
        let h = block(history(&f, "TCS.NS", "1mo", Market::In)).unwrap();
        assert_eq!(h.interval, "1d");
        let v = serde_json::to_value(&h).unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(keys, ["symbol", "currency", "period", "interval", "bars"]);
        let bar = serde_json::to_string(&h.bars[0]).unwrap();
        assert_eq!(
            bar,
            r#"{"time":"2023-11-14T22:13:20+00:00","open":100.0,"high":101.0,"low":99.0,"close":100.0,"volume":1000}"#
        );
        assert_eq!(h.bars.len(), 5);
    }

    #[test]
    fn history_of_unknown_symbol_fails() {
        let f = Fixture::default();
        assert!(block(history(&f, "NOPE.NS", "1mo", Market::In)).is_err());
        let empty = ChartData {
            timestamp: None,
            ..chart(&[], 0, 1, "INR")
        };
        let h = history_from_chart(empty, "X", "1mo", "1d");
        assert!(h.bars.is_empty());
    }

    #[test]
    fn history_range_matches_cli_periods() {
        assert_eq!(history_range("1w"), ("5d", "15m"));
        assert_eq!(history_range("1d"), ("1d", "5m"));
        assert_eq!(history_range("2y"), ("2y", "1wk"));
        assert_eq!(history_range("max"), ("max", "1mo"));
        assert!(check_period("ytd").is_ok());
        assert!(check_period("7x").is_err());
    }

    #[test]
    fn portfolio_view_excludes_unpriced_holdings_from_totals() {
        let v = block(portfolio(&Fixture::default(), Market::In, None)).unwrap();
        assert_eq!(v.holdings.len(), 3);
        // TCS 10 @ 3500 → 4100; INFY 5 @ 1600 → 1500.
        assert_eq!(v.totals.cost_basis, 35000.0 + 8000.0);
        assert_eq!(v.totals.value, 41000.0 + 7500.0);
        assert_eq!(v.totals.pnl, 5500.0);
        assert_eq!(v.totals.pnl_percent, 12.79);
        assert_eq!(v.unpriced, vec!["DELISTED.NS".to_string()]);
        let delisted = &v.holdings[2];
        assert_eq!(
            (delisted.price, delisted.value, delisted.pnl),
            (None, None, None)
        );
        assert_eq!(v.totals.currency.as_deref(), Some("INR"));
    }

    #[test]
    fn portfolio_json_matches_portfolio_json_flag() {
        // Same keys as `portfolio --json` (pinned in commands/output_tests.rs).
        let v =
            serde_json::to_value(block(portfolio(&Fixture::default(), Market::In, None)).unwrap())
                .unwrap();
        let keys: Vec<&str> = v.as_object().unwrap().keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["holdings", "totals", "unpriced", "unconverted", "notes"]
        );
        let totals: Vec<&str> = v["totals"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            totals,
            ["cost_basis", "value", "pnl", "pnl_percent", "currency"]
        );
        assert_eq!(v["holdings"][0]["symbol"], "TCS.NS");
        assert_eq!(v["holdings"][0]["pnl"], 6000.0);
        assert!(v["holdings"][2]["price"].is_null());
    }

    #[test]
    fn totals_currency_comes_from_a_holding() {
        // A US portfolio viewed with Indian indices in the same quote batch.
        let mut p = Portfolio::default();
        p.add("AAPL", 2.0, 100.0);
        let quotes = vec![
            quote("^NSEI", Some(24800.0), 0.5, "INR"),
            quote("AAPL", Some(150.0), 1.0, "USD"),
        ];
        let v = block(portfolio_view(
            &Fixture::default(),
            &p,
            &quotes,
            &Default::default(),
            Market::In,
        ));
        assert_eq!(v.totals.currency.as_deref(), Some("USD"));
        assert_eq!(v.totals.value, 300.0);
    }

    #[test]
    fn empty_portfolio_needs_no_network() {
        let f = Fixture {
            portfolio: Portfolio::default(),
            ..Default::default()
        };
        *f.fail.lock().unwrap() = Some("offline".into());
        let v = block(portfolio(&f, Market::In, None)).unwrap();
        assert!(v.holdings.is_empty());
        assert_eq!(v.totals.value, 0.0);
    }

    #[test]
    fn watchlist_keeps_order_and_unknown_symbols() {
        let mut f = Fixture::default();
        f.watchlist.add("NOPE.NS");
        let rows = block(watchlist(&f)).unwrap();
        let symbols: Vec<&str> = rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(symbols, ["RELIANCE.NS", "TCS.NS", "NOPE.NS"]);
        assert_eq!(rows[0].price, Some(1300.0));
        assert_eq!(rows[2].price, None);
    }

    #[test]
    fn alerts_never_fire_without_a_price() {
        let rows = block(alerts(&Fixture::default())).unwrap();
        let status: Vec<&str> = rows.iter().map(|r| r.status).collect();
        // TCS 4100 >= 4000; RELIANCE 1300 is not below 1000; no price for DELISTED.
        assert_eq!(status, ["triggered", "waiting", "no_price"]);
        assert_eq!(rows[2].price, None);
        assert_eq!(rows[0].condition, "above");
        assert_eq!(rows[1].index, 1);
    }

    #[test]
    fn local_file_errors_are_tagged() {
        struct Broken;
        impl DataSource for Broken {
            async fn quotes(&self, _: &[&str]) -> Result<Vec<Quote>> {
                Ok(Vec::new())
            }
            async fn chart(&self, _: &str, _: &str, _: &str) -> Result<ChartData> {
                bail!("unused")
            }
            async fn search(&self, _: &str) -> Result<Vec<SearchResult>> {
                Ok(Vec::new())
            }
            fn portfolio(&self) -> Result<Portfolio> {
                bail!("/home/me/portfolio.json is not valid JSON")
            }
        }
        let err = block(portfolio(&Broken, Market::In, None)).unwrap_err();
        assert!(err.downcast_ref::<LocalData>().is_some());
        // Market data errors are not tagged.
        let f = Fixture::default();
        *f.fail.lock().unwrap() = Some("offline".into());
        let err = block(watchlist(&f)).unwrap_err();
        assert!(err.downcast_ref::<LocalData>().is_none());
    }

    #[test]
    fn snapshot_uses_one_quote_request() {
        let f = Fixture::default();
        let s = block(snapshot(&f, Market::In)).unwrap();
        assert_eq!(f.quote_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
        let names: Vec<&str> = s.indices.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, ["NIFTY 50", "SENSEX", "BANK NIFTY", "NIFTY IT"]);
        assert_eq!(s.watchlist.len(), 2);
        assert_eq!(s.portfolio.holdings.len(), 3);
        assert_eq!(s.portfolio.totals.currency.as_deref(), Some("INR"));
        assert_eq!(
            s.alerts.iter().filter(|a| a.status == "triggered").count(),
            1
        );
    }

    /// A portfolio of a stock, two funds (AMFI code, IDCW reinvestment
    /// ISIN), an unknown fund code and two SGBs (one by ISIN).
    fn fund_fixture() -> Fixture {
        let mut f = Fixture::default();
        let mut p = Portfolio::default();
        p.add("TCS.NS", 10.0, 3500.0);
        p.add("MF:122639", 100.0, 80.0);
        p.add("MF:INF879O01308", 10.0, 100.0);
        p.add("MF:999999", 5.0, 10.0);
        p.add("SGBNOV26.NS", 2.0, 3183.0);
        p.add("IN0020180389", 1.0, 3119.0);
        f.portfolio = p;
        f.watchlist.add("MF:122639");
        f.navall = Some(crate::offmarket::tests::navall());
        f.sgbs = Some(vec![
            crate::commodity::NseSgb {
                symbol: "SGBNOV26".into(),
                isin: Some("IN0020180314".into()),
                name: None,
                issue_price: Some(3183.0),
                price: Some(12_500.0),
                change_percent: None,
                volume: None,
            },
            crate::commodity::NseSgb {
                symbol: "SGBDEC26".into(),
                isin: Some("IN0020180389".into()),
                name: None,
                issue_price: Some(3119.0),
                price: Some(12_400.0),
                change_percent: None,
                volume: None,
            },
        ]);
        f
    }

    fn row<'a>(v: &'a PortfolioView, symbol: &str) -> &'a crate::commands::HoldingRow {
        v.holdings.iter().find(|h| h.symbol == symbol).unwrap()
    }

    #[test]
    fn funds_and_sgbs_are_valued_from_amfi_and_nse() {
        let f = fund_fixture();
        let v = block(portfolio(&f, Market::In, None)).unwrap();
        let fund = row(&v, "MF:122639");
        assert_eq!(fund.price, Some(90.3546));
        assert_eq!(fund.value, Some(9035.46));
        assert_eq!(
            fund.name.as_deref(),
            Some("Parag Parikh Flexi Cap Fund - Direct Plan - Growth")
        );
        assert_eq!(
            fund.price_source,
            Some(crate::offmarket::PriceSource::AmfiNav)
        );
        assert_eq!(fund.nav_date, NaiveDate::from_ymd_opt(2026, 10, 19));
        assert_eq!(fund.currency.as_deref(), Some("INR"));
        assert_eq!(row(&v, "MF:INF879O01308").price, Some(90.3546));
        let sgb = row(&v, "SGBNOV26.NS");
        assert_eq!(sgb.value, Some(25_000.0));
        assert_eq!(
            sgb.price_source,
            Some(crate::offmarket::PriceSource::NseSgb)
        );
        assert_eq!(sgb.name.as_deref(), Some("SGB 2018-19 Series III"));
        assert_eq!(row(&v, "IN0020180389").price, Some(12_400.0));
        assert_eq!(
            row(&v, "TCS.NS").price_source,
            Some(crate::offmarket::PriceSource::Quote)
        );

        // The unknown code is N/A with a note — never 0.
        let unknown = row(&v, "MF:999999");
        assert_eq!((unknown.price, unknown.value), (None, None));
        assert_eq!(v.unpriced, ["MF:999999"]);
        assert!(
            v.notes
                .iter()
                .any(|n| n.contains("MF:999999 is not in AMFI's NAV file")),
            "{:?}",
            v.notes
        );
        assert!(v.notes.iter().any(|n| n.contains("2.5% yearly interest")));
        // TCS 41,000 + funds 9,035.46 and 903.55 + SGBs 25,000 and 12,400.
        assert_eq!(v.totals.value, 88_339.01);
        // Fund codes never go to Yahoo.
        let asked = f.requested.lock().unwrap();
        assert!(!asked.iter().any(|s| s.starts_with("MF:")), "{asked:?}");
    }

    #[test]
    fn dashboard_snapshot_values_funds_like_the_portfolio_command() {
        let f = fund_fixture();
        let cli = block(portfolio(&f, Market::In, None)).unwrap();
        let s = block(snapshot(&f, Market::In)).unwrap();
        assert_eq!(s.portfolio.totals.value, cli.totals.value);
        assert_eq!(s.portfolio.unpriced, cli.unpriced);
        assert_eq!(s.portfolio.notes, cli.notes);
        // The watchlist sees the NAV too.
        let watched = s
            .watchlist
            .iter()
            .find(|w| w.symbol == "MF:122639")
            .unwrap();
        assert_eq!(watched.price, Some(90.3546));
    }

    #[test]
    fn sgbs_fall_back_to_a_labelled_gold_estimate_when_nse_refuses() {
        let mut f = fund_fixture();
        f.sgbs = None;
        f.navall = None;
        f.quotes.push(quote("GC=F", Some(4000.0), 0.0, "USD"));
        f.quotes.push(quote("USDINR=X", Some(95.0), 0.0, "INR"));
        let v = block(portfolio(&f, Market::In, None)).unwrap();
        let sgb = row(&v, "SGBNOV26.NS");
        assert_eq!(
            sgb.price_source,
            Some(crate::offmarket::PriceSource::GoldEstimate)
        );
        // $4000/oz × ₹95 / 31.1035 g.
        assert_eq!(sgb.price, Some(12_217.28));
        assert!(
            v.notes
                .iter()
                .any(|n| n.contains("Estimated") && n.contains("HTTP 403")),
            "{:?}",
            v.notes
        );
        // AMFI down: every fund unpriced, one note naming them.
        assert_eq!(v.unpriced, ["MF:122639", "MF:INF879O01308", "MF:999999"]);
        assert!(
            v.notes.iter().any(|n| n.contains("HTTP 503")),
            "{:?}",
            v.notes
        );
    }

    #[test]
    fn snapshot_keeps_unpriced_us_indices() {
        let s = block(snapshot(&Fixture::default(), Market::Us)).unwrap();
        assert_eq!(s.indices.len(), market::US_INDICES.len());
        assert_eq!(s.indices[0].price, Some(5800.0));
        assert_eq!(s.indices[1].price, None);
    }
}
