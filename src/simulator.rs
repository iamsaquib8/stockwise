use crate::clock;
use crate::daemon::{LivePosition, Phase, PositionStatus, update_position};
use crate::market::Market;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single simulated trade
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimTrade {
    pub symbol: String,
    pub direction: String,
    pub entry_price: f64,
    pub target1: f64,
    pub target2: f64,
    pub stop_loss: f64,
    pub qty: u32,
    pub capital: f64,
    pub score: f64,
    pub confidence: String,
    pub strategies: Vec<String>,
    /// When the trade was opened (RFC 3339). `sim settle` ignores bars that
    /// closed before it. Missing in sessions from before 0.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entered_at: Option<String>,
    // Filled at end-of-day
    pub exit_price: Option<f64>,
    pub pnl: Option<f64>,
    pub pnl_pct: Option<f64>,
    pub hit_target: Option<bool>,
    pub hit_stop: Option<bool>,
}

/// A simulation session (one day)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSession {
    pub date: String,
    pub market: String,
    pub capital: f64,
    pub target_pct: f64,
    pub trades: Vec<SimTrade>,
    // Summary (filled at settle)
    pub total_pnl: Option<f64>,
    pub total_pnl_pct: Option<f64>,
    pub win_count: Option<u32>,
    pub loss_count: Option<u32>,
    pub settled: bool,
}

/// History of all simulation sessions
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SimHistory {
    pub sessions: Vec<SimSession>,
}

impl SimHistory {
    pub fn load() -> Result<Self> {
        crate::store::load("sim_history.json")
    }

    pub fn save(&self) -> Result<()> {
        crate::store::save("sim_history.json", self, false)
    }

    /// The sessions of one market (`Market::id`, e.g. `IN`). Rupee and
    /// dollar sessions must never be added together.
    pub fn in_market<'a>(&'a self, market: &'a str) -> impl Iterator<Item = &'a SimSession> + 'a {
        self.sessions
            .iter()
            .filter(move |s| s.market.eq_ignore_ascii_case(market))
    }

    /// Whether `market` already has an unsettled session dated `date`.
    pub fn has_open_session(&self, market: &str, date: &str) -> bool {
        self.in_market(market).any(|s| s.date == date && !s.settled)
    }

    /// Statistics over the settled sessions of one market.
    pub fn stats(&self, market: &str) -> SimStats {
        let settled: Vec<&SimSession> = self.in_market(market).filter(|s| s.settled).collect();
        let total_days = settled.len();
        let winning_days = settled
            .iter()
            .filter(|s| s.total_pnl.unwrap_or(0.0) > 0.0)
            .count();
        let total_pnl: f64 = settled.iter().filter_map(|s| s.total_pnl).sum();
        let total_trades: u32 = settled.iter().map(|s| s.trades.len() as u32).sum();
        let winning_trades: u32 = settled.iter().filter_map(|s| s.win_count).sum();
        let losing_trades: u32 = settled.iter().filter_map(|s| s.loss_count).sum();
        let avg_daily_pnl = if total_days > 0 {
            total_pnl / total_days as f64
        } else {
            0.0
        };
        let best_day = settled
            .iter()
            .filter_map(|s| s.total_pnl)
            .fold(f64::NEG_INFINITY, f64::max);
        let worst_day = settled
            .iter()
            .filter_map(|s| s.total_pnl)
            .fold(f64::INFINITY, f64::min);

        let daily_returns: Vec<f64> = settled.iter().filter_map(|s| s.total_pnl_pct).collect();
        let sharpe = if daily_returns.len() > 1 {
            let mean = daily_returns.iter().sum::<f64>() / daily_returns.len() as f64;
            let variance = daily_returns
                .iter()
                .map(|r| (r - mean).powi(2))
                .sum::<f64>()
                / (daily_returns.len() - 1) as f64;
            let std = variance.sqrt();
            if std > 0.0 {
                Some(mean / std * (252.0_f64).sqrt())
            } else {
                None
            }
        } else {
            None
        };

        // Max consecutive wins/losses
        let mut max_win_streak = 0u32;
        let mut max_loss_streak = 0u32;
        let mut win_streak = 0u32;
        let mut loss_streak = 0u32;
        for s in &settled {
            if s.total_pnl.unwrap_or(0.0) > 0.0 {
                win_streak += 1;
                loss_streak = 0;
                max_win_streak = max_win_streak.max(win_streak);
            } else {
                loss_streak += 1;
                win_streak = 0;
                max_loss_streak = max_loss_streak.max(loss_streak);
            }
        }

        SimStats {
            total_days,
            winning_days,
            total_pnl,
            avg_daily_pnl,
            best_day: if best_day.is_finite() {
                Some(best_day)
            } else {
                None
            },
            worst_day: if worst_day.is_finite() {
                Some(worst_day)
            } else {
                None
            },
            total_trades,
            winning_trades,
            losing_trades,
            trade_win_rate: if total_trades > 0 {
                winning_trades as f64 / total_trades as f64 * 100.0
            } else {
                0.0
            },
            day_win_rate: if total_days > 0 {
                winning_days as f64 / total_days as f64 * 100.0
            } else {
                0.0
            },
            sharpe,
            max_win_streak,
            max_loss_streak,
        }
    }
}

/// One OHLC bar from an intraday (or daily) chart.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    /// Bar start.
    pub time: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// How a trade ended when replayed over a session's bars.
#[derive(Debug, Clone, PartialEq)]
pub struct Replay {
    pub exit_price: f64,
    pub exit_time: DateTime<Utc>,
    pub pnl: f64,
    /// P&L as a percentage of the capital the trade used.
    pub pnl_pct: f64,
    pub status: PositionStatus,
    pub t1_booked: bool,
}

/// Replay `trade` over one session's `bars` with the daemon's position
/// manager: book half at T1 and move the stop to breakeven, close the rest at
/// T2, trail the stop after T1, tighten it in the wind-down, and square off
/// at the bar that starts 15 minutes before the close.
///
/// Inside a bar the order of the high and the low is unknown, so the replay
/// is conservative: open, then the adverse extreme (a stop fills first), then
/// the favourable extreme, then the close. Stops and targets fill at their
/// level unless the bar opened beyond them (a gap fills at the open).
///
/// Daily bars (`bar_len` of a day or more) are the fallback for sessions too
/// old for intraday data: the same walk over one bar, exiting at the close.
///
/// Bars that ended before `entered_at` are skipped. Returns None when no bar
/// is left to replay.
pub fn replay_trade(
    trade: &SimTrade,
    bars: &[Bar],
    bar_len: chrono::Duration,
    market: Market,
    entered_at: Option<DateTime<Utc>>,
) -> Option<Replay> {
    let daily = bar_len >= chrono::Duration::days(1);
    let mut pos = LivePosition {
        symbol: trade.symbol.clone(),
        direction: trade.direction.clone(),
        entry_price: trade.entry_price,
        current_price: trade.entry_price,
        target1: trade.target1,
        target2: trade.target2,
        stop_loss: trade.stop_loss,
        trailing_stop: trade.stop_loss,
        qty: trade.qty,
        t1_booked: false,
        remaining_qty: trade.qty,
        booked_pnl: 0.0,
        pnl: 0.0,
        pnl_pct: 0.0,
        status: PositionStatus::Open,
        strategies: trade.strategies.clone(),
        score: trade.score,
        entered_at,
        initial_stop: Some(trade.stop_loss),
    };
    let long = trade.direction == "BUY";
    let mut last_time = None;

    for bar in bars
        .iter()
        .filter(|b| entered_at.is_none_or(|t| b.time + bar_len > t))
        .filter(|b| daily || clock::is_open(market, b.time))
    {
        let phase = if daily {
            Phase::Active
        } else {
            clock::market_phase(market, bar.time)
        };
        if matches!(phase, Phase::SquareOff | Phase::PostMarket) {
            update_position(&mut pos, bar.open, Phase::SquareOff);
            return Some(finish(&pos, bar.time));
        }
        last_time = Some(bar.time);

        // Open (gaps fill at the open).
        update_position(&mut pos, bar.open, phase);
        if pos.status.is_closed() {
            return Some(finish(&pos, bar.time));
        }
        // Adverse extreme first.
        let adverse = if long { bar.low } else { bar.high };
        feed_with_stop(&mut pos, adverse, phase);
        if pos.status.is_closed() {
            return Some(finish(&pos, bar.time));
        }
        // Favourable extreme: T1 then T2 at their levels.
        let favourable = if long { bar.high } else { bar.low };
        let reached = |level: f64| {
            if long {
                favourable >= level
            } else {
                favourable <= level
            }
        };
        let (t1, t2) = (pos.target1, pos.target2);
        if !pos.t1_booked && reached(t1) {
            update_position(&mut pos, t1, phase);
        }
        if reached(t2) {
            update_position(&mut pos, t2, phase);
        } else {
            update_position(&mut pos, favourable, phase);
        }
        if pos.status.is_closed() {
            return Some(finish(&pos, bar.time));
        }
        feed_with_stop(&mut pos, bar.close, phase);
        if pos.status.is_closed() {
            return Some(finish(&pos, bar.time));
        }
    }

    // Ran out of bars before the square-off bar (a daily bar, `--now`, or a
    // truncated chart): exit at the last price seen.
    let time = last_time?;
    pos.status = PositionStatus::SquaredOff;
    Some(finish(&pos, time))
}

/// Feed a price that moved through the stop as a fill at the stop level.
fn feed_with_stop(pos: &mut LivePosition, price: f64, phase: Phase) {
    let long = pos.direction == "BUY";
    let stop = match (long, pos.t1_booked) {
        (true, true) => pos.stop_loss.max(pos.trailing_stop),
        (false, true) => pos.stop_loss.min(pos.trailing_stop),
        _ => pos.stop_loss,
    };
    let crossed = if long { price <= stop } else { price >= stop };
    update_position(pos, if crossed { stop } else { price }, phase);
}

fn finish(pos: &LivePosition, time: DateTime<Utc>) -> Replay {
    let invested = pos.entry_price * pos.qty as f64;
    Replay {
        exit_price: pos.current_price,
        exit_time: time,
        pnl: pos.pnl,
        pnl_pct: if invested > 0.0 {
            pos.pnl / invested * 100.0
        } else {
            0.0
        },
        status: pos.status,
        t1_booked: pos.t1_booked,
    }
}

#[derive(Debug)]
pub struct SimStats {
    pub total_days: usize,
    pub winning_days: usize,
    pub total_pnl: f64,
    pub avg_daily_pnl: f64,
    pub best_day: Option<f64>,
    pub worst_day: Option<f64>,
    pub total_trades: u32,
    pub winning_trades: u32,
    pub losing_trades: u32,
    pub trade_win_rate: f64,
    pub day_win_rate: f64,
    pub sharpe: Option<f64>,
    pub max_win_streak: u32,
    pub max_loss_streak: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_trade(sym: &str, pnl: f64) -> SimTrade {
        SimTrade {
            symbol: sym.into(),
            direction: "BUY".into(),
            entry_price: 100.0,
            target1: 102.0,
            target2: 104.0,
            stop_loss: 98.0,
            qty: 10,
            capital: 1000.0,
            score: 70.0,
            confidence: "HIGH".into(),
            strategies: vec!["RSI".into()],
            entered_at: None,
            exit_price: Some(100.0 + pnl / 10.0),
            pnl: Some(pnl),
            pnl_pct: Some(pnl / 10.0),
            hit_target: Some(pnl > 0.0),
            hit_stop: Some(pnl < -15.0),
        }
    }

    fn make_session(date: &str, pnl: f64, wins: u32, losses: u32) -> SimSession {
        SimSession {
            date: date.into(),
            market: "IN".into(),
            capital: 25000.0,
            target_pct: 2.0,
            trades: vec![make_trade("TEST.NS", pnl)],
            total_pnl: Some(pnl),
            total_pnl_pct: Some(pnl / 250.0),
            win_count: Some(wins),
            loss_count: Some(losses),
            settled: true,
        }
    }

    #[test]
    fn test_empty_stats() {
        let h = SimHistory::default();
        let s = h.stats("IN");
        assert_eq!(s.total_days, 0);
        assert_eq!(s.total_pnl, 0.0);
    }

    #[test]
    fn test_single_winning_day() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-04-01", 500.0, 3, 1));
        let s = h.stats("IN");
        assert_eq!(s.total_days, 1);
        assert_eq!(s.winning_days, 1);
        assert_eq!(s.total_pnl, 500.0);
        assert_eq!(s.day_win_rate, 100.0);
    }

    #[test]
    fn test_mixed_days() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-04-01", 500.0, 3, 1));
        h.sessions.push(make_session("2026-04-02", -200.0, 1, 3));
        h.sessions.push(make_session("2026-04-03", 300.0, 2, 2));
        let s = h.stats("IN");
        assert_eq!(s.total_days, 3);
        assert_eq!(s.winning_days, 2);
        assert!((s.total_pnl - 600.0).abs() < 0.01);
        assert!(s.day_win_rate > 60.0);
    }

    #[test]
    fn test_win_streak() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-04-01", 100.0, 1, 0));
        h.sessions.push(make_session("2026-04-02", 200.0, 1, 0));
        h.sessions.push(make_session("2026-04-03", 300.0, 1, 0));
        h.sessions.push(make_session("2026-04-04", -50.0, 0, 1));
        let s = h.stats("IN");
        assert_eq!(s.max_win_streak, 3);
        assert_eq!(s.max_loss_streak, 1);
    }

    #[test]
    fn test_unsettled_sessions_excluded() {
        let mut h = SimHistory::default();
        h.sessions.push(SimSession {
            date: "2026-04-01".into(),
            market: "IN".into(),
            capital: 25000.0,
            target_pct: 2.0,
            trades: vec![],
            total_pnl: None,
            total_pnl_pct: None,
            win_count: None,
            loss_count: None,
            settled: false,
        });
        let s = h.stats("IN");
        assert_eq!(s.total_days, 0); // unsettled shouldn't count
    }

    #[test]
    fn test_avg_daily_pnl() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-04-01", 300.0, 2, 1));
        h.sessions.push(make_session("2026-04-02", 100.0, 1, 2));
        let s = h.stats("IN");
        assert!((s.avg_daily_pnl - 200.0).abs() < 0.01);
    }

    #[test]
    fn test_sharpe_calculation() {
        let mut h = SimHistory::default();
        for i in 0..20 {
            h.sessions.push(make_session(
                &format!("2026-04-{:02}", i + 1),
                if i % 3 == 0 { -100.0 } else { 200.0 },
                1,
                0,
            ));
        }
        let s = h.stats("IN");
        assert!(s.sharpe.is_some());
    }

    // ── replay_trade ──

    fn ist(hh: u32, mm: u32) -> DateTime<Utc> {
        let d = chrono::NaiveDate::from_ymd_opt(2026, 10, 21).unwrap();
        clock::at(
            Market::In,
            d,
            chrono::NaiveTime::from_hms_opt(hh, mm, 0).unwrap(),
        )
    }

    fn bar(hh: u32, mm: u32, o: f64, h: f64, l: f64, c: f64) -> Bar {
        Bar {
            time: ist(hh, mm),
            open: o,
            high: h,
            low: l,
            close: c,
        }
    }

    /// Long 10 @ 100, T1 102, T2 104, stop 98.
    fn open_trade() -> SimTrade {
        let mut t = make_trade("TEST.NS", 0.0);
        t.exit_price = None;
        t.pnl = None;
        t
    }

    /// Flat bars every 5 minutes from `from` to 15:25 IST at `price`.
    fn flat_day(from: (u32, u32), price: f64) -> Vec<Bar> {
        let mut bars = Vec::new();
        let (mut h, mut m) = from;
        while (h, m) < (15, 30) {
            bars.push(bar(h, m, price, price, price, price));
            m += 5;
            if m == 60 {
                h += 1;
                m = 0;
            }
        }
        bars
    }

    fn five_min() -> chrono::Duration {
        chrono::Duration::minutes(5)
    }

    #[test]
    fn replay_stop_before_target_in_the_same_bar_is_a_stop() {
        // One wide bar touches both 97 and 105: the conservative order says
        // the stop filled first, at the stop level.
        let mut bars = vec![bar(9, 30, 100.0, 105.0, 97.0, 101.0)];
        bars.extend(flat_day((9, 35), 101.0));
        let r = replay_trade(&open_trade(), &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::StopHit);
        assert!((r.exit_price - 98.0).abs() < 1e-9);
        assert!((r.pnl + 20.0).abs() < 1e-9); // 10 × −2
    }

    #[test]
    fn replay_target_first_books_t1_then_t2() {
        let mut bars = vec![
            bar(9, 30, 100.0, 101.0, 99.5, 100.5),
            bar(9, 35, 100.5, 104.5, 100.2, 104.0),
        ];
        bars.extend(flat_day((9, 40), 104.0));
        let r = replay_trade(&open_trade(), &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::T2Hit);
        assert!(r.t1_booked);
        assert!((r.exit_price - 104.0).abs() < 1e-9);
        // 5 × (102 − 100) + 5 × (104 − 100) = 30
        assert!((r.pnl - 30.0).abs() < 1e-9);
        assert_eq!(r.exit_time, ist(9, 35));
    }

    #[test]
    fn replay_neither_exits_at_the_square_off_bar() {
        let mut bars = flat_day((9, 15), 100.5);
        // Make the 15:15 bar open at 101 so the exit price is visible.
        let i = bars.iter().position(|b| b.time == ist(15, 15)).unwrap();
        bars[i] = bar(15, 15, 101.0, 101.0, 101.0, 101.0);
        let r = replay_trade(&open_trade(), &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::SquaredOff);
        assert_eq!(r.exit_time, ist(15, 15));
        assert!((r.exit_price - 101.0).abs() < 1e-9);
        assert!((r.pnl - 10.0).abs() < 1e-9);
        assert!((r.pnl_pct - 1.0).abs() < 1e-9);
    }

    #[test]
    fn replay_ignores_bars_before_entry() {
        // The stop was hit at 09:30, but the trade was only opened at 11:02.
        let mut bars = vec![bar(9, 30, 100.0, 100.0, 96.0, 99.0)];
        bars.extend(flat_day((11, 0), 100.0));
        let entered = Some(ist(11, 2));
        let r = replay_trade(&open_trade(), &bars, five_min(), Market::In, entered).unwrap();
        assert_eq!(r.status, PositionStatus::SquaredOff);
        assert!((r.pnl).abs() < 1e-9);
    }

    #[test]
    fn replay_gap_through_the_stop_fills_at_the_open() {
        let mut bars = vec![bar(9, 15, 96.0, 97.0, 95.5, 96.5)];
        bars.extend(flat_day((9, 20), 96.5));
        let r = replay_trade(&open_trade(), &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::StopHit);
        assert!((r.exit_price - 96.0).abs() < 1e-9);
    }

    #[test]
    fn replay_short_mirrors_long() {
        let mut t = open_trade();
        t.direction = "SELL".into();
        t.target1 = 98.0;
        t.target2 = 96.0;
        t.stop_loss = 102.0;
        let mut bars = vec![bar(9, 30, 100.0, 101.0, 95.0, 96.5)];
        bars.extend(flat_day((9, 35), 96.5));
        let r = replay_trade(&t, &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::T2Hit);
        // 5 × (100 − 98) + 5 × (100 − 96) = 30
        assert!((r.pnl - 30.0).abs() < 1e-9);
    }

    #[test]
    fn replay_without_bars_is_none_and_partial_day_exits_at_last_close() {
        assert!(replay_trade(&open_trade(), &[], five_min(), Market::In, None).is_none());
        let bars = vec![
            bar(9, 30, 100.0, 100.8, 99.5, 100.5),
            bar(9, 35, 100.5, 101.0, 100.4, 100.9),
        ];
        let r = replay_trade(&open_trade(), &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::SquaredOff);
        assert!((r.exit_price - 100.9).abs() < 1e-9);
    }

    #[test]
    fn replay_daily_bar_is_conservative() {
        let day = Bar {
            time: ist(9, 15),
            open: 100.0,
            high: 105.0,
            low: 97.0,
            close: 103.0,
        };
        let r = replay_trade(
            &open_trade(),
            &[day],
            chrono::Duration::days(1),
            Market::In,
            None,
        )
        .unwrap();
        assert_eq!(r.status, PositionStatus::StopHit);
    }

    #[test]
    fn history_never_adds_rupee_and_dollar_sessions() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-01-05", 500.0, 1, 0));
        let mut us = make_session("2026-01-05", -20.0, 0, 1);
        us.market = "US".into();
        h.sessions.push(us);

        let india = h.stats("IN");
        assert_eq!(india.total_days, 1);
        assert_eq!(india.total_pnl, 500.0);
        assert_eq!(india.best_day, Some(500.0));
        assert_eq!(india.worst_day, Some(500.0));

        let us = h.stats("us");
        assert_eq!(us.total_days, 1, "market ids compare without case");
        assert_eq!(us.total_pnl, -20.0);
        assert_eq!(us.winning_days, 0);

        assert_eq!(h.stats("UK").total_days, 0);

        // An open NSE session does not block a NYSE one on the same date.
        let mut open = make_session("2026-01-06", 0.0, 0, 0);
        open.settled = false;
        h.sessions.push(open);
        assert!(h.has_open_session("IN", "2026-01-06"));
        assert!(!h.has_open_session("US", "2026-01-06"));
        assert_eq!(h.in_market("IN").count(), 2);
    }
}
