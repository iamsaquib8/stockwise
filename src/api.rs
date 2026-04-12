use anyhow::{Context, Result};
use reqwest::header;
use serde::Deserialize;

const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";

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
    // These fields come from v7 quote but may not always be present
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

pub struct YahooClient {
    client: reqwest::Client,
    crumb: String,
    cookie: String,
}

impl YahooClient {
    pub async fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_store(true)
            .connect_timeout(std::time::Duration::from_secs(15))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .context("Failed to build HTTP client")?;

        // Step 1: Hit fc.yahoo.com to get cookies
        let resp: reqwest::Response = client
            .get("https://fc.yahoo.com")
            .send()
            .await
            .context("Failed to initialize Yahoo session")?;

        // Collect Set-Cookie headers
        let cookies: Vec<String> = resp
            .headers()
            .get_all(header::SET_COOKIE)
            .iter()
            .filter_map(|v: &header::HeaderValue| v.to_str().ok())
            .map(|v: &str| {
                v.split(';')
                    .next()
                    .unwrap_or("")
                    .to_string()
            })
            .collect();
        let cookie_str = cookies.join("; ");

        // Step 2: Get crumb
        let crumb: String = client
            .get("https://query2.finance.yahoo.com/v1/test/getcrumb")
            .header(header::COOKIE, &cookie_str)
            .send()
            .await
            .context("Failed to get crumb")?
            .text()
            .await
            .context("Failed to read crumb")?;

        Ok(Self {
            client,
            crumb,
            cookie: cookie_str,
        })
    }

    pub async fn get_quote(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let joined = symbols.join(",");
        let url = format!(
            "https://query2.finance.yahoo.com/v7/finance/quote?symbols={}&crumb={}",
            joined, self.crumb
        );

        let resp: QuoteResponse = self
            .client
            .get(&url)
            .header(header::COOKIE, &self.cookie)
            .send()
            .await
            .context("Failed to fetch quote")?
            .json()
            .await
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

        let resp: ChartResponse = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch chart data")?
            .json()
            .await
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
        let resp: serde_json::Value = self
            .client
            .get(url)
            .header(header::COOKIE, &self.cookie)
            .send()
            .await
            .context("Failed to fetch")?
            .json()
            .await
            .context("Failed to parse JSON")?;
        Ok(resp)
    }

    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let url = format!(
            "https://query2.finance.yahoo.com/v1/finance/search?q={}&quotesCount=10&newsCount=0",
            query
        );
        let resp: SearchResponse = self
            .client
            .get(&url)
            .header(header::COOKIE, &self.cookie)
            .send()
            .await
            .context("Failed to search")?
            .json()
            .await
            .context("Failed to parse search response")?;
        Ok(resp.quotes)
    }
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
