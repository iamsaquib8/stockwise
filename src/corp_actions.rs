//! Splits and bonus issues that happened after a lot was bought but are not
//! yet applied to it.
//!
//! The write paths (`trade sync`, `portfolio import`, broker sync) refuse to
//! add shares while one is pending. The read paths (`portfolio`, `analyze`,
//! `checkup`, `income`, `xray`, `wrap`) call [`pending_splits`] and value an
//! in-memory copy with the event applied ([`PendingSplits::adjusted`]), so a
//! 2:1 split shows no fake 50% loss before `portfolio actions --apply` is run.
//! The file itself is only changed by `portfolio actions --apply`.

use crate::api::YahooClient;
use crate::portfolio::{Holding, Lot, Portfolio, Split};
use futures_util::StreamExt;

/// Yahoo requests in flight at once.
const CONCURRENCY: usize = 6;

/// Splits Yahoo reports after a lot's date that the lot has not taken yet,
/// per symbol.
#[derive(Debug, Clone, Default)]
pub struct PendingSplits {
    pub by_symbol: Vec<(String, Vec<Split>)>,
    /// Symbols whose corporate actions could not be fetched.
    pub unchecked: Vec<String>,
}

impl PendingSplits {
    pub fn is_empty(&self) -> bool {
        self.by_symbol.is_empty()
    }

    pub fn for_symbol(&self, symbol: &str) -> &[Split] {
        self.by_symbol
            .iter()
            .find(|(s, _)| s.eq_ignore_ascii_case(symbol))
            .map_or(&[], |(_, v)| v.as_slice())
    }

    pub fn symbols(&self) -> Vec<String> {
        self.by_symbol.iter().map(|(s, _)| s.clone()).collect()
    }

    /// The holding's share count with the pending events applied.
    pub fn adjusted_shares(&self, h: &Holding) -> f64 {
        let splits = self.for_symbol(&h.symbol);
        h.lots.iter().map(|l| adjusted_qty(l, splits)).sum()
    }

    /// A copy of `portfolio` with every pending event applied to its lots,
    /// each as a split (quantity up, price down, cost and date kept). A bonus
    /// issue gives the same share count; how Indian tax dates the bonus
    /// shares is left to `portfolio actions --apply`.
    pub fn adjusted(&self, portfolio: &Portfolio) -> Portfolio {
        if self.is_empty() {
            return portfolio.clone();
        }
        let mut p = portfolio.clone();
        for (sym, splits) in &self.by_symbol {
            for s in splits {
                let as_split = Split {
                    bonus: None,
                    ..s.clone()
                };
                p.apply_split(sym, &as_split);
            }
        }
        p
    }

    /// One line per symbol: "split pending: run `portfolio actions --apply`".
    pub fn warnings(&self) -> Vec<String> {
        self.by_symbol
            .iter()
            .map(|(sym, splits)| {
                format!(
                    "{} split pending ({}): run `stockwise portfolio actions {} --apply`. Valued at the adjusted share count meanwhile.",
                    sym,
                    describe(splits),
                    sym
                )
            })
            .collect()
    }

    /// Print [`PendingSplits::warnings`] in yellow.
    pub fn print(&self) {
        use colored::Colorize;
        for w in self.warnings() {
            println!("  {} {}", "!".yellow(), w);
        }
    }
}

/// `2:1 on 2024-10-28, 1:1 on ...`.
pub fn describe(splits: &[Split]) -> String {
    splits
        .iter()
        .map(|s| format!("{}:{} on {}", s.numerator, s.denominator, s.date))
        .collect::<Vec<_>>()
        .join(", ")
}

/// `lot`'s quantity with the `splits` it has not taken yet applied.
pub fn adjusted_qty(lot: &Lot, splits: &[Split]) -> f64 {
    splits
        .iter()
        .filter(|s| s.pending_for(lot))
        .map(Split::ratio)
        .filter(|r| r.is_finite() && *r > 0.0)
        .fold(lot.qty, |q, r| q * r)
}

/// Of `splits` (one symbol's, from Yahoo), those pending for any of `h`'s lots.
pub fn pending_of(h: &Holding, splits: Vec<Split>) -> Vec<Split> {
    splits
        .into_iter()
        .filter(|s| h.lots.iter().any(|l| s.pending_for(l)))
        .collect()
}

/// Ask Yahoo for the splits of every holding and keep those not yet applied
/// to its lots. Funds and SGBs (`MF:`, off-market) have none. A symbol whose
/// actions cannot be fetched is listed in `unchecked`, never an error.
pub async fn pending_splits(client: &YahooClient, portfolio: &Portfolio) -> PendingSplits {
    let jobs: Vec<&Holding> = portfolio
        .holdings
        .iter()
        .filter(|h| !crate::market::is_namespaced(&h.symbol))
        .filter(|h| !crate::offmarket::is_off_market(&h.symbol))
        .collect();
    let results: Vec<(String, Option<Vec<Split>>)> = futures_util::stream::iter(jobs)
        .map(|h| async move {
            let Some(since) = h.lots.iter().map(|l| l.units_date()).min() else {
                return (h.symbol.clone(), Some(Vec::new()));
            };
            let found = crate::commands::portfolio::fetch_actions(client, &h.symbol, since)
                .await
                .ok()
                .map(|(splits, _, _)| pending_of(h, splits));
            (h.symbol.clone(), found)
        })
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut out = PendingSplits::default();
    for (sym, found) in results {
        match found {
            Some(v) if !v.is_empty() => out.by_symbol.push((sym, v)),
            Some(_) => {}
            None => out.unchecked.push(sym),
        }
    }
    out.by_symbol.sort_by(|a, b| a.0.cmp(&b.0));
    out.unchecked.sort();
    out
}

/// [`pending_splits`] with a fresh client; empty when Yahoo is unreachable.
pub async fn pending_splits_live(portfolio: &Portfolio) -> PendingSplits {
    if cfg!(test) || portfolio.holdings.is_empty() {
        return PendingSplits::default();
    }
    match YahooClient::new().await {
        Ok(client) => pending_splits(&client, portfolio).await,
        Err(_) => PendingSplits::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn portfolio_with(sym: &str, qty: f64, price: f64, date: &str) -> Portfolio {
        let mut p = Portfolio::default();
        p.add_lot(sym, Lot::new(qty, price, date, "manual"));
        p
    }

    #[test]
    fn a_pending_two_for_one_split_doubles_shares_and_keeps_cost() {
        let p = portfolio_with("RELIANCE.NS", 10.0, 2800.0, "2024-01-10");
        let h = p.holding("RELIANCE.NS").unwrap();
        let split = Split::new("2024-10-28", 2.0, 1.0);
        let pending = PendingSplits {
            by_symbol: vec![(
                "RELIANCE.NS".into(),
                pending_of(h, vec![split.clone(), Split::new("2023-01-01", 5.0, 1.0)]),
            )],
            unchecked: vec![],
        };
        // The split before the purchase is not pending.
        assert_eq!(pending.for_symbol("RELIANCE.NS"), &[split]);
        assert_eq!(pending.adjusted_shares(h), 20.0);
        let adj = pending.adjusted(&p);
        let ah = adj.holding("RELIANCE.NS").unwrap();
        assert_eq!(ah.shares, 20.0);
        assert!((ah.cost_basis() - 28_000.0).abs() < 1e-6);
        // At the post-split price of 1,400 there is no loss.
        assert!((ah.shares * 1400.0 - ah.cost_basis()).abs() < 1e-6);
        assert_eq!(pending.warnings().len(), 1);
        assert!(pending.warnings()[0].contains("split pending"));
        assert!(pending.warnings()[0].contains("portfolio actions RELIANCE.NS --apply"));
        // The original is untouched.
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 10.0);
    }

    #[test]
    fn a_bonus_counts_as_shares_in_the_valuation_copy() {
        let p = portfolio_with("TCS.NS", 10.0, 3000.0, "2024-01-10");
        let mut bonus = Split::new("2024-06-01", 2.0, 1.0);
        bonus.bonus = Some("2024-06-03".into());
        let pending = PendingSplits {
            by_symbol: vec![("TCS.NS".into(), vec![bonus])],
            unchecked: vec![],
        };
        assert_eq!(pending.adjusted(&p).holding("TCS.NS").unwrap().shares, 20.0);
    }
}
