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
    pub cache_hits_memory: AtomicU64,
    pub cache_misses: AtomicU64,
}

pub static STATS: FetchStats = FetchStats {
    requests: AtomicU64::new(0),
    errors: AtomicU64::new(0),
    cache_hits_redis: AtomicU64::new(0),
    cache_hits_memory: AtomicU64::new(0),
    cache_misses: AtomicU64::new(0),
};

fn count(counter: &AtomicU64) {
    counter.fetch_add(1, Ordering::Relaxed);
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
fn host_of(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or(url)
}

// ── Hybrid Cache: Redis (primary) + In-Memory (fallback) ──

struct CacheEntry {
    data: String,
    expires: Instant,
}

pub(crate) struct HybridCache {
    redis: Option<redis::aio::MultiplexedConnection>,
    memory: HashMap<String, CacheEntry>,
}

/// The cache shared by Yahoo and an alternative data provider.
pub(crate) type SharedCache = Arc<tokio::sync::Mutex<HybridCache>>;

impl HybridCache {
    async fn new() -> Self {
        Self {
            redis: connect_redis().await,
            memory: HashMap::new(),
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
        count(if hit.is_some() {
            &STATS.cache_hits_memory
        } else {
            &STATS.cache_misses
        });
        hit
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

        // Also write to memory (fast path for same-session reads)
        if self.memory.len() > 200 {
            let now = Instant::now();
            self.memory.retain(|_, v| now < v.expires);
        }
        self.memory.insert(
            key,
            CacheEntry {
                data,
                expires: Instant::now() + ttl,
            },
        );
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct Quote {
    pub symbol: Option<String>,
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

pub struct YahooClient {
    client: reqwest::Client,
    crumb: String,
    cookie: String,
    last_request: Mutex<Instant>,
    cache: SharedCache,
    /// Opt-in alternative provider (`STOCKWISE_DATA_PROVIDER`). Quotes,
    /// charts and search go there first; Yahoo fills the gaps.
    provider: Option<providers::Provider>,
    /// NSE, for index members when the provider isn't NSE-backed.
    nse: std::sync::OnceLock<providers::nse::Nse>,
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

        // Step 1: Get cookies — retry up to 3 times
        let mut cookie_str = String::new();
        for attempt in 0..MAX_RETRIES {
            match client.get("https://fc.yahoo.com").send().await {
                Ok(resp) => {
                    let cookies: Vec<String> = resp
                        .headers()
                        .get_all(header::SET_COOKIE)
                        .iter()
                        .filter_map(|v: &header::HeaderValue| v.to_str().ok())
                        .map(|v: &str| v.split(';').next().unwrap_or("").to_string())
                        .collect();
                    cookie_str = cookies.join("; ");
                    break;
                }
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
            match client
                .get("https://query2.finance.yahoo.com/v1/test/getcrumb")
                .header(header::COOKIE, &cookie_str)
                .send()
                .await
            {
                Ok(resp) if resp.status().is_success() => {
                    crumb = resp.text().await.context("Failed to read crumb")?;
                    // A real crumb is a short token; anything else is an error page.
                    if !crumb.is_empty() && crumb.len() < 64 && !crumb.contains(char::is_whitespace)
                    {
                        break;
                    }
                    if attempt == MAX_RETRIES - 1 {
                        bail!(
                            "Yahoo Finance returned an invalid session token. Try again shortly."
                        );
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
            crumb,
            cookie: cookie_str,
            last_request: Mutex::new(Instant::now() - Duration::from_secs(1)),
            cache,
            provider,
            nse: std::sync::OnceLock::new(),
        })
    }

    /// Rate-limited GET with retry and caching (Redis + in-memory)
    async fn fetch(&self, url: &str, cache_ttl: Duration) -> Result<String> {
        self.fetch_with(url, cache_ttl, &[], true).await
    }

    /// Rate-limited GET with retry and caching, with optional extra headers.
    /// `yahoo_auth` controls whether the Yahoo session cookie is sent — it is
    /// disabled for third-party hosts (e.g. NSE) so we don't leak it.
    pub async fn fetch_with(
        &self,
        url: &str,
        cache_ttl: Duration,
        extra_headers: &[(&str, &str)],
        yahoo_auth: bool,
    ) -> Result<String> {
        let result = self
            .fetch_uncounted_with(url, cache_ttl, extra_headers, yahoo_auth)
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
        yahoo_auth: bool,
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
            let min_interval = Duration::from_millis(MIN_REQUEST_INTERVAL_MS);
            let next = (*last + min_interval).max(Instant::now());
            *last = next;
            next
        };
        let now = Instant::now();
        if sleep_until > now {
            tokio::time::sleep(sleep_until - now).await;
        }

        // Yahoo session cookies go to Yahoo only (news also comes from Google).
        let is_yahoo = yahoo_auth
            && reqwest::Url::parse(url)
                .ok()
                .and_then(|u| {
                    u.host_str()
                        .map(|h| h == "yahoo.com" || h.ends_with(".yahoo.com"))
                })
                .unwrap_or(false);

        // Fetch with retry + exponential backoff
        let mut last_status = None;
        for attempt in 0..MAX_RETRIES {
            let last_attempt = attempt == MAX_RETRIES - 1;
            let mut req = self.client.get(url);
            if is_yahoo {
                req = req.header(header::COOKIE, &self.cookie);
            }
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
                    } else {
                        bail!("{} returned HTTP {}", host_of(url), status);
                    }
                }
                Err(_e) if !last_attempt => {
                    tokio::time::sleep(retry_delay(attempt, None, 500)).await;
                }
                Err(e) => bail!(
                    "Request to {} failed after {} attempts: {}",
                    host_of(url),
                    MAX_RETRIES,
                    e
                ),
            }
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
            &[("symbols", &joined), ("crumb", &self.crumb)],
        );

        let body = self.fetch(&url, Duration::from_secs(30)).await?; // cache 30s for quotes
        let resp: QuoteResponse =
            serde_json::from_str(&body).context("Failed to parse quote response")?;
        Ok(resp.quote_response.result)
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

        let body = self.fetch(&url, ttl).await?;
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

    pub fn crumb(&self) -> &str {
        &self.crumb
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
            self.crumb
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
            self.crumb
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
    /// a browser-like Referer, and must not receive the Yahoo cookie.
    pub async fn nse_get_text(&self, url: &str, ttl: Duration) -> Result<String> {
        self.fetch_with(
            url,
            ttl,
            &[
                ("Referer", "https://www.nseindia.com/"),
                ("Accept", "application/json, text/plain, */*"),
                ("Accept-Language", "en-US,en;q=0.9"),
            ],
            false,
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
        let nse = match self.provider.as_ref().and_then(|p| p.nse()) {
            Some(nse) => nse,
            None => self.nse.get_or_init(|| {
                let nse = providers::nse::Nse::new(providers::nse::shared_session());
                nse.session().set_cache(self.cache.clone());
                nse
            }),
        };
        nse.constituents(index).await
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

    #[test]
    fn retry_delay_honours_retry_after_but_caps_it() {
        let ra = header::HeaderValue::from_static("3");
        assert_eq!(retry_delay(0, Some(&ra), 500), Duration::from_secs(3));
        let huge = header::HeaderValue::from_static("3600");
        assert_eq!(retry_delay(0, Some(&huge), 500), MAX_RETRY_DELAY);
        assert_eq!(retry_delay(1, None, 500), Duration::from_millis(1000));
    }
}
