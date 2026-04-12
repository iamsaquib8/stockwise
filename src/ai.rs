use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

const OLLAMA_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "qwen3:14b";
const AI_CACHE_PREFIX: &str = "stockwise:ai:";
const AI_CACHE_TTL_SECS: i64 = 3600; // 1 hour for AI responses

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: Option<String>,
}

fn hash_prompt(model: &str, prompt: &str) -> String {
    let mut hasher = DefaultHasher::new();
    model.hash(&mut hasher);
    prompt.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub struct AiClient {
    client: reqwest::Client,
    model: String,
    redis: Option<redis::aio::MultiplexedConnection>,
}

impl AiClient {
    pub async fn connect() -> Self {
        let model = std::env::var("STOCKWISE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let redis = match redis::Client::open("redis://127.0.0.1:6379/") {
            Ok(c) => c.get_multiplexed_tokio_connection().await.ok(),
            Err(_) => None,
        };
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap(),
            model,
            redis,
        }
    }

    // Sync constructor for backward compat (no Redis)
    pub fn new() -> Self {
        let model = std::env::var("STOCKWISE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap(),
            model,
            redis: None,
        }
    }

    pub async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", OLLAMA_URL))
            .send()
            .await
            .is_ok()
    }

    async fn cache_get(&mut self, key: &str) -> Option<String> {
        if let Some(ref mut conn) = self.redis {
            redis::cmd("GET").arg(key).query_async::<Option<String>>(conn).await.ok().flatten()
        } else {
            None
        }
    }

    async fn cache_set(&mut self, key: &str, value: &str) {
        if let Some(ref mut conn) = self.redis {
            let _: Result<(), _> = redis::cmd("SETEX")
                .arg(key)
                .arg(AI_CACHE_TTL_SECS)
                .arg(value)
                .query_async(conn)
                .await;
        }
    }

    pub async fn generate(&mut self, prompt: &str) -> Result<String> {
        // Check Redis cache first
        let cache_key = format!("{}{}", AI_CACHE_PREFIX, hash_prompt(&self.model, prompt));
        if let Some(cached) = self.cache_get(&cache_key).await {
            return Ok(cached);
        }

        let req = OllamaRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            stream: false,
        };

        let raw: String = self
            .client
            .post(format!("{}/api/generate", OLLAMA_URL))
            .json(&req)
            .send()
            .await
            .context("Failed to connect to Ollama. Is it running? (ollama serve)")?
            .text()
            .await
            .context("Failed to read Ollama response")?;

        let response_text = if let Ok(resp) = serde_json::from_str::<OllamaResponse>(&raw) {
            resp.response.unwrap_or_default()
        } else {
            let mut combined = String::new();
            for line in raw.lines() {
                if let Ok(obj) = serde_json::from_str::<OllamaResponse>(line) {
                    if let Some(r) = obj.response {
                        combined.push_str(&r);
                    }
                }
            }
            combined
        };

        // Strip qwen3 <think>...</think> tags
        let cleaned = strip_think_tags(&response_text);
        let result = cleaned.trim().to_string();

        // Cache to Redis (1 hour TTL)
        if !result.is_empty() {
            self.cache_set(&cache_key, &result).await;
        }

        Ok(result)
    }

    pub async fn analyze_stock(&mut self, data: &StockData) -> Result<String> {
        let prompt = format!(
r#"You are a stock market analyst. Analyze this stock and give a concise investment opinion.

Stock: {} ({})
Price: {:.2} | Change: {:+.2}%
P/E: {} | Forward P/E: {} | P/B: {}
52-Week: {:.2} - {:.2}
Market Cap: {:.0}
EPS TTM: {} | EPS Forward: {}
Dividend Yield: {}%
50-Day MA: {:.2} | 200-Day MA: {:.2}
RSI: {} | Beta: {}
Sector: {} | Industry: {}

Give a 3-4 sentence analysis covering:
1. Current valuation (cheap/fair/expensive)
2. Momentum and trend
3. Key risk
4. Overall verdict (BUY/HOLD/SELL for short-term and long-term)

Be direct and specific. No disclaimers."#,
            data.symbol, data.name, data.price, data.change_pct,
            data.pe.map_or("N/A".to_string(), |v| format!("{:.1}", v)),
            data.forward_pe.map_or("N/A".to_string(), |v| format!("{:.1}", v)),
            data.pb.map_or("N/A".to_string(), |v| format!("{:.1}", v)),
            data.week52_low, data.week52_high, data.market_cap,
            data.eps_ttm.map_or("N/A".to_string(), |v| format!("{:.2}", v)),
            data.eps_fwd.map_or("N/A".to_string(), |v| format!("{:.2}", v)),
            data.div_yield.map_or("N/A".to_string(), |v| format!("{:.2}", v * 100.0)),
            data.ma50, data.ma200,
            data.rsi.map_or("N/A".to_string(), |v| format!("{:.0}", v)),
            data.beta.map_or("N/A".to_string(), |v| format!("{:.2}", v)),
            data.sector, data.industry,
        );
        self.generate(&prompt).await
    }

    pub async fn generate_intraday_report(&mut self, report_data: &str) -> Result<String> {
        let prompt = format!(
r#"You are an expert intraday trader. Based on this scan data, write a concise morning trading brief.

{}

Write a 5-7 sentence trading brief covering:
1. Market regime and overall sentiment today
2. Top 2-3 actionable trades with entry/exit reasoning
3. Which sectors to focus on
4. Key risk to watch today
5. Overall confidence level

Be specific with price levels. Write like a professional trading desk note."#,
            report_data
        );
        self.generate(&prompt).await
    }

    pub async fn generate_longterm_report(&mut self, report_data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a long-term investment advisor. Based on this analysis, write an investment memo.

{}

Write a concise investment memo covering:
1. Top 3 stocks to accumulate via SIP and why
2. Key macro risks to the portfolio
3. Suggested allocation strategy
4. 1-year outlook
5. What to avoid

Write like a fund manager's note to clients. Be specific and actionable."#,
            report_data
        );
        self.generate(&prompt).await
    }

    /// Quick 2-3 sentence insight for any data context (fast, for inline use)
    pub async fn quick_insight(&mut self, context: &str) -> Result<String> {
        let prompt = format!(
            "You are a stock analyst. Given this data, give a 2-3 sentence actionable insight. Be specific with numbers. No disclaimers.\n\n{}",
            context
        );
        self.generate(&prompt).await
    }

    /// Technical analysis interpretation
    pub async fn interpret_technicals(&mut self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a technical analyst. Interpret these indicators together and give a clear trade signal.

{}

In 3-4 sentences:
1. What do these indicators mean together? (confluent or conflicting?)
2. What's the most likely price action?
3. Specific entry/exit suggestion
Be direct."#, data);
        self.generate(&prompt).await
    }

    /// Compare two or more stocks
    pub async fn compare_stocks(&mut self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are an investment analyst. Compare these stocks and pick a winner.

{}

In 3-4 sentences: Which stock is the best investment right now and why? Be specific about valuation, growth, and risk. Give a clear verdict."#, data);
        self.generate(&prompt).await
    }

    /// Backtest interpretation
    pub async fn interpret_backtest(&mut self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a quantitative analyst. Interpret these backtest results.

{}

In 3-4 sentences: Is this strategy viable? What market conditions would it work best in? Key risks? Would you deploy real capital on it?"#, data);
        self.generate(&prompt).await
    }

    /// Screen results analysis
    pub async fn analyze_screen(&mut self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a stock screener analyst. Analyze these screened stocks.

{}

In 3-4 sentences: Which 2-3 stocks stand out most? Any value traps to avoid? What makes the top picks compelling?"#, data);
        self.generate(&prompt).await
    }

    pub async fn generate_portfolio_report(&mut self, report_data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a portfolio analyst. Review this portfolio and give actionable advice.

{}

Write a concise portfolio review covering:
1. Portfolio health assessment
2. Concentration risk
3. What to buy more of and why
4. What to trim/sell and why
5. Missing exposures (sectors/themes to add)

Be direct and specific with recommendations."#,
            report_data
        );
        self.generate(&prompt).await
    }
}

/// Strip <think>...</think> tags from qwen3-style thinking models
fn strip_think_tags(text: &str) -> String {
    let mut result = text.to_string();
    // Remove <think>...</think> blocks (can be multiline)
    while let Some(start) = result.find("<think>") {
        if let Some(end) = result.find("</think>") {
            result = format!("{}{}", &result[..start], &result[end + 8..]);
        } else {
            // Unclosed think tag — remove from <think> to end
            result = result[..start].to_string();
            break;
        }
    }
    result
}

/// Structured stock data for AI prompts
pub struct StockData {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub change_pct: f64,
    pub pe: Option<f64>,
    pub forward_pe: Option<f64>,
    pub pb: Option<f64>,
    pub week52_low: f64,
    pub week52_high: f64,
    pub market_cap: f64,
    pub eps_ttm: Option<f64>,
    pub eps_fwd: Option<f64>,
    pub div_yield: Option<f64>,
    pub ma50: f64,
    pub ma200: f64,
    pub rsi: Option<f64>,
    pub beta: Option<f64>,
    pub sector: String,
    pub industry: String,
}

impl StockData {
    pub fn from_quote(q: &crate::api::Quote) -> Self {
        Self {
            symbol: q.symbol.clone().unwrap_or_default(),
            name: q.short_name.clone().or(q.long_name.clone()).unwrap_or_default(),
            price: q.regular_market_price.unwrap_or(0.0),
            change_pct: q.regular_market_change_percent.unwrap_or(0.0),
            pe: q.trailing_pe,
            forward_pe: q.forward_pe,
            pb: q.price_to_book,
            week52_low: q.fifty_two_week_low.unwrap_or(0.0),
            week52_high: q.fifty_two_week_high.unwrap_or(0.0),
            market_cap: q.market_cap.unwrap_or(0.0),
            eps_ttm: q.eps_trailing_twelve_months,
            eps_fwd: q.eps_forward,
            div_yield: q.trailing_annual_dividend_yield,
            ma50: q.fifty_day_average.unwrap_or(0.0),
            ma200: q.two_hundred_day_average.unwrap_or(0.0),
            rsi: None, // set separately from chart data
            beta: q.beta,
            sector: q.sector.clone().unwrap_or_default(),
            industry: q.industry.clone().unwrap_or_default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Quote;

    #[test]
    fn test_strip_think_tags_basic() {
        let input = "<think>reasoning here</think>actual answer";
        assert_eq!(strip_think_tags(input), "actual answer");
    }

    #[test]
    fn test_strip_think_tags_multiline() {
        let input = "<think>\nline 1\nline 2\n</think>\nreal output";
        assert_eq!(strip_think_tags(input), "\nreal output");
    }

    #[test]
    fn test_strip_think_tags_none() {
        let input = "normal response without tags";
        assert_eq!(strip_think_tags(input), "normal response without tags");
    }

    #[test]
    fn test_strip_think_tags_unclosed() {
        let input = "before<think>incomplete";
        assert_eq!(strip_think_tags(input), "before");
    }

    #[test]
    fn test_strip_think_tags_empty_input() {
        assert_eq!(strip_think_tags(""), "");
    }

    #[test]
    fn test_strip_multiple_think_blocks() {
        let input = "<think>first</think>middle<think>second</think>end";
        assert_eq!(strip_think_tags(input), "middleend");
    }

    #[test]
    fn test_strip_think_tags_empty_block() {
        let input = "<think></think>clean";
        assert_eq!(strip_think_tags(input), "clean");
    }

    #[test]
    fn test_hash_prompt_deterministic() {
        let h1 = hash_prompt("qwen3:14b", "analyze RELIANCE");
        let h2 = hash_prompt("qwen3:14b", "analyze RELIANCE");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_hash_prompt_different_prompts() {
        let h1 = hash_prompt("qwen3:14b", "prompt A");
        let h2 = hash_prompt("qwen3:14b", "prompt B");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_prompt_different_models() {
        let h1 = hash_prompt("model1", "same prompt");
        let h2 = hash_prompt("model2", "same prompt");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_hash_prompt_hex_format() {
        let h = hash_prompt("model", "prompt");
        // Should be non-empty hex string
        assert!(!h.is_empty());
        assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_stock_data_from_quote() {
        let q = Quote {
            symbol: Some("TEST.NS".into()),
            short_name: Some("Test Corp".into()),
            regular_market_price: Some(100.0),
            regular_market_change_percent: Some(1.5),
            trailing_pe: Some(20.0),
            forward_pe: Some(18.0),
            price_to_book: Some(3.0),
            fifty_two_week_low: Some(80.0),
            fifty_two_week_high: Some(120.0),
            market_cap: Some(1_000_000_000.0),
            eps_trailing_twelve_months: Some(5.0),
            eps_forward: Some(5.5),
            trailing_annual_dividend_yield: Some(0.02),
            fifty_day_average: Some(95.0),
            two_hundred_day_average: Some(90.0),
            beta: Some(1.2),
            sector: Some("Technology".into()),
            industry: Some("Software".into()),
            ..Default::default()
        };
        let sd = StockData::from_quote(&q);
        assert_eq!(sd.symbol, "TEST.NS");
        assert_eq!(sd.name, "Test Corp");
        assert_eq!(sd.price, 100.0);
        assert_eq!(sd.change_pct, 1.5);
        assert_eq!(sd.pe, Some(20.0));
        assert_eq!(sd.forward_pe, Some(18.0));
        assert_eq!(sd.sector, "Technology");
        assert_eq!(sd.industry, "Software");
        assert!(sd.rsi.is_none()); // RSI set separately
    }

    #[test]
    fn test_stock_data_from_quote_fallback_name() {
        let q = Quote {
            symbol: Some("TCS.NS".into()),
            short_name: None,
            long_name: Some("Tata Consultancy Services".into()),
            regular_market_price: Some(3500.0),
            ..Default::default()
        };
        let sd = StockData::from_quote(&q);
        assert_eq!(sd.name, "Tata Consultancy Services");
    }

    #[test]
    fn test_stock_data_from_quote_missing_fields() {
        let q = Quote {
            symbol: Some("UNKNOWN.NS".into()),
            ..Default::default()
        };
        let sd = StockData::from_quote(&q);
        assert_eq!(sd.price, 0.0);
        assert_eq!(sd.pe, None);
        assert_eq!(sd.sector, "");
    }
}
