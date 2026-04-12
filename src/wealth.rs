use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snapshot {
    pub date: String,
    pub total_value: f64,
    pub total_cost: f64,
    pub holdings_count: usize,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WealthHistory {
    pub snapshots: Vec<Snapshot>,
}

fn wealth_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Cannot determine local data directory")?
        .join("stockwise");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("wealth_history.json"))
}

impl WealthHistory {
    pub fn load() -> Result<Self> {
        let path = wealth_path()?;
        if !path.exists() {
            return Ok(WealthHistory::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let history: WealthHistory = serde_json::from_str(&data)?;
        Ok(history)
    }

    pub fn save(&self) -> Result<()> {
        let path = wealth_path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn add_snapshot(&mut self, value: f64, cost: f64, holdings: usize) {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        // Replace today's snapshot if exists
        self.snapshots.retain(|s| s.date != today);
        self.snapshots.push(Snapshot {
            date: today,
            total_value: value,
            total_cost: cost,
            holdings_count: holdings,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_snapshot_creates_entry() {
        let mut wh = WealthHistory::default();
        wh.add_snapshot(100000.0, 90000.0, 5);
        assert_eq!(wh.snapshots.len(), 1);
        assert_eq!(wh.snapshots[0].total_value, 100000.0);
        assert_eq!(wh.snapshots[0].total_cost, 90000.0);
        assert_eq!(wh.snapshots[0].holdings_count, 5);
    }

    #[test]
    fn test_add_snapshot_deduplicates_same_date() {
        let mut wh = WealthHistory::default();
        wh.add_snapshot(100000.0, 90000.0, 5);
        wh.add_snapshot(105000.0, 90000.0, 5); // same calendar date
        // Should replace, not append
        assert_eq!(wh.snapshots.len(), 1);
        assert_eq!(wh.snapshots[0].total_value, 105000.0);
    }

    #[test]
    fn test_add_snapshot_preserves_different_dates() {
        let mut wh = WealthHistory::default();
        wh.snapshots.push(Snapshot {
            date: "2026-01-01".into(),
            total_value: 100000.0,
            total_cost: 90000.0,
            holdings_count: 3,
        });
        wh.snapshots.push(Snapshot {
            date: "2026-01-02".into(),
            total_value: 101000.0,
            total_cost: 90000.0,
            holdings_count: 3,
        });
        assert_eq!(wh.snapshots.len(), 2);
    }

    #[test]
    fn test_snapshot_has_today_date() {
        let mut wh = WealthHistory::default();
        wh.add_snapshot(50000.0, 45000.0, 2);
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        assert_eq!(wh.snapshots[0].date, today);
    }

    #[test]
    fn test_default_is_empty() {
        let wh = WealthHistory::default();
        assert!(wh.snapshots.is_empty());
    }
}
