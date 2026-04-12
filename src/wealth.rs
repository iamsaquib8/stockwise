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
