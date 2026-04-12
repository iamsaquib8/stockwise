use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Watchlist {
    pub symbols: Vec<String>,
}

fn watchlist_path() -> Result<PathBuf> {
    let dir = dirs::data_local_dir()
        .context("Cannot determine local data directory")?
        .join("stockwise");
    std::fs::create_dir_all(&dir)?;
    Ok(dir.join("watchlist.json"))
}

impl Watchlist {
    pub fn load() -> Result<Self> {
        let path = watchlist_path()?;
        if !path.exists() {
            return Ok(Watchlist::default());
        }
        let data = std::fs::read_to_string(&path)?;
        let wl: Watchlist = serde_json::from_str(&data)?;
        Ok(wl)
    }

    pub fn save(&self) -> Result<()> {
        let path = watchlist_path()?;
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, data)?;
        Ok(())
    }

    pub fn add(&mut self, symbol: &str) -> bool {
        let upper = symbol.to_uppercase();
        if self.symbols.contains(&upper) {
            return false;
        }
        self.symbols.push(upper);
        true
    }

    pub fn remove(&mut self, symbol: &str) -> bool {
        let upper = symbol.to_uppercase();
        let before = self.symbols.len();
        self.symbols.retain(|s| s != &upper);
        self.symbols.len() < before
    }
}
