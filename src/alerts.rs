use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub symbol: String,
    pub condition: AlertCondition,
    pub target: f64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AlertCondition {
    Above,
    Below,
}

impl std::fmt::Display for AlertCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertCondition::Above => write!(f, "above"),
            AlertCondition::Below => write!(f, "below"),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AlertStore {
    pub alerts: Vec<Alert>,
}

fn alerts_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Cannot determine local data directory")?
        .join("stockwise");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("alerts.json"))
}

impl AlertStore {
    pub fn load() -> Result<Self> {
        let path = alerts_path()?;
        if !path.exists() {
            return Ok(AlertStore::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let store: AlertStore = serde_json::from_str(&data)?;
        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        let path = alerts_path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn add(&mut self, symbol: &str, condition: AlertCondition, target: f64) {
        self.alerts.push(Alert {
            symbol: symbol.to_uppercase(),
            condition,
            target,
            created_at: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        });
    }

    pub fn remove_by_index(&mut self, index: usize) -> bool {
        if index < self.alerts.len() {
            self.alerts.remove(index);
            true
        } else {
            false
        }
    }

    pub fn check(&self, symbol: &str, price: f64) -> Vec<&Alert> {
        self.alerts
            .iter()
            .filter(|a| {
                a.symbol == symbol.to_uppercase()
                    && match a.condition {
                        AlertCondition::Above => price >= a.target,
                        AlertCondition::Below => price <= a.target,
                    }
            })
            .collect()
    }
}
