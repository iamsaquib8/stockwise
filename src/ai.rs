use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const OLLAMA_URL: &str = "http://localhost:11434";
const DEFAULT_MODEL: &str = "gemma3:12b";

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: String,
}

pub struct AiClient {
    client: reqwest::Client,
    model: String,
}

impl AiClient {
    pub fn new() -> Self {
        let model = std::env::var("STOCKWISE_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap(),
            model,
        }
    }

    pub async fn is_available(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", OLLAMA_URL))
            .send()
            .await
            .is_ok()
    }

    pub async fn generate(&self, prompt: &str) -> Result<String> {
        let req = OllamaRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            stream: false,
        };

        let resp: OllamaResponse = self
            .client
            .post(format!("{}/api/generate", OLLAMA_URL))
            .json(&req)
            .send()
            .await
            .context("Failed to connect to Ollama. Is it running? (ollama serve)")?
            .json()
            .await
            .context("Failed to parse Ollama response")?;

        Ok(resp.response.trim().to_string())
    }

    pub async fn analyze_stock(&self, data: &StockData) -> Result<String> {
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

    pub async fn generate_intraday_report(&self, report_data: &str) -> Result<String> {
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

    pub async fn generate_longterm_report(&self, report_data: &str) -> Result<String> {
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
    pub async fn quick_insight(&self, context: &str) -> Result<String> {
        let prompt = format!(
            "You are a stock analyst. Given this data, give a 2-3 sentence actionable insight. Be specific with numbers. No disclaimers.\n\n{}",
            context
        );
        self.generate(&prompt).await
    }

    /// Technical analysis interpretation
    pub async fn interpret_technicals(&self, data: &str) -> Result<String> {
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
    pub async fn compare_stocks(&self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are an investment analyst. Compare these stocks and pick a winner.

{}

In 3-4 sentences: Which stock is the best investment right now and why? Be specific about valuation, growth, and risk. Give a clear verdict."#, data);
        self.generate(&prompt).await
    }

    /// Backtest interpretation
    pub async fn interpret_backtest(&self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a quantitative analyst. Interpret these backtest results.

{}

In 3-4 sentences: Is this strategy viable? What market conditions would it work best in? Key risks? Would you deploy real capital on it?"#, data);
        self.generate(&prompt).await
    }

    /// Screen results analysis
    pub async fn analyze_screen(&self, data: &str) -> Result<String> {
        let prompt = format!(
r#"You are a stock screener analyst. Analyze these screened stocks.

{}

In 3-4 sentences: Which 2-3 stocks stand out most? Any value traps to avoid? What makes the top picks compelling?"#, data);
        self.generate(&prompt).await
    }

    pub async fn generate_portfolio_report(&self, report_data: &str) -> Result<String> {
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
