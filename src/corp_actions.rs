//! Splits and bonus issues that happened after a lot was bought but are not
//! yet applied to it.
//!
//! The write paths (`trade sync`, `portfolio import`, broker sync) refuse to
//! add shares while one is pending. The read paths (`portfolio`, `analyze`,
//! `checkup`, `income`, `xray`, `wrap`) call [`pending_splits`] and value an
//! in-memory copy with the event applied ([`PendingSplits::adjusted`]), so a
//! 2:1 split shows no fake 50% loss before `portfolio actions --apply` is run.
//! Indian demergers ([`crate::portfolio::DEMERGERS`]) are detected the same
//! way, from the lots alone: the copy holds the new company's shares and the
//! parent at its reduced cost. The file itself is only changed by
//! `portfolio actions --apply`.

use crate::api::YahooClient;
use crate::portfolio::{Demerger, Holding, Lot, Portfolio, Split};
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
    /// Demergers not yet applied to a parent lot held on the record date:
    /// (held parent symbol, demerger). Found from the lots, no network.
    pub demergers: Vec<(String, &'static Demerger)>,
    /// Accounts a pending demerger leaves unchanged because child shares
    /// already there may or may not be its own (see
    /// [`Portfolio::apply_demerger`]), one message each.
    pub demerger_conflicts: Vec<String>,
}

impl PendingSplits {
    /// Only the demergers pending in `portfolio` (for callers with no network).
    pub fn offline(portfolio: &Portfolio) -> Self {
        let mut out = Self::default();
        out.set_demergers(portfolio);
        out
    }

    /// Find the demergers pending in `portfolio` and what applying them
    /// would leave in conflict.
    fn set_demergers(&mut self, portfolio: &Portfolio) {
        self.demergers = portfolio.pending_demergers();
        if self.demergers.is_empty() {
            return;
        }
        let mut trial = portfolio.clone();
        self.demerger_conflicts = self
            .demergers
            .iter()
            .flat_map(|(sym, d)| {
                trial
                    .apply_demerger(sym, d)
                    .conflicts
                    .into_iter()
                    .map(move |c| format!("{} demerger into {}: {}", sym, d.child, c))
            })
            .collect();
    }

    pub fn is_empty(&self) -> bool {
        self.by_symbol.is_empty() && self.demergers.is_empty()
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
    /// oldest first, so a demerger's entitlement counts the splits and
    /// bonuses before its record date. A demerger is applied as `portfolio
    /// actions --apply` would (a lot of the new company per parent lot, the
    /// parent's cost reduced). Splits and bonuses are each applied as a
    /// split (quantity up, price down, cost and date kept). A bonus issue
    /// gives the same share count; how Indian tax dates the bonus shares is
    /// left to `portfolio actions --apply`.
    pub fn adjusted(&self, portfolio: &Portfolio) -> Portfolio {
        if self.is_empty() {
            return portfolio.clone();
        }
        let mut p = portfolio.clone();
        let mut events: Vec<(&str, &str, Option<Split>, Option<&Demerger>)> = Vec::new();
        for (sym, splits) in &self.by_symbol {
            for s in splits {
                let as_split = Split {
                    bonus: None,
                    ..s.clone()
                };
                events.push((s.date.as_str(), sym.as_str(), Some(as_split), None));
            }
        }
        for (sym, d) in &self.demergers {
            events.push((d.record_date, sym.as_str(), None, Some(*d)));
        }
        // Stable: on the same day a split goes before a demerger.
        events.sort_by(|a, b| a.0.cmp(b.0).then(a.3.is_some().cmp(&b.3.is_some())));
        for (_, sym, split, demerger) in events {
            if let Some(s) = split {
                p.apply_split(sym, &s);
            }
            if let Some(d) = demerger {
                p.apply_demerger(sym, d);
            }
        }
        p
    }

    /// One line per symbol: "split pending: run `portfolio actions --apply`",
    /// then one per pending demerger.
    pub fn warnings(&self) -> Vec<String> {
        let splits = self.by_symbol.iter().map(|(sym, splits)| {
            format!(
                "{} split pending ({}): run `stockwise portfolio actions {} --apply`. Valued at the adjusted share count meanwhile.",
                sym,
                describe(splits),
                sym
            )
        });
        let demergers = self.demergers.iter().map(|(sym, d)| {
            format!(
                "{} demerger pending ({}): run `stockwise portfolio actions {} --apply`. Valued with the {} shares and the reduced cost meanwhile.",
                sym,
                d.describe(sym),
                sym,
                d.child_symbol(sym)
            )
        });
        let conflicts = self.demerger_conflicts.iter().cloned();
        splits.chain(demergers).chain(conflicts).collect()
    }

    /// Print [`PendingSplits::warnings`] in yellow.
    pub fn print(&self) {
        use colored::Colorize;
        for w in self.warnings() {
            println!("  {} {}", "!".yellow(), w);
        }
    }
}

/// `symbols` plus the new companies of the demergers pending in
/// `portfolio`, for a valuation made before [`PendingSplits::adjusted`]
/// adds their lots.
pub fn with_demerger_children(portfolio: &Portfolio, mut symbols: Vec<String>) -> Vec<String> {
    for (_, d) in portfolio.pending_demergers() {
        for child in portfolio.demerger_children(d) {
            if !symbols.contains(&child) {
                symbols.push(child);
            }
        }
    }
    symbols
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
    // Owned jobs: a stream over borrowed holdings trips the `Send` bound of
    // the HTTP and MCP handlers that call this.
    let jobs: Vec<(String, Option<String>, Holding)> = portfolio
        .holdings
        .iter()
        .filter(|h| !crate::market::is_namespaced(&h.symbol))
        .filter(|h| !crate::offmarket::is_off_market(&h.symbol))
        .map(|h| {
            let since = h.lots.iter().map(|l| l.units_date().to_string()).min();
            (h.symbol.clone(), since, h.clone())
        })
        .collect();
    let results: Vec<(String, Option<Vec<Split>>)> = futures_util::stream::iter(jobs)
        .map(
            |(sym, since, h): (String, Option<String>, Holding)| async move {
                let Some(since) = since else {
                    return (sym, Some(Vec::new()));
                };
                let found = crate::commands::portfolio::fetch_actions(client, &sym, &since)
                    .await
                    .ok()
                    .map(|(splits, _, _)| pending_of(&h, splits));
                (sym, found)
            },
        )
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
    // With the splits known, the trial runs them first (see `adjusted`).
    let after_splits = PendingSplits {
        by_symbol: out.by_symbol.clone(),
        ..PendingSplits::default()
    }
    .adjusted(portfolio);
    out.set_demergers(&after_splits);
    out
}

/// [`pending_splits`] with a fresh client; only the demergers (which need
/// no network) when Yahoo is unreachable.
pub async fn pending_splits_live(portfolio: &Portfolio) -> PendingSplits {
    if cfg!(test) || portfolio.holdings.is_empty() {
        return PendingSplits::offline(portfolio);
    }
    match YahooClient::new().await {
        Ok(client) => pending_splits(&client, portfolio).await,
        Err(_) => PendingSplits::offline(portfolio),
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
            demergers: vec![],
            demerger_conflicts: vec![],
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
            demergers: vec![],
            demerger_conflicts: vec![],
        };
        assert_eq!(pending.adjusted(&p).holding("TCS.NS").unwrap().shares, 20.0);
    }

    #[test]
    fn a_pending_demerger_shows_in_the_valuation_copy_only() {
        let p = portfolio_with("ITC.NS", 1000.0, 400.0, "2020-03-02");
        let pending = PendingSplits::offline(&p);
        assert!(!pending.is_empty());
        let adj = pending.adjusted(&p);
        let hotels = adj.holding("ITCHOTELS.NS").unwrap();
        assert_eq!(hotels.shares, 100.0);
        assert!(
            (hotels.cost_basis() + adj.holding("ITC.NS").unwrap().cost_basis() - 400_000.0).abs()
                < 1e-6
        );
        assert!(
            p.holding("ITCHOTELS.NS").is_none(),
            "the original is untouched"
        );
        let w = pending.warnings();
        assert_eq!(w.len(), 1);
        assert!(w[0].contains("demerger pending"), "{}", w[0]);
        assert!(
            w[0].contains("portfolio actions ITC.NS --apply"),
            "{}",
            w[0]
        );
        assert_eq!(
            with_demerger_children(&p, vec!["ITC.NS".into()]),
            vec!["ITC.NS".to_string(), "ITCHOTELS.NS".to_string()]
        );
        // Once applied, nothing is pending.
        assert!(PendingSplits::offline(&adj).is_empty());
    }
}
