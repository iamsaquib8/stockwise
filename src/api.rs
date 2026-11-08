use crate::market;
use crate::providers::{self, MarketData};
use anyhow::{Context, Result, bail};
use reqwest::header;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";
const MAX_RETRIES: u32 = 3;
const MIN_REQUEST_INTERVAL_MS: u64 = 200;
const REDIS_KEY_PREFIX: &str = "stockwise:";
const DEFAULT_REDIS_URL: &str = "redis://127.0.0.1:6379/";
/// Never sleep longer than this for a single retry, whatever Retry-After says.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(10);

/// Process-wide request counters, reported by `stockwise serve` at `/metrics`.
pub struct FetchStats {
    /// HTTP requests sent to the data provider, retries included.
    pub requests: AtomicU64,
    /// Fetches that failed after all retries.
    pub errors: AtomicU64,
    pub cache_hits_redis: AtomicU64,
    pub cache_hits_sqlite: AtomicU64,
    pub cache_hits_memory: AtomicU64,
    pub cache_misses: AtomicU64,
}

pub static STATS: FetchStats = FetchStats {
    requests: AtomicU64::new(0),
    errors: AtomicU64::new(0),
    cache_hits_redis: AtomicU64::new(0),
    cache_hits_sqlite: AtomicU64::new(0),
    cache_hits_memory: AtomicU64::new(0),
    cache_misses: AtomicU64::new(0),
};

fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
}

/// Do not render a provider URL from reqwest errors: Yahoo query strings may
/// contain the per-session `crumb` credential.
fn fetch_failure_message(url: &str, error: reqwest::Error) -> String {
    format!(
        "Request to {} failed after {} attempts: {}",
        host_of(url),
        MAX_RETRIES,
        error.without_url()
    )
}

/// Redis URL: `STOCKWISE_REDIS_URL`, then `redis_url` in the config file,
/// then local Redis. An empty env var has always meant "off", so it counts
/// as set.
pub fn redis_url() -> String {
    std::env::var("STOCKWISE_REDIS_URL")
        .ok()
        .or_else(|| crate::config::get().redis_url.clone())
        .unwrap_or_else(|| DEFAULT_REDIS_URL.into())
}

/// Connect to the optional Redis cache.
///
/// [`redis_url`] picks the server; `off` disables it. Short timeouts mean a
/// missing or hung Redis never slows a command down — StockWise falls back
/// to its in-memory cache.
pub async fn connect_redis() -> Option<redis::aio::MultiplexedConnection> {
    let url = redis_url();
    if url.is_empty() || url.eq_ignore_ascii_case("off") {
        return None;
    }
    let client = redis::Client::open(url).ok()?;
    let config = redis::AsyncConnectionConfig::new()
        .set_connection_timeout(Some(Duration::from_millis(300)))
        .set_response_timeout(Some(Duration::from_millis(500)));
    client
        .get_multiplexed_async_connection_with_config(&config)
        .await
        .ok()
}

/// Cache key for a request URL: the per-session `crumb` parameter is dropped
/// so cached quotes survive across runs.
fn cache_key(url: &str) -> String {
    let Ok(mut parsed) = reqwest::Url::parse(url) else {
        return url.to_string();
    };
    let kept: Vec<(String, String)> = parsed
        .query_pairs()
        .filter(|(k, _)| k != "crumb")
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    if kept.is_empty() {
        parsed.set_query(None);
    } else {
        parsed.query_pairs_mut().clear().extend_pairs(kept);
    }
    parsed.to_string()
}

/// Build `base` + `/segment` + `?query`, percent-encoding every part, so a
/// symbol like `M&M.NS` or a search for "AT&T" can't break the URL.
pub fn build_url(base: &str, segment: Option<&str>, query: &[(&str, &str)]) -> String {
    let mut url = reqwest::Url::parse(base).expect("static base URL is valid");
    if let Some(seg) = segment {
        url.path_segments_mut()
            .expect("base URL can have path segments")
            .push(seg);
    }
    if !query.is_empty() {
        url.query_pairs_mut().extend_pairs(query);
    }
    url.to_string()
}

/// Delay before retry `attempt` (0-based), honouring a server Retry-After.
fn retry_delay(attempt: u32, retry_after: Option<&header::HeaderValue>, base_ms: u64) -> Duration {
    let from_header = retry_after
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    from_header
        .unwrap_or_else(|| Duration::from_millis(base_ms * 2u64.pow(attempt)))
        .min(MAX_RETRY_DELAY)
}

/// Hostname of a URL, for error messages ("query2.finance.yahoo.com", ...)
/// A request the server refused with a status that retrying cannot fix
/// (anything but 429 or 5xx). Kept as a type so callers can tell a missing
/// resource from a network failure; see [`is_not_found`].
#[derive(Debug)]
pub struct HttpStatusError {
    host: String,
    status: reqwest::StatusCode,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} returned HTTP {}", self.host, self.status)
    }
}

impl std::error::Error for HttpStatusError {}

/// True when `err` is (or wraps) an HTTP 404 from [`YahooClient`], such as a
/// symbol Yahoo has no chart for.
pub fn is_not_found(err: &anyhow::Error) -> bool {
    err.downcast_ref::<NoPriceHistory>().is_some()
        || err
            .downcast_ref::<HttpStatusError>()
            .is_some_and(|e| e.status == reqwest::StatusCode::NOT_FOUND)
}

fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
}

// ── Hybrid Cache: Redis (primary) + In-Memory (fallback) ──

/// The in-memory fallback is bounded so a long-lived `serve`, `tui`, `mcp` or
/// `ask` process cannot grow with every researched symbol. Redis, when
/// configured, keeps the long-lived copies.
const MEMORY_CACHE_MAX_ENTRIES: usize = 500;
const MEMORY_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;
const MEMORY_CACHE_MAX_TTL: Duration = Duration::from_secs(3600);

struct CacheEntry {
    data: String,
    expires: Instant,
    inserted: Instant,
}

pub(crate) struct HybridCache {
    redis: Option<redis::aio::MultiplexedConnection>,
    memory: HashMap<String, CacheEntry>,
    /// The SQLite `cache.db`, used as L2 when Redis is not connected.
    l2: Option<crate::db::L2>,
}

/// The cache shared by Yahoo and an alternative data provider.
pub(crate) type SharedCache = Arc<tokio::sync::Mutex<HybridCache>>;

impl HybridCache {
    async fn new() -> Self {
        Self {
            redis: connect_redis().await,
            memory: HashMap::new(),
            l2: crate::db::l2(),
        }
    }

    #[cfg(test)]
    pub(crate) fn in_memory() -> Self {
        Self {
            redis: None,
            memory: HashMap::new(),
            l2: None,
        }
    }

    /// In-memory L1 over a given `cache.db`, no Redis.
    #[cfg(test)]
    pub(crate) fn with_l2(l2: crate::db::L2) -> Self {
        Self {
            l2: Some(l2),
            ..Self::in_memory()
        }
    }

    fn has_redis(&self) -> bool {
        self.redis.is_some()
    }

    pub(crate) async fn get(&mut self, key: &str) -> Option<String> {
        let redis_key = format!("{}{}", REDIS_KEY_PREFIX, key);

        // Try Redis first
        if let Some(ref mut conn) = self.redis {
            match redis::cmd("GET")
                .arg(&redis_key)
                .query_async::<Option<String>>(conn)
                .await
            {
                Ok(Some(v)) => {
                    count(&STATS.cache_hits_redis);
                    return Some(v);
                }
                Ok(None) => {}
                // Timed out or dropped: stop using Redis for this run.
                Err(_) => self.redis = None,
            }
        }

        // Fallback to memory
        let hit = self.memory.get(key).and_then(|e| {
            if Instant::now() < e.expires {
                Some(e.data.clone())
            } else {
                None
            }
        });
        if hit.is_some() {
            count(&STATS.cache_hits_memory);
            return hit;
        }

        // Persistent L2: a response fetched by an earlier command.
        if self.redis.is_none()
            && let Some(l2) = self.l2
            && let Some(v) = crate::db::CacheDb::l2_get(l2, key)
        {
            count(&STATS.cache_hits_sqlite);
            return Some(v);
        }
        count(&STATS.cache_misses);
        None
    }

    pub(crate) async fn set(&mut self, key: String, data: String, ttl: Duration) {
        let ttl_secs = ttl.as_secs() as i64;
        let redis_key = format!("{}{}", REDIS_KEY_PREFIX, key);

        // Write to Redis
        if let Some(ref mut conn) = self.redis {
            let _: Result<(), _> = redis::cmd("SETEX")
                .arg(&redis_key)
                .arg(ttl_secs.max(1))
                .arg(&data)
                .query_async(conn)
                .await;
        }

        // Without Redis, keep the response on disk for the next command.
        if self.redis.is_none()
            && let Some(l2) = self.l2
        {
            crate::db::CacheDb::l2_set(l2, &key, &data, ttl.as_secs());
        }

        // Also write to memory (fast path for same-session reads). The
        // in-memory copy is capped in count, bytes and TTL; Redis above keeps
        // the full-lived entry.
        let ttl = ttl.min(MEMORY_CACHE_MAX_TTL);
        self.make_room(key.len() + data.len());
        let now = Instant::now();
        self.memory.insert(
            key,
            CacheEntry {
                data,
                expires: now + ttl,
                inserted: now,
            },
        );
    }

    fn memory_bytes(&self) -> usize {
        self.memory
            .iter()
            .map(|(key, entry)| key.len() + entry.data.len())
            .sum()
    }

    /// Drop expired entries, then the oldest, until one more entry of `extra`
    /// bytes fits under the count and byte caps.
    fn make_room(&mut self, extra: usize) {
        if self.memory.len() < MEMORY_CACHE_MAX_ENTRIES
            && self.memory_bytes() + extra <= MEMORY_CACHE_MAX_BYTES
        {
            return;
        }
        let now = Instant::now();
        self.memory.retain(|_, entry| now < entry.expires);
        if self.memory.len() < MEMORY_CACHE_MAX_ENTRIES
            && self.memory_bytes() + extra <= MEMORY_CACHE_MAX_BYTES
        {
            return;
        }
        let mut by_age: Vec<(Instant, String)> = self
            .memory
            .iter()
            .map(|(key, entry)| (entry.inserted, key.clone()))
            .collect();
        by_age.sort_by_key(|(inserted, _)| *inserted);
        for (_, key) in by_age {
            if self.memory.len() < MEMORY_CACHE_MAX_ENTRIES
                && self.memory_bytes() + extra <= MEMORY_CACHE_MAX_BYTES
            {
                break;
            }
            self.memory.remove(&key);
        }
    }
}

// ── Data Models ──

#[derive(Debug, Deserialize)]
pub struct QuoteResponse {
    #[serde(rename = "quoteResponse")]
    pub quote_response: QuoteResponseInner,
}

#[derive(Debug, Deserialize)]
pub struct QuoteResponseInner {
    pub result: Vec<Quote>,
}

impl Quote {
    /// Yahoo answers an unknown ticker such as `500325.BO` with a row whose
    /// `quoteType` is `NONE` and every other field null. It is not a quote.
    pub fn is_placeholder(&self) -> bool {
        self.quote_type.as_deref() == Some("NONE")
    }

    /// Dividend yield as a fraction (0.0448 = 4.48%).
    ///
    /// Yahoo's `trailingAnnualDividendYield` is 0 for many NSE stocks
    /// (RELIANCE, M&M) and for Infosys it divides a dollar dividend by a rupee
    /// price (0.0005). Its `dividendYield` field, a percentage, is right in
    /// those cases. Use the trailing figure when it is positive and within a
    /// factor of two of `dividendYield / 100`, otherwise that one.
    pub fn dividend_yield_fraction(&self) -> Option<f64> {
        let trailing = self
            .trailing_annual_dividend_yield
            .filter(|v| v.is_finite() && *v > 0.0);
        let reported = self
            .dividend_yield
            .filter(|v| v.is_finite() && *v > 0.0)
            .map(|v| v / 100.0);
        match (trailing, reported) {
            (Some(t), Some(r)) if (0.5..=2.0).contains(&(t / r)) => Some(t),
            (_, Some(r)) => Some(r),
            (Some(t), None) => Some(t),
            // Nothing positive: keep what Yahoo said (0 or absent).
            (None, None) => self.trailing_annual_dividend_yield,
        }
    }

    /// A row that describes a real instrument with a price.
    pub fn is_usable(&self) -> bool {
        !self.is_placeholder() && self.regular_market_price.is_some()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Quote {
    pub symbol: Option<String>,
    #[serde(rename = "quoteType")]
    pub quote_type: Option<String>,
    #[serde(rename = "shortName")]
    pub short_name: Option<String>,
    #[serde(rename = "longName")]
    pub long_name: Option<String>,
    #[serde(rename = "regularMarketPrice")]
    pub regular_market_price: Option<f64>,
    #[serde(rename = "regularMarketChange")]
    pub regular_market_change: Option<f64>,
    #[serde(rename = "regularMarketChangePercent")]
    pub regular_market_change_percent: Option<f64>,
    #[serde(rename = "regularMarketVolume")]
    pub regular_market_volume: Option<u64>,
    #[serde(rename = "regularMarketOpen")]
    pub regular_market_open: Option<f64>,
    #[serde(rename = "regularMarketDayHigh")]
    pub regular_market_day_high: Option<f64>,
    #[serde(rename = "regularMarketDayLow")]
    pub regular_market_day_low: Option<f64>,
    #[serde(rename = "regularMarketPreviousClose")]
    pub regular_market_previous_close: Option<f64>,
    /// When the price was last updated (Unix seconds).
    #[serde(
        rename = "regularMarketTime",
        default,
        deserialize_with = "lenient_unix_time"
    )]
    pub regular_market_time: Option<i64>,
    #[serde(rename = "fiftyDayAverage")]
    pub fifty_day_average: Option<f64>,
    #[serde(rename = "twoHundredDayAverage")]
    pub two_hundred_day_average: Option<f64>,
    #[serde(rename = "fiftyTwoWeekHigh")]
    pub fifty_two_week_high: Option<f64>,
    #[serde(rename = "fiftyTwoWeekLow")]
    pub fifty_two_week_low: Option<f64>,
    #[serde(rename = "marketCap")]
    pub market_cap: Option<f64>,
    #[serde(rename = "trailingPE")]
    pub trailing_pe: Option<f64>,
    #[serde(rename = "forwardPE")]
    pub forward_pe: Option<f64>,
    #[serde(rename = "trailingAnnualDividendYield")]
    pub trailing_annual_dividend_yield: Option<f64>,
    #[serde(rename = "epsTrailingTwelveMonths")]
    pub eps_trailing_twelve_months: Option<f64>,
    #[serde(rename = "epsForward")]
    pub eps_forward: Option<f64>,
    #[serde(rename = "priceToBook")]
    pub price_to_book: Option<f64>,
    #[serde(rename = "bookValue")]
    pub book_value: Option<f64>,
    pub exchange: Option<String>,
    pub currency: Option<String>,
    #[serde(rename = "averageDailyVolume3Month")]
    pub average_daily_volume_3_month: Option<u64>,
    #[serde(rename = "enterpriseToRevenue")]
    pub enterprise_to_revenue: Option<f64>,
    #[serde(rename = "enterpriseToEbitda")]
    pub enterprise_to_ebitda: Option<f64>,
    #[serde(rename = "revenuePerShare")]
    pub revenue_per_share: Option<f64>,
    #[serde(rename = "returnOnEquity")]
    pub return_on_equity: Option<f64>,
    #[serde(rename = "profitMargins")]
    pub profit_margins: Option<f64>,
    #[serde(rename = "debtToEquity")]
    pub debt_to_equity: Option<f64>,
    #[serde(rename = "currentRatio")]
    pub current_ratio: Option<f64>,
    #[serde(rename = "earningsQuarterlyGrowth")]
    pub earnings_quarterly_growth: Option<f64>,
    #[serde(rename = "revenueGrowth")]
    pub revenue_growth: Option<f64>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    #[serde(rename = "targetMeanPrice")]
    pub target_mean_price: Option<f64>,
    #[serde(rename = "recommendationMean")]
    pub recommendation_mean: Option<f64>,
    #[serde(rename = "recommendationKey")]
    pub recommendation_key: Option<String>,
    #[serde(rename = "numberOfAnalystOpinions")]
    pub number_of_analyst_opinions: Option<u32>,
    pub beta: Option<f64>,
    #[serde(rename = "dividendYield")]
    pub dividend_yield: Option<f64>,
    /// Which provider supplied the price when it wasn't Yahoo
    /// (`STOCKWISE_DATA_PROVIDER`); omitted from JSON otherwise.
    #[serde(rename = "dataSource", skip_serializing_if = "Option::is_none")]
    pub data_source: Option<String>,
    /// NSE: share of the day's traded quantity marked for delivery, in
    /// percent. Only the NSE provider fills it; omitted from JSON otherwise.
    #[serde(rename = "deliveryPercent", skip_serializing_if = "Option::is_none")]
    pub delivery_percent: Option<f64>,
    /// The exchange's price band for the day (upper circuit limit); NSE
    /// provider only.
    #[serde(rename = "upperCircuit", skip_serializing_if = "Option::is_none")]
    pub upper_circuit: Option<f64>,
    /// Lower circuit limit; NSE provider only.
    #[serde(rename = "lowerCircuit", skip_serializing_if = "Option::is_none")]
    pub lower_circuit: Option<f64>,
}

/// Unix seconds as a number or Yahoo's `{raw, fmt}`; anything else is None
/// rather than a failed quote.
fn lenient_unix_time<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let v = Option::<serde_json::Value>::deserialize(d)?;
    Ok(v.and_then(|v| {
        v.as_i64()
            .or_else(|| v.as_f64().map(|f| f as i64))
            .or_else(|| v.get("raw").and_then(serde_json::Value::as_i64))
    }))
}

#[derive(Debug, Deserialize)]
pub struct ChartResponse {
    pub chart: ChartResult,
}

#[derive(Debug, Deserialize)]
pub struct ChartResult {
    pub result: Option<Vec<ChartData>>,
}

#[derive(Debug, Deserialize)]
pub struct ChartData {
    pub meta: Option<ChartMeta>,
    pub timestamp: Option<Vec<i64>>,
    pub indicators: Indicators,
    /// Present when requested with `events=div,splits`.
    #[serde(default)]
    pub events: Option<ChartEvents>,
}

/// The last completed daily session's range, the input to pivot levels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriorSession {
    pub date: chrono::NaiveDate,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// The session date of a bar stamped `ts` (UTC seconds) on an exchange
/// `offset` seconds east of UTC. Three hours of slack absorb a daylight-saving
/// change since the bar (the chart carries one offset for all of its bars),
/// but only where there is an offset to be wrong: at offset 0 (crypto, and
/// charts without one) a bar stamped 22:30 UTC still belongs to that UTC day.
fn bar_date(ts: i64, offset: i64) -> Option<chrono::NaiveDate> {
    let slack = if offset == 0 { 0 } else { 3 * 3600 };
    Some(chrono::DateTime::from_timestamp(ts + offset + slack, 0)?.date_naive())
}

impl ChartData {
    /// High, low and close of the last *completed* session in a daily chart,
    /// for classic pivot points, which come from the previous session. While
    /// today's session is still running (the market is open or has not
    /// closed yet, or the market never closes, like crypto), the last bar is
    /// today's live one and is skipped. Bars missing a high, low or close are
    /// ignored. Only meaningful for `1d` bars: a weekly bar would be a week.
    pub fn prior_session(
        &self,
        now: chrono::DateTime<chrono::Utc>,
        market: crate::market::Market,
    ) -> Option<PriorSession> {
        let offset = self.meta.as_ref().and_then(|m| m.gmtoffset).unwrap_or(0);
        // The chart's own symbol decides the market: a BTC-USD chart never
        // closes, whatever market the command was run with.
        let market = match self.meta.as_ref().and_then(|m| m.symbol.as_deref()) {
            Some(symbol) => crate::market::market_of_symbol(symbol, market),
            None => market,
        };
        let q = self.indicators.quote.first()?;
        let (highs, lows, closes) = (q.high.as_deref()?, q.low.as_deref()?, q.close.as_deref()?);
        let mut bars: Vec<PriorSession> = self
            .timestamp
            .as_deref()?
            .iter()
            .enumerate()
            .filter_map(|(i, ts)| {
                Some(PriorSession {
                    date: bar_date(*ts, offset)?,
                    high: (*highs.get(i)?)?,
                    low: (*lows.get(i)?)?,
                    close: (*closes.get(i)?)?,
                })
            })
            .collect();
        let today = crate::clock::session_date(market, now);
        let running = bars.last().is_some_and(|b| {
            b.date == today && crate::clock::close_at(market, today).is_none_or(|c| now < c)
        });
        if running {
            bars.pop();
        }
        bars.pop()
    }

    /// Closing prices with the session date of each bar, dated in the
    /// exchange's time zone (`gmtoffset`; see [`bar_date`] for the slack). Bars without a close are skipped.
    pub fn dated_closes(&self) -> Vec<(chrono::NaiveDate, f64)> {
        let offset = self.meta.as_ref().and_then(|m| m.gmtoffset).unwrap_or(0);
        let closes = self
            .indicators
            .quote
            .first()
            .and_then(|q| q.close.as_deref())
            .unwrap_or_default();
        self.timestamp
            .as_deref()
            .unwrap_or_default()
            .iter()
            .zip(closes)
            .filter_map(|(ts, close)| Some((bar_date(*ts, offset)?, (*close)?)))
            .collect()
    }
}

/// Dividends and splits from the chart API, keyed by an opaque bucket
/// timestamp — use each event's own `date`.
#[derive(Debug, Default, Deserialize)]
pub struct ChartEvents {
    #[serde(default)]
    pub dividends: HashMap<String, DividendEvent>,
    #[serde(default)]
    pub splits: HashMap<String, SplitEvent>,
    /// Currency of the dividend amounts (from the chart's metadata).
    #[serde(skip)]
    pub currency: Option<String>,
}

/// Cash dividend per share. Yahoo adjusts past amounts for later splits
/// (RELIANCE's ₹9 in 2023 shows as ₹4.50 after its 2024 1:1 bonus).
#[derive(Debug, Deserialize)]
pub struct DividendEvent {
    pub amount: f64,
    pub date: i64,
}

/// A split, or a bonus issue (Yahoo reports a 1:1 bonus as a 2:1 split).
#[derive(Debug, Deserialize)]
pub struct SplitEvent {
    pub date: i64,
    pub numerator: f64,
    pub denominator: f64,
}

#[derive(Debug, Deserialize)]
pub struct ChartMeta {
    #[serde(rename = "regularMarketPrice")]
    pub regular_market_price: Option<f64>,
    #[serde(rename = "chartPreviousClose")]
    pub chart_previous_close: Option<f64>,
    pub symbol: Option<String>,
    #[serde(rename = "longName")]
    pub long_name: Option<String>,
    #[serde(rename = "shortName")]
    pub short_name: Option<String>,
    #[serde(rename = "regularMarketDayHigh")]
    pub regular_market_day_high: Option<f64>,
    #[serde(rename = "regularMarketDayLow")]
    pub regular_market_day_low: Option<f64>,
    #[serde(rename = "regularMarketVolume")]
    pub regular_market_volume: Option<u64>,
    #[serde(rename = "fiftyTwoWeekHigh")]
    pub fifty_two_week_high: Option<f64>,
    #[serde(rename = "fiftyTwoWeekLow")]
    pub fifty_two_week_low: Option<f64>,
    pub currency: Option<String>,
    #[serde(rename = "exchangeName")]
    pub exchange_name: Option<String>,
    /// Seconds east of UTC for the exchange's time zone, to date its bars.
    #[serde(default)]
    pub gmtoffset: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct Indicators {
    pub quote: Vec<QuoteIndicator>,
}

#[derive(Debug, Deserialize)]
pub struct QuoteIndicator {
    pub open: Option<Vec<Option<f64>>>,
    pub high: Option<Vec<Option<f64>>>,
    pub low: Option<Vec<Option<f64>>>,
    pub close: Option<Vec<Option<f64>>>,
    pub volume: Option<Vec<Option<u64>>>,
}

#[derive(Debug, Deserialize)]
pub struct SearchResponse {
    pub quotes: Vec<SearchResult>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct SearchResult {
    pub symbol: Option<String>,
    #[serde(rename = "shortname")]
    pub short_name: Option<String>,
    #[serde(rename = "longname")]
    pub long_name: Option<String>,
    #[serde(rename = "quoteType")]
    pub quote_type: Option<String>,
    pub exchange: Option<String>,
}

// ── Client ──

/// Yahoo's two-step handshake: a cookie from `cookie_url`, then a crumb for it
/// from `crumb_url`. Each step is tried up to `MAX_RETRIES` times. The cookie
/// is kept by the client's cookie store (which sends it to `yahoo.com` hosts
/// only) and replaces the previous one when the handshake is run again.
async fn open_session(
    client: &reqwest::Client,
    cookie_url: String,
    crumb_url: String,
) -> Result<String> {
    // Step 1: Get cookies — retry up to 3 times
    for attempt in 0..MAX_RETRIES {
        match client.get(&cookie_url).send().await {
            Ok(_) => break,
            Err(_e) if attempt < MAX_RETRIES - 1 => {
                let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                tokio::time::sleep(delay).await;
                continue;
            }
            Err(e) => bail!(
                "Cannot reach Yahoo Finance ({}). Check your internet connection.",
                e
            ),
        }
    }

    // Step 2: Get crumb — retry
    let mut crumb = String::new();
    for attempt in 0..MAX_RETRIES {
        match client.get(&crumb_url).send().await {
            Ok(resp) if resp.status().is_success() => {
                crumb = resp.text().await.context("Failed to read crumb")?;
                // A real crumb is a short token; anything else is an error page.
                if !crumb.is_empty() && crumb.len() < 64 && !crumb.contains(char::is_whitespace) {
                    break;
                }
                if attempt == MAX_RETRIES - 1 {
                    bail!("Yahoo Finance returned an invalid session token. Try again shortly.");
                }
                tokio::time::sleep(retry_delay(attempt, None, 500)).await;
            }
            Ok(resp) => {
                let status = resp.status();
                if attempt == MAX_RETRIES - 1 {
                    bail!(
                        "Yahoo Finance refused to start a session (HTTP {}). {}",
                        status,
                        if status.as_u16() == 429 {
                            "You are being rate limited — wait a minute and retry."
                        } else {
                            "Try again shortly."
                        }
                    );
                }
                let delay = retry_delay(attempt, resp.headers().get(header::RETRY_AFTER), 500);
                tokio::time::sleep(delay).await;
            }
            Err(_e) if attempt < MAX_RETRIES - 1 => {
                tokio::time::sleep(retry_delay(attempt, None, 500)).await;
            }
            Err(e) => bail!(
                "Cannot reach Yahoo Finance ({}). Check your internet connection.",
                e
            ),
        }
    }
    Ok(crumb)
}

/// The crumb for the endpoints that need one, and which handshake it came
/// from. (The cookie that goes with it lives in the client's cookie store.)
struct YahooSession {
    crumb: String,
    /// 1 for the first handshake, one more for each renewal.
    generation: u64,
    opened: Instant,
}

/// A crumb the server refused (HTTP 401) is renewed, but a new handshake is
/// not started more often than this.
const CRUMB_RENEW_AFTER: Duration = Duration::from_secs(60);

/// After a failed handshake the error is returned from memory this long
/// instead of starting another handshake.
const HANDSHAKE_FAILURE_TTL: Duration = Duration::from_secs(45);

/// Stands in for the crumb in URLs built before the session exists. `fetch`
/// swaps in the real crumb, opening the session first if needed.
const CRUMB_PLACEHOLDER: &str = "STOCKWISE_CRUMB_PLACEHOLDER";

/// A chart request for a ticker Yahoo does not know: the readable message,
/// still counted by [`is_not_found`] like the 404 it replaces.
#[derive(Debug)]
struct NoPriceHistory(String);

impl std::fmt::Display for NoPriceHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for NoPriceHistory {}

/// A typed "no chart for this symbol" error, for sources other than the Yahoo
/// client (tests, alternative providers). [`is_not_found`] recognises it.
pub(crate) fn no_history_error(symbol: &str) -> anyhow::Error {
    anyhow::Error::new(NoPriceHistory(no_history_message(symbol)))
}

/// What a chart request for a ticker Yahoo does not know says instead of
/// the raw "HTTP 404", with built-in "did you mean" names where it has them.
fn no_history_message(symbol: &str) -> String {
    let bare = symbol.trim_end_matches(".NS").trim_end_matches(".BO");
    let mut msg =
        format!("No price history for {symbol} — check the ticker with `stockwise search {bare}`");
    let near: Vec<String> = market::suggest_symbols(symbol, 3)
        .into_iter()
        .map(|(s, name)| format!("{s} ({name})"))
        .collect();
    if !near.is_empty() {
        msg.push_str(&format!(". Did you mean: {}", near.join(", ")));
    }
    msg
}

pub struct YahooClient {
    client: reqwest::Client,
    /// Opened on the first request that needs it (quotes, quoteSummary,
    /// fundamentals, options; chart and search never do) and renewed after
    /// an HTTP 401. Held across the handshake, so concurrent callers wait
    /// for one handshake instead of each starting their own.
    session: tokio::sync::Mutex<Option<YahooSession>>,
    /// When the last handshake failed and what it said. Crumb requests within
    /// [`HANDSHAKE_FAILURE_TTL`] get this error back without another
    /// handshake (3 cookie + 3 crumb attempts, with sleeps and connect
    /// timeouts) against a server that has just refused one.
    handshake_failure: Mutex<Option<(Instant, String)>>,
    crumb_renew_after: Duration,
    last_request: Mutex<Instant>,
    cache: SharedCache,
    /// Opt-in alternative provider (`STOCKWISE_DATA_PROVIDER`). Quotes,
    /// charts and search go there first; Yahoo fills the gaps.
    provider: Option<providers::Provider>,
    /// NSE, for index members when the provider isn't NSE-backed.
    nse: std::sync::OnceLock<providers::nse::Nse>,
    #[cfg(test)]
    pub(crate) url_rewrite: Option<std::sync::Arc<dyn Fn(&str) -> String + Send + Sync>>,
}

impl YahooClient {
    pub async fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client")?;

        let cache = Arc::new(tokio::sync::Mutex::new(HybridCache::new().await));
        let provider = match providers::Selection::from_env() {
            providers::Selection::Ready(p) => Some(p.with_cache(cache.clone())),
            providers::Selection::Yahoo => None,
            // Misconfigured: say so (doctor explains), keep working on Yahoo.
            providers::Selection::MissingKey(kind) => {
                eprintln!(
                    "  \u{24d8} STOCKWISE_DATA_PROVIDER names {} but {} is not set; using Yahoo Finance",
                    kind.name(),
                    kind.key_env()
                );
                None
            }
            providers::Selection::Unknown(name) => {
                eprintln!(
                    "  \u{24d8} Unknown STOCKWISE_DATA_PROVIDER '{}' (alphavantage, massive, twelvedata, nse, bse, exchange); using Yahoo Finance",
                    name
                );
                None
            }
        };

        Ok(Self {
            client,
            session: tokio::sync::Mutex::new(None),
            handshake_failure: Mutex::new(None),
            last_request: Mutex::new(Instant::now() - Duration::from_secs(1)),
            crumb_renew_after: CRUMB_RENEW_AFTER,
            cache,
            provider,
            nse: std::sync::OnceLock::new(),
            #[cfg(test)]
            url_rewrite: None,
        })
    }

    /// The current crumb and its generation, opening the session on first
    /// use. A failed handshake is remembered for [`HANDSHAKE_FAILURE_TTL`]:
    /// crumb requests in that time get the same error at once, then the next
    /// one tries again. Requests that need no crumb never see the error.
    async fn crumb_now(&self) -> Result<(String, u64)> {
        let mut slot = self.session.lock().await;
        if let Some(s) = slot.as_ref() {
            return Ok((s.crumb.clone(), s.generation));
        }
        let crumb = self.handshake_remembering_failure().await?;
        *slot = Some(YahooSession {
            crumb: crumb.clone(),
            generation: 1,
            opened: Instant::now(),
        });
        Ok((crumb, 1))
    }

    /// A new crumb after the server refused the one of generation `seen`
    /// (HTTP 401): a fresh handshake, unless another request has renewed it
    /// meanwhile (its crumb is used) or the last handshake was under a minute
    /// ago (`None`: the 401 stands).
    async fn renew_crumb(&self, seen: u64) -> Result<Option<String>> {
        let mut slot = self.session.lock().await;
        let generation = match slot.as_ref() {
            Some(s) if s.generation != seen => return Ok(Some(s.crumb.clone())),
            Some(s) if s.opened.elapsed() < self.crumb_renew_after => return Ok(None),
            Some(s) => s.generation + 1,
            None => 1,
        };
        let crumb = self.handshake_remembering_failure().await?;
        *slot = Some(YahooSession {
            crumb: crumb.clone(),
            generation,
            opened: Instant::now(),
        });
        Ok(Some(crumb))
    }

    /// [`Self::handshake`], except that a failure within the last
    /// [`HANDSHAKE_FAILURE_TTL`] is returned again without a new attempt.
    /// Called with the session lock held, so callers queue behind one attempt.
    async fn handshake_remembering_failure(&self) -> Result<String> {
        if let Some((at, msg)) = self.handshake_failure.lock().expect("lock").as_ref()
            && at.elapsed() < HANDSHAKE_FAILURE_TTL
        {
            bail!("{msg}");
        }
        match self.handshake().await {
            Ok(crumb) => {
                *self.handshake_failure.lock().expect("lock") = None;
                Ok(crumb)
            }
            Err(e) => {
                *self.handshake_failure.lock().expect("lock") =
                    Some((Instant::now(), format!("{e:#}")));
                Err(e)
            }
        }
    }

    async fn handshake(&self) -> Result<String> {
        open_session(
            &self.client,
            self.handshake_url("https://fc.yahoo.com"),
            self.handshake_url("https://query2.finance.yahoo.com/v1/test/getcrumb"),
        )
        .await
    }

    #[cfg(test)]
    fn handshake_url(&self, url: &str) -> String {
        match &self.url_rewrite {
            Some(rw) => rw(url),
            None => url.to_string(),
        }
    }

    #[cfg(not(test))]
    fn handshake_url(&self, url: &str) -> String {
        url.to_string()
    }

    #[cfg(test)]
    pub fn for_tests_with_client(client: reqwest::Client, crumb: &str) -> Self {
        Self {
            client,
            session: tokio::sync::Mutex::new(Some(YahooSession {
                crumb: crumb.to_string(),
                generation: 1,
                opened: Instant::now(),
            })),
            handshake_failure: Mutex::new(None),
            last_request: Mutex::new(Instant::now() - Duration::from_secs(1)),
            crumb_renew_after: CRUMB_RENEW_AFTER,
            cache: Arc::new(tokio::sync::Mutex::new(HybridCache::in_memory())),
            provider: None,
            nse: std::sync::OnceLock::new(),
            url_rewrite: None,
        }
    }

    /// A client that opens its Yahoo session for real (against whatever
    /// `rewrite` points the URLs at) and may carry a data provider.
    #[cfg(test)]
    pub(crate) fn lazy_for_tests<F>(provider: Option<providers::Provider>, rewrite: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        Self {
            client: reqwest::Client::builder()
                .cookie_store(true)
                .build()
                .expect("HTTP client builds"),
            session: tokio::sync::Mutex::new(None),
            handshake_failure: Mutex::new(None),
            last_request: Mutex::new(Instant::now() - Duration::from_secs(1)),
            crumb_renew_after: Duration::ZERO,
            cache: Arc::new(tokio::sync::Mutex::new(HybridCache::in_memory())),
            provider,
            nse: std::sync::OnceLock::new(),
            url_rewrite: Some(std::sync::Arc::new(rewrite)),
        }
    }

    #[cfg(test)]
    pub fn for_tests_with_rewrite<F>(client: reqwest::Client, crumb: &str, rewrite: F) -> Self
    where
        F: Fn(&str) -> String + Send + Sync + 'static,
    {
        Self {
            client,
            session: tokio::sync::Mutex::new(Some(YahooSession {
                crumb: crumb.to_string(),
                generation: 1,
                opened: Instant::now(),
            })),
            handshake_failure: Mutex::new(None),
            last_request: Mutex::new(Instant::now() - Duration::from_secs(1)),
            crumb_renew_after: CRUMB_RENEW_AFTER,
            cache: Arc::new(tokio::sync::Mutex::new(HybridCache::in_memory())),
            provider: None,
            nse: std::sync::OnceLock::new(),
            url_rewrite: Some(std::sync::Arc::new(rewrite)),
        }
    }

    /// Rate-limited GET with retry and caching (Redis + in-memory)
    async fn fetch(&self, url: &str, cache_ttl: Duration) -> Result<String> {
        self.fetch_with(url, cache_ttl, &[]).await
    }

    /// Rate-limited GET with retry and caching, with optional extra headers.
    pub async fn fetch_with(
        &self,
        url: &str,
        cache_ttl: Duration,
        extra_headers: &[(&str, &str)],
    ) -> Result<String> {
        let result = self
            .fetch_uncounted_with(url, cache_ttl, extra_headers)
            .await;
        if result.is_err() {
            count(&STATS.errors);
        }
        result
    }

    async fn fetch_uncounted_with(
        &self,
        url: &str,
        cache_ttl: Duration,
        extra_headers: &[(&str, &str)],
    ) -> Result<String> {
        // Check cache first (Redis → memory)
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(&cache_key(url)).await {
                return Ok(cached);
            }
        }

        // Rate limit — reserve the next slot while holding the lock so
        // concurrent callers serialize on a monotonically increasing schedule
        // (the std Mutex is released before the await, never held across it).
        let sleep_until = {
            let mut last = self.last_request.lock().unwrap();
            #[cfg(test)]
            let min_interval = Duration::from_millis(0);
            #[cfg(not(test))]
            let min_interval = Duration::from_millis(MIN_REQUEST_INTERVAL_MS);
            let next = (*last + min_interval).max(Instant::now());
            *last = next;
            next
        };
        let now = Instant::now();
        if sleep_until > now {
            tokio::time::sleep(sleep_until - now).await;
        }

        // Open the Yahoo session only for URLs that carry the crumb. The
        // cookie that goes with it is the client's cookie store's business:
        // it sends Yahoo's cookies to yahoo.com hosts and nowhere else.
        let template = url;
        let mut crumb_generation = None;
        let mut live = if template.contains(CRUMB_PLACEHOLDER) {
            let (crumb, generation) = self.crumb_now().await?;
            crumb_generation = Some(generation);
            template.replace(CRUMB_PLACEHOLDER, &crumb)
        } else {
            template.to_string()
        };
        let mut renewed = false;

        // Fetch with retry + exponential backoff
        let mut last_status = None;
        let mut attempt = 0;
        while attempt < MAX_RETRIES {
            let url = live.as_str();
            #[cfg(test)]
            let url_to_fetch = match &self.url_rewrite {
                Some(rw) => rw(url),
                None => url.to_string(),
            };
            #[cfg(not(test))]
            let url_to_fetch = url;
            let last_attempt = attempt == MAX_RETRIES - 1;
            let mut req = self.client.get(url_to_fetch);
            for (k, v) in extra_headers {
                req = req.header(*k, *v);
            }
            count(&STATS.requests);
            let result = req.send().await;

            match result {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let body = resp.text().await.context("Failed to read response body")?;
                        // Cache to Redis + memory
                        let mut cache = self.cache.lock().await;
                        cache.set(cache_key(url), body.clone(), cache_ttl).await;
                        return Ok(body);
                    }
                    last_status = Some(status);
                    if status.as_u16() == 429 {
                        if last_attempt {
                            break;
                        }
                        let delay =
                            retry_delay(attempt + 1, resp.headers().get(header::RETRY_AFTER), 1000);
                        eprintln!(
                            "  Rate limited by {}. Waiting {}s...",
                            host_of(url),
                            delay.as_secs()
                        );
                        tokio::time::sleep(delay).await;
                    } else if status.is_server_error() {
                        if last_attempt {
                            break;
                        }
                        tokio::time::sleep(retry_delay(attempt, None, 500)).await;
                    } else if status.as_u16() == 401
                        && !renewed
                        && let Some(generation) = crumb_generation
                    {
                        // Yahoo ended the session: one fresh handshake, then
                        // the same request again with the new crumb. (A
                        // failed handshake leaves the 401 as the answer.)
                        renewed = true;
                        if let Ok(Some(crumb)) = self.renew_crumb(generation).await {
                            live = template.replace(CRUMB_PLACEHOLDER, &crumb);
                            continue;
                        }
                        return Err(HttpStatusError {
                            host: host_of(url).to_string(),
                            status,
                        }
                        .into());
                    } else {
                        return Err(HttpStatusError {
                            host: host_of(url).to_string(),
                            status,
                        }
                        .into());
                    }
                }
                Err(_e) if !last_attempt => {
                    tokio::time::sleep(retry_delay(attempt, None, 500)).await;
                }
                Err(e) => bail!("{}", fetch_failure_message(url, e)),
            }
            attempt += 1;
        }
        match last_status {
            Some(s) if s.as_u16() == 429 => {
                bail!("Yahoo Finance is rate limiting requests. Wait a minute and try again.")
            }
            Some(s) => bail!(
                "Yahoo Finance returned HTTP {} after {} attempts",
                s,
                MAX_RETRIES
            ),
            None => bail!(
                "Request to Yahoo Finance failed after {} attempts",
                MAX_RETRIES
            ),
        }
    }

    /// Quotes for `symbols`. With a data provider configured, it prices the
    /// symbols it covers and Yahoo fills every other field and symbol; a
    /// provider error or exhausted free tier falls back to Yahoo alone.
    pub async fn get_quote(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let Some(provider) = &self.provider else {
            return self.yahoo_quote(symbols).await;
        };
        let covered: Vec<&str> = symbols
            .iter()
            .copied()
            .filter(|s| provider.supports(s))
            .collect();
        // Bulk scans (movers, screeners) stay on Yahoo when the free tier
        // can't price them all: one table never mixes sources or freshness.
        if covered.is_empty() || !provider.can_serve(&covered) {
            return self.yahoo_quote(symbols).await;
        }
        let from_provider = provider.quotes(&covered).await.unwrap_or_default();
        let yahoo = match self.yahoo_quote(symbols).await {
            Ok(q) => q,
            Err(e) if from_provider.is_empty() => return Err(e),
            Err(_) => Vec::new(),
        };
        Ok(combine_quotes(symbols, from_provider, yahoo))
    }

    /// Quotes straight from Yahoo Finance, ignoring any configured provider.
    pub async fn yahoo_quote(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let joined = symbols.join(",");
        let url = build_url(
            "https://query2.finance.yahoo.com/v7/finance/quote",
            None,
            &[("symbols", &joined), ("crumb", CRUMB_PLACEHOLDER)],
        );

        let body = self.fetch(&url, Duration::from_secs(30)).await?; // cache 30s for quotes
        let resp: QuoteResponse =
            serde_json::from_str(&body).context("Failed to parse quote response")?;
        Ok(resp
            .quote_response
            .result
            .into_iter()
            .filter(|q| !q.is_placeholder())
            .map(|mut q| {
                // Every reader takes the yield from this field; make it right.
                q.trailing_annual_dividend_yield = q.dividend_yield_fraction();
                q
            })
            .collect())
    }

    /// Price history. A configured data provider serves daily, weekly and
    /// monthly bars when its plan covers the whole range; otherwise (and for
    /// intraday bars) Yahoo does.
    pub async fn get_chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        if let Some(provider) = &self.provider
            && provider.supports(symbol)
            && let Ok(chart) = provider.chart(symbol, range, interval).await
        {
            return Ok(chart);
        }
        self.yahoo_chart(symbol, range, interval).await
    }

    /// Price history straight from Yahoo Finance.
    pub async fn yahoo_chart(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> Result<ChartData> {
        // Yahoo has no "1w" range; StockWise accepts it as an alias for 5 days.
        let range = if range == "1w" { "5d" } else { range };
        let url = build_url(
            "https://query1.finance.yahoo.com/v8/finance/chart",
            Some(symbol),
            &[("range", range), ("interval", interval)],
        );

        // Cache chart data longer for bigger timeframes
        let ttl = match range {
            "1d" | "5d" => Duration::from_secs(60),
            "1mo" | "3mo" => Duration::from_secs(300),
            _ => Duration::from_secs(600), // 10 min for 1y+
        };

        let body =
            self.fetch(&url, ttl)
                .await
                .map_err(|e| match e.downcast_ref::<HttpStatusError>() {
                    Some(h) if h.status == reqwest::StatusCode::NOT_FOUND => {
                        anyhow::Error::new(NoPriceHistory(no_history_message(symbol)))
                    }
                    _ => e,
                })?;
        let resp: ChartResponse =
            serde_json::from_str(&body).context("Failed to parse chart response")?;

        resp.chart
            .result
            .and_then(|mut r| {
                if r.is_empty() {
                    None
                } else {
                    Some(r.remove(0))
                }
            })
            .context("No chart data returned")
    }

    /// Bars between two instants (Unix seconds), e.g. one past session's
    /// 5-minute bars. Yahoo keeps 5-minute bars for about 60 days and hourly
    /// bars for about two years.
    pub async fn get_chart_between(
        &self,
        symbol: &str,
        start: i64,
        end: i64,
        interval: &str,
    ) -> Result<ChartData> {
        let (period1, period2) = (start.to_string(), end.to_string());
        let url = build_url(
            "https://query1.finance.yahoo.com/v8/finance/chart",
            Some(symbol),
            &[
                ("period1", &period1),
                ("period2", &period2),
                ("interval", interval),
            ],
        );
        let body = self.fetch(&url, Duration::from_secs(600)).await?;
        let resp: ChartResponse =
            serde_json::from_str(&body).context("Failed to parse chart response")?;
        resp.chart
            .result
            .and_then(|r| r.into_iter().next())
            .context("No chart data returned")
    }

    /// Dividends and splits for `symbol` from `since` (Unix seconds) to today.
    pub async fn get_corporate_actions(&self, symbol: &str, since: i64) -> Result<ChartEvents> {
        // End at the next midnight so the URL (and its cache entry) is stable for the day.
        let day = 86_400;
        let until = (chrono::Utc::now().timestamp() / day + 1) * day;
        // At least a month back, so the range always has a monthly bar.
        let since = since.min(until - 40 * day);
        let (from, to) = (since.to_string(), until.to_string());
        let url = build_url(
            "https://query1.finance.yahoo.com/v8/finance/chart",
            Some(symbol),
            &[
                ("period1", from.as_str()),
                ("period2", to.as_str()),
                ("interval", "1mo"),
                ("events", "div,splits"),
            ],
        );
        let body = self.fetch(&url, Duration::from_secs(600)).await?;
        let resp: ChartResponse =
            serde_json::from_str(&body).context("Failed to parse chart response")?;
        let data = resp
            .chart
            .result
            .and_then(|mut r| (!r.is_empty()).then(|| r.remove(0)))
            .with_context(|| format!("No data for {}", symbol))?;
        let mut events = data.events.unwrap_or_default();
        events.currency = data.meta.and_then(|m| m.currency);
        Ok(events)
    }

    /// Crumb for URLs you build yourself. It is a placeholder: `fetch` swaps in
    /// the real crumb when the request goes out, so building a URL never
    /// touches the network.
    pub fn crumb(&self) -> &str {
        CRUMB_PLACEHOLDER
    }

    pub async fn raw_get(&self, url: &str) -> Result<serde_json::Value> {
        let body = self.fetch(url, Duration::from_secs(60)).await?;
        let resp: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse JSON")?;
        Ok(resp)
    }

    /// Fetch raw text (for RSS/XML endpoints)
    pub async fn raw_get_text(&self, url: &str) -> Result<String> {
        self.fetch(url, Duration::from_secs(300)).await
    }

    /// Yahoo `quoteSummary` — returns the first result object containing the
    /// requested modules (e.g. `financialData`, `earningsTrend`, ...).
    /// Cached for 15 minutes; these modules change at most daily.
    pub async fn quote_summary(&self, symbol: &str, modules: &[&str]) -> Result<serde_json::Value> {
        let url = format!(
            "https://query2.finance.yahoo.com/v10/finance/quoteSummary/{}?modules={}&crumb={}",
            symbol,
            modules.join(","),
            CRUMB_PLACEHOLDER
        );
        let body = self.fetch(&url, Duration::from_secs(900)).await?;
        let mut resp: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse quoteSummary response")?;
        resp.get_mut("quoteSummary")
            .and_then(|q| q.get_mut("result"))
            .and_then(|r| r.as_array_mut())
            .and_then(|a| {
                if a.is_empty() {
                    None
                } else {
                    Some(a.remove(0))
                }
            })
            .context("quoteSummary returned no data")
    }

    /// Yahoo fundamentals time series (e.g. `quarterlyTotalRevenue`).
    /// Returns the raw `timeseries.result` array. Cached for 1 hour.
    pub async fn fundamentals_timeseries(
        &self,
        symbol: &str,
        types: &[&str],
        days_back: i64,
    ) -> Result<Vec<serde_json::Value>> {
        let now = chrono::Utc::now().timestamp();
        // Round period bounds to the day so the cache key is stable.
        let period2 = now - now.rem_euclid(86_400) + 86_400;
        let period1 = period2 - days_back * 86_400;
        let url = format!(
            "https://query1.finance.yahoo.com/ws/fundamentals-timeseries/v1/finance/timeseries/{}?type={}&period1={}&period2={}&crumb={}",
            symbol,
            types.join(","),
            period1,
            period2,
            CRUMB_PLACEHOLDER
        );
        let body = self.fetch(&url, Duration::from_secs(3600)).await?;
        let resp: serde_json::Value =
            serde_json::from_str(&body).context("Failed to parse timeseries response")?;
        Ok(resp
            .get("timeseries")
            .and_then(|t| t.get("result"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default())
    }

    /// GET from NSE India (JSON APIs + archives). NSE rejects requests without
    /// a browser-like Referer. (The cookie store keeps Yahoo's cookie to yahoo.com.)
    pub async fn nse_get_text(&self, url: &str, ttl: Duration) -> Result<String> {
        self.fetch_with(
            url,
            ttl,
            &[
                ("Referer", "https://www.nseindia.com/"),
                ("Accept", "application/json, text/plain, */*"),
                ("Accept-Language", "en-US,en;q=0.9"),
            ],
        )
        .await
    }

    pub async fn nse_get_json(&self, url: &str, ttl: Duration) -> Result<serde_json::Value> {
        let body = self.nse_get_text(url, ttl).await?;
        serde_json::from_str(&body).context("Failed to parse NSE response")
    }

    /// Yahoo symbols of an NSE index's members (`NIFTY 50` →
    /// `RELIANCE.NS`, ...), from NSE whichever provider is configured. With
    /// an NSE-backed provider the members' quotes come along, so quoting
    /// them right after costs no further NSE requests. Errors are not
    /// announced: the caller decides what to fall back to.
    pub async fn index_constituents(&self, index: &str) -> Result<Vec<String>> {
        self.nse_client().constituents(index).await
    }

    /// An NSE index with its members' quotes from one bulk request (NIFTY 500
    /// prices 500 stocks at once). No Yahoo call; errors are not announced.
    pub async fn index_snapshot(&self, index: &str) -> Result<providers::nse::IndexSnapshot> {
        self.nse_client().index_snapshot(index).await
    }

    fn nse_client(&self) -> &providers::nse::Nse {
        match self.provider.as_ref().and_then(|p| p.nse()) {
            Some(nse) => nse,
            None => self.nse.get_or_init(|| {
                let nse = providers::nse::Nse::new(providers::nse::shared_session());
                nse.session().set_cache(self.cache.clone());
                nse
            }),
        }
    }

    /// Symbol search: the data provider's matches when it has any, else
    /// Yahoo's.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        if let Some(provider) = &self.provider
            && let Ok(results) = provider.search(query).await
            && !results.is_empty()
        {
            return Ok(results);
        }
        self.yahoo_search(query).await
    }

    /// Symbol search straight from Yahoo Finance.
    pub async fn yahoo_search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let url = build_url(
            "https://query2.finance.yahoo.com/v1/finance/search",
            None,
            &[("q", query), ("quotesCount", "10"), ("newsCount", "0")],
        );
        let body = self.fetch(&url, Duration::from_secs(120)).await?;
        let resp: SearchResponse =
            serde_json::from_str(&body).context("Failed to parse search response")?;
        Ok(resp.quotes)
    }
}

impl MarketData for YahooClient {
    fn name(&self) -> &'static str {
        "Yahoo Finance"
    }

    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        self.yahoo_quote(symbols).await
    }

    async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        self.yahoo_chart(symbol, range, interval).await
    }

    async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        self.yahoo_search(query).await
    }
}

/// Merge provider quotes into Yahoo's, in Yahoo's order, keeping symbols
/// only one side returned. Symbols match case-insensitively.
fn combine_quotes(symbols: &[&str], from_provider: Vec<Quote>, yahoo: Vec<Quote>) -> Vec<Quote> {
    let same = |q: &Quote, s: &str| {
        q.symbol
            .as_deref()
            .is_some_and(|x| x.eq_ignore_ascii_case(s))
    };
    let mut provider = from_provider;
    let mut yahoo = yahoo;
    let mut out = Vec::with_capacity(symbols.len());
    for symbol in symbols {
        let y = yahoo
            .iter()
            .position(|q| same(q, symbol))
            .map(|i| yahoo.remove(i));
        match provider.iter().position(|q| same(q, symbol)) {
            Some(i) => {
                let p = provider.remove(i);
                // Keep Yahoo's spelling of the symbol, which callers match on.
                let symbol = y
                    .as_ref()
                    .and_then(|q| q.symbol.clone())
                    .or(p.symbol.clone());
                let mut merged = providers::merge_quote(p, y);
                merged.symbol = symbol;
                out.push(merged);
            }
            None => out.extend(y),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn memory_cache_stays_under_its_caps() {
        let mut cache = HybridCache::in_memory();
        let week = Duration::from_secs(7 * 24 * 3600);

        // A long TTL is capped in memory; Redis would keep the full TTL.
        cache.set("ttl".into(), "v".into(), week).await;
        let capped = cache.memory["ttl"].expires;
        assert!(capped <= Instant::now() + MEMORY_CACHE_MAX_TTL + Duration::from_secs(1));

        for i in 0..(MEMORY_CACHE_MAX_ENTRIES + 20) {
            cache
                .set(format!("key{i}"), format!("value{i}"), week)
                .await;
        }
        assert_eq!(cache.memory.len(), MEMORY_CACHE_MAX_ENTRIES);
        assert!(cache.memory_bytes() <= MEMORY_CACHE_MAX_BYTES);
        // The oldest entries made room for the newest.
        assert!(cache.get("ttl").await.is_none());
        assert!(cache.get("key0").await.is_none());
        let newest = format!("key{}", MEMORY_CACHE_MAX_ENTRIES + 19);
        assert_eq!(
            cache.get(&newest).await.as_deref(),
            Some(format!("value{}", MEMORY_CACHE_MAX_ENTRIES + 19).as_str())
        );
    }

    #[test]
    fn combine_quotes_overlays_provider_prices_in_request_order() {
        let q = |sym: &str, price: f64, src: Option<&str>| Quote {
            symbol: Some(sym.into()),
            regular_market_price: Some(price),
            data_source: src.map(str::to_string),
            ..Default::default()
        };
        let yahoo = vec![q("TCS.NS", 3500.0, None), q("AAPL", 230.0, None)];
        let provider = vec![
            q("AAPL", 236.0, Some("Alpha Vantage")),
            q("MSFT", 410.0, Some("Alpha Vantage")),
        ];
        let out = combine_quotes(&["AAPL", "TCS.NS", "MSFT", "NOPE"], provider, yahoo);
        let got: Vec<(&str, f64, Option<&str>)> = out
            .iter()
            .map(|q| {
                (
                    q.symbol.as_deref().unwrap(),
                    q.regular_market_price.unwrap(),
                    q.data_source.as_deref(),
                )
            })
            .collect();
        assert_eq!(
            got,
            [
                ("AAPL", 236.0, Some("Alpha Vantage")),
                ("TCS.NS", 3500.0, None),
                ("MSFT", 410.0, Some("Alpha Vantage")),
            ]
        );
    }

    #[test]
    fn data_source_is_left_out_of_yahoo_json() {
        let json = serde_json::to_value(Quote::default()).unwrap();
        assert!(json.get("dataSource").is_none());
        let tagged = Quote {
            data_source: Some("Twelve Data".into()),
            ..Default::default()
        };
        assert_eq!(
            serde_json::to_value(tagged).unwrap()["dataSource"],
            "Twelve Data"
        );
    }

    #[test]
    fn build_url_encodes_ampersand_in_symbols() {
        let url = build_url(
            "https://query2.finance.yahoo.com/v7/finance/quote",
            None,
            &[("symbols", "M&M.NS,TCS.NS"), ("crumb", "abc")],
        );
        assert!(url.contains("symbols=M%26M.NS%2CTCS.NS"), "{url}");
        let parsed = reqwest::Url::parse(&url).unwrap();
        let symbols = parsed
            .query_pairs()
            .find(|(k, _)| k == "symbols")
            .unwrap()
            .1;
        assert_eq!(symbols, "M&M.NS,TCS.NS");
    }

    #[test]
    fn build_url_encodes_path_segment() {
        let url = build_url(
            "https://query1.finance.yahoo.com/v8/finance/chart",
            Some("M&M.NS"),
            &[("range", "1mo")],
        );
        let parsed = reqwest::Url::parse(&url).unwrap();
        assert_eq!(
            parsed.path_segments().unwrap().next_back().unwrap(),
            "M&M.NS"
        );
        assert!(url.starts_with("https://query1.finance.yahoo.com/v8/finance/chart/"));
    }

    #[test]
    fn cache_key_drops_crumb() {
        let a = cache_key("https://x.yahoo.com/q?symbols=AAPL&crumb=one");
        let b = cache_key("https://x.yahoo.com/q?symbols=AAPL&crumb=two");
        assert_eq!(a, b);
        assert!(!a.contains("crumb"));
        assert!(a.contains("symbols=AAPL"));
    }

    #[tokio::test]
    async fn failed_request_does_not_expose_crumb_in_error_text() {
        // Port 0 cannot accept a TCP connection; this produces a real
        // reqwest transport error with the original URL attached, offline.
        let url = "http://127.0.0.1:0/v7/finance/quote?crumb=SYNTHETIC_SECRET";
        let error = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .get(url)
            .send()
            .await
            .expect_err("port 0 must reject the request");
        assert!(
            error.url().is_some(),
            "regression requires an error carrying the URL"
        );
        let message = fetch_failure_message(url, error);
        assert!(message.contains("127.0.0.1"));
        assert!(!message.contains("SYNTHETIC_SECRET"));
        assert!(!message.contains("crumb="));
    }

    #[test]
    fn retry_delay_honours_retry_after_but_caps_it() {
        let ra = header::HeaderValue::from_static("3");
        assert_eq!(retry_delay(0, Some(&ra), 500), Duration::from_secs(3));
        let huge = header::HeaderValue::from_static("3600");
        assert_eq!(retry_delay(0, Some(&huge), 500), MAX_RETRY_DELAY);
        assert_eq!(retry_delay(1, None, 500), Duration::from_millis(1000));
    }

    fn to_server(server_url: &str) -> impl Fn(&str) -> String + Send + Sync + 'static + use<> {
        let base = server_url.to_string();
        move |url: &str| {
            let parsed = reqwest::Url::parse(url).unwrap();
            match parsed.query() {
                Some(q) => format!("{}{}?{}", base, parsed.path(), q),
                None => format!("{}{}", base, parsed.path()),
            }
        }
    }

    const CHART_BODY: &str = r#"{"chart":{"result":[{"meta":{"symbol":"TCS.NS","regularMarketPrice":3500.0},"timestamp":[1],"indicators":{"quote":[{"close":[3500.0]}]}}],"error":null}}"#;

    #[tokio::test]
    async fn chart_works_when_the_crumb_handshake_is_rate_limited() {
        // Every request answers 429 except the first, the chart one.
        let server = crate::testutil::MockServer::start(vec![
            (200, CHART_BODY.into()),
            (429, "Too Many Requests".into()),
        ])
        .await;
        let client = YahooClient::lazy_for_tests(None, to_server(&server.url));

        let chart = client.get_chart("TCS.NS", "1mo", "1d").await.unwrap();
        assert_eq!(chart.meta.unwrap().regular_market_price, Some(3500.0));
        let reqs = server.requests();
        assert_eq!(reqs.len(), 1, "chart needs no cookie or crumb: {reqs:?}");
        assert!(reqs[0].target.starts_with("/v8/finance/chart/TCS.NS"));
        assert!(!reqs[0].target.contains("crumb"));

        // A crumb endpoint reports the handshake failure.
        let err = client.quote_summary("TCS.NS", &["financialData"]).await;
        let msg = format!("{:#}", err.unwrap_err());
        assert!(msg.contains("rate limited"), "{msg}");
    }

    #[tokio::test]
    async fn a_failed_handshake_is_remembered_for_a_while() {
        let server =
            crate::testutil::MockServer::start(vec![(429, "Too Many Requests".into())]).await;
        let client = YahooClient::lazy_for_tests(None, to_server(&server.url));
        let modules = ["financialData"];

        let first = client.quote_summary("AAA.NS", &modules).await.unwrap_err();
        let after_one = server.requests().len();
        assert!(format!("{first:#}").contains("rate limited"), "{first:#}");
        assert_eq!(
            after_one,
            1 + MAX_RETRIES as usize,
            "one cookie try, three crumb tries"
        );

        for i in 0..10 {
            let err = client
                .quote_summary(&format!("S{i}.NS"), &modules)
                .await
                .unwrap_err();
            assert_eq!(format!("{err:#}"), format!("{first:#}"));
        }
        assert_eq!(server.requests().len(), after_one, "no further handshakes");

        // Once the memory expires the next call tries again.
        *client.handshake_failure.lock().unwrap() =
            Some((Instant::now() - HANDSHAKE_FAILURE_TTL, "old".into()));
        client.quote_summary("BBB.NS", &modules).await.unwrap_err();
        assert!(server.requests().len() > after_one);
    }

    #[tokio::test]
    async fn chart_404_names_the_symbol_and_suggests_a_ticker() {
        let server = crate::testutil::MockServer::start(vec![
            (
                404,
                r#"{"chart":{"result":null,"error":{"code":"Not Found"}}}"#.into(),
            ),
            (404, "{}".into()),
        ])
        .await;
        let client = YahooClient::lazy_for_tests(None, to_server(&server.url));

        let err = client
            .get_chart("RELIANC.NS", "1mo", "1d")
            .await
            .unwrap_err();
        // Callers that skip a symbol Yahoo has no chart for still see a 404.
        assert!(is_not_found(&err));
        let msg = format!("{err:#}");
        assert!(msg.contains("No price history for RELIANC.NS"), "{msg}");
        assert!(msg.contains("stockwise search RELIANC"), "{msg}");
        assert!(msg.contains("Did you mean: RELIANCE.NS"), "{msg}");
        assert!(!msg.contains("HTTP"), "{msg}");

        let msg = client
            .get_chart("ZZQQXX.NS", "1mo", "1d")
            .await
            .unwrap_err()
            .to_string();
        assert!(msg.starts_with("No price history for ZZQQXX.NS"), "{msg}");
        assert!(!msg.contains("Did you mean"), "{msg}");
    }

    #[tokio::test]
    async fn crumb_endpoints_open_the_session_once_and_send_the_crumb() {
        let summary = r#"{"quoteSummary":{"result":[{"financialData":{}}]}}"#;
        let server = crate::testutil::MockServer::start_with_headers(vec![
            (
                200,
                vec![("set-cookie".into(), "A3=abc; Path=/".into())],
                String::new(),
            ),
            (200, Vec::new(), "CRUMB123".into()),
            (200, Vec::new(), summary.into()),
        ])
        .await;
        let client = YahooClient::lazy_for_tests(None, to_server(&server.url));
        client
            .quote_summary("TCS.NS", &["financialData"])
            .await
            .unwrap();
        client
            .fundamentals_timeseries("TCS.NS", &["annualTotalRevenue"], 400)
            .await
            .ok();
        let reqs = server.requests();
        assert_eq!(reqs[0].target, "/");
        assert_eq!(reqs[1].target, "/v1/test/getcrumb");
        assert!(reqs[2].target.contains("quoteSummary/TCS.NS"));
        assert!(
            reqs[2].target.contains("crumb=CRUMB123"),
            "{}",
            reqs[2].target
        );
        assert_eq!(reqs[2].header("cookie"), Some("A3=abc"));
        let handshakes = reqs
            .iter()
            .filter(|r| r.target.contains("getcrumb"))
            .count();
        assert_eq!(handshakes, 1, "second crumb request reuses the session");
    }

    fn cookie_response(cookie: &str) -> (u16, Vec<(String, String)>, String) {
        (
            200,
            vec![("set-cookie".into(), format!("{cookie}; Path=/"))],
            String::new(),
        )
    }

    // #155: Yahoo ends the session in a long-running process.
    #[tokio::test]
    async fn a_401_renews_the_crumb_and_retries_once() {
        let summary = r#"{"quoteSummary":{"result":[{"financialData":{}}]}}"#;
        let server = crate::testutil::MockServer::start_with_headers(vec![
            cookie_response("A3=old"),
            (200, Vec::new(), "OLDCRUMB".into()),
            (
                401,
                Vec::new(),
                r#"{"finance":{"error":"Invalid Crumb"}}"#.into(),
            ),
            cookie_response("A3=new"),
            (200, Vec::new(), "NEWCRUMB".into()),
            (200, Vec::new(), summary.into()),
        ])
        .await;
        let client = YahooClient::lazy_for_tests(None, to_server(&server.url));
        client
            .quote_summary("TCS.NS", &["financialData"])
            .await
            .expect("the retry with the new crumb succeeds");
        let reqs = server.requests();
        let targets: Vec<&str> = reqs
            .iter()
            .map(|r| r.target.split('?').next().unwrap())
            .collect();
        assert_eq!(
            targets,
            [
                "/",
                "/v1/test/getcrumb",
                "/v10/finance/quoteSummary/TCS.NS",
                "/",
                "/v1/test/getcrumb",
                "/v10/finance/quoteSummary/TCS.NS"
            ]
        );
        assert!(reqs[2].target.contains("crumb=OLDCRUMB"));
        assert!(
            reqs[5].target.contains("crumb=NEWCRUMB"),
            "{}",
            reqs[5].target
        );
        // The cookie store sends the cookie, and the new one replaced the old.
        assert_eq!(reqs[2].header("cookie"), Some("A3=old"));
        assert_eq!(reqs[5].header("cookie"), Some("A3=new"));
        // Later crumb requests reuse the renewed session.
        client
            .quote_summary("INFY.NS", &["financialData"])
            .await
            .unwrap();
        assert_eq!(server.requests().len(), 7);
    }

    #[tokio::test]
    async fn a_401_that_survives_the_renewal_is_an_error_after_one_retry() {
        let server = crate::testutil::MockServer::start_with_headers(vec![
            cookie_response("A3=a"),
            (200, Vec::new(), "CRUMB1".into()),
            (401, Vec::new(), String::new()),
            cookie_response("A3=b"),
            (200, Vec::new(), "CRUMB2".into()),
            (401, Vec::new(), String::new()),
        ])
        .await;
        let client = YahooClient::lazy_for_tests(None, to_server(&server.url));
        let err = client
            .quote_summary("TCS.NS", &["financialData"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 401"), "{err:#}");
        // Handshake, 401, handshake, 401: one renewal, no loop.
        let n = server.requests().len();
        assert_eq!(
            n,
            6,
            "{:?}",
            server
                .requests()
                .iter()
                .map(|r| &r.target)
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn the_crumb_is_not_renewed_more_than_once_a_minute() {
        let server = crate::testutil::MockServer::start_with_headers(vec![
            cookie_response("A3=a"),
            (200, Vec::new(), "CRUMB1".into()),
            (401, Vec::new(), String::new()),
        ])
        .await;
        let mut client = YahooClient::lazy_for_tests(None, to_server(&server.url));
        client.crumb_renew_after = CRUMB_RENEW_AFTER;
        let err = client
            .quote_summary("TCS.NS", &["financialData"])
            .await
            .unwrap_err();
        assert!(err.to_string().contains("HTTP 401"), "{err:#}");
        assert_eq!(
            server.requests().len(),
            3,
            "handshake, then the one refused call"
        );
    }

    #[tokio::test]
    async fn concurrent_401s_share_one_new_handshake() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // `/` hands out cookie n; the crumb is "C<n>"; only crumb C2 works.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let homes = Arc::new(AtomicUsize::new(0));
        let counter = homes.clone();
        tokio::spawn(async move {
            while let Ok((mut stream, _)) = listener.accept().await {
                let counter = counter.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = stream.read(&mut buf).await.unwrap_or(0);
                    let head = String::from_utf8_lossy(&buf[..n]).to_string();
                    let line = head.lines().next().unwrap_or("").to_string();
                    let reply = if line.starts_with("GET / ") {
                        let n = counter.fetch_add(1, Ordering::SeqCst) + 1;
                        format!(
                            "HTTP/1.1 200 X\r\nset-cookie: A3={n}; Path=/\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        )
                    } else if line.contains("getcrumb") {
                        let crumb = format!("C{}", counter.load(Ordering::SeqCst));
                        format!(
                            "HTTP/1.1 200 X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{crumb}",
                            crumb.len()
                        )
                    } else {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        if line.contains("crumb=C2") {
                            let body = r#"{"quoteSummary":{"result":[{"financialData":{}}]}}"#;
                            format!(
                                "HTTP/1.1 200 X\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                                body.len()
                            )
                        } else {
                            "HTTP/1.1 401 X\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                                .to_string()
                        }
                    };
                    let _ = stream.write_all(reply.as_bytes()).await;
                    let _ = stream.shutdown().await;
                });
            }
        });
        let client = YahooClient::lazy_for_tests(None, to_server(&url));
        let modules = ["financialData"];
        let (a, b, c, d) = tokio::join!(
            client.quote_summary("AAA.NS", &modules),
            client.quote_summary("BBB.NS", &modules),
            client.quote_summary("CCC.NS", &modules),
            client.quote_summary("DDD.NS", &modules),
        );
        for r in [a, b, c, d] {
            r.expect("succeeds after the shared renewal");
        }
        assert_eq!(
            homes.load(Ordering::SeqCst),
            2,
            "the first handshake and one renewal for four callers"
        );
    }

    /// Two commands, no Redis: the second is served from `cache.db` and
    /// sends no request.
    #[tokio::test]
    async fn second_command_is_served_from_cache_db_without_a_request() {
        let body =
            r#"{"quoteResponse":{"result":[{"symbol":"TCS.NS","regularMarketPrice":3500.0}]}}"#;
        let server = crate::testutil::MockServer::start(vec![(200, body.into())]).await;
        let db: crate::db::L2 = Box::leak(Box::new(Mutex::new(
            crate::db::CacheDb::open_in_memory().unwrap(),
        )));
        // Each client is a fresh process as far as the in-memory cache goes.
        let mut prices = Vec::new();
        for _ in 0..2 {
            let mut client = YahooClient::for_tests_with_rewrite(
                reqwest::Client::new(),
                "crumb",
                to_server(&server.url),
            );
            client.cache = Arc::new(tokio::sync::Mutex::new(HybridCache::with_l2(db)));
            let started = Instant::now();
            let quotes = client.get_quote(&["TCS.NS"]).await.unwrap();
            prices.push((quotes[0].regular_market_price, started.elapsed()));
        }
        assert_eq!(prices[0].0, Some(3500.0));
        assert_eq!(prices[1].0, Some(3500.0));
        assert_eq!(
            server.requests().len(),
            1,
            "one HTTP request across two calls"
        );
        assert!(
            prices[1].1 < Duration::from_millis(100),
            "{:?}",
            prices[1].1
        );
    }

    #[tokio::test]
    async fn new_sends_no_request_and_opens_no_session() {
        let client = YahooClient::new().await.unwrap();
        assert!(client.session.lock().await.is_none());
    }

    #[tokio::test]
    async fn provider_quote_works_when_the_yahoo_handshake_fails() {
        let yahoo = crate::testutil::MockServer::start(vec![(429, "no".into())]).await;
        let nse_server = crate::testutil::MockServer::start(vec![
            (
                200,
                include_str!("providers/fixtures/nse_quote_equity.json").into(),
            ),
            (
                200,
                include_str!("providers/fixtures/nse_trade_info.json").into(),
            ),
        ])
        .await;
        let mut site = providers::nse::site(&nse_server.url);
        site.home = None;
        let provider = providers::Provider::Nse(providers::nse::Nse::new(Arc::new(
            providers::session::Session::for_tests(site, providers::session::DiskCache::Off),
        )));
        let client = YahooClient::lazy_for_tests(Some(provider), to_server(&yahoo.url));
        let quotes = client.get_quote(&["RELIANCE.NS"]).await.unwrap();
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].symbol.as_deref(), Some("RELIANCE.NS"));
        assert_eq!(quotes[0].data_source.as_deref(), Some("NSE"));
    }

    #[tokio::test]
    async fn yahoo_quote_drops_none_placeholder_rows() {
        let body = r#"{"quoteResponse":{"result":[
            {"symbol":"500325.BO","quoteType":"NONE"},
            {"symbol":"RELIANCE.BO","quoteType":"EQUITY","regularMarketPrice":1450.0}
        ],"error":null}}"#;
        let server = crate::testutil::MockServer::start(vec![(200, body.into())]).await;
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "c",
            to_server(&server.url),
        );
        let got = client
            .yahoo_quote(&["500325.BO", "RELIANCE.BO"])
            .await
            .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].symbol.as_deref(), Some("RELIANCE.BO"));
        assert!(got[0].is_usable());
    }

    #[test]
    fn dividend_yield_fraction_covers_zero_mismatch_and_normal() {
        let q = |trailing: Option<f64>, reported: Option<f64>| Quote {
            trailing_annual_dividend_yield: trailing,
            dividend_yield: reported,
            ..Default::default()
        };
        // RELIANCE: trailing 0, dividendYield 0.46 (percent).
        let y = q(Some(0.0), Some(0.46)).dividend_yield_fraction().unwrap();
        assert!((y - 0.0046).abs() < 1e-12);
        // INFY: a dollar dividend over a rupee price, 0.00046 against 4.48%.
        let y = q(Some(0.00046), Some(4.48))
            .dividend_yield_fraction()
            .unwrap();
        assert!((y - 0.0448).abs() < 1e-12);
        // Normal: the two agree, so the trailing figure wins.
        let y = q(Some(0.0152), Some(1.5))
            .dividend_yield_fraction()
            .unwrap();
        assert!((y - 0.0152).abs() < 1e-12);
        // Only one of them.
        assert_eq!(q(Some(0.02), None).dividend_yield_fraction(), Some(0.02));
        assert_eq!(q(None, Some(2.0)).dividend_yield_fraction(), Some(0.02));
        // A non-payer stays a zero, an unknown stays unknown.
        assert_eq!(q(Some(0.0), None).dividend_yield_fraction(), Some(0.0));
        assert_eq!(q(None, None).dividend_yield_fraction(), None);
    }

    #[tokio::test]
    async fn yahoo_quote_fixes_the_trailing_dividend_yield() {
        let body = r#"{"quoteResponse":{"result":[
            {"symbol":"RELIANCE.NS","quoteType":"EQUITY","regularMarketPrice":1318.0,
             "trailingAnnualDividendYield":0.0,"dividendYield":0.46},
            {"symbol":"INFY.NS","quoteType":"EQUITY","regularMarketPrice":1181.0,
             "trailingAnnualDividendYield":0.00046,"dividendYield":4.48}
        ],"error":null}}"#;
        let server = crate::testutil::MockServer::start(vec![(200, body.into())]).await;
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "c",
            to_server(&server.url),
        );
        let got = client
            .yahoo_quote(&["RELIANCE.NS", "INFY.NS"])
            .await
            .unwrap();
        let yields: Vec<f64> = got
            .iter()
            .map(|q| q.trailing_annual_dividend_yield.unwrap())
            .collect();
        assert!((yields[0] - 0.0046).abs() < 1e-12);
        assert!((yields[1] - 0.0448).abs() < 1e-12);
    }

    /// A daily chart whose bars open at 09:15 IST (03:45 UTC) on `days`, with
    /// (high, low, close) each; a `None` close is a bar still being formed.
    fn nse_chart(days: &[(&str, f64, f64, Option<f64>)]) -> ChartData {
        let ts = |d: &str| {
            chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(3, 45, 0)
                .unwrap()
                .and_utc()
                .timestamp()
        };
        serde_json::from_value(serde_json::json!({
            "meta": {"gmtoffset": 19800},
            "timestamp": days.iter().map(|d| ts(d.0)).collect::<Vec<_>>(),
            "indicators": {"quote": [{
                "open": days.iter().map(|d| Some(d.1)).collect::<Vec<_>>(),
                "high": days.iter().map(|d| Some(d.1 + 5.0)).collect::<Vec<_>>(),
                "low": days.iter().map(|d| Some(d.2)).collect::<Vec<_>>(),
                "close": days.iter().map(|d| d.3).collect::<Vec<_>>(),
            }]}
        }))
        .unwrap()
    }

    fn utc(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s).unwrap().to_utc()
    }

    #[test]
    fn pivots_skip_the_live_bar_while_the_session_runs() {
        use crate::market::Market;
        // Fri 30 Oct, Mon 2 Nov and today's live bar (Tue 3 Nov).
        let chart = nse_chart(&[
            ("2026-10-30", 100.0, 90.0, Some(95.0)),
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
            ("2026-11-03", 120.0, 108.0, None),
        ]);
        // 11:30 IST on the 3rd: the market is open, so Monday is the last full session.
        let p = chart
            .prior_session(utc("2026-11-03T06:00:00Z"), Market::In)
            .unwrap();
        assert_eq!(
            p.date,
            chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
        );
        assert_eq!((p.high, p.low, p.close), (115.0, 100.0, 105.0));
        // The same instant, a minute later: the same levels (the live bar moved).
        let moved = nse_chart(&[
            ("2026-10-30", 100.0, 90.0, Some(95.0)),
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
            ("2026-11-03", 125.0, 104.0, Some(124.0)),
        ]);
        assert_eq!(
            moved.prior_session(utc("2026-11-03T06:01:00Z"), Market::In),
            Some(p)
        );
        // Before the open (07:30 IST) the 3rd has no bar yet: Monday again.
        let early = nse_chart(&[
            ("2026-10-30", 100.0, 90.0, Some(95.0)),
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
        ]);
        assert_eq!(
            early
                .prior_session(utc("2026-11-03T02:00:00Z"), Market::In)
                .unwrap()
                .close,
            105.0
        );
    }

    #[test]
    fn pivots_use_the_last_bar_after_the_close_and_on_weekends() {
        use crate::market::Market;
        let chart = nse_chart(&[
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
            ("2026-11-03", 120.0, 108.0, Some(118.0)),
        ]);
        // 16:30 IST on the 3rd: the session has closed, so its bar counts.
        let p = chart
            .prior_session(utc("2026-11-03T11:00:00Z"), Market::In)
            .unwrap();
        assert_eq!(p.close, 118.0);
        // Saturday 7 Nov: the last bar is Friday's (here the 3rd), complete.
        assert_eq!(
            chart
                .prior_session(utc("2026-11-07T06:00:00Z"), Market::In)
                .unwrap()
                .close,
            118.0
        );
    }

    #[test]
    fn crypto_never_closes_so_todays_bar_is_always_skipped() {
        use crate::market::Market;
        let mut chart = nse_chart(&[
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
            ("2026-11-03", 120.0, 108.0, Some(118.0)),
        ]);
        chart.meta.as_mut().unwrap().gmtoffset = Some(0);
        // Bars are dated 03:45 + 3h slack, 2 and 3 Nov; at 20:00 UTC on the 3rd
        // the 3rd's bar is still forming.
        let p = chart
            .prior_session(utc("2026-11-03T20:00:00Z"), Market::Crypto)
            .unwrap();
        assert_eq!(p.close, 105.0);
    }

    /// The chart's own symbol picks the market, not the command's (default India).
    #[test]
    fn the_symbols_own_market_decides_whether_a_session_runs() {
        use crate::market::Market;
        let days = [
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
            ("2026-11-03", 120.0, 108.0, None),
        ];
        // BTC-USD at 20:00Z on the 3rd is 01:30 IST on the 4th: judged by
        // Indian hours the bar of the 3rd would look finished.
        let mut btc = nse_chart(&days);
        btc.meta.as_mut().unwrap().gmtoffset = Some(0);
        btc.meta.as_mut().unwrap().symbol = Some("BTC-USD".into());
        let now = utc("2026-11-03T20:00:00Z");
        assert_eq!(btc.prior_session(now, Market::In).unwrap().close, 105.0);
        assert_eq!(btc.prior_session(now, Market::Crypto).unwrap().close, 105.0);
        // ^GSPC at 21:30 IST (11:00 ET) on a weekday: the US session is still
        // open, though the Indian one closed at 15:30 IST.
        let mut spx = nse_chart(&[
            ("2026-11-02", 110.0, 100.0, Some(105.0)),
            ("2026-11-03", 120.0, 108.0, Some(118.0)),
        ]);
        spx.meta.as_mut().unwrap().gmtoffset = Some(-18_000);
        spx.meta.as_mut().unwrap().symbol = Some("^GSPC".into());
        let now = utc("2026-11-03T16:00:00Z");
        assert_eq!(spx.prior_session(now, Market::In).unwrap().close, 105.0);
        // Without a symbol on the chart the caller's market is all there is.
        spx.meta.as_mut().unwrap().symbol = None;
        assert_eq!(spx.prior_session(now, Market::In).unwrap().close, 118.0);
    }

    /// The DST slack must not push a late UTC bar into the next day when the
    /// chart has no exchange offset (crypto).
    #[test]
    fn a_bar_stamped_late_in_the_utc_day_keeps_its_date_at_offset_zero() {
        use crate::market::Market;
        let stamp = |s: &str| utc(s).timestamp();
        let mut chart: ChartData = serde_json::from_value(serde_json::json!({
            "meta": {"gmtoffset": 0, "symbol": "BTC-USD"},
            "timestamp": [stamp("2026-11-02T22:30:00Z"), stamp("2026-11-03T22:30:00Z")],
            "indicators": {"quote": [{
                "high": [110.0, 120.0], "low": [100.0, 108.0], "close": [105.0, 118.0]}]}
        }))
        .unwrap();
        let day = |d| chrono::NaiveDate::from_ymd_opt(2026, 11, d).unwrap();
        assert_eq!(
            chart.dated_closes(),
            [(day(2), 105.0), (day(3), 118.0)],
            "22:30Z is still the same UTC day"
        );
        // 01:00Z on the 4th: the bar of the 3rd is complete and is the prior session.
        let p = chart
            .prior_session(utc("2026-11-04T01:00:00Z"), Market::Crypto)
            .unwrap();
        assert_eq!((p.date, p.close), (day(3), 118.0));
        // With an exchange offset the slack stays: 09:15 IST bars date as before.
        chart.meta.as_mut().unwrap().gmtoffset = Some(19_800);
        assert_eq!(
            chart.dated_closes()[0].0,
            day(3),
            "22:30Z is 04:00 IST next day"
        );
    }
}
