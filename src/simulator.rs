use crate::backtest::costs::{Brokerage, CostModel, Side as CostSide};
use crate::clock;
use crate::daemon::{LivePosition, Phase, PositionStatus, update_position};
use crate::market::Market;
use anyhow::Result;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};

// ══════════════════════════════════════════════════════════
// PAPER COSTS
// ══════════════════════════════════════════════════════════

/// Environment variable that picks the paper cost preset: `in-intraday`,
/// `in-delivery`, `us` or `none`.
pub const COSTS_ENV: &str = "STOCKWISE_SIM_COSTS";
/// Environment variable that overrides the brokerage per order: `20`,
/// `0.03%` or `20,0.03%` (the lower of both).
pub const BROKERAGE_ENV: &str = "STOCKWISE_SIM_BROKERAGE";
/// Environment variable that overrides the slippage per fill, in basis points.
pub const SLIPPAGE_ENV: &str = "STOCKWISE_SIM_SLIPPAGE_BPS";

/// The charges and slippage paper results pay in `market` (`Market::id`).
///
/// Defaults: NSE intraday at a discount broker for India (₹20 or 0.03% per
/// order, the lower; 0.025% STT on the sell; exchange and SEBI fees; GST;
/// 0.003% stamp duty on the buy) and commission-free for every other market.
/// Both pay 5 bps of slippage per fill. [`COSTS_ENV`], [`BROKERAGE_ENV`] and
/// [`SLIPPAGE_ENV`] override them; an unreadable value is ignored with a
/// warning.
pub fn paper_cost_model(market: &str) -> CostModel {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
    cost_model_from(
        market,
        env(COSTS_ENV),
        env(BROKERAGE_ENV),
        env(SLIPPAGE_ENV),
    )
}

/// Print a cost-setting warning once per process: the daemon reads the
/// settings on every tick.
fn warn_once(msg: String) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static WARNED: AtomicBool = AtomicBool::new(false);
    if !WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("  warning: {msg}");
    }
}

fn cost_model_from(
    market: &str,
    preset: Option<String>,
    brokerage: Option<String>,
    slippage: Option<String>,
) -> CostModel {
    let default = || {
        if market.eq_ignore_ascii_case("IN") {
            CostModel::in_intraday()
        } else {
            CostModel::us()
        }
    };
    let mut model = match preset.as_deref().map(|p| p.trim().to_ascii_lowercase()) {
        None => default(),
        Some(p) => match p.as_str() {
            "in-intraday" => CostModel::in_intraday(),
            "in-delivery" => CostModel::in_delivery(),
            "us" => CostModel::us(),
            "none" => CostModel::none(),
            other => {
                warn_once(format!(
                    "{COSTS_ENV}={other} is not in-intraday, in-delivery, us or none; using the default"
                ));
                default()
            }
        },
    };
    if let Some(b) = brokerage {
        match Brokerage::parse(&b) {
            Ok(b) => model.brokerage = b,
            Err(e) => warn_once(format!("{BROKERAGE_ENV}: {e}")),
        }
    }
    if let Some(s) = slippage {
        match s.trim().parse::<f64>() {
            Ok(v) if v.is_finite() && v >= 0.0 => model.slippage_bps = v,
            _ => warn_once(format!(
                "{SLIPPAGE_ENV}={s} is not a number of basis points"
            )),
        }
    }
    model
}

/// Charges plus slippage for one order of `qty` at `price`.
fn order_cost(model: &CostModel, side: CostSide, qty: u32, price: f64) -> f64 {
    let turnover = price * qty as f64;
    if !(turnover > 0.0 && turnover.is_finite()) {
        return 0.0;
    }
    model.charges(side, turnover).total() + turnover * model.slippage_bps / 10_000.0
}

/// The exit fills of a paper trade: half at level 1 when it was booked (the
/// position manager books `qty / 2`), the rest at `exit`.
pub fn exit_fills(qty: u32, t1_booked: bool, target1: f64, exit: f64) -> Vec<(u32, f64)> {
    if t1_booked {
        let booked = qty / 2;
        vec![(booked, target1), (qty - booked, exit)]
    } else {
        vec![(qty, exit)]
    }
}

/// Charges and slippage of a whole paper round trip: one opening order of
/// `qty` at `entry`, then the closing `fills`. A long opens with a purchase
/// and closes with sales; a short the other way round, so STT and stamp duty
/// land on the right leg.
pub fn round_trip_cost(
    model: &CostModel,
    long: bool,
    qty: u32,
    entry: f64,
    fills: &[(u32, f64)],
) -> f64 {
    let (open, close) = if long {
        (CostSide::Buy, CostSide::Sell)
    } else {
        (CostSide::Sell, CostSide::Buy)
    };
    order_cost(model, open, qty, entry)
        + fills
            .iter()
            .filter(|(q, _)| *q > 0)
            .map(|&(q, p)| order_cost(model, close, q, p))
            .sum::<f64>()
}

// ══════════════════════════════════════════════════════════
// TRACK RECORD
// ══════════════════════════════════════════════════════════

/// Fewest closed paper trades before Kelly sizing uses the measured win rate.
pub const MIN_TRADES_FOR_KELLY: usize = 30;

/// Win rate and payoff measured from closed paper trades (net of costs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrackRecord {
    pub trades: usize,
    /// Share of trades with a positive P&L, 0–1.
    pub win_rate: f64,
    /// Average win over average loss; infinite when no trade lost.
    pub win_loss_ratio: f64,
}

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
    /// How far the trailing exit level hangs behind the price (1.5 × ATR at
    /// entry). Missing in sessions from before 0.9, which trail by 1.5%.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trail_distance: Option<f64>,
    // Filled at end-of-day
    /// Brokerage, statutory charges and slippage, already taken out of `pnl`.
    /// Missing in sessions settled before 0.9, whose P&L is gross.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charges: Option<f64>,
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
    /// The cost model fixed when the session started (`sim start`, or the
    /// daemon's own), so settling later under other settings never changes
    /// what the session pays. Missing in sessions from before 0.9.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_model: Option<CostModel>,
}

/// History of all simulation sessions
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct SimHistory {
    pub sessions: Vec<SimSession>,
}

impl SimSession {
    /// The charges and slippage this session pays: the model saved with it,
    /// else the current settings (sessions from before 0.9).
    pub fn costs(&self) -> CostModel {
        self.cost_model
            .clone()
            .unwrap_or_else(|| paper_cost_model(&self.market))
    }
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
        // Day statistics group sessions by date: several sessions on one
        // date (settle --now, then start again) are one day.
        let mut daily: std::collections::BTreeMap<&str, f64> = std::collections::BTreeMap::new();
        for s in &settled {
            *daily.entry(s.date.as_str()).or_default() += s.total_pnl.unwrap_or(0.0);
        }
        let distinct_days = daily.len();
        let gross_trades = settled
            .iter()
            .flat_map(|s| &s.trades)
            .filter(|t| t.pnl.is_some() && t.charges.is_none())
            .count() as u32;
        let winning_days = daily.values().filter(|p| **p > 0.0).count();
        let total_pnl: f64 = settled.iter().filter_map(|s| s.total_pnl).sum();
        let total_trades: u32 = settled.iter().map(|s| s.trades.len() as u32).sum();
        let winning_trades: u32 = settled.iter().filter_map(|s| s.win_count).sum();
        let losing_trades: u32 = settled.iter().filter_map(|s| s.loss_count).sum();
        let avg_daily_pnl = if distinct_days > 0 {
            total_pnl / distinct_days as f64
        } else {
            0.0
        };
        let best_day = daily.values().copied().fold(f64::NEG_INFINITY, f64::max);
        let worst_day = daily.values().copied().fold(f64::INFINITY, f64::min);

        let sharpe = sharpe_over_trading_days(&settled, market_of(market));
        let total_charges: f64 = settled
            .iter()
            .flat_map(|s| &s.trades)
            .filter_map(|t| t.charges)
            .sum();

        // Max consecutive winning / losing dates.
        let mut max_win_streak = 0u32;
        let mut max_loss_streak = 0u32;
        let mut win_streak = 0u32;
        let mut loss_streak = 0u32;
        for &pnl in daily.values() {
            if pnl > 0.0 {
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
            distinct_days,
            gross_trades,
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
            day_win_rate: if distinct_days > 0 {
                winning_days as f64 / distinct_days as f64 * 100.0
            } else {
                0.0
            },
            sharpe,
            max_win_streak,
            max_loss_streak,
            total_charges,
        }
    }

    /// Win rate and payoff of the closed trades of `market`, once there are
    /// at least [`MIN_TRADES_FOR_KELLY`]; None before that.
    pub fn track_record(&self, market: &str) -> Option<TrackRecord> {
        let pnls: Vec<f64> = self
            .in_market(market)
            .filter(|s| s.settled)
            .flat_map(|s| &s.trades)
            // Only net-accounted results: trades settled before 0.9 carry a
            // gross P&L (`charges` missing) and would flatter the record.
            .filter(|t| t.charges.is_some())
            .filter_map(|t| t.pnl)
            .filter(|p| p.is_finite())
            .collect();
        if pnls.len() < MIN_TRADES_FOR_KELLY {
            return None;
        }
        let wins: Vec<f64> = pnls.iter().copied().filter(|p| *p > 0.0).collect();
        let losses: Vec<f64> = pnls.iter().copied().filter(|p| *p <= 0.0).collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let avg_loss = -mean(&losses);
        Some(TrackRecord {
            trades: pnls.len(),
            win_rate: wins.len() as f64 / pnls.len() as f64,
            win_loss_ratio: if avg_loss > 0.0 {
                mean(&wins) / avg_loss
            } else {
                f64::INFINITY
            },
        })
    }
}

fn market_of(id: &str) -> Market {
    <Market as clap::ValueEnum>::from_str(id, true).unwrap_or(Market::In)
}

/// Annualised Sharpe ratio of the daily return over **every trading day**
/// from the first settled session to the last: a trading day with no session
/// counts as a 0% day instead of being skipped, so a bot that sits out the
/// bad days is not flattered. Sessions on the same date add up.
fn sharpe_over_trading_days(settled: &[&SimSession], market: Market) -> Option<f64> {
    use std::collections::BTreeMap;
    let mut by_day: BTreeMap<NaiveDate, f64> = BTreeMap::new();
    for s in settled {
        let (Ok(date), Some(pct)) = (
            NaiveDate::parse_from_str(&s.date, "%Y-%m-%d"),
            s.total_pnl_pct,
        ) else {
            continue;
        };
        *by_day.entry(date).or_default() += pct;
    }
    let (&first, &last) = (by_day.keys().next()?, by_day.keys().next_back()?);
    let mut day = first;
    while day < last {
        day = day.succ_opt()?;
        if clock::is_trading_day(market, day) {
            by_day.entry(day).or_default();
        }
    }
    let returns: Vec<f64> = by_day.into_values().collect();
    if returns.len() < 2 {
        return None;
    }
    let n = returns.len() as f64;
    let mean = returns.iter().sum::<f64>() / n;
    let variance = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / (n - 1.0);
    let std = variance.sqrt();
    (std > 0.0).then(|| mean / std * 252.0_f64.sqrt())
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
        // The plan's trail hangs 1.5 × ATR behind the price; sessions from
        // before 0.9 have no distance and start the trail at the stop.
        trailing_stop: match trade.trail_distance {
            Some(d) if crate::intraday::Direction::is_long_str(&trade.direction) => {
                trade.entry_price - d
            }
            Some(d) => trade.entry_price + d,
            None => trade.stop_loss,
        },
        trail_distance: trade.trail_distance,
        charges: None,
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
    let long = crate::intraday::Direction::is_long_str(&trade.direction);
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
    let long = crate::intraday::Direction::is_long_str(&pos.direction);
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
    /// Settled sessions. Day statistics (winning days, day win rate, best
    /// and worst day, average, streaks) group them by date.
    pub total_days: usize,
    /// Distinct dates among the settled sessions.
    pub distinct_days: usize,
    /// Settled trades whose P&L is gross: settled before 0.9, without
    /// charges recorded.
    pub gross_trades: u32,
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
    /// Charges and slippage paid by the settled trades (already in the P&L).
    pub total_charges: f64,
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
            trail_distance: None,
            charges: None,
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
            cost_model: None,
        }
    }

    #[test]
    fn old_history_with_buy_and_sell_still_loads_as_long_and_short() {
        let mut long = serde_json::to_value(make_trade("A.NS", 10.0)).unwrap();
        assert_eq!(long["direction"], "BUY"); // fixture uses the pre-0.8.1 spelling
        let mut short = serde_json::to_value(make_trade("B.NS", 10.0)).unwrap();
        short["direction"] = "SELL".into();
        long["direction"] = "BUY".into();
        let long: SimTrade = serde_json::from_value(long).unwrap();
        let short: SimTrade = serde_json::from_value(short).unwrap();
        assert!(crate::intraday::Direction::is_long_str(&long.direction));
        assert!(!crate::intraday::Direction::is_long_str(&short.direction));
        assert_eq!(
            crate::intraday::Direction::label_of(&short.direction),
            "short"
        );
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
        assert_eq!(s.distinct_days, 3);
        assert_eq!(s.gross_trades, 3);
        assert_eq!(s.winning_days, 2);
        assert!((s.total_pnl - 600.0).abs() < 0.01);
        assert!(s.day_win_rate > 60.0);
    }

    #[test]
    fn day_statistics_group_sessions_by_date() {
        // 100 winning sessions on one date and 19 losing dates: one winning
        // day of 20, not 100 of 119.
        let mut h = SimHistory::default();
        for _ in 0..100 {
            h.sessions.push(make_session("2026-04-01", 10.0, 1, 0));
        }
        for d in 2..=20 {
            h.sessions
                .push(make_session(&format!("2026-04-{d:02}"), -1.0, 0, 1));
        }
        let s = h.stats("IN");
        assert_eq!(
            (s.total_days, s.distinct_days, s.winning_days),
            (119, 20, 1)
        );
        assert!((s.day_win_rate - 5.0).abs() < 1e-9);
        assert_eq!(s.best_day, Some(1000.0));
        assert_eq!(s.worst_day, Some(-1.0));
        assert_eq!((s.max_win_streak, s.max_loss_streak), (1, 19));
        assert!((s.avg_daily_pnl - (1000.0 - 19.0) / 20.0).abs() < 1e-9);
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
            cost_model: None,
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
    fn sharpe_counts_every_trading_day_in_the_period() {
        // Mon 2026-10-26 +1%, Fri 2026-10-30 +1%: five trading days, three of
        // them without a session. Over traded days only the two equal returns
        // had no spread (no Sharpe at all); over all five days the returns
        // are [1, 0, 0, 0, 1].
        let mut h = SimHistory::default();
        for date in ["2026-10-26", "2026-10-30"] {
            let mut s = make_session(date, 250.0, 1, 0);
            s.total_pnl_pct = Some(1.0);
            h.sessions.push(s);
        }
        let r = [1.0, 0.0, 0.0, 0.0, 1.0];
        let mean = 0.4;
        let sd = (r.iter().map(|x: &f64| (x - mean).powi(2)).sum::<f64>() / 4.0).sqrt();
        let want = mean / sd * 252.0_f64.sqrt();
        assert!((h.stats("IN").sharpe.unwrap() - want).abs() < 1e-9);
        // A weekend in the middle adds nothing: Fri → Mon is two days.
        let mut h = SimHistory::default();
        for (date, pct) in [("2026-10-30", 1.0), ("2026-11-02", -1.0)] {
            let mut s = make_session(date, 0.0, 0, 0);
            s.total_pnl_pct = Some(pct);
            h.sessions.push(s);
        }
        let sd = 2.0_f64.sqrt();
        assert!((h.stats("IN").sharpe.unwrap() - 0.0 / sd).abs() < 1e-9);
    }

    #[test]
    fn india_intraday_costs_take_the_expected_amount() {
        // Long 100 @ ₹1,000, closed @ ₹1,010 at the default NSE intraday
        // rates.
        let m = cost_model_from("IN", None, None, None);
        let cost = round_trip_cost(&m, true, 100, 1000.0, &[(100, 1010.0)]);
        // Purchase of ₹1,00,000: brokerage ₹20 (0.03% would be ₹30),
        // exchange 0.00297%, SEBI 0.0001%, GST 18% of those, stamp 0.003%,
        // slippage 5 bps.
        let buy = 20.0 + 2.97 + 0.1 + 0.18 * (20.0 + 2.97 + 0.1) + 3.0 + 50.0;
        // Sale of ₹1,01,000: brokerage ₹20, STT 0.025%, exchange, SEBI, GST,
        // slippage.
        let ex = 101_000.0 * 0.00297 / 100.0;
        let sebi = 101_000.0 * 0.0001 / 100.0;
        let sell = 20.0 + 25.25 + ex + sebi + 0.18 * (20.0 + ex + sebi) + 50.5;
        assert!(
            (cost - (buy + sell)).abs() < 1e-6,
            "{cost} vs {}",
            buy + sell
        );
        // A short pays the STT on its opening sale and stamp duty on the
        // closing purchase instead.
        let short = round_trip_cost(&m, false, 100, 1000.0, &[(100, 1010.0)]);
        assert!(short > 0.0 && (short - cost).abs() < 1.0);
        // US: commission-free, slippage only.
        let us = cost_model_from("US", None, None, None);
        let c = round_trip_cost(&us, true, 10, 100.0, &[(10, 101.0)]);
        assert!((c - (1000.0 + 1010.0) * 5.0 / 10_000.0).abs() < 1e-9);
        // Overrides.
        let free = cost_model_from("IN", Some("none".into()), None, Some("0".into()));
        assert_eq!(round_trip_cost(&free, true, 10, 100.0, &[(10, 101.0)]), 0.0);
        let flat = cost_model_from("US", None, Some("1".into()), Some("0".into()));
        assert_eq!(
            round_trip_cost(&flat, true, 10, 100.0, &[(5, 102.0), (5, 101.0)]),
            3.0
        );
        // Level 1 booked half: two closing fills.
        assert_eq!(
            exit_fills(11, true, 102.0, 99.0),
            vec![(5, 102.0), (6, 99.0)]
        );
        assert_eq!(exit_fills(11, false, 102.0, 99.0), vec![(11, 99.0)]);
    }

    #[test]
    fn track_record_needs_thirty_closed_trades() {
        let mut h = SimHistory::default();
        let net = |date: &str, pnl: f64| {
            let mut s = make_session(date, pnl, 0, 0);
            s.trades[0].charges = Some(1.0);
            s
        };
        for i in 0..29 {
            h.sessions
                .push(net("2026-04-01", if i % 3 == 0 { -10.0 } else { 20.0 }));
        }
        // Gross results from before 0.9 never count toward the net record.
        for _ in 0..40 {
            h.sessions.push(make_session("2026-03-01", 20.0, 1, 0));
        }
        assert_eq!(h.track_record("IN"), None);
        h.sessions.push(net("2026-04-02", -10.0));
        let tr = h.track_record("IN").unwrap();
        assert_eq!(tr.trades, 30);
        // 19 wins of 20 each, 11 losses of 10 each.
        assert!((tr.win_rate - 19.0 / 30.0).abs() < 1e-9);
        assert!((tr.win_loss_ratio - 2.0).abs() < 1e-9);
        assert_eq!(h.track_record("US"), None);
    }

    #[test]
    fn replay_trails_by_the_plan_distance() {
        // T1 102 books half and moves the exit level to 100; the trail of
        // 1.5 points then follows the high of 103.5 to 102: the last bar's
        // low of 101.8 closes the rest there.
        let mut t = open_trade();
        t.target2 = 110.0;
        t.trail_distance = Some(1.5);
        let mut bars = vec![
            bar(9, 30, 100.0, 103.5, 99.5, 103.0),
            bar(9, 35, 103.0, 103.2, 101.8, 102.5),
        ];
        bars.extend(flat_day((9, 40), 102.5));
        let r = replay_trade(&t, &bars, five_min(), Market::In, None).unwrap();
        assert_eq!(r.status, PositionStatus::StopHit);
        assert!(r.t1_booked);
        assert!((r.exit_price - 102.0).abs() < 1e-9, "{}", r.exit_price);
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
