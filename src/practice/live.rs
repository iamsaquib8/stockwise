//! Challenges over live paper sessions: the bot's `sim start` picks,
//! settled by `sim settle`. The rules are checked after the fact over the
//! settled sessions, each counting as one day.

use super::challenge::{Active, Facts, Outcome, Violation};
use super::score::{self, BuyFact, RunInput};
use super::tape::DayBar;
use crate::simulator::{SimHistory, SimSession};
use anyhow::Result;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

pub const FILE: &str = "sim_challenge.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LiveChallenge {
    pub active: Active,
    /// `IN` or `US`, as `sim start` records sessions.
    pub market: String,
}

/// A live challenge evaluated over its sessions so far.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveRun {
    pub input: RunInput,
    pub facts: Facts,
    pub violations: Vec<Violation>,
    pub sessions: usize,
    pub from: NaiveDate,
    pub to: NaiveDate,
}

impl LiveChallenge {
    pub fn load() -> Result<Option<LiveChallenge>> {
        crate::store::load::<Option<LiveChallenge>>(FILE)
    }

    pub fn save(&self) -> Result<()> {
        crate::store::save(FILE, self, false)
    }

    pub fn clear() -> Result<()> {
        let path = crate::store::path(FILE)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// The settled sessions that count, oldest first: this market, from the
    /// start date on, one per trading day (the first settled that day), and
    /// no more than the challenge's length.
    pub fn sessions<'a>(&self, history: &'a SimHistory) -> Vec<&'a SimSession> {
        let start = self.active.start_date.format("%Y-%m-%d").to_string();
        let mut out: Vec<&SimSession> = Vec::new();
        for s in history
            .sessions
            .iter()
            .filter(|s| s.settled && s.market == self.market && s.date >= start)
        {
            if !out.iter().any(|o| o.date == s.date) {
                out.push(s);
            }
        }
        out.sort_by(|a, b| a.date.cmp(&b.date));
        if self.active.end_day > 0 {
            out.truncate(self.active.end_day);
        }
        out
    }

    pub fn is_complete(&self, sessions: usize) -> bool {
        sessions >= self.active.end_day
    }

    /// Score input, goal facts and broken rules over `sessions`, with the
    /// benchmark's daily bars `bench` (they may start before the sessions).
    pub fn evaluate(&self, sessions: &[&SimSession], bench: &[DayBar]) -> LiveRun {
        let mut active = self.active.clone();
        active.violations.clear();
        let dates: Vec<NaiveDate> = sessions
            .iter()
            .filter_map(|s| NaiveDate::parse_from_str(&s.date, "%Y-%m-%d").ok())
            .collect();
        let close_on = |d: NaiveDate| {
            bench
                .iter()
                .take_while(|b| b.date <= d)
                .last()
                .map(|b| b.close)
        };
        let base = dates
            .first()
            .and_then(|first| bench.iter().take_while(|b| b.date < *first).last())
            .map(|b| b.close)
            .or_else(|| dates.first().and_then(|d| close_on(*d)));

        let mut nav = vec![1.0];
        let mut benchmark = vec![base.unwrap_or(1.0)];
        let mut exposure = Vec::new();
        let mut buys = Vec::new();
        let mut symbols: Vec<&str> = Vec::new();
        let mut max_positions = 0;
        for (s, &date) in sessions.iter().zip(&dates) {
            let r = s.total_pnl_pct.unwrap_or(0.0) / 100.0;
            nav.push(nav.last().unwrap() * (1.0 + r));
            benchmark.push(close_on(date).or(base).unwrap_or(1.0));
            let used: f64 = s.trades.iter().map(|t| t.capital).sum();
            exposure.push(if s.capital > 0.0 {
                (used / s.capital).min(1.0)
            } else {
                0.0
            });
            max_positions = max_positions.max(s.trades.len());
            for t in &s.trades {
                let weight = if s.capital > 0.0 {
                    t.capital / s.capital * 100.0
                } else {
                    100.0
                };
                let with_stop = t.stop_loss > 0.0;
                buys.push(BuyFact {
                    with_stop,
                    weight_percent: weight,
                });
                active
                    .violations
                    .extend(active.check_buy(date, &t.symbol, with_stop, weight));
                if !symbols.contains(&t.symbol.as_str()) {
                    symbols.push(&t.symbol);
                }
            }
            active.check_day(date, score::current_drawdown_percent(&nav), None);
        }
        let hard = active.hard_broken().is_some();
        let input = RunInput {
            nav: nav.clone(),
            benchmark: benchmark.clone(),
            exposure: exposure.clone(),
            buys,
            // Live trades carry no sector.
            max_sectors: 0,
            symbols_held: symbols.len(),
            hard_break: hard,
            soft_violations: active.soft_violations(),
            periods_per_year: 252.0,
        };
        let change = |v: &[f64]| (v.last().unwrap() / v[0] - 1.0) * 100.0;
        let facts = Facts {
            return_percent: change(&nav),
            benchmark_return_percent: change(&benchmark),
            exposure_percent: if exposure.is_empty() {
                0.0
            } else {
                exposure.iter().sum::<f64>() / exposure.len() as f64 * 100.0
            },
            max_drawdown_percent: score::max_drawdown_percent(&nav),
            benchmark_max_drawdown_percent: score::max_drawdown_percent(&benchmark),
            max_positions,
            buys: input.buys.len(),
            equity: 0.0,
            sip_value: None,
        };
        LiveRun {
            input,
            facts,
            violations: active.violations,
            sessions: sessions.len(),
            from: self.active.start_date,
            to: dates.last().copied().unwrap_or(self.active.start_date),
        }
    }

    /// The outcome over `run` (`abandoned` before enough sessions).
    pub fn outcome(&self, run: &LiveRun) -> Outcome {
        let mut active = self.active.clone();
        active.violations = run.violations.clone();
        active.finish(run.to, &run.facts, !self.is_complete(run.sessions))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::practice::challenge;
    use crate::practice::tape::tests::{d, flat};
    use crate::simulator::SimTrade;

    fn session(date: &str, pnl_pct: f64, stop: f64, capital_used: f64) -> SimSession {
        SimSession {
            date: date.into(),
            market: "IN".into(),
            capital: 25_000.0,
            target_pct: 2.0,
            trades: vec![SimTrade {
                symbol: "TCS.NS".into(),
                direction: "BUY".into(),
                entry_price: 100.0,
                target1: 102.0,
                target2: 104.0,
                stop_loss: stop,
                qty: 10,
                capital: capital_used,
                score: 70.0,
                confidence: "HIGH".into(),
                strategies: vec![],
                entered_at: None,
                trail_distance: None,
                charges: None,
                exit_price: Some(100.0),
                pnl: Some(0.0),
                pnl_pct: Some(0.0),
                hit_target: None,
                hit_stop: None,
            }],
            total_pnl: Some(250.0 * pnl_pct),
            total_pnl_pct: Some(pnl_pct),
            win_count: Some(1),
            loss_count: Some(0),
            settled: true,
            cost_model: None,
        }
    }

    fn live(name: &str) -> LiveChallenge {
        let def = challenge::load(name).unwrap();
        let days = def.days;
        LiveChallenge {
            active: Active::new(def, 0, d("2026-10-01"), days),
            market: "IN".into(),
        }
    }

    #[test]
    fn evaluates_sessions_against_the_benchmark() {
        let c = live("beat-the-index");
        let mut history = SimHistory::default();
        history
            .sessions
            .push(session("2026-09-30", 5.0, 98.0, 10_000.0)); // before the start
        for (i, p) in [1.0, -0.5, 1.0].iter().enumerate() {
            history
                .sessions
                .push(session(&format!("2026-10-0{}", i + 1), *p, 98.0, 10_000.0));
        }
        let mut other = session("2026-10-02", 9.0, 98.0, 1.0);
        other.market = "US".into();
        history.sessions.push(other);
        let sessions = c.sessions(&history);
        assert_eq!(sessions.len(), 3);
        // Index: 100 on 30 Sep, then 101, 100, 100.
        let bench = flat("2026-09-30", &[100.0, 101.0, 100.0, 100.0]);
        let run = c.evaluate(&sessions, &bench);
        assert_eq!(run.input.nav.len(), 4);
        assert_eq!(run.input.benchmark, [100.0, 101.0, 100.0, 100.0]);
        assert!((run.facts.return_percent - 1.4995).abs() < 1e-3);
        assert!((run.facts.exposure_percent - 40.0).abs() < 1e-9);
        assert!(run.violations.is_empty());
        let out = c.outcome(&run);
        assert!(!out.passed);
        assert_eq!(out.reason, "abandoned"); // 3 of 30 sessions
    }

    #[test]
    fn counts_one_session_a_day_and_stops_at_the_length() {
        let mut c = live("steady-hands");
        c.active.end_day = 2;
        let mut history = SimHistory::default();
        history
            .sessions
            .push(session("2026-10-01", 1.0, 98.0, 5_000.0));
        history
            .sessions
            .push(session("2026-10-01", 9.0, 98.0, 5_000.0)); // same day again
        history
            .sessions
            .push(session("2026-10-02", 1.0, 98.0, 5_000.0));
        history
            .sessions
            .push(session("2026-10-05", -30.0, 98.0, 5_000.0)); // after the end
        let sessions = c.sessions(&history);
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].total_pnl_pct, Some(1.0));
        assert_eq!(sessions[1].date, "2026-10-02");
        assert!(c.is_complete(sessions.len()));
    }

    #[test]
    fn rules_are_checked_after_the_fact() {
        let mut c = live("position-limit");
        c.active.end_day = 2;
        let mut history = SimHistory::default();
        history
            .sessions
            .push(session("2026-10-01", 1.0, 98.0, 5_000.0));
        history
            .sessions
            .push(session("2026-10-02", 1.0, 98.0, 10_000.0)); // 40%
        let sessions = c.sessions(&history);
        let run = c.evaluate(&sessions, &flat("2026-09-30", &[100.0; 3]));
        assert_eq!(run.violations.len(), 1);
        assert!(run.input.hard_break);
        let out = c.outcome(&run);
        assert!(!out.passed);
        assert!(out.reason.contains("20%"), "{}", out.reason);

        let mut c = live("stop-loss");
        c.active.end_day = 1;
        let mut history = SimHistory::default();
        history
            .sessions
            .push(session("2026-10-01", 1.0, 0.0, 5_000.0));
        let run = c.evaluate(&c.sessions(&history), &[]);
        assert!(run.input.hard_break);
        assert_eq!(run.input.benchmark, [1.0, 1.0]);
    }
}
