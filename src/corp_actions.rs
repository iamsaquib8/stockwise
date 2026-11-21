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
use serde::{Deserialize, Serialize};

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
}

impl PendingSplits {
    /// Only the demergers pending in `portfolio` (for callers with no network).
    pub fn offline(portfolio: &Portfolio) -> Self {
        Self {
            demergers: portfolio.pending_demergers(),
            ..Self::default()
        }
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

    /// A copy of `portfolio` with every pending event applied to its lots.
    /// A demerger is applied as `portfolio actions --apply` would (a lot of
    /// the new company per parent lot, the parent's cost reduced). Splits
    /// and bonuses are each applied as a split (quantity up, price down,
    /// cost and date kept). A bonus issue gives the same share count; how
    /// Indian tax dates the bonus shares is left to `portfolio actions --apply`.
    pub fn adjusted(&self, portfolio: &Portfolio) -> Portfolio {
        if self.is_empty() {
            return portfolio.clone();
        }
        let mut p = portfolio.clone();
        for (sym, d) in &self.demergers {
            p.apply_demerger(sym, d);
        }
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
        splits.chain(demergers).collect()
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
    for (sym, d) in portfolio.pending_demergers() {
        let child = d.child_symbol(&sym);
        if !symbols.contains(&child) {
            symbols.push(child);
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
    let mut out = PendingSplits::offline(portfolio);
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

/// A rights issue: holders on `record_date` could buy `rights_shares` new
/// shares for every `held_shares` at `price` each. Subscribing was the
/// holder's choice, so `portfolio actions` only reports the entitlement;
/// shares bought are a purchase of their own (`portfolio add`), dated on
/// the allotment.
#[derive(Debug, PartialEq)]
pub struct RightsIssue {
    /// Tickers the company trades or traded under (NSE, no suffix).
    pub parents: &'static [&'static str],
    pub rights_shares: f64,
    pub held_shares: f64,
    /// Issue price per share, in the listing's currency.
    pub price: f64,
    pub record_date: &'static str,
    /// Anything else the holder needs (partly paid calls).
    pub note: &'static str,
    /// The company's own filing the figures come from.
    pub source: &'static str,
}

/// Rights issues `portfolio actions` reports. Add a row only from the
/// company's own filing, never from memory or news reports.
pub const RIGHTS_ISSUES: &[RightsIssue] = &[
    // Reliance Industries: 1 for 15 at Rs 1,257, record date 14 May 2020
    // (abridged letter of offer; RIL media release of 30 Apr 2020).
    RightsIssue {
        parents: &["RELIANCE"],
        rights_shares: 1.0,
        held_shares: 15.0,
        price: 1257.0,
        record_date: "2020-05-14",
        note: "partly paid: Rs 314.25 on application, Rs 314.25 in May 2021, Rs 628.50 in Nov 2021",
        source: "https://www.business.hsbc.bank.in/-/media/library/markets-selective/india/pdf/reliance-industries-limited-abridged-letter-of-offer.pdf",
    },
    // Bharti Airtel: 1 for 14 at Rs 535, record date 28 Sep 2021, allotted
    // 27 Oct 2021 (the company's first and final call notice).
    RightsIssue {
        parents: &["BHARTIARTL"],
        rights_shares: 1.0,
        held_shares: 14.0,
        price: 535.0,
        record_date: "2021-09-28",
        note: "partly paid: Rs 133.75 on application, Rs 401.25 final call in Mar 2026",
        source: "https://assets.airtel.in/static-assets/cms/investor/docs/Bharti-Airtel-First-and-Final-Call-Notice.pdf",
    },
];

/// Shares of a rights issue an account was entitled to.
#[derive(Debug, Clone, PartialEq)]
pub struct RightsEntitlement {
    pub symbol: String,
    pub account: Option<String>,
    pub issue: &'static RightsIssue,
    /// Whole rights shares (the exchange rounds entitlements down).
    pub shares: f64,
}

/// The rights issues in [`RIGHTS_ISSUES`] the holdings were entitled to,
/// per account, leaving out those already recorded (a lot dated within 120
/// days after the record date at the issue price).
pub fn rights_entitlements(portfolio: &Portfolio) -> Vec<RightsEntitlement> {
    let mut out = Vec::new();
    for h in &portfolio.holdings {
        let (base, _) = crate::portfolio::indian_base(&h.symbol);
        for issue in RIGHTS_ISSUES
            .iter()
            .filter(|r| r.parents.contains(&base.as_str()))
        {
            let recorded = h.lots.iter().any(|l| {
                (l.price - issue.price).abs() < 1e-6
                    && l.date.as_str() >= issue.record_date
                    && chrono::NaiveDate::parse_from_str(&l.date, "%Y-%m-%d")
                        .ok()
                        .zip(chrono::NaiveDate::parse_from_str(issue.record_date, "%Y-%m-%d").ok())
                        .is_some_and(|(l, r)| (l - r).num_days() <= 120)
            });
            if recorded {
                continue;
            }
            let mut accounts: Vec<(Option<String>, f64)> = Vec::new();
            let held = h
                .lots
                .iter()
                .filter(|l| l.date.as_str() < issue.record_date)
                .map(|l| {
                    (
                        l.account_key().map(str::to_string),
                        crate::portfolio::qty_on(l, issue.record_date),
                    )
                })
                .chain(
                    portfolio
                        .realized
                        .iter()
                        .filter(|s| {
                            s.symbol == h.symbol
                                && s.buy_date.as_str() < issue.record_date
                                && s.sell_date.as_str() >= issue.record_date
                        })
                        .map(|s| (s.account_key().map(str::to_string), s.qty)),
                );
            for (account, q) in held {
                match accounts.iter_mut().find(|(a, _)| *a == account) {
                    Some((_, t)) => *t += q,
                    None => accounts.push((account, q)),
                }
            }
            for (account, q) in accounts {
                let shares = (q * issue.rights_shares / issue.held_shares + 1e-9).floor();
                if shares >= 1.0 {
                    out.push(RightsEntitlement {
                        symbol: h.symbol.clone(),
                        account,
                        issue,
                        shares,
                    });
                }
            }
        }
    }
    out
}

/// What explains the difference between the ledger's and a broker's
/// quantity of one symbol, and so what to do about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DriftFix {
    /// The quantities match.
    InSync,
    /// A split or bonus Yahoo reports, not yet applied to the lots, makes
    /// up the difference.
    UnappliedSplit { detail: String },
    /// The broker holds shares of a demerger not yet recorded for `parent`.
    UnappliedDemerger { parent: String },
    /// The broker's quantity is a whole multiple (or fraction) of the
    /// ledger's: a split, bonus or consolidation nobody recorded, or a buy
    /// (sale) of `qty` shares.
    PossibleSplit { ratio: String, qty: f64 },
    /// The broker has `qty` more shares: a buy not recorded.
    MissingBuy { qty: f64 },
    /// The broker has `qty` fewer shares: a sale (or transfer out) not recorded.
    MissingSell { qty: f64 },
}

/// One symbol's quantity in the ledger next to the broker's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftRow {
    pub symbol: String,
    /// Shares in the lots of the broker's account (all lots when none
    /// names an account).
    pub ledger_qty: f64,
    pub broker_qty: f64,
    /// The broker's average price, when it gives one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub broker_avg: Option<f64>,
    pub fix: DriftFix,
}

impl DriftRow {
    pub fn in_sync(&self) -> bool {
        self.fix == DriftFix::InSync
    }

    /// The command or step that fixes the drift, in words.
    pub fn suggestion(&self) -> String {
        let sym = &self.symbol;
        match &self.fix {
            DriftFix::InSync => "in sync".into(),
            DriftFix::UnappliedSplit { detail } => format!(
                "split/bonus not applied ({detail}): run `stockwise portfolio actions {sym} --apply`"
            ),
            DriftFix::UnappliedDemerger { parent } => format!(
                "demerger of {parent} not recorded: run `stockwise portfolio actions {parent} --apply`"
            ),
            DriftFix::PossibleSplit { ratio, qty } => format!(
                "the broker holds {ratio} times the ledger: a split, bonus or consolidation not \
                 recorded (check `stockwise portfolio actions {sym}`, or record it with \
                 `portfolio demerger`/`rename` if it was a corporate restructuring), or a {} of {} \
                 share(s) not recorded",
                if self.broker_qty > self.ledger_qty {
                    "buy"
                } else {
                    "sale"
                },
                round6(*qty)
            ),
            DriftFix::MissingBuy { qty } => format!(
                "missing buy of {} share(s): import the tradebook, or add it with \
                 `stockwise portfolio add {sym} {} <price> --date <date>` (`portfolio sync --apply` \
                 adds it with today's date, marked estimated)",
                round6(*qty),
                round6(*qty)
            ),
            DriftFix::MissingSell { qty } => format!(
                "missing sale of {} share(s): record it with `stockwise portfolio sell {sym} {} <price> --date <date>` \
                 (or it was transferred out)",
                round6(*qty),
                round6(*qty)
            ),
        }
    }
}

fn round6(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// `r` as `n:d` when it is a whole multiple (2:1, 10:1) or a whole
/// fraction (1:5) up to 1:50 or 50:1, or a common bonus ratio (3:2, 5:4).
fn clean_ratio(r: f64) -> Option<String> {
    if !r.is_finite() || r <= 0.0 || (r - 1.0).abs() < 1e-9 {
        return None;
    }
    let near = |x: f64| (x - x.round()).abs() < 1e-6;
    if r > 1.0 && near(r) && r <= 50.0 {
        return Some(format!("{}:1", r.round()));
    }
    if r < 1.0 && near(1.0 / r) && 1.0 / r <= 50.0 {
        return Some(format!("1:{}", (1.0 / r).round()));
    }
    for (n, d) in [
        (3.0, 2.0),
        (4.0, 3.0),
        (5.0, 4.0),
        (6.0, 5.0),
        (5.0, 2.0),
        (7.0, 5.0),
    ] {
        if (r - n / d).abs() < 1e-9 {
            return Some(format!("{n}:{d}"));
        }
    }
    None
}

/// Compare the ledger with one broker's holdings (`broker` is its account
/// name, e.g. `zerodha`; `holdings` are (portfolio symbol, quantity,
/// average price)). Every symbol the broker holds is listed, and every
/// holding with lots in that account. The ledger is never changed: each
/// row says what explains the difference.
pub fn drift_report(
    portfolio: &Portfolio,
    broker: &str,
    holdings: &[(String, f64, Option<f64>)],
    pending: &PendingSplits,
) -> Vec<DriftRow> {
    // The account's lots of a holding: those naming it, or every lot when
    // none names an account.
    let lots_of = |sym: &str| -> Vec<Lot> {
        let Some(h) = portfolio.holding(sym) else {
            return Vec::new();
        };
        if h.lots.iter().any(|l| l.account_key() == Some(broker)) {
            h.lots
                .iter()
                .filter(|l| l.account_key() == Some(broker))
                .cloned()
                .collect()
        } else if h.lots.iter().all(|l| l.account_key().is_none()) {
            h.lots.clone()
        } else {
            Vec::new()
        }
    };
    let mut symbols: Vec<String> = holdings.iter().map(|(s, _, _)| s.to_uppercase()).collect();
    for h in &portfolio.holdings {
        if !crate::offmarket::is_off_market(&h.symbol)
            && h.lots.iter().any(|l| l.account_key() == Some(broker))
        {
            symbols.push(h.symbol.clone());
        }
    }
    symbols.sort();
    symbols.dedup();
    let demergers = portfolio.pending_demergers();
    symbols
        .into_iter()
        .map(|sym| {
            let lots = lots_of(&sym);
            let ledger: f64 = lots.iter().map(|l| l.qty).sum();
            let (broker_qty, broker_avg) = holdings
                .iter()
                .filter(|(s, _, _)| s.eq_ignore_ascii_case(&sym))
                .fold((0.0, None), |(q, a), (_, bq, ba)| (q + bq, a.or(*ba)));
            let diff = broker_qty - ledger;
            let splits = pending.for_symbol(&sym);
            let adjusted: f64 = lots.iter().map(|l| adjusted_qty(l, splits)).sum();
            let fix = if diff.abs() < 1e-6 {
                DriftFix::InSync
            } else if let Some((parent, _)) = demergers
                .iter()
                .find(|(p, d)| ledger <= 1e-9 && d.child_symbol(p).eq_ignore_ascii_case(&sym))
            {
                DriftFix::UnappliedDemerger {
                    parent: parent.clone(),
                }
            } else if !splits.is_empty() && (adjusted - broker_qty).abs() < 1e-6 {
                DriftFix::UnappliedSplit {
                    detail: describe(splits),
                }
            } else if let Some(ratio) = (ledger > 1e-9 && broker_qty > 1e-9)
                .then(|| clean_ratio(broker_qty / ledger))
                .flatten()
            {
                DriftFix::PossibleSplit {
                    ratio,
                    qty: diff.abs(),
                }
            } else if diff > 0.0 {
                DriftFix::MissingBuy { qty: diff }
            } else {
                DriftFix::MissingSell { qty: -diff }
            };
            DriftRow {
                symbol: sym,
                ledger_qty: ledger,
                broker_qty,
                broker_avg,
                fix,
            }
        })
        .collect()
}

/// The drift reports the daily read-only broker sync saved, one per broker.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SavedDrift {
    #[serde(default)]
    pub reports: Vec<DriftReport>,
}

/// One broker's holdings compared with the ledger.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftReport {
    /// The broker id (`zerodha`), which is also the account name.
    pub broker: String,
    /// `YYYY-MM-DD HH:MM`, local time.
    pub at: String,
    pub rows: Vec<DriftRow>,
}

impl DriftReport {
    pub fn drifted(&self) -> impl Iterator<Item = &DriftRow> {
        self.rows.iter().filter(|r| !r.in_sync())
    }
}

/// Where the daily sync keeps its reports.
pub const DRIFT_FILE: &str = "holdings-drift.json";

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

    fn zerodha_lot(qty: f64, price: f64, date: &str) -> Lot {
        let mut l = Lot::new(qty, price, date, "import:kite");
        l.account = Some("zerodha".into());
        l
    }

    #[test]
    fn the_sync_drift_report_explains_each_difference_and_changes_nothing() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", zerodha_lot(10.0, 1500.0, "2024-01-10"));
        p.add_lot("TCS.NS", zerodha_lot(5.0, 3500.0, "2023-01-10"));
        p.add_lot("SBIN.NS", zerodha_lot(20.0, 600.0, "2023-01-10"));
        p.add_lot("HDFCBANK.NS", zerodha_lot(8.0, 1600.0, "2023-01-10"));
        p.add_lot("WIPRO.NS", zerodha_lot(7.0, 400.0, "2023-01-10"));
        p.add_lot("ITC.NS", zerodha_lot(100.0, 400.0, "2020-03-02"));
        // Another account's lots are not this broker's.
        let mut other = Lot::new(3.0, 1500.0, "2024-02-01", "import:groww");
        other.account = Some("groww".into());
        p.add_lot("INFY.NS", other);
        let before = serde_json::to_string(&p).unwrap();
        let broker = vec![
            ("INFY.NS".to_string(), 10.0, Some(1500.0)),
            // A 2:1 split Yahoo reports but the lots have not taken.
            ("TCS.NS".to_string(), 10.0, Some(1750.0)),
            // Sold 5 without recording it.
            ("SBIN.NS".to_string(), 15.0, Some(600.0)),
            // Twice the ledger with no split known: maybe a bonus, maybe a buy.
            ("HDFCBANK.NS".to_string(), 16.0, Some(800.0)),
            // Not in the ledger at all.
            ("NTPC.NS".to_string(), 12.0, Some(350.0)),
            ("ITC.NS".to_string(), 100.0, Some(400.0)),
            // The demerger's new shares, not recorded yet.
            ("ITCHOTELS.NS".to_string(), 10.0, Some(150.0)),
        ];
        let pending = PendingSplits {
            by_symbol: vec![("TCS.NS".into(), vec![Split::new("2024-06-01", 2.0, 1.0)])],
            unchecked: vec![],
            demergers: p.pending_demergers(),
        };
        let rows = drift_report(&p, "zerodha", &broker, &pending);
        let fix = |sym: &str| {
            rows.iter()
                .find(|r| r.symbol == sym)
                .unwrap_or_else(|| panic!("{sym} missing: {rows:?}"))
                .fix
                .clone()
        };
        assert_eq!(
            fix("INFY.NS"),
            DriftFix::InSync,
            "the groww lot is left out"
        );
        assert!(matches!(fix("TCS.NS"), DriftFix::UnappliedSplit { .. }));
        assert_eq!(fix("SBIN.NS"), DriftFix::MissingSell { qty: 5.0 });
        assert_eq!(
            fix("HDFCBANK.NS"),
            DriftFix::PossibleSplit {
                ratio: "2:1".into(),
                qty: 8.0
            }
        );
        assert_eq!(fix("NTPC.NS"), DriftFix::MissingBuy { qty: 12.0 });
        assert_eq!(fix("ITC.NS"), DriftFix::InSync);
        assert_eq!(
            fix("ITCHOTELS.NS"),
            DriftFix::UnappliedDemerger {
                parent: "ITC.NS".into()
            }
        );
        // Held here, absent at the broker: a sale or transfer not recorded.
        assert_eq!(fix("WIPRO.NS"), DriftFix::MissingSell { qty: 7.0 });
        let r = rows.iter().find(|r| r.symbol == "TCS.NS").unwrap();
        assert!(
            r.suggestion().contains("portfolio actions TCS.NS --apply"),
            "{}",
            r.suggestion()
        );
        let r = rows.iter().find(|r| r.symbol == "NTPC.NS").unwrap();
        assert!(
            r.suggestion().contains("portfolio add NTPC.NS 12"),
            "{}",
            r.suggestion()
        );
        let report = DriftReport {
            broker: "zerodha".into(),
            at: "2026-11-21 16:00".into(),
            rows: rows.clone(),
        };
        assert_eq!(report.drifted().count(), 6);
        // Read-only.
        assert_eq!(serde_json::to_string(&p).unwrap(), before);
    }

    #[test]
    fn clean_ratios_cover_splits_consolidations_and_common_bonuses() {
        assert_eq!(clean_ratio(2.0).as_deref(), Some("2:1"));
        assert_eq!(clean_ratio(0.1).as_deref(), Some("1:10"));
        assert_eq!(clean_ratio(1.5).as_deref(), Some("3:2"));
        assert_eq!(clean_ratio(1.0), None);
        assert_eq!(clean_ratio(1.37), None);
    }

    #[test]
    fn a_rights_entitlement_is_reported_until_the_subscribed_lot_is_added() {
        let mut p = Portfolio::default();
        p.add_lot(
            "RELIANCE.NS",
            Lot::new(31.0, 1400.0, "2019-05-02", "manual"),
        );
        let r = rights_entitlements(&p);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].shares, 2.0, "31 / 15, rounded down");
        assert_eq!(r[0].issue.price, 1257.0);
        p.add_lot("RELIANCE.NS", Lot::new(2.0, 1257.0, "2020-06-12", "manual"));
        assert!(rights_entitlements(&p).is_empty());
        for issue in RIGHTS_ISSUES {
            assert!(issue.source.starts_with("https://"));
            assert!(issue.price > 0.0 && issue.rights_shares > 0.0 && issue.held_shares > 0.0);
        }
    }
}
