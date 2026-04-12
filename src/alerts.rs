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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_alert() {
        let mut store = AlertStore::default();
        store.add("RELIANCE.NS", AlertCondition::Above, 1500.0);
        assert_eq!(store.alerts.len(), 1);
        assert_eq!(store.alerts[0].symbol, "RELIANCE.NS");
        assert_eq!(store.alerts[0].target, 1500.0);
    }

    #[test]
    fn test_check_above_triggered() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 100.0);
        let triggered = store.check("TEST.NS", 105.0);
        assert_eq!(triggered.len(), 1);
    }

    #[test]
    fn test_check_above_not_triggered() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 100.0);
        let triggered = store.check("TEST.NS", 95.0);
        assert_eq!(triggered.len(), 0);
    }

    #[test]
    fn test_check_below_triggered() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Below, 100.0);
        let triggered = store.check("TEST.NS", 95.0);
        assert_eq!(triggered.len(), 1);
    }

    #[test]
    fn test_check_wrong_symbol() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 100.0);
        let triggered = store.check("OTHER.NS", 200.0);
        assert_eq!(triggered.len(), 0);
    }

    #[test]
    fn test_remove_by_index() {
        let mut store = AlertStore::default();
        store.add("A.NS", AlertCondition::Above, 100.0);
        store.add("B.NS", AlertCondition::Below, 50.0);
        assert!(store.remove_by_index(0));
        assert_eq!(store.alerts.len(), 1);
        assert_eq!(store.alerts[0].symbol, "B.NS");
    }

    #[test]
    fn test_remove_invalid_index() {
        let mut store = AlertStore::default();
        store.add("A.NS", AlertCondition::Above, 100.0);
        assert!(!store.remove_by_index(5));
        assert_eq!(store.alerts.len(), 1);
    }

    #[test]
    fn test_multiple_alerts_same_symbol() {
        let mut store = AlertStore::default();
        store.add("TEST.NS", AlertCondition::Above, 110.0);
        store.add("TEST.NS", AlertCondition::Below, 90.0);
        // Price at 85 triggers below only
        let triggered = store.check("TEST.NS", 85.0);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].condition, AlertCondition::Below);
        // Price at 115 triggers above only
        let triggered = store.check("TEST.NS", 115.0);
        assert_eq!(triggered.len(), 1);
        assert_eq!(triggered[0].condition, AlertCondition::Above);
    }

    #[test]
    fn test_condition_display() {
        assert_eq!(format!("{}", AlertCondition::Above), "above");
        assert_eq!(format!("{}", AlertCondition::Below), "below");
    }
}
