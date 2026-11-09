//! `stockwise init`: the first-run path. Pick a market, import holdings,
//! optionally show the MCP config, then print three next commands.
//!
//! Every step has a flag. Without a TTY on stdin nothing is ever prompted:
//! a step without its flag is skipped. Broker credentials are never asked.

use super::*;
use crate::portfolio::Portfolio;
use std::io::{BufRead, IsTerminal, Write};

const IMPORT_SOURCES: &[&str] = &["kite", "groww", "upstox", "indmoney", "cas"];

const MCP_SNIPPET: &str = r#"{
  "mcpServers": {
    "stockwise": { "command": "stockwise", "args": ["mcp"] }
  }
}"#;

/// Split `--import SOURCE:FILE` at the first colon. A Windows drive path such
/// as `groww:C:\h.csv` keeps its own colon because only the first one splits.
pub fn parse_import_spec(spec: &str) -> Result<(&str, &str)> {
    let (source, file) = spec
        .split_once(':')
        .filter(|(s, f)| !s.is_empty() && !f.is_empty())
        .with_context(|| {
            format!("Use --import SOURCE:FILE, e.g. groww:holdings.csv (got `{spec}`)")
        })?;
    if !IMPORT_SOURCES.contains(&source) {
        bail!(
            "Unknown import source `{source}` — choose one of: {}",
            IMPORT_SOURCES.join(", ")
        );
    }
    Ok((source, file))
}

fn ask(question: &str, default: &str) -> Result<String> {
    print!("  {} [{}] ", question, default);
    std::io::stdout().flush()?;
    let mut line = String::new();
    std::io::stdin().lock().read_line(&mut line)?;
    let line = line.trim();
    Ok(if line.is_empty() { default } else { line }.to_string())
}

fn ask_yes(question: &str, default_yes: bool) -> Result<bool> {
    let answer = ask(question, if default_yes { "Y/n" } else { "y/N" })?;
    Ok(match answer.to_ascii_lowercase().as_str() {
        "y/n" => default_yes,
        "n/y" => default_yes,
        a => a.starts_with('y'),
    })
}

/// Holdings as a plain table from local data, so it needs no network.
fn print_holdings() -> Result<()> {
    let p = Portfolio::load()?;
    if p.holdings.is_empty() {
        println!("  {}", "No holdings yet.".dimmed());
        return Ok(());
    }
    println!(
        "\n  {:<18} {:>12} {:>14}",
        "Symbol".bold(),
        "Shares".bold(),
        "Avg cost".bold()
    );
    for h in &p.holdings {
        println!("  {:<18} {:>12.2} {:>14.2}", h.symbol, h.shares, h.avg_cost);
    }
    println!(
        "\n  {} holdings. `stockwise portfolio` adds live prices and P&L.",
        p.holdings.len()
    );
    Ok(())
}

/// Parse an `init` profile response: either a 1-keystroke number (`1`..`5`)
/// or a `<MODE[:RISK]>` string (`passive`, `active:low`, `active:med`, `active:high`, `ultra`).
pub fn parse_init_profile(input: &str) -> Result<crate::config::ProfileSpec> {
    let trimmed = input.trim();
    let spec_str = match trimmed {
        "1" => "passive",
        "2" => "active:low",
        "3" => "active:med",
        "4" => "active:high",
        "5" => "ultra",
        other => other,
    };
    crate::config::parse_profile_spec(spec_str).map_err(|e| anyhow::anyhow!("{e}"))
}

pub async fn cmd_init(
    market: Market,
    market_given: bool,
    profile_given: Option<crate::config::ProfileSpec>,
    imports: &[String],
    no_mcp: bool,
) -> Result<()> {
    let interactive = std::io::stdin().is_terminal();
    print_header("StockWise setup");

    // 1. Default market.
    let mut market = market;
    if !market_given && interactive {
        let answer = ask("Default market — in (India, NSE) or us?", "in")?;
        market = match answer.to_ascii_lowercase().as_str() {
            "us" => Market::Us,
            "in" => Market::In,
            other => bail!("Choose `in` or `us` (got `{other}`)"),
        };
    }
    if market_given || interactive {
        let value = crate::value_name(market);
        let path = crate::config::path().context(
            "Cannot determine the config directory — set STOCKWISE_CONFIG to a file path",
        )?;
        crate::config::set_market(&path, &value)?;
        println!(
            "  {} Default market set to {} in {}",
            "✓".green(),
            value.cyan(),
            path.display()
        );
    }

    // 2. Investor profile (1-keystroke or --profile flag).
    let mut chosen_profile = profile_given;
    if chosen_profile.is_none() && interactive {
        let answer = ask(
            "Investor profile — 1:passive  2:active:low  3:active:med  4:active:high  5:ultra?",
            "3",
        )?;
        chosen_profile = Some(parse_init_profile(&answer)?);
    }
    if let Some(spec) = chosen_profile {
        let path = crate::config::path().context(
            "Cannot determine the config directory — set STOCKWISE_CONFIG to a file path",
        )?;
        crate::config::set_profile(
            &path,
            &crate::config::ProfileUpdate {
                mode: Some(spec.mode),
                risk: Some(spec.effective_risk()),
                ..Default::default()
            },
        )?;
        println!(
            "  {} Investor profile set to {} in {}",
            "✓".green(),
            spec.canonical().cyan(),
            path.display()
        );
    }

    // 2. Holdings.
    let mut specs: Vec<(String, String)> = Vec::new();
    for spec in imports {
        let (s, f) = parse_import_spec(spec)?;
        specs.push((s.to_string(), f.to_string()));
    }
    if specs.is_empty() && interactive && ask_yes("Import holdings now?", false)? {
        let source = ask(&format!("Source ({})?", IMPORT_SOURCES.join(", ")), "groww")?;
        parse_import_spec(&format!("{source}:x"))?;
        let file = ask("Path to the exported file?", "")?;
        if file.is_empty() {
            bail!("No file given — run `stockwise import {source} <file>` later");
        }
        specs.push((source, file));
    }
    for (source, file) in &specs {
        cmd_import(source, file, false, None, market).await?;
    }
    if !specs.is_empty() {
        print_holdings()?;
    }

    // 3. MCP.
    if !no_mcp && interactive && ask_yes("Use StockWise from Claude or Cursor?", false)? {
        println!("\n  Add this to Claude Desktop or `~/.cursor/mcp.json`:\n\n{MCP_SNIPPET}\n");
    }

    // 4. Next steps.
    println!("\n  {}", "Next:".bold());
    for (cmd, what) in [
        ("stockwise harvest", "tax-loss harvesting candidates"),
        ("stockwise tax in", "India capital-gains report"),
        ("stockwise mf search <name>", "find a mutual fund"),
    ] {
        println!("    {:<28} {}", cmd.cyan().bold(), what);
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_spec_splits_at_the_first_colon() {
        assert_eq!(
            parse_import_spec("groww:h.csv").unwrap(),
            ("groww", "h.csv")
        );
        assert_eq!(
            parse_import_spec("kite:C:\\h.csv").unwrap(),
            ("kite", "C:\\h.csv")
        );
    }

    #[test]
    fn import_spec_rejects_bad_input() {
        assert!(parse_import_spec("groww").is_err());
        assert!(parse_import_spec(":file").is_err());
        assert!(parse_import_spec("groww:").is_err());
        let e = parse_import_spec("robinhood:f.csv")
            .unwrap_err()
            .to_string();
        assert!(e.contains("Unknown import source"), "{e}");
    }

    #[test]
    fn parse_init_profile_accepts_keystrokes_and_names() {
        assert_eq!(parse_init_profile("1").unwrap().canonical(), "passive:low");
        assert_eq!(parse_init_profile("2").unwrap().canonical(), "active:low");
        assert_eq!(parse_init_profile("3").unwrap().canonical(), "active:med");
        assert_eq!(parse_init_profile("4").unwrap().canonical(), "active:high");
        assert_eq!(parse_init_profile("5").unwrap().canonical(), "ultra");
        assert_eq!(
            parse_init_profile("passive").unwrap().canonical(),
            "passive:low"
        );
        assert!(parse_init_profile("9").is_err());
    }
}
