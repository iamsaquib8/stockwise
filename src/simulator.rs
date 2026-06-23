use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

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

fn sim_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Cannot determine local data directory")?
        .join("stockwise");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("sim_history.json"))
}

impl SimHistory {
    pub fn load() -> Result<Self> {
        let path = sim_path()?;
        if !path.exists() {
            return Ok(SimHistory::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let history: SimHistory = serde_json::from_str(&data)?;
        Ok(history)
    }

    pub fn save(&self) -> Result<()> {
        let path = sim_path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn stats(&self) -> SimStats {
        let settled: Vec<&SimSession> = self.sessions.iter().filter(|s| s.settled).collect();
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
        let s = h.stats();
        assert_eq!(s.total_days, 0);
        assert_eq!(s.total_pnl, 0.0);
    }

    #[test]
    fn test_single_winning_day() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-04-01", 500.0, 3, 1));
        let s = h.stats();
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
        let s = h.stats();
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
        let s = h.stats();
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
        let s = h.stats();
        assert_eq!(s.total_days, 0); // unsettled shouldn't count
    }

    #[test]
    fn test_avg_daily_pnl() {
        let mut h = SimHistory::default();
        h.sessions.push(make_session("2026-04-01", 300.0, 2, 1));
        h.sessions.push(make_session("2026-04-02", 100.0, 1, 2));
        let s = h.stats();
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
        let s = h.stats();
        assert!(s.sharpe.is_some());
    }
}
