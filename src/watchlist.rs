use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Watchlist {
    pub symbols: Vec<String>,
}

impl Watchlist {
    pub fn load() -> Result<Self> {
        let mut wl: Self = crate::store::load("watchlist.json")?;
        wl.repair_symbols();
        Ok(wl)
    }

    /// Undo the market suffix older versions put on namespaced symbols
    /// (`watch add MF:120503` stored `MF:120503.NS`), dropping a repeat that
    /// creates. The repair is saved with the next change to the watchlist.
    fn repair_symbols(&mut self) -> bool {
        let mut changed = false;
        let mut seen: Vec<String> = Vec::with_capacity(self.symbols.len());
        for sym in std::mem::take(&mut self.symbols) {
            let sym = match crate::market::repair_namespaced(&sym) {
                Some(fixed) => {
                    changed = true;
                    fixed
                }
                None => sym,
            };
            if seen.contains(&sym) {
                changed = true;
            } else {
                seen.push(sym);
            }
        }
        self.symbols = seen;
        changed
    }

    pub fn save(&self) -> Result<()> {
        crate::store::save("watchlist.json", self, false)
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
    fn fund_codes_stored_with_a_market_suffix_are_repaired() {
        let mut wl = Watchlist {
            symbols: vec![
                "MF:120503.NS".into(),
                "TCS.NS".into(),
                "MF:120503".into(),
                "MF:INF179K01UT0.L".into(),
            ],
        };
        assert!(wl.repair_symbols());
        assert_eq!(wl.symbols, ["MF:120503", "TCS.NS", "MF:INF179K01UT0"]);
        assert!(!wl.repair_symbols(), "idempotent");
        assert_eq!(wl.symbols.len(), 3);
    }

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
