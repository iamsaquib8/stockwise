//! Paper-trading simulator.

use super::*;
use crate::clock;
use crate::daemon::PositionStatus;
use crate::simulator::{Bar, replay_trade};
use chrono::{DateTime, NaiveDate, Utc};

/// The currency symbol of a session's own market; the `-m` market's when
/// the saved id is not one this version knows.
fn session_currency(session: &SimSession, fallback: Market) -> &'static str {
    use clap::ValueEnum;
    Market::from_str(&session.market, true)
        .unwrap_or(fallback)
        .currency_symbol()
}

pub async fn cmd_sim(
    action: &str,
    amount: Option<f64>,
    target: Option<f64>,
    market: Market,
) -> Result<()> {
    let csym = market.currency_symbol();

    match action {
        "start" => {
            market.require_bot_support("sim start")?;
            let capital = amount.unwrap_or(25000.0);
            let target_pct = target.unwrap_or(2.0);
            let now = chrono::Utc::now();
            let session_date = clock::session_date(market, now);
            let today = session_date.format("%Y-%m-%d").to_string();
            // A session belongs to one trading day; `sim settle` replays that
            // day's bars from the moment of entry.
            if !clock::is_trading_day(market, session_date) {
                bail!(
                    "{} is closed today ({}). Start a session on a trading day.",
                    clock::exchange(market),
                    clock::closed_reason(market, now)
                );
            }
            if clock::close_at(market, session_date).is_some_and(|close| now >= close) {
                bail!(
                    "{} has closed for today. Start a session before or during the next one ({}).",
                    clock::exchange(market),
                    clock::next_open(market, now)
                        .map(|t| clock::format_local(market, t))
                        .unwrap_or_default()
                );
            }

            let mut history = SimHistory::load()?;
            // One open session per market and day: an NSE session does not
            // block a NYSE one.
            if history.has_open_session(market.id(), &today) {
                println!(
                    "  {} Already have an open {} simulation for today.",
                    "!".yellow(),
                    market.label()
                );
                println!(
                    "  Run {} to settle it, or {} to check status.",
                    "stockwise sim settle".cyan(),
                    "stockwise sim status".cyan()
                );
                return Ok(());
            }

            print_header("Paper Trading Simulation — Starting");
            println!(
                "  Date: {}  |  Capital: {}{:.0}  |  Target: {}%\n",
                today.bold(),
                csym,
                capital,
                target_pct
            );
            println!("  {} Running intraday bot scan...\n", "⟳".yellow());

            let client = YahooClient::new().await?;
            let signals = intraday::scan_intraday(&client, market).await?;
            let plans = intraday::generate_trade_plans(&signals, capital, target_pct, 1.0);

            if plans.is_empty() {
                println!(
                    "  {} No trades qualified today. Try tomorrow.",
                    "→".yellow()
                );
                return Ok(());
            }

            let sim_trades: Vec<SimTrade> = plans
                .iter()
                .take(5)
                .map(|p| SimTrade {
                    symbol: p.signal.symbol.clone(),
                    direction: p.signal.direction.to_string(),
                    entry_price: p.entry,
                    target1: p.target1,
                    target2: p.target2,
                    stop_loss: p.stop_loss,
                    qty: p.qty,
                    capital: p.capital_required,
                    score: p.signal.score,
                    confidence: p.signal.confidence.to_string(),
                    strategies: p
                        .signal
                        .strategies
                        .iter()
                        .map(|s| s.name.to_string())
                        .collect(),
                    entered_at: Some(now.to_rfc3339()),
                    exit_price: None,
                    pnl: None,
                    pnl_pct: None,
                    hit_target: None,
                    hit_stop: None,
                })
                .collect();

            println!("  {} Simulation trades locked in:\n", "✓".green().bold());
            println!(
                "  {:<14} {:>6} {:>10} {:>10} {:>10} {:>6}",
                "Symbol".bold(),
                "Qty".bold(),
                "Entry".bold(),
                "Target".bold(),
                "Stop".bold(),
                "Score".bold()
            );
            println!("  {}", "─".repeat(60).dimmed());
            for t in &sim_trades {
                println!(
                    "  {:<14} {:>6} {:>10} {:>10} {:>10} {:>6}",
                    t.symbol.cyan(),
                    t.qty,
                    format!("{}{:.2}", csym, t.entry_price),
                    format!("{}{:.2}", csym, t.target2).green(),
                    format!("{}{:.2}", csym, t.stop_loss).red(),
                    format!("{:.0}", t.score)
                );
            }

            history.sessions.push(SimSession {
                date: today,
                market: market.id().into(),
                capital,
                target_pct,
                trades: sim_trades,
                total_pnl: None,
                total_pnl_pct: None,
                win_count: None,
                loss_count: None,
                settled: false,
            });
            history.save()?;

            println!(
                "\n  {} Trades recorded. Run {} at EOD to see results.",
                "✓".green(),
                "stockwise sim settle".cyan()
            );
            println!(
                "  {} Run {} anytime to check live P&L.",
                "→".dimmed(),
                "stockwise sim status".cyan()
            );
            println!();
        }
        "status" => {
            let history = SimHistory::load()?;
            let open: Vec<&SimSession> = history.sessions.iter().filter(|s| !s.settled).collect();
            if open.is_empty() {
                println!(
                    "\n  {} No open simulations. Run {}.",
                    "→".dimmed(),
                    "stockwise sim start".cyan()
                );
                return Ok(());
            }

            let client = YahooClient::new().await?;
            for session in &open {
                let csym = session_currency(session, market);
                print_header(&format!("Simulation Status — {}", session.date));
                println!(
                    "  Capital: {}{:.0}  |  Target: {}%\n",
                    csym, session.capital, session.target_pct
                );
                let syms: Vec<&str> = session.trades.iter().map(|t| t.symbol.as_str()).collect();
                let quotes = client.get_quote(&syms).await?;

                println!(
                    "  {:<14} {:>6} {:>10} {:>10} {:>12} {:>10}",
                    "Symbol".bold(),
                    "Qty".bold(),
                    "Entry".bold(),
                    "Now".bold(),
                    "P&L".bold(),
                    "Status".bold()
                );
                println!("  {}", "─".repeat(66).dimmed());

                let mut total_pnl = 0.0_f64;
                for t in &session.trades {
                    let current = quotes
                        .iter()
                        .find(|q| q.symbol.as_deref() == Some(&t.symbol))
                        .and_then(|q| q.regular_market_price)
                        .unwrap_or(t.entry_price);
                    let is_long = t.direction == "BUY";
                    let pnl = if is_long {
                        (current - t.entry_price) * t.qty as f64
                    } else {
                        (t.entry_price - current) * t.qty as f64
                    };
                    total_pnl += pnl;
                    let hit_t2 = if is_long {
                        current >= t.target2
                    } else {
                        current <= t.target2
                    };
                    let hit_stop = if is_long {
                        current <= t.stop_loss
                    } else {
                        current >= t.stop_loss
                    };
                    let hit_t1 = if is_long {
                        current >= t.target1
                    } else {
                        current <= t.target1
                    };
                    let status = if hit_t2 {
                        "TARGET HIT".green().bold().to_string()
                    } else if hit_stop {
                        "STOPPED".red().bold().to_string()
                    } else if hit_t1 {
                        "T1 hit".green().to_string()
                    } else {
                        "Open".yellow().to_string()
                    };
                    let pnl_str = if pnl >= 0.0 {
                        format!("+{}{:.0}", csym, pnl).green().to_string()
                    } else {
                        format!("-{}{:.0}", csym, pnl.abs()).red().to_string()
                    };
                    println!(
                        "  {:<14} {:>6} {:>10} {:>10} {:>12} {:>10}",
                        t.symbol.cyan(),
                        t.qty,
                        format!("{}{:.2}", csym, t.entry_price),
                        format!("{}{:.2}", csym, current),
                        pnl_str,
                        status
                    );
                }
                println!("  {}", "─".repeat(66).dimmed());
                let total_str = if total_pnl >= 0.0 {
                    format!("+{}{:.0}", csym, total_pnl)
                        .green()
                        .bold()
                        .to_string()
                } else {
                    format!("-{}{:.0}", csym, total_pnl.abs())
                        .red()
                        .bold()
                        .to_string()
                };
                let target_profit = session.capital * (session.target_pct / 100.0);
                let on_track = if total_pnl >= target_profit {
                    "TARGET MET".green().bold().to_string()
                } else {
                    format!("{}{:.0} to go", csym, target_profit - total_pnl)
                        .yellow()
                        .to_string()
                };
                println!("  Total: {}  |  {}", total_str, on_track);
            }
            println!();
        }
        "settle" => return cmd_sim_settle(false).await,
        "history" | "stats" => return cmd_sim_history(market, Format::Table),
        "reset" => return cmd_sim_reset(false),
        _ => {
            println!();
            println!("  {} Paper Trading Simulator:", "→".cyan());
            println!();
            println!(
                "    {} — Lock in today's bot trades",
                "stockwise sim start [AMOUNT] [TARGET%]".bold()
            );
            println!("    {} — Check live P&L", "stockwise sim status".bold());
            println!(
                "    {} — Settle from the day's 5-minute bars (after the close)",
                "stockwise sim settle".bold()
            );
            println!(
                "    {} — View cumulative stats + confidence",
                "stockwise sim history".bold()
            );
            println!("    {} — Clear all data", "stockwise sim reset".bold());
            println!();
            println!("  {} Practice:", "→".cyan());
            println!();
            println!(
                "    {} — Trade a past market day by day, the future hidden",
                "stockwise sim replay start covid-crash-2020".bold()
            );
            println!(
                "    {} — Goal-based challenges (replay or live sessions)",
                "stockwise sim challenge list".bold()
            );
            println!(
                "    {} — Badges, streaks, best scores",
                "stockwise sim profile".bold()
            );
            println!(
                "    {} — Share your last result",
                "stockwise sim card --svg card.svg".bold()
            );
            println!();
            println!(
                "  {} Morning: {} → Track: {} → EOD: {}",
                "Workflow:".dimmed(),
                "sim start".cyan(),
                "sim status".cyan(),
                "sim settle".cyan()
            );
            println!();
        }
    }
    Ok(())
}

/// The bot's confidence rating from settled sessions: `high`, `moderate` or
/// `low`; `None` until there are 5 days of history.
fn sim_confidence(stats: &crate::simulator::SimStats) -> Option<&'static str> {
    if stats.total_days < 5 {
        None
    } else if stats.day_win_rate >= 65.0 && stats.trade_win_rate >= 55.0 && stats.total_pnl > 0.0 {
        Some("high")
    } else if stats.day_win_rate >= 50.0 && stats.total_pnl > 0.0 {
        Some("moderate")
    } else {
        Some("low")
    }
}

/// `sim history --json`: statistics over settled sessions, the confidence
/// rating, and every session (open ones have null P&L).
pub(crate) fn sim_history_json(
    history: &SimHistory,
    market: Market,
    stats: &crate::simulator::SimStats,
) -> serde_json::Value {
    let sessions: Vec<serde_json::Value> = history
        .in_market(market.id())
        .map(|s| {
            serde_json::json!({
                "date": s.date,
                "market": s.market,
                "capital": s.capital,
                "target_percent": s.target_pct,
                "settled": s.settled,
                "trades": s.trades.len(),
                "pnl": round_opt(s.total_pnl, 2),
                "pnl_percent": round_opt(s.total_pnl_pct, 2),
                "wins": s.win_count,
                "losses": s.loss_count,
            })
        })
        .collect();
    serde_json::json!({
        "stats": {
            "days": stats.total_days,
            "winning_days": stats.winning_days,
            "day_win_rate_percent": round(stats.day_win_rate, 2),
            "total_pnl": round(stats.total_pnl, 2),
            "avg_daily_pnl": round(stats.avg_daily_pnl, 2),
            "best_day": round_opt(stats.best_day, 2),
            "worst_day": round_opt(stats.worst_day, 2),
            "trades": stats.total_trades,
            "winning_trades": stats.winning_trades,
            "losing_trades": stats.losing_trades,
            "trade_win_rate_percent": round(stats.trade_win_rate, 2),
            "sharpe": round_opt(stats.sharpe, 3),
            "max_win_streak": stats.max_win_streak,
            "max_loss_streak": stats.max_loss_streak,
        },
        "confidence": sim_confidence(stats),
        "sessions": sessions,
    })
}

/// `sim history`: past sessions and the bot's confidence rating.
pub fn cmd_sim_history(market: Market, fmt: Format) -> Result<()> {
    let csym = market.currency_symbol();
    let history = SimHistory::load()?;
    let stats = history.stats(market.id());
    if fmt == Format::Json {
        return output::print_json(&sim_history_json(&history, market, &stats));
    }
    if stats.total_days == 0 {
        println!(
            "\n  {} No history. Run {}.",
            "→".dimmed(),
            "stockwise sim start".cyan()
        );
        return Ok(());
    }

    print_header(&format!("Paper Trading Performance — {}", market.label()));
    let daily_pnls: Vec<f64> = history
        .in_market(market.id())
        .filter(|s| s.settled)
        .filter_map(|s| s.total_pnl)
        .collect();
    if daily_pnls.len() > 1 {
        let mut equity = vec![0.0_f64];
        for &pnl in &daily_pnls {
            equity.push(equity.last().unwrap() + pnl);
        }
        let color = if *equity.last().unwrap() >= 0.0 {
            "green"
        } else {
            "red"
        };
        for line in charts::line_chart(&equity, 50, 8, color, "Cumulative P&L") {
            println!("{}", line);
        }
        println!();
        println!("  {}", "Daily P&L".bold());
        for s in history.in_market(market.id()).filter(|s| s.settled) {
            let pnl = s.total_pnl.unwrap_or(0.0);
            let bar_len = (pnl.abs()
                / stats
                    .best_day
                    .unwrap_or(1.0)
                    .abs()
                    .max(stats.worst_day.unwrap_or(1.0).abs())
                * 20.0)
                .min(20.0) as usize;
            let bar = if pnl >= 0.0 {
                "█".repeat(bar_len.max(1)).green().to_string()
            } else {
                "█".repeat(bar_len.max(1)).red().to_string()
            };
            let ps = if pnl >= 0.0 {
                format!("+{}{:.0}", csym, pnl).green().to_string()
            } else {
                format!("-{}{:.0}", csym, pnl.abs()).red().to_string()
            };
            println!("  {} {} {}", s.date.dimmed(), bar, ps);
        }
    }

    print_section("Performance");
    print_kv("Days Simulated", &stats.total_days.to_string());
    print_kv(
        "Winning Days",
        &format!("{} ({:.0}%)", stats.winning_days, stats.day_win_rate),
    );
    let ts = if stats.total_pnl >= 0.0 {
        format!("+{}{:.0}", csym, stats.total_pnl)
            .green()
            .bold()
            .to_string()
    } else {
        format!("-{}{:.0}", csym, stats.total_pnl.abs())
            .red()
            .bold()
            .to_string()
    };
    print_kv("Total P&L", &ts);
    print_kv(
        "Avg Daily P&L",
        &format!("{}{:.0}", csym, stats.avg_daily_pnl),
    );
    if let Some(b) = stats.best_day {
        print_kv(
            "Best Day",
            &format!("+{}{:.0}", csym, b).green().to_string(),
        );
    }
    if let Some(w) = stats.worst_day {
        print_kv("Worst Day", &format!("{}{:.0}", csym, w).red().to_string());
    }
    print_kv("Total Trades", &stats.total_trades.to_string());
    print_kv("Trade Win Rate", &format!("{:.0}%", stats.trade_win_rate));
    print_kv("Max Win Streak", &format!("{} days", stats.max_win_streak));
    print_kv(
        "Max Loss Streak",
        &format!("{} days", stats.max_loss_streak),
    );
    if let Some(s) = stats.sharpe {
        print_kv("Sharpe (ann.)", &format!("{:.2}", s));
    }

    print_section("Bot Confidence");
    if let Some(level) = sim_confidence(&stats) {
        let conf = match level {
            "high" => "HIGH — Bot is consistently profitable. Consider going live."
                .green()
                .bold()
                .to_string(),
            "moderate" => "MODERATE — Profitable but inconsistent. Keep testing."
                .yellow()
                .to_string(),
            _ => "LOW — Underperforming. Review strategy.".red().to_string(),
        };
        println!("  {}", conf);
    } else {
        println!(
            "  {} Need 5+ days for confidence rating ({}/5)",
            "→".dimmed(),
            stats.total_days
        );
    }
    println!();
    Ok(())
}

fn session_market(code: &str) -> Market {
    <Market as clap::ValueEnum>::from_str(code, true).unwrap_or(Market::In)
}

/// Bars of one session, finest first: 5-minute bars (Yahoo keeps ~60
/// days), then hourly (~2 years), then the daily bar. Returns the bars, the
/// bar length and a label, or None if the exchange has no bars for the day.
async fn session_bars(
    client: &YahooClient,
    symbol: &str,
    market: Market,
    date: NaiveDate,
) -> Result<Option<(Vec<Bar>, chrono::Duration, &'static str)>> {
    let start = clock::at(market, date, chrono::NaiveTime::MIN).timestamp();
    let end = start + 24 * 3600;
    let mut last_err = None;
    let mut answered = false;
    for (interval, len, label) in [
        ("5m", chrono::Duration::minutes(5), "5-minute bars"),
        ("60m", chrono::Duration::hours(1), "hourly bars"),
        ("1d", chrono::Duration::days(1), "the daily bar"),
    ] {
        match client.get_chart_between(symbol, start, end, interval).await {
            Ok(chart) => {
                answered = true;
                let bars = chart_bars(&chart, market, date);
                if !bars.is_empty() {
                    return Ok(Some((bars, len, label)));
                }
            }
            Err(e) => last_err = Some(e),
        }
    }
    match last_err {
        Some(e) if !answered => Err(e),
        _ => Ok(None),
    }
}

/// OHLC bars from a chart response that belong to `date`, skipping bars
/// with a missing value (never filled with 0).
fn chart_bars(chart: &crate::api::ChartData, market: Market, date: NaiveDate) -> Vec<Bar> {
    let Some(ts) = &chart.timestamp else {
        return Vec::new();
    };
    let Some(q) = chart.indicators.quote.first() else {
        return Vec::new();
    };
    let col = |c: &Option<Vec<Option<f64>>>, i: usize| {
        c.as_ref().and_then(|v| v.get(i).copied().flatten())
    };
    ts.iter()
        .enumerate()
        .filter_map(|(i, &t)| {
            let time = DateTime::from_timestamp(t, 0)?;
            Some(Bar {
                time,
                open: col(&q.open, i)?,
                high: col(&q.high, i)?,
                low: col(&q.low, i)?,
                close: col(&q.close, i)?,
            })
        })
        .filter(|b| clock::session_date(market, b.time) == date)
        .collect()
}

/// `sim settle`: close every open session from its own day's intraday bars —
/// did the stop or the target hit first? — instead of today's price.
/// Today's session is refused until the market closes, unless `now`.
pub async fn cmd_sim_settle(now_flag: bool) -> Result<()> {
    let mut history = SimHistory::load()?;
    if history.sessions.iter().all(|s| s.settled) {
        println!("\n  {} No open simulations.\n", "→".dimmed());
        return Ok(());
    }
    let now = Utc::now();
    let mut client: Option<YahooClient> = None;
    let mut settled_any = false;
    let mut skipped: Vec<String> = Vec::new();

    for session in history.sessions.iter_mut().filter(|s| !s.settled) {
        let market = session_market(&session.market);
        let csym = market.currency_symbol();
        let Ok(date) = NaiveDate::parse_from_str(&session.date, "%Y-%m-%d") else {
            skipped.push(format!("{}: not a valid session date", session.date));
            continue;
        };
        if let Some(close) = clock::close_at(market, date)
            && now < close
            && !now_flag
        {
            skipped.push(format!(
                "{}: the {} session hasn't closed yet — settle after {}, or pass --now to settle at the latest bar",
                session.date,
                clock::exchange(market),
                clock::format_local(market, close)
            ));
            continue;
        }
        if client.is_none() {
            client = Some(YahooClient::new().await?);
        }
        let client = client.as_ref().expect("client was just created");

        // Replay every trade before touching the session, so a missing
        // chart leaves it open instead of half-settled.
        let mut results = Vec::new();
        let mut problem = None;
        for trade in &session.trades {
            let entered_at = trade
                .entered_at
                .as_deref()
                .and_then(|t| DateTime::parse_from_rfc3339(t).ok())
                .map(|t| t.with_timezone(&Utc));
            match session_bars(client, &trade.symbol, market, date).await {
                Ok(Some((bars, len, label))) => {
                    match replay_trade(trade, &bars, len, market, entered_at) {
                        Some(r) => results.push((r, label)),
                        None => {
                            problem = Some(format!("no bars for {} after the entry", trade.symbol));
                            break;
                        }
                    }
                }
                Ok(None) => {
                    problem = Some(format!(
                        "no price bars for {} on that day{}",
                        trade.symbol,
                        clock::holiday(market, date)
                            .map(|h| format!(" ({} holiday: {})", clock::exchange(market), h))
                            .unwrap_or_default()
                    ));
                    break;
                }
                Err(e) => {
                    problem = Some(format!("could not fetch {}: {:#}", trade.symbol, e));
                    break;
                }
            }
        }
        if let Some(p) = problem {
            skipped.push(format!("{}: {} — left open", session.date, p));
            continue;
        }

        print_header(&format!("Settling — {}", session.date));
        println!(
            "  {:<14} {:>10} {:>10} {:>12}  {}",
            "Symbol".bold(),
            "Entry".bold(),
            "Exit".bold(),
            "P&L".bold(),
            "Result".bold()
        );
        println!("  {}", "─".repeat(64).dimmed());
        let mut total_pnl = 0.0_f64;
        let (mut wins, mut losses) = (0u32, 0u32);
        let mut sources: Vec<&str> = Vec::new();
        for (trade, (r, label)) in session.trades.iter_mut().zip(&results) {
            trade.exit_price = Some(r.exit_price);
            trade.pnl = Some(r.pnl);
            trade.pnl_pct = Some(r.pnl_pct);
            trade.hit_target = Some(r.status == PositionStatus::T2Hit);
            trade.hit_stop = Some(r.status == PositionStatus::StopHit);
            total_pnl += r.pnl;
            if r.pnl > 0.0 {
                wins += 1;
            } else {
                losses += 1;
            }
            if !sources.contains(label) {
                sources.push(label);
            }
            let at = clock::local(market, r.exit_time).format("%H:%M");
            let result = match r.status {
                PositionStatus::T2Hit => format!("TARGET {}", at).green().bold().to_string(),
                PositionStatus::StopHit if r.t1_booked => {
                    format!("T1, then stop {}", at).yellow().to_string()
                }
                PositionStatus::StopHit => format!("STOPPED {}", at).red().to_string(),
                _ => format!("Exit {}", at).dimmed().to_string(),
            };
            let pnl_str = if r.pnl >= 0.0 {
                format!("+{}{:.0}", csym, r.pnl).green().to_string()
            } else {
                format!("-{}{:.0}", csym, r.pnl.abs()).red().to_string()
            };
            println!(
                "  {:<14} {:>10} {:>10} {:>12}  {}",
                trade.symbol.cyan(),
                format!("{}{:.2}", csym, trade.entry_price),
                format!("{}{:.2}", csym, r.exit_price),
                pnl_str,
                result
            );
        }

        let total_pnl_pct = (total_pnl / session.capital) * 100.0;
        session.total_pnl = Some(total_pnl);
        session.total_pnl_pct = Some(total_pnl_pct);
        session.win_count = Some(wins);
        session.loss_count = Some(losses);
        session.settled = true;
        settled_any = true;

        println!("  {}", "─".repeat(64).dimmed());
        let verdict = if total_pnl >= session.capital * (session.target_pct / 100.0) {
            format!(
                "TARGET MET — {}{:.0} ({:+.2}%)",
                csym, total_pnl, total_pnl_pct
            )
            .green()
            .bold()
            .to_string()
        } else if total_pnl > 0.0 {
            format!(
                "Partial win — {}{:.0} ({:+.2}%)",
                csym, total_pnl, total_pnl_pct
            )
            .yellow()
            .to_string()
        } else {
            format!(
                "Loss — {}{:.0} ({:.2}%)",
                csym,
                total_pnl.abs(),
                total_pnl_pct
            )
            .red()
            .to_string()
        };
        println!("  {} {}", "→".bold(), verdict);
        println!(
            "  {} Replayed from {}; a bar that touched both the stop and a target counts as the stop.",
            "→".dimmed(),
            sources.join(" and ")
        );
    }

    if settled_any {
        history.save()?;
    }
    for s in &skipped {
        println!("  {} {}", "!".yellow(), s);
    }
    if !settled_any && !skipped.is_empty() {
        bail!("No session settled.");
    }
    println!(
        "\n  {} Run {} for stats.\n",
        "→".dimmed(),
        "stockwise sim history".cyan()
    );
    Ok(())
}

// ── Deep Dive: Everything about a stock ──

/// Clear the simulator history after confirmation. The old file is kept as a
/// timestamped backup so a mistaken reset can be undone by hand.
pub fn cmd_sim_reset(yes: bool) -> Result<()> {
    let history = SimHistory::load()?;
    if history.sessions.is_empty() {
        println!("  Simulator history is already empty.\n");
        return Ok(());
    }
    if !yes {
        use std::io::{BufRead, IsTerminal, Write};
        if !std::io::stdin().is_terminal() {
            bail!("Refusing to delete simulator history without confirmation. Pass --yes.");
        }
        print!(
            "  Delete {} simulator session(s)? Type {} to confirm: ",
            history.sessions.len(),
            "yes".bold()
        );
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().lock().read_line(&mut line)?;
        if line.trim() != "yes" {
            println!("  Cancelled — nothing was deleted.\n");
            return Ok(());
        }
    }
    let path = crate::store::path("sim_history.json")?;
    let backup = path.with_extension(format!(
        "json.{}.bak",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    ));
    std::fs::copy(&path, &backup)
        .with_context(|| format!("Could not back up {}", path.display()))?;
    SimHistory::default().save()?;
    println!(
        "  {} Simulator history cleared. Backup: {}\n",
        "✓".green(),
        backup.display()
    );
    Ok(())
}
