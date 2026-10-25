//! AI assistants: `ask` (a question answered by the configured model with
//! the read-only tools) and `mcp` (the same tools served to Claude, ChatGPT,
//! Cursor and other MCP clients).

use super::*;
use crate::ai::AiClient;
use crate::ask::{self, AskOptions};
use crate::tools::ToolContext;

/// Why `ask` can't run, with the fix.
fn ask_unavailable(ai: &AiClient) -> String {
    if ai.config_error().is_some() || ai.provider().is_cloud() {
        return ai.unavailable_reason();
    }
    format!(
        "`ask` needs an AI model with tool calling, but Ollama is not reachable at {}. Install it \
         from https://ollama.com, run `ollama serve` and `ollama pull {}` (set STOCKWISE_MODEL for \
         another model, OLLAMA_HOST for a remote server, or STOCKWISE_AI_PROVIDER for a cloud \
         provider).",
        crate::ai::ollama_url(),
        ai.model()
    )
}

/// `stockwise ask`.
pub async fn cmd_ask(question: &str, opts: AskOptions, market: Market, json: bool) -> Result<()> {
    // Running `ask` is the opt-in, as for `report`.
    let ai = AiClient::connect_explicit().await;
    if !ai.is_available().await {
        bail!("{}", ask_unavailable(&ai));
    }
    let mut ctx = ToolContext::new(views::LiveSource::new(), market);
    if ai.provider().is_cloud() {
        eprintln!(
            "  \u{24d8} {} also receives what the tools return — including your holdings, lots \
             and tax figures if it asks for them.",
            ai.provider().name()
        );
    } else {
        ctx.max_result_bytes = ask::OLLAMA_RESULT_BYTES;
    }
    {
        use std::io::IsTerminal;
        if !json && std::io::stderr().is_terminal() {
            eprintln!(
                "  {} Asking {} ({}); it can call StockWise's read-only tools…",
                "⟳".dimmed(),
                ai.model(),
                ai.provider().name()
            );
        }
    }
    let t = ask::ask(&ai, &ctx, question, &opts).await?;
    if json {
        return output::print_json(&t);
    }
    println!("\n{}\n", t.answer);
    let mut used: Vec<String> = Vec::new();
    for s in &t.steps {
        if !used.contains(&s.tool) {
            used.push(s.tool.clone());
        }
    }
    let tools = if used.is_empty() {
        "no tools".to_string()
    } else {
        format!("tools: {}", used.join(", "))
    };
    if let Some(why) = &t.stopped_early {
        println!("  {}", format!("Stopped early: {why}.").yellow());
    }
    println!(
        "  {}\n",
        format!(
            "{} · {} ({}) · educational, not financial advice",
            tools, t.model, t.provider
        )
        .dimmed()
    );
    Ok(())
}

/// `stockwise mcp`.
pub async fn cmd_mcp(
    http: Option<&str>,
    allow_remote: bool,
    allow_orders: bool,
    list_tools: bool,
    market: Market,
) -> Result<()> {
    if list_tools {
        let mut ctx = ToolContext::new(views::LiveSource::new(), market);
        ctx.allow_orders = allow_orders;
        let tools: Vec<serde_json::Value> = ctx
            .tools()
            .iter()
            .map(|t| {
                serde_json::json!({
                    "name": t.name,
                    "title": t.title,
                    "description": t.description,
                    "inputSchema": t.input_schema(),
                })
            })
            .collect();
        return output::print_json(&tools);
    }
    serve_mcp(http, allow_remote, allow_orders, market).await
}

#[cfg(feature = "mcp")]
async fn serve_mcp(
    http: Option<&str>,
    allow_remote: bool,
    allow_orders: bool,
    market: Market,
) -> Result<()> {
    crate::mcp::run(market, allow_orders, http, allow_remote).await
}

#[cfg(not(feature = "mcp"))]
async fn serve_mcp(
    _http: Option<&str>,
    _allow_remote: bool,
    _allow_orders: bool,
    _market: Market,
) -> Result<()> {
    bail!(
        "This stockwise was built without the `mcp` feature. Reinstall with \
         `cargo install stockwise` (default features)."
    )
}
