//! `daemon` workers: the intraday paper-trading loop and the daily long-term scan.

use super::*;
use crate::clock;
use crate::daemon::{DaemonState, Event, Phase, PositionStatus, Shutdown};
use crate::notify::{Message, Notifier};
use chrono::{DateTime, Datelike, Utc};
use std::collections::HashMap;
use std::time::Duration;

pub async fn cmd_daemon(mode: &str, amount: Option<f64>, market: Market) -> Result<()> {
    // Both loops follow exchange sessions; crypto and FX have none.
    market.require_bot_support("daemon")?;
    let csym = market.currency_symbol();

    match mode {
        "intraday" => return run_intraday(amount, market).await,

        "longterm" => {
            print_header("Long-Term Daemon — Daily Run");
            println!("  {} Running post-market analysis...\n", "⟳".yellow());

            let client = YahooClient::new().await?;

            // 1. Score all stocks
            println!("  [1/5] Scoring stocks...");
            let symbols = market.universe();
            let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
            let quotes = client.get_quote(&sym_refs).await?;
            let mut scores: Vec<crate::longterm::LongTermScore> = Vec::new();
            for q in &quotes {
                let sym = q.symbol.as_deref().unwrap_or("");
                let hist = client.get_chart(sym, "1y", "1d").await.ok().and_then(|c| {
                    c.indicators
                        .quote
                        .first()
                        .and_then(|qi| qi.close.as_ref())
                        .map(|c| c.iter().filter_map(|v| *v).collect::<Vec<f64>>())
                });
                scores.push(crate::longterm::score_for_longterm(q, hist.as_deref()));
            }
            scores.sort_by(|a, b| {
                b.total_score
                    .partial_cmp(&a.total_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            println!(
                "    Top 3: {}",
                scores
                    .iter()
                    .take(3)
                    .map(|s| format!("{} ({:.0})", s.symbol, s.total_score))
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            // 2. Check portfolio
            println!("  [2/5] Checking portfolio...");
            let portfolio = Portfolio::load()?;
            if !portfolio.holdings.is_empty() {
                let p_syms: Vec<String> = portfolio
                    .holdings
                    .iter()
                    .map(|h| h.symbol.clone())
                    .collect();
                // Totals in the market's currency (like `wealth`, so snapshots
                // never mix currencies); holdings without a price or an
                // exchange rate are left out and skip the wealth snapshot.
                let base = super::portfolio::market_currency(market);
                let val =
                    super::portfolio::value_holdings(&client, &p_syms, market, Some(base)).await?;
                let totals = val.totals(&portfolio);
                let (total_value, total_cost) = (totals.value, totals.cost);
                let unpriced = totals.unpriced.len() + totals.unconverted.len();
                let csym = market::currency_symbol(Some(val.base()));
                let pnl = total_value - total_cost;
                let pnl_str = if pnl >= 0.0 {
                    format!("+{}{:.0}", csym, pnl).green().to_string()
                } else {
                    format!("-{}{:.0}", csym, pnl.abs()).red().to_string()
                };
                println!(
                    "    Portfolio: {}{:.0} ({}) | {} holdings",
                    csym,
                    total_value,
                    pnl_str,
                    portfolio.holdings.len()
                );

                // Wealth snapshot — skipped when a price is missing, so the
                // history never records a fake drop.
                if unpriced == 0 {
                    let mut wealth = crate::wealth::WealthHistory::load()?;
                    wealth.add_snapshot(total_value, total_cost, portfolio.holdings.len());
                    wealth.save()?;
                } else {
                    println!(
                        "    {} {} holding(s) without a live price — wealth snapshot skipped.",
                        "!".yellow(),
                        unpriced
                    );
                }
            } else {
                println!("    No portfolio holdings.");
            }

            // 3. Check alerts
            println!("  [3/5] Checking alerts...");
            let store = crate::alerts::AlertStore::load()?;
            if !store.alerts.is_empty() {
                let a_syms: Vec<String> = store.alerts.iter().map(|a| a.symbol.clone()).collect();
                let mut unique: Vec<&str> = a_syms.iter().map(|s| s.as_str()).collect();
                unique.sort();
                unique.dedup();
                let a_quotes = client.get_quote(&unique).await?;
                let rows =
                    views::alert_rows(&client, &store.alerts, &a_quotes, chrono::Utc::now()).await;
                let mut triggered = 0;
                for (row, alert) in rows.iter().zip(&store.alerts) {
                    if row.status != "triggered" {
                        continue;
                    }
                    triggered += 1;
                    let now = row
                        .price
                        .map(|p| format!(" (now {}{:.2})", csym, p))
                        .unwrap_or_default();
                    println!(
                        "    {} {} {} — TRIGGERED{}",
                        "⚠".yellow(),
                        alert.symbol,
                        alert.describe(crate::i18n::Lang::En),
                        now
                    );
                }
                if triggered == 0 {
                    println!("    No alerts triggered.");
                }
            }

            // 4. Tax harvest check
            println!("  [4/5] Tax harvest scan...");
            if !portfolio.holdings.is_empty() {
                let p_syms: Vec<String> = portfolio
                    .holdings
                    .iter()
                    .map(|h| h.symbol.clone())
                    .collect();
                let p_refs: Vec<&str> = p_syms.iter().map(|s| s.as_str()).collect();
                let p_quotes = client.get_quote(&p_refs).await?;
                let losers: Vec<_> = portfolio
                    .holdings
                    .iter()
                    .filter(|h| {
                        p_quotes
                            .iter()
                            .find(|q| q.symbol.as_deref() == Some(&h.symbol))
                            .and_then(|q| q.regular_market_price)
                            .is_some_and(|price| price < h.avg_cost)
                    })
                    .collect();
                if !losers.is_empty() {
                    println!(
                        "    {} positions with losses (harvest candidates): {}",
                        losers.len(),
                        losers
                            .iter()
                            .map(|h| h.symbol.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                } else {
                    println!("    No tax-loss harvest opportunities.");
                }
            }

            // 5. AI report
            println!("  [5/5] Generating AI research notes...");
            let mut ai = crate::ai::AiClient::connect().await;
            if ai.is_available().await {
                let mut data = String::new();
                for s in scores.iter().take(10) {
                    data.push_str(&format!(
                        "{}: score={:.0}, moat={}, 5Y={:+.0}%, risk={}\n",
                        s.symbol, s.total_score, s.moat, s.projected_5y_return, s.risk_tier
                    ));
                }
                if let Ok(report) = ai.generate_longterm_report(&data).await {
                    print_section("AI Research Notes");
                    print_ai_answer(&report);
                }
            } else {
                println!("    Ollama not running. Skipping AI report.");
            }

            println!("\n  {} Long-term daemon complete.", "✓".green().bold());
        }

        _ => {
            println!();
            println!("  {} Daemon Modes:", "→".cyan());
            println!();
            println!(
                "    {} — Paper-trading loop during market hours (NSE 9:00–16:00 IST)",
                "stockwise daemon intraday [CAPITAL]".bold()
            );
            println!(
                "    {} — Daily long-term analysis (run after market close)",
                "stockwise daemon longterm".bold()
            );
            println!();
            println!(
                "  {} The intraday daemon only paper-trades: it never sends broker orders.",
                "→".dimmed()
            );
            println!(
                "  {} It scans, enters, monitors, and squares off automatically.",
                "→".dimmed()
            );
            println!(
                "  {} Results are saved to simulation history.",
                "→".dimmed()
            );
            println!();
        }
    }
    Ok(())
}

const DEFAULT_CAPITAL: f64 = 25000.0;
const TARGET_PCT: f64 = 2.0;

fn currency(market: Market) -> &'static str {
    market.currency_symbol()
}

fn market_code(market: Market) -> &'static str {
    market.id()
}

/// Exchange-local wall-clock time for log lines.
fn hhmmss(market: Market, at: DateTime<Utc>) -> String {
    clock::local(market, at).format("%H:%M:%S").to_string()
}

/// One Yahoo client for the life of a worker loop, rebuilt after repeated
/// failures (the session crumb can expire in a long-running process).
#[derive(Default)]
pub(super) struct Feed {
    client: Option<YahooClient>,
    failures: u32,
}

impl Feed {
    pub(super) async fn client(&mut self) -> Result<&YahooClient> {
        if self.client.is_none() {
            self.client = Some(YahooClient::new().await?);
        }
        Ok(self.client.as_ref().expect("client was just created"))
    }

    /// Record a failed tick; returns how long to wait before the retry.
    pub(super) fn failed(&mut self) -> Duration {
        self.failures += 1;
        if self.failures.is_multiple_of(3) {
            self.client = None;
        }
        crate::daemon::retry_delay(self.failures)
    }

    pub(super) fn succeeded(&mut self) {
        self.failures = 0;
    }
}

/// Last prices by symbol. Symbols without a price are left out — callers
/// keep the previous price rather than valuing a position at 0.
pub(super) async fn live_prices(
    client: &YahooClient,
    symbols: &[String],
) -> Result<HashMap<String, f64>> {
    let refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&refs).await?;
    Ok(quotes
        .into_iter()
        .filter_map(|q| {
            let price = q
                .regular_market_price
                .filter(|p| p.is_finite() && *p > 0.0)?;
            Some((q.symbol?, price))
        })
        .collect())
}

/// How long to sleep while the market is closed: until the next pre-market
/// (15 minutes before the open), in chunks of at most 30 minutes.
fn wait_for_next_session(market: Market, now: DateTime<Utc>) -> Duration {
    let wake = clock::next_open(market, now).map(|open| open - chrono::Duration::minutes(15));
    match wake {
        Some(t) if t > now => (t - now)
            .to_std()
            .unwrap_or(Duration::from_secs(30))
            .clamp(Duration::from_secs(30), Duration::from_secs(1800)),
        _ => Duration::from_secs(30),
    }
}

fn announce_phase(phase: Phase, market: Market, now: DateTime<Utc>) {
    let text = match phase {
        Phase::PreMarket => "PRE-MARKET — Scanning...".yellow().bold().to_string(),
        Phase::Opening => "MARKET OPEN — Entering positions"
            .green()
            .bold()
            .to_string(),
        Phase::Active => "ACTIVE TRADING — Monitoring".cyan().bold().to_string(),
        Phase::WindDown => "WIND-DOWN — Tightening stops, no new entries"
            .yellow()
            .to_string(),
        Phase::SquareOff => "SQUARE OFF — Closing all positions"
            .red()
            .bold()
            .to_string(),
        Phase::PostMarket => "POST-MARKET — Settling".dimmed().to_string(),
        Phase::Closed => {
            let next = clock::next_open(market, now)
                .map(|t| format!(" — next session {}", clock::format_local(market, t)))
                .unwrap_or_default();
            format!(
                "MARKET CLOSED ({}){}",
                clock::closed_reason(market, now),
                next
            )
            .dimmed()
            .to_string()
        }
    };
    println!("\n  {} [{}] {}", "▶".bold(), hhmmss(market, now), text);
}

/// Print events and send them as one notification.
async fn report(events: &[Event], market: Market, notifier: &Notifier) {
    if events.is_empty() {
        return;
    }
    let csym = currency(market);
    let now = hhmmss(market, Utc::now());
    let entries = events
        .iter()
        .filter(|e| matches!(e, Event::Entered { .. }))
        .count();
    if entries > 0 {
        println!("  {} {} positions entered:\n", "✓".green().bold(), entries);
    }
    for e in events {
        match e {
            Event::Entered {
                symbol,
                direction,
                qty,
                entry,
                target1,
                target2,
                stop,
            } => println!(
                "    {} {} {} × {} @ {}{:.2}  T1:{}{:.2}  T2:{}{:.2}  SL:{}{:.2}",
                "→".green(),
                direction,
                symbol.cyan(),
                qty,
                csym,
                entry,
                csym,
                target1,
                csym,
                target2,
                csym,
                stop
            ),
            Event::KillSwitch { .. } => println!(
                "  🛑 [{}] {} {}",
                now,
                "KILL SWITCH".red().bold(),
                e.text(csym)
            ),
            Event::T1Booked { .. } => {
                println!("  ⚡ [{}] {}", now, e.text(csym).green().bold())
            }
            Event::Closed { status, .. } => {
                let line = e.text(csym);
                let line = match status {
                    PositionStatus::T2Hit => line.green().bold().to_string(),
                    PositionStatus::StopHit => line.red().bold().to_string(),
                    _ => line.yellow().to_string(),
                };
                println!("  ⚡ [{}] {}", now, line);
            }
        }
    }
    if let Some(msg) = events_message(events, csym) {
        notifier.send_logged(&msg).await;
    }
}

/// One notification for a tick's events.
fn events_message(events: &[Event], csym: &str) -> Option<Message> {
    let first = events.first()?;
    let entries = events
        .iter()
        .filter(|e| matches!(e, Event::Entered { .. }))
        .count();
    let title = if events.iter().any(|e| matches!(e, Event::KillSwitch { .. })) {
        "StockWise daemon: kill switch triggered".to_string()
    } else if entries > 0 {
        format!("StockWise daemon: entered {} paper positions", entries)
    } else if events.len() == 1 {
        format!("StockWise daemon: {}", first.text(csym))
    } else {
        format!("StockWise daemon: {} position updates", events.len())
    };
    let body = if events.len() == 1 && entries == 0 {
        String::new()
    } else {
        events
            .iter()
            .map(|e| e.text(csym))
            .collect::<Vec<_>>()
            .join("\n")
    };
    Some(Message::new(title, body))
}

fn print_status(state: &DaemonState, market: Market, phase: Phase) {
    let csym = currency(market);
    let total = state.total_pnl();
    let total_str = if total >= 0.0 {
        crate::daemon::signed_money(csym, total).green().to_string()
    } else {
        crate::daemon::signed_money(csym, total).red().to_string()
    };
    println!(
        "  {} [{}] {} | {} open | P&L: {} | Phase: {}",
        "●".dimmed(),
        hhmmss(market, Utc::now()),
        format!("Tick #{}", state.tick_count).dimmed(),
        state.open_count(),
        total_str,
        phase
    );
}

fn print_eod_table(state: &DaemonState, market: Market) {
    let csym = currency(market);
    println!("\n  {}", "─".repeat(60).dimmed());
    print_header(&format!("End of Day Settlement — {}", state.date));
    println!(
        "  {:<14} {:>10} {:>10} {:>12} {:>12}",
        "Symbol".bold(),
        "Entry".bold(),
        "Exit".bold(),
        "P&L".bold(),
        "Status".bold()
    );
    println!("  {}", "─".repeat(62).dimmed());
    for p in &state.positions {
        let pnl = crate::daemon::signed_money(csym, p.pnl);
        let pnl = if p.pnl >= 0.0 {
            pnl.green().to_string()
        } else {
            pnl.red().to_string()
        };
        println!(
            "  {:<14} {:>10} {:>10} {:>12} {:>12}",
            p.symbol.cyan(),
            format!("{}{:.2}", csym, p.entry_price),
            format!("{}{:.2}", csym, p.current_price),
            pnl,
            p.status
        );
    }
    println!("  {}", "─".repeat(62).dimmed());
    let total = state.total_pnl();
    let wins = state.positions.iter().filter(|p| p.pnl > 0.0).count();
    let total_str = crate::daemon::signed_money(csym, total);
    let total_str = if total >= 0.0 {
        total_str.green().bold().to_string()
    } else {
        total_str.red().bold().to_string()
    };
    println!(
        "  Total: {}  |  {} wins, {} losses",
        total_str,
        wins,
        state.positions.len() - wins
    );
}

fn eod_message(state: &DaemonState, market: Market, title: &str) -> Message {
    let csym = currency(market);
    let wins = state.positions.iter().filter(|p| p.pnl > 0.0).count();
    let mut body = format!(
        "P&L {} ({:+.2}% of {}{:.0}) — {} wins, {} losses",
        crate::daemon::signed_money(csym, state.total_pnl()),
        state.total_pnl() / state.capital * 100.0,
        csym,
        state.capital,
        wins,
        state.positions.len() - wins
    );
    for p in &state.positions {
        body.push_str(&format!(
            "\n{} {} → {}{:.2} {} ({})",
            p.symbol,
            p.direction,
            csym,
            p.current_price,
            crate::daemon::signed_money(csym, p.pnl),
            p.status
        ));
    }
    Message::new(format!("{} {}", title, state.date), body)
}

/// Record the day in the simulator history and mark the state settled.
fn settle_day(state: &mut DaemonState, market: Market) -> Result<()> {
    print_eod_table(state, market);
    let mut history = SimHistory::load()?;
    if crate::daemon::record_session(&mut history, state.to_sim_session(true)) {
        history.save()?;
    }
    state.settled = true;
    println!("  {} Saved to simulation history.", "✓".green());
    Ok(())
}

/// A day whose close the daemon missed (it was not running): hand the
/// entries to `sim settle`, which replays that day's intraday bars.
fn hand_over_to_sim_settle(state: &mut DaemonState) -> Result<bool> {
    if !state.entered || state.settled {
        return Ok(false);
    }
    let mut history = SimHistory::load()?;
    if crate::daemon::record_session(&mut history, state.to_sim_session(false)) {
        history.save()?;
    }
    state.settled = true;
    println!(
        "  {} The daemon wasn't running at the close on {}: its {} position(s) were saved \
         as an open simulator session. Run {} to settle them from that day's 5-minute bars.",
        "!".yellow(),
        state.date,
        state.positions.len(),
        "stockwise sim settle".cyan()
    );
    Ok(true)
}

struct Tick {
    events: Vec<Event>,
    wait: Duration,
    done: bool,
}

impl Tick {
    fn wait(secs: u64) -> Self {
        Tick {
            events: Vec::new(),
            wait: Duration::from_secs(secs),
            done: false,
        }
    }
}

/// One iteration of the intraday loop. Network calls come first and state
/// changes after them, so a tick cancelled by Ctrl+C leaves the state as it
/// was; any error is retried by the caller.
async fn tick(
    state: &mut DaemonState,
    phase: Phase,
    market: Market,
    feed: &mut Feed,
    sector_scanned: &mut bool,
    now: DateTime<Utc>,
) -> Result<Tick> {
    if state.settled {
        return Ok(Tick {
            events: Vec::new(),
            wait: wait_for_next_session(market, now),
            done: false,
        });
    }
    match phase {
        Phase::Closed => {
            // Entered but closed: the close (and post-market) passed while
            // the daemon was down.
            if state.entered {
                hand_over_to_sim_settle(state)?;
            }
            Ok(Tick {
                events: Vec::new(),
                wait: wait_for_next_session(market, now),
                done: false,
            })
        }
        Phase::PreMarket => {
            if !state.entered && !*sector_scanned && market == Market::In {
                println!("  {} Scanning sectors and candidates...", "⟳".yellow());
                let client = feed.client().await?;
                if let Ok(heats) = intraday::scan_sector_heat(client).await {
                    let hot: Vec<_> = heats.iter().filter(|h| h.hot).collect();
                    if !hot.is_empty() {
                        println!(
                            "  🔥 Hot sectors: {}",
                            hot.iter()
                                .map(|h| format!("{} ({:+.1}%)", h.name, h.change_pct))
                                .collect::<Vec<_>>()
                                .join(", ")
                        );
                    }
                }
                *sector_scanned = true;
            }
            Ok(Tick::wait(30))
        }
        Phase::Opening if state.entered => monitor(state, phase, market, feed).await,
        Phase::Opening => {
            if state.risk.killed {
                return Ok(Tick::wait(15));
            }
            println!("  {} Running 9-strategy scan...", "⟳".yellow());
            let client = feed.client().await?;
            let signals = intraday::scan_intraday(client, market).await?;
            let limits = intraday::PlanLimits {
                max_risk_pct: 1.0,
                max_position_pct: state.risk.max_position_pct,
                max_positions: state.risk.max_positions,
            };
            let plans = intraday::generate_trade_plans_with(
                &signals,
                state.capital,
                state.target_pct,
                limits,
            );
            if plans.is_empty() {
                println!(
                    "  {} No qualified trades yet. Rescanning in a minute.",
                    "→".yellow()
                );
                return Ok(Tick::wait(60));
            }
            Ok(Tick {
                events: state.enter(&plans, now),
                wait: Duration::from_secs(15),
                done: false,
            })
        }
        Phase::Active | Phase::WindDown | Phase::SquareOff => {
            monitor(state, phase, market, feed).await
        }
        Phase::PostMarket => {
            if state.open_count() > 0 {
                // The square-off was missed (daemon down, or every square-off
                // tick failed): last prices may be hours old, so let
                // `sim settle` replay the day's bars instead.
                hand_over_to_sim_settle(state)?;
            } else if state.entered {
                settle_day(state, market)?;
            } else {
                state.settled = true;
                println!("\n  {} No trades today.", "→".dimmed());
            }
            Ok(Tick {
                events: Vec::new(),
                wait: Duration::ZERO,
                done: true,
            })
        }
    }
}

/// Mark open positions to live prices (after the entries, through the
/// square-off).
async fn monitor(
    state: &mut DaemonState,
    phase: Phase,
    market: Market,
    feed: &mut Feed,
) -> Result<Tick> {
    if state.open_count() == 0 {
        return Ok(Tick::wait(60));
    }
    let client = feed.client().await?;
    let prices = live_prices(client, &state.open_symbols()).await?;
    let mut events = state.apply_prices(&prices, phase);
    if phase == Phase::SquareOff {
        // Anything without a fresh price closes at its last one.
        events.extend(state.square_off_all());
    }
    state.tick_count += 1;
    if state.tick_count.is_multiple_of(5) || !events.is_empty() {
        print_status(state, market, phase);
    }
    if state.open_count() == 0 && !events.is_empty() {
        println!(
            "\n  {} All positions closed. Waiting for post-market.",
            "✓".green().bold()
        );
    }
    let secs = match phase {
        Phase::Opening => 15,
        Phase::SquareOff => 30,
        _ => 60,
    };
    Ok(Tick {
        events,
        wait: Duration::from_secs(secs),
        done: false,
    })
}

/// Ctrl+C / SIGTERM: square off at last prices, settle, save, exit 0.
async fn stop_intraday(state: &mut DaemonState, market: Market, notifier: &Notifier) -> Result<()> {
    let open = state.open_count();
    if open > 0 {
        println!(
            "\n  {} Stopping — closing {} open paper position(s) at their last price.",
            "■".yellow().bold(),
            open
        );
    } else {
        println!("\n  {} Stopping.", "■".yellow().bold());
    }
    let events = state.square_off_all();
    for e in &events {
        println!("  {} {}", "→".yellow(), e.text(currency(market)));
    }
    // Save first: if the history write fails, a restart still has the day.
    state.save(Utc::now())?;
    let had_positions = state.entered && !state.settled;
    if had_positions {
        settle_day(state, market)?;
        state.save(Utc::now())?;
    }
    if had_positions {
        notifier
            .send_logged(&eod_message(state, market, "StockWise daemon stopped —"))
            .await;
    }
    println!(
        "\n  {} Daemon stopped.{}",
        "✓".green().bold(),
        if had_positions {
            " Results are in `stockwise sim history`."
        } else {
            ""
        }
    );
    Ok(())
}

async fn run_intraday(amount: Option<f64>, market: Market) -> Result<()> {
    let csym = currency(market);
    let code = market_code(market);
    let notifier = Notifier::from_env();
    let mut shutdown =
        Shutdown::install("Today's state is in daemon_state.json; restart the daemon to resume.");
    let now = Utc::now();
    let today = clock::session_date(market, now);

    // Resume today's session, or hand an unfinished earlier one to `sim settle`.
    let mut state = match DaemonState::load(code)? {
        Some(saved) if saved.date == today && saved.market == code => saved,
        Some(mut stale) => {
            hand_over_to_sim_settle(&mut stale)?;
            DaemonState::new(
                today,
                code,
                amount.unwrap_or(DEFAULT_CAPITAL),
                TARGET_PCT,
                now,
            )
        }
        None => DaemonState::new(
            today,
            code,
            amount.unwrap_or(DEFAULT_CAPITAL),
            TARGET_PCT,
            now,
        ),
    };
    if !state.capital.is_finite() || state.capital <= 0.0 {
        bail!("Capital must be a positive amount.");
    }

    print_header("Intraday Daemon — Starting");
    println!(
        "  Capital: {}{:.0}  |  Target: {}%  |  Max Risk: {}% daily",
        csym, state.capital, state.target_pct, state.risk.daily_loss_limit_pct
    );
    println!(
        "  Max {} positions, {}% each  |  Paper trading only — never sends broker orders",
        state.risk.max_positions, state.risk.max_position_pct
    );
    if state.entered || state.tick_count > 0 {
        println!(
            "  {} Resuming today's session from {}: {} open position(s), P&L {}",
            "↻".cyan().bold(),
            crate::daemon::state_file(&state.market),
            state.open_count(),
            crate::daemon::signed_money(csym, state.total_pnl())
        );
        if let Some(a) = amount
            && (a - state.capital).abs() > f64::EPSILON
        {
            println!(
                "  {} Keeping today's capital {}{:.0} (ignoring {}{:.0}).",
                "!".yellow(),
                csym,
                state.capital,
                csym,
                a
            );
        }
    }
    if state.settled {
        println!(
            "  {} Today's session is already settled — see {}. Waiting for the next session.",
            "→".dimmed(),
            "stockwise sim history".cyan()
        );
    }
    if let Some(w) = clock::coverage_warning(market, today.year()) {
        println!("  {} {}", "!".yellow(), w);
    }
    if notifier.is_enabled() {
        let names: Vec<String> = notifier
            .config()
            .channels()
            .iter()
            .map(|c| c.to_string())
            .collect();
        println!("  Notifications: {}", names.join(", "));
    }
    println!("  {}", "─".repeat(60).dimmed());
    println!(
        "  {} Press Ctrl+C to stop: open positions are squared off at their last price and recorded.\n",
        "→".dimmed()
    );

    let mut feed = Feed::default();
    let mut last_phase: Option<Phase> = None;
    let mut sector_scanned = false;
    let mut late_start_noted = false;

    loop {
        if shutdown.requested() {
            return stop_intraday(&mut state, market, &notifier).await;
        }
        let now = Utc::now();
        let date = clock::session_date(market, now);
        if state.date != date {
            if let Err(e) = hand_over_to_sim_settle(&mut state) {
                eprintln!(
                    "  {} Could not hand {} over to the simulator: {:#}",
                    "!".yellow(),
                    state.date,
                    e
                );
            }
            state = DaemonState::new(date, code, state.capital, state.target_pct, now);
            sector_scanned = false;
            late_start_noted = false;
        }
        let phase = clock::market_phase(market, now);
        if last_phase != Some(phase) {
            announce_phase(phase, market, now);
            last_phase = Some(phase);
        }
        if !state.entered
            && !state.settled
            && !late_start_noted
            && matches!(phase, Phase::Active | Phase::WindDown | Phase::SquareOff)
        {
            println!(
                "  {} The entry window (first 15 minutes after the open) has passed — no new positions today.",
                "→".dimmed()
            );
            late_start_noted = true;
        }

        let outcome = tokio::select! {
            r = tick(&mut state, phase, market, &mut feed, &mut sector_scanned, now) => Some(r),
            _ = shutdown.wait() => None,
        };
        let Some(outcome) = outcome else {
            continue; // stop requested mid-tick; handled at the top
        };
        let wait = match outcome {
            Ok(t) => {
                feed.succeeded();
                report(&t.events, market, &notifier).await;
                if let Err(e) = state.save(Utc::now()) {
                    eprintln!(
                        "  {} Could not save {}: {:#}",
                        "!".yellow(),
                        crate::daemon::state_file(&state.market),
                        e
                    );
                }
                if t.done {
                    if state.entered && state.open_count() == 0 {
                        notifier
                            .send_logged(&eod_message(&state, market, "StockWise daemon EOD"))
                            .await;
                    } else if state.entered {
                        notifier
                            .send_logged(&Message::new(
                                format!("StockWise daemon missed the square-off on {}", state.date),
                                "Run `stockwise sim settle` to settle the day from its 5-minute bars.",
                            ))
                            .await;
                    }
                    tokio::select! {
                        _ = eod_ai_report(&state, market) => {}
                        _ = shutdown.wait() => {}
                    }
                    println!(
                        "\n  {} Daemon complete for today. Exiting.",
                        "✓".green().bold()
                    );
                    return Ok(());
                }
                t.wait
            }
            Err(e) => {
                let wait = feed.failed();
                eprintln!(
                    "  {} [{}] Tick failed: {:#} — retrying in {}s",
                    "!".yellow(),
                    hhmmss(market, Utc::now()),
                    e,
                    wait.as_secs()
                );
                if let Err(e) = state.save(Utc::now()) {
                    eprintln!(
                        "  {} Could not save {}: {:#}",
                        "!".yellow(),
                        crate::daemon::state_file(&state.market),
                        e
                    );
                }
                wait
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.wait() => {}
        }
    }
}

async fn eod_ai_report(state: &DaemonState, market: Market) {
    if !state.entered {
        return;
    }
    let csym = currency(market);
    let mut ai = crate::ai::AiClient::connect().await;
    if !ai.is_available().await {
        return;
    }
    println!("  {} Generating AI end-of-day notes...", "⟳".yellow());
    let wins = state.positions.iter().filter(|p| p.pnl > 0.0).count();
    let mut data = format!(
        "Intraday results: P&L={}{:.0}, {} wins {} losses\n",
        csym,
        state.total_pnl(),
        wins,
        state.positions.len() - wins
    );
    for p in &state.positions {
        data.push_str(&format!(
            "{}: entry={:.2}, exit={:.2}, pnl={:.0}, status={}\n",
            p.symbol, p.entry_price, p.current_price, p.pnl, p.status
        ));
    }
    if let Ok(report) = ai.generate_intraday_report(&data).await {
        print_section("AI End-of-Day Notes");
        print_ai_answer(&report);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_become_one_message() {
        let closed = Event::Closed {
            symbol: "TCS.NS".into(),
            status: PositionStatus::StopHit,
            price: 98.0,
            pnl: -20.0,
        };
        let msg = events_message(std::slice::from_ref(&closed), "₹").unwrap();
        assert_eq!(
            msg.title,
            "StockWise daemon: STOPPED TCS.NS at ₹98.00 (P&L -₹20)"
        );
        assert!(msg.body.is_empty());

        let kill = Event::KillSwitch {
            loss: -800.0,
            limit: 750.0,
        };
        let msg = events_message(&[kill, closed], "₹").unwrap();
        assert!(msg.title.contains("kill switch"));
        assert_eq!(msg.body.lines().count(), 2);
        assert!(events_message(&[], "₹").is_none());
    }

    #[test]
    fn closed_market_waits_until_pre_market_in_bounded_chunks() {
        // Friday 18:00 IST → wake within 30 minutes, never busy-loop.
        let fri = clock::at(
            Market::In,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 23).unwrap(),
            chrono::NaiveTime::from_hms_opt(18, 0, 0).unwrap(),
        );
        assert_eq!(
            wait_for_next_session(Market::In, fri),
            Duration::from_secs(1800)
        );
        // 08:50 IST → 10 minutes to the 09:00 pre-market.
        let early = clock::at(
            Market::In,
            chrono::NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
            chrono::NaiveTime::from_hms_opt(8, 50, 0).unwrap(),
        );
        assert_eq!(
            wait_for_next_session(Market::In, early),
            Duration::from_secs(600)
        );
    }
}

// ── Multi-timeframe Chart ──
