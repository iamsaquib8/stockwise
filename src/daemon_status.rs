//! What the intraday daemon is doing, for `stockwise serve` → `/metrics`.
//!
//! The daemon saves its day state after every tick, one file per market
//! (`daemon_state.json` for NSE, `daemon_state_us.json` for NYSE). This
//! module reads only the fields the metrics need, leniently, so the server
//! keeps working if the daemon's state gains fields. Paper positions only;
//! the daemon never places live orders.

use chrono::{DateTime, Utc};
use serde::Deserialize;

/// A status older than this means the daemon stopped. Its slowest loop, the
/// wait while the market is closed, saves at least every 30 minutes.
pub const STALE_AFTER_SECS: i64 = 45 * 60;

/// Markets whose daemon is always reported by the metrics (as `0` when it
/// has never run).
pub const MARKETS: [&str; 2] = ["IN", "US"];

/// Every market a daemon can run for, with its own state file.
const STATE_MARKETS: [&str; 9] = ["IN", "US", "UK", "DE", "JP", "HK", "SG", "CA", "AU"];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DaemonStatus {
    /// `IN` or `US`.
    pub market: String,
    /// ISO code of the currency of the money fields: `INR` for `IN`, `USD`
    /// for `US` (empty if the market is unknown).
    pub currency: String,
    /// Unix seconds of the last save.
    pub updated_at: i64,
    pub capital: f64,
    pub open_positions: usize,
    pub realized_pnl: f64,
    pub unrealized_pnl: f64,
    /// Realized + unrealized for the day.
    pub day_pnl: f64,
    /// True once the daily loss limit tripped; no new entries today.
    pub kill_switch: bool,
    /// The day is over and recorded in the simulator history.
    pub settled: bool,
}

#[derive(Deserialize)]
struct RawPosition {
    #[serde(default)]
    status: String,
    #[serde(default)]
    pnl: f64,
}

#[derive(Deserialize, Default)]
struct RawRisk {
    #[serde(default)]
    realized_pnl: f64,
    #[serde(default)]
    killed: bool,
}

#[derive(Deserialize)]
struct RawState {
    updated_at: DateTime<Utc>,
    #[serde(default)]
    market: String,
    #[serde(default)]
    capital: f64,
    #[serde(default)]
    positions: Vec<RawPosition>,
    #[serde(default)]
    risk: RawRisk,
    #[serde(default)]
    settled: bool,
}

/// ISO currency code of a market's paper money (`IN` → `INR`).
fn currency_code(market: &str) -> &'static str {
    use crate::market::Market;
    use clap::ValueEnum;
    match Market::from_str(market, true) {
        Ok(Market::In) => "INR",
        Ok(Market::Us) => "USD",
        Ok(Market::Global(x)) => x.info().currency,
        _ => "",
    }
}

impl DaemonStatus {
    /// Parse the daemon's saved state. Open positions are those still `Open`
    /// or partly booked (`T1Hit`), unless the day is `settled`: then the
    /// session is over (a missed square-off is handed to `sim settle` with
    /// its rows still marked `Open`), so nothing is open and nothing is
    /// unrealized.
    pub fn from_state_json(json: &str) -> Option<Self> {
        let raw: RawState = serde_json::from_str(json).ok()?;
        let open: Vec<&RawPosition> = raw
            .positions
            .iter()
            .filter(|p| !raw.settled && (p.status == "Open" || p.status == "T1Hit"))
            .collect();
        // fold from +0.0: an empty f64 sum is -0.0, which prints as `-0`.
        let unrealized = open.iter().fold(0.0, |acc, p| acc + p.pnl);
        let market = if raw.market.is_empty() {
            "IN".to_string()
        } else {
            raw.market.to_ascii_uppercase()
        };
        Some(Self {
            currency: currency_code(&market).to_string(),
            market,
            updated_at: raw.updated_at.timestamp(),
            capital: raw.capital,
            open_positions: open.len(),
            realized_pnl: raw.risk.realized_pnl,
            unrealized_pnl: unrealized,
            day_pnl: raw.risk.realized_pnl + unrealized,
            kill_switch: raw.risk.killed,
            settled: raw.settled,
        })
    }

    pub fn age_secs(&self, now: i64) -> i64 {
        (now - self.updated_at).max(0)
    }

    pub fn is_fresh(&self, now: i64) -> bool {
        self.age_secs(now) <= STALE_AFTER_SECS
    }
}

/// The last saved state of each market's daemon that has run against this
/// data directory (unreadable files are skipped).
pub fn load() -> Vec<DaemonStatus> {
    STATE_MARKETS
        .iter()
        .filter_map(|m| {
            let path = crate::store::path(crate::daemon::state_file(m)).ok()?;
            let json = std::fs::read_to_string(path).ok()?;
            DaemonStatus::from_state_json(&json)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape `daemon intraday` saves (extra fields are ignored).
    const STATE: &str = r#"{
        "date": "2026-10-21", "market": "IN", "capital": 25000.0, "target_pct": 2.0,
        "entered": true,
        "positions": [
            {"symbol": "TCS.NS", "status": "Open", "pnl": 30.0, "current_price": 4100.0},
            {"symbol": "INFY.NS", "status": "T1Hit", "pnl": 20.0},
            {"symbol": "SBIN.NS", "status": "StopHit", "pnl": -50.0}
        ],
        "risk": {"capital": 25000.0, "daily_loss_limit_pct": 3.0, "max_positions": 5,
                 "max_position_pct": 30.0, "realized_pnl": -50.0, "killed": false},
        "tick_count": 12, "settled": false,
        "updated_at": "2026-10-21T05:30:00Z"
    }"#;

    #[test]
    fn counts_open_positions_and_day_pnl() {
        let s = DaemonStatus::from_state_json(STATE).unwrap();
        assert_eq!(s.open_positions, 2);
        assert_eq!(s.unrealized_pnl, 50.0);
        assert_eq!(s.realized_pnl, -50.0);
        assert_eq!(s.day_pnl, 0.0);
        assert_eq!(s.capital, 25000.0);
        assert!(!s.kill_switch);
        assert!(!s.settled);
        assert_eq!(s.market, "IN");
        assert_eq!(s.updated_at, 1_792_560_600);
    }

    #[test]
    fn settled_day_reports_nothing_open() {
        // A missed square-off is handed to `sim settle` and the state is
        // marked settled, but the rows keep their `Open`/`T1Hit` status.
        let json = STATE.replace(r#""settled": false"#, r#""settled": true"#);
        let s = DaemonStatus::from_state_json(&json).unwrap();
        assert!(s.settled);
        assert_eq!(s.open_positions, 0);
        assert_eq!(s.unrealized_pnl, 0.0);
        assert_eq!(s.realized_pnl, -50.0);
        assert_eq!(s.day_pnl, -50.0);
    }

    #[test]
    fn currency_follows_the_market() {
        let us = STATE.replace(r#""market": "IN""#, r#""market": "US""#);
        assert_eq!(
            DaemonStatus::from_state_json(STATE).unwrap().currency,
            "INR"
        );
        assert_eq!(DaemonStatus::from_state_json(&us).unwrap().currency, "USD");
        let uk = STATE.replace(r#""market": "IN""#, r#""market": "UK""#);
        assert_eq!(DaemonStatus::from_state_json(&uk).unwrap().currency, "GBP");
    }

    #[test]
    fn unreadable_state_is_none() {
        assert!(DaemonStatus::from_state_json("{").is_none());
        assert!(DaemonStatus::from_state_json(r#"{"capital": 1.0}"#).is_none());
        let minimal = DaemonStatus::from_state_json(r#"{"updated_at": "2026-10-21T05:30:00Z"}"#);
        assert_eq!(minimal.unwrap().open_positions, 0);
    }

    #[test]
    fn staleness() {
        let s = DaemonStatus {
            updated_at: 1000,
            ..Default::default()
        };
        assert!(s.is_fresh(1000 + STALE_AFTER_SECS));
        assert!(!s.is_fresh(1001 + STALE_AFTER_SECS));
        assert_eq!(s.age_secs(500), 0);
    }
}
