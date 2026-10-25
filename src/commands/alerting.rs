//! `alert watch` — a loop that checks price alerts while their market is
//! open and notifies once per trigger — and `notify`, which shows and tests
//! the notification channels.

use super::worker::Feed;
use super::*;
use crate::alerts::alert_market;
use crate::clock;
use crate::daemon::Shutdown;
use crate::notify::{Channel, Message, Notifier};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

/// Remembers which alerts already fired, across restarts.
const WATCH_STATE: &str = "alert_watch.json";

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
struct Fired {
    /// Exchange-local date of the last notification.
    last: NaiveDate,
    /// The condition has been false since the last notification.
    armed: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct WatchState {
    fired: HashMap<String, Fired>,
}

impl WatchState {
    /// Whether an alert whose condition is `hit` should notify now. An alert
    /// notifies when its condition becomes true, re-arms once the condition
    /// has been false, and notifies at most once a day. A `per_session`
    /// condition (today's change or volume) also re-arms with each new
    /// session, since nothing checks it between the close and the next open.
    fn should_notify(&mut self, key: &str, hit: bool, today: NaiveDate) -> bool {
        self.should_notify_with(key, hit, today, false)
    }

    fn should_notify_with(
        &mut self,
        key: &str,
        hit: bool,
        today: NaiveDate,
        per_session: bool,
    ) -> bool {
        if per_session
            && let Some(f) = self.fired.get_mut(key)
            && f.last != today
        {
            f.armed = true;
        }
        match (hit, self.fired.get_mut(key)) {
            (true, None) => {
                self.fired.insert(
                    key.to_string(),
                    Fired {
                        last: today,
                        armed: false,
                    },
                );
                true
            }
            (true, Some(f)) if f.armed && f.last != today => {
                *f = Fired {
                    last: today,
                    armed: false,
                };
                true
            }
            (true, Some(_)) => false,
            (false, Some(f)) => {
                f.armed = true;
                false
            }
            (false, None) => false,
        }
    }

    /// Restore an alert's entry from before a trigger that couldn't be sent.
    fn undo(&mut self, key: &str, previous: Option<Fired>) {
        match previous {
            Some(f) => {
                self.fired.insert(key.to_string(), f);
            }
            None => {
                self.fired.remove(key);
            }
        }
    }

    /// Forget alerts that were removed.
    fn retain(&mut self, keys: &[String]) {
        self.fired.retain(|k, _| keys.contains(k));
    }
}

fn alert_key(a: &crate::alerts::Alert) -> String {
    let mut key = format!("{}|{}|{}|{}", a.symbol, a.condition, a.target, a.created_at);
    // Alerts from before technical conditions have no period: same key as before.
    if let Some(p) = a.period {
        key.push_str(&format!("|{p}"));
    }
    key
}

fn is_due(symbol: &str, now: DateTime<Utc>) -> bool {
    alert_market(symbol).is_none_or(|m| clock::is_open(m, now))
}

/// Longest sleep while every watched market is closed.
const MAX_IDLE_WAIT: Duration = Duration::from_secs(1800);

/// How long to sleep when nothing is due: until the earliest next open of
/// the markets the alerts trade on, in chunks of at most 30 minutes.
fn wait_for_open(markets: &[Market], now: DateTime<Utc>, interval: Duration) -> Duration {
    markets
        .iter()
        .filter_map(|m| clock::next_open(*m, now))
        .min()
        .and_then(|t| (t - now).to_std().ok())
        .unwrap_or(interval)
        .clamp(interval.min(MAX_IDLE_WAIT), MAX_IDLE_WAIT)
}

struct Trigger {
    /// Watch-state key and its entry before this trigger, to undo the
    /// "already notified" mark if the notification can't be delivered.
    key: String,
    previous: Option<Fired>,
    symbol: String,
    /// The condition in words (English: notifications aren't localized).
    condition: String,
    price: Option<f64>,
    currency: Option<String>,
    /// The indicator reading for technical alerts, e.g. "RSI 28.4".
    reading: Option<String>,
}

impl Trigger {
    fn text(&self) -> String {
        let mut text = format!("{} {}", self.symbol, self.condition);
        if let Some(price) = self.price {
            text.push_str(&format!(
                " — now {}",
                plain_price(price, self.currency.as_deref())
            ));
        }
        if let Some(reading) = &self.reading {
            text.push_str(&format!(" ({reading})"));
        }
        text
    }
}

fn plain_price(price: f64, currency: Option<&str>) -> String {
    format!("{}{:.2}", market::currency_symbol(currency), price)
}

fn trigger_message(triggers: &[Trigger]) -> Option<Message> {
    match triggers {
        [] => None,
        [t] => Some(Message::new(format!("StockWise alert: {}", t.text()), "")),
        many => Some(Message::new(
            format!("StockWise: {} alerts triggered", many.len()),
            many.iter()
                .map(Trigger::text)
                .collect::<Vec<_>>()
                .join("\n"),
        )),
    }
}

/// The result of one pass over the alerts.
struct Pass {
    /// Alerts configured.
    total: usize,
    /// Alerts whose market was open (all of them with `--once`).
    due: usize,
    /// Markets the alerts trade on (round-the-clock symbols excluded).
    markets: Vec<Market>,
    triggers: Vec<Trigger>,
}

/// One pass over the alerts that are due (all of them if `all`).
async fn check_alerts(
    feed: &mut Feed,
    state: &mut WatchState,
    market: Market,
    now: DateTime<Utc>,
    all: bool,
    verbose: bool,
) -> Result<Pass> {
    let store = AlertStore::load()?;
    let keys: Vec<String> = store.alerts.iter().map(alert_key).collect();
    state.retain(&keys);
    let mut markets = Vec::new();
    for m in store.alerts.iter().filter_map(|a| alert_market(&a.symbol)) {
        if !markets.contains(&m) {
            markets.push(m);
        }
    }
    let due: Vec<&crate::alerts::Alert> = store
        .alerts
        .iter()
        .filter(|a| all || is_due(&a.symbol, now))
        .collect();
    let mut pass = Pass {
        total: store.alerts.len(),
        due: due.len(),
        markets,
        triggers: Vec::new(),
    };
    if due.is_empty() {
        return Ok(pass);
    }
    let mut symbols: Vec<&str> = due.iter().map(|a| a.symbol.as_str()).collect();
    symbols.sort();
    symbols.dedup();
    let client = feed.client().await?;
    let quotes = views::quotes_with_off_market(client, &symbols).await?.0;
    // Daily bars once per symbol per check, for the technical conditions.
    let due_alerts: Vec<crate::alerts::Alert> = due.iter().map(|a| (*a).clone()).collect();
    let data = views::alert_data(client, &due_alerts, &quotes, now).await;

    for alert in due {
        let d = data.get(&alert.symbol).cloned().unwrap_or_default();
        let condition = alert.describe(crate::i18n::Lang::En);
        let today = clock::session_date(alert_market(&alert.symbol).unwrap_or(market), now);
        let key = alert_key(alert);
        let Some(reading) = alert.evaluate(&d) else {
            // Past results with the next date not known yet: re-arm, so the
            // next quarter's date notifies again.
            if alert.needs_results() {
                state.should_notify(&key, false, today);
            }
            if verbose {
                let why = if alert.condition.is_price() {
                    "no live price"
                } else if alert.needs_results() {
                    "no upcoming results date"
                } else {
                    "not enough data"
                };
                println!(
                    "  {} {} {} — {}, skipped",
                    "?".yellow(),
                    alert.symbol.cyan(),
                    condition,
                    why
                );
            }
            continue;
        };
        let hit = reading.hit;
        let price = d
            .price
            .filter(|p| p.is_finite() && *p > 0.0)
            .or_else(|| d.bars.as_ref().and_then(|b| b.close.last().copied()));
        let currency = d.currency.clone();
        let reading_text = alert.reading_text(&reading);
        let previous = state.fired.get(&key).copied();
        let fire = state.should_notify_with(&key, hit, today, alert.condition.per_session());
        if verbose {
            let status = if fire {
                "TRIGGERED".red().bold().to_string()
            } else if hit {
                "triggered (already notified)".yellow().to_string()
            } else {
                "waiting".dimmed().to_string()
            };
            println!(
                "  {} {} | Now: {}{} | {}",
                alert.symbol.cyan(),
                alert.describe(crate::i18n::lang()),
                price.map_or_else(
                    || "N/A".to_string(),
                    |p| format_price(p, currency.as_deref())
                ),
                reading_text
                    .as_deref()
                    .map(|r| format!(" | {r}"))
                    .unwrap_or_default(),
                status
            );
        }
        if fire {
            pass.triggers.push(Trigger {
                key,
                previous,
                symbol: alert.symbol.clone(),
                condition,
                price,
                currency,
                reading: reading_text,
            });
        }
    }
    Ok(pass)
}

/// `alert watch`: check alerts every `interval` seconds while their market
/// is open; notify each trigger once. `once` runs a single pass over every
/// alert regardless of market hours (for cron).
pub async fn cmd_alert_watch(interval: u64, once: bool, market: Market) -> Result<()> {
    let interval = Duration::from_secs(interval);
    let notifier = Notifier::from_env();
    let mut state: WatchState = crate::store::load(WATCH_STATE)?;
    let mut feed = Feed::default();

    if once {
        print_header("Alert Watch — single pass");
        let now = Utc::now();
        let pass = check_alerts(&mut feed, &mut state, market, now, true, true).await?;
        if pass.total == 0 {
            println!("  {}", NO_ALERTS.dimmed());
        } else {
            if pass.triggers.is_empty() {
                println!("\n  {}", "No new triggers.".dimmed());
            }
            report_triggers(&pass.triggers, &mut state, market, &notifier).await;
        }
        crate::store::save(WATCH_STATE, &state, false)?;
        println!();
        return Ok(());
    }

    let mut shutdown = Shutdown::install("Alert state is saved after every check.");
    print_header("Alert Watch");
    println!(
        "  Checking every {}s while each alert's market is open (NSE 09:15–15:30 IST, NYSE 09:30–16:00 ET).",
        interval.as_secs()
    );
    let channels = notifier.config().channels();
    if channels.is_empty() {
        println!(
            "  {} No notification channels configured — triggers print here only. See {}.",
            "→".dimmed(),
            "stockwise notify".cyan()
        );
    } else {
        let names: Vec<String> = channels.iter().map(Channel::to_string).collect();
        println!("  Notifications: {}", names.join(", "));
    }
    println!("  {} Press Ctrl+C to stop.\n", "→".dimmed());

    let mut passes = 0u32;
    let mut idle_note: Option<String> = None;
    loop {
        if shutdown.requested() {
            println!("\n  {} Alert watch stopped.", "✓".green());
            return Ok(());
        }
        let now = Utc::now();
        let pass = tokio::select! {
            r = check_alerts(&mut feed, &mut state, market, now, false, false) => Some(r),
            _ = shutdown.wait() => None,
        };
        let Some(pass) = pass else { continue };
        let wait = match pass {
            Ok(pass) => {
                feed.succeeded();
                report_triggers(&pass.triggers, &mut state, market, &notifier).await;
                if let Err(e) = crate::store::save(WATCH_STATE, &state, false) {
                    eprintln!("  {} Could not save {}: {:#}", "!".yellow(), WATCH_STATE, e);
                }
                if pass.total == 0 {
                    note_once(
                        &mut idle_note,
                        "No alerts yet — add one with `stockwise alert add`; watching for new ones.",
                    );
                    interval
                } else if pass.due == 0 {
                    let next = pass
                        .markets
                        .iter()
                        .filter_map(|m| {
                            clock::next_open(*m, now).map(|t| clock::format_local(*m, t))
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    note_once(
                        &mut idle_note,
                        &format!("Markets closed — next open: {}.", next),
                    );
                    wait_for_open(&pass.markets, now, interval)
                } else {
                    idle_note = None;
                    passes += 1;
                    if passes.is_multiple_of(15) {
                        println!(
                            "  {} [{}] {} alert(s) checked, all quiet",
                            "●".dimmed(),
                            clock::local(market, now).format("%H:%M:%S"),
                            pass.due
                        );
                    }
                    interval
                }
            }
            Err(e) => {
                let wait = feed.failed();
                eprintln!(
                    "  {} [{}] Check failed: {:#} — retrying in {}s",
                    "!".yellow(),
                    clock::local(market, Utc::now()).format("%H:%M:%S"),
                    e,
                    wait.as_secs()
                );
                wait
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(wait) => {}
            _ = shutdown.wait() => {}
        }
    }
}

fn note_once(last: &mut Option<String>, note: &str) {
    if last.as_deref() != Some(note) {
        println!("  {} {}", "→".dimmed(), note);
        *last = Some(note.to_string());
    }
}

/// Print and send the triggers. If every configured channel failed, the
/// triggers are un-marked so the next check tries again.
async fn report_triggers(
    triggers: &[Trigger],
    state: &mut WatchState,
    market: Market,
    notifier: &Notifier,
) {
    if triggers.is_empty() {
        return;
    }
    let now = clock::local(market, Utc::now()).format("%H:%M:%S");
    for t in triggers {
        let price = t
            .price
            .map(|p| format!(" — now {}", format_price(p, t.currency.as_deref()).bold()))
            .unwrap_or_default();
        let reading = t
            .reading
            .as_deref()
            .map(|r| format!(" ({r})"))
            .unwrap_or_default();
        println!(
            "  {} [{}] {} {}{}{}",
            "⚠".yellow().bold(),
            now,
            t.symbol.cyan().bold(),
            t.condition,
            price,
            reading
        );
    }
    if let Some(msg) = trigger_message(triggers)
        && !notifier.send_logged(&msg).await
    {
        for t in triggers {
            state.undo(&t.key, t.previous);
        }
        eprintln!("  ! Not delivered — will try again at the next check.");
    }
}

/// `notify` (status) and `notify test`.
pub async fn cmd_notify(action: &str) -> Result<()> {
    let notifier = Notifier::from_env();
    let cfg = notifier.config();
    print_header("Notifications");
    for channel in [
        Channel::Telegram,
        Channel::Slack,
        Channel::Discord,
        Channel::Email,
        Channel::Desktop,
    ] {
        match cfg.destination(channel) {
            Some(dest) if cfg.channels().contains(&channel) => {
                println!("  {} {:<9} {}", "✓".green(), channel, dest)
            }
            _ => println!(
                "  {} {:<9} {}",
                "·".dimmed(),
                channel,
                "not configured".dimmed()
            ),
        }
    }
    for p in &cfg.problems {
        println!("  {} {}", "!".yellow(), p);
    }

    if cfg.channels().is_empty() {
        if action == "test" && !cfg.problems.is_empty() {
            bail!("No channel is fully configured — fix the problems above.");
        }
        println!("\n  Set any of these environment variables to turn a channel on:\n");
        for (var, what) in crate::notify::ENV_VARS {
            println!("    {:<28} {}", var.bold(), what.dimmed());
        }
        println!(
            "\n  Then run {} to send a test message. Alerts ({}) and the intraday daemon use every configured channel.\n",
            "stockwise notify test".cyan(),
            "stockwise alert watch".cyan()
        );
        return Ok(());
    }
    if action != "test" {
        println!(
            "\n  {} Run {} to send a test message.\n",
            "→".dimmed(),
            "stockwise notify test".cyan()
        );
        return Ok(());
    }

    println!();
    let results = notifier
        .send(&Message::new(
            "StockWise test notification",
            "If you can read this, StockWise can reach you here.",
        ))
        .await;
    let mut failed = 0;
    for (channel, result) in &results {
        match result {
            Ok(()) => println!("  {} {:<9} sent", "✓".green(), channel),
            Err(e) => {
                failed += 1;
                println!("  {} {:<9} {:#}", "✗".red(), channel, e);
            }
        }
    }
    println!();
    if failed > 0 {
        bail!("{} of {} channel(s) failed.", failed, results.len());
    }
    if !cfg.problems.is_empty() {
        bail!("Sent, but some channels are misconfigured — see the problems above.");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    #[test]
    fn an_alert_notifies_once_until_it_rearms() {
        let mut s = WatchState::default();
        assert!(!s.should_notify("a", false, day(21)));
        assert!(s.should_notify("a", true, day(21)));
        // Still above the level: no repeat.
        assert!(!s.should_notify("a", true, day(21)));
        assert!(!s.should_notify("a", true, day(22)));
        // Dips back, crosses again the same day: still quiet (once a day).
        assert!(!s.should_notify("a", false, day(22)));
        assert!(s.should_notify("a", true, day(23)));
        assert!(!s.should_notify("a", false, day(23)));
        assert!(!s.should_notify("a", true, day(23)));
    }

    #[test]
    fn per_session_alerts_rearm_each_session() {
        let mut s = WatchState::default();
        // +3% all of day 21: one notification.
        assert!(s.should_notify_with("c", true, day(21), true));
        assert!(!s.should_notify_with("c", true, day(21), true));
        // Up 3% again on the 22nd, never seen false in between: notifies.
        assert!(s.should_notify_with("c", true, day(22), true));
        assert!(!s.should_notify_with("c", true, day(22), true));
        // A level alert in the same spot stays quiet until it re-arms.
        assert!(s.should_notify("l", true, day(21)));
        assert!(!s.should_notify("l", true, day(22)));
    }

    #[test]
    fn an_undelivered_trigger_fires_again() {
        let mut s = WatchState::default();
        let previous = s.fired.get("a").copied();
        assert!(s.should_notify("a", true, day(21)));
        s.undo("a", previous);
        assert!(s.should_notify("a", true, day(21)));
        // Undo restores a re-armed entry too.
        s.should_notify("a", false, day(22));
        let previous = s.fired.get("a").copied();
        assert!(s.should_notify("a", true, day(23)));
        s.undo("a", previous);
        assert!(s.should_notify("a", true, day(23)));
    }

    #[test]
    fn long_intervals_do_not_break_the_idle_wait() {
        let sat = clock::at(
            Market::In,
            day(24),
            chrono::NaiveTime::from_hms_opt(9, 10, 0).unwrap(),
        );
        assert_eq!(
            wait_for_open(&[Market::In], sat, Duration::from_secs(3600)),
            MAX_IDLE_WAIT
        );
    }

    #[test]
    fn watch_keys_of_old_alerts_do_not_change() {
        let mut store = AlertStore::default();
        store.add("TCS.NS", AlertCondition::Above, 4200.0);
        store.add("TCS.NS", AlertCondition::RsiBelow, 30.0);
        let created = store.alerts[0].created_at.clone();
        assert_eq!(
            alert_key(&store.alerts[0]),
            format!("TCS.NS|above|4200|{created}")
        );
        assert_eq!(
            alert_key(&store.alerts[1]),
            format!("TCS.NS|rsi-below|30|{created}|14")
        );
    }

    #[test]
    fn removed_alerts_are_forgotten() {
        let mut s = WatchState::default();
        s.should_notify("a", true, day(21));
        s.should_notify("b", true, day(21));
        s.retain(&["b".to_string()]);
        assert!(s.fired.contains_key("b") && !s.fired.contains_key("a"));
        // A re-added alert with the same key fires again.
        assert!(s.should_notify("a", true, day(21)));
    }

    #[test]
    fn alerts_are_due_only_while_their_market_is_open() {
        // 10:00 IST on Wed 21 Oct 2026 = 00:30 ET: NSE open, NYSE closed.
        let t = clock::at(
            Market::In,
            day(21),
            chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
        );
        assert!(is_due("TCS.NS", t));
        assert!(!is_due("AAPL", t));
        assert!(is_due("BTC-USD", t));
        // Dussehra: NSE closed all day.
        let holiday = clock::at(
            Market::In,
            day(20),
            chrono::NaiveTime::from_hms_opt(10, 0, 0).unwrap(),
        );
        assert!(!is_due("TCS.NS", holiday));
    }

    #[test]
    fn closed_markets_sleep_until_the_open_within_bounds() {
        let interval = Duration::from_secs(60);
        // 09:10 IST → 5 minutes to the NSE open.
        let t = clock::at(
            Market::In,
            day(21),
            chrono::NaiveTime::from_hms_opt(9, 10, 0).unwrap(),
        );
        assert_eq!(
            wait_for_open(&[Market::In], t, interval),
            Duration::from_secs(300)
        );
        // Saturday: capped at 30 minutes.
        let sat = clock::at(
            Market::In,
            day(24),
            chrono::NaiveTime::from_hms_opt(9, 10, 0).unwrap(),
        );
        assert_eq!(
            wait_for_open(&[Market::In, Market::Us], sat, interval),
            Duration::from_secs(1800)
        );
    }

    #[test]
    fn several_triggers_become_one_message() {
        let t = |s: &str| Trigger {
            key: s.into(),
            previous: None,
            symbol: s.into(),
            condition: "above 100.00".into(),
            price: Some(101.5),
            currency: Some("INR".into()),
            reading: None,
        };
        let one = trigger_message(&[t("TCS.NS")]).unwrap();
        assert_eq!(
            one.title,
            "StockWise alert: TCS.NS above 100.00 — now ₹101.50"
        );
        let rsi = Trigger {
            condition: "RSI(14) below 30".into(),
            reading: Some("RSI 28.4".into()),
            ..t("INFY.NS")
        };
        assert_eq!(
            trigger_message(&[rsi]).unwrap().title,
            "StockWise alert: INFY.NS RSI(14) below 30 — now ₹101.50 (RSI 28.4)"
        );
        let two = trigger_message(&[t("TCS.NS"), t("INFY.NS")]).unwrap();
        assert_eq!(two.title, "StockWise: 2 alerts triggered");
        assert_eq!(two.body.lines().count(), 2);
        assert!(trigger_message(&[]).is_none());
    }
}
