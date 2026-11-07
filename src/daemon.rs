//! Intraday worker state: positions, the risk manager, the day's persisted
//! state (`daemon_state.json`), and the shutdown / retry plumbing the
//! `daemon` and `alert watch` loops share.
//!
//! The worker is paper-only: nothing in here talks to a broker.

use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;

pub use crate::clock::Phase;

/// Live position tracked by the daemon
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivePosition {
    pub symbol: String,
    pub direction: String,
    pub entry_price: f64,
    pub current_price: f64,
    pub target1: f64,
    pub target2: f64,
    pub stop_loss: f64,
    pub trailing_stop: f64,
    pub qty: u32,
    pub t1_booked: bool,    // has T1 been partially booked?
    pub remaining_qty: u32, // qty left after partial booking
    pub booked_pnl: f64,    // realized P&L locked in on the booked half at T1
    pub pnl: f64,
    pub pnl_pct: f64,
    pub status: PositionStatus,
    pub strategies: Vec<String>,
    pub score: f64,
    /// When the position was opened (used to replay bars from that point).
    #[serde(default)]
    pub entered_at: Option<DateTime<Utc>>,
    /// Stop-loss at entry, before breakeven / wind-down tightening.
    #[serde(default)]
    pub initial_stop: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionStatus {
    Open,
    T1Hit,      // partial booked
    T2Hit,      // fully closed at target
    StopHit,    // stopped out
    SquaredOff, // forced close at EOD
}

impl std::fmt::Display for PositionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PositionStatus::Open => write!(f, "OPEN"),
            PositionStatus::T1Hit => write!(f, "LEVEL 1"),
            PositionStatus::T2Hit => write!(f, "LEVEL 2"),
            PositionStatus::StopHit => write!(f, "EXIT LEVEL"),
            PositionStatus::SquaredOff => write!(f, "SQUARED OFF"),
        }
    }
}

/// Risk manager state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RiskState {
    pub capital: f64,
    pub daily_loss_limit_pct: f64,
    pub max_positions: usize,
    pub max_position_pct: f64,
    pub realized_pnl: f64,
    pub killed: bool, // kill switch triggered
}

impl RiskState {
    pub fn new(capital: f64) -> Self {
        Self {
            capital,
            daily_loss_limit_pct: 3.0,
            max_positions: 5,
            max_position_pct: 30.0,
            realized_pnl: 0.0,
            killed: false,
        }
    }

    pub fn check_daily_limit(&mut self, unrealized_pnl: f64) -> bool {
        let total = self.realized_pnl + unrealized_pnl;
        let limit = self.capital * (self.daily_loss_limit_pct / 100.0);
        if total < -limit {
            self.killed = true;
            true // limit breached
        } else {
            false
        }
    }

    pub fn can_enter(&self, positions_count: usize) -> bool {
        !self.killed && positions_count < self.max_positions
    }

    pub fn record_exit(&mut self, pnl: f64) {
        self.realized_pnl += pnl;
    }
}

/// Total (booked plus open) P&L as a percentage of the entry cost
/// (`entry_price * qty`). Zero when nothing was committed.
fn pnl_pct_of_cost(pos: &LivePosition) -> f64 {
    let cost = pos.entry_price * pos.qty as f64;
    if cost > 0.0 && cost.is_finite() {
        pos.pnl / cost * 100.0
    } else {
        0.0
    }
}

/// Update a position with new price, handle T1/stop/trailing
pub fn update_position(pos: &mut LivePosition, new_price: f64, phase: Phase) {
    pos.current_price = new_price;
    let is_long = crate::intraday::Direction::is_long_str(&pos.direction);

    // Calculate P&L = realized (booked half) + open leg on the remaining qty.
    let leg = if is_long {
        (new_price - pos.entry_price) * pos.remaining_qty as f64
    } else {
        (pos.entry_price - new_price) * pos.remaining_qty as f64
    };
    pos.pnl = pos.booked_pnl + leg;
    // Return on the capital committed at entry: booked plus open P&L over the
    // full entry cost, so a partly booked position is not valued as if all of
    // it were still open at the current price.
    pos.pnl_pct = pnl_pct_of_cost(pos);

    if pos.status == PositionStatus::T2Hit
        || pos.status == PositionStatus::StopHit
        || pos.status == PositionStatus::SquaredOff
    {
        return; // already closed
    }

    // Check stop loss
    let stopped = if is_long {
        new_price <= pos.stop_loss
    } else {
        new_price >= pos.stop_loss
    };
    if stopped {
        pos.status = PositionStatus::StopHit;
        return;
    }

    // Check trailing stop
    let trail_stopped = if is_long {
        new_price <= pos.trailing_stop
    } else {
        new_price >= pos.trailing_stop
    };
    if trail_stopped && pos.t1_booked {
        pos.status = PositionStatus::StopHit; // trailing stop after T1
        return;
    }

    // Check T2
    let t2_hit = if is_long {
        new_price >= pos.target2
    } else {
        new_price <= pos.target2
    };
    if t2_hit {
        pos.status = PositionStatus::T2Hit;
        return;
    }

    // Check T1 (partial book)
    let t1_hit = if is_long {
        new_price >= pos.target1
    } else {
        new_price <= pos.target1
    };
    if t1_hit && !pos.t1_booked {
        pos.t1_booked = true;
        let book_qty = pos.qty / 2;
        pos.remaining_qty = pos.qty - book_qty;
        // Lock in realized P&L on the booked half (at target1).
        pos.booked_pnl += if is_long {
            (pos.target1 - pos.entry_price) * book_qty as f64
        } else {
            (pos.entry_price - pos.target1) * book_qty as f64
        };
        // Move stop to breakeven after T1, but only if it tightens the stop
        // (entry is the correct side for both longs and shorts).
        if is_long {
            if pos.entry_price > pos.stop_loss {
                pos.stop_loss = pos.entry_price;
            }
        } else if pos.entry_price < pos.stop_loss {
            pos.stop_loss = pos.entry_price;
        }
        pos.status = PositionStatus::T1Hit;
        // Only the remaining half is still open at the new price.
        pos.pnl = pos.booked_pnl
            + if is_long {
                (new_price - pos.entry_price) * pos.remaining_qty as f64
            } else {
                (pos.entry_price - new_price) * pos.remaining_qty as f64
            };
        pos.pnl_pct = pnl_pct_of_cost(pos);
    }

    // Update trailing stop (only moves in favorable direction)
    if is_long {
        let atr_trail = new_price * 0.015; // ~1.5% trail
        let new_trail = new_price - atr_trail;
        if new_trail > pos.trailing_stop {
            pos.trailing_stop = new_trail;
        }
    } else {
        let atr_trail = new_price * 0.015;
        let new_trail = new_price + atr_trail;
        if new_trail < pos.trailing_stop {
            pos.trailing_stop = new_trail;
        }
    }

    // Wind-down: tighten stops on both sides
    if phase == Phase::WindDown {
        if is_long {
            let tight = new_price * 0.995; // 0.5% below for longs
            if tight > pos.stop_loss {
                pos.stop_loss = tight;
            }
        } else {
            let tight = new_price * 1.005; // 0.5% above for shorts
            if tight < pos.stop_loss {
                pos.stop_loss = tight;
            }
        }
    }

    // Square-off phase: force close
    if phase == Phase::SquareOff {
        pos.status = PositionStatus::SquaredOff;
    }
}

/// Convert intraday trade plans to live positions opened at `entered_at`.
pub fn plans_to_positions(
    plans: &[crate::intraday::TradePlan],
    entered_at: DateTime<Utc>,
) -> Vec<LivePosition> {
    plans
        .iter()
        .map(|p| LivePosition {
            symbol: p.signal.symbol.clone(),
            direction: p.signal.direction.to_string(),
            entry_price: p.entry,
            current_price: p.entry,
            target1: p.target1,
            target2: p.target2,
            stop_loss: p.stop_loss,
            trailing_stop: p.trailing_stop,
            qty: p.qty,
            t1_booked: false,
            remaining_qty: p.qty,
            booked_pnl: 0.0,
            pnl: 0.0,
            pnl_pct: 0.0,
            status: PositionStatus::Open,
            strategies: p
                .signal
                .strategies
                .iter()
                .map(|s| s.name.to_string())
                .collect(),
            score: p.signal.score,
            entered_at: Some(entered_at),
            initial_stop: Some(p.stop_loss),
        })
        .collect()
}

impl PositionStatus {
    /// Closed positions take no more price updates.
    pub fn is_closed(self) -> bool {
        matches!(
            self,
            PositionStatus::T2Hit | PositionStatus::StopHit | PositionStatus::SquaredOff
        )
    }
}

impl LivePosition {
    pub fn is_open(&self) -> bool {
        !self.status.is_closed()
    }
}

/// Something that happened in the day's session. The worker prints it and
/// sends it to the configured notification channels.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Entered {
        symbol: String,
        direction: String,
        qty: u32,
        entry: f64,
        target1: f64,
        target2: f64,
        stop: f64,
    },
    T1Booked {
        symbol: String,
        price: f64,
        booked_pnl: f64,
    },
    Closed {
        symbol: String,
        status: PositionStatus,
        price: f64,
        pnl: f64,
    },
    KillSwitch {
        loss: f64,
        limit: f64,
    },
}

/// `+₹120` / `-₹45` without colours.
pub fn signed_money(csym: &str, v: f64) -> String {
    if v >= 0.0 {
        format!("+{}{:.0}", csym, v)
    } else {
        format!("-{}{:.0}", csym, v.abs())
    }
}

impl Event {
    /// Plain-text one-liner for notifications and logs.
    pub fn text(&self, csym: &str) -> String {
        match self {
            Event::Entered {
                symbol,
                direction,
                qty,
                entry,
                target1,
                target2,
                stop,
            } => format!(
                "OPENED paper {} {} x{} @ {}{:.2} (level 1 {:.2}, level 2 {:.2}, exit level {:.2})",
                crate::intraday::Direction::label_of(direction),
                symbol,
                qty,
                csym,
                entry,
                target1,
                target2,
                stop
            ),
            Event::T1Booked {
                symbol,
                price,
                booked_pnl,
            } => format!(
                "LEVEL 1 reached {} — booked half at {}{:.2} ({}), exit level moved to the opening price",
                symbol,
                csym,
                price,
                signed_money(csym, *booked_pnl)
            ),
            Event::Closed {
                symbol,
                status,
                price,
                pnl,
            } => {
                let what = match status {
                    PositionStatus::T2Hit => "LEVEL 2 reached",
                    PositionStatus::StopHit => "EXIT LEVEL reached",
                    _ => "SQUARED OFF",
                };
                format!(
                    "{} {} at {}{:.2} (P&L {})",
                    what,
                    symbol,
                    csym,
                    price,
                    signed_money(csym, *pnl)
                )
            }
            Event::KillSwitch { loss, limit } => format!(
                "KILL SWITCH — day's P&L {} breached the {}{:.0} daily loss limit; closing everything",
                signed_money(csym, *loss),
                csym,
                limit
            ),
        }
    }
}

/// File the intraday worker saves its day's state to after every tick:
/// `daemon_state.json` for NSE, `daemon_state_us.json` for NYSE and
/// `daemon_state_<code>.json` for a global exchange, so a daemon per market
/// can run side by side.
pub fn state_file(market: &str) -> &'static str {
    match market.to_ascii_uppercase().as_str() {
        "US" => "daemon_state_us.json",
        "UK" => "daemon_state_uk.json",
        "DE" => "daemon_state_de.json",
        "JP" => "daemon_state_jp.json",
        "HK" => "daemon_state_hk.json",
        "SG" => "daemon_state_sg.json",
        "CA" => "daemon_state_ca.json",
        "AU" => "daemon_state_au.json",
        _ => "daemon_state.json",
    }
}

/// One trading day of the intraday worker, persisted after every tick so a
/// restart the same day resumes the open positions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonState {
    /// Exchange-local session date.
    pub date: NaiveDate,
    /// `IN` or `US`.
    pub market: String,
    pub capital: f64,
    pub target_pct: f64,
    /// Positions were opened today (or the entry scan ran and found trades).
    pub entered: bool,
    pub positions: Vec<LivePosition>,
    pub risk: RiskState,
    pub tick_count: u32,
    /// The day is over: results are in the simulator history.
    pub settled: bool,
    pub updated_at: DateTime<Utc>,
}

impl DaemonState {
    pub fn new(
        date: NaiveDate,
        market: &str,
        capital: f64,
        target_pct: f64,
        now: DateTime<Utc>,
    ) -> Self {
        Self {
            date,
            market: market.to_string(),
            capital,
            target_pct,
            entered: false,
            positions: Vec::new(),
            risk: RiskState::new(capital),
            tick_count: 0,
            settled: false,
            updated_at: now,
        }
    }

    /// The saved state for a market (`IN` / `US`), if any.
    pub fn load(market: &str) -> anyhow::Result<Option<Self>> {
        crate::store::load(state_file(market))
    }

    pub fn save(&mut self, now: DateTime<Utc>) -> anyhow::Result<()> {
        self.updated_at = now;
        crate::store::save(state_file(&self.market), self, false)
    }

    pub fn open_count(&self) -> usize {
        self.positions.iter().filter(|p| p.is_open()).count()
    }

    pub fn open_symbols(&self) -> Vec<String> {
        self.positions
            .iter()
            .filter(|p| p.is_open())
            .map(|p| p.symbol.clone())
            .collect()
    }

    pub fn total_pnl(&self) -> f64 {
        self.positions.iter().map(|p| p.pnl).sum()
    }

    /// Open the day's positions from the (already capital-capped) plans.
    pub fn enter(
        &mut self,
        plans: &[crate::intraday::TradePlan],
        now: DateTime<Utc>,
    ) -> Vec<Event> {
        let n = plans.len().min(self.risk.max_positions);
        self.positions = plans_to_positions(&plans[..n], now);
        self.entered = true;
        self.positions
            .iter()
            .map(|p| Event::Entered {
                symbol: p.symbol.clone(),
                direction: p.direction.clone(),
                qty: p.qty,
                entry: p.entry_price,
                target1: p.target1,
                target2: p.target2,
                stop: p.stop_loss,
            })
            .collect()
    }

    /// Mark open positions to `prices`, book exits with the risk manager,
    /// and trip the kill switch if the day's loss passes the limit. Symbols
    /// missing from `prices` keep their last price — never valued at 0.
    pub fn apply_prices(&mut self, prices: &HashMap<String, f64>, phase: Phase) -> Vec<Event> {
        let mut events = Vec::new();
        for pos in self.positions.iter_mut().filter(|p| p.is_open()) {
            let Some(&price) = prices.get(&pos.symbol) else {
                continue;
            };
            let was_t1 = pos.t1_booked;
            update_position(pos, price, phase);
            if pos.t1_booked && !was_t1 {
                events.push(Event::T1Booked {
                    symbol: pos.symbol.clone(),
                    price: pos.target1,
                    booked_pnl: pos.booked_pnl,
                });
            }
            if pos.status.is_closed() {
                self.risk.record_exit(pos.pnl);
                events.push(Event::Closed {
                    symbol: pos.symbol.clone(),
                    status: pos.status,
                    price: pos.current_price,
                    pnl: pos.pnl,
                });
            }
        }

        if !self.risk.killed {
            let unrealized: f64 = self
                .positions
                .iter()
                .filter(|p| p.is_open())
                .map(|p| p.pnl)
                .sum();
            if self.risk.check_daily_limit(unrealized) {
                events.push(Event::KillSwitch {
                    loss: self.risk.realized_pnl + unrealized,
                    limit: self.risk.capital * self.risk.daily_loss_limit_pct / 100.0,
                });
                events.extend(self.square_off_all());
            }
        }
        events
    }

    /// Close every open position at its last price (paper square-off).
    pub fn square_off_all(&mut self) -> Vec<Event> {
        let mut events = Vec::new();
        for pos in self.positions.iter_mut().filter(|p| p.is_open()) {
            pos.status = PositionStatus::SquaredOff;
            self.risk.record_exit(pos.pnl);
            events.push(Event::Closed {
                symbol: pos.symbol.clone(),
                status: pos.status,
                price: pos.current_price,
                pnl: pos.pnl,
            });
        }
        events
    }

    /// The day as a simulator session. `settled` sessions carry exits and
    /// P&L; unsettled ones carry only the entries (with the original stop),
    /// for `sim settle` to replay from intraday bars.
    pub fn to_sim_session(&self, settled: bool) -> crate::simulator::SimSession {
        let trades: Vec<crate::simulator::SimTrade> = self
            .positions
            .iter()
            .map(|p| crate::simulator::SimTrade {
                symbol: p.symbol.clone(),
                direction: p.direction.clone(),
                entry_price: p.entry_price,
                target1: p.target1,
                target2: p.target2,
                stop_loss: if settled {
                    p.stop_loss
                } else {
                    p.initial_stop.unwrap_or(p.stop_loss)
                },
                qty: p.qty,
                capital: p.entry_price * p.qty as f64,
                score: p.score,
                confidence: "DAEMON".into(),
                strategies: p.strategies.clone(),
                entered_at: p.entered_at.map(|t| t.to_rfc3339()),
                exit_price: settled.then_some(p.current_price),
                pnl: settled.then_some(p.pnl),
                pnl_pct: settled.then_some(p.pnl_pct),
                hit_target: settled.then_some(p.status == PositionStatus::T2Hit),
                hit_stop: settled.then_some(p.status == PositionStatus::StopHit),
            })
            .collect();
        let total_pnl = self.total_pnl();
        let wins = self.positions.iter().filter(|p| p.pnl > 0.0).count() as u32;
        crate::simulator::SimSession {
            date: self.date.format("%Y-%m-%d").to_string(),
            market: self.market.clone(),
            capital: self.capital,
            target_pct: self.target_pct,
            total_pnl: settled.then_some(total_pnl),
            total_pnl_pct: settled.then_some(total_pnl / self.capital * 100.0),
            win_count: settled.then_some(wins),
            loss_count: settled.then_some(trades.len() as u32 - wins),
            trades,
            settled,
        }
    }
}

/// Add a daemon session to the simulator history unless the same day's
/// daemon session is already there (a crash between the two saves must not
/// record the day twice). Returns false if it was already recorded.
pub fn record_session(
    history: &mut crate::simulator::SimHistory,
    session: crate::simulator::SimSession,
) -> bool {
    let symbols: Vec<&str> = session.trades.iter().map(|t| t.symbol.as_str()).collect();
    let duplicate = history.sessions.iter().any(|s| {
        s.date == session.date
            && s.market == session.market
            && s.trades.iter().all(|t| t.confidence == "DAEMON")
            && s.trades
                .iter()
                .map(|t| t.symbol.as_str())
                .eq(symbols.iter().copied())
    });
    if !duplicate {
        history.sessions.push(session);
    }
    !duplicate
}

/// Delay before retrying after `failures` consecutive failed ticks:
/// 15 s, 30 s, 60 s, … capped at 5 minutes. A worker never gives up.
pub fn retry_delay(failures: u32) -> Duration {
    let secs = 15u64 << failures.saturating_sub(1).min(5);
    Duration::from_secs(secs.min(300))
}

/// Ctrl+C (and SIGTERM on Unix) as a flag the worker loops poll and await.
/// The first signal asks for a clean stop; a second one exits immediately
/// (state is already on disk from the last tick).
pub struct Shutdown {
    rx: tokio::sync::watch::Receiver<bool>,
}

impl Shutdown {
    pub fn install(second_signal_note: &'static str) -> Self {
        let (tx, rx) = tokio::sync::watch::channel(false);
        tokio::spawn(async move {
            wait_for_signal().await;
            let _ = tx.send(true);
            wait_for_signal().await;
            eprintln!(
                "\n  Interrupted again — exiting now. {}",
                second_signal_note
            );
            std::process::exit(130);
        });
        Self { rx }
    }

    pub fn requested(&self) -> bool {
        *self.rx.borrow()
    }

    /// Resolves once a stop was requested.
    pub async fn wait(&mut self) {
        let _ = self.rx.wait_for(|stop| *stop).await;
    }
}

async fn wait_for_signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {}
                    _ = term.recv() => {}
                }
            }
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_position(entry: f64, t1: f64, t2: f64, stop: f64) -> LivePosition {
        LivePosition {
            symbol: "TEST.NS".into(),
            direction: "BUY".into(),
            entry_price: entry,
            current_price: entry,
            target1: t1,
            target2: t2,
            stop_loss: stop,
            trailing_stop: entry - 5.0,
            qty: 10,
            t1_booked: false,
            remaining_qty: 10,
            booked_pnl: 0.0,
            pnl: 0.0,
            pnl_pct: 0.0,
            status: PositionStatus::Open,
            strategies: vec!["RSI".into()],
            score: 75.0,
            entered_at: None,
            initial_stop: None,
        }
    }

    /// State files written before 0.8.1 store `BUY` / `SELL`. They must load,
    /// act as long / short, and show as long / short.
    #[test]
    fn old_state_files_with_buy_and_sell_still_load() {
        let mut long = serde_json::to_value(make_position(100.0, 103.0, 106.0, 97.0)).unwrap();
        long["direction"] = "BUY".into();
        let mut short = serde_json::to_value(make_position(100.0, 97.0, 94.0, 103.0)).unwrap();
        short["direction"] = "SELL".into();
        let mut long: LivePosition = serde_json::from_value(long).unwrap();
        let mut short: LivePosition = serde_json::from_value(short).unwrap();
        update_position(&mut long, 96.0, Phase::Active);
        assert_eq!(long.status, PositionStatus::StopHit);
        update_position(&mut short, 104.0, Phase::Active);
        assert_eq!(short.status, PositionStatus::StopHit);
        assert_eq!(
            crate::intraday::Direction::label_of(&long.direction),
            "long"
        );
        assert_eq!(
            crate::intraday::Direction::label_of(&short.direction),
            "short"
        );
        // New files use the new spelling and behave the same.
        let mut new = make_position(100.0, 103.0, 106.0, 97.0);
        new.direction = crate::intraday::Direction::Long.to_string();
        update_position(&mut new, 96.0, Phase::Active);
        assert_eq!(new.status, PositionStatus::StopHit);
        assert!(serde_json::to_string(&new).unwrap().contains("\"long\""));
    }

    /// Screens and notifications describe paper positions without a call.
    #[test]
    fn event_text_has_no_buy_sell_entry_target_or_stop_wording() {
        let events = [
            Event::Entered {
                symbol: "TCS.NS".into(),
                direction: "BUY".into(),
                qty: 10,
                entry: 100.0,
                target1: 102.0,
                target2: 104.0,
                stop: 98.0,
            },
            Event::T1Booked {
                symbol: "TCS.NS".into(),
                price: 102.0,
                booked_pnl: 10.0,
            },
            Event::Closed {
                symbol: "TCS.NS".into(),
                status: PositionStatus::T2Hit,
                price: 104.0,
                pnl: 30.0,
            },
            Event::Closed {
                symbol: "TCS.NS".into(),
                status: PositionStatus::StopHit,
                price: 98.0,
                pnl: -20.0,
            },
        ];
        let mut text = events
            .iter()
            .map(|e| e.text("₹"))
            .collect::<Vec<_>>()
            .join("\n");
        for s in [
            PositionStatus::Open,
            PositionStatus::T1Hit,
            PositionStatus::T2Hit,
            PositionStatus::StopHit,
            PositionStatus::SquaredOff,
        ] {
            text.push_str(&format!("\n{s}"));
        }
        crate::verdict::testing::assert_neutral(&text);
        for banned in ["ENTRY", "SL ", "T1", "T2", "STOPPED", "TARGET"] {
            assert!(!text.contains(banned), "{banned:?} in {text}");
        }
        assert!(text.contains("long"));
    }

    #[test]
    fn test_stop_loss_hit() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 96.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::StopHit);
    }

    #[test]
    fn test_target1_partial_book() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 103.5, Phase::Active);
        assert_eq!(pos.status, PositionStatus::T1Hit);
        assert!(pos.t1_booked);
        assert_eq!(pos.remaining_qty, 5); // 50% booked
        assert_eq!(pos.stop_loss, 100.0); // moved to breakeven
    }

    #[test]
    fn test_target2_full_close() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 107.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::T2Hit);
    }

    #[test]
    fn test_trailing_stop_moves_up() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        let initial_trail = pos.trailing_stop;
        update_position(&mut pos, 102.0, Phase::Active);
        assert!(
            pos.trailing_stop > initial_trail,
            "Trailing stop should move up"
        );
    }

    #[test]
    fn test_trailing_stop_doesnt_move_down() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 102.0, Phase::Active);
        let trail_after_up = pos.trailing_stop;
        update_position(&mut pos, 101.0, Phase::Active);
        assert_eq!(
            pos.trailing_stop, trail_after_up,
            "Trailing stop should not move down"
        );
    }

    #[test]
    fn test_square_off_phase() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 101.0, Phase::SquareOff);
        assert_eq!(pos.status, PositionStatus::SquaredOff);
    }

    #[test]
    fn test_pnl_calculation() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 102.0, Phase::Active);
        assert!((pos.pnl - 20.0).abs() < 0.01); // 10 shares * 2.0
        assert!((pos.pnl_pct - 2.0).abs() < 0.01);
        // 105 crosses T1 (103): 5 booked at 103 (+15) + 5 open at 105 (+25).
        update_position(&mut pos, 105.0, Phase::Active);
        assert!((pos.pnl - 40.0).abs() < 0.01);
        // Total 40 on a cost of 10 * 100 = 1000 → 4.0%, not (105-100)/100.
        assert!((pos.pnl_pct - 4.0).abs() < 0.01);
    }

    #[test]
    fn pnl_pct_counts_the_booked_half_for_a_long() {
        // 100 shares at 100; T1 at 110 books 50 (+500); the rest is marked at
        // 105 (+250). Total 750 on a cost of 10,000 = +7.5% (the old formula
        // gave +5.0%).
        let mut pos = make_position(100.0, 110.0, 130.0, 90.0);
        pos.qty = 100;
        pos.remaining_qty = 100;
        pos.trailing_stop = 50.0;
        update_position(&mut pos, 110.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::T1Hit);
        assert!((pos.booked_pnl - 500.0).abs() < 1e-9);
        // The tick that books T1 already reports the combined return:
        // 500 + 50 * 10 = 1000 → 10.0%.
        assert!((pos.pnl_pct - 10.0).abs() < 1e-9);
        update_position(&mut pos, 105.0, Phase::Active);
        assert!((pos.pnl - 750.0).abs() < 1e-9);
        assert!((pos.pnl_pct - 7.5).abs() < 1e-9);
    }

    #[test]
    fn pnl_pct_stays_positive_when_a_booked_position_retraces() {
        // Issue case: 10 @ 100, T1 103 books 5 (+15); price falls to 99
        // (open leg -5). Total +10 on 1,000 = +1.0%, not -1.0%.
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 103.0, Phase::Active);
        update_position(&mut pos, 99.0, Phase::Active);
        assert!((pos.pnl - 10.0).abs() < 1e-9);
        assert!((pos.pnl_pct - 1.0).abs() < 1e-9);
    }

    #[test]
    fn pnl_pct_counts_the_booked_half_for_a_short() {
        // Short 10 @ 100, T1 97 books 5 (+15); rest marked at 98 (+10).
        // Total +25 on 1,000 = +2.5%.
        let mut pos = make_position(100.0, 97.0, 94.0, 103.0);
        pos.direction = "SELL".into();
        pos.trailing_stop = 110.0;
        update_position(&mut pos, 97.0, Phase::Active);
        update_position(&mut pos, 98.0, Phase::Active);
        assert!((pos.pnl - 25.0).abs() < 1e-9);
        assert!((pos.pnl_pct - 2.5).abs() < 1e-9);
    }

    #[test]
    fn test_risk_state_daily_limit() {
        let mut risk = RiskState::new(25000.0);
        assert!(!risk.killed);
        assert!(risk.can_enter(0));
        // 3% of 25000 = 750
        let breached = risk.check_daily_limit(-800.0);
        assert!(breached);
        assert!(risk.killed);
        assert!(!risk.can_enter(0));
    }

    #[test]
    fn test_risk_position_limit() {
        let risk = RiskState::new(25000.0);
        assert!(risk.can_enter(4)); // 4 < 5 max
        assert!(!risk.can_enter(5)); // 5 = max, can't enter
    }

    #[test]
    fn test_risk_record_exit() {
        let mut risk = RiskState::new(25000.0);
        risk.record_exit(200.0);
        risk.record_exit(-100.0);
        assert!((risk.realized_pnl - 100.0).abs() < 0.01);
    }

    #[test]
    fn test_closed_position_not_updated() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        pos.status = PositionStatus::T2Hit;
        update_position(&mut pos, 50.0, Phase::Active); // should be ignored
        assert_eq!(pos.status, PositionStatus::T2Hit); // still T2Hit, not StopHit
    }

    #[test]
    fn test_wind_down_tightens_stop() {
        let mut pos = make_position(100.0, 103.0, 106.0, 97.0);
        update_position(&mut pos, 102.0, Phase::WindDown);
        assert!(pos.stop_loss > 97.0, "Wind-down should tighten stop");
    }

    #[test]
    fn test_short_pnl_and_pct() {
        // Short from 100, price falls to 95 → +5/share profit.
        let mut pos = make_position(100.0, 90.0, 85.0, 103.0);
        pos.direction = "SELL".into();
        pos.trailing_stop = 105.0;
        update_position(&mut pos, 95.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::Open);
        assert!((pos.pnl - 50.0).abs() < 0.01); // 10 shares * 5.0
        assert!((pos.pnl_pct - 5.0).abs() < 0.01); // (100-95)/100
    }

    #[test]
    fn test_t1_tick_above_target1_values_only_the_open_half_at_the_price() {
        // Long 10 @ 100, T1 102: a tick at 103 books 5 at 102 (+10) and
        // leaves 5 open at 103 (+15) → 25, not 30.
        let mut pos = make_position(100.0, 102.0, 110.0, 97.0);
        update_position(&mut pos, 103.0, Phase::Active);
        assert!((pos.pnl - 25.0).abs() < 1e-9, "{}", pos.pnl);
        // Same tick at square-off closes at 25.
        let mut pos = make_position(100.0, 102.0, 110.0, 97.0);
        update_position(&mut pos, 103.0, Phase::SquareOff);
        assert_eq!(pos.status, PositionStatus::SquaredOff);
        assert!((pos.pnl - 25.0).abs() < 1e-9, "{}", pos.pnl);
    }

    #[test]
    fn test_short_wind_down_tightens_stop() {
        let mut pos = make_position(100.0, 90.0, 85.0, 103.0);
        pos.direction = "SELL".into();
        pos.trailing_stop = 105.0;
        update_position(&mut pos, 96.0, Phase::WindDown);
        assert!(
            pos.stop_loss < 103.0,
            "Wind-down should tighten a short's stop downward"
        );
    }

    #[test]
    fn test_t1_books_realized_pnl() {
        // T1 at 104 books 5 shares * (104-100) = 20 realized.
        let mut pos = make_position(100.0, 104.0, 110.0, 97.0);
        update_position(&mut pos, 104.0, Phase::Active);
        assert_eq!(pos.status, PositionStatus::T1Hit);
        assert!((pos.booked_pnl - 20.0).abs() < 0.01);
        // Next tick at 106: pnl = booked(20) + 5 remaining * (106-100)=30 → 50.
        update_position(&mut pos, 106.0, Phase::Active);
        assert!((pos.pnl - 50.0).abs() < 0.01);
    }

    // ── DaemonState ──

    fn state_with(positions: Vec<LivePosition>) -> DaemonState {
        let now = DateTime::parse_from_rfc3339("2026-10-21T04:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let mut s = DaemonState::new(
            NaiveDate::from_ymd_opt(2026, 10, 21).unwrap(),
            "IN",
            25000.0,
            2.0,
            now,
        );
        s.positions = positions;
        s.entered = true;
        s
    }

    fn prices(pairs: &[(&str, f64)]) -> HashMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn target2_close_is_recorded_with_the_risk_manager() {
        let mut s = state_with(vec![make_position(100.0, 103.0, 106.0, 97.0)]);
        let events = s.apply_prices(&prices(&[("TEST.NS", 107.0)]), Phase::Active);
        assert_eq!(s.positions[0].status, PositionStatus::T2Hit);
        assert!((s.risk.realized_pnl - 70.0).abs() < 1e-9);
        assert!(matches!(
            events.as_slice(),
            [Event::Closed {
                status: PositionStatus::T2Hit,
                ..
            }]
        ));
        // A closed position takes no more updates or exits.
        let events = s.apply_prices(&prices(&[("TEST.NS", 90.0)]), Phase::Active);
        assert!(events.is_empty());
        assert!((s.risk.realized_pnl - 70.0).abs() < 1e-9);
    }

    #[test]
    fn t1_then_t2_reports_both_and_books_the_whole_trade() {
        let mut s = state_with(vec![make_position(100.0, 104.0, 110.0, 97.0)]);
        let events = s.apply_prices(&prices(&[("TEST.NS", 104.0)]), Phase::Active);
        assert!(matches!(events.as_slice(), [Event::T1Booked { .. }]));
        assert_eq!(s.risk.realized_pnl, 0.0); // not closed yet
        s.apply_prices(&prices(&[("TEST.NS", 110.0)]), Phase::Active);
        // 5 × 4 booked at T1 + 5 × 10 at T2.
        assert!((s.risk.realized_pnl - 70.0).abs() < 1e-9);
    }

    #[test]
    fn missing_price_keeps_the_last_price() {
        let mut s = state_with(vec![make_position(100.0, 103.0, 106.0, 97.0)]);
        s.apply_prices(&prices(&[("TEST.NS", 101.0)]), Phase::Active);
        let events = s.apply_prices(&prices(&[]), Phase::Active);
        assert!(events.is_empty());
        assert_eq!(s.positions[0].current_price, 101.0);
        assert!((s.positions[0].pnl - 10.0).abs() < 1e-9);
    }

    #[test]
    fn kill_switch_closes_everything_once() {
        let mut a = make_position(100.0, 103.0, 106.0, 50.0);
        a.qty = 100;
        a.remaining_qty = 100;
        let mut b = make_position(200.0, 203.0, 206.0, 100.0);
        b.symbol = "OTHER.NS".into();
        let mut s = state_with(vec![a, b]);
        // −₹800 on A breaches 3% of ₹25,000 (₹750).
        let events = s.apply_prices(&prices(&[("TEST.NS", 92.0)]), Phase::Active);
        assert!(matches!(events[0], Event::KillSwitch { .. }));
        assert_eq!(events.len(), 3);
        assert!(s.risk.killed);
        assert_eq!(s.open_count(), 0);
        assert!((s.risk.realized_pnl + 800.0).abs() < 1e-9);
        // Later ticks don't trip it again.
        assert!(
            s.apply_prices(&prices(&[("TEST.NS", 80.0)]), Phase::Active)
                .is_empty()
        );
    }

    #[test]
    fn square_off_all_records_each_open_position_once() {
        let mut closed = make_position(100.0, 103.0, 106.0, 97.0);
        closed.symbol = "DONE.NS".into();
        closed.status = PositionStatus::StopHit;
        let mut s = state_with(vec![make_position(100.0, 103.0, 106.0, 97.0), closed]);
        s.apply_prices(&prices(&[("TEST.NS", 102.0)]), Phase::Active);
        let events = s.square_off_all();
        assert_eq!(events.len(), 1);
        assert!((s.risk.realized_pnl - 20.0).abs() < 1e-9);
        assert!(s.square_off_all().is_empty());
    }

    #[test]
    fn entries_are_capped_at_max_positions() {
        let plan = |sym: &str| crate::intraday::TradePlan {
            signal: crate::intraday::IntradaySignal {
                symbol: sym.into(),
                name: sym.into(),
                price: 100.0,
                currency: None,
                change_pct: 0.0,
                volume_ratio: 1.0,
                regime: crate::intraday::MarketRegime::Ranging,
                confidence: crate::intraday::Confidence::High,
                direction: crate::intraday::Direction::Long,
                score: 80.0,
                strategies: vec![],
                rsi: None,
                vwap: None,
                above_vwap: false,
                macd_histogram: None,
                bb_position: None,
                atr_pct: Some(2.0),
                obv_trend: crate::intraday::OBVTrend::Flat,
                gap_detected: false,
                near_support: false,
                near_resistance: false,
                pivot: None,
                support1: None,
                resistance1: None,
            },
            entry: 100.0,
            target1: 101.0,
            target2: 102.0,
            stop_loss: 99.0,
            trailing_stop: 97.0,
            qty: 10,
            capital_required: 1000.0,
            max_risk: 10.0,
            expected_profit: 20.0,
            risk_reward: 2.0,
            kelly_fraction: 0.2,
            position_pct: 4.0,
        };
        let mut s = state_with(vec![]);
        s.entered = false;
        let plans: Vec<_> = (0..7).map(|i| plan(&format!("S{i}.NS"))).collect();
        let events = s.enter(&plans, s.updated_at);
        assert_eq!(events.len(), 5);
        assert!(s.entered);
        assert_eq!(s.positions[0].initial_stop, Some(99.0));
        assert_eq!(s.positions[0].entered_at, Some(s.updated_at));
    }

    #[test]
    fn state_round_trips_through_json() {
        let mut s = state_with(vec![make_position(100.0, 103.0, 106.0, 97.0)]);
        s.apply_prices(&prices(&[("TEST.NS", 103.5)]), Phase::Active);
        let json = serde_json::to_string(&s).unwrap();
        let back: DaemonState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.date, s.date);
        assert_eq!(back.positions[0].status, PositionStatus::T1Hit);
        assert_eq!(back.positions[0].remaining_qty, 5);
        assert_eq!(back.risk.capital, 25000.0);
    }

    #[test]
    fn sim_sessions_from_the_daemon() {
        let mut s = state_with(vec![make_position(100.0, 103.0, 106.0, 97.0)]);
        s.positions[0].initial_stop = Some(97.0);
        s.apply_prices(&prices(&[("TEST.NS", 103.5)]), Phase::Active);
        s.square_off_all();

        let settled = s.to_sim_session(true);
        assert!(settled.settled);
        assert_eq!(settled.date, "2026-10-21");
        assert_eq!(settled.win_count, Some(1));
        assert_eq!(settled.trades[0].exit_price, Some(103.5));
        assert_eq!(settled.trades[0].stop_loss, 100.0); // moved to breakeven

        // Unsettled hand-over keeps the original stop for the replay.
        let open = s.to_sim_session(false);
        assert!(!open.settled);
        assert_eq!(open.trades[0].stop_loss, 97.0);
        assert_eq!(open.trades[0].exit_price, None);
        assert_eq!(open.total_pnl, None);
    }

    #[test]
    fn a_day_is_recorded_once() {
        let s = state_with(vec![make_position(100.0, 103.0, 106.0, 97.0)]);
        let mut history = crate::simulator::SimHistory::default();
        assert!(record_session(&mut history, s.to_sim_session(true)));
        assert!(!record_session(&mut history, s.to_sim_session(true)));
        assert_eq!(history.sessions.len(), 1);
    }

    #[test]
    fn each_market_has_its_own_state_file() {
        assert_eq!(state_file("IN"), "daemon_state.json");
        assert_eq!(state_file("US"), "daemon_state_us.json");
        // Every global exchange gets its own file.
        for x in crate::market::Exchange::ALL {
            let id = crate::market::Market::Global(x).id();
            assert_eq!(
                state_file(id),
                format!("daemon_state_{}.json", id.to_ascii_lowercase())
            );
        }
    }

    #[test]
    fn retry_delay_backs_off_and_is_bounded() {
        assert_eq!(retry_delay(1), Duration::from_secs(15));
        assert_eq!(retry_delay(2), Duration::from_secs(30));
        assert_eq!(retry_delay(4), Duration::from_secs(120));
        assert_eq!(retry_delay(6), Duration::from_secs(300));
        assert_eq!(retry_delay(1000), Duration::from_secs(300));
    }
}
