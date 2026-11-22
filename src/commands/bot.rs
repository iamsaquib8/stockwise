//! `stockwise telegram`: a two-way Telegram bot (#265). It answers `/hot`,
//! `/today`, `/review` and `/digest` from the configured chat only, and sends
//! the digest every trading day at `STOCKWISE_DIGEST_TIME` (local time) if
//! the worker has not sent it already.
//!
//! It long-polls Telegram's `getUpdates` with the same bot token and chat id
//! as the notifier; nothing listens on a port. Messages from any other chat
//! are ignored.

use super::*;
use crate::notify::Notifier;
use serde::Deserialize;
use std::time::Duration;

const STATE_FILE: &str = "bot_state.json";
/// Telegram's limit is 4096; leave room.
const CHUNK: usize = 3900;
const HELP: &str = "StockWise bot — commands:\n\
/today — what moved in your portfolio and why\n\
/hot — your hottest names, then the market\n\
/review — holdings with P&L, weight and monitor signals\n\
/digest — today's digest now\n\
/help — this list\n\n\
Data and rule-based signals for education, not investment advice.";

#[derive(Debug, Default, Serialize, Deserialize)]
struct BotState {
    /// Last update id handled.
    offset: i64,
}

/// A bot command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BotCmd {
    Today,
    Hot,
    Review,
    Digest,
    Help,
}

/// The command in a message: `/hot`, `/hot@MyBot`, `hot`. Pure.
pub fn route(text: &str) -> Option<BotCmd> {
    let word = text.split_whitespace().next()?;
    let word = word.trim_start_matches('/');
    let word = word.split('@').next().unwrap_or(word).to_ascii_lowercase();
    Some(match word.as_str() {
        "today" | "pulse" => BotCmd::Today,
        "hot" => BotCmd::Hot,
        "review" | "portfolio" => BotCmd::Review,
        "digest" => BotCmd::Digest,
        "start" | "help" => BotCmd::Help,
        _ => return None,
    })
}

/// One incoming message: `(update_id, chat_id, text)`.
pub fn parse_updates(v: &serde_json::Value) -> Vec<(i64, String, String)> {
    v.get("result")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|u| {
                    let id = u.get("update_id")?.as_i64()?;
                    let msg = u.get("message").or_else(|| u.get("edited_message"));
                    let chat = msg
                        .and_then(|m| m.get("chat"))
                        .and_then(|c| c.get("id"))
                        .map(|c| c.to_string().trim_matches('"').to_string())
                        .unwrap_or_default();
                    let text = msg
                        .and_then(|m| m.get("text"))
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    Some((id, chat, text))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Split a long reply on line breaks into Telegram-sized pieces.
pub fn chunks(text: &str, max: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if !cur.is_empty() && cur.chars().count() + line.chars().count() + 1 > max {
            out.push(std::mem::take(&mut cur));
        }
        if line.chars().count() > max {
            let chars: Vec<char> = line.chars().collect();
            for piece in chars.chunks(max) {
                out.push(piece.iter().collect());
            }
            continue;
        }
        if !cur.is_empty() {
            cur.push('\n');
        }
        cur.push_str(line);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// `/review`: each holding's action (EXIT and TRIM first), weight, gain over
/// cost, day move and the monitor's recent signals for it.
fn review_text(p: &crate::pulse::Pulse, portfolio: &Portfolio) -> String {
    let state: crate::monitor::MonitorState =
        crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
    let mut rows: Vec<&crate::pulse::Row> = p.held().collect();
    // Most urgent action first (EXIT, TRIM, ADD, HOLD, no call), then weight.
    let urgency =
        |r: &crate::pulse::Row| r.action.as_ref().map_or(u8::MAX, |a| a.kind.review_rank());
    rows.sort_by(|a, b| {
        urgency(a)
            .cmp(&urgency(b))
            .then_with(|| b.weight.total_cmp(&a.weight))
    });
    let mut out = vec![format!(
        "Holdings review ({} positions){}",
        rows.len(),
        p.value
            .map(|v| format!(
                " — {}{v:.0}",
                market::currency_symbol(Some(&p.base_currency))
            ))
            .unwrap_or_default()
    )];
    for r in rows {
        let gain = portfolio
            .holding(&r.symbol)
            .filter(|h| h.avg_cost > 0.0)
            .and_then(|h| Some((r.price? / h.avg_cost - 1.0) * 100.0));
        let signals: Vec<&str> = state
            .recent
            .iter()
            .rev()
            .filter(|s| s.symbol == r.symbol)
            .take(2)
            .map(|s| s.title.as_str())
            .collect();
        out.push(format!(
            "• {} {:.1}% of portfolio, {} vs cost, day {}{}{}",
            r.symbol,
            r.weight * 100.0,
            gain.map(|g| format!("{g:+.1}%"))
                .unwrap_or_else(|| "—".into()),
            r.change_pct
                .map(|c| format!("{c:+.1}%"))
                .unwrap_or_else(|| "—".into()),
            r.action
                .as_ref()
                .map(|a| format!(" [{}]", super::actions::format_action_line(a)))
                .unwrap_or_default(),
            if signals.is_empty() {
                String::new()
            } else {
                format!(" — {}", signals.join("; "))
            }
        ));
    }
    if p.held().any(|r| r.action.is_some()) {
        out.push(String::new());
        out.push(crate::action::DISCLAIMER.to_string());
    }
    out.join("\n")
}

/// The reply to a command.
async fn answer(cmd: BotCmd, client: &YahooClient, market: Market) -> String {
    let result: Result<String> = async {
        Ok(match cmd {
            BotCmd::Help => HELP.to_string(),
            BotCmd::Today => {
                let p =
                    crate::pulse::gather(client, market, crate::pulse::Options::default()).await?;
                format!("Today\n{}", super::today::plain_today(&p, 8))
            }
            BotCmd::Hot => {
                let r = super::hot::build_hot(client, market, None, None, 5, true).await?;
                format!("What's hot\n{}", super::hot::plain_hot(&r, 5))
            }
            BotCmd::Review => {
                let p = crate::pulse::gather(
                    client,
                    market,
                    crate::pulse::Options {
                        news: false,
                        watchlist: false,
                        ..Default::default()
                    },
                )
                .await?;
                review_text(&p, &Portfolio::load()?)
            }
            BotCmd::Digest => {
                let mut st: crate::monitor::MonitorState =
                    crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
                let (d, _) =
                    super::monitoring::build_digest(client, market, Vec::new(), &mut st).await?;
                let m = crate::digest::render(&d);
                format!("{}\n{}", m.title, m.body)
            }
        })
    }
    .await;
    result.unwrap_or_else(|e| format!("Sorry — that failed: {e:#}"))
}

/// `HH:MM` local time to send the digest, from `STOCKWISE_DIGEST_TIME`.
fn digest_time() -> Option<chrono::NaiveTime> {
    crate::config::notify_setting("STOCKWISE_DIGEST_TIME")
        .and_then(|t| chrono::NaiveTime::parse_from_str(t.trim(), "%H:%M").ok())
}

pub async fn cmd_telegram_bot(once: bool, market: Market) -> Result<()> {
    let notifier = Notifier::from_env();
    let Some(tg) = notifier.config().telegram.clone() else {
        bail!(
            "The bot needs Telegram: set STOCKWISE_TELEGRAM_TOKEN and STOCKWISE_TELEGRAM_CHAT_ID (see `stockwise notify`)."
        );
    };
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(70))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let base = format!("{}/bot{}", tg.api_base.trim_end_matches('/'), tg.token);
    let client = YahooClient::new().await?;
    let mut state: BotState = crate::store::load(STATE_FILE).unwrap_or_default();
    let send = |text: String| {
        let http = http.clone();
        let url = format!("{base}/sendMessage");
        let chat = tg.chat_id.clone();
        async move {
            for piece in chunks(&text, CHUNK) {
                let body = serde_json::json!({
                    "chat_id": chat,
                    "text": piece,
                    "disable_web_page_preview": true,
                });
                if let Err(e) = http.post(&url).json(&body).send().await {
                    eprintln!("  ! Telegram send failed: {}", e.without_url());
                }
            }
        }
    };
    let mut shutdown = crate::daemon::Shutdown::install("The bot stops after the current reply.");
    if !once {
        print_header("Telegram bot");
        println!(
            "  Answering /today, /hot, /review and /digest from chat {}.",
            tg.chat_id
        );
        match digest_time() {
            Some(t) => println!(
                "  Digest every trading day at {} (local time).",
                t.format("%H:%M")
            ),
            None => println!(
                "  {} Set STOCKWISE_DIGEST_TIME=16:15 for a scheduled digest.",
                "→".dimmed()
            ),
        }
        println!("  {} Press Ctrl+C to stop.\n", "→".dimmed());
    }
    let mut ignored_note = false;
    loop {
        if shutdown.requested() {
            break;
        }
        // Scheduled digest, once per closed session date (shared with the worker).
        if let Some(t) = digest_time() {
            let now = chrono::Local::now();
            let utc = chrono::Utc::now();
            let sess = crate::clock::session_date(market, utc);
            let session_closed = crate::clock::close_at(market, sess).is_some_and(|c| utc >= c);
            if now.time() >= t && crate::clock::is_trading_day(market, sess) && session_closed {
                let mut st: crate::monitor::MonitorState =
                    crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
                let date = sess.to_string();
                if st.last_digest.as_deref() != Some(date.as_str())
                    && let Ok((d, signals)) =
                        super::monitoring::build_digest(&client, market, Vec::new(), &mut st).await
                {
                    let _ = super::monitoring::send_digest_once_with_cursor(
                        &notifier, &d, &signals, st.cursor,
                    )
                    .await;
                }
            }
        }
        let url = format!(
            "{base}/getUpdates?timeout={}&offset={}",
            if once { 0 } else { 50 },
            state.offset + 1
        );
        let polled = tokio::select! {
            r = http.get(&url).send() => Some(r),
            _ = shutdown.wait() => None,
        };
        let Some(polled) = polled else { break };
        let updates: serde_json::Value = match polled {
            Ok(resp) => {
                if !resp.status().is_success() {
                    let status = resp.status();
                    if once {
                        bail!("Telegram getUpdates failed with HTTP {status}");
                    }
                    eprintln!("  ! Telegram poll HTTP {status} — retrying");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
                match resp.json::<serde_json::Value>().await {
                    Ok(v) if v.get("ok").and_then(|b| b.as_bool()) == Some(true) => v,
                    Ok(v) => {
                        let desc = v
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or("unknown Telegram error");
                        if once {
                            bail!("Telegram getUpdates rejected: {desc}");
                        }
                        eprintln!("  ! Telegram poll rejected: {desc} — retrying");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                    Err(e) => {
                        if once {
                            bail!("Telegram getUpdates invalid JSON: {e}");
                        }
                        eprintln!("  ! Telegram poll parse failed: {e} — retrying");
                        tokio::time::sleep(Duration::from_secs(10)).await;
                        continue;
                    }
                }
            }
            Err(e) => {
                if once {
                    bail!("Telegram poll failed: {}", e.without_url());
                }
                eprintln!("  ! Telegram poll failed: {} — retrying", e.without_url());
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
        };
        for (id, chat, text) in parse_updates(&updates) {
            state.offset = state.offset.max(id);
            if chat != tg.chat_id {
                if !ignored_note {
                    eprintln!(
                        "  · Ignoring messages from chats other than {}.",
                        tg.chat_id
                    );
                    ignored_note = true;
                }
                continue;
            }
            let Some(cmd) = route(&text) else {
                send(HELP.to_string()).await;
                continue;
            };
            println!(
                "  {} {:?}",
                chrono::Local::now().format("%H:%M:%S").to_string().dimmed(),
                cmd
            );
            send(answer(cmd, &client, market).await).await;
        }
        crate::store::save(STATE_FILE, &state, false)?;
        if once {
            break;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commands_route_with_or_without_the_slash_and_bot_name() {
        assert_eq!(route("/hot"), Some(BotCmd::Hot));
        assert_eq!(route("/today@StockWiseBot please"), Some(BotCmd::Today));
        assert_eq!(route("review"), Some(BotCmd::Review));
        assert_eq!(route("/start"), Some(BotCmd::Help));
        assert_eq!(route("buy RELIANCE"), None);
        assert_eq!(route(""), None);
    }

    #[test]
    fn updates_parse_chat_and_text() {
        let v = serde_json::json!({"ok": true, "result": [
            {"update_id": 10, "message": {"chat": {"id": 42}, "text": "/hot"}},
            {"update_id": 11, "message": {"chat": {"id": -100123}, "text": "/today"}},
            {"update_id": 12, "callback_query": {}}
        ]});
        let u = parse_updates(&v);
        assert_eq!(u[0], (10, "42".to_string(), "/hot".to_string()));
        assert_eq!(u[1].1, "-100123");
        assert_eq!(u[2], (12, String::new(), String::new()));
    }

    #[test]
    fn long_replies_split_on_lines() {
        let text = (0..10)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let c = chunks(&text, 20);
        assert!(c.iter().all(|p| p.chars().count() <= 20), "{c:?}");
        assert_eq!(c.join("\n"), text);
        assert_eq!(chunks(&"x".repeat(45), 20).len(), 3);
    }
}
