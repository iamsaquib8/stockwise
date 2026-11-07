//! Market-data providers behind one trait.
//!
//! Yahoo Finance (`api::YahooClient`) is the default and is always there:
//! it fills every field another provider lacks (fundamentals, 52-week range,
//! names), serves symbols a provider doesn't cover (NSE stocks, indices),
//! and takes over when a provider errors or runs out of free-tier calls.
//!
//! An alternative provider is opt-in: `STOCKWISE_DATA_PROVIDER` names it and
//! its API key comes from the environment. Commands keep calling
//! `YahooClient::get_quote` / `get_chart` / `search`; the client routes to
//! the provider underneath.
//!
//! | Provider      | `STOCKWISE_DATA_PROVIDER` | Key variable          | Covers                         |
//! |---------------|---------------------------|-----------------------|--------------------------------|
//! | Alpha Vantage | `alphavantage`            | `ALPHAVANTAGE_API_KEY`| US, BSE (`.BO`), FX, crypto    |
//! | Massive       | `massive` / `polygon`     | `MASSIVE_API_KEY`     | US stocks, FX, crypto          |
//! | Twelve Data   | `twelvedata`              | `TWELVEDATA_API_KEY`  | US stocks, FX, crypto          |
//! | NSE website   | `nse`                     | — (no key)            | NSE stocks (`.NS`), NSE indices |
//! | BSE website   | `bse`                     | — (no key)            | BSE stocks (`.BO`, scrip codes) |
//! | NSE + BSE     | `exchange`                | — (no key)            | `.NS` and NSE indices on NSE, `.BO` on BSE |
//!
//! The exchange providers read the JSON endpoints behind nseindia.com and
//! bseindia.com through one polite session per site (`session.rs`).

pub mod alphavantage;
pub mod bse;
pub mod massive;
pub mod nse;
pub mod session;
pub mod twelvedata;

use crate::api::{ChartData, ChartMeta, Indicators, Quote, QuoteIndicator, SearchResult};
use anyhow::{Context, Result, bail};
use chrono::{Datelike, NaiveDate};
use std::future::Future;
use std::sync::Mutex;
use std::time::Duration;
use std::time::Instant;

/// A source of quotes, price history and symbol search. Symbols are always
/// Yahoo-style (`AAPL`, `RELIANCE.BO`, `BTC-USD`, `USDINR=X`); each provider
/// maps them to its own syntax and back.
pub trait MarketData {
    /// Human-readable provider name.
    fn name(&self) -> &'static str;

    /// Latest quotes. Symbols the provider can't price are left out.
    fn quotes(&self, symbols: &[&str]) -> impl Future<Output = Result<Vec<Quote>>> + Send;

    /// Price history for a Yahoo `range` (`1mo`, `1y`, ...) and `interval`
    /// (`1d`, `1wk`, `1mo`, ...).
    fn chart(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> impl Future<Output = Result<ChartData>> + Send;

    /// Find symbols by name or ticker.
    fn search(&self, query: &str) -> impl Future<Output = Result<Vec<SearchResult>>> + Send;
}

/// The alternative providers. Yahoo is not listed: it is the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    AlphaVantage,
    Massive,
    TwelveData,
    /// NSE's website (no key).
    Nse,
    /// BSE's website (no key).
    Bse,
    /// NSE for `.NS` and NSE indices, BSE for `.BO`, Yahoo for the rest.
    Exchange,
}

impl ProviderKind {
    pub fn parse(value: &str) -> Option<Self> {
        match value
            .trim()
            .to_ascii_lowercase()
            .replace(['-', '_', ' '], "")
            .as_str()
        {
            "alphavantage" | "av" => Some(ProviderKind::AlphaVantage),
            "massive" | "polygon" | "polygonio" => Some(ProviderKind::Massive),
            "twelvedata" | "12data" => Some(ProviderKind::TwelveData),
            "nse" | "nseindia" => Some(ProviderKind::Nse),
            "bse" | "bseindia" => Some(ProviderKind::Bse),
            "exchange" | "exchanges" | "nsebse" | "nse+bse" => Some(ProviderKind::Exchange),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ProviderKind::AlphaVantage => "Alpha Vantage",
            ProviderKind::Massive => "Massive (Polygon.io)",
            ProviderKind::TwelveData => "Twelve Data",
            ProviderKind::Nse => "NSE",
            ProviderKind::Bse => "BSE",
            ProviderKind::Exchange => "NSE + BSE",
        }
    }

    /// Variables checked for the API key, preferred first; empty for the
    /// exchanges, which need none.
    pub fn key_envs(self) -> &'static [&'static str] {
        match self {
            ProviderKind::AlphaVantage => &["ALPHAVANTAGE_API_KEY", "ALPHA_VANTAGE_API_KEY"],
            ProviderKind::Massive => &["MASSIVE_API_KEY", "POLYGON_API_KEY"],
            ProviderKind::TwelveData => &["TWELVEDATA_API_KEY", "TWELVE_DATA_API_KEY"],
            ProviderKind::Nse | ProviderKind::Bse | ProviderKind::Exchange => &[],
        }
    }

    pub fn key_env(self) -> &'static str {
        self.key_envs().first().copied().unwrap_or_default()
    }

    /// Whether the provider needs an API key.
    pub fn needs_key(self) -> bool {
        !self.key_envs().is_empty()
    }

    /// Provider requests allowed per time window before the rest of the
    /// symbols go to Yahoo, and how long the window is. Free tiers are tiny
    /// (Alpha Vantage: 25 a day; Massive: 5 a minute; Twelve Data: 8 a
    /// minute), so a scan of 30 stocks must not burn — or wait on — the whole
    /// allowance: one command (a CLI run lasts seconds) gets one window's
    /// worth. A long-running `serve`, `tui`, `mcp` or daemon gets the same
    /// allowance again when the window ends. Windows start with the first
    /// request in them, so two can sit back to back with a burst at the end of
    /// one and the start of the next: Alpha Vantage's 25 a day is 4 every
    /// 4 h 48 min, whose worst case in any 24 hours is 23 (5 would reach 29).
    fn allowance(self) -> (u32, Duration) {
        const MINUTE: Duration = Duration::from_secs(60);
        match self {
            ProviderKind::AlphaVantage => (4, Duration::from_secs(17_280)),
            ProviderKind::Massive => (5, MINUTE),
            ProviderKind::TwelveData => (8, MINUTE),
            ProviderKind::Nse => (nse::QUOTE_BATCH, MINUTE),
            ProviderKind::Bse => (bse::QUOTE_BATCH, MINUTE),
            ProviderKind::Exchange => (nse::QUOTE_BATCH, MINUTE),
        }
    }

    /// Calendar days of daily history the free plan returns, or `None` when
    /// it reaches back years (Twelve Data: up to 5000 bars). Longer ranges —
    /// and `max` — go to Yahoo without spending a request.
    fn history_days(self) -> Option<i64> {
        match self {
            // `outputsize=compact`: the latest 100 trading days.
            ProviderKind::AlphaVantage => Some(140),
            ProviderKind::Massive => Some(730),
            ProviderKind::TwelveData => None,
            ProviderKind::Nse | ProviderKind::Exchange => Some(nse::HISTORY_DAYS),
            ProviderKind::Bse => Some(bse::HISTORY_DAYS),
        }
    }
}

/// What `STOCKWISE_DATA_PROVIDER` asks for.
pub enum Selection {
    /// Not set (or `yahoo`): Yahoo only.
    Yahoo,
    Ready(Provider),
    /// A provider is named but its key variable is empty.
    MissingKey(ProviderKind),
    /// The value names no provider.
    Unknown(String),
}

impl Selection {
    /// From the environment; the provider name may also come from
    /// `data_provider` in the config file (API keys only from the environment).
    pub fn from_env() -> Self {
        Self::from_lookup(|k| {
            if k == "STOCKWISE_DATA_PROVIDER" {
                crate::config::setting(k, |c| c.data_provider.as_ref())
            } else {
                std::env::var(k).ok()
            }
        })
    }

    fn from_lookup(env: impl Fn(&str) -> Option<String>) -> Self {
        let raw = env("STOCKWISE_DATA_PROVIDER").unwrap_or_default();
        let raw = raw.trim();
        if raw.is_empty() || raw.eq_ignore_ascii_case("yahoo") {
            return Selection::Yahoo;
        }
        let Some(kind) = ProviderKind::parse(raw) else {
            return Selection::Unknown(raw.to_string());
        };
        if !kind.needs_key() {
            return Selection::Ready(Provider::new(kind, String::new(), None));
        }
        let key = kind
            .key_envs()
            .iter()
            .filter_map(|k| env(k))
            .map(|k| k.trim().to_string())
            .find(|k| !k.is_empty());
        match key {
            Some(key) => Selection::Ready(Provider::new(kind, key, None)),
            None => Selection::MissingKey(kind),
        }
    }
}

/// A configured alternative provider.
pub enum Provider {
    AlphaVantage(alphavantage::AlphaVantage),
    Massive(massive::Massive),
    TwelveData(twelvedata::TwelveData),
    Nse(nse::Nse),
    Bse(bse::Bse),
    Exchange(Exchange),
}

/// `STOCKWISE_DATA_PROVIDER=exchange`: each Indian symbol from its own
/// exchange.
pub struct Exchange {
    pub nse: nse::Nse,
    pub bse: bse::Bse,
}

impl Provider {
    /// `base_url` overrides the API endpoint (tests use a local mock).
    pub fn new(kind: ProviderKind, key: String, base_url: Option<String>) -> Self {
        let nse = || {
            nse::Nse::new(match &base_url {
                Some(base) => std::sync::Arc::new(session::Session::new(
                    nse::site(base),
                    session::Limiter::polite(),
                )),
                None => nse::shared_session(),
            })
        };
        let bse = || {
            bse::Bse::new(match &base_url {
                Some(base) => std::sync::Arc::new(session::Session::new(
                    bse::site(base),
                    session::Limiter::polite(),
                )),
                None => bse::shared_session(),
            })
        };
        let http = |default: &str| {
            Http::new(
                kind,
                key.clone(),
                base_url.clone().unwrap_or_else(|| default.into()),
            )
        };
        match kind {
            ProviderKind::AlphaVantage => Provider::AlphaVantage(alphavantage::AlphaVantage::new(
                http(alphavantage::BASE_URL),
            )),
            ProviderKind::Massive => {
                Provider::Massive(massive::Massive::new(http(massive::BASE_URL)))
            }
            ProviderKind::TwelveData => {
                Provider::TwelveData(twelvedata::TwelveData::new(http(twelvedata::BASE_URL)))
            }
            ProviderKind::Nse => Provider::Nse(nse()),
            ProviderKind::Bse => Provider::Bse(bse()),
            ProviderKind::Exchange => Provider::Exchange(Exchange {
                nse: nse(),
                bse: bse(),
            }),
        }
    }

    /// The key-based providers' HTTP state; `None` for the exchanges.
    fn http(&self) -> Option<&Http> {
        match self {
            Provider::AlphaVantage(p) => Some(&p.http),
            Provider::Massive(p) => Some(&p.http),
            Provider::TwelveData(p) => Some(&p.http),
            Provider::Nse(_) | Provider::Bse(_) | Provider::Exchange(_) => None,
        }
    }

    pub fn kind(&self) -> ProviderKind {
        match self {
            Provider::Nse(_) => ProviderKind::Nse,
            Provider::Bse(_) => ProviderKind::Bse,
            Provider::Exchange(_) => ProviderKind::Exchange,
            Provider::AlphaVantage(p) => p.http.kind,
            Provider::Massive(p) => p.http.kind,
            Provider::TwelveData(p) => p.http.kind,
        }
    }

    /// Share the Redis/in-memory cache, so free-tier calls survive between
    /// runs.
    pub fn with_cache(mut self, cache: crate::api::SharedCache) -> Self {
        match &mut self {
            Provider::AlphaVantage(p) => p.http.cache = Some(cache),
            Provider::Massive(p) => p.http.cache = Some(cache),
            Provider::TwelveData(p) => p.http.cache = Some(cache),
            Provider::Nse(p) => p.session().set_cache(cache),
            Provider::Bse(p) => p.session().set_cache(cache),
            Provider::Exchange(p) => {
                p.nse.session().set_cache(cache.clone());
                p.bse.session().set_cache(cache);
            }
        }
        self
    }

    /// Whether this provider can price `symbol` at all.
    pub fn supports(&self, symbol: &str) -> bool {
        match self {
            Provider::AlphaVantage(_) => alphavantage::target(symbol).is_some(),
            Provider::Massive(_) => massive::ticker(symbol).is_some(),
            Provider::TwelveData(_) => twelvedata::target(symbol).is_some(),
            Provider::Nse(p) => p.supports(symbol),
            Provider::Bse(p) => p.supports(symbol),
            Provider::Exchange(p) => p.nse.supports(symbol) || p.bse.supports(symbol),
        }
    }

    /// Requests left in the current window before quotes fall back to Yahoo. For the
    /// exchanges, how many uncached symbols one quote request may ask for
    /// (0 after a refusal).
    pub fn remaining(&self) -> u32 {
        match self {
            Provider::Nse(p) => p.remaining(),
            Provider::Bse(p) => p.remaining(),
            Provider::Exchange(p) => p.nse.remaining().max(p.bse.remaining()),
            _ => {
                let Some(http) = self.http() else { return 0 };
                http.remaining()
            }
        }
    }

    /// Whether one quote request for `symbols` (all supported) fits what
    /// the provider has left; if not, the whole request goes to Yahoo so a
    /// table never mixes sources. Exchange quotes already fetched with an
    /// index snapshot cost nothing.
    pub fn can_serve(&self, symbols: &[&str]) -> bool {
        match self {
            Provider::Nse(p) => p.quote_cost(symbols) <= p.remaining(),
            Provider::Bse(p) => p.quote_cost(symbols) <= p.remaining(),
            Provider::Exchange(p) => {
                let (ns, bo): (Vec<&str>, Vec<&str>) =
                    symbols.iter().partition(|s| p.nse.supports(s));
                (ns.is_empty() || p.nse.quote_cost(&ns) <= p.nse.remaining())
                    && (bo.is_empty() || p.bse.quote_cost(&bo) <= p.bse.remaining())
            }
            _ => symbols.len() as u32 <= self.remaining(),
        }
    }

    /// `(source, symbol)` pairs `doctor` asks for to show the provider
    /// answers — one per exchange in mixed mode.
    pub fn probes(&self) -> Vec<(&'static str, &'static str)> {
        match self {
            Provider::Nse(_) => vec![("NSE", "RELIANCE.NS")],
            Provider::Bse(_) => vec![("BSE", "500325.BO")],
            Provider::Exchange(_) => vec![("NSE", "RELIANCE.NS"), ("BSE", "500325.BO")],
            _ => vec![(self.name(), "AAPL")],
        }
    }

    /// The NSE side of the provider, if it has one (for index members).
    pub fn nse(&self) -> Option<&nse::Nse> {
        match self {
            Provider::Nse(p) => Some(p),
            Provider::Exchange(p) => Some(&p.nse),
            _ => None,
        }
    }
}

impl MarketData for Provider {
    fn name(&self) -> &'static str {
        self.kind().name()
    }

    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        match self {
            Provider::Nse(p) => return p.quotes(symbols).await,
            Provider::Bse(p) => return p.quotes(symbols).await,
            Provider::Exchange(p) => return p.quotes(symbols).await,
            _ => {}
        }
        let mut out = Vec::new();
        for symbol in symbols {
            let quote = match self {
                Provider::AlphaVantage(p) => p.quote(symbol).await,
                Provider::Massive(p) => p.quote(symbol).await,
                Provider::TwelveData(p) => p.quote(symbol).await,
                Provider::Nse(_) | Provider::Bse(_) | Provider::Exchange(_) => unreachable!(),
            };
            match quote {
                Ok(Some(mut q)) => {
                    q.symbol = Some(symbol.to_string());
                    q.data_source = Some(self.name().to_string());
                    out.push(q);
                }
                Ok(None) => {}
                Err(e) => {
                    // Rate limit, refused key or budget gone: keep what we
                    // have, the caller asks Yahoo for the rest.
                    if self.remaining() == 0 {
                        if out.is_empty() {
                            return Err(e);
                        }
                        break;
                    }
                    // A single symbol is worth an error (doctor shows it);
                    // in a list, one bad symbol is skipped.
                    if symbols.len() == 1 {
                        return Err(e);
                    }
                }
            }
        }
        Ok(out)
    }

    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        match self {
            Provider::Nse(p) => return p.chart(symbol, range, interval).await,
            Provider::Bse(p) => return p.chart(symbol, range, interval).await,
            Provider::Exchange(p) if p.nse.supports(symbol) => {
                return p.nse.chart(symbol, range, interval).await;
            }
            Provider::Exchange(p) => return p.bse.chart(symbol, range, interval).await,
            _ => {}
        }
        let Some(step) = Resample::for_interval(interval) else {
            bail!("{} serves daily, weekly and monthly bars only", self.name());
        };
        let today = chrono::Utc::now().date_naive();
        let start = range_start(range, today);
        if let Some(days) = self.kind().history_days() {
            let oldest = today - chrono::Duration::days(days);
            if start.is_none_or(|s| s < oldest) {
                bail!(
                    "{} keeps {} days of history on the free plan",
                    self.name(),
                    days
                );
            }
        }
        let series = match self {
            Provider::AlphaVantage(p) => p.daily(symbol, start).await?,
            Provider::Massive(p) => p.daily(symbol, start).await?,
            Provider::TwelveData(p) => p.daily(symbol, start).await?,
            Provider::Nse(_) | Provider::Bse(_) | Provider::Exchange(_) => unreachable!(),
        };
        let series = series.since(start);
        if !series.covers(start) {
            bail!(
                "{} has no history back to {} for {} on this plan",
                self.name(),
                start.map_or("the start".into(), |d| d.to_string()),
                symbol
            );
        }
        Ok(series.resample(step).into_chart(symbol))
    }

    async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        match self {
            Provider::AlphaVantage(p) => p.search(query).await,
            Provider::Massive(p) => p.search(query).await,
            Provider::TwelveData(p) => p.search(query).await,
            Provider::Nse(p) => p.search(query).await,
            Provider::Bse(p) => p.search(query).await,
            Provider::Exchange(p) => p.search(query).await,
        }
    }
}

impl Exchange {
    /// NSE and BSE quotes, in request order.
    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let (ns, bo): (Vec<&str>, Vec<&str>) = symbols
            .iter()
            .filter(|s| self.nse.supports(s) || self.bse.supports(s))
            .partition(|s| self.nse.supports(s));
        let from_nse = if ns.is_empty() {
            Ok(Vec::new())
        } else {
            self.nse.quotes(&ns).await
        };
        let from_bse = if bo.is_empty() {
            Ok(Vec::new())
        } else {
            self.bse.quotes(&bo).await
        };
        let mut all = match (from_nse, from_bse) {
            (Err(e), Err(_)) => return Err(e),
            (Err(e), Ok(b)) if b.is_empty() => return Err(e),
            (Ok(n), Err(e)) if n.is_empty() => return Err(e),
            (n, b) => n
                .unwrap_or_default()
                .into_iter()
                .chain(b.unwrap_or_default())
                .collect::<Vec<_>>(),
        };
        let mut out = Vec::with_capacity(all.len());
        for symbol in symbols {
            if let Some(i) = all.iter().position(|q| {
                q.symbol
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case(symbol))
            }) {
                out.push(all.remove(i));
            }
        }
        Ok(out)
    }

    /// NSE's matches first, then BSE's, ten in all.
    async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let from_nse = self.nse.search(query).await;
        let from_bse = self.bse.search(query).await;
        let (n, b) = match (from_nse, from_bse) {
            (Err(e), Err(_)) => return Err(e),
            (n, b) => (n.unwrap_or_default(), b.unwrap_or_default()),
        };
        // Leave BSE up to three places when NSE fills the list.
        let nse_places = 10 - b.len().min(3);
        let mut out: Vec<SearchResult> = n.into_iter().take(nse_places).chain(b).collect();
        out.truncate(10);
        Ok(out)
    }
}

/// How long a provider is left alone after a network error, and after it
/// refused the key.
const NETWORK_PAUSE: Duration = Duration::from_secs(180);
const DENIED_PAUSE: Duration = Duration::from_secs(3600);

/// A provider's request allowance: `limit` requests per `window`, counted
/// from the first request in it, plus a pause while the provider is being
/// left alone. Times are passed in, so tests can move the clock.
#[derive(Debug)]
struct Allowance {
    limit: u32,
    window: Duration,
    started: Option<Instant>,
    used: u32,
    paused_until: Option<Instant>,
    /// The fallback notice for the current pause has been printed.
    noticed: bool,
}

impl Allowance {
    fn new(limit: u32, window: Duration) -> Self {
        Self {
            limit,
            window,
            started: None,
            used: 0,
            paused_until: None,
            noticed: false,
        }
    }

    /// Start a new window when the last one is over, and end a pause that
    /// has run out (the next one is announced again).
    fn refresh(&mut self, now: Instant) {
        if self.started.is_some_and(|s| now >= s + self.window) {
            self.started = None;
            self.used = 0;
        }
        if self.paused_until.is_some_and(|p| now >= p) {
            self.paused_until = None;
            self.noticed = false;
        }
    }

    fn remaining(&mut self, now: Instant) -> u32 {
        self.refresh(now);
        if self.paused_until.is_some() {
            0
        } else {
            self.limit.saturating_sub(self.used)
        }
    }

    /// Spend one request, or say why none is left.
    fn take(&mut self, now: Instant) -> Result<(), &'static str> {
        self.refresh(now);
        if self.paused_until.is_some() {
            return Err("is paused for now");
        }
        if self.used >= self.limit {
            // Used up: wait for the window to end instead of asking again.
            self.paused_until = self.started.map(|s| s + self.window);
            return Err("request allowance used up");
        }
        self.started.get_or_insert(now);
        self.used += 1;
        Ok(())
    }

    fn pause(&mut self, now: Instant, for_: Duration) {
        let until = now + for_;
        self.paused_until = Some(self.paused_until.map_or(until, |p| p.max(until)));
    }

    /// True the first time it is called in each pause.
    fn announce(&mut self) -> bool {
        !std::mem::replace(&mut self.noticed, true)
    }

    /// Tests: move every recorded time `by` into the past.
    #[cfg(test)]
    fn rewind(&mut self, by: Duration) {
        self.started = self.started.map(|s| s - by);
        self.paused_until = self.paused_until.map(|p| p - by);
    }
}

/// When an end-of-day quote for trading day `date` was struck, as Unix
/// seconds: the close of that day on the symbol's exchange, so the quote's
/// trade date (the exchange's calendar day of this time) is `date`. Symbols
/// with no exchange calendar (crypto, FX) and days the calendar lists as
/// closed get noon UTC of `date`.
pub fn trade_time(symbol: &str, date: NaiveDate) -> Option<i64> {
    crate::alerts::alert_market(symbol)
        .and_then(|m| crate::clock::close_at(m, date))
        .map(|t| t.timestamp())
        .or_else(|| date.and_hms_opt(12, 0, 0).map(|t| t.and_utc().timestamp()))
}

/// Overlay a provider quote on Yahoo's quote for the same symbol. Price and
/// day fields come from the provider; everything it lacks (name, 52-week
/// range, fundamentals, currency) stays Yahoo's. If the provider gave a price
/// but no change, the change is recomputed against Yahoo's previous close so
/// price and change never disagree. The trade time comes with the price: an
/// end-of-day quote from yesterday keeps yesterday's date instead of
/// borrowing the time of Yahoo's live trade.
pub fn merge_quote(provider: Quote, yahoo: Option<Quote>) -> Quote {
    let Some(mut q) = yahoo else {
        return provider;
    };
    macro_rules! take {
        ($($field:ident),*) => {
            $(if provider.$field.is_some() { q.$field = provider.$field.clone(); })*
        };
    }
    take!(
        regular_market_price,
        regular_market_time,
        regular_market_open,
        regular_market_day_high,
        regular_market_day_low,
        regular_market_volume,
        regular_market_previous_close,
        fifty_two_week_high,
        fifty_two_week_low,
        average_daily_volume_3_month
    );
    if provider.regular_market_change.is_some() {
        q.regular_market_change = provider.regular_market_change;
        q.regular_market_change_percent = provider.regular_market_change_percent;
    } else if let (Some(price), Some(prev)) =
        (q.regular_market_price, q.regular_market_previous_close)
        && prev != 0.0
    {
        q.regular_market_change = Some(price - prev);
        q.regular_market_change_percent = Some((price - prev) / prev * 100.0);
    }
    if q.currency.is_none() {
        q.currency = provider.currency;
    }
    if q.short_name.is_none() && q.long_name.is_none() {
        q.short_name = provider.short_name;
    }
    if q.exchange.is_none() {
        q.exchange = provider.exchange;
    }
    // Exchange-only fields: Yahoo never has them.
    q.delivery_percent = provider.delivery_percent;
    q.upper_circuit = provider.upper_circuit;
    q.lower_circuit = provider.lower_circuit;
    q.data_source = provider.data_source;
    q
}

// ── HTTP ──

/// HTTP for one provider: shared cache, per-run request budget, and an
/// off-switch once the provider reports its limit. The API key never ends up
/// in a cache key or an error message.
pub struct Http {
    kind: ProviderKind,
    client: reqwest::Client,
    base: String,
    key: String,
    cache: Option<crate::api::SharedCache>,
    allowance: Mutex<Allowance>,
}

/// How a provider wants its key.
pub enum Auth {
    /// `?name=KEY` (Alpha Vantage has no header option).
    Query(&'static str),
    /// `name: prefix KEY`.
    Header(&'static str, &'static str),
}

impl Http {
    fn new(kind: ProviderKind, key: String, base: String) -> Self {
        Self {
            kind,
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(20))
                .build()
                .expect("HTTP client builds"),
            base: base.trim_end_matches('/').to_string(),
            key,
            cache: None,
            allowance: Mutex::new({
                let (limit, window) = kind.allowance();
                Allowance::new(limit, window)
            }),
        }
    }

    fn allowance(&self) -> std::sync::MutexGuard<'_, Allowance> {
        self.allowance.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Requests left in this window (0 while paused).
    pub fn remaining(&self) -> u32 {
        self.allowance().remaining(Instant::now())
    }

    /// Stop using the provider for `for_`, and say so on stderr once per
    /// pause: quotes and charts come from Yahoo meanwhile.
    fn pause(&self, for_: Duration, why: String) {
        let first = {
            let mut a = self.allowance();
            a.pause(Instant::now(), for_);
            a.announce()
        };
        if first {
            eprintln!(
                "  \u{24d8} {}: {} — using Yahoo Finance instead, trying again in {}",
                self.kind.name(),
                why,
                format_wait(for_)
            );
        }
    }

    /// GET `path?query` and return the JSON body. `check` inspects a parsed
    /// body for the provider's in-band errors (HTTP 200 with an error
    /// message); only bodies that pass are cached.
    pub async fn get_json(
        &self,
        path: &str,
        query: &[(&str, &str)],
        auth: Auth,
        ttl: Duration,
        check: fn(&serde_json::Value) -> Result<(), ProviderError>,
    ) -> Result<serde_json::Value> {
        let name = self.kind.name();
        let mut url = reqwest::Url::parse(&format!("{}{}", self.base, path))
            .with_context(|| format!("bad {} URL", name))?;
        url.query_pairs_mut().extend_pairs(query);
        let cache_key = format!("provider:{}:{}", self.kind.key_env(), url);

        if let Some(cache) = &self.cache
            && let Some(body) = cache.lock().await.get(&cache_key).await
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
        {
            return Ok(json);
        }

        // Take one request from the allowance.
        let taken = self.allowance().take(Instant::now());
        if let Err(why) = taken {
            if why == "request allowance used up" {
                let wait = {
                    let a = self.allowance();
                    a.paused_until.map_or(Duration::ZERO, |p| {
                        p.saturating_duration_since(Instant::now())
                    })
                };
                self.pause(wait, format!("{why} (free tier)"));
            }
            bail!("{} {}", name, why);
        }

        let mut send_url = url;
        if let Auth::Query(param) = auth {
            send_url.query_pairs_mut().append_pair(param, &self.key);
        }
        let mut req = self.client.get(send_url);
        if let Auth::Header(header, prefix) = auth {
            req = req.header(header, format!("{}{}", prefix, self.key));
        }
        let resp = match req.send().await {
            Ok(resp) => resp,
            // Unreachable or timing out: don't make every remaining symbol
            // wait for it too.
            Err(e) => {
                let msg = format!("request failed ({})", e.without_url());
                self.pause(NETWORK_PAUSE, msg.clone());
                bail!("{}: {}", name, msg);
            }
        };
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("{} response unreadable: {}", name, e.without_url()))?;
        let json: serde_json::Value =
            serde_json::from_str(&body).unwrap_or(serde_json::Value::Null);

        let verdict = if status.as_u16() == 429 {
            Err(ProviderError::RateLimited(
                error_text(&json).unwrap_or("HTTP 429").into(),
            ))
        } else if status.as_u16() == 401 || status.as_u16() == 403 {
            Err(ProviderError::Denied(
                error_text(&json)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
            ))
        } else if !status.is_success() {
            Err(ProviderError::Other(
                error_text(&json)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("HTTP {}", status.as_u16())),
            ))
        } else if json.is_null() {
            Err(ProviderError::Other("response is not JSON".into()))
        } else {
            check(&json)
        };

        match verdict {
            Ok(()) => {
                if let Some(cache) = &self.cache {
                    cache.lock().await.set(cache_key, body, ttl).await;
                }
                Ok(json)
            }
            Err(ProviderError::Other(msg)) => bail!("{}: {}", name, redact(&msg, &self.key)),
            Err(e) => {
                // Rate limit: stop asking until the window is over. Refused
                // key: stop for a good while.
                let pause = match e {
                    ProviderError::RateLimited(_) => self.allowance().window,
                    _ => DENIED_PAUSE,
                };
                let msg = redact(&e.to_string(), &self.key);
                self.pause(pause, msg.clone());
                bail!("{}: {}", name, msg)
            }
        }
    }
}

/// Errors a provider can report inside an HTTP 200 body, or by status.
#[derive(Debug)]
pub enum ProviderError {
    RateLimited(String),
    Denied(String),
    Other(String),
}

impl std::fmt::Display for ProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderError::RateLimited(m) => write!(f, "rate limit reached ({})", m),
            ProviderError::Denied(m) => write!(f, "access denied ({})", m),
            ProviderError::Other(m) => write!(f, "{}", m),
        }
    }
}

/// "45 s", "3 min", "2 h" for a notice.
fn format_wait(d: Duration) -> String {
    let secs = d.as_secs().max(1);
    match secs {
        0..=89 => format!("{secs} s"),
        90..=5399 => format!("{} min", secs.div_ceil(60)),
        _ => format!("{} h", secs.div_ceil(3600)),
    }
}

/// A message from a provider error body: `message`, `error`, or the
/// Alpha Vantage `Information` / `Note` / `Error Message` keys.
fn error_text(json: &serde_json::Value) -> Option<&str> {
    ["message", "error", "Information", "Note", "Error Message"]
        .iter()
        .find_map(|k| json[*k].as_str())
}

fn redact(text: &str, key: &str) -> String {
    if key.len() >= 4 {
        text.replace(key, "<key>")
    } else {
        text.to_string()
    }
}

/// Parse a JSON number or numeric string (`"236.3900"`, `"1.83%"`).
fn num(v: &serde_json::Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| {
            v.as_str()
                .and_then(|s| s.trim().trim_end_matches('%').parse::<f64>().ok())
        })
        .filter(|x| x.is_finite())
}

fn int(v: &serde_json::Value) -> Option<u64> {
    num(v).filter(|x| *x >= 0.0).map(|x| x as u64)
}

// ── Daily series → ChartData ──

/// One daily bar, dated (no time of day).
#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub date: NaiveDate,
    pub open: Option<f64>,
    pub high: Option<f64>,
    pub low: Option<f64>,
    pub close: Option<f64>,
    pub volume: Option<u64>,
}

/// Daily bars in date order, plus what the provider said about currency.
#[derive(Clone, Debug, Default)]
pub struct Series {
    pub bars: Vec<Bar>,
    pub currency: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Resample {
    Day,
    Week,
    Month,
}

impl Resample {
    /// Provider charts are built from daily bars; intraday intervals stay on
    /// Yahoo (free tiers don't serve them).
    fn for_interval(interval: &str) -> Option<Self> {
        match interval {
            "1d" => Some(Resample::Day),
            "5d" | "1wk" => Some(Resample::Week),
            "1mo" | "3mo" => Some(Resample::Month),
            _ => None,
        }
    }
}

/// First day a Yahoo `range` covers, or `None` for `max`.
pub fn range_start(range: &str, today: NaiveDate) -> Option<NaiveDate> {
    let days = match range {
        "1d" => 1,
        "5d" | "1w" => 7,
        "1mo" => 31,
        "3mo" => 92,
        "6mo" => 183,
        "1y" => 366,
        "2y" => 731,
        "5y" => 1827,
        "10y" => 3653,
        "ytd" => return NaiveDate::from_ymd_opt(today.year(), 1, 1),
        _ => return None,
    };
    Some(today - chrono::Duration::days(days))
}

impl Series {
    pub fn sorted(mut self) -> Self {
        self.bars.sort_by_key(|b| b.date);
        self.bars.dedup_by_key(|b| b.date);
        self
    }

    fn since(mut self, start: Option<NaiveDate>) -> Self {
        if let Some(start) = start {
            self.bars.retain(|b| b.date >= start);
        }
        self
    }

    /// Whether the bars reach back to `start`. A week of slack covers
    /// weekends and holidays at the start of a range.
    fn covers(&self, start: Option<NaiveDate>) -> bool {
        let Some(first) = self.bars.first() else {
            return false;
        };
        match start {
            Some(start) => first.date <= start + chrono::Duration::days(7),
            None => true,
        }
    }

    fn resample(self, step: Resample) -> Self {
        if step == Resample::Day {
            return self;
        }
        let bucket = |d: NaiveDate| match step {
            Resample::Week => d - chrono::Duration::days(d.weekday().num_days_from_monday() as i64),
            _ => NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap_or(d),
        };
        let mut out: Vec<Bar> = Vec::new();
        for bar in self.bars {
            match out.last_mut() {
                Some(last) if bucket(last.date) == bucket(bar.date) => {
                    last.high = max_opt(last.high, bar.high);
                    last.low = min_opt(last.low, bar.low);
                    last.close = bar.close.or(last.close);
                    last.volume = match (last.volume, bar.volume) {
                        (Some(a), Some(b)) => Some(a + b),
                        (a, b) => a.or(b),
                    };
                }
                _ => out.push(bar),
            }
        }
        Self {
            bars: out,
            currency: self.currency,
        }
    }

    /// Yahoo-shaped chart data. Each bar is stamped at 12:00 UTC on its date,
    /// so it shows as the same day in every time zone from UTC-12 to UTC+12.
    fn into_chart(self, symbol: &str) -> ChartData {
        let noon = |d: NaiveDate| {
            d.and_hms_opt(12, 0, 0)
                .map(|t| t.and_utc().timestamp())
                .unwrap_or_default()
        };
        let last_close = self.bars.iter().rev().find_map(|b| b.close);
        ChartData {
            meta: Some(ChartMeta {
                regular_market_price: last_close,
                chart_previous_close: None,
                symbol: Some(symbol.to_string()),
                long_name: None,
                short_name: None,
                regular_market_day_high: self.bars.last().and_then(|b| b.high),
                regular_market_day_low: self.bars.last().and_then(|b| b.low),
                regular_market_volume: self.bars.last().and_then(|b| b.volume),
                fifty_two_week_high: None,
                fifty_two_week_low: None,
                currency: self.currency,
                exchange_name: None,
                gmtoffset: None,
            }),
            timestamp: Some(self.bars.iter().map(|b| noon(b.date)).collect()),
            indicators: Indicators {
                quote: vec![QuoteIndicator {
                    open: Some(self.bars.iter().map(|b| b.open).collect()),
                    high: Some(self.bars.iter().map(|b| b.high).collect()),
                    low: Some(self.bars.iter().map(|b| b.low).collect()),
                    close: Some(self.bars.iter().map(|b| b.close).collect()),
                    volume: Some(self.bars.iter().map(|b| b.volume).collect()),
                }],
            },
            // Dividends/splits come from Yahoo (`get_corporate_actions`).
            events: None,
        }
    }
}

/// One intraday bar, stamped with its start (Unix seconds).
#[derive(Clone, Debug, PartialEq)]
pub struct TimedBar {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: Option<u64>,
}

/// Yahoo-shaped chart data from intraday bars.
fn timed_chart(symbol: &str, bars: &[TimedBar], currency: Option<&str>) -> ChartData {
    let last = bars.last();
    ChartData {
        meta: Some(ChartMeta {
            regular_market_price: last.map(|b| b.close),
            chart_previous_close: None,
            symbol: Some(symbol.to_string()),
            long_name: None,
            short_name: None,
            regular_market_day_high: bars.iter().map(|b| b.high).reduce(f64::max),
            regular_market_day_low: bars.iter().map(|b| b.low).reduce(f64::min),
            regular_market_volume: None,
            fifty_two_week_high: None,
            fifty_two_week_low: None,
            currency: currency.map(str::to_string),
            exchange_name: None,
            gmtoffset: None,
        }),
        timestamp: Some(bars.iter().map(|b| b.ts).collect()),
        indicators: Indicators {
            quote: vec![QuoteIndicator {
                open: Some(bars.iter().map(|b| Some(b.open)).collect()),
                high: Some(bars.iter().map(|b| Some(b.high)).collect()),
                low: Some(bars.iter().map(|b| Some(b.low)).collect()),
                close: Some(bars.iter().map(|b| Some(b.close)).collect()),
                volume: Some(bars.iter().map(|b| b.volume).collect()),
            }],
        },
        events: None,
    }
}

/// Split one CSV line into fields (double quotes group, `""` escapes a
/// quote).
fn csv_fields(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.trim_end_matches(['\r', '\n']).chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    out.push(cur);
    out
}

fn max_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (x, y) => x.or(y),
    }
}

fn min_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, y) => x.or(y),
    }
}

/// A quote from the last two daily bars: the latest bar's prices, with the
/// change against the bar before it.
fn quote_from_bars(series: &Series) -> Option<Quote> {
    let last = series.bars.last()?;
    let prev_close = series
        .bars
        .len()
        .checked_sub(2)
        .and_then(|i| series.bars[i].close);
    let price = last.close?;
    let mut q = Quote {
        regular_market_price: Some(price),
        regular_market_open: last.open,
        regular_market_day_high: last.high,
        regular_market_day_low: last.low,
        regular_market_volume: last.volume,
        regular_market_previous_close: prev_close,
        currency: series.currency.clone(),
        ..Default::default()
    };
    if let Some(prev) = prev_close
        && prev != 0.0
    {
        q.regular_market_change = Some(price - prev);
        q.regular_market_change_percent = Some((price - prev) / prev * 100.0);
    }
    Some(q)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::MockServer;
    use std::sync::atomic::Ordering;

    /// A Massive aggregates answer with two daily bars.
    const MASSIVE_BAR: &str = r#"{"results":[{"c":75.0875,"h":75.15,"l":73.7975,"o":74.06,"t":1577941200000,"v":135647456},{"c":74.3575,"h":75.145,"l":74.125,"o":74.2875,"t":1578027600000,"v":146535512}],"status":"OK"}"#;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn bar(date: &str, close: f64) -> Bar {
        Bar {
            date: d(date),
            open: Some(close - 1.0),
            high: Some(close + 2.0),
            low: Some(close - 2.0),
            close: Some(close),
            volume: Some(100),
        }
    }

    #[test]
    fn selection_reads_provider_and_key() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert!(matches!(Selection::from_lookup(env(&[])), Selection::Yahoo));
        assert!(matches!(
            Selection::from_lookup(env(&[("STOCKWISE_DATA_PROVIDER", "Yahoo")])),
            Selection::Yahoo
        ));
        assert!(matches!(
            Selection::from_lookup(env(&[("STOCKWISE_DATA_PROVIDER", "polygon")])),
            Selection::MissingKey(ProviderKind::Massive)
        ));
        match Selection::from_lookup(env(&[
            ("STOCKWISE_DATA_PROVIDER", "alpha-vantage"),
            ("ALPHA_VANTAGE_API_KEY", " k123 "),
        ])) {
            Selection::Ready(p) => assert_eq!(p.kind(), ProviderKind::AlphaVantage),
            _ => panic!("expected a ready provider"),
        }
        assert!(matches!(
            Selection::from_lookup(env(&[("STOCKWISE_DATA_PROVIDER", "bloomberg")])),
            Selection::Unknown(v) if v == "bloomberg"
        ));
        // The exchanges need no key.
        for (name, kind) in [
            ("nse", ProviderKind::Nse),
            ("BSE", ProviderKind::Bse),
            ("exchange", ProviderKind::Exchange),
            ("nse+bse", ProviderKind::Exchange),
        ] {
            match Selection::from_lookup(env_one("STOCKWISE_DATA_PROVIDER", name)) {
                Selection::Ready(p) => assert_eq!(p.kind(), kind),
                _ => panic!("{name} should be ready without a key"),
            }
        }
    }

    fn env_one(key: &'static str, value: &'static str) -> impl Fn(&str) -> Option<String> {
        move |k: &str| (k == key).then(|| value.to_string())
    }

    #[test]
    fn exchange_mode_routes_each_suffix_to_its_exchange() {
        let p = Provider::new(
            ProviderKind::Exchange,
            String::new(),
            Some("http://127.0.0.1:9".into()),
        );
        assert!(p.supports("TCS.NS"));
        assert!(p.supports("^NSEI"));
        assert!(p.supports("500325.BO"));
        assert!(p.supports("RELIANCE.BO"));
        assert!(!p.supports("AAPL"));
        assert!(!p.supports("^BSESN"));
        assert!(!p.supports("BTC-USD"));
        assert_eq!(p.name(), "NSE + BSE");
        assert_eq!(p.probes(), [("NSE", "RELIANCE.NS"), ("BSE", "500325.BO")]);
        assert!(p.nse().is_some());
        assert!(p.can_serve(&["TCS.NS", "INFY.NS", "500325.BO"]));
        let many: Vec<String> = (0..20).map(|i| format!("S{i}.NS")).collect();
        let refs: Vec<&str> = many.iter().map(String::as_str).collect();
        assert!(!p.can_serve(&refs), "a big scan goes to Yahoo");
        let nse_only = Provider::new(
            ProviderKind::Nse,
            String::new(),
            Some("http://127.0.0.1:9".into()),
        );
        assert!(!nse_only.supports("RELIANCE.BO"));
        assert!(nse_only.nse().is_some());
    }

    #[tokio::test]
    async fn exchange_quotes_keep_request_order_across_exchanges() {
        use crate::testutil::MockServer;
        let nse_server = MockServer::start(vec![(
            200,
            include_str!("fixtures/nse_quote_equity.json").into(),
        )])
        .await;
        let bse_server =
            MockServer::start(vec![(200, include_str!("fixtures/bse_header.json").into())]).await;
        let mut nse_site = nse::site(&nse_server.url);
        nse_site.home = None;
        let ex = Exchange {
            nse: nse::Nse::new(std::sync::Arc::new(session::Session::for_tests(
                nse_site,
                session::DiskCache::Off,
            ))),
            bse: bse::Bse::new(std::sync::Arc::new(session::Session::for_tests(
                bse::site(&bse_server.url),
                session::DiskCache::Off,
            ))),
        };
        let quotes = ex
            .quotes(&["500325.BO", "AAPL", "RELIANCE.NS"])
            .await
            .unwrap();
        let got: Vec<(&str, &str)> = quotes
            .iter()
            .map(|q| {
                (
                    q.symbol.as_deref().unwrap(),
                    q.data_source.as_deref().unwrap(),
                )
            })
            .collect();
        assert_eq!(got, [("500325.BO", "BSE"), ("RELIANCE.NS", "NSE")]);
    }

    #[test]
    fn merge_carries_exchange_only_fields() {
        let provider = Quote {
            regular_market_price: Some(1451.6),
            delivery_percent: Some(52.35),
            upper_circuit: Some(1597.6),
            lower_circuit: Some(1307.2),
            data_source: Some("NSE".into()),
            ..Default::default()
        };
        let yahoo = Quote {
            regular_market_price: Some(1450.0),
            trailing_pe: Some(24.0),
            ..Default::default()
        };
        let q = merge_quote(provider, Some(yahoo));
        assert_eq!(q.delivery_percent, Some(52.35));
        assert_eq!(q.upper_circuit, Some(1597.6));
        assert_eq!(q.lower_circuit, Some(1307.2));
        assert_eq!(q.trailing_pe, Some(24.0));
        let json = serde_json::to_value(&q).unwrap();
        assert_eq!(json["deliveryPercent"], 52.35);
        assert_eq!(json["upperCircuit"], 1597.6);
        // Yahoo-only quotes keep their JSON shape.
        let plain = serde_json::to_value(Quote::default()).unwrap();
        assert!(plain.get("deliveryPercent").is_none());
        assert!(plain.get("lowerCircuit").is_none());
    }

    // #157: the free-tier allowance refills with time.
    #[test]
    fn allowance_refills_after_its_window() {
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let mut a = Allowance::new(5, Duration::from_secs(60));
        for i in 0..5 {
            assert_eq!(a.remaining(at(i)), 5 - i as u32);
            assert!(a.take(at(i)).is_ok());
        }
        // Spent: no more, for the rest of the window (from the first request).
        assert_eq!(a.remaining(at(30)), 0);
        assert!(a.take(at(30)).is_err());
        assert!(a.take(at(59)).is_err());
        // A new window, a full allowance, an hour later too.
        assert_eq!(a.remaining(at(60)), 5);
        assert!(a.take(at(60)).is_ok());
        assert_eq!(a.remaining(at(3600)), 5);
    }

    /// The most requests an [`Allowance`] lets through in any 24 hours.
    /// Windows start with their first request, so a caller can put each
    /// window's other requests at its very start or very end; every
    /// combination over six back-to-back windows is tried.
    fn worst_case_per_day(limit: u32, window: Duration) -> usize {
        let w = window.as_secs();
        let windows = 6u32;
        let mut worst = 0;
        // `tails[k]`: how many of window k's requests (after the first) wait for its end.
        let mut tails = vec![0u32; windows as usize];
        loop {
            let t0 = Instant::now();
            let mut a = Allowance::new(limit, window);
            let mut sent: Vec<u64> = Vec::new();
            for (k, tail) in tails.iter().enumerate() {
                let start = k as u64 * w;
                let at = std::iter::once(start)
                    .chain((0..limit - 1 - tail).map(|_| start))
                    .chain((0..*tail).map(|_| start + w - 1));
                for at in at {
                    if a.take(t0 + Duration::from_secs(at)).is_ok() {
                        sent.push(at);
                    }
                }
            }
            let most = sent
                .iter()
                .map(|&t| sent.iter().filter(|&&u| u >= t && u < t + 86_400).count())
                .max()
                .unwrap();
            worst = worst.max(most);
            // Next combination (odometer over 0..limit per window).
            let mut i = 0;
            while i < tails.len() && tails[i] == limit - 1 {
                tails[i] = 0;
                i += 1;
            }
            if i == tails.len() {
                return worst;
            }
            tails[i] += 1;
        }
    }

    // The provider's own limit is 25 a day.
    #[test]
    fn alpha_vantage_never_exceeds_25_requests_in_any_24_hours() {
        let (limit, window) = ProviderKind::AlphaVantage.allowance();
        let worst = worst_case_per_day(limit, window);
        assert!(worst <= 25, "{worst} requests in 24 h");
        assert_eq!(worst, 23);
        // The old 5 per window: 29 in one day, over the limit.
        assert_eq!(worst_case_per_day(5, window), 29);
    }

    #[test]
    fn a_pause_ends_and_is_announced_once_each_time() {
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let mut a = Allowance::new(8, Duration::from_secs(60));
        a.pause(at(0), NETWORK_PAUSE);
        assert!(a.announce());
        assert!(!a.announce(), "once per pause");
        assert_eq!(a.remaining(at(10)), 0);
        assert!(a.take(at(179)).is_err());
        // Over: usable again, and the next pause is announced again.
        assert_eq!(a.remaining(at(180)), 8);
        assert!(a.take(at(180)).is_ok());
        a.pause(at(200), NETWORK_PAUSE);
        assert!(a.announce());
    }

    #[test]
    fn wait_times_read_naturally() {
        assert_eq!(format_wait(Duration::from_secs(45)), "45 s");
        assert_eq!(format_wait(Duration::from_secs(180)), "3 min");
        assert_eq!(format_wait(Duration::from_secs(17_280)), "5 h");
    }

    #[tokio::test]
    async fn a_spent_allowance_comes_back_with_the_window() {
        let server = MockServer::start(vec![(200, MASSIVE_BAR.into())]).await;
        let p = Provider::new(ProviderKind::Massive, "k".into(), Some(server.url.clone()));
        let symbols: Vec<String> = (0..7).map(|i| format!("SYM{i}")).collect();
        let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
        assert_eq!(p.quotes(&refs).await.unwrap().len(), 5);
        assert_eq!(p.remaining(), 0);
        assert_eq!(server.requests().len(), 5);
        // A minute on, in the same process: quotes come from the provider again.
        p.http()
            .unwrap()
            .allowance()
            .rewind(Duration::from_secs(61));
        assert_eq!(p.remaining(), 5);
        assert_eq!(p.quotes(&["AAPL"]).await.unwrap().len(), 1);
        assert_eq!(server.requests().len(), 6);
    }

    #[tokio::test]
    async fn a_network_error_pauses_the_provider_for_a_few_minutes_only() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // The first connection is cut without an answer, later ones are served.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = hits.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                if counter.fetch_add(1, Ordering::SeqCst) == 0 {
                    drop(stream);
                    continue;
                }
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                let body = MASSIVE_BAR;
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(reply.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        let p = Provider::new(ProviderKind::Massive, "k".into(), Some(url));
        assert!(p.quotes(&["AAPL"]).await.is_err());
        assert_eq!(p.remaining(), 0, "paused after the error");
        // While paused no request goes out.
        assert!(p.quotes(&["AAPL"]).await.is_err());
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // After the pause the provider is used again.
        p.http()
            .unwrap()
            .allowance()
            .rewind(NETWORK_PAUSE + Duration::from_secs(1));
        assert_eq!(p.quotes(&["AAPL"]).await.unwrap().len(), 1);
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_rate_limit_pauses_until_the_window_is_over() {
        let server = MockServer::start(vec![
            (
                429,
                r#"{"error":"You exceeded the maximum requests per minute"}"#.into(),
            ),
            (200, MASSIVE_BAR.into()),
        ])
        .await;
        let p = Provider::new(ProviderKind::Massive, "k".into(), Some(server.url.clone()));
        assert!(p.quotes(&["AAPL"]).await.is_err());
        assert_eq!(p.remaining(), 0);
        assert!(p.quotes(&["AAPL"]).await.is_err());
        assert_eq!(server.requests().len(), 1, "no request while paused");
        p.http()
            .unwrap()
            .allowance()
            .rewind(Duration::from_secs(61));
        assert_eq!(p.quotes(&["AAPL"]).await.unwrap().len(), 1);
    }

    #[test]
    fn merge_keeps_the_providers_trade_time_with_its_price() {
        // Massive's free plan: yesterday's close. Yahoo's quote is live today.
        let yesterday = d("2026-10-20");
        let provider = Quote {
            regular_market_price: Some(339.21),
            regular_market_time: trade_time("AAPL", yesterday),
            data_source: Some("Massive".into()),
            ..Default::default()
        };
        let today_live = 1_792_700_000; // 21 Oct 2026, during the NY session
        let yahoo = Quote {
            regular_market_price: Some(341.0),
            regular_market_time: Some(today_live),
            ..Default::default()
        };
        let q = merge_quote(provider.clone(), Some(yahoo.clone()));
        assert_eq!(q.regular_market_time, trade_time("AAPL", yesterday));
        let traded = chrono::DateTime::from_timestamp(q.regular_market_time.unwrap(), 0)
            .map(|t| crate::calendar::exchange_today("AAPL", t));
        assert_eq!(traded, Some(yesterday));
        // A provider with no trade time leaves Yahoo's alone.
        let no_time = Quote {
            regular_market_time: None,
            ..provider
        };
        let q = merge_quote(no_time, Some(yahoo));
        assert_eq!(q.regular_market_time, Some(today_live));
    }

    #[test]
    fn trade_time_is_the_exchange_close_of_that_day() {
        // Tue 20 Oct 2026: NYSE closes 16:00 EDT = 20:00 UTC.
        let day = d("2026-10-20");
        assert_eq!(trade_time("AAPL", day), Some(1_792_526_400));
        // NSE closes 15:30 IST = 10:00 UTC (Wed 21 Oct; the 20th is Dussehra).
        assert_eq!(trade_time("TCS.NS", d("2026-10-21")), Some(1_792_576_800));
        // Crypto has no calendar, and neither does a day NSE is closed:
        // noon UTC of the day.
        assert_eq!(trade_time("BTC-USD", day), Some(1_792_497_600));
        assert_eq!(trade_time("TCS.NS", day), Some(1_792_497_600));
    }

    #[test]
    fn merge_keeps_yahoo_fundamentals_and_takes_provider_prices() {
        let yahoo = Quote {
            symbol: Some("AAPL".into()),
            short_name: Some("Apple Inc.".into()),
            regular_market_price: Some(230.0),
            regular_market_change: Some(1.0),
            regular_market_change_percent: Some(0.4),
            regular_market_previous_close: Some(229.0),
            trailing_pe: Some(35.0),
            currency: Some("USD".into()),
            ..Default::default()
        };
        let provider = Quote {
            symbol: Some("AAPL".into()),
            regular_market_price: Some(236.39),
            regular_market_change: Some(4.25),
            regular_market_change_percent: Some(1.83),
            data_source: Some("Alpha Vantage".into()),
            ..Default::default()
        };
        let q = merge_quote(provider, Some(yahoo.clone()));
        assert_eq!(q.regular_market_price, Some(236.39));
        assert_eq!(q.regular_market_change, Some(4.25));
        assert_eq!(q.trailing_pe, Some(35.0));
        assert_eq!(q.short_name.as_deref(), Some("Apple Inc."));
        assert_eq!(q.data_source.as_deref(), Some("Alpha Vantage"));

        // A provider price without a change is measured against Yahoo's
        // previous close, never left paired with Yahoo's stale change.
        let price_only = Quote {
            regular_market_price: Some(231.29),
            data_source: Some("X".into()),
            ..Default::default()
        };
        let q = merge_quote(price_only, Some(yahoo));
        assert!((q.regular_market_change.unwrap() - 2.29).abs() < 1e-9);
        assert!((q.regular_market_change_percent.unwrap() - 1.0).abs() < 1e-9);

        // No Yahoo quote: the provider's quote stands alone.
        let alone = merge_quote(
            Quote {
                regular_market_price: Some(1.0),
                ..Default::default()
            },
            None,
        );
        assert_eq!(alone.regular_market_price, Some(1.0));
    }

    #[test]
    fn range_start_maps_yahoo_ranges() {
        let today = d("2026-10-21");
        assert_eq!(range_start("1mo", today), Some(d("2026-09-20")));
        assert_eq!(range_start("ytd", today), Some(d("2026-01-01")));
        assert_eq!(range_start("1y", today), Some(d("2025-10-20")));
        assert_eq!(range_start("max", today), None);
    }

    #[test]
    fn coverage_check_detects_short_history() {
        let series = Series {
            bars: vec![bar("2026-06-01", 10.0), bar("2026-10-20", 12.0)],
            currency: None,
        };
        assert!(series.covers(Some(d("2026-05-30"))));
        assert!(!series.covers(Some(d("2025-10-20"))));
        assert!(series.covers(None));
        assert!(!Series::default().covers(None));
    }

    #[test]
    fn resample_builds_weekly_and_monthly_bars() {
        // Mon 2026-10-12 .. Fri 2026-10-16, then Mon 2026-10-19.
        let series = Series {
            bars: vec![
                bar("2026-10-12", 10.0),
                bar("2026-10-14", 14.0),
                bar("2026-10-16", 12.0),
                bar("2026-10-19", 20.0),
            ],
            currency: Some("USD".into()),
        };
        let weekly = series.clone().resample(Resample::Week);
        assert_eq!(weekly.bars.len(), 2);
        let w = &weekly.bars[0];
        assert_eq!(w.date, d("2026-10-12"));
        assert_eq!(w.open, Some(9.0));
        assert_eq!(w.close, Some(12.0));
        assert_eq!(w.high, Some(16.0));
        assert_eq!(w.low, Some(8.0));
        assert_eq!(w.volume, Some(300));
        let monthly = series.resample(Resample::Month);
        assert_eq!(monthly.bars.len(), 1);
        assert_eq!(monthly.bars[0].close, Some(20.0));
    }

    #[test]
    fn chart_is_yahoo_shaped_with_noon_utc_timestamps() {
        let series = Series {
            bars: vec![bar("2026-10-19", 10.0), bar("2026-10-20", 11.0)],
            currency: Some("USD".into()),
        };
        let chart = series.into_chart("AAPL");
        let ts = chart.timestamp.unwrap();
        assert_eq!(ts.len(), 2);
        let first = chrono::DateTime::from_timestamp(ts[0], 0).unwrap();
        assert_eq!(first.to_rfc3339(), "2026-10-19T12:00:00+00:00");
        let meta = chart.meta.unwrap();
        assert_eq!(meta.regular_market_price, Some(11.0));
        assert_eq!(meta.currency.as_deref(), Some("USD"));
        assert_eq!(
            chart.indicators.quote[0].close.as_ref().unwrap()[1],
            Some(11.0)
        );
    }

    #[test]
    fn quote_from_bars_uses_the_last_two_closes() {
        let series = Series {
            bars: vec![bar("2026-10-19", 100.0), bar("2026-10-20", 102.0)],
            currency: Some("USD".into()),
        };
        let q = quote_from_bars(&series).unwrap();
        assert_eq!(q.regular_market_price, Some(102.0));
        assert_eq!(q.regular_market_previous_close, Some(100.0));
        assert_eq!(q.regular_market_change, Some(2.0));
        assert_eq!(q.regular_market_change_percent, Some(2.0));
        assert!(quote_from_bars(&Series::default()).is_none());
    }

    #[test]
    fn numbers_parse_from_strings_and_percentages() {
        assert_eq!(num(&serde_json::json!("236.3900")), Some(236.39));
        assert_eq!(num(&serde_json::json!("1.8308%")), Some(1.8308));
        assert_eq!(num(&serde_json::json!(4.5)), Some(4.5));
        assert_eq!(num(&serde_json::json!("n/a")), None);
        assert_eq!(int(&serde_json::json!("4908740")), Some(4_908_740));
    }

    #[test]
    fn redact_hides_the_key() {
        assert_eq!(
            redact("bad key sk-abcdef here", "sk-abcdef"),
            "bad key <key> here"
        );
    }
}
