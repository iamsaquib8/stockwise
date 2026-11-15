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

/// Which conversation `ask` carries on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AskSession {
    New,
    Latest,
    Id(i64),
}

/// What the user asked `ask` to do.
#[derive(Debug, Clone)]
pub struct AskRequest {
    pub question: String,
    pub interactive: bool,
    pub session: AskSession,
    pub playbook: Option<ask::Playbook>,
    pub auto: bool,
}

/// `text` cut to `max` characters, with an ellipsis when cut.
fn short(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= max {
        return one_line;
    }
    let mut s: String = one_line.chars().take(max.saturating_sub(1)).collect();
    s.push('…');
    s
}

/// `stockwise ask --sessions`.
pub fn cmd_ask_sessions(json: bool) -> Result<()> {
    let sessions = crate::db::with_db(|db| db.sessions(30))
        .context("Saved conversations need the cache database (cache.db), which is off")?;
    if json {
        return output::print_json(&sessions);
    }
    print_header("Saved conversations");
    if sessions.is_empty() {
        println!(
            "  {}",
            "None yet. Ask something: stockwise ask \"...\"".dimmed()
        );
    }
    for s in &sessions {
        let when = chrono::DateTime::from_timestamp(s.updated_at, 0)
            .map(|d| {
                d.with_timezone(&chrono::Local)
                    .format("%Y-%m-%d %H:%M")
                    .to_string()
            })
            .unwrap_or_default();
        println!(
            "  #{:<4} {}  {:<3} {}  {}",
            s.id,
            when,
            s.turns,
            short(&s.title, 48),
            format!("{} {}", s.provider, s.model).dimmed()
        );
    }
    println!();
    println!(
        "  {}",
        "Carry one on: stockwise ask --resume ID \"...\" (or -c for the newest)".dimmed()
    );
    println!();
    Ok(())
}

/// `stockwise ask`.
pub async fn cmd_ask(req: AskRequest, opts: AskOptions, market: Market, json: bool) -> Result<()> {
    let mut first = req.question.trim().to_string();
    if let Some(p) = req.playbook {
        first = if first.is_empty() {
            p.prompt().to_string()
        } else {
            format!("{} Also: {}", p.prompt(), first)
        };
    }
    if first.is_empty() && !req.interactive {
        bail!(
            "Ask a question, e.g. stockwise ask \"how did my portfolio do this month?\" \
             (or -i for a conversation, --playbook checkup for a portfolio checkup)"
        );
    }
    if req.interactive && json {
        bail!("--json answers one question; leave it off with -i");
    }
    // Running `ask` is the opt-in, as for `report`.
    let ai = AiClient::connect_explicit().await;
    if !ai.is_available().await {
        bail!("{}", ask_unavailable(&ai));
    }
    let mut ctx = ToolContext::new(views::LiveSource::new(), market);
    if req.auto {
        ctx.workspace = Some(crate::tools::Workspace::for_user("ask", false)?);
    }
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
    let system = ask::system_prompt(market, req.auto);
    let (mut chat, mut session) = match req.session {
        AskSession::New => (ask::Chat::new(&ai, system), None),
        wanted => {
            let id = match wanted {
                AskSession::Id(id) => Some(id),
                _ => None,
            };
            let row = crate::db::with_db(|db| db.session(id))
                .context("Saved conversations need the cache database (cache.db), which is off")?
                .with_context(|| match id {
                    Some(id) => {
                        format!("No saved conversation #{id} (see stockwise ask --sessions)")
                    }
                    None => "No saved conversation yet; ask without -c to start one".to_string(),
                })?;
            let saved: ask::SavedChat = serde_json::from_str(&row.chat_json)
                .context("That saved conversation cannot be read")?;
            (ask::Chat::resume(&ai, system, saved)?, Some(row.id))
        }
    };
    let can_change = if req.auto {
        "read and change your workspace (undo: stockwise undo)"
    } else {
        "only read"
    };
    let interactive_terminal = {
        use std::io::IsTerminal;
        !json && std::io::stderr().is_terminal()
    };
    if interactive_terminal {
        eprintln!(
            "  {} {} ({}); its StockWise tools {}{}",
            "⟳".dimmed(),
            ai.model(),
            ai.provider().name(),
            can_change,
            match session {
                Some(id) => format!("; conversation #{id}"),
                None => String::new(),
            }
        );
    }
    let title = if first.is_empty() {
        "conversation".to_string()
    } else if let Some(p) = req.playbook {
        format!("playbook {:?}", p).to_lowercase()
    } else {
        first.clone()
    };
    let mut next = (!first.is_empty()).then_some(first);
    loop {
        let question = match next.take() {
            Some(q) => q,
            None => {
                if !req.interactive {
                    return Ok(());
                }
                eprint!("{} ", "you ›".cyan().bold());
                use std::io::Write;
                std::io::stderr().flush().ok();
                let mut line = String::new();
                if std::io::stdin().read_line(&mut line)? == 0 {
                    eprintln!();
                    return Ok(());
                }
                let line = line.trim();
                if matches!(line, "exit" | "quit" | "/exit" | "/quit") {
                    return Ok(());
                }
                if line.is_empty() {
                    continue;
                }
                line.to_string()
            }
        };
        let mut t = match ask::ask_in(&ai, &ctx, &mut chat, &question, &opts).await {
            Ok(t) => t,
            Err(e) if req.interactive => {
                eprintln!("  {} {:#}", "✗".red(), e);
                continue;
            }
            Err(e) => return Err(e),
        };
        // Save after each answer; a missing cache.db only loses the history.
        if let Ok(saved) = serde_json::to_string(&chat.saved()) {
            session = crate::db::with_db(|db| {
                db.save_session(
                    session,
                    &short(&title, 80),
                    ai.provider().name(),
                    ai.model(),
                    &saved,
                )
            })
            .or(session);
        }
        t.session = session;
        if json {
            return output::print_json(&t);
        }
        print_answer(&t);
    }
}

fn print_answer(t: &ask::Transcript) {
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
    let changed = t
        .steps
        .iter()
        .filter(|s| s.ok && crate::tools::access(&s.tool) != crate::tools::Access::Read)
        .count();
    if changed > 0 {
        println!(
            "  {}",
            format!("{changed} change(s) made; stockwise undo --list shows them.").yellow()
        );
    }
    println!(
        "  {}\n",
        format!(
            "{} · {} ({}){} · educational, not financial advice",
            tools,
            t.model,
            t.provider,
            t.session
                .map(|id| format!(" · conversation #{id}, carry on with -c"))
                .unwrap_or_default()
        )
        .dimmed()
    );
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

/// What `stockwise connect` does.
#[derive(Debug, Default)]
pub struct ConnectOptions {
    pub clients: Vec<String>,
    pub all: bool,
    pub list: bool,
    pub remove: bool,
    pub writes: bool,
    pub portfolio: bool,
    pub skill: bool,
    pub dry_run: bool,
}

pub fn cmd_connect(o: ConnectOptions, json: bool) -> Result<()> {
    use crate::connect;
    let home = dirs::home_dir().context("Cannot find your home folder")?;
    let all = connect::clients(&home);
    if o.list {
        let rows: Vec<_> = all
            .iter()
            .map(|c| {
                serde_json::json!({
                    "id": c.id,
                    "name": c.name,
                    "found": connect::detected(c),
                    "config": c.paths.first().map(|p| p.display().to_string()),
                    "skill": c.skills.as_ref().map(|p| p.join("stockwise").display().to_string()),
                })
            })
            .collect();
        if json {
            return output::print_json(&rows);
        }
        print_header("AI apps StockWise can connect to");
        for (c, r) in all.iter().zip(&rows) {
            let found = connect::detected(c);
            let mark = if found { "●".green() } else { "○".dimmed() };
            let where_ =
                r["config"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| match c.format {
                        connect::Format::ClaudeCli => "claude mcp add".into(),
                        _ => "paste by hand".into(),
                    });
            println!("  {} {:<15} {:<30} {}", mark, c.id, c.name, where_.dimmed());
        }
        println!();
        println!("  {}", "● found on this computer".dimmed());
        println!();
        return Ok(());
    }
    let chosen: Vec<&connect::Client> = if !o.clients.is_empty() {
        all.iter()
            .filter(|c| o.clients.iter().any(|id| id == c.id))
            .collect()
    } else if o.all {
        all.iter().collect()
    } else {
        all.iter().filter(|c| connect::detected(c)).collect()
    };
    if chosen.is_empty() {
        bail!(
            "No AI apps found on this computer. Name one (stockwise connect --list shows them), or use --all."
        );
    }
    let launch = connect::Launch::this(o.writes, o.portfolio)?;
    let outcomes: Vec<connect::Outcome> = chosen
        .iter()
        .map(|c| connect::apply(c, &launch, o.remove, o.skill, o.dry_run))
        .collect();
    if json {
        return output::print_json(&serde_json::json!({
            "command": launch.line(),
            "results": outcomes,
        }));
    }
    print_header(if o.remove {
        "Disconnecting StockWise"
    } else {
        "Connecting StockWise"
    });
    for (c, r) in chosen.iter().zip(&outcomes) {
        let mark = match r.status {
            "connected" | "removed" => "✓".green(),
            "manual" => "→".yellow(),
            _ => "✗".red(),
        };
        println!("  {} {:<30} {}", mark, c.name, r.detail.dimmed());
    }
    println!();
    if !o.remove {
        let mode = if o.portfolio {
            "reads, workspace changes and portfolio lots (undo: stockwise undo)"
        } else if o.writes {
            "reads and workspace changes (undo: stockwise undo)"
        } else {
            "read-only (add --allow-writes to let it update your watchlist, alerts, goals and profile)"
        };
        println!("  Access: {}", mode);
        if launch.command.contains("/target/") || launch.command.contains("\\target\\") {
            println!(
                "  {}",
                "Note: this is a build folder copy; `cargo install --path .` (or a release download) and connect again so the apps keep working after a clean."
                    .yellow()
            );
        }
        println!(
            "  Restart each app, then ask it: {}",
            if o.writes {
                "\"set up my StockWise profile\""
            } else {
                "\"how is my portfolio doing?\""
            }
        );
        println!();
    }
    if outcomes.iter().any(|r| r.status == "failed") {
        bail!("Some apps could not be connected (see above)");
    }
    Ok(())
}
