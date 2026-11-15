//! `stockwise ask "question"`: the configured AI model answers with the
//! read-only tools of `crate::tools` (the same ones `stockwise mcp` serves).
//!
//! The loop: send the question and the tool definitions; run the tools the
//! model asks for; send the results back; repeat until the model answers.
//! The number of model requests, tool calls and tokens are capped, and the
//! last allowed request has tools turned off so the model must answer with
//! what it has. Each provider speaks its own tool-calling dialect:
//! Ollama `/api/chat`, the OpenAI Responses API, Anthropic Messages and
//! Gemini `generateContent`.
//!
//! A [`Chat`] can carry on over several questions (`ask -i`) and be saved
//! and resumed (`ask -c`, `--resume`): before each new question, older tool
//! results are trimmed so the history stays small.

use crate::ai::{AiClient, AiProvider, CLOUD_MAX_TOKENS};
use crate::tools::{NOTE, Tool, ToolContext};
use crate::views::DataSource;
use anyhow::{Result, anyhow, bail};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Context window asked of Ollama: tool definitions alone take a few
/// thousand tokens, more than older defaults leave for the conversation.
const OLLAMA_NUM_CTX: u32 = 16_384;
/// Longest Ollama response: room for a thinking model's reasoning and the
/// answer, while the conversation keeps most of the window.
const OLLAMA_NUM_PREDICT: u32 = 4_096;
/// Tool results for local models are kept smaller than for cloud models.
pub const OLLAMA_RESULT_BYTES: usize = 12_000;

/// Limits on one `ask`.
#[derive(Debug, Clone, Copy)]
pub struct AskOptions {
    /// Most model requests; the last one has tools turned off.
    pub max_steps: usize,
    /// Most tool calls in total.
    pub max_tool_calls: usize,
    /// Input plus output tokens across requests; past it, the next request
    /// must answer.
    pub token_budget: u64,
    /// Longest single response.
    pub max_output_tokens: u32,
    /// Print each tool call to stderr.
    pub verbose: bool,
}

impl Default for AskOptions {
    fn default() -> Self {
        Self {
            max_steps: 6,
            max_tool_calls: 12,
            token_budget: 100_000,
            max_output_tokens: CLOUD_MAX_TOKENS,
            verbose: false,
        }
    }
}

/// A prepared task for `ask --playbook`: a fixed request that names the
/// tools to use, so the same checkup runs the same way each time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum Playbook {
    /// Portfolio checkup: value, concentration against the profile, risk, dates ahead
    Checkup,
    /// Capital gains so far this year, loss harvesting, advance tax
    TaxSeason,
    /// Results and dividend dates ahead for holdings and the watchlist, and recent results
    Earnings,
}

impl Playbook {
    /// The request sent as the first question.
    pub fn prompt(self) -> &'static str {
        match self {
            Self::Checkup => {
                "Give me a checkup of my portfolio. Use portfolio_analyze first, then calendar \
for my holdings and alerts_list. Report: total value, profit or loss and XIRR; the split by \
market, currency, cap tier and sector; any holding or the small-cap share above my profile's \
limits; holdings with missing prices; results or dividend dates in the next 30 days; and alerts \
close to triggering. End with up to three questions I could look into next. Facts only."
            }
            Self::TaxSeason => {
                "Prepare my capital-gains picture for the current financial year. Use \
tax_estimate, harvest and tax_advance. Report realised gains and losses by short and long term, \
the section 112A exemption used and still left, unrealised losses that could offset gains (as \
facts), and the advance-tax dates and amounts due. Remind me these are estimates from the lots \
StockWise has recorded, to check against my broker's tax P&L."
            }
            Self::Earnings => {
                "Look at results season for my holdings and watchlist. Use calendar for dates in \
the next 30 days, and quarterly_results for those that reported in the last 30 days: revenue, \
profit and margin against the same quarter a year ago. Facts only, one line a stock."
            }
        }
    }
}

/// What `ask --json` prints.
#[derive(Debug, Serialize)]
pub struct Transcript {
    pub question: String,
    pub provider: &'static str,
    pub model: String,
    pub answer: String,
    pub steps: Vec<Step>,
    pub usage: Usage,
    /// Why the model had to answer before it was done, if it did.
    pub stopped_early: Option<String>,
    /// The saved conversation this turn belongs to (`ask -c`, `--resume`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<i64>,
    pub note: &'static str,
}

/// One tool call.
#[derive(Debug, Serialize)]
pub struct Step {
    /// Which model request asked for it (1-based).
    pub request: usize,
    pub tool: String,
    pub arguments: Value,
    pub ok: bool,
    pub error: Option<String>,
    /// Size of the result sent back to the model.
    pub result_bytes: usize,
}

#[derive(Debug, Default, Serialize)]
pub struct Usage {
    pub requests: usize,
    pub tool_calls: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl Usage {
    fn total(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// The system prompt: the shared educational rules, then what the
/// assistant may do and how to answer.
pub fn system_prompt(market: crate::market::Market, can_change: bool) -> String {
    let today = chrono::Local::now().format("%A %d %B %Y").to_string();
    crate::ai::prompts::ask(
        &market_code(market),
        &today,
        &profile_line(&crate::config::effective_profile()),
        can_change,
    )
}

/// One paragraph on the user's investor profile for the system prompt.
pub fn profile_line(p: &crate::config::EffectiveProfile) -> String {
    use crate::config::Source;
    let mut line = format!(
        "The user's investor profile: {} (mode {}, risk {}), horizon {} years, largest weight of \
one stock {:.0}%, small and micro caps together {:.0}%, tax slab {:.0}% ({} regime). Use it to \
pick what matters to them, and measure concentration against these limits.",
        p.label,
        p.mode.as_str(),
        p.risk.as_str(),
        p.horizon_years,
        p.max_stock_weight,
        p.max_smallcap_weight,
        p.tax_slab,
        p.tax_regime.as_str(),
    );
    if p.mode_source == Source::Default && p.risk_source == Source::Default {
        line.push_str(
            " These are defaults: the user has not set up a profile. When it matters, mention \
that profile_questions (or `stockwise profile set`) sets it up.",
        );
    }
    line
}
fn market_code(market: crate::market::Market) -> String {
    use clap::ValueEnum;
    market
        .to_possible_value()
        .map(|v| v.get_name().to_string())
        .unwrap_or_else(|| "in".into())
}

/// A tool call the model asked for.
#[derive(Debug, Clone)]
struct Call {
    /// The provider's id for matching the result (OpenAI `call_id`,
    /// Anthropic `tool_use` id, Gemini's optional id). Empty when none.
    id: String,
    name: String,
    args: Value,
}

/// One model reply, after its assistant turn was added to the history.
#[derive(Debug, Default)]
struct Reply {
    text: String,
    calls: Vec<Call>,
    input_tokens: u64,
    output_tokens: u64,
    /// The answer was cut off by the output limit.
    truncated: bool,
    /// Anthropic paused a long turn; send the history again to resume.
    paused: bool,
}

/// A conversation in one provider's format.
pub struct Chat {
    provider: AiProvider,
    model: String,
    system: String,
    /// Ollama `messages`, OpenAI `input` items, Anthropic `messages`, Gemini `contents`.
    history: Vec<Value>,
    fallbacks: bool,
    /// Changes the write tools made during the current question.
    made: Vec<String>,
    /// Told to the model with the next question: changes an earlier,
    /// failed attempt made, so a retry does not make them twice.
    carry: Option<String>,
}

/// A chat as it is saved between runs: the provider and model it belongs
/// to, and its history in that provider's format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedChat {
    pub provider: String,
    pub model: String,
    pub history: Vec<Value>,
}

/// Tool results longer than this are trimmed once a new question starts.
const KEEP_RESULT_BYTES: usize = 1_500;

impl Chat {
    /// A new chat with no messages yet.
    pub fn new(ai: &AiClient, system: String) -> Self {
        let provider = ai.provider();
        let mut history = Vec::new();
        if provider == AiProvider::Ollama {
            history.push(json!({"role": "system", "content": system}));
        }
        Self {
            provider,
            model: ai.model().to_string(),
            system,
            history,
            fallbacks: ai.anthropic_fallbacks(),
            made: Vec::new(),
            carry: None,
        }
    }

    /// A saved chat, carried on with `ai` (which must be the same provider
    /// and model) and a fresh system prompt.
    pub fn resume(ai: &AiClient, system: String, saved: SavedChat) -> Result<Self> {
        let provider = ai.provider();
        if saved.provider != provider.name() || saved.model != ai.model() {
            bail!(
                "That conversation was with {} ({}); this run uses {} ({}). Set \
STOCKWISE_AI_PROVIDER and STOCKWISE_MODEL to match, or start a new one.",
                saved.provider,
                saved.model,
                provider.name(),
                ai.model()
            );
        }
        let mut chat = Self::new(ai, system);
        let rest = saved
            .history
            .into_iter()
            .filter(|m| !(provider == AiProvider::Ollama && m["role"] == "system"));
        chat.history.extend(rest);
        Ok(chat)
    }

    /// The chat to save.
    pub fn saved(&self) -> SavedChat {
        SavedChat {
            provider: self.provider.name().to_string(),
            model: self.model.clone(),
            history: self.history.clone(),
        }
    }

    /// Whether any question has been asked yet.
    pub fn is_empty(&self) -> bool {
        !self.history.iter().any(|m| m["role"] == "user")
    }

    /// Add the user's next question.
    fn push_user(&mut self, question: &str) {
        self.trim_old_results();
        let question = match self.carry.take() {
            Some(note) => format!("{note}\n\n{question}"),
            None => question.to_string(),
        };
        let msg = match self.provider {
            AiProvider::Gemini => json!({"role": "user", "parts": [{"text": question}]}),
            _ => json!({"role": "user", "content": question}),
        };
        self.history.push(msg);
        self.drop_old_exchanges();
    }

    /// Whether `m` is one of the user's questions (not tool results).
    fn is_question(&self, m: &Value) -> bool {
        if m.get("role").and_then(Value::as_str) != Some("user") {
            return false;
        }
        match self.provider {
            AiProvider::Gemini => m
                .get("parts")
                .and_then(Value::as_array)
                .is_some_and(|ps| ps.iter().any(|p| p.get("text").is_some())),
            _ => m.get("content").is_some_and(Value::is_string),
        }
    }

    /// Size of the history as sent, in bytes.
    pub fn history_bytes(&self) -> usize {
        self.history.iter().map(|m| m.to_string().len()).sum()
    }

    /// Most history bytes sent: half of Ollama's context window (the tool
    /// definitions and the answer need the rest), about 100k tokens for a
    /// cloud model.
    fn history_budget(&self) -> usize {
        match self.provider {
            AiProvider::Ollama => OLLAMA_NUM_CTX as usize * 4 / 2,
            _ => 400_000,
        }
    }

    /// Drop the oldest whole exchanges (a question and everything up to the
    /// next one, tool calls and results included) until the history fits
    /// its budget. The system message and the newest question stay.
    fn drop_old_exchanges(&mut self) {
        while self.history_bytes() > self.history_budget() {
            let qs: Vec<usize> = (0..self.history.len())
                .filter(|&i| self.is_question(&self.history[i]))
                .collect();
            let [first, second, ..] = qs[..] else {
                return;
            };
            self.history.drain(first..second);
        }
    }

    /// Replace long tool results already in the history by a short note:
    /// the answers that used them stay, and the model can call again.
    fn trim_old_results(&mut self) {
        let note = "earlier tool result, trimmed; call the tool again if you need it";
        let long = |v: &Value| match v {
            Value::String(s) => s.len() > KEEP_RESULT_BYTES,
            other => other.to_string().len() > KEEP_RESULT_BYTES,
        };
        for m in &mut self.history {
            match self.provider {
                AiProvider::Ollama if m["role"] == "tool" && long(&m["content"]) => {
                    m["content"] = json!(note);
                }
                AiProvider::OpenAi if m["type"] == "function_call_output" && long(&m["output"]) => {
                    m["output"] = json!(note);
                }
                // `get_mut`, not indexing: indexing a missing key would add it.
                AiProvider::Anthropic => {
                    if let Some(blocks) = m.get_mut("content").and_then(Value::as_array_mut) {
                        for b in blocks {
                            if b.get("type").and_then(Value::as_str) != Some("tool_result") {
                                continue;
                            }
                            if let Some(c) = b.get_mut("content").filter(|c| long(c)) {
                                *c = json!(note);
                            }
                        }
                    }
                }
                AiProvider::Gemini => {
                    if let Some(parts) = m.get_mut("parts").and_then(Value::as_array_mut) {
                        for p in parts {
                            if let Some(r) = p
                                .get_mut("functionResponse")
                                .and_then(|f| f.get_mut("response"))
                                .filter(|r| long(r))
                            {
                                *r = json!({ "note": note });
                            }
                        }
                    }
                }
                _ => {}
            }
        }
    }

    /// The request path and body; without tools when `final_turn`.
    fn request(&self, tools: &[&Tool], final_turn: bool, max_out: u32) -> (String, Value) {
        match self.provider {
            AiProvider::Ollama => {
                let mut body = json!({
                    "model": self.model,
                    "messages": self.history,
                    "stream": false,
                    "options": {
                        "num_ctx": OLLAMA_NUM_CTX,
                        "num_predict": max_out.min(OLLAMA_NUM_PREDICT),
                    },
                });
                if !final_turn {
                    body["tools"] = tools
                        .iter()
                        .map(|t| {
                            json!({"type": "function", "function": {
                                "name": t.name,
                                "description": t.description,
                                "parameters": t.input_schema(),
                            }})
                        })
                        .collect();
                }
                ("/api/chat".into(), body)
            }
            AiProvider::OpenAi => {
                let mut body = json!({
                    "model": self.model,
                    "instructions": self.system,
                    "input": self.history,
                    "tools": tools.iter().map(|t| json!({
                        "type": "function",
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema(),
                        // Optional parameters stay optional.
                        "strict": false,
                    })).collect::<Vec<_>>(),
                    "max_output_tokens": max_out,
                    // Nothing kept on OpenAI's side: reasoning travels back
                    // encrypted with the conversation instead.
                    "store": false,
                    "include": ["reasoning.encrypted_content"],
                });
                if final_turn {
                    body["tool_choice"] = json!("none");
                }
                ("/v1/responses".into(), body)
            }
            AiProvider::Anthropic => {
                let mut body = json!({
                    "model": self.model,
                    "max_tokens": max_out,
                    "system": self.system,
                    "tools": tools.iter().map(|t| json!({
                        "name": t.name,
                        "description": t.description,
                        "input_schema": t.input_schema(),
                    })).collect::<Vec<_>>(),
                    "messages": self.history,
                });
                if final_turn {
                    body["tool_choice"] = json!({"type": "none"});
                }
                if self.fallbacks {
                    body["fallbacks"] = json!("default");
                }
                ("/v1/messages".into(), body)
            }
            AiProvider::Gemini => {
                let mut body = json!({
                    "systemInstruction": {"parts": [{"text": self.system}]},
                    "contents": self.history,
                    "tools": [{"functionDeclarations": tools.iter().map(|t| json!({
                        "name": t.name,
                        "description": t.description,
                        "parametersJsonSchema": t.input_schema(),
                    })).collect::<Vec<_>>()}],
                    "generationConfig": {"maxOutputTokens": max_out},
                });
                if final_turn {
                    body["toolConfig"] = json!({"functionCallingConfig": {"mode": "NONE"}});
                }
                (
                    format!("/v1beta/models/{}:generateContent", self.model),
                    body,
                )
            }
        }
    }

    /// Read a reply and append the model's turn to the history, exactly as
    /// received (providers need their own blocks back: thinking signatures,
    /// encrypted reasoning, call ids).
    /// Tokens the model can read, when the conversation must fit it (Ollama
    /// drops the start of a conversation that doesn't).
    fn context_window(&self) -> Option<u64> {
        (self.provider == AiProvider::Ollama).then_some(OLLAMA_NUM_CTX as u64)
    }

    fn take_reply(&mut self, resp: &Value) -> Result<Reply> {
        match self.provider {
            AiProvider::Ollama => self.take_ollama(resp),
            AiProvider::OpenAi => self.take_openai(resp),
            AiProvider::Anthropic => self.take_anthropic(resp),
            AiProvider::Gemini => self.take_gemini(resp),
        }
    }

    fn take_ollama(&mut self, resp: &Value) -> Result<Reply> {
        let msg = &resp["message"];
        if !msg.is_object() {
            bail!("Ollama returned no message");
        }
        let calls = msg["tool_calls"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| {
                let f = &c["function"];
                Some(Call {
                    id: c["id"].as_str().unwrap_or_default().to_string(),
                    name: f["name"].as_str()?.to_string(),
                    args: f["arguments"].clone(),
                })
            })
            .collect();
        self.history.push(msg.clone());
        Ok(Reply {
            text: msg["content"].as_str().unwrap_or_default().to_string(),
            calls,
            input_tokens: resp["prompt_eval_count"].as_u64().unwrap_or(0),
            output_tokens: resp["eval_count"].as_u64().unwrap_or(0),
            truncated: resp["done_reason"].as_str() == Some("length"),
            paused: false,
        })
    }

    fn take_openai(&mut self, resp: &Value) -> Result<Reply> {
        let mut reply = Reply {
            input_tokens: resp["usage"]["input_tokens"].as_u64().unwrap_or(0),
            output_tokens: resp["usage"]["output_tokens"].as_u64().unwrap_or(0),
            truncated: resp["status"].as_str() == Some("incomplete"),
            ..Default::default()
        };
        let mut refusal = None;
        for item in resp["output"].as_array().into_iter().flatten() {
            match item["type"].as_str() {
                Some("function_call") => {
                    let args = item["arguments"]
                        .as_str()
                        .and_then(|a| serde_json::from_str(a).ok())
                        .unwrap_or_else(|| item["arguments"].clone());
                    reply.calls.push(Call {
                        id: item["call_id"].as_str().unwrap_or_default().to_string(),
                        name: item["name"].as_str().unwrap_or_default().to_string(),
                        args,
                    });
                }
                Some("message") => {
                    for part in item["content"].as_array().into_iter().flatten() {
                        match part["type"].as_str() {
                            Some("output_text") => {
                                reply.text.push_str(part["text"].as_str().unwrap_or(""))
                            }
                            Some("refusal") => refusal = part["refusal"].as_str(),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
            self.history.push(item.clone());
        }
        if reply.text.is_empty()
            && reply.calls.is_empty()
            && let Some(r) = refusal
        {
            bail!("OpenAI declined the request: {}", r);
        }
        Ok(reply)
    }

    fn take_anthropic(&mut self, resp: &Value) -> Result<Reply> {
        let stop = resp["stop_reason"].as_str().unwrap_or("");
        if stop == "refusal" {
            let category = resp["stop_details"]["category"]
                .as_str()
                .unwrap_or("unspecified");
            bail!("Anthropic declined the request (category: {})", category);
        }
        let content = resp["content"].clone();
        let mut reply = Reply {
            input_tokens: resp["usage"]["input_tokens"].as_u64().unwrap_or(0)
                + resp["usage"]["cache_read_input_tokens"]
                    .as_u64()
                    .unwrap_or(0)
                + resp["usage"]["cache_creation_input_tokens"]
                    .as_u64()
                    .unwrap_or(0),
            output_tokens: resp["usage"]["output_tokens"].as_u64().unwrap_or(0),
            truncated: stop == "max_tokens",
            paused: stop == "pause_turn",
            ..Default::default()
        };
        for block in content.as_array().into_iter().flatten() {
            match block["type"].as_str() {
                Some("text") => reply.text.push_str(block["text"].as_str().unwrap_or("")),
                Some("tool_use") => reply.calls.push(Call {
                    id: block["id"].as_str().unwrap_or_default().to_string(),
                    name: block["name"].as_str().unwrap_or_default().to_string(),
                    args: block["input"].clone(),
                }),
                _ => {}
            }
        }
        // A tool input cut off by max_tokens can still parse: never run it.
        if reply.truncated && !reply.calls.is_empty() {
            bail!(
                "The model's tool call was cut off by the output limit ({} tokens)",
                reply.output_tokens
            );
        }
        self.history
            .push(json!({"role": "assistant", "content": content}));
        Ok(reply)
    }

    fn take_gemini(&mut self, resp: &Value) -> Result<Reply> {
        if let Some(reason) = resp["promptFeedback"]["blockReason"].as_str() {
            bail!("Gemini blocked the prompt ({})", reason);
        }
        let candidate = &resp["candidates"][0];
        let finish = candidate["finishReason"].as_str().unwrap_or("STOP");
        let content = &candidate["content"];
        let mut reply = Reply {
            input_tokens: resp["usageMetadata"]["promptTokenCount"]
                .as_u64()
                .unwrap_or(0),
            output_tokens: resp["usageMetadata"]["candidatesTokenCount"]
                .as_u64()
                .unwrap_or(0)
                + resp["usageMetadata"]["thoughtsTokenCount"]
                    .as_u64()
                    .unwrap_or(0),
            truncated: finish == "MAX_TOKENS",
            ..Default::default()
        };
        for part in content["parts"].as_array().into_iter().flatten() {
            if part["thought"].as_bool() == Some(true) {
                continue;
            }
            if let Some(t) = part["text"].as_str() {
                reply.text.push_str(t);
            }
            if let Some(f) = part.get("functionCall") {
                reply.calls.push(Call {
                    id: f["id"].as_str().unwrap_or_default().to_string(),
                    name: f["name"].as_str().unwrap_or_default().to_string(),
                    args: f["args"].clone(),
                });
            }
        }
        if reply.text.is_empty() && reply.calls.is_empty() && finish != "STOP" {
            bail!("Gemini returned no answer (finish reason {})", finish);
        }
        if content.is_object() {
            self.history.push(content.clone());
        }
        Ok(reply)
    }

    /// Send tool results back: each is the result JSON (or `{"error": ...}`).
    fn add_results(&mut self, results: Vec<(Call, Value, bool)>) {
        match self.provider {
            AiProvider::Ollama => {
                for (call, v, _) in results {
                    self.history.push(json!({
                        "role": "tool",
                        "tool_name": call.name,
                        "content": v.to_string(),
                    }));
                }
            }
            AiProvider::OpenAi => {
                for (call, v, _) in results {
                    self.history.push(json!({
                        "type": "function_call_output",
                        "call_id": call.id,
                        "output": v.to_string(),
                    }));
                }
            }
            AiProvider::Anthropic => {
                // Every result of a turn goes back in one user message.
                let blocks: Vec<Value> = results
                    .into_iter()
                    .map(|(call, v, is_error)| {
                        let mut b = json!({
                            "type": "tool_result",
                            "tool_use_id": call.id,
                            "content": v.to_string(),
                        });
                        if is_error {
                            b["is_error"] = json!(true);
                        }
                        b
                    })
                    .collect();
                self.history
                    .push(json!({"role": "user", "content": blocks}));
            }
            AiProvider::Gemini => {
                let parts: Vec<Value> = results
                    .into_iter()
                    .map(|(call, v, _)| {
                        let mut r = json!({"name": call.name, "response": v});
                        if !call.id.is_empty() {
                            r["id"] = json!(call.id);
                        }
                        json!({"functionResponse": r})
                    })
                    .collect();
                self.history.push(json!({"role": "user", "parts": parts}));
            }
        }
    }
}

/// A clearer message for the errors people hit first: a model without tool
/// calling, or an Ollama model that isn't pulled.
fn explain(e: anyhow::Error, ai: &AiClient) -> anyhow::Error {
    let msg = format!("{e:#}");
    let lower = msg.to_lowercase();
    let model = ai.model();
    let provider = ai.provider();
    let no_tools = [
        "does not support tools",
        "does not support tool",
        "tool use is not supported",
        "tools are not supported",
        "function calling is not enabled",
        "does not support function",
    ]
    .iter()
    .any(|p| lower.contains(p));
    if no_tools {
        return anyhow!(
            "{} model '{}' does not support tool calling, which `ask` needs. {}",
            provider.name(),
            model,
            if provider == AiProvider::Ollama {
                "Pull one that does — e.g. `ollama pull qwen3:14b` (or llama3.1, qwen2.5, mistral-nemo) — and set STOCKWISE_MODEL."
            } else {
                "Set STOCKWISE_MODEL to a model with tool use."
            }
        );
    }
    if provider == AiProvider::Ollama && lower.contains("not found") && lower.contains("model") {
        return anyhow!(
            "Ollama model '{}' is not installed. Run: ollama pull {}  (or set STOCKWISE_MODEL)",
            model,
            model
        );
    }
    e
}

/// Tool calls of one model turn that run at the same time. The Yahoo and NSE
/// clients space their requests themselves, so this only overlaps the waiting.
const MAX_PARALLEL_TOOLS: usize = 4;

/// Answer `question` with the tools in `ctx`, in a new chat.
pub async fn ask<D: DataSource>(
    ai: &AiClient,
    ctx: &ToolContext<D>,
    question: &str,
    opts: &AskOptions,
) -> Result<Transcript> {
    let mut chat = Chat::new(ai, system_prompt(ctx.market, ctx.workspace.is_some()));
    ask_in(ai, ctx, &mut chat, question, opts).await
}

/// Answer `question` as the next turn of `chat`. On an error the chat is
/// left as it was before the question.
pub async fn ask_in<D: DataSource>(
    ai: &AiClient,
    ctx: &ToolContext<D>,
    chat: &mut Chat,
    question: &str,
    opts: &AskOptions,
) -> Result<Transcript> {
    let question = question.trim();
    if question.is_empty() {
        bail!("Ask a question, e.g. stockwise ask \"how did my portfolio do this month?\"");
    }
    let before = chat.history.clone();
    let carried = chat.carry.clone();
    chat.made.clear();
    chat.push_user(question);
    let out = turn(ai, ctx, chat, question, opts).await;
    match out {
        Ok(t) => Ok(t),
        Err(e) => {
            chat.history = before;
            chat.carry = carried;
            if chat.made.is_empty() {
                return Err(e);
            }
            // The files changed although the answer failed: say so, and
            // tell the model with the next question so a retry does not
            // make the same change again.
            let made = chat.made.join("; ");
            let note = format!(
                "(Note: a previous attempt at this failed after making these changes, which \
are saved: {made}. Do not make them again.)"
            );
            chat.carry = Some(match chat.carry.take() {
                Some(old) => format!("{old}\n{note}"),
                None => note,
            });
            Err(e.context(format!(
                "These changes were made before the failure and are saved (stockwise undo \
reverses them): {made}"
            )))
        }
    }
}

async fn turn<D: DataSource>(
    ai: &AiClient,
    ctx: &ToolContext<D>,
    chat: &mut Chat,
    question: &str,
    opts: &AskOptions,
) -> Result<Transcript> {
    let tools = ctx.tools();
    let mut t = Transcript {
        question: question.to_string(),
        provider: ai.provider().name(),
        model: ai.model().to_string(),
        answer: String::new(),
        steps: Vec::new(),
        usage: Usage::default(),
        stopped_early: None,
        session: None,
        note: NOTE,
    };
    // Estimated length of the next prompt, in tokens (for a model whose
    // context window it must fit).
    let mut next_prompt: u64 = chat.history_bytes() as u64 / 4;
    loop {
        let limit = if t.usage.requests + 1 >= opts.max_steps.max(1) {
            Some(format!("reached {} model requests", opts.max_steps))
        } else if t.usage.tool_calls >= opts.max_tool_calls {
            Some(format!("reached {} tool calls", opts.max_tool_calls))
        } else if t.usage.total() >= opts.token_budget {
            Some(format!("used {} tokens", t.usage.total()))
        } else if chat
            .context_window()
            .is_some_and(|w| next_prompt > w * 3 / 4)
        {
            Some("filled most of the model's context window".to_string())
        } else {
            None
        };
        let final_turn = limit.is_some();
        let (path, body) = chat.request(&tools, final_turn, opts.max_output_tokens);
        let resp = ai
            .post_api(&path, &body)
            .await
            .map_err(|e| explain(e, ai))?;
        t.usage.requests += 1;
        let reply = chat.take_reply(&resp)?;
        t.usage.input_tokens += reply.input_tokens;
        t.usage.output_tokens += reply.output_tokens;
        next_prompt = reply.input_tokens + reply.output_tokens;

        if reply.paused && !final_turn {
            continue;
        }
        if reply.calls.is_empty() || final_turn {
            let answer = crate::ai::strip_think_tags(&reply.text).trim().to_string();
            if answer.is_empty() {
                bail!(
                    "{} ({}) returned no answer{}",
                    ai.provider().name(),
                    ai.model(),
                    limit.map(|l| format!(" after it {l}")).unwrap_or_default()
                );
            }
            t.answer = answer;
            t.stopped_early = if reply.truncated {
                Some("the answer reached the output limit and may be cut off".into())
            } else {
                limit.filter(|_| !t.steps.is_empty())
            };
            return Ok(t);
        }

        // Decide, in the model's order, which calls the limit still allows;
        // those then run together (at most `MAX_PARALLEL_TOOLS` at a time),
        // and their answers are recorded in the model's order again.
        let mut planned = Vec::new();
        for call in reply.calls {
            if t.usage.tool_calls >= opts.max_tool_calls {
                planned.push((call, false));
            } else {
                t.usage.tool_calls += 1;
                if opts.verbose {
                    eprintln!("  → {} {}", call.name, call.args);
                }
                planned.push((call, true));
            }
        }
        let outcomes: Vec<Result<Value>> =
            futures_util::stream::iter(planned.iter().map(|(call, allowed)| async move {
                if *allowed {
                    ctx.call(&call.name, &call.args).await
                } else {
                    Ok(Value::Null)
                }
            }))
            .buffered(MAX_PARALLEL_TOOLS)
            .collect()
            .await;
        let mut results = Vec::new();
        for ((call, allowed), outcome) in planned.into_iter().zip(outcomes) {
            let (value, ok, error) = if !allowed {
                let msg = "Tool-call limit reached: answer with what you have.".to_string();
                (json!({ "error": msg }), false, Some(msg))
            } else {
                match outcome {
                    Ok(v) => {
                        if crate::tools::access(&call.name) != crate::tools::Access::Read {
                            let id = v
                                .pointer("/data/change_id")
                                .or_else(|| v.get("change_id"))
                                .and_then(Value::as_u64);
                            chat.made.push(match id {
                                Some(id) => format!("{} {} (change #{id})", call.name, call.args),
                                None => format!("{} {}", call.name, call.args),
                            });
                        }
                        (v, true, None)
                    }
                    Err(e) => {
                        // This text goes back to the model, which may be a
                        // cloud one: no local paths, no Yahoo crumb. The full
                        // error stays on this machine.
                        let msg = crate::tools::client_error(&e);
                        if opts.verbose && format!("{e:#}") != msg {
                            eprintln!("    full error (not sent): {e:#}");
                        }
                        (json!({ "error": msg }), false, Some(msg))
                    }
                }
            };
            let bytes = value.to_string().len();
            // About four bytes a token for JSON.
            next_prompt += bytes as u64 / 4;
            if opts.verbose {
                match &error {
                    None => eprintln!("    ✓ {bytes} bytes"),
                    Some(e) => eprintln!("    ✗ {e}"),
                }
            }
            t.steps.push(Step {
                request: t.usage.requests,
                tool: call.name.clone(),
                arguments: call.args.clone(),
                ok,
                error,
                result_bytes: bytes,
            });
            results.push((call, value, !ok));
        }
        chat.add_results(results);
    }
}

#[cfg(test)]
mod tests;
