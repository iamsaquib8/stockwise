use anyhow::{Context, Result, bail};
use reqwest::header;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";
const MAX_RETRIES: u32 = 3;
const MIN_REQUEST_INTERVAL_MS: u64 = 200; // 5 requests/second max

// ── Response Cache ──

struct CacheEntry {
    data: String,
    expires: Instant,
}

struct ResponseCache {
    entries: HashMap<String, CacheEntry>,
}

impl ResponseCache {
    fn new() -> Self {
        Self { entries: HashMap::new() }
    }

    fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).and_then(|e| {
            if Instant::now() < e.expires {
                Some(e.data.as_str())
            } else {
                None
            }
        })
    }

    fn set(&mut self, key: String, data: String, ttl: Duration) {
        // Evict expired entries periodically
        if self.entries.len() > 200 {
            let now = Instant::now();
            self.entries.retain(|_, v| now < v.expires);
        }
        self.entries.insert(key, CacheEntry {
            data,
            expires: Instant::now() + ttl,
        });
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

#[derive(Debug, Clone, Default, Deserialize)]
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

#[derive(Debug, Deserialize)]
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
    cache: Mutex<ResponseCache>,
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
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(e) => bail!("Failed to connect to Yahoo Finance after {} retries: {}", MAX_RETRIES, e),
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
                Ok(resp) => {
                    crumb = resp.text().await.context("Failed to read crumb")?;
                    break;
                }
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(e) => bail!("Failed to get crumb after {} retries: {}", MAX_RETRIES, e),
            }
        }

        Ok(Self {
            client,
            crumb,
            cookie: cookie_str,
            last_request: Mutex::new(Instant::now() - Duration::from_secs(1)),
            cache: Mutex::new(ResponseCache::new()),
        })
    }

    /// Rate-limited GET with retry and caching
    async fn fetch(&self, url: &str, cache_ttl: Duration) -> Result<String> {
        // Check cache first
        {
            let cache = self.cache.lock().unwrap();
            if let Some(cached) = cache.get(url) {
                return Ok(cached.to_string());
            }
        }

        // Rate limit: wait if too soon after last request
        {
            let mut last = self.last_request.lock().unwrap();
            let elapsed = last.elapsed();
            let min_interval = Duration::from_millis(MIN_REQUEST_INTERVAL_MS);
            if elapsed < min_interval {
                drop(last); // release lock before sleeping
                tokio::time::sleep(min_interval - elapsed).await;
                let mut last = self.last_request.lock().unwrap();
                *last = Instant::now();
            } else {
                *last = Instant::now();
            }
        }

        // Fetch with retry + exponential backoff
        for attempt in 0..MAX_RETRIES {
            let result = self
                .client
                .get(url)
                .header(header::COOKIE, &self.cookie)
                .send()
                .await;

            match result {
                Ok(resp) => {
                    let status = resp.status();
                    if status.is_success() {
                        let body = resp.text().await.context("Failed to read response body")?;
                        // Cache the response
                        let mut cache = self.cache.lock().unwrap();
                        cache.set(url.to_string(), body.clone(), cache_ttl);
                        return Ok(body);
                    } else if status.as_u16() == 429 {
                        // Rate limited — back off aggressively
                        let delay = Duration::from_secs(2u64.pow(attempt + 1));
                        eprintln!("  Rate limited by Yahoo Finance. Waiting {}s...", delay.as_secs());
                        tokio::time::sleep(delay).await;
                        continue;
                    } else if status.as_u16() >= 500 {
                        // Server error — retry
                        let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                        tokio::time::sleep(delay).await;
                        continue;
                    } else {
                        bail!("Yahoo Finance returned HTTP {}", status);
                    }
                }
                Err(e) if attempt < MAX_RETRIES - 1 => {
                    let delay = Duration::from_millis(500 * 2u64.pow(attempt));
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(e) => bail!("Request failed after {} retries: {}", MAX_RETRIES, e),
            }
        }
        bail!("Request failed after {} retries", MAX_RETRIES)
    }

    pub async fn get_quote(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let joined = symbols.join(",");
        let url = format!(
            "https://query2.finance.yahoo.com/v7/finance/quote?symbols={}&crumb={}",
            joined, self.crumb
        );

        let body = self.fetch(&url, Duration::from_secs(30)).await?; // cache 30s for quotes
        let resp: QuoteResponse = serde_json::from_str(&body)
            .context("Failed to parse quote response")?;
        Ok(resp.quote_response.result)
    }

    pub async fn get_chart(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> Result<ChartData> {
        let url = format!(
            "https://query1.finance.yahoo.com/v8/finance/chart/{}?range={}&interval={}",
            symbol, range, interval
        );

        // Cache chart data longer for bigger timeframes
        let ttl = match range {
            "1d" | "5d" => Duration::from_secs(60),
            "1mo" | "3mo" => Duration::from_secs(300),
            _ => Duration::from_secs(600), // 10 min for 1y+
        };

        let body = self.fetch(&url, ttl).await?;
        let resp: ChartResponse = serde_json::from_str(&body)
            .context("Failed to parse chart response")?;

        resp.chart
            .result
            .and_then(|mut r| if r.is_empty() { None } else { Some(r.remove(0)) })
            .context("No chart data returned")
    }

    pub fn crumb(&self) -> &str {
        &self.crumb
    }

    pub async fn raw_get(&self, url: &str) -> Result<serde_json::Value> {
        let body = self.fetch(url, Duration::from_secs(60)).await?;
        let resp: serde_json::Value = serde_json::from_str(&body)
            .context("Failed to parse JSON")?;
        Ok(resp)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://query2.finance.yahoo.com/v1/finance/search?q={}&quotesCount=10&newsCount=0",
            query
        );
        let body = self.fetch(&url, Duration::from_secs(120)).await?;
        let resp: SearchResponse = serde_json::from_str(&body)
            .context("Failed to parse search response")?;
        Ok(resp.quotes)
    }
}
