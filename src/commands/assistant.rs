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
        if crate::ai::ollama_is_remote() {
            eprintln!(
                "  \u{24d8} Ollama at {} is not on this computer; it also receives what the tools \
                 return — including your holdings, lots and tax figures if it asks for them.",
                crate::ai::ollama_host_label()
            );
        }
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
/// What an MCP server may do beyond reading.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct McpAccess {
    /// `order_preview` (dry runs only).
    pub orders: bool,
    /// Workspace changes: watchlist, alerts, goals, targets, profile, notes.
    pub writes: bool,
    /// Portfolio lot changes too (implies `writes`).
    pub portfolio: bool,
}

pub async fn cmd_mcp(
    http: Option<&str>,
    allow_remote: bool,
    access: McpAccess,
    list_tools: bool,
    market: Market,
) -> Result<()> {
    if allow_remote && (access.writes || access.portfolio) {
        bail!(
            "--allow-writes and --allow-portfolio are for a server on this machine; \
             they cannot be combined with --allow-remote"
        );
    }
    if list_tools {
        let mut ctx = ToolContext::new(views::LiveSource::new(), market);
        ctx.allow_orders = access.orders;
        if access.writes || access.portfolio {
            ctx.workspace = Some(crate::tools::Workspace::for_user("mcp", access.portfolio)?);
        }
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
    serve_mcp(http, allow_remote, access, market).await
}

#[cfg(feature = "mcp")]
async fn serve_mcp(
    http: Option<&str>,
    allow_remote: bool,
    access: McpAccess,
    market: Market,
) -> Result<()> {
    // Keeps held and watched quotes warm in cache.db while the server runs.
    let _ticker = crate::worker::spawn_ticker(market, crate::worker::bus());
    crate::mcp::run(market, access, http, allow_remote).await
}

#[cfg(not(feature = "mcp"))]
async fn serve_mcp(
    _http: Option<&str>,
    _allow_remote: bool,
    _access: McpAccess,
    _market: Market,
) -> Result<()> {
    bail!(
        "This stockwise was built without the `mcp` feature. Reinstall with \
         `cargo install stockwise` (default features)."
    )
}

/// `stockwise undo`: put back the files of an assistant's change, or list
/// the changes.
pub fn cmd_undo(id: Option<u64>, list: bool, force: bool, json: bool) -> Result<()> {
    let journal = crate::audit::Journal::default_location()?;
    if list {
        let changes = journal.list()?;
        if json {
            return output::print_json(&changes);
        }
        print_header("Assistant changes");
        if changes.is_empty() {
            println!("  {}", "No changes recorded.".dimmed());
            println!();
            return Ok(());
        }
        for c in changes.iter().take(30) {
            let at = c.at.get(..16).unwrap_or(&c.at).replace('T', " ");
            let line = format!("  #{:<4} {}  {:<4} {}", c.id, at, c.source, c.summary);
            if c.undone {
                println!("{}  {}", line.dimmed(), "(undone)".dimmed());
            } else {
                println!("{}", line);
            }
        }
        println!();
        return Ok(());
    }
    let change = journal.undo(id, force)?;
    if json {
        return output::print_json(&change);
    }
    println!(
        "  {} Undid #{}: {} ({} file{})",
        "✓".green(),
        change.id,
        change.summary,
        change.files.len(),
        if change.files.len() == 1 { "" } else { "s" }
    );
    Ok(())
}
