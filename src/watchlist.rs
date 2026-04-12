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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_add_new_symbol() {
        let mut wl = Watchlist::default();
        assert!(wl.add("RELIANCE"));
        assert_eq!(wl.symbols, vec!["RELIANCE"]);
    }

    #[test]
    fn test_add_uppercases_symbol() {
        let mut wl = Watchlist::default();
        wl.add("reliance");
        assert_eq!(wl.symbols[0], "RELIANCE");
    }

    #[test]
    fn test_add_duplicate_returns_false() {
        let mut wl = Watchlist::default();
        assert!(wl.add("RELIANCE"));
        assert!(!wl.add("RELIANCE"));
        assert_eq!(wl.symbols.len(), 1);
    }

    #[test]
    fn test_add_duplicate_case_insensitive() {
        let mut wl = Watchlist::default();
        assert!(wl.add("reliance"));
        assert!(!wl.add("RELIANCE"));
        assert_eq!(wl.symbols.len(), 1);
    }

    #[test]
    fn test_add_multiple_symbols() {
        let mut wl = Watchlist::default();
        wl.add("RELIANCE");
        wl.add("TCS");
        wl.add("INFY");
        assert_eq!(wl.symbols.len(), 3);
    }

    #[test]
    fn test_remove_existing_symbol() {
        let mut wl = Watchlist::default();
        wl.add("RELIANCE");
        wl.add("TCS");
        assert!(wl.remove("RELIANCE"));
        assert_eq!(wl.symbols.len(), 1);
        assert_eq!(wl.symbols[0], "TCS");
    }

    #[test]
    fn test_remove_nonexistent_returns_false() {
        let mut wl = Watchlist::default();
        wl.add("RELIANCE");
        assert!(!wl.remove("TCS"));
        assert_eq!(wl.symbols.len(), 1);
    }

    #[test]
    fn test_remove_case_insensitive() {
        let mut wl = Watchlist::default();
        wl.add("RELIANCE");
        assert!(wl.remove("reliance"));
        assert!(wl.symbols.is_empty());
    }

    #[test]
    fn test_remove_from_empty() {
        let mut wl = Watchlist::default();
        assert!(!wl.remove("RELIANCE"));
    }
}
