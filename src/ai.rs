use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};

const DEFAULT_OLLAMA_URL: &str = "http://localhost:11434";

/// Ollama server URL. Honours the standard `OLLAMA_HOST` variable, then
/// `ollama_host` in the config file (`host:port` or a full URL), defaulting
/// to the local server.
pub(crate) fn ollama_url() -> String {
    match crate::config::setting("OLLAMA_HOST", |c| c.ollama_host.as_ref()) {
        Some(h) if !h.trim().is_empty() => {
            let h = h.trim().trim_end_matches('/');
            if h.starts_with("http://") || h.starts_with("https://") {
                h.to_string()
            } else {
                format!("http://{}", h)
            }
        }
        _ => DEFAULT_OLLAMA_URL.to_string(),
    }
}
const DEFAULT_MODEL: &str = "qwen3:14b";

/// AI model: `STOCKWISE_MODEL`, then `model` in the config file, then the
/// configured provider's default.
pub(crate) fn model_name() -> String {
    crate::config::setting("STOCKWISE_MODEL", |c| c.model.as_ref())
        .filter(|m| !m.trim().is_empty())
        .unwrap_or_else(|| {
            configured_provider()
                .map_or(DEFAULT_MODEL, |p| p.default_model())
                .to_string()
        })
}
const AI_CACHE_PREFIX: &str = "stockwise:ai:";
const AI_CACHE_TTL_SECS: i64 = 3600; // 1 hour for AI responses
/// Upper bound on a cloud model's answer. The prompts ask for a few
/// sentences; this only stops a runaway response.
const CLOUD_MAX_TOKENS: u32 = 16_000;

/// Where AI requests go. Ollama (local) is the default; the cloud providers
/// are used only when `STOCKWISE_AI_PROVIDER` names one, and then only for
/// `report` or with `--ai` — the same opt-in as local AI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiProvider {
    Ollama,
    OpenAi,
    Anthropic,
    Gemini,
}

impl AiProvider {
    /// Parse `STOCKWISE_AI_PROVIDER` (case-insensitive; empty means Ollama).
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "" | "ollama" | "local" => Some(AiProvider::Ollama),
            "openai" | "chatgpt" => Some(AiProvider::OpenAi),
            "anthropic" | "claude" => Some(AiProvider::Anthropic),
            "gemini" | "google" => Some(AiProvider::Gemini),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            AiProvider::Ollama => "Ollama",
            AiProvider::OpenAi => "OpenAI",
            AiProvider::Anthropic => "Anthropic",
            AiProvider::Gemini => "Google Gemini",
        }
    }

    /// Environment variable holding the API key (none for Ollama).
    pub fn key_env(self) -> Option<&'static str> {
        match self {
            AiProvider::Ollama => None,
            AiProvider::OpenAi => Some("OPENAI_API_KEY"),
            AiProvider::Anthropic => Some("ANTHROPIC_API_KEY"),
            AiProvider::Gemini => Some("GEMINI_API_KEY"),
        }
    }

    /// Model used when `STOCKWISE_MODEL` is not set.
    pub fn default_model(self) -> &'static str {
        match self {
            AiProvider::Ollama => DEFAULT_MODEL,
            AiProvider::OpenAi => OPENAI_DEFAULT_MODEL,
            AiProvider::Anthropic => ANTHROPIC_DEFAULT_MODEL,
            AiProvider::Gemini => GEMINI_DEFAULT_MODEL,
        }
    }

    fn default_base_url(self) -> String {
        match self {
            AiProvider::Ollama => ollama_url(),
            AiProvider::OpenAi => "https://api.openai.com".into(),
            AiProvider::Anthropic => "https://api.anthropic.com".into(),
            AiProvider::Gemini => "https://generativelanguage.googleapis.com".into(),
        }
    }

    pub fn is_cloud(self) -> bool {
        self != AiProvider::Ollama
    }
}

const OPENAI_DEFAULT_MODEL: &str = "gpt-6.1-sol";
const ANTHROPIC_DEFAULT_MODEL: &str = "claude-opus-5-5";
const GEMINI_DEFAULT_MODEL: &str = "gemini-3.8-flash";
const ANTHROPIC_VERSION: &str = "2023-06-01";
/// Claude models that take server-side refusal fallbacks (`fallbacks:
/// "default"`); other models reject or ignore the parameter.
const ANTHROPIC_FALLBACK_MODELS: &[&str] = &[
    "claude-fable-5-1",
    "claude-fable-5",
    "claude-opus-5-5",
    "claude-opus-5",
    "claude-sonnet-5-5",
];
const ANTHROPIC_FALLBACK_BETA: &str = "server-side-fallback-2026-07-01";

/// The configured provider: `STOCKWISE_AI_PROVIDER`, defaulting to Ollama.
/// An unknown value is an error rather than a silent fallback, so a typo
/// never sends data somewhere unexpected — or nowhere when the user expects
/// a cloud model.
pub fn configured_provider() -> Result<AiProvider> {
    let raw = crate::config::setting("STOCKWISE_AI_PROVIDER", |c| c.ai_provider.as_ref())
        .unwrap_or_default();
    AiProvider::parse(&raw).with_context(|| {
        format!(
            "Unknown STOCKWISE_AI_PROVIDER '{}' — use ollama, openai, anthropic or gemini",
            raw.trim()
        )
    })
}

/// Print, once per run, that a prompt is about to leave the machine.
fn notify_cloud_send(provider: AiProvider, model: &str) {
    static SHOWN: AtomicBool = AtomicBool::new(false);
    if !SHOWN.swap(true, Ordering::Relaxed) {
        eprintln!(
            "  \u{24d8} Sending this request's market data to {} ({}). Unset STOCKWISE_AI_PROVIDER to keep AI local.",
            provider.name(),
            model
        );
    }
}

#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    prompt: String,
    stream: bool,
}

#[derive(Deserialize)]
struct OllamaResponse {
    response: Option<String>,
    error: Option<String>,
}

/// Whether analysis commands (analyze, technical, compare, screen, backtest,
/// deep) append an AI section. Off by default: a local model turns a
/// sub-second command into a 40–60 second one. Enabled by `--ai` or
/// `STOCKWISE_AI=1`. `report` always uses AI.
static AUTO_AI: AtomicBool = AtomicBool::new(false);

pub fn set_auto_ai(enabled: bool) {
    AUTO_AI.store(enabled, Ordering::Relaxed);
}

pub fn auto_ai_enabled() -> bool {
    AUTO_AI.load(Ordering::Relaxed)
}

fn hash_prompt(model: &str, prompt: &str) -> String {
    let mut hasher = DefaultHasher::new();
    PROMPT_VERSION.hash(&mut hasher);
    model.hash(&mut hasher);
    prompt.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

pub struct AiClient {
    client: reqwest::Client,
    provider: AiProvider,
    model: String,
    api_key: Option<String>,
    base_url: String,
    /// The user asked for this AI output directly (`report`), so a cloud
    /// provider may be used without `--ai`.
    explicit: bool,
    /// An invalid `STOCKWISE_AI_PROVIDER`: AI stays off and this says why.
    config_error: Option<String>,
    redis: Option<redis::aio::MultiplexedConnection>,
}

impl AiClient {
    /// Client for optional AI sections. With a cloud provider it only
    /// generates when `--ai` is on (see `is_available`).
    pub async fn connect() -> Self {
        let mut ai = Self::new();
        ai.redis = crate::api::connect_redis().await;
        ai
    }

    /// Client for a command whose whole purpose is AI output (`report`):
    /// running it is the opt-in, so a configured cloud provider is used even
    /// without `--ai`.
    pub async fn connect_explicit() -> Self {
        let mut ai = Self::connect().await;
        ai.explicit = true;
        ai
    }

    // Sync constructor for backward compat (no Redis)
    pub fn new() -> Self {
        match configured_provider() {
            Ok(provider) => Self::with_provider(provider, None, None),
            // Never guess where to send data: AI stays off until it's fixed.
            Err(e) => {
                let mut ai = Self::with_provider(AiProvider::Ollama, None, None);
                ai.config_error = Some(format!("{:#}", e));
                ai
            }
        }
    }

    /// Build a client for `provider`. `base_url` overrides the API endpoint
    /// (tests point it at a local mock server).
    pub fn with_provider(
        provider: AiProvider,
        api_key: Option<String>,
        base_url: Option<String>,
    ) -> Self {
        let model = crate::config::setting("STOCKWISE_MODEL", |c| c.model.as_ref())
            .filter(|m| !m.trim().is_empty())
            .unwrap_or_else(|| provider.default_model().to_string());
        let api_key = api_key.or_else(|| {
            provider
                .key_env()
                .and_then(|k| std::env::var(k).ok())
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        });
        Self {
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(120))
                .build()
                .unwrap(),
            provider,
            model,
            api_key,
            base_url: base_url.unwrap_or_else(|| provider.default_base_url()),
            explicit: false,
            config_error: None,
            redis: None,
        }
    }

    /// True when auto-AI is enabled (`--ai`) and the provider can answer.
    pub async fn auto_available(&self) -> bool {
        auto_ai_enabled() && self.is_available().await
    }

    pub fn model(&self) -> &str {
        &self.model
    }

    pub fn provider(&self) -> AiProvider {
        self.provider
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    /// The `STOCKWISE_AI_PROVIDER` problem that keeps AI off, if any.
    pub fn config_error(&self) -> Option<&str> {
        self.config_error.as_deref()
    }

    /// Whether the configured model is pulled on the Ollama server.
    /// `None` when the server can't be reached.
    pub async fn has_model(&self) -> Option<bool> {
        let tags: serde_json::Value = self
            .client
            .get(format!("{}/api/tags", ollama_url()))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .ok()?
            .json()
            .await
            .ok()?;
        let wanted = &self.model;
        Some(
            tags["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["name"].as_str())
                .any(|name| name == wanted || name == format!("{wanted}:latest")),
        )
    }

    /// Whether AI output can be generated now. Ollama: the server answers.
    /// Cloud: an API key is set and the user opted in — `--ai` or a command
    /// that is explicitly about AI (`report`). Never probes a cloud API.
    pub async fn is_available(&self) -> bool {
        if self.config_error.is_some() {
            return false;
        }
        if self.provider.is_cloud() {
            return self.api_key.is_some() && (self.explicit || auto_ai_enabled());
        }
        self.client
            .get(format!("{}/api/tags", ollama_url()))
            // Probe quickly so an unreachable OLLAMA_HOST never stalls a command.
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .is_ok()
    }

    /// Why `report` can't run, for its error message.
    pub fn unavailable_reason(&self) -> String {
        if let Some(e) = &self.config_error {
            return e.clone();
        }
        match self.provider.key_env() {
            Some(key) if self.api_key.is_none() => format!(
                "{} is selected (STOCKWISE_AI_PROVIDER) but {} is not set; set it, or unset STOCKWISE_AI_PROVIDER to use local Ollama",
                self.provider.name(),
                key
            ),
            Some(_) => format!(
                "{} is configured but not enabled for this command — add --ai",
                self.provider.name()
            ),
            None => format!(
                "Reports need a local AI model, but Ollama is not reachable. Install it from \
                 https://ollama.com, then run `ollama serve` and `ollama pull {}` \
                 (set STOCKWISE_MODEL to use another model, OLLAMA_HOST for a remote server, \
                 or STOCKWISE_AI_PROVIDER for a cloud provider).",
                self.model
            ),
        }
    }

    async fn cache_get(&mut self, key: &str) -> Option<String> {
        if let Some(ref mut conn) = self.redis {
            redis::cmd("GET")
                .arg(key)
                .query_async::<Option<String>>(conn)
                .await
                .ok()
                .flatten()
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
        // Cloud answers are cached under a provider-qualified key; Ollama
        // keeps its original keys.
        let cache_model = if self.provider.is_cloud() {
            format!("{}:{}", self.provider.name(), self.model)
        } else {
            self.model.clone()
        };
        let cache_key = format!("{}{}", AI_CACHE_PREFIX, hash_prompt(&cache_model, prompt));
        if let Some(cached) = self.cache_get(&cache_key).await {
            return Ok(cached);
        }

        let response_text = match self.provider {
            AiProvider::Ollama => self.generate_ollama(prompt).await?,
            cloud => {
                let key = self.api_key.clone().with_context(|| {
                    format!(
                        "STOCKWISE_AI_PROVIDER={} needs {}",
                        cloud.name(),
                        cloud.key_env().unwrap_or_default()
                    )
                })?;
                notify_cloud_send(cloud, &self.model);
                match cloud {
                    AiProvider::OpenAi => self.generate_openai(&key, prompt).await?,
                    AiProvider::Anthropic => self.generate_anthropic(&key, prompt).await?,
                    AiProvider::Gemini => self.generate_gemini(&key, prompt).await?,
                    AiProvider::Ollama => unreachable!("handled above"),
                }
            }
        };

        // Strip qwen3 <think>...</think> tags
        let cleaned = strip_think_tags(&response_text);
        let result = cleaned.trim().to_string();

        if result.is_empty() {
            bail!(
                "{} returned an empty response (model '{}')",
                self.provider.name(),
                self.model
            );
        }
        // Cache to Redis (1 hour TTL)
        self.cache_set(&cache_key, &result).await;

        Ok(result)
    }

    async fn generate_ollama(&self, prompt: &str) -> Result<String> {
        {
            use std::io::IsTerminal;
            if std::io::stderr().is_terminal() {
                eprintln!(
                    "  \u{27f3} Asking {} via Ollama (local models can take a minute)...",
                    self.model
                );
            }
        }

        let req = OllamaRequest {
            model: self.model.clone(),
            prompt: prompt.to_string(),
            stream: false,
        };

        let raw: String = self
            .client
            .post(format!("{}/api/generate", self.base_url))
            .json(&req)
            .send()
            .await
            .context("Failed to connect to Ollama. Is it running? (ollama serve)")?
            .text()
            .await
            .context("Failed to read Ollama response")?;

        Ok(
            if let Ok(resp) = serde_json::from_str::<OllamaResponse>(&raw) {
                if let Some(err) = resp.error {
                    if err.contains("not found") {
                        bail!(
                            "Ollama model '{}' is not installed. Run: ollama pull {}  (or set STOCKWISE_MODEL)",
                            self.model,
                            self.model
                        );
                    }
                    bail!("Ollama error: {}", err);
                }
                resp.response.unwrap_or_default()
            } else {
                let mut combined = String::new();
                for line in raw.lines() {
                    if let Ok(obj) = serde_json::from_str::<OllamaResponse>(line)
                        && let Some(r) = obj.response
                    {
                        combined.push_str(&r);
                    }
                }
                combined
            },
        )
    }

    /// POST a JSON body to a cloud API and return the parsed reply, turning
    /// an HTTP error into the provider's own error message. The key travels
    /// only in `headers`, never in the URL, so it can't leak into errors.
    async fn post_json(
        &self,
        url: &str,
        headers: &[(&str, &str)],
        body: &serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut req = self.client.post(url).json(body);
        for (name, value) in headers {
            req = req.header(*name, *value);
        }
        let resp = req.send().await.map_err(|e| {
            anyhow::anyhow!(
                "Could not reach {}: {}",
                self.provider.name(),
                e.without_url()
            )
        })?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .with_context(|| format!("Failed to read the {} response", self.provider.name()))?;
        let json: serde_json::Value =
            serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
        if !status.is_success() {
            bail!(
                "{} returned HTTP {}: {}",
                self.provider.name(),
                status.as_u16(),
                cloud_error_message(&json).unwrap_or("no error details")
            );
        }
        if json.is_null() {
            bail!(
                "{} returned a response that is not JSON",
                self.provider.name()
            );
        }
        Ok(json)
    }

    async fn generate_openai(&self, key: &str, prompt: &str) -> Result<String> {
        let body = serde_json::json!({
            "model": self.model,
            "input": prompt,
            // Don't keep the prompt (market data, holdings) on OpenAI's side.
            "store": false,
        });
        let auth = format!("Bearer {}", key);
        let json = self
            .post_json(
                &format!("{}/v1/responses", self.base_url),
                &[("authorization", &auth)],
                &body,
            )
            .await?;
        parse_openai_response(&json)
    }

    async fn generate_anthropic(&self, key: &str, prompt: &str) -> Result<String> {
        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": CLOUD_MAX_TOKENS,
            "messages": [{"role": "user", "content": prompt}],
        });
        let mut headers = vec![("x-api-key", key), ("anthropic-version", ANTHROPIC_VERSION)];
        // Retry a safety-classifier decline on Anthropic's recommended
        // fallback model instead of failing the command.
        if ANTHROPIC_FALLBACK_MODELS.contains(&self.model.as_str()) {
            body["fallbacks"] = serde_json::json!("default");
            headers.push(("anthropic-beta", ANTHROPIC_FALLBACK_BETA));
        }
        let json = self
            .post_json(&format!("{}/v1/messages", self.base_url), &headers, &body)
            .await?;
        parse_anthropic_response(&json)
    }

    async fn generate_gemini(&self, key: &str, prompt: &str) -> Result<String> {
        let body = serde_json::json!({
            "contents": [{"role": "user", "parts": [{"text": prompt}]}],
        });
        let url = format!(
            "{}/v1beta/models/{}:generateContent",
            self.base_url, self.model
        );
        let json = self
            .post_json(&url, &[("x-goog-api-key", key)], &body)
            .await?;
        parse_gemini_response(&json)
    }

    pub async fn analyze_stock(&mut self, data: &StockData) -> Result<String> {
        self.generate(&prompts::analyze_stock(data)).await
    }

    pub async fn generate_intraday_report(&mut self, report_data: &str) -> Result<String> {
        self.generate(&prompts::intraday_report(report_data)).await
    }

    pub async fn generate_longterm_report(&mut self, report_data: &str) -> Result<String> {
        self.generate(&prompts::longterm_report(report_data)).await
    }

    /// Quick 2-3 sentence observation for any data context (fast, for inline use)
    pub async fn quick_insight(&mut self, context: &str) -> Result<String> {
        self.generate(&prompts::quick_insight(context)).await
    }

    /// Technical analysis interpretation
    pub async fn interpret_technicals(&mut self, data: &str) -> Result<String> {
        self.generate(&prompts::technicals(data)).await
    }

    /// Compare two or more stocks
    pub async fn compare_stocks(&mut self, data: &str) -> Result<String> {
        self.generate(&prompts::compare(data)).await
    }

    /// Backtest interpretation
    pub async fn interpret_backtest(&mut self, data: &str) -> Result<String> {
        self.generate(&prompts::backtest(data)).await
    }

    /// Screen results analysis
    pub async fn analyze_screen(&mut self, data: &str) -> Result<String> {
        self.generate(&prompts::screen(data)).await
    }

    pub async fn generate_portfolio_report(&mut self, report_data: &str) -> Result<String> {
        self.generate(&prompts::portfolio_report(report_data)).await
    }
}

/// Bumped whenever the prompts change, and part of every cache key, so an
/// answer cached under older wording (v1 asked for BUY/HOLD/SELL verdicts)
/// is never shown again.
pub const PROMPT_VERSION: u32 = 2;

/// Printed under every AI section.
pub const DISCLAIMER: &str = "AI-generated educational analysis, not investment advice or a \
     recommendation to buy, sell or hold. StockWise is not a SEBI-registered investment adviser \
     or research analyst.";

/// The prompts behind every AI section. Each starts with [`prompts::RULES`]:
/// the model explains signals, risks and what to watch, in neutral terms,
/// and never makes a personal buy/sell/hold call — in line with SEBI's
/// rules for investment advice and research.
pub mod prompts {
    use super::StockData;

    /// Common rules every prompt starts with.
    pub const RULES: &str = "You write short, educational market commentary for StockWise, an \
open-source tool for individual investors. Rules:
- Explain what the data shows: signals, trends, valuation context, risks, and what to watch next.
- Use neutral, conditional language (\"the indicators point to...\", \"a risk is...\", \"if X, then Y\").
- Never tell the reader to buy, sell, hold, accumulate, trim, enter, exit or avoid anything. Give \
no price targets, entry or exit levels, stop-losses, position sizes or allocations for the reader, \
and no ratings such as BUY, SELL or HOLD.
- Do not pick a single best investment or a winner.
- Stick to the data given and say when it is missing or too thin to judge. No hype, no guarantees.
- Do not add a disclaimer; StockWise prints one.

";

    fn with_rules(task: String) -> String {
        format!("{RULES}{task}")
    }

    fn num(v: Option<f64>, decimals: usize) -> String {
        v.map_or("N/A".to_string(), |v| format!("{v:.decimals$}"))
    }

    pub fn analyze_stock(data: &StockData) -> String {
        with_rules(format!(
            r#"Explain this stock's numbers for a reader learning to analyse companies.

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

In 3-4 sentences cover:
1. Valuation in context (how the multiples compare with the stock's growth and typical ranges)
2. What the trend and momentum signals show
3. The main risks visible in the data
4. What to watch next, short term and long term"#,
            data.symbol,
            data.name,
            data.price,
            data.change_pct,
            num(data.pe, 1),
            num(data.forward_pe, 1),
            num(data.pb, 1),
            data.week52_low,
            data.week52_high,
            data.market_cap,
            num(data.eps_ttm, 2),
            num(data.eps_fwd, 2),
            num(data.div_yield.map(|v| v * 100.0), 2),
            data.ma50,
            data.ma200,
            num(data.rsi, 0),
            num(data.beta, 2),
            data.sector,
            data.industry,
        ))
    }

    pub fn intraday_report(report_data: &str) -> String {
        with_rules(format!(
            r#"These are today's intraday scan results. Write market notes for the session.

{report_data}

In 5-7 sentences cover:
1. The market regime and overall tone in the data
2. The 2-3 names with the strongest signals, which signals they are, and what would weaken them
3. Sectors showing unusual strength or weakness
4. The main risks to watch today
5. How reliable these signals look (sample size, agreement between strategies)

Mention price levels only as levels the data shows (recent highs, lows, averages)."#
        ))
    }

    pub fn longterm_report(report_data: &str) -> String {
        with_rules(format!(
            r#"These are long-term scores for a set of stocks. Write research notes on them.

{report_data}

Cover, concisely:
1. Which names score highest and which factors drive their scores
2. Macro and sector risks that could affect the group
3. How concentrated or diversified the high scorers are (sectors, styles)
4. What could change the picture over the next year
5. Red flags or weak spots in the data"#
        ))
    }

    pub fn quick_insight(context: &str) -> String {
        with_rules(format!(
            "In 2-3 sentences, give an educational observation about this data, with specific numbers.\n\n{context}"
        ))
    }

    pub fn technicals(data: &str) -> String {
        with_rules(format!(
            r#"Interpret these technical indicators together.

{data}

In 3-4 sentences:
1. Do the indicators agree or conflict, and what does that usually mean?
2. Which scenarios are possible next (if/then), and which signals would confirm each?
3. Which levels in the data are worth watching?"#
        ))
    }

    pub fn compare(data: &str) -> String {
        with_rules(format!(
            r#"Compare these stocks.

{data}

In 3-4 sentences: how do they differ on valuation, growth and risk, which looks stronger on each measure, and what trade-offs does a reader face between them? Do not name an overall pick."#
        ))
    }

    pub fn backtest(data: &str) -> String {
        with_rules(format!(
            r#"Interpret these backtest results.

{data}

In 3-4 sentences: how robust do the results look (number of trades, drawdown, versus the stock's own return over the period)? In which market conditions would the strategy tend to work or fail? What are the key risks, and what would be worth testing next? Remember that past results do not predict future ones."#
        ))
    }

    pub fn screen(data: &str) -> String {
        with_rules(format!(
            r#"These stocks passed a screen. Explain the results.

{data}

In 3-4 sentences: which names stand out on the screened measures and why, which could be value traps or data artefacts, and what a reader should check before relying on the screen?"#
        ))
    }

    pub fn portfolio_report(report_data: &str) -> String {
        with_rules(format!(
            r#"Review this portfolio's data as educational notes for its owner.

{report_data}

Cover, concisely:
1. Overall picture: gains and losses, and how they are spread
2. Concentration: the largest positions and how much of the value they are
3. Holdings whose data stands out (large moves, high valuations, missing prices) and why
4. Exposures the portfolio has little or none of (sectors, asset types), described, not prescribed
5. Risks worth understanding and what to watch"#
        ))
    }

    /// Every prompt, built from sample data — for tests.
    #[cfg(test)]
    pub fn all_for_test() -> Vec<(&'static str, String)> {
        let data = "TCS.NS: price=4100.00, P/E=30.1, chg=+1.50%";
        vec![
            ("analyze_stock", analyze_stock(&StockData::sample())),
            ("intraday_report", intraday_report(data)),
            ("longterm_report", longterm_report(data)),
            ("quick_insight", quick_insight(data)),
            ("technicals", technicals(data)),
            ("compare", compare(data)),
            ("backtest", backtest(data)),
            ("screen", screen(data)),
            ("portfolio_report", portfolio_report(data)),
        ]
    }
}

/// The human-readable message in a cloud API error body. OpenAI and Gemini
/// use `{"error": {"message": ...}}`; Anthropic nests it the same way under
/// `{"type": "error", "error": {...}}`.
fn cloud_error_message(json: &serde_json::Value) -> Option<&str> {
    json["error"]["message"]
        .as_str()
        .or_else(|| json["error"].as_str())
        .or_else(|| json["message"].as_str())
}

/// Text of an OpenAI Responses API reply: the `output_text` parts of the
/// `message` items in `output` (reasoning items carry no text).
fn parse_openai_response(json: &serde_json::Value) -> Result<String> {
    if json["status"].as_str() == Some("incomplete") {
        let reason = json["incomplete_details"]["reason"]
            .as_str()
            .unwrap_or("unknown");
        bail!("OpenAI stopped before finishing the answer ({})", reason);
    }
    let mut text = String::new();
    let mut refusal = None;
    for item in json["output"].as_array().into_iter().flatten() {
        if item["type"] != "message" {
            continue;
        }
        for part in item["content"].as_array().into_iter().flatten() {
            match part["type"].as_str() {
                Some("output_text") => text.push_str(part["text"].as_str().unwrap_or("")),
                Some("refusal") => refusal = part["refusal"].as_str(),
                _ => {}
            }
        }
    }
    if text.is_empty()
        && let Some(r) = refusal
    {
        bail!("OpenAI declined the request: {}", r);
    }
    Ok(text)
}

/// Text of an Anthropic Messages reply: the `text` blocks of `content`
/// (thinking blocks are skipped). A `refusal` stop reason is an error, even
/// if partial text came back.
fn parse_anthropic_response(json: &serde_json::Value) -> Result<String> {
    if json["stop_reason"].as_str() == Some("refusal") {
        let category = json["stop_details"]["category"]
            .as_str()
            .unwrap_or("unspecified");
        bail!("Anthropic declined the request (category: {})", category);
    }
    Ok(json["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|b| b["type"] == "text")
        .filter_map(|b| b["text"].as_str())
        .collect::<Vec<_>>()
        .join(""))
}

/// Text of a Gemini `generateContent` reply: the text parts of the first
/// candidate, skipping thought summaries. A blocked prompt or a safety stop
/// is an error.
fn parse_gemini_response(json: &serde_json::Value) -> Result<String> {
    if let Some(reason) = json["promptFeedback"]["blockReason"].as_str() {
        bail!("Gemini blocked the prompt ({})", reason);
    }
    let candidate = &json["candidates"][0];
    let text: String = candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| p["thought"].as_bool() != Some(true))
        .filter_map(|p| p["text"].as_str())
        .collect();
    if text.is_empty()
        && let Some(reason) = candidate["finishReason"].as_str()
        && reason != "STOP"
    {
        bail!("Gemini returned no text (finish reason {})", reason);
    }
    Ok(text)
}

/// Strip <think>...</think> tags from qwen3-style thinking models
fn strip_think_tags(text: &str) -> String {
    let mut result = text.to_string();
    // Remove <think>...</think> blocks (can be multiline). Search for the
    // closing tag *after* the opening one, so a stray "</think>" earlier in the
    // text can't corrupt the slice.
    while let Some(start) = result.find("<think>") {
        if let Some(rel_end) = result[start..].find("</think>") {
            let end = start + rel_end + "</think>".len();
            result = format!("{}{}", &result[..start], &result[end..]);
        } else {
            // Unclosed think tag — remove from <think> to end
            result.truncate(start);
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
            name: q
                .short_name
                .clone()
                .or(q.long_name.clone())
                .unwrap_or_default(),
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

    #[cfg(test)]
    pub fn sample() -> Self {
        Self::from_quote(&crate::api::Quote {
            symbol: Some("TCS.NS".into()),
            short_name: Some("Tata Consultancy Services".into()),
            regular_market_price: Some(4100.0),
            regular_market_change_percent: Some(1.5),
            trailing_pe: Some(30.1),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Quote;

    /// Phrases that make a prompt ask for a personal call. The shared rules
    /// name some of them to forbid them, so only each prompt's own part is
    /// checked.
    const CALLS: &[&str] = &[
        "verdict",
        "buy",
        "sell",
        "hold",
        "accumulate",
        "actionable",
        "recommendation",
        "trim",
        "winner",
        "pick a",
        "entry/exit",
        "deploy real capital",
        "what to avoid",
        "allocation strategy",
        "no disclaimers",
    ];

    #[test]
    fn prompts_ask_for_education_not_calls() {
        for (name, prompt) in prompts::all_for_test() {
            let task = prompt
                .strip_prefix(prompts::RULES)
                .unwrap_or_else(|| panic!("{name} does not start with the shared rules"))
                .to_lowercase();
            let words: Vec<&str> = task.split(|c: char| !c.is_ascii_alphanumeric()).collect();
            for call in CALLS {
                let asks = if call.contains([' ', '/']) {
                    task.contains(call)
                } else {
                    words.contains(call)
                };
                assert!(!asks, "{name} prompt asks for '{call}':\n{task}");
            }
            assert!(task.contains("tcs.ns"), "{name} prompt lost its data");
        }
        let rules = prompts::RULES.to_lowercase();
        for must in [
            "never tell the reader to buy, sell, hold",
            "no ratings such as buy, sell or hold",
            "do not pick a single best investment",
            "what to watch",
            "risks",
        ] {
            assert!(rules.contains(must), "rules miss '{must}'");
        }
    }

    #[test]
    fn disclaimer_says_it_is_education_not_advice() {
        let d = DISCLAIMER.to_lowercase();
        assert!(d.contains("educational") && d.contains("not investment advice"));
        assert!(d.contains("sebi"));
        assert!(!DISCLAIMER.contains('\n'), "one line under each AI section");
    }

    #[test]
    fn cache_keys_change_with_the_prompt_version() {
        // v1 answers (BUY/HOLD/SELL verdicts) are cached under keys that
        // did not include a version; a versioned key never matches them.
        let mut v1 = DefaultHasher::new();
        "qwen3:14b".hash(&mut v1);
        "p".hash(&mut v1);
        assert_ne!(hash_prompt("qwen3:14b", "p"), format!("{:x}", v1.finish()));
        const { assert!(PROMPT_VERSION >= 2) };
    }

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

    #[test]
    fn provider_parse_accepts_names_and_aliases() {
        assert_eq!(AiProvider::parse(""), Some(AiProvider::Ollama));
        assert_eq!(AiProvider::parse("Ollama"), Some(AiProvider::Ollama));
        assert_eq!(AiProvider::parse("OPENAI"), Some(AiProvider::OpenAi));
        assert_eq!(AiProvider::parse("claude"), Some(AiProvider::Anthropic));
        assert_eq!(AiProvider::parse(" gemini "), Some(AiProvider::Gemini));
        assert_eq!(AiProvider::parse("gpt"), None);
        assert!(!AiProvider::Ollama.is_cloud());
        assert_eq!(AiProvider::Gemini.key_env(), Some("GEMINI_API_KEY"));
    }

    #[test]
    fn openai_response_text_skips_reasoning_items() {
        let json = serde_json::json!({
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": []},
                {"type": "message", "role": "assistant", "content": [
                    {"type": "output_text", "text": "RELIANCE looks ", "annotations": []},
                    {"type": "output_text", "text": "fairly valued.", "annotations": []}
                ]}
            ]
        });
        assert_eq!(
            parse_openai_response(&json).unwrap(),
            "RELIANCE looks fairly valued."
        );
        let refused = serde_json::json!({"status": "completed", "output": [
            {"type": "message", "content": [{"type": "refusal", "refusal": "Can't help"}]}
        ]});
        assert!(
            parse_openai_response(&refused)
                .unwrap_err()
                .to_string()
                .contains("Can't help")
        );
        let cut = serde_json::json!({"status": "incomplete", "incomplete_details": {"reason": "max_output_tokens"}});
        assert!(
            parse_openai_response(&cut)
                .unwrap_err()
                .to_string()
                .contains("max_output_tokens")
        );
    }

    #[test]
    fn anthropic_response_text_skips_thinking_and_rejects_refusals() {
        let json = serde_json::json!({
            "type": "message",
            "stop_reason": "end_turn",
            "content": [
                {"type": "thinking", "thinking": "", "signature": "abc"},
                {"type": "text", "text": "HOLD. "},
                {"type": "text", "text": "Valuation is stretched."}
            ]
        });
        assert_eq!(
            parse_anthropic_response(&json).unwrap(),
            "HOLD. Valuation is stretched."
        );
        let refusal = serde_json::json!({
            "stop_reason": "refusal",
            "stop_details": {"type": "refusal", "category": "cyber"},
            "content": []
        });
        let err = parse_anthropic_response(&refusal).unwrap_err().to_string();
        assert!(err.contains("declined") && err.contains("cyber"), "{err}");
    }

    #[test]
    fn gemini_response_text_skips_thoughts_and_reports_blocks() {
        let json = serde_json::json!({
            "candidates": [{
                "content": {"role": "model", "parts": [
                    {"text": "thinking...", "thought": true},
                    {"text": "Momentum is strong."}
                ]},
                "finishReason": "STOP"
            }]
        });
        assert_eq!(parse_gemini_response(&json).unwrap(), "Momentum is strong.");
        let blocked = serde_json::json!({"promptFeedback": {"blockReason": "SAFETY"}});
        assert!(
            parse_gemini_response(&blocked)
                .unwrap_err()
                .to_string()
                .contains("SAFETY")
        );
        let empty = serde_json::json!({"candidates": [{"finishReason": "MAX_TOKENS"}]});
        assert!(
            parse_gemini_response(&empty)
                .unwrap_err()
                .to_string()
                .contains("MAX_TOKENS")
        );
    }

    #[test]
    fn cloud_error_message_reads_each_provider_shape() {
        let openai = serde_json::json!({"error": {"message": "Incorrect API key", "type": "invalid_request_error"}});
        let anthropic = serde_json::json!({"type": "error", "error": {"type": "authentication_error", "message": "invalid x-api-key"}});
        let gemini = serde_json::json!({"error": {"code": 400, "message": "API key not valid", "status": "INVALID_ARGUMENT"}});
        assert_eq!(cloud_error_message(&openai), Some("Incorrect API key"));
        assert_eq!(cloud_error_message(&anthropic), Some("invalid x-api-key"));
        assert_eq!(cloud_error_message(&gemini), Some("API key not valid"));
        assert_eq!(cloud_error_message(&serde_json::Value::Null), None);
    }

    fn cloud_client(provider: AiProvider, model: &str, url: &str) -> AiClient {
        let mut ai =
            AiClient::with_provider(provider, Some("sk-test-123".into()), Some(url.into()));
        ai.model = model.into();
        ai.explicit = true;
        ai
    }

    #[tokio::test]
    async fn anthropic_request_shape_and_fallback_opt_in() {
        let server = crate::testutil::MockServer::start(vec![(
            200,
            r#"{"type":"message","stop_reason":"end_turn","content":[{"type":"text","text":"BUY"}]}"#.into(),
        )])
        .await;
        let mut ai = cloud_client(AiProvider::Anthropic, "claude-opus-5-5", &server.url);
        assert_eq!(ai.generate("prompt one").await.unwrap(), "BUY");
        let mut older = cloud_client(AiProvider::Anthropic, "claude-haiku-4-5", &server.url);
        older.generate("prompt two").await.unwrap();

        let reqs = server.requests();
        let req = &reqs[0];
        assert_eq!(req.method, "POST");
        assert_eq!(req.target, "/v1/messages");
        assert_eq!(req.header("x-api-key"), Some("sk-test-123"));
        assert_eq!(req.header("anthropic-version"), Some("2023-06-01"));
        assert_eq!(req.header("anthropic-beta"), Some(ANTHROPIC_FALLBACK_BETA));
        let body = req.json();
        assert_eq!(body["model"], "claude-opus-5-5");
        assert_eq!(body["fallbacks"], "default");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"], "prompt one");
        assert!(body["max_tokens"].as_u64().unwrap() > 0);
        // Models without server-side fallback get neither the field nor the beta.
        assert_eq!(reqs[1].header("anthropic-beta"), None);
        assert!(reqs[1].json().get("fallbacks").is_none());
    }

    #[tokio::test]
    async fn openai_request_uses_bearer_auth_and_responses_api() {
        let server = crate::testutil::MockServer::start(vec![(
            200,
            r#"{"status":"completed","output":[{"type":"message","content":[{"type":"output_text","text":"SELL"}]}]}"#.into(),
        )])
        .await;
        let mut ai = cloud_client(AiProvider::OpenAi, "gpt-test", &server.url);
        assert_eq!(ai.generate("p").await.unwrap(), "SELL");
        let req = &server.requests()[0];
        assert_eq!(req.target, "/v1/responses");
        assert_eq!(req.header("authorization"), Some("Bearer sk-test-123"));
        assert_eq!(req.json()["model"], "gpt-test");
        assert_eq!(req.json()["input"], "p");
        assert_eq!(req.json()["store"], false);
    }

    #[tokio::test]
    async fn gemini_request_keeps_the_key_out_of_the_url() {
        let server = crate::testutil::MockServer::start(vec![(
            200,
            r#"{"candidates":[{"content":{"parts":[{"text":"HOLD"}]},"finishReason":"STOP"}]}"#
                .into(),
        )])
        .await;
        let mut ai = cloud_client(AiProvider::Gemini, "gemini-test", &server.url);
        assert_eq!(ai.generate("p").await.unwrap(), "HOLD");
        let req = &server.requests()[0];
        assert_eq!(req.target, "/v1beta/models/gemini-test:generateContent");
        assert!(!req.target.contains("sk-test"));
        assert_eq!(req.header("x-goog-api-key"), Some("sk-test-123"));
        assert_eq!(req.json()["contents"][0]["parts"][0]["text"], "p");
    }

    #[tokio::test]
    async fn cloud_http_errors_show_the_provider_message_not_the_key() {
        let server = crate::testutil::MockServer::start(vec![(
            401,
            r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#
                .into(),
        )])
        .await;
        let mut ai = cloud_client(AiProvider::OpenAi, "gpt-test", &server.url);
        let err = format!("{:#}", ai.generate("p").await.unwrap_err());
        assert!(
            err.contains("HTTP 401") && err.contains("Incorrect API key"),
            "{err}"
        );
        assert!(!err.contains("sk-test-123"), "{err}");
    }

    #[tokio::test]
    async fn cloud_ai_needs_a_key_and_an_opt_in() {
        let mut ai = AiClient::with_provider(
            AiProvider::OpenAi,
            Some("k".into()),
            Some("http://127.0.0.1:9".into()),
        );
        // A key alone is not consent: optional AI sections need --ai.
        if !auto_ai_enabled() {
            assert!(!ai.is_available().await);
        }
        ai.explicit = true;
        assert!(ai.is_available().await);
        ai.api_key = None;
        assert!(!ai.is_available().await);
        assert!(ai.unavailable_reason().contains("OPENAI_API_KEY"));
    }
}
