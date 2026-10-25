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

use crate::ai::{AiClient, AiProvider, CLOUD_MAX_TOKENS};
use crate::tools::{NOTE, Tool, ToolContext};
use crate::views::DataSource;
use anyhow::{Result, anyhow, bail};
use serde::Serialize;
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

/// The system prompt: what the model is, what it may do, and how to answer.
pub fn system_prompt(market: crate::market::Market) -> String {
    let today = chrono::Local::now().format("%A %d %B %Y");
    format!(
        "You are the research assistant of StockWise, a stock-market tool running in the user's \
         terminal. Answer the user's question with the StockWise tools: they return live (often \
         delayed) market data and the user's own portfolio, watchlist and alerts. Today is \
         {today}. The user's default market is `{}`: bare tickers such as TCS resolve there; \
         pass `market` for other markets (us, crypto, fx, commodity, uk, jp, ...).\n\n\
         Call tools for every number instead of guessing, and keep calls few and targeted. Then \
         answer in plain English, briefly, quoting the figures you used with their currency \
         and date. Say when data is missing or a tool failed. This is educational information, \
         not financial advice: describe what the data shows and the risks, and do not tell the \
         user to buy, sell or hold. You cannot place orders.",
        market_code(market)
    )
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
struct Chat {
    provider: AiProvider,
    model: String,
    system: String,
    /// Ollama `messages`, OpenAI `input` items, Anthropic `messages`, Gemini `contents`.
    history: Vec<Value>,
    fallbacks: bool,
}

impl Chat {
    fn new(ai: &AiClient, system: String, question: &str) -> Self {
        let provider = ai.provider();
        let first = match provider {
            AiProvider::Ollama | AiProvider::Anthropic => {
                json!({"role": "user", "content": question})
            }
            AiProvider::OpenAi => json!({"role": "user", "content": question}),
            AiProvider::Gemini => json!({"role": "user", "parts": [{"text": question}]}),
        };
        let mut history = Vec::new();
        if provider == AiProvider::Ollama {
            history.push(json!({"role": "system", "content": system}));
        }
        history.push(first);
        Self {
            provider,
            model: ai.model().to_string(),
            system,
            history,
            fallbacks: ai.anthropic_fallbacks(),
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

/// Answer `question` with the tools in `ctx`.
pub async fn ask<D: DataSource>(
    ai: &AiClient,
    ctx: &ToolContext<D>,
    question: &str,
    opts: &AskOptions,
) -> Result<Transcript> {
    let question = question.trim();
    if question.is_empty() {
        bail!("Ask a question, e.g. stockwise ask \"how did my portfolio do this month?\"");
    }
    let tools = ctx.tools();
    let mut chat = Chat::new(ai, system_prompt(ctx.market), question);
    let mut t = Transcript {
        question: question.to_string(),
        provider: ai.provider().name(),
        model: ai.model().to_string(),
        answer: String::new(),
        steps: Vec::new(),
        usage: Usage::default(),
        stopped_early: None,
        note: NOTE,
    };
    // Estimated length of the next prompt, in tokens (for a model whose
    // context window it must fit).
    let mut next_prompt: u64 = 0;
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

        let mut results = Vec::new();
        for call in reply.calls {
            let (value, ok, error) = if t.usage.tool_calls >= opts.max_tool_calls {
                let msg = "Tool-call limit reached: answer with what you have.".to_string();
                (json!({ "error": msg }), false, Some(msg))
            } else {
                t.usage.tool_calls += 1;
                if opts.verbose {
                    eprintln!("  → {} {}", call.name, call.args);
                }
                match ctx.call(&call.name, &call.args).await {
                    Ok(v) => (v, true, None),
                    Err(e) => {
                        let msg = format!("{e:#}");
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
