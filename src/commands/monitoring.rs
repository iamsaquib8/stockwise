//! `stockwise monitor` and `stockwise digest`, and the monitor passes the
//! background worker runs: price signals during the session (sent at once),
//! everything after the close (sent in the digest).

use super::*;
use crate::monitor::{MonitorState, Signal, Thresholds};
use crate::notify::{Message, Notifier};
use chrono::Utc;

/// Holdings checked per post-close pass for ownership, rating and insider
/// changes (each is a slower lookup).
const SLOW_CHECKS_PER_PASS: usize = 8;

/// The next session date after `now` for `market`.
fn next_session(market: Market, now: chrono::DateTime<Utc>) -> Option<chrono::NaiveDate> {
    let today = crate::clock::session_date(market, now);
    let mut t = now;
    for _ in 0..10 {
        let open = crate::clock::next_open(market, t)?;
        let d = crate::clock::session_date(market, open);
        if d > today {
            return Some(d);
        }
        t = open + chrono::Duration::hours(1);
    }
    None
}

/// Price signals for every holding from its daily bars (one chart each).
async fn price_pass(
    client: &YahooClient,
    portfolio: &Portfolio,
    t: &Thresholds,
    intraday_only: bool,
) -> Vec<Signal> {
    let held: Vec<(String, f64)> = portfolio
        .holdings
        .iter()
        .filter(|h| !market::is_namespaced(&h.symbol) && h.shares > 0.0)
        .map(|h| (h.symbol.clone(), h.avg_cost))
        .collect();
    let mut per: Vec<Vec<Signal>> = Vec::new();
    for chunk in held.chunks(8) {
        let futs: Vec<_> = chunk
            .iter()
            .map(|(s, cost)| async move {
                let Ok(Ok(chart)) = tokio::time::timeout(
                    std::time::Duration::from_secs(10),
                    client.get_chart(s, "1y", "1d"),
                )
                .await
                else {
                    return Vec::new();
                };
                let series = crate::research::Series::from_chart(&chart);
                crate::monitor::price_signals(s, &series, (*cost > 0.0).then_some(*cost), t)
            })
            .collect();
        per.extend(futures_util::future::join_all(futs).await);
    }
    per.into_iter()
        .flatten()
        .filter(|s| !intraday_only || s.kind.intraday())
        .collect()
}

/// Every signal for the portfolio: prices, dates tomorrow, and (for a
/// rotating handful of holdings) ownership, rating and insider changes.
pub(crate) async fn full_pass(
    client: &YahooClient,
    market: Market,
    state: &mut MonitorState,
) -> Result<Vec<Signal>> {
    let t = Thresholds::from_config();
    let portfolio = Portfolio::load()?;
    if !t.enabled || portfolio.holdings.is_empty() {
        return Ok(Vec::new());
    }
    let now = Utc::now();
    let mut signals = price_pass(client, &portfolio, &t, false).await;

    let stocks: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .filter(|s| crate::market::AssetClass::of(s) == crate::market::AssetClass::Stock)
        .collect();
    // Dates tomorrow, each symbol on its own exchange's calendar.
    let mut cal = crate::calendar::Calendar::new(client);
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    for s in &stocks {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        let m = crate::alerts::alert_market(s).unwrap_or(market);
        if let Some(tomorrow) = next_session(m, now) {
            let events = cal.events_for(s).await;
            signals.extend(crate::monitor::event_signals(&events, tomorrow));
        }
    }
    cal.save();

    // Ownership, ratings and insiders: a few holdings per pass, round-robin.
    let mut sorted = stocks.clone();
    sorted.sort();
    if !sorted.is_empty() {
        let start = state.cursor % sorted.len();
        let n = SLOW_CHECKS_PER_PASS.min(sorted.len());
        let since = now.timestamp() - 3 * 86_400;
        for i in 0..n {
            let s = &sorted[(start + i) % sorted.len()];
            let m = crate::alerts::alert_market(s).unwrap_or(market);
            let needs = crate::research::Needs::NONE.with_ownership(true);
            let Ok(Ok(r)) = tokio::time::timeout(
                std::time::Duration::from_secs(20),
                crate::research::fetch(client, s, m, 7, needs),
            )
            .await
            else {
                continue;
            };
            signals.extend(crate::monitor::ownership_signals(s, &r.holdings));
            signals.extend(crate::monitor::rating_signals(s, &r.actions, since));
            signals.extend(crate::monitor::insider_signals(
                s,
                &r.insiders,
                since,
                r.currency.as_deref(),
            ));
        }
        state.cursor = (start + n) % sorted.len();
    }
    Ok(state.fresh(signals))
}

/// The intraday pass the worker runs while a held market is open: new price
/// signals, sent together as one message and remembered once delivered.
/// Returns how many were sent.
pub(crate) async fn intraday_pass(client: &YahooClient, notifier: &Notifier) -> Result<usize> {
    let t = Thresholds::from_config();
    let portfolio = Portfolio::load()?;
    if !t.enabled || portfolio.holdings.is_empty() {
        return Ok(0);
    }
    let signals = price_pass(client, &portfolio, &t, true).await;
    let state: MonitorState = crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
    let fresh = state.fresh(signals);
    if fresh.is_empty() {
        return Ok(0);
    }
    let msg = match fresh.as_slice() {
        [one] => one.message(),
        many => crate::notify::batch(
            &format!("StockWise: {} holdings moving", many.len()),
            &many.iter().map(Signal::message).collect::<Vec<_>>(),
        ),
    };
    if notifier.send_logged(&msg).await {
        crate::store::update::<MonitorState, ()>(crate::monitor::STATE_FILE, |st| {
            st.record(&fresh);
            st.last_pass = Some(Utc::now().timestamp());
            Ok(())
        })?;
    }
    Ok(fresh.len())
}

/// Build the digest for today: pulse, monitor signals, alerts logged since
/// the start of the session, and `notices`.
pub(crate) async fn build_digest(
    client: &YahooClient,
    market: Market,
    notices: Vec<Message>,
    state: &mut MonitorState,
) -> Result<(crate::digest::Digest, Vec<Signal>)> {
    let pulse = crate::pulse::gather(
        client,
        market,
        crate::pulse::Options {
            days: 1,
            watchlist: false,
            news: true,
            events: true,
            event_days: 7,
        },
    )
    .await?;
    let now = Utc::now();
    let date = crate::worker::last_closed_session(market, now)
        .unwrap_or_else(|| crate::clock::session_date(market, now));
    let mut d = crate::digest::Digest::from_pulse(&pulse, &date.to_string(), 3);
    let signals = full_pass(client, market, state).await.unwrap_or_default();
    d.signals = signals.clone();
    let start = date
        .and_hms_opt(0, 0, 0)
        .map_or(0, |t| t.and_utc().timestamp() - 6 * 3600);
    d.alerts = crate::digest::alerts_since(start);
    d.notices = notices;
    Ok((d, signals))
}

/// Send `digest` once per session date. Returns whether it was sent now.
pub(crate) async fn send_digest_once(
    notifier: &Notifier,
    digest: &crate::digest::Digest,
    signals: &[Signal],
) -> Result<bool> {
    let state: MonitorState = crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
    if state.last_digest.as_deref() == Some(digest.date.as_str()) {
        return Ok(false);
    }
    if digest.is_empty() {
        return Ok(false);
    }
    let msg = crate::digest::render(digest);
    if !notifier.send_logged(&msg).await {
        return Ok(false);
    }
    crate::store::update::<MonitorState, ()>(crate::monitor::STATE_FILE, |st| {
        st.record(signals);
        st.last_digest = Some(digest.date.clone());
        st.last_pass = Some(Utc::now().timestamp());
        Ok(())
    })?;
    Ok(true)
}

/// [`send_digest_once`], also persisting the advanced `cursor` from `full_pass`.
pub(crate) async fn send_digest_once_with_cursor(
    notifier: &Notifier,
    digest: &crate::digest::Digest,
    signals: &[Signal],
    cursor: usize,
) -> Result<bool> {
    let sent = send_digest_once(notifier, digest, signals).await?;
    if sent {
        crate::store::update::<MonitorState, ()>(crate::monitor::STATE_FILE, |st| {
            st.cursor = cursor;
            Ok(())
        })?;
    }
    Ok(sent)
}

/// `stockwise monitor [status|run]`.
pub async fn cmd_monitor(action: &str, notify: bool, json: bool, market: Market) -> Result<()> {
    let t = Thresholds::from_config();
    let state: MonitorState = crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
    if action == "run" {
        let client = YahooClient::new().await?;
        let mut st = state;
        let signals = full_pass(&client, market, &mut st).await?;
        let mut sent_ok = false;
        if notify && !signals.is_empty() {
            let notifier = Notifier::from_env();
            let msg = crate::notify::batch(
                &format!("StockWise monitor: {} new", signals.len()),
                &signals.iter().map(Signal::message).collect::<Vec<_>>(),
            );
            if notifier.send_logged(&msg).await {
                crate::store::update::<MonitorState, ()>(crate::monitor::STATE_FILE, |s| {
                    s.record(&signals);
                    s.cursor = st.cursor;
                    Ok(())
                })?;
                sent_ok = true;
            }
        } else {
            let _ = crate::store::update::<MonitorState, ()>(crate::monitor::STATE_FILE, |s| {
                s.cursor = st.cursor;
                Ok(())
            });
        }
        if json {
            return output::print_json(&serde_json::json!({
                "signals": signals,
                "notified": sent_ok,
            }));
        }
        print_header("Monitor — one pass over every holding");
        if signals.is_empty() {
            println!("  {}", "Nothing new.".dimmed());
        }
        for s in &signals {
            let mark = match s.priority {
                crate::notify::Priority::High => "▲".red().bold().to_string(),
                crate::notify::Priority::Normal => "●".yellow().to_string(),
                crate::notify::Priority::Low => "·".dimmed().to_string(),
            };
            println!("  {} {}  {}", mark, s.title.bold(), s.detail.dimmed());
        }
        if sent_ok {
            println!("\n  {} Sent and remembered.", "✓".green());
        } else if !notify && !signals.is_empty() {
            println!(
                "\n  {} Not sent (add --notify). The worker sends these on its own.",
                "→".dimmed()
            );
        }
        println!();
        return Ok(());
    }
    // status
    if json {
        return output::print_json(&serde_json::json!({
            "thresholds": t,
            "last_pass": state.last_pass,
            "last_digest": state.last_digest,
            "recent": state.recent,
            "worker_running": crate::worker::running_pid().is_some(),
        }));
    }
    print_header("Monitor");
    println!(
        "  {} every holding, no setup — {}",
        if t.enabled {
            "On:".green().to_string()
        } else {
            "Off:".red().to_string()
        },
        "set [monitor] in the config file or STOCKWISE_MONITOR_<KEY>".dimmed()
    );
    println!(
        "  Day drop ≥ {:.1}% or ≥ {:.1}σ · new 52-week low · {:.0}% below cost · {:.0}% off the high · gap ≥ {:.1}%",
        t.day_drop_pct, t.day_drop_sigma, t.from_cost_pct, t.from_peak_pct, t.gap_pct
    );
    println!(
        "  Dates tomorrow · promoter pledge and shareholding · rating changes · insider trades"
    );
    println!(
        "  Digest after the close: {}",
        if t.digest { "on" } else { "off" }
    );
    match crate::worker::running_pid() {
        Some(pid) => println!("  Worker running (pid {pid})."),
        None => println!(
            "  {} The worker is not running: {} (or {} for a login service).",
            "!".yellow(),
            "stockwise worker start".cyan(),
            "stockwise worker install".cyan()
        ),
    }
    if let Some(ts) = state
        .last_pass
        .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
    {
        println!(
            "  Last pass {}",
            ts.with_timezone(&chrono::Local).format("%d %b %H:%M")
        );
    }
    if !state.recent.is_empty() {
        print_section("Recent signals");
        for s in state.recent.iter().rev().take(15) {
            println!("  • {}  {}", s.title, s.detail.dimmed());
        }
    }
    println!(
        "\n  {} {} runs a pass now.\n",
        "→".dimmed(),
        "stockwise monitor run".cyan()
    );
    Ok(())
}

/// `stockwise digest [--send] [--json]`.
pub async fn cmd_digest(send: bool, json: bool, market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let mut state: MonitorState =
        crate::store::load(crate::monitor::STATE_FILE).unwrap_or_default();
    let (digest, signals) = build_digest(&client, market, Vec::new(), &mut state).await?;
    let msg = crate::digest::render(&digest);
    if send {
        let notifier = Notifier::from_env();
        if !notifier.is_enabled() {
            bail!("No notification channel is configured — see `stockwise notify`.");
        }
        // An explicit --send always goes out, even if today's was sent.
        if notifier.send_logged(&msg).await {
            crate::store::update::<MonitorState, ()>(crate::monitor::STATE_FILE, |st| {
                st.record(&signals);
                st.cursor = state.cursor;
                Ok(())
            })?;
        } else {
            bail!("Every channel failed.");
        }
    }
    if json {
        return output::print_json(&digest);
    }
    print_header(&msg.title);
    println!("{}", msg.body);
    println!();
    if send {
        println!("  {} Sent.", "✓".green());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_next_session_skips_weekends() {
        // Friday 20 Nov 2026, after the NSE close (12:00 UTC = 17:30 IST).
        let fri = chrono::NaiveDate::from_ymd_opt(2026, 11, 20)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
            .and_utc();
        let next = next_session(Market::In, fri).unwrap();
        assert_eq!(next, chrono::NaiveDate::from_ymd_opt(2026, 11, 23).unwrap());
    }
}
