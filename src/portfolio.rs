use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: String,
    pub shares: f64,
    pub avg_cost: f64,
    pub added_at: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Portfolio {
    pub holdings: Vec<Holding>,
}

fn portfolio_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Cannot determine local data directory")?
        .join("stockwise");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("portfolio.json"))
}

impl Portfolio {
    pub fn load() -> Result<Self> {
        let path = portfolio_path()?;
        if !path.exists() {
            return Ok(Portfolio::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let portfolio: Portfolio = serde_json::from_str(&data)?;
        Ok(portfolio)
    }

    pub fn save(&self) -> Result<()> {
        let path = portfolio_path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn add(&mut self, symbol: &str, shares: f64, cost: f64) {
        // If already holding, average the cost
        if let Some(h) = self.holdings.iter_mut().find(|h| h.symbol == symbol) {
            let total_cost = h.shares * h.avg_cost + shares * cost;
            h.shares += shares;
            h.avg_cost = total_cost / h.shares;
        } else {
            self.holdings.push(Holding {
                symbol: symbol.to_uppercase(),
                shares,
                avg_cost: cost,
                added_at: chrono::Local::now().format("%Y-%m-%d").to_string(),
            });
        }
    }

    pub fn remove(&mut self, symbol: &str) -> bool {
        let before = self.holdings.len();
        self.holdings.retain(|h| h.symbol != symbol.to_uppercase());
        self.holdings.len() < before
    }
}
