//! Holdings tracked lot by lot. Every purchase is a [`Lot`] with its own
//! quantity, price and date, so tax holding periods, FIFO sales, splits and
//! XIRR use real purchase dates. Sales are kept in `realized` and dividends in
//! `dividends`.
//!
//! The default portfolio is `portfolio.json`; named ones (`--portfolio NAME`)
//! live in `portfolios/NAME.json`. Files written by StockWise 0.3 and earlier
//! (one average cost per holding) are upgraded on first load, keeping a
//! one-time `.bak` copy of the old file.

use anyhow::{Context, Result, bail};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// File format version. 1 (no `version` field): one average cost per
/// holding. 2: lots, realized sales and dividends.
pub const SCHEMA_VERSION: u32 = 2;

/// Quantities closer than this are equal (shares are f64 for fractional US shares).
const EPS: f64 = 1e-9;

pub fn today() -> String {
    chrono::Local::now().format("%Y-%m-%d").to_string()
}

/// Parse a `YYYY-MM-DD` date given on the command line; it can't be in the future.
pub fn parse_date(s: &str) -> Result<NaiveDate> {
    let d = NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
        .with_context(|| format!("Invalid date '{}': use YYYY-MM-DD", s))?;
    if d > chrono::Local::now().date_naive() {
        bail!("Date {} is in the future", d);
    }
    Ok(d)
}

/// Parse a trade date as brokers export it: `2024-03-15`, `2024-03-15 09:30:00`,
/// `15-03-2024`, `15/03/2024`, `15-Mar-2024` or `2024/03/15`. A numeric
/// `a/b/YYYY` is day-first unless `month_first` (US files), or unless the
/// first number can only be a month.
pub fn parse_trade_date(raw: &str, month_first: bool) -> Option<NaiveDate> {
    let s = raw.trim().trim_matches('"');
    // Drop a time part: "2024-03-15T09:30:00" / "2024-03-15 09:30:00".
    let s = match s.split_once(['T', ' ']) {
        Some((date, time)) if time.contains(':') => date,
        _ => s,
    };
    for fmt in ["%Y-%m-%d", "%Y/%m/%d", "%d-%b-%Y", "%d %b %Y", "%d-%B-%Y"] {
        if let Ok(d) = NaiveDate::parse_from_str(s, fmt) {
            return Some(d);
        }
    }
    // Excel serial dates (e.g. 45366 for 2024-03-15) from .xlsx tradebooks.
    if let Ok(serial) = s.parse::<f64>() {
        let days = serial.floor() as i64;
        if (30_000..=60_000).contains(&days)
            && let Some(epoch) = NaiveDate::from_ymd_opt(1899, 12, 30)
        {
            return epoch.checked_add_signed(chrono::Duration::days(days));
        }
    }
    let parts: Vec<&str> = s.split(['/', '-', '.']).collect();
    if parts.len() != 3 || parts[2].len() != 4 {
        return None;
    }
    let a: u32 = parts[0].parse().ok()?;
    let b: u32 = parts[1].parse().ok()?;
    let y: i32 = parts[2].parse().ok()?;
    let (d, m) = if (month_first && a <= 12) || b > 12 {
        (b, a)
    } else {
        (a, b)
    };
    NaiveDate::from_ymd_opt(y, m, d)
}

fn date_of(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// A merger that paid shareholders of `old` with `new_shares` of `new` for
/// every `old_shares` held. Trades in `old` up to `through` (the record date)
/// are in old shares.
#[derive(Debug, PartialEq)]
pub struct Merger {
    pub old: &'static str,
    pub new: &'static str,
    pub new_shares: f64,
    pub old_shares: f64,
    /// Last day a trade in `old` can be dated, `YYYY-MM-DD`.
    pub through: &'static str,
}

/// Mergers whose old ticker an import must convert (not just rename): the
/// shares received keep the original cost and purchase date (Sec. 47(vii),
/// 49(2) of the Income-tax Act); only the quantity changes.
pub const MERGERS: &[Merger] = &[
    // HDFC Ltd into HDFC Bank, effective 1 Jul 2023, record date 13 Jul 2023:
    // 42 HDFC Bank shares for every 25 HDFC.
    Merger {
        old: "HDFC",
        new: "HDFCBANK",
        new_shares: 42.0,
        old_shares: 25.0,
        through: "2023-07-13",
    },
    // Mindtree into LTIMindtree, effective 14 Nov 2022, record date 24 Nov 2022:
    // 73 LTIMindtree (LTI) shares for every 100 Mindtree.
    Merger {
        old: "MINDTREE",
        new: "LTIM",
        new_shares: 73.0,
        old_shares: 100.0,
        through: "2022-11-24",
    },
];

/// A trade in an old ticker restated in the shares of the company that took
/// it over.
#[derive(Debug, Clone, PartialEq)]
pub struct MergerSwap {
    /// The new ticker, with the exchange suffix the old one had.
    pub symbol: String,
    pub qty: f64,
    pub price: f64,
    pub merger: &'static Merger,
}

/// Restate a dated trade in a merged-away ticker (`HDFC`, `MINDTREE.NS`) as
/// the acquirer's shares: quantity × ratio, price ÷ ratio, so the cost is
/// unchanged. `None` when the symbol is not a merged one, the row has no
/// date, or it is dated after the merger (a plain rename then).
pub fn merger_swap(symbol: &str, date: Option<&str>, qty: f64, price: f64) -> Option<MergerSwap> {
    let date = date?;
    let (base, suffix) = indian_base(symbol);
    let merger = MERGERS
        .iter()
        .find(|m| m.old == base && date <= m.through)?;
    Some(MergerSwap {
        symbol: format!("{}{}", merger.new, suffix),
        qty: qty * merger.new_shares / merger.old_shares,
        price: price * merger.old_shares / merger.new_shares,
        merger,
    })
}

/// `RELIANCE.NS` → (`RELIANCE`, `.NS`); `TCS` → (`TCS`, ``). Upper-cased.
fn indian_base(symbol: &str) -> (String, String) {
    let upper = symbol.trim().to_uppercase();
    match upper.rsplit_once('.') {
        Some((b, s)) if s == "NS" || s == "BO" => (b.to_string(), format!(".{s}")),
        _ => (upper, String::new()),
    }
}

/// An Indian demerger: holders of the parent on `record_date` received
/// `child_shares` of `child` for every `parent_shares` held, and kept their
/// parent shares.
///
/// Indian tax (Income-tax Act) treats it as no transfer (Sec. 47(vid)). The
/// parent's pre-demerger cost is split: `child_cost_pct` percent becomes the
/// cost of the new shares (Sec. 49(2C)) and the parent keeps the rest
/// (Sec. 49(2D)). The new shares take the parent's purchase date
/// (Explanation 1(i)(g) to Sec. 2(42A)), so each parent lot gets a child
/// lot of its own with the same date.
#[derive(Debug, PartialEq)]
pub struct Demerger {
    /// Tickers the parent trades or traded under (NSE, no suffix).
    pub parents: &'static [&'static str],
    /// The resulting company's ticker (NSE, no suffix).
    pub child: &'static str,
    pub child_shares: f64,
    pub parent_shares: f64,
    /// `YYYY-MM-DD`. Under T+1 settlement the ex-date is the record date, so
    /// a lot bought before this day was entitled; one bought on it was not.
    pub record_date: &'static str,
    /// Percent of the parent's pre-demerger cost that moves to the new shares.
    pub child_cost_pct: f64,
    /// The official notice the ratio, record date and percentage come from.
    pub source: &'static str,
}

/// Indian demergers `portfolio actions --apply` records. Every figure comes
/// from the company's own filing with the exchanges, cited per row. Add a
/// row only from such a filing (or a CBDT circular), never from memory or
/// news reports.
pub const DEMERGERS: &[Demerger] = &[
    // Reliance Industries → Reliance Strategic Investments (renamed Jio
    // Financial Services). RIL's letter to BSE/NSE of 19 Jul 2023: record
    // date Thursday 20 Jul 2023, 1 RSIL share for every 1 RIL share; cost
    // of acquisition 95.32% RIL, 4.68% RSIL.
    Demerger {
        parents: &["RELIANCE"],
        child: "JIOFIN",
        child_shares: 1.0,
        parent_shares: 1.0,
        record_date: "2023-07-20",
        child_cost_pct: 4.68,
        source: "https://www.ril.com/sites/default/files/2023-08/SEIntimation_ApportionmentofCost_0.pdf",
    },
    // ITC → ITC Hotels. ITC's letter to NSE/BSE/CSE of 28 Jan 2025 (revised;
    // the 27 Jan letter had a typo in its worked example only): record date
    // 6 Jan 2025, 1 ITCHL share for every 10 ITC shares; cost of acquisition
    // 86.49% ITC, 13.51% ITC Hotels.
    Demerger {
        parents: &["ITC"],
        child: "ITCHOTELS",
        child_shares: 1.0,
        parent_shares: 10.0,
        record_date: "2025-01-06",
        child_cost_pct: 13.51,
        source: "https://www.itcportal.com/content/dam/itc-corporate/pdfs/proposed-demerger-of-hotels-business/Apportionment%20of%20cost%20of%20acquisition%20of%20Ordinary%20Shares%20of%20ITC%20Limited%20and%20Equity%20Shares%20of%20ITC%20Hotels%20Limited%20-%20Revised.pdf",
    },
    // Tata Motors (renamed Tata Motors Passenger Vehicles, TATAMOTORS →
    // TMPV) → TML Commercial Vehicles (renamed Tata Motors Ltd, TMCV).
    // TMPV's letter to BSE/NSE of 12 Nov 2025: record date 14 Oct 2025,
    // 1 TMCV share for every 1 share held; cost of acquisition 68.85% TMPV,
    // 31.15% TMCV.
    Demerger {
        parents: &["TATAMOTORS", "TMPV"],
        child: "TMCV",
        child_shares: 1.0,
        parent_shares: 1.0,
        record_date: "2025-10-14",
        child_cost_pct: 31.15,
        source: "https://www.bseindia.com/xml-data/corpfiling/AttachHis/bece4d69-3276-4305-88a5-440ed2a2660e.pdf",
    },
];

/// The prefix of the adjustment a demerger leaves on the lots it touched.
const DEMERGER_PREFIX: &str = "demerger:";

/// The `source` of a lot received in a demerger.
pub const DEMERGER_SOURCE: &str = "demerger";

/// The demergers whose parent `symbol` (`ITC.NS`, `TATAMOTORS`) is.
pub fn demergers_for(symbol: &str) -> Vec<&'static Demerger> {
    let (base, _) = indian_base(symbol);
    DEMERGERS
        .iter()
        .filter(|d| d.parents.contains(&base.as_str()))
        .collect()
}

impl Demerger {
    /// The adjustment recorded on every lot it touched: `demerger:2023-07-20:JIOFIN`.
    pub fn id(&self) -> String {
        format!("{DEMERGER_PREFIX}{}:{}", self.record_date, self.child)
    }

    pub fn ratio(&self) -> f64 {
        self.child_shares / self.parent_shares
    }

    /// The child's ticker on the exchange `parent` (a held symbol) is on.
    pub fn child_symbol(&self, parent: &str) -> String {
        format!("{}{}", self.child, indian_base(parent).1)
    }

    /// Whether `lot` was held on the record date and has not received it yet.
    pub fn pending_for(&self, lot: &Lot) -> bool {
        lot.date.as_str() < self.record_date && !lot.adjustments.contains(&self.id())
    }

    /// Whether `sale` sold shares held on the record date (bought before it,
    /// sold on or after it) and was not yet adjusted for it.
    pub fn pending_for_sale(&self, sale: &Sale) -> bool {
        sale.buy_date.as_str() < self.record_date
            && sale.sell_date.as_str() >= self.record_date
            && !sale
                .origin
                .as_ref()
                .is_some_and(|o| o.adjustments.contains(&self.id()))
    }

    /// Whether `symbol` (any listing) is this demerger's parent.
    pub fn is_parent(&self, symbol: &str) -> bool {
        self.parents.contains(&indian_base(symbol).0.as_str())
    }

    /// Whether `symbol` (any listing) is this demerger's child.
    pub fn is_child(&self, symbol: &str) -> bool {
        indian_base(symbol).0 == self.child
    }

    /// `1 JIOFIN.NS for 1 on 2023-07-20, 4.68% of cost`.
    pub fn describe(&self, parent: &str) -> String {
        format!(
            "{} {} for {} on {}, {}% of cost",
            self.child_shares,
            self.child_symbol(parent),
            self.parent_shares,
            self.record_date,
            self.child_cost_pct
        )
    }
}

/// A quantity (of a lot or sale whose split record is `adjustments`) on
/// `date`: splits applied after that day are taken back out. A bonus
/// received as a lot of its own never changed the quantity.
fn qty_on(qty: f64, adjustments: &[String], date: &str) -> f64 {
    adjustments
        .iter()
        .filter_map(|a| {
            let mut parts = a.split(':');
            let (d, n, m) = (parts.next()?, parts.next()?, parts.next()?);
            if parts.next().is_some() || d <= date || date_of(d).is_none() {
                return None;
            }
            let r = n.parse::<f64>().ok()? / m.parse::<f64>().ok()?;
            (r.is_finite() && r > 0.0).then_some(r)
        })
        .fold(qty, |q, r| q / r)
}

/// Split `cost` between the parent and the child of a demerger: the child's
/// `pct` percent rounded to the paisa, the parent keeps the exact rest, so
/// the two always add up to `cost`. Amounts stay `f64` like every other
/// amount in the portfolio file. Returns (parent, child).
fn split_basis(cost: f64, pct: f64) -> (f64, f64) {
    let child = (cost * pct).round() / 100.0;
    (cost - child, child)
}

/// [`split_basis`] for an optional amount (charges).
fn split_charges(charges: Option<f64>, pct: f64) -> (Option<f64>, Option<f64>) {
    match charges {
        Some(c) => {
            let (p, ch) = split_basis(c, pct);
            (Some(p).filter(|x| *x > EPS), Some(ch).filter(|x| *x > EPS))
        }
        None => (None, None),
    }
}

/// What a demerger did.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DemergerOutcome {
    /// Open parent lots whose cost was split.
    pub lots: usize,
    /// Parent shares sold after the record date (before this was applied)
    /// whose sale cost was split.
    pub sales: usize,
    /// The child lots added, with the child symbol each went to.
    pub child_lots: Vec<(String, Lot)>,
    /// Fractions of a child share the company paid in cash (not recorded).
    pub fraction_dropped: f64,
    /// Child shares already in the portfolio (a broker snapshot) that were
    /// taken as this demerger's shares instead of being added again.
    pub reconciled: f64,
    /// Accounts left unchanged, one message each (see [`Portfolio::apply_demerger`]).
    pub conflicts: Vec<String>,
}

impl DemergerOutcome {
    pub fn child_shares(&self) -> f64 {
        self.child_lots.iter().map(|(_, l)| l.qty).sum()
    }

    pub fn child_cost(&self) -> f64 {
        self.child_lots.iter().map(|(_, l)| l.cost()).sum()
    }
}

/// Shares held on a record date, by account: (account, parent shares).
/// The company issues whole child shares per demat account, so the
/// entitlement is rounded down once per account, across every listing.
fn entitlements(items: &[(Option<String>, f64)], r: f64) -> Vec<(Option<String>, f64)> {
    let mut out: Vec<(Option<String>, f64)> = Vec::new();
    for (account, qty) in items {
        match out.iter_mut().find(|(a, _)| a == account) {
            Some((_, q)) => *q += qty * r,
            None => out.push((account.clone(), qty * r)),
        }
    }
    out
}

/// The child shares one holding of `share` exact child shares gets when its
/// account is entitled to `exact` in all: the account's whole shares shared
/// in proportion. Zero when the account gets no whole share.
fn allotted(share: f64, exact: f64) -> f64 {
    let whole = (exact + 1e-9).floor();
    if whole < 1.0 || exact <= EPS || share <= EPS {
        0.0
    } else {
        share * whole / exact
    }
}

/// A lot of `d`'s child: the parent's purchase date (the tax date), shares
/// counted as held only from the record date (see [`Lot::held_from`]).
fn child_lot(d: &Demerger, qty: f64, cost: f64, date: &str, account: Option<String>) -> Lot {
    let mut child = Lot::new(qty, cost / qty, date, DEMERGER_SOURCE);
    child.account = account;
    // The quantity is the child's share count on the record date, and the
    // shares were not in the account before it.
    child.units_as_of = Some(d.record_date.to_string());
    child.adjustments.push(d.id());
    child
}

/// Apply `d` to the parent `lots` of one listing and account held on its
/// record date and not yet adjusted for it (a tradebook replay). Each such
/// lot keeps its quantity and date and keeps `100 − child_cost_pct` percent
/// of its cost and charges; a child lot with the same date gets the rest.
/// Whole shares per account, shared among its lots in proportion; an
/// account entitled to less than one share gets none and its lots keep
/// their whole cost. Every lot touched is marked, so applying again does
/// nothing. `listing` is the parent symbol the lots are held under.
pub fn apply_demerger_to(lots: &mut [Lot], d: &Demerger, listing: &str) -> DemergerOutcome {
    let mut out = DemergerOutcome::default();
    let r = d.ratio();
    if !r.is_finite() || r <= 0.0 || !(0.0..100.0).contains(&d.child_cost_pct) {
        return out;
    }
    let pending: Vec<usize> = (0..lots.len())
        .filter(|&i| d.pending_for(&lots[i]))
        .collect();
    let held = |l: &Lot| qty_on(l.qty, &l.adjustments, d.record_date);
    let items: Vec<(Option<String>, f64)> = pending
        .iter()
        .map(|&i| (lots[i].account_key().map(str::to_string), held(&lots[i])))
        .collect();
    let accounts = entitlements(&items, r);
    for (&i, (account, qty)) in pending.iter().zip(items) {
        let exact = accounts
            .iter()
            .find(|(a, _)| *a == account)
            .map_or(0.0, |(_, q)| *q);
        let lot = &mut lots[i];
        lot.adjustments.push(d.id());
        out.lots += 1;
        let child_qty = allotted(qty * r, exact);
        out.fraction_dropped += (qty * r - child_qty).max(0.0);
        if child_qty <= EPS {
            continue;
        }
        let (parent_cost, child_cost) = split_basis(lot.cost(), d.child_cost_pct);
        let (parent_charges, child_charges) = split_charges(lot.charges, d.child_cost_pct);
        let mut child = child_lot(d, child_qty, child_cost, &lot.date, account);
        child.date_estimated = lot.date_estimated;
        child.fx_currency = lot.fx_currency.clone();
        child.fx_rate = lot.fx_rate;
        child.charges = child_charges;
        out.child_lots.push((d.child_symbol(listing), child));
        if lot.qty > EPS {
            lot.price = parent_cost / lot.qty;
        }
        lot.charges = parent_charges;
    }
    out
}

/// One purchase.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lot {
    pub qty: f64,
    /// Price paid per share (adjusted by any split or bonus applied since).
    pub price: f64,
    /// Purchase date, `YYYY-MM-DD`.
    pub date: String,
    /// Where the lot came from: `manual`, `import:kite`, `broker:angel`, `migrated`, ...
    pub source: String,
    /// True when `date` is not the real purchase date (upgraded from the old
    /// format, or a broker/import snapshot without dates). Tax and XIRR flag it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub date_estimated: bool,
    /// Splits and bonuses already applied to this lot (ids like `2024-10-28:2:1`),
    /// so `portfolio actions --apply` never applies one twice.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<String>,
    /// For a lot whose estimated date was corrected: the date its quantity
    /// was recorded. The quantity is already in that day's share count, so
    /// only splits after it can still apply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units_as_of: Option<String>,
    /// The broker order this lot was bought with (`trade sync`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    /// The demat account holding the lot (`zerodha`, `groww`, ...). FIFO,
    /// imports and syncs work per account. Lots saved without one belong to
    /// the account their `source` names, or to no account at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The currency the lot was paid for in, when it differs from the one it
    /// trades in (rupees for a US share): `portfolio add --currency`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_currency: Option<String>,
    /// Units of `fx_currency` paid per unit of the lot's own (major)
    /// currency on the purchase date. With it the cost in `fx_currency` is
    /// exactly what was paid (see [`Lot::base_cost`]); without it the cost is
    /// converted at the purchase date's reference rate (`fx_history`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_rate: Option<f64>,
    /// Brokerage, taxes and fees paid on the purchase, in the lot's own
    /// currency. Part of the cost of the shares sold out of it, so realized
    /// P&L is net of them; a partial sale takes its share.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charges: Option<f64>,
}

/// The demat account a source stands for: `import:kite` and `broker:zerodha`
/// are `zerodha`, `import:groww` is `groww`, `broker:angel` is `angel`.
/// `manual`, `sync`, `migrated` and sources without a broker (`import:csv`,
/// `import:cas`) name no account.
pub fn account_of_source(source: &str) -> Option<&str> {
    match source {
        "import:kite" => Some("zerodha"),
        "import:groww" => Some("groww"),
        "import:upstox" => Some("upstox"),
        "import:indmoney" => Some("indmoney"),
        s => s.strip_prefix("broker:").filter(|b| !b.is_empty()),
    }
}

impl Lot {
    pub fn new(qty: f64, price: f64, date: impl Into<String>, source: &str) -> Self {
        Self {
            qty,
            price,
            date: date.into(),
            source: source.to_string(),
            date_estimated: false,
            adjustments: Vec::new(),
            units_as_of: None,
            order_id: None,
            account: None,
            fx_currency: None,
            fx_rate: None,
            charges: None,
        }
    }

    /// What was paid in `currency` for the lot (its cost in its own major
    /// currency times [`Lot::fx_rate`]), when the lot was paid for in it.
    /// `cost` is the lot's cost in its own major currency.
    pub fn base_cost(&self, currency: &str, cost: f64) -> Option<f64> {
        let paid = self.fx_currency.as_deref()?;
        let rate = self.fx_rate.filter(|r| r.is_finite() && *r > 0.0)?;
        paid.eq_ignore_ascii_case(currency).then_some(cost * rate)
    }

    /// Take `take` shares out of the lot: its charges shrink in proportion.
    /// Returns the charges that went with the shares taken.
    fn shrink(&mut self, take: f64) -> Option<f64> {
        let before = self.qty;
        self.qty -= take;
        let charges = self.charges?;
        if before <= EPS {
            return None;
        }
        let gone = charges * (take / before).min(1.0);
        self.charges = Some(charges - gone).filter(|c| *c > EPS);
        Some(gone)
    }

    /// The lot's account: the one saved with it, else the one its source names.
    pub fn account_key(&self) -> Option<&str> {
        self.account
            .as_deref()
            .or_else(|| account_of_source(&self.source))
    }

    /// The day the lot's quantity is expressed in: its purchase date, or a
    /// later day when the quantity was recorded after splits (see `units_as_of`).
    pub fn units_date(&self) -> &str {
        match self.units_as_of.as_deref() {
            Some(d) if d > self.date.as_str() => d,
            _ => &self.date,
        }
    }

    pub fn cost(&self) -> f64 {
        self.qty * self.price
    }

    /// The first day the lot's shares were in the account: its purchase
    /// date, or for shares received in a demerger the record date (their
    /// `date` is the parent's purchase date, which tax uses). A sale, or a
    /// share count, before this day cannot include them.
    pub fn held_from(&self) -> &str {
        match self.units_as_of.as_deref() {
            Some(d) if self.source == DEMERGER_SOURCE && d > self.date.as_str() => d,
            _ => &self.date,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: String,
    /// Total quantity. Derived from `lots`; kept in the file so older
    /// StockWise versions can still read it.
    pub shares: f64,
    /// Average cost per share. Derived from `lots`.
    pub avg_cost: f64,
    /// When the holding was first added to StockWise (not a purchase date).
    #[serde(default)]
    pub added_at: String,
    /// Open lots, oldest first.
    #[serde(default)]
    pub lots: Vec<Lot>,
}

impl Holding {
    fn new(symbol: String, lots: Vec<Lot>) -> Self {
        let mut h = Self {
            symbol,
            shares: 0.0,
            avg_cost: 0.0,
            added_at: today(),
            lots,
        };
        h.recompute();
        h
    }

    /// Re-sort lots oldest first and refresh the derived `shares`/`avg_cost`.
    fn recompute(&mut self) {
        self.lots.retain(|l| l.qty > EPS);
        self.lots.sort_by(|a, b| a.date.cmp(&b.date));
        self.shares = self.lots.iter().map(|l| l.qty).sum();
        self.avg_cost = if self.shares > EPS {
            self.cost_basis() / self.shares
        } else {
            0.0
        };
    }

    pub fn cost_basis(&self) -> f64 {
        self.lots.iter().map(Lot::cost).sum()
    }

    pub fn has_estimated_dates(&self) -> bool {
        self.lots.iter().any(|l| l.date_estimated)
    }
}

/// Shares sold out of one lot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sale {
    pub symbol: String,
    pub qty: f64,
    pub buy_date: String,
    pub buy_price: f64,
    pub sell_date: String,
    pub sell_price: f64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub buy_date_estimated: bool,
    /// `manual` for `portfolio sell`, `import:<broker>` for a tradebook import.
    #[serde(default)]
    pub source: String,
    /// The broker sell order behind this sale (`trade sync`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    /// Bought and sold on the same day (netted before any older lot was
    /// touched). Indian tax treats it as speculative business income.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub intraday: bool,
    /// The demat account the shares were sold from (see [`Lot::account`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// What the sold lot carried besides its date and price, so
    /// `portfolio lots --undo-sale` can put it back exactly. Sales recorded
    /// before 0.9 have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<SoldLot>,
    /// Bonus issues received on the sold lot as a lot of their own
    /// (`bonus:<split id>`, see [`Split::bonus_id`]): the sold quantity is in
    /// the count before them, so dividends before them count those shares.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bonuses: Vec<String>,
    /// Sold out of a lot of bonus shares: allotted, not bought, so never
    /// intraday even when sold on the allotment date.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub bonus_shares: bool,
    /// Brokerage, taxes and fees of the sale plus the sold shares' part of
    /// the lot's purchase charges, in the holding's currency. Realized P&L
    /// is net of them ([`Sale::gain`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charges: Option<f64>,
    /// The lot's [`Lot::fx_currency`] and [`Lot::fx_rate`]: what the sold
    /// shares cost in the currency they were paid for in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx_currency: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub buy_fx_rate: Option<f64>,
}

/// The parts of a [`Lot`] a [`Sale`] does not otherwise keep.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SoldLot {
    pub source: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub adjustments: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units_as_of: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// The sold shares' part of the lot's purchase charges.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charges: Option<f64>,
}

/// How many earlier versions of a portfolio file are kept
/// (`portfolio.json.1` is the newest).
pub const BACKUPS_KEPT: usize = 5;

impl Sale {
    fn from_lot(symbol: &str, lot: &Lot, qty: f64, date: &str, price: f64, source: &str) -> Self {
        // `lot` is the lot as it was before the shares were taken.
        let buy_charges = lot
            .charges
            .filter(|_| lot.qty > EPS)
            .map(|c| c * (qty / lot.qty).min(1.0))
            .filter(|c| *c > EPS);
        Self {
            symbol: symbol.to_string(),
            qty,
            buy_date: lot.date.clone(),
            buy_price: lot.price,
            sell_date: date.to_string(),
            sell_price: price,
            buy_date_estimated: lot.date_estimated,
            source: source.to_string(),
            order_id: None,
            intraday: false,
            account: lot.account_key().map(str::to_string),
            origin: Some(SoldLot {
                source: lot.source.clone(),
                adjustments: lot.adjustments.clone(),
                units_as_of: lot.units_as_of.clone(),
                order_id: lot.order_id.clone(),
                account: lot.account.clone(),
                charges: buy_charges,
            }),
            bonuses: lot
                .adjustments
                .iter()
                .filter(|a| a.starts_with("bonus:"))
                .cloned()
                .collect(),
            bonus_shares: lot.source == BONUS_SOURCE,
            charges: buy_charges,
            fx_currency: lot.fx_currency.clone(),
            buy_fx_rate: lot.fx_rate,
        }
    }

    /// Add the sale's own charges (brokerage, STT, fees) to those it took
    /// from its lot.
    pub fn add_charges(&mut self, charges: f64) {
        if charges.is_finite() && charges > 0.0 {
            self.charges = Some(self.charges.unwrap_or(0.0) + charges);
        }
    }

    /// The sale's charges, 0 when none were recorded.
    pub fn charges(&self) -> f64 {
        self.charges.unwrap_or(0.0)
    }

    /// The lot this sale took its shares from, with the sold quantity.
    pub fn to_lot(&self) -> Lot {
        let mut lot = Lot::new(self.qty, self.buy_price, self.buy_date.clone(), "manual");
        lot.date_estimated = self.buy_date_estimated;
        lot.fx_currency = self.fx_currency.clone();
        lot.fx_rate = self.buy_fx_rate;
        match &self.origin {
            Some(o) => {
                lot.source = o.source.clone();
                lot.adjustments = o.adjustments.clone();
                lot.units_as_of = o.units_as_of.clone();
                lot.order_id = o.order_id.clone();
                lot.account = o.account.clone();
                lot.charges = o.charges;
            }
            None => {
                // An older sale does not say which splits its lot had taken.
                // Its quantity is in the share count of the sale date, so
                // only splits after that day can still apply.
                lot.account = self.account_key().map(str::to_string);
                if self.sell_date > self.buy_date {
                    lot.units_as_of = Some(self.sell_date.clone());
                }
            }
        }
        lot
    }

    /// The first day the sold shares were in the account: the purchase
    /// date, or the record date for shares received in a demerger (see
    /// [`Lot::held_from`]).
    pub fn held_from(&self) -> &str {
        match &self.origin {
            Some(o) if o.source == DEMERGER_SOURCE => match o.units_as_of.as_deref() {
                Some(d) if d > self.buy_date.as_str() => d,
                _ => &self.buy_date,
            },
            _ => &self.buy_date,
        }
    }

    /// The sale's account: the one saved with it, else the one its source names.
    pub fn account_key(&self) -> Option<&str> {
        self.account
            .as_deref()
            .or_else(|| account_of_source(&self.source))
    }

    /// Proceeds less cost less charges, in the holding's currency.
    pub fn gain(&self) -> f64 {
        self.qty * (self.sell_price - self.buy_price) - self.charges()
    }

    /// A same-day buy and sell: flagged when it was recorded, or bought and
    /// sold on one day with a real (not estimated) purchase date.
    pub fn is_intraday(&self) -> bool {
        self.intraday
            || (self.buy_date == self.sell_date && !self.buy_date_estimated && !self.bonus_shares)
    }
}

/// A cash dividend received on the shares held at its ex-date.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DividendReceived {
    pub symbol: String,
    pub ex_date: String,
    /// Per share, in today's share count (Yahoo adjusts for later splits).
    pub per_share: f64,
    /// Shares held at the ex-date, in today's share count.
    pub qty: f64,
    pub amount: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// The `source` of a lot of bonus shares (see [`Split::bonus`]).
pub const BONUS_SOURCE: &str = "bonus";

/// A stock split or bonus issue: `numerator` shares for every `denominator`
/// held. Yahoo reports Indian bonus issues as splits (1:1 bonus = 2:1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Split {
    /// The ex-date.
    pub date: String,
    pub numerator: f64,
    pub denominator: f64,
    /// For an Indian bonus issue: the allotment date. Indian tax treats
    /// bonus shares as a new purchase at zero cost on that date
    /// (Sec. 55(2)(aa)(iiia)), so the older lots keep their quantity and
    /// cost and the new shares become a lot of their own. `None`: a real
    /// split, or a bonus outside India, which multiplies the older lots.
    pub bonus: Option<String>,
}

impl Split {
    pub fn new(date: impl Into<String>, numerator: f64, denominator: f64) -> Self {
        Self {
            date: date.into(),
            numerator,
            denominator,
            bonus: None,
        }
    }

    pub fn ratio(&self) -> f64 {
        self.numerator / self.denominator
    }

    pub fn id(&self) -> String {
        format!("{}:{}:{}", self.date, self.numerator, self.denominator)
    }

    /// The adjustment recorded on a lot that received this event as bonus
    /// shares in a lot of their own (its quantity was left as it was).
    pub fn bonus_id(&self) -> String {
        format!("bonus:{}", self.id())
    }

    /// Whether this event was already applied to `lot`, as a split or as a bonus.
    pub fn applied_to(&self, lot: &Lot) -> bool {
        let (id, bonus) = (self.id(), self.bonus_id());
        lot.adjustments.iter().any(|a| *a == id || *a == bonus)
    }

    /// Whether `lot`'s quantity predates this split and was not yet adjusted for it.
    pub fn pending_for(&self, lot: &Lot) -> bool {
        lot.units_date() < self.date.as_str() && !self.applied_to(lot)
    }

    /// The allotment date when this is to be recorded as zero-cost bonus
    /// shares (a ratio above one); `None` for a split.
    fn bonus_allotment(&self) -> Option<&str> {
        self.bonus.as_deref().filter(|_| self.ratio() > 1.0)
    }
}

/// Apply `split` to the `lots` bought before it that were not yet adjusted.
/// A split multiplies quantity and divides price, keeping cost and date. A
/// bonus ([`Split::bonus`]) leaves those lots as they are and adds the new
/// shares, `qty × (ratio − 1)` per lot, as zero-cost lots dated on the
/// allotment date (one per account). Returns how many of the older lots
/// were adjusted. The lots are left sorted by date.
fn apply_split_to(lots: &mut Vec<Lot>, split: &Split) -> usize {
    let r = split.ratio();
    if !r.is_finite() || r <= 0.0 {
        return 0;
    }
    let id = split.id();
    let mut changed = 0;
    match split.bonus_allotment() {
        None => {
            for lot in lots.iter_mut().filter(|l| split.pending_for(l)) {
                lot.qty *= r;
                lot.price /= r;
                lot.adjustments.push(id.clone());
                changed += 1;
            }
        }
        Some(allotted) => {
            let mut bonus: Vec<(Option<String>, f64)> = Vec::new();
            for lot in lots.iter_mut().filter(|l| split.pending_for(l)) {
                let account = lot.account_key().map(str::to_string);
                let qty = lot.qty * (r - 1.0);
                match bonus.iter_mut().find(|(a, _)| *a == account) {
                    Some((_, q)) => *q += qty,
                    None => bonus.push((account, qty)),
                }
                lot.adjustments.push(split.bonus_id());
                changed += 1;
            }
            for (account, qty) in bonus {
                let mut lot = Lot::new(qty, 0.0, allotted, BONUS_SOURCE);
                lot.account = account;
                // Already in the share count after this event: never apply it again.
                lot.adjustments.push(id.clone());
                lots.push(lot);
            }
        }
    }
    if changed > 0 {
        lots.sort_by(|a, b| a.date.cmp(&b.date));
    }
    changed
}

/// A dividend per share as Yahoo reports it (adjusted for later splits).
#[derive(Debug, Clone, PartialEq)]
pub struct DividendEvent {
    pub ex_date: String,
    pub per_share: f64,
}

/// What a quantity-only sync did to a holding.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncChange {
    New,
    Unchanged,
    /// A lot for this many shares was added.
    Added(f64),
    /// This many shares were taken from the oldest lots (not recorded as a sale).
    Reduced(f64),
}

/// One row of a broker or CSV import.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportRow {
    pub symbol: String,
    pub qty: f64,
    pub price: f64,
    /// Purchase or trade date, when the file has one.
    pub date: Option<String>,
    /// A sell row from a tradebook.
    pub sell: bool,
    /// For a row restated after a merger ([`merger_swap`]): the merger's last
    /// trade date. The quantity is already in the acquirer's share count on
    /// that day, so splits and bonuses up to it must not be applied again.
    pub units_as_of: Option<String>,
}

/// A trade from an imported tradebook, kept so the next file for the same
/// broker is merged with it instead of replacing it. Quantities are as
/// traded (before any split).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImportedTrade {
    pub symbol: String,
    /// The import source, e.g. `import:kite`.
    pub source: String,
    pub sell: bool,
    pub qty: f64,
    pub price: f64,
    /// `YYYY-MM-DD`; `None` for a row without a date (treated as today).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// The demat account the file belongs to.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// See [`ImportRow::units_as_of`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub units_as_of: Option<String>,
}

impl ImportedTrade {
    fn same_trade(&self, other: &Self) -> bool {
        self.sell == other.sell
            && self.qty == other.qty
            && self.price == other.price
            && self.date == other.date
            && self.units_as_of == other.units_as_of
    }
}

/// `stored` plus the trades of `incoming` it does not have yet. A trade
/// counts as many times as it appears, so two identical fills in one file
/// stay two, while a trade in both files (a re-import, or two overlapping
/// downloads) is counted once.
fn merge_trades(stored: &[ImportedTrade], incoming: &[ImportedTrade]) -> Vec<ImportedTrade> {
    let mut out = stored.to_vec();
    let mut seen: Vec<&ImportedTrade> = Vec::new();
    for t in incoming {
        if seen.iter().any(|s| s.same_trade(t)) {
            continue;
        }
        seen.push(t);
        let have = stored.iter().filter(|s| s.same_trade(t)).count();
        let want = incoming.iter().filter(|s| s.same_trade(t)).count();
        for _ in have..want {
            out.push(t.clone());
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportOutcome {
    /// No dates in the file: existing lots kept, quantity matched.
    Synced(SyncChange),
    /// Dated rows: the symbol's lots rebuilt from the trades imported so far
    /// (this file merged with earlier files from the same source).
    Lots {
        lots: usize,
        sales: usize,
        /// Splits applied while replaying a tradebook.
        splits: usize,
    },
    /// Left unchanged: sells of `unmatched` shares have no earlier buy in the
    /// trades imported so far. The trades are kept, so importing the earlier
    /// tradebook completes the history.
    Unmatched { unmatched: f64 },
    /// Left unchanged: the holding has lots or sales from elsewhere (manual
    /// entries, another source, or an import made before StockWise kept
    /// imported trades) that rebuilding it would replace (allowed with
    /// `replace`).
    WouldReplace,
}

/// How [`Portfolio::apply_import`] treats a file.
#[derive(Debug, Clone, Copy)]
pub struct ImportOptions<'a> {
    /// Recorded on lots and sales, e.g. `import:kite`.
    pub source: &'a str,
    /// Rebuild dated symbols even if that replaces lots or sales from
    /// other sources.
    pub replace: bool,
    /// The file has a buy/sell column: quantities are as traded, so splits
    /// (from `splits`) are applied while replaying it.
    pub tradebook: bool,
    /// Known splits per symbol.
    pub splits: &'a [(String, Vec<Split>)],
    /// The demat account the file belongs to. `None`: the one `source` names
    /// (`import:kite` → `zerodha`); a source with none works on the whole
    /// holding.
    pub account: Option<&'a str>,
}

/// One execution from a broker's trade book, ready for
/// [`Portfolio::apply_broker_fills`].
#[derive(Debug, Clone, PartialEq)]
pub struct BrokerFill {
    /// `--broker` id, e.g. `zerodha`.
    pub broker: String,
    pub order_id: String,
    /// The broker's trade id, or a stand-in built from the order id,
    /// quantity, price and time when it has none.
    pub fill_id: String,
    /// Portfolio symbol, e.g. `INFY.NS`.
    pub symbol: String,
    pub sell: bool,
    pub qty: f64,
    pub price: f64,
    /// `YYYY-MM-DD`.
    pub date: String,
    /// `HH:MM:SS`, when the broker gives one; orders the fills of a day.
    pub time: Option<String>,
}

/// A broker fill `trade sync` already applied. Kept even after its lot is
/// sold or reduced, so running a sync twice never counts a fill twice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SyncedFill {
    pub broker: String,
    pub order_id: String,
    pub fill_id: String,
    pub date: String,
}

/// What one broker order's new fills did to the portfolio.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "result", rename_all = "snake_case")]
pub enum FillEffect {
    /// A buy: one new lot.
    Lot,
    /// A sell: shares taken FIFO, one sale per lot touched.
    Sold { sales: Vec<Sale> },
    /// A sell larger than the shares held on that date: left out (and not
    /// marked as synced) until the missing lots are added.
    Unmatched { held: f64 },
}

/// One broker order (its fills not yet synced, merged) and its effect.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OrderFill {
    pub symbol: String,
    pub order_id: String,
    pub side: &'static str,
    pub date: String,
    pub time: Option<String>,
    /// Total quantity of the new fills.
    pub qty: f64,
    /// Volume-weighted average price of the new fills.
    pub price: f64,
    /// How many fills were merged (more than one = a partial fill).
    pub fills: usize,
    #[serde(flatten)]
    pub effect: FillEffect,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct FillSummary {
    pub orders: Vec<OrderFill>,
    /// Fills skipped because an earlier sync applied them.
    pub already_synced: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Portfolio {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub holdings: Vec<Holding>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub realized: Vec<Sale>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dividends: Vec<DividendReceived>,
    /// Broker fills applied by `trade sync`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub broker_fills: Vec<SyncedFill>,
    /// Trades from imported tradebooks, so the next file is merged with them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imported: Vec<ImportedTrade>,
    /// Target allocation weights by symbol or sector (key -> percentage).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<String, f64>,
    /// Drift tolerance band in percent (e.g. 2.0 = +/-2%).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub drift_tolerance: Option<f64>,
}

impl Default for Portfolio {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            holdings: Vec::new(),
            realized: Vec::new(),
            dividends: Vec::new(),
            broker_fills: Vec::new(),
            imported: Vec::new(),
            targets: BTreeMap::new(),
            drift_tolerance: None,
        }
    }
}

/// Take `qty` shares from the oldest lots bought on or before `date` (lots
/// must be sorted oldest first). Returns the slices taken and any quantity
/// that could not be matched.
fn take_fifo(lots: &mut Vec<Lot>, qty: f64, date: &str) -> (Vec<(Lot, f64)>, f64) {
    let mut remaining = qty;
    let mut taken = Vec::new();
    for lot in lots.iter_mut().filter(|l| l.held_from() <= date) {
        if remaining <= EPS {
            break;
        }
        let take = if remaining >= lot.qty - EPS {
            lot.qty
        } else {
            remaining
        };
        taken.push((lot.clone(), take));
        lot.shrink(take);
        remaining -= take;
    }
    lots.retain(|l| l.qty > EPS);
    (taken, if remaining > EPS { remaining } else { 0.0 })
}

/// The lots and sales a list of trades produces.
struct Replay {
    lots: Vec<Lot>,
    sales: Vec<Sale>,
    /// Shares sold with no earlier buy.
    unmatched: f64,
    /// Splits applied while replaying.
    splits: usize,
    /// Child lots of the demergers applied while replaying, with their symbol.
    children: Vec<(String, Lot)>,
}

/// Replay `trades` (oldest first) for one symbol: buys become lots, sells
/// come out FIFO, and `splits` and `demergers` are applied as they happen,
/// so a sale after a demerger's record date carries the reduced cost.
fn replay_trades(
    sym: &str,
    trades: &[ImportedTrade],
    source: &str,
    account: Option<&str>,
    (splits, demergers): (&[Split], &[&Demerger]),
    today: &str,
) -> Replay {
    let new_lot = |qty: f64, price: f64, date: &str| {
        let mut lot = Lot::new(qty, price, date, source);
        lot.account = account.map(str::to_string);
        lot
    };
    let mut applied: Vec<String> = Vec::new();
    let mut children: Vec<(String, Lot)> = Vec::new();
    // Splits and demergers up to `date`, oldest first (a split dated on a
    // record date before the demerger). A split after a record date applied
    // ahead of it is taken back out of the entitlement (see `qty_on`).
    let apply_until = |lots: &mut Vec<Lot>,
                       date: &str,
                       applied: &mut Vec<String>,
                       children: &mut Vec<(String, Lot)>| {
        let mut events: Vec<(&str, Option<&Split>, Option<&Demerger>)> = splits
            .iter()
            .filter(|s| s.date.as_str() <= date)
            .map(|s| (s.date.as_str(), Some(s), None))
            .chain(
                demergers
                    .iter()
                    .filter(|d| d.record_date <= date)
                    .map(|d| (d.record_date, None, Some(*d))),
            )
            .collect();
        events.sort_by(|a, b| a.0.cmp(b.0).then(a.2.is_some().cmp(&b.2.is_some())));
        for (_, split, demerger) in events {
            if let Some(split) = split
                && apply_split_to(lots, split) > 0
                && !applied.contains(&split.id())
            {
                applied.push(split.id());
            }
            if let Some(d) = demerger {
                children.extend(apply_demerger_to(lots, d, sym).child_lots);
            }
        }
    };
    let mut lots: Vec<Lot> = Vec::new();
    let mut sales = Vec::new();
    let mut unmatched = 0.0;
    let date_of_trade = |t: &ImportedTrade| t.date.clone().unwrap_or_else(|| today.to_string());
    let mut i = 0;
    while i < trades.len() {
        // One day's trades (the list is sorted by date).
        let date = date_of_trade(&trades[i]);
        let end = trades[i..]
            .iter()
            .position(|t| date_of_trade(t) != date)
            .map_or(trades.len(), |n| i + n);
        let day = &trades[i..end];
        i = end;
        apply_until(&mut lots, &date, &mut applied, &mut children);
        let (all_buys, sells): (Vec<&ImportedTrade>, Vec<&ImportedTrade>) =
            day.iter().partition(|t| !t.sell);
        // A buy without a date is dated today only as an estimate: it is not
        // known to be the same day as a sale, so it never nets (intraday).
        let (buys, estimated): (Vec<&ImportedTrade>, Vec<&ImportedTrade>) =
            all_buys.into_iter().partition(|t| t.date.is_some());
        let bought: f64 = buys.iter().map(|t| t.qty).sum();
        let sold: f64 = sells.iter().map(|t| t.qty).sum();
        let net = bought.min(sold);
        let average =
            |v: &[&ImportedTrade], qty: f64| v.iter().map(|t| t.qty * t.price).sum::<f64>() / qty;
        if net > EPS {
            // Bought and sold on one day: the matched quantity is an
            // intraday sale at the day's average prices; only what is left
            // over touches the lots.
            let (buy_avg, sell_avg) = (average(&buys, bought), average(&sells, sold));
            let mut sale = Sale::from_lot(
                sym,
                &new_lot(net, buy_avg, &date),
                net,
                &date,
                sell_avg,
                source,
            );
            sale.intraday = true;
            sales.push(sale);
            if bought - net > EPS {
                let mut lot = new_lot(bought - net, buy_avg, &date);
                // Restated only when every buy of the day was, to the same day.
                let first = &buys[0].units_as_of;
                if buys.iter().all(|t| &t.units_as_of == first) {
                    lot.units_as_of = first.clone();
                }
                lots.push(lot);
                lots.sort_by(|a, b| a.date.cmp(&b.date));
            }
            for t in &estimated {
                let mut lot = new_lot(t.qty, t.price, &date);
                lot.date_estimated = true;
                lot.units_as_of = t.units_as_of.clone();
                lots.push(lot);
                lots.sort_by(|a, b| a.date.cmp(&b.date));
            }
            if sold - net > EPS {
                let (taken, left) = take_fifo(&mut lots, sold - net, &date);
                sales.extend(
                    taken
                        .iter()
                        .map(|(lot, q)| Sale::from_lot(sym, lot, *q, &date, sell_avg, source)),
                );
                unmatched += left;
            }
            continue;
        }
        for t in buys.into_iter().chain(estimated) {
            let mut lot = new_lot(t.qty, t.price, &date);
            lot.date_estimated = t.date.is_none();
            lot.units_as_of = t.units_as_of.clone();
            lots.push(lot);
            lots.sort_by(|a, b| a.date.cmp(&b.date));
        }
        for t in sells {
            let (taken, left) = take_fifo(&mut lots, t.qty, &date);
            sales.extend(
                taken
                    .iter()
                    .map(|(lot, q)| Sale::from_lot(sym, lot, *q, &date, t.price, source)),
            );
            unmatched += left;
        }
    }
    // Bring the remaining lots up to today's share count.
    apply_until(&mut lots, "9999-12-31", &mut applied, &mut children);
    Replay {
        lots,
        sales,
        unmatched,
        splits: applied.len(),
        children,
    }
}

/// Whether the lots and sales already recorded equal what a replay produced
/// (to the precision a CSV carries).
fn same_state(
    old_lots: &[&Lot],
    new_lots: &[Lot],
    old_sales: &[&Sale],
    new_sales: &[Sale],
) -> bool {
    let n = |x: f64| format!("{:.6}", x);
    let lot_key = |l: &Lot| format!("{}|{}|{}", l.date, n(l.qty), n(l.price));
    let sale_key = |s: &Sale| {
        format!(
            "{}|{}|{}|{}|{}",
            s.buy_date,
            s.sell_date,
            n(s.qty),
            n(s.buy_price),
            n(s.sell_price)
        )
    };
    let sorted = |mut v: Vec<String>| {
        v.sort();
        v
    };
    sorted(old_lots.iter().map(|l| lot_key(l)).collect())
        == sorted(new_lots.iter().map(lot_key).collect())
        && sorted(old_sales.iter().map(|s| sale_key(s)).collect())
            == sorted(new_sales.iter().map(sale_key).collect())
}

/// Which lots and sales of one symbol an account's import, sync or sale works
/// on. Another account's lots are never touched.
struct Scope {
    account: Option<String>,
    /// Lots and sales with no account (manual entries, older files) count as
    /// this account's, because no other account holds the symbol.
    claim: bool,
}

impl Scope {
    fn has(&self, key: Option<&str>) -> bool {
        match &self.account {
            // A source with no account works on the whole holding.
            None => true,
            Some(a) => key == Some(a.as_str()) || (key.is_none() && self.claim),
        }
    }

    fn lot(&self, l: &Lot) -> bool {
        self.has(l.account_key())
    }

    fn sale(&self, s: &Sale) -> bool {
        self.has(s.account_key())
    }
}

/// Run `f` on only the lots in `scope`, then put them back with the rest.
fn scoped<R>(lots: &mut Vec<Lot>, scope: &Scope, f: impl FnOnce(&mut Vec<Lot>) -> R) -> R {
    let (mut inside, mut rest): (Vec<Lot>, Vec<Lot>) =
        std::mem::take(lots).into_iter().partition(|l| scope.lot(l));
    let out = f(&mut inside);
    rest.extend(inside);
    *lots = rest;
    out
}

/// Like [`take_fifo`], but shares bought on `date` itself go first: a buy and
/// a sell on one day net against each other (intraday) before an older lot
/// is touched. The flag is true for the slices that came from that day.
/// A lot whose date is only an estimate (`date_estimated`: a holdings
/// snapshot or sync dated it today) is not known to be from that day, so it
/// is taken like an older lot and never flagged.
fn take_netting(lots: &mut Vec<Lot>, qty: f64, date: &str) -> (Vec<(Lot, f64, bool)>, f64) {
    let (mut same_day, mut older): (Vec<Lot>, Vec<Lot>) = std::mem::take(lots)
        .into_iter()
        // Bonus shares are allotted, not bought: never an intraday buy.
        .partition(|l| l.date == date && !l.date_estimated && l.source != BONUS_SOURCE);
    let (netted, rest) = take_fifo(&mut same_day, qty, date);
    let (before, left) = take_fifo(&mut older, rest, date);
    older.extend(same_day);
    older.sort_by(|a, b| a.date.cmp(&b.date));
    *lots = older;
    let taken = netted
        .into_iter()
        .map(|(l, q)| (l, q, true))
        .chain(before.into_iter().map(|(l, q)| (l, q, false)))
        .collect();
    (taken, left)
}

impl Portfolio {
    /// Load the active portfolio (see [`select`]).
    pub fn load() -> Result<Self> {
        Self::load_named(active())
    }

    pub fn load_named(name: Option<&str>) -> Result<Self> {
        Self::load_from(&crate::store::path(&file_name(name))?)
    }

    /// Change the active portfolio under a lock: take `<file>.lock`, load the
    /// current copy, apply `change`, save, unlock. Another command that
    /// saved in the meantime is kept, not overwritten. Do network work first
    /// and apply only its result here; a change that fails saves nothing.
    pub fn update<R>(change: impl FnOnce(&mut Self) -> Result<R>) -> Result<R> {
        let path = crate::store::path(&file_name(active()))?;
        let _lock = crate::store::lock(&path)?;
        let mut p = Self::load_from(&path)?;
        let out = change(&mut p)?;
        p.save_to(&path)?;
        Ok(out)
    }

    /// Save to the *active* portfolio's file — not necessarily the one a
    /// `load_named` call read.
    pub fn save(&self) -> Result<()> {
        self.save_to(&crate::store::path(&file_name(active()))?)
    }

    /// Set a target weight for a symbol or sector.
    pub fn set_target(&mut self, key: impl Into<String>, percentage: f64) {
        self.targets.insert(key.into(), percentage);
    }

    /// Remove a target weight by symbol or sector (exact or case-insensitive).
    pub fn remove_target(&mut self, key: &str) -> bool {
        if self.targets.remove(key).is_some() {
            return true;
        }
        let matching = self
            .targets
            .keys()
            .find(|k| k.eq_ignore_ascii_case(key))
            .cloned();
        if let Some(k) = matching {
            self.targets.remove(&k).is_some()
        } else {
            false
        }
    }

    /// Clear all configured target weights.
    pub fn clear_targets(&mut self) {
        self.targets.clear();
    }

    /// Set or clear the default drift tolerance band in percent.
    pub fn set_drift_tolerance(&mut self, tolerance: Option<f64>) {
        self.drift_tolerance = tolerance;
    }

    /// Load `path`, upgrading an old-format file in place: the original is
    /// copied to `<file>.bak` (only if no backup exists yet), then the new
    /// format is written.
    /// Read `path` without writing anything: the format is upgraded and
    /// symbols repaired in memory only. For backups, which must stay as saved.
    pub fn read_only(path: &Path) -> Result<Self> {
        let mut p: Self = crate::store::load_from(path)?;
        if p.version > SCHEMA_VERSION {
            bail!(
                "{} was written by a newer StockWise (format {}). Upgrade StockWise to read it.",
                path.display(),
                p.version
            );
        }
        p.normalize();
        p.repair_symbols();
        Ok(p)
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        let mut p: Self = crate::store::load_from(path)?;
        if p.version > SCHEMA_VERSION {
            bail!(
                "{} was written by a newer StockWise (format {}). Upgrade StockWise to read it.",
                path.display(),
                p.version
            );
        }
        let upgrade = path.exists() && p.version < SCHEMA_VERSION;
        p.normalize();
        let repaired = p.repair_symbols();
        if repaired && !upgrade && path.exists() {
            // Best effort: a read-only data directory (or `serve`) still
            // loads, with the repair kept in memory until the next save.
            if let Err(e) = p.save_repaired(path) {
                eprintln!("  Warning: repaired fund symbols in memory only ({e:#})");
            }
            return Ok(p);
        }
        if upgrade {
            let bak = backup_path(path);
            if !bak.exists() {
                std::fs::copy(path, &bak).with_context(|| {
                    format!("Cannot back up {} before upgrading it", path.display())
                })?;
            }
            p.save_to(path)?;
        }
        Ok(p)
    }

    /// Write `path`, first moving the current file to `<file>.1` (and older
    /// copies up to `<file>.5`), so `portfolio restore` can bring one back.
    /// A save that changes nothing rotates nothing.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        self.save_with(
            path,
            |a: &Path, b: &Path| std::fs::rename(a, b),
            |p: &Path| crate::store::save_to(p, self, false),
        )
    }

    /// [`Portfolio::save_to`] with the rename and the final write passed in,
    /// so tests can make either fail.
    fn save_with(
        &self,
        path: &Path,
        rename: impl Fn(&Path, &Path) -> std::io::Result<()>,
        write: impl FnOnce(&Path) -> Result<()>,
    ) -> Result<()> {
        if let Ok(old) = std::fs::read(path) {
            let new = serde_json::to_string_pretty(self)?;
            // Backup 1 already holding the current file means an earlier save
            // rotated and then failed to write: the history is in place, so a
            // retry must not shift it again.
            let kept = std::fs::read(numbered_backup(path, 1)).is_ok_and(|b| b == old);
            if old != new.as_bytes() && !kept {
                rotate_backups_with(path, &rename)?;
            }
        }
        write(path)
    }

    /// Replace `path` with its backup `n` (1 = newest). The file being
    /// replaced becomes backup 1, so a restore can itself be restored.
    pub fn restore_backup(path: &Path, n: usize) -> Result<Self> {
        let from = numbered_backup(path, n);
        if !from.exists() {
            bail!("No backup #{} of {}", n, path.display());
        }
        let p = Self::read_only(&from)
            .with_context(|| format!("Backup {} is not a readable portfolio", from.display()))?;
        p.save_to(path)?;
        Ok(p)
    }

    /// Save symbols repaired on load, keeping the original once as `.bak`.
    fn save_repaired(&self, path: &Path) -> Result<()> {
        let bak = backup_path(path);
        if !bak.exists() {
            std::fs::copy(path, &bak).with_context(|| {
                format!("Cannot back up {} before repairing it", path.display())
            })?;
        }
        self.save_to(path)
    }

    /// Give every old-format holding a single lot (dated when it was added to
    /// StockWise, flagged as estimated) and refresh derived fields.
    fn normalize(&mut self) {
        for h in &mut self.holdings {
            if h.lots.is_empty() && h.shares > EPS {
                let date = if date_of(&h.added_at).is_some() {
                    h.added_at.clone()
                } else {
                    today()
                };
                let mut lot = Lot::new(h.shares, h.avg_cost, date, "migrated");
                lot.date_estimated = true;
                h.lots.push(lot);
            }
            h.recompute();
        }
        self.version = SCHEMA_VERSION;
    }

    /// Undo the market suffix older versions put on namespaced symbols
    /// (`portfolio add MF:120503` stored `MF:120503.NS`), merging a holding
    /// that then appears twice. Returns whether anything changed; running it
    /// again changes nothing.
    fn repair_symbols(&mut self) -> bool {
        fn repair(symbol: &mut String) -> bool {
            match crate::market::repair_namespaced(symbol) {
                Some(fixed) => {
                    *symbol = fixed;
                    true
                }
                None => false,
            }
        }
        let mut changed = false;
        for h in &mut self.holdings {
            changed |= repair(&mut h.symbol);
        }
        for s in &mut self.realized {
            changed |= repair(&mut s.symbol);
        }
        for d in &mut self.dividends {
            changed |= repair(&mut d.symbol);
        }
        if changed {
            let mut merged: Vec<Holding> = Vec::with_capacity(self.holdings.len());
            for h in std::mem::take(&mut self.holdings) {
                match merged.iter_mut().find(|m| m.symbol == h.symbol) {
                    Some(m) => {
                        m.lots.extend(h.lots);
                        if !h.added_at.is_empty()
                            && (m.added_at.is_empty() || h.added_at < m.added_at)
                        {
                            m.added_at = h.added_at;
                        }
                        m.recompute();
                    }
                    None => merged.push(h),
                }
            }
            self.holdings = merged;
        }
        changed
    }

    pub fn holding(&self, symbol: &str) -> Option<&Holding> {
        let sym = symbol.to_uppercase();
        self.holdings.iter().find(|h| h.symbol == sym)
    }

    fn holding_mut(&mut self, symbol: &str) -> Option<&mut Holding> {
        let sym = symbol.to_uppercase();
        self.holdings.iter_mut().find(|h| h.symbol == sym)
    }

    /// The lots and sales of `symbol` that `account` works on (see [`Scope`]).
    fn scope(&self, symbol: &str, account: Option<&str>) -> Scope {
        let Some(a) = account else {
            return Scope {
                account: None,
                claim: true,
            };
        };
        let sym = symbol.to_uppercase();
        let in_lots = self
            .holding(&sym)
            .into_iter()
            .flat_map(|h| h.lots.iter().map(Lot::account_key));
        let in_sales = self
            .realized
            .iter()
            .filter(|s| s.symbol == sym)
            .map(Sale::account_key);
        let elsewhere = in_lots.chain(in_sales).any(|k| k.is_some_and(|k| k != a));
        Scope {
            account: Some(a.to_string()),
            claim: !elsewhere,
        }
    }

    /// Add a purchase as a new lot.
    pub fn add_lot(&mut self, symbol: &str, lot: Lot) {
        match self.holding_mut(symbol) {
            Some(h) => {
                h.lots.push(lot);
                h.recompute();
            }
            None => self
                .holdings
                .push(Holding::new(symbol.to_uppercase(), vec![lot])),
        }
    }

    /// Add shares bought today.
    pub fn add(&mut self, symbol: &str, shares: f64, cost: f64) {
        self.add_lot(symbol, Lot::new(shares, cost, today(), "manual"));
    }

    /// Mirror a broker or import snapshot. See [`Portfolio::sync_quantity`].
    pub fn set(&mut self, symbol: &str, shares: f64, avg_cost: f64) {
        self.sync_quantity(symbol, shares, avg_cost, "sync");
    }

    /// Match a snapshot that has a quantity and average cost but no purchase
    /// dates (a broker holdings list). Existing lots are kept and only the
    /// quantity changes, so running a sync twice never doubles a position:
    /// more shares add a lot for the difference (priced so the total cost
    /// matches the snapshot when that gives a sensible price), fewer shares
    /// come out of the oldest lots. A reduction is not recorded as a sale —
    /// its price is unknown.
    ///
    /// The snapshot is one broker's: it is matched against the lots of the
    /// account `source` names (`import:groww` → `groww`), so a second
    /// broker's file adds to the first broker's shares instead of replacing
    /// them. A source with no account matches the whole holding.
    pub fn sync_quantity(&mut self, symbol: &str, qty: f64, avg: f64, source: &str) -> SyncChange {
        self.sync_account(symbol, qty, avg, source, account_of_source(source))
    }

    fn sync_account(
        &mut self,
        symbol: &str,
        qty: f64,
        avg: f64,
        source: &str,
        account: Option<&str>,
    ) -> SyncChange {
        let scope = self.scope(symbol, account);
        let mut lot = Lot::new(qty, avg, today(), source);
        lot.date_estimated = true;
        lot.account = account.map(str::to_string);
        let Some(h) = self.holding_mut(symbol) else {
            self.add_lot(symbol, lot);
            return SyncChange::New;
        };
        // Lots with no account join this account when no other holds the symbol.
        if let (Some(a), true) = (account, scope.claim) {
            for l in h.lots.iter_mut().filter(|l| l.account_key().is_none()) {
                l.account = Some(a.to_string());
            }
        }
        let (shares, cost) = h
            .lots
            .iter()
            .filter(|l| scope.lot(l))
            .fold((0.0, 0.0), |(q, c), l| (q + l.qty, c + l.cost()));
        let diff = qty - shares;
        if diff > EPS {
            let implied = (qty * avg - cost) / diff;
            lot.qty = diff;
            if implied.is_finite() && implied > 0.0 {
                lot.price = implied;
            }
            h.lots.push(lot);
            h.recompute();
            SyncChange::Added(diff)
        } else if diff < -EPS {
            scoped(&mut h.lots, &scope, |lots| {
                take_fifo(lots, -diff, "9999-12-31")
            });
            h.recompute();
            SyncChange::Reduced(-diff)
        } else {
            SyncChange::Unchanged
        }
    }

    /// Replace a holding's lots (an import that lists every lot with its date).
    pub fn replace_lots(&mut self, symbol: &str, lots: Vec<Lot>) {
        let sym = symbol.to_uppercase();
        if lots.is_empty() {
            self.holdings.retain(|h| h.symbol != sym);
            return;
        }
        match self.holding_mut(&sym) {
            Some(h) => {
                h.lots = lots;
                h.recompute();
            }
            None => self.holdings.push(Holding::new(sym, lots)),
        }
    }

    /// Replace the lots in `scope` with `lots`, keeping other accounts' lots.
    fn replace_scoped_lots(&mut self, symbol: &str, scope: &Scope, lots: Vec<Lot>) {
        let sym = symbol.to_uppercase();
        match self.holding_mut(&sym) {
            Some(h) => {
                h.lots.retain(|l| !scope.lot(l));
                h.lots.extend(lots);
                if h.lots.is_empty() {
                    self.holdings.retain(|h| h.symbol != sym);
                } else {
                    h.recompute();
                }
            }
            None => self.replace_lots(&sym, lots),
        }
    }

    /// Sell `qty` shares at `price` on `date`, oldest lots first (FIFO — how
    /// Indian demat accounts work and the IRS default). Records one [`Sale`]
    /// per lot touched and drops the holding when nothing is left.
    pub fn sell_fifo(
        &mut self,
        symbol: &str,
        qty: f64,
        price: f64,
        date: &str,
        source: &str,
    ) -> Result<Vec<Sale>> {
        self.sell_lots(symbol, qty, price, date, source, None, false)
    }

    /// [`Portfolio::sell_fifo`] from one account's lots only (`None`: any),
    /// taking shares bought on `date` first when `net_same_day` (a buy and
    /// sell on one day are intraday and leave the older lots alone); those
    /// slices are flagged `intraday`.
    fn sell_lots(
        &mut self,
        symbol: &str,
        qty: f64,
        price: f64,
        date: &str,
        source: &str,
        account: Option<&str>,
        net_same_day: bool,
    ) -> Result<Vec<Sale>> {
        if qty <= EPS || !qty.is_finite() {
            bail!("Quantity to sell must be positive");
        }
        if price < 0.0 || !price.is_finite() {
            bail!("Sale price can't be negative");
        }
        let sym = symbol.to_uppercase();
        let scope = self.scope(&sym, account);
        let Some(h) = self.holding_mut(&sym) else {
            bail!("{} is not in this portfolio", sym);
        };
        let (held_now, held_then) =
            h.lots
                .iter()
                .filter(|l| scope.lot(l))
                .fold((0.0, 0.0), |(now, then), l| {
                    (
                        now + l.qty,
                        then + if l.held_from() <= date { l.qty } else { 0.0 },
                    )
                });
        let of_account = account.map_or(String::new(), |a| format!(" in {a}"));
        if qty > held_then + EPS {
            if held_then + EPS < held_now {
                bail!(
                    "Cannot sell {} {}{} on {}: only {} were bought by then",
                    qty,
                    sym,
                    of_account,
                    date,
                    held_then
                );
            }
            bail!(
                "Cannot sell {} {}{}: you hold {}",
                qty,
                sym,
                of_account,
                held_now
            );
        }
        let taken: Vec<(Lot, f64, bool)> = scoped(&mut h.lots, &scope, |lots| {
            if net_same_day {
                take_netting(lots, qty, date).0
            } else {
                take_fifo(lots, qty, date)
                    .0
                    .into_iter()
                    .map(|(l, q)| (l, q, false))
                    .collect()
            }
        });
        h.recompute();
        if h.lots.is_empty() {
            self.holdings.retain(|h| h.symbol != sym);
        }
        let sales: Vec<Sale> = taken
            .iter()
            .map(|(lot, q, intraday)| {
                let mut sale = Sale::from_lot(&sym, lot, *q, date, price, source);
                sale.intraday = *intraday;
                sale
            })
            .collect();
        self.realized.extend(sales.iter().cloned());
        Ok(sales)
    }

    /// Correct the purchase date of lot `n` (1-based, oldest first) of `symbol`.
    pub fn set_lot_date(&mut self, symbol: &str, n: usize, date: &str) -> Result<()> {
        let sym = symbol.to_uppercase();
        let Some(h) = self.holding_mut(&sym) else {
            bail!("{} is not in this portfolio", sym);
        };
        let count = h.lots.len();
        let lot = n
            .checked_sub(1)
            .and_then(|i| h.lots.get_mut(i))
            .with_context(|| format!("{} has no lot #{} (it has {})", sym, n, count))?;
        // An estimated lot's quantity was recorded on its old date, already
        // reflecting any split before then — keep that so `actions` doesn't
        // apply those splits again.
        if lot.date_estimated && lot.units_as_of.is_none() {
            lot.units_as_of = Some(lot.date.clone());
        }
        lot.date = date.to_string();
        lot.date_estimated = false;
        h.recompute();
        Ok(())
    }

    /// Change the quantity and/or price of lot `n` (1-based, oldest first).
    pub fn edit_lot(
        &mut self,
        symbol: &str,
        n: usize,
        qty: Option<f64>,
        price: Option<f64>,
    ) -> Result<Lot> {
        if let Some(q) = qty
            && !(q > EPS && q.is_finite())
        {
            bail!("Quantity must be positive (use --delete to drop the lot)");
        }
        if let Some(p) = price
            && !(p >= 0.0 && p.is_finite())
        {
            bail!("Price can't be negative");
        }
        let sym = symbol.to_uppercase();
        let Some(h) = self.holding_mut(&sym) else {
            bail!("{} is not in this portfolio", sym);
        };
        let count = h.lots.len();
        let lot = n
            .checked_sub(1)
            .and_then(|i| h.lots.get_mut(i))
            .with_context(|| format!("{} has no lot #{} (it has {})", sym, n, count))?;
        if let Some(q) = qty {
            lot.qty = q;
        }
        if let Some(p) = price {
            lot.price = p;
        }
        let edited = lot.clone();
        h.recompute();
        Ok(edited)
    }

    /// Delete lot `n` (1-based, oldest first) without recording a sale.
    /// The holding goes when its last lot does.
    pub fn delete_lot(&mut self, symbol: &str, n: usize) -> Result<Lot> {
        let sym = symbol.to_uppercase();
        let Some(h) = self.holding_mut(&sym) else {
            bail!("{} is not in this portfolio", sym);
        };
        let count = h.lots.len();
        if n == 0 || n > count {
            bail!("{} has no lot #{} (it has {})", sym, n, count);
        }
        let lot = h.lots.remove(n - 1);
        h.recompute();
        if h.lots.is_empty() {
            self.holdings.retain(|h| h.symbol != sym);
        }
        Ok(lot)
    }

    /// The sales of `symbol`, in the order recorded (numbered from 1 in
    /// `portfolio lots`).
    pub fn sales_of(&self, symbol: &str) -> Vec<&Sale> {
        let sym = symbol.to_uppercase();
        self.realized.iter().filter(|s| s.symbol == sym).collect()
    }

    /// Undo sale `n` of `symbol` (1-based, see [`Portfolio::sales_of`]): drop
    /// it from the realised gains and put its shares back as the lot they
    /// came from, merged into that lot when part of it is still held.
    pub fn undo_sale(&mut self, symbol: &str, n: usize) -> Result<Sale> {
        let sym = symbol.to_uppercase();
        let positions: Vec<usize> = self
            .realized
            .iter()
            .enumerate()
            .filter(|(_, s)| s.symbol == sym)
            .map(|(i, _)| i)
            .collect();
        let idx = n
            .checked_sub(1)
            .and_then(|i| positions.get(i).copied())
            .with_context(|| format!("{} has no sale #{} (it has {})", sym, n, positions.len()))?;
        let sale = self.realized.remove(idx);
        let mut lot = sale.to_lot();
        // Splits the rest of the position took after this sale: on lots still
        // held, and on the lots of later sales (which keep their split record).
        let known: Vec<String> = self
            .holding(&sym)
            .into_iter()
            .flat_map(|h| h.lots.iter().flat_map(|l| l.adjustments.iter().cloned()))
            .chain(
                self.realized
                    .iter()
                    .filter(|s| s.symbol == sym)
                    .filter_map(|s| s.origin.as_ref())
                    .flat_map(|o| o.adjustments.iter().cloned()),
            )
            .collect();
        catch_up_splits(&mut lot, known);
        match self.holding_mut(&sym) {
            Some(h) => {
                let same = h.lots.iter_mut().find(|l| {
                    l.date == lot.date
                        && l.date_estimated == lot.date_estimated
                        && (l.price - lot.price).abs() < 1e-9
                        && l.source == lot.source
                        && l.account == lot.account
                        && l.adjustments == lot.adjustments
                        && l.units_as_of == lot.units_as_of
                        && l.order_id == lot.order_id
                });
                match same {
                    Some(l) => l.qty += lot.qty,
                    None => h.lots.push(lot),
                }
                h.recompute();
            }
            None => self.holdings.push(Holding::new(sym, vec![lot])),
        }
        Ok(sale)
    }

    pub fn remove(&mut self, symbol: &str) -> bool {
        let before = self.holdings.len();
        self.holdings.retain(|h| h.symbol != symbol.to_uppercase());
        self.holdings.len() < before
    }

    /// Apply import rows symbol by symbol. A symbol whose rows have no dates
    /// (a holdings snapshot) keeps its lots and only syncs the quantity.
    ///
    /// A symbol with a tradebook (a buy/sell column, or any sell) is rebuilt
    /// from every trade imported for it so far: the file's trades are merged
    /// with those kept from earlier files of the same source (a trade in both
    /// is counted once), so yearly tradebooks can be imported in any order,
    /// and re-importing a file changes nothing. Buys become lots, sells come
    /// out FIFO and are recorded as realized; with `tradebook`, splits are
    /// applied as they happen. A symbol is left alone when a sell has no
    /// earlier buy yet (the trades are kept for the next file), or (without
    /// `replace`) when the rebuild would drop lots or sales from elsewhere.
    /// A file of dated buys only (a lot list) is the whole state: it
    /// replaces the symbol's lots.
    pub fn apply_import(
        &mut self,
        rows: &[ImportRow],
        opts: ImportOptions,
    ) -> Vec<(String, ImportOutcome)> {
        let source = opts.source;
        let account = opts.account.or_else(|| account_of_source(source));
        let mut order: Vec<String> = Vec::new();
        for r in rows {
            let sym = r.symbol.to_uppercase();
            if !order.contains(&sym) {
                order.push(sym);
            }
        }
        let mut out = Vec::new();
        for sym in order {
            let group: Vec<&ImportRow> = rows
                .iter()
                .filter(|r| r.symbol.to_uppercase() == sym)
                .collect();
            let lot_mode = group.iter().any(|r| r.date.is_some() || r.sell);
            if !lot_mode {
                let qty: f64 = group.iter().map(|r| r.qty).sum();
                let cost: f64 = group.iter().map(|r| r.qty * r.price).sum();
                if qty > EPS {
                    let change = self.sync_account(&sym, qty, cost / qty, source, account);
                    out.push((sym, ImportOutcome::Synced(change)));
                }
                continue;
            }
            let ledger = opts.tradebook || group.iter().any(|r| r.sell);
            let incoming: Vec<ImportedTrade> = group
                .iter()
                .map(|r| ImportedTrade {
                    symbol: sym.clone(),
                    source: source.to_string(),
                    sell: r.sell,
                    qty: r.qty,
                    price: r.price,
                    date: r.date.clone(),
                    account: account.map(str::to_string),
                    units_as_of: r.units_as_of.clone(),
                })
                .collect();
            let stored: Vec<ImportedTrade> = if ledger && !opts.replace {
                self.imported
                    .iter()
                    .filter(|t| {
                        t.symbol == sym && t.source == source && t.account.as_deref() == account
                    })
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
            let mut trades = if ledger {
                merge_trades(&stored, &incoming)
            } else {
                incoming
            };
            let today = today();
            let day = |t: &ImportedTrade| t.date.clone().unwrap_or_else(|| today.clone());
            // Oldest first; buys before sells on the same day. The rest of
            // the key makes the order the same whichever file came first.
            trades.sort_by(|a, b| {
                day(a)
                    .cmp(&day(b))
                    .then(a.sell.cmp(&b.sell))
                    .then(a.price.total_cmp(&b.price))
                    .then(a.qty.total_cmp(&b.qty))
            });
            let first = day(&trades[0]);
            // Only this account's lots and sales are rebuilt: another
            // account's are left alone.
            let scope = self.scope(&sym, account);
            // Bonus lots of this account's shares are rebuilt by the replay.
            let own_source = |l: &Lot| l.source == source || l.source == BONUS_SOURCE;
            let foreign_lots = self
                .holding(&sym)
                .is_some_and(|h| h.lots.iter().any(|l| scope.lot(l) && !own_source(l)));
            let foreign_sales = self.realized.iter().any(|s| {
                s.symbol == sym && scope.sale(s) && s.source != source && s.sell_date >= first
            });
            if (foreign_lots || foreign_sales) && !opts.replace {
                out.push((sym, ImportOutcome::WouldReplace));
                continue;
            }
            // A file of lots alone is the whole state, so it deletes the
            // trades kept from this source's tradebooks and the sales they
            // recorded. Never do that unasked.
            if !ledger && !opts.replace {
                let kept_trades = self.imported.iter().any(|t| {
                    t.symbol == sym && t.source == source && t.account.as_deref() == account
                });
                let own_sales = self
                    .realized
                    .iter()
                    .any(|s| s.symbol == sym && scope.sale(s) && s.source == source);
                if kept_trades || own_sales {
                    out.push((sym, ImportOutcome::WouldReplace));
                    continue;
                }
            }

            let splits: &[Split] = if opts.tradebook {
                opts.splits
                    .iter()
                    .find(|(s, _)| s.to_uppercase() == sym)
                    .map_or(&[], |(_, v)| v.as_slice())
            } else {
                &[]
            };
            // Demergers are applied with the splits: a tradebook has no row
            // for the shares they gave. A file of lots lists those itself.
            let demergers = if opts.tradebook {
                demergers_for(&sym)
            } else {
                Vec::new()
            };
            let replay =
                replay_trades(&sym, &trades, source, account, (splits, &demergers), &today);

            // Lots and sales this source recorded before StockWise kept its
            // trades: rebuilding from one file must reproduce them, or an
            // earlier file would be lost.
            let own_lots: Vec<&Lot> = self
                .holding(&sym)
                .map(|h| {
                    h.lots
                        .iter()
                        .filter(|l| scope.lot(l) && own_source(l))
                        .collect()
                })
                .unwrap_or_default();
            let own_sales: Vec<&Sale> = self
                .realized
                .iter()
                .filter(|s| s.symbol == sym && scope.sale(s) && s.source == source)
                .collect();
            let known = self
                .imported
                .iter()
                .any(|t| t.symbol == sym && t.source == source && t.account.as_deref() == account);
            if ledger
                && !opts.replace
                && !known
                && (!own_lots.is_empty() || !own_sales.is_empty())
                && !same_state(&own_lots, &replay.lots, &own_sales, &replay.sales)
            {
                out.push((sym, ImportOutcome::WouldReplace));
                continue;
            }

            if ledger {
                // Keep the trades even when a sell has no buy yet: the
                // earlier tradebook completes them.
                // (`replace` also drops the kept trades of the lots it replaces.)
                let same =
                    |t: &ImportedTrade| t.source == source && t.account.as_deref() == account;
                self.imported.retain(|t| {
                    t.symbol != sym
                        || if opts.replace {
                            !scope.has(t.account.as_deref())
                        } else {
                            !same(t)
                        }
                });
                self.imported.extend(trades);
            } else {
                self.imported.retain(|t| {
                    t.symbol != sym || !(t.source == source && t.account.as_deref() == account)
                });
            }
            // A sell with no earlier buy means the trades so far don't cover
            // this symbol's whole history: leave the holding as it is.
            if replay.unmatched > EPS {
                out.push((
                    sym,
                    ImportOutcome::Unmatched {
                        unmatched: replay.unmatched,
                    },
                ));
                continue;
            }
            self.realized.retain(|s| {
                s.symbol != sym || !scope.sale(s) || (s.source != source && s.sell_date < first)
            });
            let (n, n_sales) = (replay.lots.len(), replay.sales.len());
            self.realized.extend(replay.sales);
            self.replace_scoped_lots(&sym, &scope, replay.lots);
            self.replace_demerger_children(&scope, replay.children);
            out.push((
                sym,
                ImportOutcome::Lots {
                    lots: n,
                    sales: n_sales,
                    splits: replay.splits,
                },
            ));
        }
        out
    }

    /// Apply fills from a broker's trade book. Fills already synced (by
    /// broker, order id and fill id) are skipped. The new fills of one order on one
    /// day are merged at their average price; orders are applied in time
    /// order: a buy becomes a lot (dated, `broker:<id>`, with its order id),
    /// a sell comes out of the shares bought that same day first (an
    /// intraday sale), then the oldest lots held on that date, and is
    /// recorded as realized. A sell larger than the shares held then is
    /// left out and not marked as synced, so a later run can apply it once
    /// the missing lots are added. Running this twice changes nothing.
    pub fn apply_broker_fills(&mut self, fills: &[BrokerFill]) -> FillSummary {
        let mut summary = FillSummary::default();
        // A fill is known by broker, order and fill id: exchange trade ids
        // alone can repeat across days.
        let mut seen: Vec<(&str, &str, &str)> = Vec::new();
        // The fills of each order and day, in first-seen order.
        let mut groups: Vec<Vec<&BrokerFill>> = Vec::new();
        for f in fills {
            let key = (f.broker.as_str(), f.order_id.as_str(), f.fill_id.as_str());
            let synced = self.broker_fills.iter().any(|s| {
                s.broker == f.broker && s.order_id == f.order_id && s.fill_id == f.fill_id
            });
            if synced || seen.contains(&key) {
                summary.already_synced += usize::from(synced);
                continue;
            }
            seen.push(key);
            let same = |g: &Vec<&BrokerFill>| {
                let h = g[0];
                h.broker == f.broker
                    && h.order_id == f.order_id
                    && h.date == f.date
                    && h.sell == f.sell
                    && h.symbol.eq_ignore_ascii_case(&f.symbol)
            };
            match groups.iter_mut().find(|g| same(g)) {
                Some(g) => g.push(f),
                None => groups.push(vec![f]),
            }
        }
        let first_time = |g: &[&BrokerFill]| {
            g.iter()
                .filter_map(|f| f.time.clone())
                .min()
                .unwrap_or_default()
        };
        // Buys before sells on a day, so a sell nets against that day's buys.
        groups.sort_by(|a, b| {
            (a[0].date.as_str(), a[0].sell, first_time(a)).cmp(&(
                b[0].date.as_str(),
                b[0].sell,
                first_time(b),
            ))
        });

        for g in groups {
            let head = g[0];
            let qty: f64 = g.iter().map(|f| f.qty).sum();
            let price = g.iter().map(|f| f.qty * f.price).sum::<f64>() / qty;
            // Round the average to the paisa / cent it can't be finer than.
            let price = (price * 10_000.0).round() / 10_000.0;
            let source = format!("broker:{}", head.broker);
            // Each broker is an account: its fills never touch another's lots.
            let account = account_of_source(&source);
            let effect = if head.sell {
                let scope = self.scope(&head.symbol, account);
                let held: f64 = self.holding(&head.symbol).map_or(0.0, |h| {
                    h.lots
                        .iter()
                        .filter(|l| scope.lot(l) && l.held_from() <= head.date.as_str())
                        .map(|l| l.qty)
                        .sum()
                });
                let sold = (qty <= held + EPS)
                    .then(|| {
                        self.sell_lots(&head.symbol, qty, price, &head.date, &source, account, true)
                    })
                    .and_then(Result::ok);
                match sold {
                    Some(mut sales) => {
                        let start = self.realized.len() - sales.len();
                        for s in self.realized[start..].iter_mut().chain(sales.iter_mut()) {
                            s.order_id = Some(head.order_id.clone());
                        }
                        FillEffect::Sold { sales }
                    }
                    None => FillEffect::Unmatched { held },
                }
            } else {
                let mut lot = Lot::new(qty, price, head.date.clone(), &source);
                lot.order_id = Some(head.order_id.clone());
                lot.account = account.map(str::to_string);
                self.add_lot(&head.symbol, lot);
                FillEffect::Lot
            };
            if !matches!(effect, FillEffect::Unmatched { .. }) {
                self.broker_fills.extend(g.iter().map(|f| SyncedFill {
                    broker: f.broker.clone(),
                    order_id: f.order_id.clone(),
                    fill_id: f.fill_id.clone(),
                    date: f.date.clone(),
                }));
            }
            summary.orders.push(OrderFill {
                symbol: head.symbol.to_uppercase(),
                order_id: head.order_id.clone(),
                side: if head.sell { "SELL" } else { "BUY" },
                date: head.date.clone(),
                time: g.iter().filter_map(|f| f.time.clone()).min(),
                qty,
                price,
                fills: g.len(),
                effect,
            });
        }
        summary
    }

    /// Apply a split or bonus to the lots bought before it. A split
    /// multiplies quantity and divides price, so cost and purchase date are
    /// unchanged. An Indian bonus ([`Split::bonus`]) keeps those lots and
    /// adds the bonus shares as a zero-cost lot dated on the allotment date.
    /// Lots already adjusted for it are skipped. Returns how many of the
    /// older lots were adjusted.
    pub fn apply_split(&mut self, symbol: &str, split: &Split) -> usize {
        let Some(h) = self.holding_mut(symbol) else {
            return 0;
        };
        let changed = apply_split_to(&mut h.lots, split);
        if changed > 0 {
            h.recompute();
        }
        changed
    }

    /// Record demerger `d` (`symbol` is any listing of its parent). Every
    /// parent lot held on the record date, under any listing (`.NS`, `.BO`,
    /// an old ticker), keeps its quantity and date and keeps
    /// `100 − child_cost_pct` percent of its cost and charges; a lot of the
    /// child, dated on the same purchase date and held from the record date,
    /// gets the rest. A parent lot sold after the record date, before this
    /// was applied, had received the child shares too: its sale's cost is
    /// split the same way and its child shares are added.
    ///
    /// The child shares are whole per account, rounded down once across all
    /// listings and shared among the account's lots in proportion.
    ///
    /// Child shares already in an account that may be this demerger's (no
    /// tradebook, broker order or `portfolio add` behind them, such as a
    /// broker snapshot) are taken as its shares when they cover the
    /// entitlement, instead of being added again. When they cover only part
    /// of it, that account is left unchanged and reported in `conflicts`:
    /// they could be the demerger's shares or a separate purchase.
    ///
    /// Everything touched is marked, so applying twice changes nothing.
    pub fn apply_demerger(&mut self, symbol: &str, d: &Demerger) -> DemergerOutcome {
        let mut out = DemergerOutcome::default();
        let r = d.ratio();
        if !d.is_parent(symbol)
            || !r.is_finite()
            || r <= 0.0
            || !(0.0..100.0).contains(&d.child_cost_pct)
        {
            return out;
        }
        let id = d.id();
        let rec = d.record_date;
        // What was held on the record date: open lots and later sales.
        let mut lots: Vec<(usize, usize, Option<String>, f64)> = Vec::new();
        for (hi, h) in self.holdings.iter().enumerate() {
            if !d.is_parent(&h.symbol) {
                continue;
            }
            for (li, l) in h.lots.iter().enumerate() {
                if d.pending_for(l) {
                    let held = qty_on(l.qty, &l.adjustments, rec);
                    lots.push((hi, li, l.account_key().map(str::to_string), held));
                }
            }
        }
        let mut sales: Vec<(usize, Option<String>, f64)> = Vec::new();
        for (si, s) in self.realized.iter().enumerate() {
            if d.is_parent(&s.symbol) && d.pending_for_sale(s) {
                let adj = s.origin.as_ref().map_or(&[][..], |o| &o.adjustments[..]);
                sales.push((
                    si,
                    s.account_key().map(str::to_string),
                    qty_on(s.qty, adj, rec),
                ));
            }
        }
        if lots.is_empty() && sales.is_empty() {
            return out;
        }
        let items: Vec<(Option<String>, f64)> = lots
            .iter()
            .map(|(_, _, a, q)| (a.clone(), *q))
            .chain(sales.iter().map(|(_, a, q)| (a.clone(), *q)))
            .collect();
        let accounts = entitlements(&items, r);

        // Child shares the portfolio may already hold for this demerger.
        let mut skip: Vec<Option<String>> = Vec::new();
        for (account, exact) in &accounts {
            let whole = (exact + 1e-9).floor();
            if whole < 1.0 {
                continue;
            }
            let pool = self.unexplained_child_lots(d, account.as_deref());
            let held: f64 = pool
                .iter()
                .map(|&(hi, li)| self.holdings[hi].lots[li].qty)
                .sum();
            if held <= EPS {
                continue;
            }
            if held + 1e-9 >= whole {
                let mut left = whole;
                for (hi, li) in pool {
                    let lot = &mut self.holdings[hi].lots[li];
                    let take = left.min(lot.qty);
                    lot.shrink(take);
                    left -= take;
                    if left <= EPS {
                        break;
                    }
                }
                out.reconciled += whole;
            } else {
                skip.push(account.clone());
                let who = account
                    .as_deref()
                    .map_or("Your portfolio".to_string(), |a| format!("Account {a}"));
                out.conflicts.push(format!(
                    "{who} already holds {held} {child} without a tradebook purchase behind \
                     them, fewer than the {whole} the demerger gave. They could be those shares \
                     or a separate purchase, so this account was left unchanged: correct those \
                     {child} lots (or remove them) and run `portfolio actions --apply` again.",
                    child = d.child
                ));
            }
        }
        let exact_of = |account: &Option<String>| {
            accounts
                .iter()
                .find(|(a, _)| a == account)
                .map_or(0.0, |(_, q)| *q)
        };

        let mut touched: Vec<usize> = Vec::new();
        for (hi, li, account, held) in lots {
            if skip.contains(&account) {
                continue;
            }
            let listing = self.holdings[hi].symbol.clone();
            let lot = &mut self.holdings[hi].lots[li];
            lot.adjustments.push(id.clone());
            out.lots += 1;
            touched.push(hi);
            let child_qty = allotted(held * r, exact_of(&account));
            out.fraction_dropped += (held * r - child_qty).max(0.0);
            if child_qty <= EPS {
                continue;
            }
            let (parent_cost, child_cost) = split_basis(lot.cost(), d.child_cost_pct);
            let (parent_charges, child_charges) = split_charges(lot.charges, d.child_cost_pct);
            let mut child = child_lot(d, child_qty, child_cost, &lot.date, account);
            child.date_estimated = lot.date_estimated;
            child.fx_currency = lot.fx_currency.clone();
            child.fx_rate = lot.fx_rate;
            child.charges = child_charges;
            if lot.qty > EPS {
                lot.price = parent_cost / lot.qty;
            }
            lot.charges = parent_charges;
            out.child_lots.push((d.child_symbol(&listing), child));
        }
        for (si, account, held) in sales {
            if skip.contains(&account) {
                continue;
            }
            let sale = &mut self.realized[si];
            let origin = sale.origin.get_or_insert_with(|| SoldLot {
                source: sale.source.clone(),
                adjustments: Vec::new(),
                units_as_of: (sale.sell_date > sale.buy_date).then(|| sale.sell_date.clone()),
                order_id: None,
                account: sale.account.clone(),
                charges: None,
            });
            origin.adjustments.push(id.clone());
            out.sales += 1;
            let child_qty = allotted(held * r, exact_of(&account));
            out.fraction_dropped += (held * r - child_qty).max(0.0);
            if child_qty <= EPS {
                continue;
            }
            let (parent_cost, child_cost) =
                split_basis(sale.qty * sale.buy_price, d.child_cost_pct);
            let (parent_charges, child_charges) = split_charges(origin.charges, d.child_cost_pct);
            if let Some(moved) = child_charges {
                origin.charges = parent_charges;
                sale.charges = sale.charges.map(|c| c - moved).filter(|c| *c > EPS);
            }
            if sale.qty > EPS {
                sale.buy_price = parent_cost / sale.qty;
            }
            let mut child = child_lot(d, child_qty, child_cost, &sale.buy_date, account);
            child.date_estimated = sale.buy_date_estimated;
            child.fx_currency = sale.fx_currency.clone();
            child.fx_rate = sale.buy_fx_rate;
            child.charges = child_charges;
            out.child_lots.push((d.child_symbol(&sale.symbol), child));
        }
        touched.dedup();
        for hi in touched {
            self.holdings[hi].recompute();
        }
        for (sym, lot) in &out.child_lots {
            self.add_lot(sym, lot.clone());
        }
        out
    }

    /// Lots of `d`'s child in `account` that may be the demerger's own
    /// shares recorded some other way (a broker snapshot, a holdings file):
    /// not from this demerger, not a broker order, not typed in with
    /// `portfolio add`, not a bonus, and not built from a kept tradebook.
    /// Oldest first, as (holding, lot) indices.
    fn unexplained_child_lots(&self, d: &Demerger, account: Option<&str>) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (hi, h) in self.holdings.iter().enumerate() {
            if !d.is_child(&h.symbol) {
                continue;
            }
            for (li, l) in h.lots.iter().enumerate() {
                let tradebook = self
                    .imported
                    .iter()
                    .any(|t| t.symbol == h.symbol && t.source == l.source && !t.sell);
                let ours = l.source == DEMERGER_SOURCE;
                if l.account_key() == account
                    && !ours
                    && !tradebook
                    && l.order_id.is_none()
                    && l.source != "manual"
                    && l.source != BONUS_SOURCE
                    && l.qty > EPS
                {
                    out.push((hi, li));
                }
            }
        }
        out.sort_by(|a, b| {
            self.holdings[a.0].lots[a.1]
                .date
                .cmp(&self.holdings[b.0].lots[b.1].date)
        });
        out
    }

    /// Demergers in [`DEMERGERS`] not yet applied to a lot held on their
    /// record date (open, or sold after it), as (a parent symbol held, the
    /// demerger), once per demerger. No network needed.
    pub fn pending_demergers(&self) -> Vec<(String, &'static Demerger)> {
        let mut out = Vec::new();
        for d in DEMERGERS {
            let open = self
                .holdings
                .iter()
                .find(|h| d.is_parent(&h.symbol) && h.lots.iter().any(|l| d.pending_for(l)))
                .map(|h| h.symbol.clone());
            let sold = || {
                self.realized
                    .iter()
                    .find(|s| d.is_parent(&s.symbol) && d.pending_for_sale(s))
                    .map(|s| s.symbol.clone())
            };
            if let Some(sym) = open.or_else(sold) {
                out.push((sym, d));
            }
        }
        out
    }

    /// The child symbols pending demerger `d` would add lots to: one per
    /// parent listing with a lot held on the record date.
    pub fn demerger_children(&self, d: &Demerger) -> Vec<String> {
        let open = self
            .holdings
            .iter()
            .filter(|h| d.is_parent(&h.symbol) && h.lots.iter().any(|l| d.pending_for(l)))
            .map(|h| d.child_symbol(&h.symbol));
        let sold = self
            .realized
            .iter()
            .filter(|s| d.is_parent(&s.symbol) && d.pending_for_sale(s))
            .map(|s| d.child_symbol(&s.symbol));
        let mut out: Vec<String> = open.chain(sold).collect();
        out.sort();
        out.dedup();
        out
    }

    /// Apply `splits` of `symbol` and the `demergers` of its parent in
    /// effective-date order (a split on a record date first), so each
    /// demerger's entitlement is the share count on its record date.
    /// Returns what each demerger did.
    pub fn apply_in_order(
        &mut self,
        symbol: &str,
        splits: &[Split],
        demergers: &[&Demerger],
    ) -> Vec<DemergerOutcome> {
        let mut events: Vec<(&str, Option<&Split>, Option<&Demerger>)> = splits
            .iter()
            .map(|s| (s.date.as_str(), Some(s), None))
            .chain(demergers.iter().map(|d| (d.record_date, None, Some(*d))))
            .collect();
        // Stable: a split dated on a record date stays ahead of the demerger.
        events.sort_by(|a, b| a.0.cmp(b.0).then(a.2.is_some().cmp(&b.2.is_some())));
        let mut out = Vec::new();
        for (_, split, demerger) in events {
            if let Some(s) = split {
                self.apply_split(symbol, s);
            }
            if let Some(d) = demerger {
                out.push(self.apply_demerger(symbol, d));
            }
        }
        out
    }

    /// After a tradebook replay rebuilt `parent`'s lots in `scope`, put the
    /// child lots the replay's demergers produced in place of the ones an
    /// earlier run recorded for the same demerger and account, less the
    /// child shares sold since. An unchanged re-import therefore leaves the
    /// child holding as it was.
    fn replace_demerger_children(&mut self, scope: &Scope, children: Vec<(String, Lot)>) {
        let mut by_symbol: Vec<(String, String, Vec<Lot>)> = Vec::new();
        for (sym, lot) in children {
            let Some(id) = lot
                .adjustments
                .iter()
                .find(|a| a.starts_with(DEMERGER_PREFIX))
                .cloned()
            else {
                continue;
            };
            match by_symbol.iter_mut().find(|(s, i, _)| *s == sym && *i == id) {
                Some((_, _, v)) => v.push(lot),
                None => by_symbol.push((sym, id, vec![lot])),
            }
        }
        for (sym, id, mut lots) in by_symbol {
            let ours = |l: &Lot| l.source == DEMERGER_SOURCE && l.adjustments.contains(&id);
            if let Some(h) = self.holding_mut(&sym) {
                h.lots.retain(|l| !(ours(l) && scope.lot(l)));
                h.recompute();
            }
            let sold: f64 = self
                .realized
                .iter()
                .filter(|s| s.symbol == sym && scope.sale(s))
                .filter(|s| {
                    s.origin
                        .as_ref()
                        .is_some_and(|o| o.source == DEMERGER_SOURCE && o.adjustments.contains(&id))
                })
                .map(|s| s.qty)
                .sum();
            lots.sort_by(|a, b| a.date.cmp(&b.date));
            take_fifo(&mut lots, sold, "9999-12-31");
            for lot in lots {
                self.add_lot(&sym, lot);
            }
        }
    }

    /// Shares of `symbol` held at the start of `date` (bought before it, not
    /// yet sold), in today's share count: `splits` not yet applied to a lot,
    /// and splits after a sale, are multiplied in. A lot that received a
    /// bonus after `date` as a separate lot counts its bonus shares here too,
    /// since that bonus lot was not yet held on `date`.
    pub fn shares_on(&self, symbol: &str, date: &str, splits: &[Split]) -> f64 {
        let sym = symbol.to_uppercase();
        let later = |after: &str, lot: Option<&Lot>| -> f64 {
            splits
                .iter()
                .filter(|s| {
                    s.date.as_str() > after
                        && lot.is_none_or(|l| {
                            s.pending_for(l)
                                || (s.date.as_str() > date && l.adjustments.contains(&s.bonus_id()))
                        })
                })
                .map(Split::ratio)
                .product()
        };
        let open: f64 = self.holding(&sym).map_or(0.0, |h| {
            h.lots
                .iter()
                .filter(|l| l.held_from() < date)
                .map(|l| l.qty * later(&l.date, Some(l)))
                .sum()
        });
        let sold: f64 = self
            .realized
            .iter()
            .filter(|s| s.symbol == sym && s.held_from() < date && s.sell_date.as_str() >= date)
            .map(|s| {
                // Bonus shares received on the sold lot after `date`.
                let bonus: f64 = splits
                    .iter()
                    // Not a bonus after the sale: `later` already counts it.
                    .filter(|b| {
                        b.date.as_str() > date
                            && b.date <= s.sell_date
                            && s.bonuses.contains(&b.bonus_id())
                    })
                    .map(Split::ratio)
                    .product();
                s.qty * later(&s.sell_date, None) * bonus
            })
            .sum();
        open + sold
    }

    /// Shares a dividend is due on, or `None` if it is already recorded or
    /// nothing was held at its ex-date.
    pub fn dividend_due(&self, symbol: &str, ev: &DividendEvent, splits: &[Split]) -> Option<f64> {
        let sym = symbol.to_uppercase();
        if self
            .dividends
            .iter()
            .any(|d| d.symbol == sym && d.ex_date == ev.ex_date)
        {
            return None;
        }
        let qty = self.shares_on(&sym, &ev.ex_date, splits);
        (qty > EPS).then_some(qty)
    }

    /// Record dividend `ev` on the shares held at its ex-date. Idempotent.
    pub fn record_dividend(
        &mut self,
        symbol: &str,
        ev: &DividendEvent,
        splits: &[Split],
        currency: Option<&str>,
    ) -> Option<DividendReceived> {
        let qty = self.dividend_due(symbol, ev, splits)?;
        let d = DividendReceived {
            symbol: symbol.to_uppercase(),
            ex_date: ev.ex_date.clone(),
            per_share: ev.per_share,
            qty,
            amount: qty * ev.per_share,
            currency: currency.map(str::to_string),
        };
        self.dividends.push(d.clone());
        Some(d)
    }

    /// Every symbol with cash flows: current holdings, then fully sold ones.
    pub fn symbols_with_history(&self) -> Vec<String> {
        let mut out: Vec<String> = self.holdings.iter().map(|h| h.symbol.clone()).collect();
        for s in self.realized.iter().map(|s| &s.symbol) {
            if !out.contains(s) {
                out.push(s.clone());
            }
        }
        out
    }

    /// Dated cash flows of `symbol` for XIRR: every purchase negative (open
    /// lots and sold ones), sale proceeds and dividends positive. The caller
    /// adds today's value of what is still held.
    pub fn cash_flows(&self, symbol: &str) -> Vec<(NaiveDate, f64)> {
        self.cash_flow_items(symbol)
            .into_iter()
            .map(|f| (f.date, f.amount))
            .collect()
    }

    /// [`Portfolio::cash_flows`] with what each flow needs to be converted
    /// at its own date's exchange rate. Purchases include their charges and
    /// sale proceeds are net of the sale's.
    pub fn cash_flow_items(&self, symbol: &str) -> Vec<CashFlow> {
        let sym = symbol.to_uppercase();
        let mut flows = Vec::new();
        if let Some(h) = self.holding(&sym) {
            for l in &h.lots {
                if let Some(d) = date_of(&l.date) {
                    flows.push(CashFlow {
                        date: d,
                        amount: -(l.cost() + l.charges.unwrap_or(0.0)),
                        paid: l.fx_currency.clone().zip(l.fx_rate),
                        currency: None,
                        kind: FlowKind::Buy,
                    });
                }
            }
        }
        for s in self.realized.iter().filter(|s| s.symbol == sym) {
            if let (Some(b), Some(d)) = (date_of(&s.buy_date), date_of(&s.sell_date)) {
                flows.push(CashFlow {
                    date: b,
                    amount: -s.qty * s.buy_price,
                    paid: s.fx_currency.clone().zip(s.buy_fx_rate),
                    currency: None,
                    kind: FlowKind::Buy,
                });
                flows.push(CashFlow {
                    date: d,
                    amount: s.qty * s.sell_price - s.charges(),
                    paid: None,
                    currency: None,
                    kind: FlowKind::Sale,
                });
            }
        }
        for d in self.dividends.iter().filter(|d| d.symbol == sym) {
            if let Some(x) = date_of(&d.ex_date) {
                flows.push(CashFlow {
                    date: x,
                    amount: d.amount,
                    paid: None,
                    currency: d.currency.clone(),
                    kind: FlowKind::Dividend,
                });
            }
        }
        flows.sort_by_key(|f| f.date);
        flows
    }
}

/// What a [`CashFlow`] was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowKind {
    Buy,
    Sale,
    Dividend,
}

/// One dated cash flow of a holding, in its own currency unless `currency`
/// says otherwise (a dividend paid in another currency).
#[derive(Debug, Clone, PartialEq)]
pub struct CashFlow {
    pub date: NaiveDate,
    pub amount: f64,
    /// For a purchase paid in another currency: that currency and the rate
    /// paid (see [`Lot::fx_rate`]).
    pub paid: Option<(String, f64)>,
    pub currency: Option<String>,
    pub kind: FlowKind,
}

/// Annualized money-weighted return (XIRR) of `flows` plus `value_now`
/// received on `today`. `None` with under 30 days of history, where an
/// annualized figure means little, or when there is no solution.
pub fn xirr(flows: &[(NaiveDate, f64)], value_now: f64, today: NaiveDate) -> Option<f64> {
    let first = flows.iter().map(|f| f.0).min()?;
    let span = (today - first).num_days();
    if span < 30 {
        return None;
    }
    let years = |d: NaiveDate| (d - first).num_days() as f64 / 365.0;
    let mut cf: Vec<(f64, f64)> = flows.iter().map(|(d, a)| (years(*d), *a)).collect();
    cf.push((years(today), value_now));
    crate::technical::xirr(&cf)
}

/// The currency a symbol trades in, from its exchange suffix — used when no
/// live quote says otherwise.
pub fn symbol_currency(symbol: &str) -> Option<&'static str> {
    let s = symbol.to_uppercase();
    // `MF:<AMFI code>` — Indian mutual funds from a CAS import.
    if s.ends_with(".NS") || s.ends_with(".BO") || s.ends_with("-INR") || s.starts_with("MF:") {
        Some("INR")
    } else if s.ends_with(".L") {
        Some("GBp")
    } else if let Some(x) = crate::market::exchange_of(&s) {
        // Other global exchanges: their market-table currency.
        let info = x.info();
        Some(info.quote_unit.unwrap_or(info.currency))
    } else if s.ends_with("-USD") {
        Some("USD")
    } else if s.contains('.') || s.contains('=') || s.starts_with('^') {
        None
    } else {
        Some("USD")
    }
}

/// Exchange rates into one base currency, to total holdings that trade in
/// different currencies.
#[derive(Debug, Clone)]
pub struct Fx {
    pub base: String,
    /// (currency, units of base per unit).
    rates: Vec<(String, f64)>,
}

impl Fx {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.to_string(),
            rates: Vec::new(),
        }
    }

    pub fn set_rate(&mut self, currency: &str, rate: f64) {
        if rate.is_finite() && rate > 0.0 {
            self.rates.retain(|(c, _)| c != currency);
            self.rates.push((currency.to_string(), rate));
        }
    }

    pub fn rate(&self, currency: &str) -> Option<f64> {
        if currency == self.base {
            return Some(1.0);
        }
        if currency == "GBp" {
            // London prices are quoted in pence.
            return self.rate("GBP").map(|r| r / 100.0);
        }
        self.rates
            .iter()
            .find(|(c, _)| c == currency)
            .map(|(_, r)| *r)
    }

    /// `amount` in `currency`, in the base currency. `None` when no rate is known.
    pub fn convert(&self, amount: f64, currency: &str) -> Option<f64> {
        self.rate(currency).map(|r| amount * r)
    }

    /// Rates that were used, for a "converted at" note.
    pub fn used(&self) -> &[(String, f64)] {
        &self.rates
    }
}

// ── Named portfolios ──

static ACTIVE: OnceLock<Option<String>> = OnceLock::new();

pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 40
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !ok {
        bail!(
            "Invalid portfolio name '{}': use letters, digits, '-' or '_' (up to 40 characters)",
            name
        );
    }
    Ok(())
}

/// Normalize a portfolio name: `None` for the default portfolio.
pub fn normalize_name(name: Option<&str>) -> Result<Option<String>> {
    match name.map(str::trim) {
        None | Some("") => Ok(None),
        Some(n) if n.eq_ignore_ascii_case("default") => Ok(None),
        Some(n) => {
            validate_name(n)?;
            Ok(Some(n.to_lowercase()))
        }
    }
}

/// Choose the portfolio this run reads and writes (`--portfolio` /
/// `STOCKWISE_PORTFOLIO`). Call once, before any load.
pub fn select(name: Option<&str>) -> Result<()> {
    let name = normalize_name(name)?;
    let _ = ACTIVE.set(name);
    Ok(())
}

/// The selected named portfolio, or `None` for the default one.
pub fn active() -> Option<&'static str> {
    ACTIVE.get().and_then(|n| n.as_deref())
}

/// Data file of a portfolio, relative to the data directory.
pub fn file_name(name: Option<&str>) -> String {
    match name {
        None => "portfolio.json".into(),
        Some(n) => format!("portfolios/{}.json", n),
    }
}

/// A data file that belongs to the active portfolio (e.g. its wealth history):
/// `file` for the default portfolio, `portfolios/NAME.file` for a named one.
pub fn scoped_file(file: &str) -> String {
    match active() {
        None => file.to_string(),
        Some(n) => format!("portfolios/{}.{}", n, file),
    }
}

/// Named portfolios saved on disk, sorted.
pub fn list_names() -> Result<Vec<String>> {
    let dir = crate::store::path("portfolios")?;
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            if let Some(stem) = file.strip_suffix(".json")
                && validate_name(stem).is_ok()
            {
                names.push(stem.to_string());
            }
        }
    }
    names.sort();
    Ok(names)
}

/// `<file>.<n>`: the `n`th newest earlier version of a portfolio file.
pub fn numbered_backup(path: &Path, n: usize) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

/// The backups of `path` that exist, newest first, with their numbers.
pub fn backups_of(path: &Path) -> Vec<(usize, PathBuf)> {
    (1..=BACKUPS_KEPT)
        .map(|n| (n, numbered_backup(path, n)))
        .filter(|(_, p)| p.exists())
        .collect()
}

/// Shift `<file>.1..4` up by one (dropping `.5`) and put a copy of `path`
/// at `.1`. The copy is staged first, so a failed copy leaves the numbered
/// history untouched and a retry does not push another version out.
#[cfg(test)]
fn rotate_backups(path: &Path) -> Result<()> {
    rotate_backups_with(path, &|a: &Path, b: &Path| std::fs::rename(a, b))
}

fn rotate_backups_with(
    path: &Path,
    rename: &impl Fn(&Path, &Path) -> std::io::Result<()>,
) -> Result<()> {
    let mut staged = path.as_os_str().to_owned();
    staged.push(".1.tmp");
    let staged = PathBuf::from(staged);
    std::fs::copy(path, &staged)
        .with_context(|| format!("Cannot back up {} to {}", path.display(), staged.display()))?;
    // Shift only up to the first free number: with `.1..=.5` all present the
    // oldest is dropped, but after a rotation that stopped half-way the gap
    // it left absorbs the shift, so a retry drops nothing more.
    let gap = (1..=BACKUPS_KEPT)
        .find(|&n| !numbered_backup(path, n).exists())
        .unwrap_or(BACKUPS_KEPT);
    for n in (1..gap).rev() {
        let from = numbered_backup(path, n);
        rename(&from, &numbered_backup(path, n + 1))
            .with_context(|| format!("Cannot rotate backup {}", from.display()))?;
    }
    let first = numbered_backup(path, 1);
    rename(&staged, &first)
        .with_context(|| format!("Cannot move the backup to {}", first.display()))?;
    Ok(())
}

/// A split recorded on another lot of the same holding (an `adjustments`
/// id, `DATE:NUM:DEN`) that falls after `lot`'s share count and that `lot`
/// has not taken yet is applied to it, so a lot put back by an undone sale
/// is in the same share count as the lots still held.
fn catch_up_splits(lot: &mut Lot, mut ids: Vec<String>) {
    ids.sort();
    ids.dedup();
    for id in ids {
        let mut parts = id.splitn(3, ':');
        let (Some(date), Some(num), Some(den)) = (parts.next(), parts.next(), parts.next()) else {
            continue;
        };
        let (Ok(num), Ok(den)) = (num.parse::<f64>(), den.parse::<f64>()) else {
            continue;
        };
        let split = Split {
            date: date.to_string(),
            numerator: num,
            denominator: den,
            bonus: None,
        };
        let r = split.ratio();
        if r.is_finite() && r > 0.0 && split.pending_for(lot) {
            lot.qty *= r;
            lot.price /= r;
            lot.adjustments.push(id);
        }
    }
}

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_0_8_portfolio_file_loads_without_the_new_lot_and_sale_fields() {
        // Written by 0.8.1: no fx_currency, fx_rate or charges anywhere.
        let json = r#"{
            "version": 2,
            "holdings": [{"symbol": "AAPL", "shares": 10.0, "avg_cost": 100.0,
                "added_at": "2024-01-02",
                "lots": [{"qty": 10.0, "price": 100.0, "date": "2024-01-02", "source": "manual"}]}],
            "realized": [{"symbol": "MSFT", "qty": 1.0, "buy_date": "2024-01-02",
                "buy_price": 100.0, "sell_date": "2025-01-02", "sell_price": 120.0,
                "source": "manual"}]
        }"#;
        let path = temp_dir("old-0.8-file").join("portfolio.json");
        std::fs::write(&path, json).unwrap();
        let p = Portfolio::read_only(&path).unwrap();
        let lot = &p.holding("AAPL").unwrap().lots[0];
        assert_eq!(
            (lot.fx_currency.as_deref(), lot.fx_rate, lot.charges),
            (None, None, None)
        );
        assert_eq!(p.realized[0].charges, None);
        assert_eq!(p.realized[0].gain(), 20.0);
        // Saved again, the new fields stay out of the file while unset.
        let out = serde_json::to_string(&p).unwrap();
        assert!(
            !out.contains("fx_rate") && !out.contains("charges"),
            "{out}"
        );
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn lot(qty: f64, price: f64, date: &str) -> Lot {
        Lot::new(qty, price, date, "manual")
    }

    fn opts(source: &str) -> ImportOptions<'_> {
        ImportOptions {
            source,
            replace: false,
            tradebook: false,
            splits: &[],
            account: None,
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stockwise-portfolio-{}-{}",
            name,
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_add_new_holding() {
        let mut p = Portfolio::default();
        p.add("RELIANCE", 10.0, 2500.0);
        assert_eq!(p.holdings.len(), 1);
        assert_eq!(p.holdings[0].symbol, "RELIANCE");
        assert_eq!(p.holdings[0].shares, 10.0);
        assert_eq!(p.holdings[0].avg_cost, 2500.0);
        assert_eq!(p.holdings[0].lots.len(), 1);
    }

    #[test]
    fn test_add_uppercases_symbol() {
        let mut p = Portfolio::default();
        p.add("reliance", 5.0, 1000.0);
        assert_eq!(p.holdings[0].symbol, "RELIANCE");
    }

    #[test]
    fn test_add_averages_cost_and_keeps_lots() {
        let mut p = Portfolio::default();
        p.add_lot("TCS", lot(10.0, 3000.0, "2023-01-10"));
        p.add_lot("TCS", lot(5.0, 3600.0, "2024-06-01"));
        let h = &p.holdings[0];
        assert_eq!(h.shares, 15.0);
        assert!((h.avg_cost - 3200.0).abs() < 1e-9);
        // The second purchase keeps its own date instead of the first lot's.
        assert_eq!(h.lots[1].date, "2024-06-01");
    }

    #[test]
    fn test_lots_are_kept_oldest_first() {
        let mut p = Portfolio::default();
        p.add_lot("TCS", lot(1.0, 10.0, "2024-06-01"));
        p.add_lot("TCS", lot(1.0, 20.0, "2022-01-01"));
        assert_eq!(p.holdings[0].lots[0].date, "2022-01-01");
    }

    #[test]
    fn test_add_multiple_stocks() {
        let mut p = Portfolio::default();
        p.add("RELIANCE", 10.0, 2500.0);
        p.add("TCS", 5.0, 3500.0);
        p.add("INFY", 20.0, 1500.0);
        assert_eq!(p.holdings.len(), 3);
    }

    #[test]
    fn test_remove() {
        let mut p = Portfolio::default();
        p.add("RELIANCE", 10.0, 2500.0);
        p.add("TCS", 5.0, 3500.0);
        assert!(p.remove("reliance"));
        assert_eq!(p.holdings.len(), 1);
        assert!(!p.remove("RELIANCE"));
        assert!(!Portfolio::default().remove("X"));
    }

    // ── Migration ──

    const OLD_FORMAT: &str = r#"{
  "holdings": [
    { "symbol": "TCS.NS", "shares": 10.0, "avg_cost": 3500.0, "added_at": "2023-04-12" },
    { "symbol": "AAPL", "shares": 2.5, "avg_cost": 180.0, "added_at": "not a date" }
  ]
}"#;

    #[test]
    fn old_format_is_migrated_with_a_one_time_backup() {
        let dir = temp_dir("migrate");
        let path = dir.join("portfolio.json");
        std::fs::write(&path, OLD_FORMAT).unwrap();

        let p = Portfolio::load_from(&path).unwrap();
        assert_eq!(p.version, SCHEMA_VERSION);
        let tcs = p.holding("TCS.NS").unwrap();
        assert_eq!(tcs.lots.len(), 1);
        assert_eq!(tcs.lots[0].qty, 10.0);
        assert_eq!(tcs.lots[0].price, 3500.0);
        assert_eq!(tcs.lots[0].date, "2023-04-12");
        assert_eq!(tcs.lots[0].source, "migrated");
        assert!(tcs.lots[0].date_estimated);
        assert_eq!(tcs.shares, 10.0);
        assert_eq!(tcs.avg_cost, 3500.0);
        // An unparseable date falls back to today rather than failing the load.
        assert_eq!(p.holding("AAPL").unwrap().lots[0].date, today());

        // The original file is backed up, and the new format written.
        let bak = dir.join("portfolio.json.bak");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), OLD_FORMAT);
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("\"version\": 2"), "{written}");
        assert!(written.contains("\"lots\""));
        // Older StockWise versions can still read the summary fields.
        assert!(written.contains("\"avg_cost\": 3500.0"));

        // Loading again changes nothing.
        let again = Portfolio::load_from(&path).unwrap();
        assert_eq!(again.holdings[0].lots, p.holdings[0].lots);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    #[test]
    fn fund_codes_stored_with_a_market_suffix_are_repaired_on_load() {
        let dir = temp_dir("repair-mf");
        let path = dir.join("portfolio.json");
        let mut old = Portfolio::default();
        // `portfolio add MF:120503` used to store `MF:120503.NS`; a later CAS
        // import added the same fund under its real symbol.
        old.add_lot("MF:120503.NS", lot(10.0, 50.0, "2024-01-10"));
        old.add_lot("MF:120503", lot(5.0, 60.0, "2023-06-01"));
        old.add_lot("MF:INF179K01UT0-USD", lot(2.0, 100.0, "2024-03-01"));
        old.add_lot("TCS.NS", lot(1.0, 3000.0, "2024-01-01"));
        old.realized.push(Sale {
            symbol: "MF:118834.NS".into(),
            qty: 1.0,
            buy_date: "2023-01-01".into(),
            buy_price: 10.0,
            sell_date: "2024-01-01".into(),
            sell_price: 12.0,
            buy_date_estimated: false,
            source: "manual".into(),
            order_id: None,
            intraday: false,
            account: None,
            origin: None,
            bonuses: Vec::new(),
            bonus_shares: false,
            charges: None,
            fx_currency: None,
            buy_fx_rate: None,
        });
        old.save_to(&path).unwrap();

        let p = Portfolio::load_from(&path).unwrap();
        let symbols: Vec<&str> = p.holdings.iter().map(|h| h.symbol.as_str()).collect();
        assert_eq!(symbols, ["MF:120503", "MF:INF179K01UT0", "TCS.NS"]);
        let fund = p.holding("MF:120503").unwrap();
        assert_eq!(fund.shares, 15.0);
        assert_eq!(fund.lots.len(), 2);
        assert_eq!(fund.lots[0].date, "2023-06-01", "lots stay oldest first");
        assert!((fund.avg_cost - 800.0 / 15.0).abs() < 1e-9);
        assert_eq!(p.realized[0].symbol, "MF:118834");

        // The repair is saved (with a one-time backup) and is idempotent.
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(!written.contains("MF:120503.NS"), "{written}");
        assert!(dir.join("portfolio.json.bak").exists());
        let again = Portfolio::load_from(&path).unwrap();
        assert_eq!(again.holdings.len(), 3);
        assert_eq!(again.holding("MF:120503").unwrap().lots, fund.lots);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    #[cfg(unix)]
    #[test]
    fn repair_still_loads_from_a_read_only_data_directory() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("repair-ro");
        let path = dir.join("portfolio.json");
        let mut old = Portfolio::default();
        old.add_lot("MF:120503.NS", lot(10.0, 50.0, "2024-01-10"));
        old.save_to(&path).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let p = Portfolio::load_from(&path);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let p = p.expect("a failed repair save must not fail the load");
        assert_eq!(p.holdings[0].symbol, "MF:120503");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), written);
    }

    #[test]
    fn migration_never_overwrites_an_existing_backup() {
        let dir = temp_dir("migrate-bak");
        let path = dir.join("portfolio.json");
        let bak = dir.join("portfolio.json.bak");
        std::fs::write(&path, OLD_FORMAT).unwrap();
        std::fs::write(&bak, "first backup").unwrap();
        Portfolio::load_from(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "first backup");
    }

    #[test]
    fn missing_file_is_an_empty_new_format_portfolio() {
        let dir = temp_dir("missing");
        let path = dir.join("portfolio.json");
        let p = Portfolio::load_from(&path).unwrap();
        assert!(p.holdings.is_empty());
        assert!(!path.exists());
        assert!(!dir.join("portfolio.json.bak").exists());
    }

    #[test]
    fn file_from_a_newer_version_is_refused() {
        let dir = temp_dir("newer");
        let path = dir.join("portfolio.json");
        std::fs::write(&path, r#"{"version": 99, "holdings": []}"#).unwrap();
        let err = Portfolio::load_from(&path).unwrap_err().to_string();
        assert!(err.contains("newer StockWise"), "{err}");
    }

    #[test]
    fn new_format_round_trips() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("p.json");
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(10.0, 3000.0, "2023-01-10"));
        p.sell_fifo("TCS.NS", 4.0, 3500.0, "2024-02-01", "manual")
            .unwrap();
        p.save_to(&path).unwrap();
        let back = Portfolio::load_from(&path).unwrap();
        assert_eq!(back.holdings[0].lots, p.holdings[0].lots);
        assert_eq!(back.realized, p.realized);
        assert!(!dir.join("p.json.bak").exists());
    }

    // ── FIFO sales ──

    #[test]
    fn sell_takes_oldest_lots_first_and_records_realized() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(10.0, 1000.0, "2022-01-01"));
        p.add_lot("INFY.NS", lot(10.0, 1500.0, "2024-01-01"));
        let sales = p
            .sell_fifo("infy.ns", 15.0, 1600.0, "2024-06-01", "manual")
            .unwrap();
        assert_eq!(sales.len(), 2);
        assert_eq!((sales[0].qty, sales[0].buy_price), (10.0, 1000.0));
        assert_eq!((sales[1].qty, sales[1].buy_price), (5.0, 1500.0));
        assert_eq!(sales[0].gain(), 6000.0);
        assert_eq!(sales[1].gain(), 500.0);
        assert_eq!(p.realized.len(), 2);
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!(h.shares, 5.0);
        assert_eq!(h.lots.len(), 1);
        assert_eq!(h.lots[0].date, "2024-01-01");
        assert_eq!(h.avg_cost, 1500.0);
    }

    #[test]
    fn selling_everything_drops_the_holding_but_keeps_the_sale() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(3.0, 100.0, "2024-01-01"));
        p.sell_fifo("TCS.NS", 3.0, 90.0, "2024-02-01", "manual")
            .unwrap();
        assert!(p.holdings.is_empty());
        assert_eq!(p.realized[0].gain(), -30.0);
        assert_eq!(p.symbols_with_history(), vec!["TCS.NS".to_string()]);
    }

    #[test]
    fn overselling_or_selling_before_buying_is_an_error() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(5.0, 100.0, "2024-01-01"));
        p.add_lot("TCS.NS", lot(5.0, 100.0, "2024-06-01"));
        let err = p
            .sell_fifo("TCS.NS", 11.0, 1.0, "2024-07-01", "manual")
            .unwrap_err();
        assert!(err.to_string().contains("you hold 10"), "{err}");
        let err = p
            .sell_fifo("TCS.NS", 6.0, 1.0, "2024-03-01", "manual")
            .unwrap_err();
        assert!(err.to_string().contains("only 5"), "{err}");
        assert!(p.sell_fifo("NOPE", 1.0, 1.0, "2024-03-01", "m").is_err());
        assert!(p.sell_fifo("TCS.NS", 0.0, 1.0, "2024-03-01", "m").is_err());
        // Nothing changed after the failed sales.
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 10.0);
        assert!(p.realized.is_empty());
    }

    #[test]
    fn fractional_sell_of_whole_position_leaves_no_dust() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", lot(0.1, 100.0, "2024-01-01"));
        p.add_lot("AAPL", lot(0.2, 100.0, "2024-01-02"));
        p.sell_fifo("AAPL", 0.3, 120.0, "2024-03-01", "manual")
            .unwrap();
        assert!(p.holdings.is_empty());
    }

    #[test]
    fn lot_date_can_be_corrected() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(1.0, 10.0, "2024-01-01"));
        p.add_lot("TCS.NS", lot(2.0, 10.0, "2024-06-01"));
        p.holdings[0].lots[1].date_estimated = true;
        p.set_lot_date("tcs.ns", 2, "2023-03-01").unwrap();
        let h = p.holding("TCS.NS").unwrap();
        // Re-sorted, and no longer estimated.
        assert_eq!(h.lots[0].date, "2023-03-01");
        assert_eq!(h.lots[0].qty, 2.0);
        assert!(!h.has_estimated_dates());
        assert!(p.set_lot_date("TCS.NS", 3, "2023-03-01").is_err());
        assert!(p.set_lot_date("TCS.NS", 0, "2023-03-01").is_err());
    }

    // ── Broker sync and imports ──

    #[test]
    fn sync_keeps_lots_and_only_adjusts_quantity() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(10.0, 3000.0, "2023-01-10"));
        assert_eq!(
            p.sync_quantity("TCS.NS", 10.0, 3100.0, "broker:angel"),
            SyncChange::Unchanged
        );
        assert_eq!(p.holding("TCS.NS").unwrap().lots[0].price, 3000.0);

        // 5 more shares; broker average 3200 → the new lot cost 3600.
        assert_eq!(
            p.sync_quantity("TCS.NS", 15.0, 3200.0, "broker:angel"),
            SyncChange::Added(5.0)
        );
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots[0].date, "2023-01-10");
        assert!((h.lots[1].price - 3600.0).abs() < 1e-9);
        assert!(h.lots[1].date_estimated);

        // Fewer shares: taken from the oldest lot, no sale recorded.
        assert_eq!(
            p.sync_quantity("TCS.NS", 12.0, 3200.0, "broker:angel"),
            SyncChange::Reduced(3.0)
        );
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots[0].qty, 7.0);
        assert!(p.realized.is_empty());

        assert_eq!(
            p.sync_quantity("INFY.NS", 4.0, 1500.0, "x"),
            SyncChange::New
        );
    }

    #[test]
    fn import_without_dates_is_idempotent() {
        let rows = vec![ImportRow {
            symbol: "TCS.NS".into(),
            qty: 10.0,
            price: 3500.0,
            date: None,
            sell: false,
            units_as_of: None,
        }];
        let mut p = Portfolio::default();
        p.apply_import(&rows, opts("import:kite"));
        let out = p.apply_import(&rows, opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Unchanged));
        assert_eq!(p.holdings[0].shares, 10.0);
    }

    #[test]
    fn tradebook_import_rebuilds_lots_and_realized_idempotently() {
        let row = |date: &str, qty: f64, price: f64, sell: bool| ImportRow {
            symbol: "INFY.NS".into(),
            qty,
            price,
            date: Some(date.into()),
            sell,
            units_as_of: None,
        };
        let rows = vec![
            row("2024-05-01", 5.0, 1500.0, false),
            row("2023-01-01", 10.0, 1000.0, false),
            row("2024-06-01", 12.0, 1600.0, true),
            row("2024-07-01", 3.0, 1700.0, true),
        ];
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(99.0, 1.0, "2020-01-01"));
        p.sell_fifo("INFY.NS", 1.0, 1.5, "2024-06-01", "manual")
            .unwrap();

        // The manual lot and sale would be replaced: refused without `replace`.
        let out = p.apply_import(&rows, opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::WouldReplace);
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 98.0);

        let replace = ImportOptions {
            replace: true,
            ..opts("import:kite")
        };
        let out = p.apply_import(&rows, replace);
        assert_eq!(
            out[0].1,
            ImportOutcome::Lots {
                lots: 0,
                sales: 3,
                splits: 0
            }
        );
        // Bought 15, sold 15: the holding is gone, three FIFO slices realized
        // (the manual sale inside the file's range is replaced).
        assert!(p.holding("INFY.NS").is_none());
        assert_eq!(p.realized.len(), 3);
        assert_eq!(p.realized[0].buy_date, "2023-01-01");
        assert_eq!(p.realized[1].qty, 2.0);
        // Re-importing the same file changes nothing, even without `replace`.
        p.apply_import(&rows, opts("import:kite"));
        assert_eq!(p.realized.len(), 3);
    }

    #[test]
    fn tradebook_replay_applies_splits_between_buy_and_sell() {
        // 10 bought at 2800, 2:1 bonus, then 20 (post-bonus) sold at 1500.
        // Bought after the Jio Financial demerger, so only the split applies.
        let rows = vec![
            ImportRow {
                symbol: "RELIANCE.NS".into(),
                qty: 10.0,
                price: 2800.0,
                date: Some("2023-08-02".into()),
                sell: false,
                units_as_of: None,
            },
            ImportRow {
                symbol: "RELIANCE.NS".into(),
                qty: 15.0,
                price: 1500.0,
                date: Some("2025-01-10".into()),
                sell: true,
                units_as_of: None,
            },
        ];
        let splits = vec![("RELIANCE.NS".to_string(), vec![split_2_for_1()])];
        let tradebook = ImportOptions {
            tradebook: true,
            splits: &splits,
            ..opts("import:kite")
        };
        let mut p = Portfolio::default();
        let out = p.apply_import(&rows, tradebook);
        assert_eq!(
            out[0].1,
            ImportOutcome::Lots {
                lots: 1,
                sales: 1,
                splits: 1
            }
        );
        assert_eq!(p.realized[0].buy_price, 1400.0);
        assert_eq!(p.realized[0].gain(), 15.0 * 100.0);
        let h = p.holding("RELIANCE.NS").unwrap();
        assert_eq!((h.shares, h.avg_cost), (5.0, 1400.0));
        // Already adjusted: `actions` has nothing left to apply.
        assert_eq!(p.apply_split("RELIANCE.NS", &split_2_for_1()), 0);
        // Re-import is stable.
        p.apply_import(&rows, tradebook);
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 5.0);
        assert_eq!(p.realized.len(), 1);
    }

    fn trade(sym: &str, date: &str, sell: bool, qty: f64, price: f64) -> ImportRow {
        ImportRow {
            symbol: sym.into(),
            qty,
            price,
            date: Some(date.into()),
            sell,
            units_as_of: None,
        }
    }

    fn tradebook_opts(source: &str) -> ImportOptions<'_> {
        ImportOptions {
            tradebook: true,
            ..opts(source)
        }
    }

    /// Lots and sales as comparable text, to compare two portfolios.
    fn state(p: &Portfolio) -> (Vec<String>, Vec<String>) {
        let mut lots = Vec::new();
        for h in &p.holdings {
            for l in &h.lots {
                lots.push(format!(
                    "{} {} {} {} {}",
                    h.symbol, l.date, l.qty, l.price, l.source
                ));
            }
        }
        let mut sales: Vec<String> = p
            .realized
            .iter()
            .map(|s| {
                format!(
                    "{} {} {} {} {} {} {}",
                    s.symbol, s.qty, s.buy_date, s.buy_price, s.sell_date, s.sell_price, s.source
                )
            })
            .collect();
        sales.sort();
        (lots, sales)
    }

    #[test]
    fn a_second_tradebook_is_merged_not_replacing_the_first() {
        // The repro from the issue: one tradebook per year.
        let y2024 = vec![trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0)];
        let y2025 = vec![
            trade("TCS.NS", "2025-02-01", false, 5.0, 4000.0),
            trade("TCS.NS", "2025-03-01", true, 3.0, 4100.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&y2024, tradebook_opts("import:kite"));
        let out = p.apply_import(&y2025, tradebook_opts("import:kite"));
        assert_eq!(
            out[0].1,
            ImportOutcome::Lots {
                lots: 2,
                sales: 1,
                splits: 0
            }
        );
        let h = p.holding("TCS.NS").unwrap();
        // 10 + 5 - 3 = 12 shares; the sale came out of the 2024 lot (FIFO).
        assert_eq!(h.shares, 12.0);
        assert_eq!(
            (h.lots[0].qty, h.lots[0].date.as_str()),
            (7.0, "2024-01-10")
        );
        assert_eq!((h.lots[1].qty, h.lots[1].price), (5.0, 4000.0));
        assert_eq!(p.realized.len(), 1);
        let sale = &p.realized[0];
        assert_eq!(
            (sale.qty, sale.buy_date.as_str(), sale.buy_price),
            (3.0, "2024-01-10", 3500.0)
        );
        // Cost 7*3500 + 5*4000 = 44,500; gain on the sale 3 * 600.
        assert_eq!(h.cost_basis(), 44_500.0);
        assert_eq!(sale.gain(), 1800.0);
    }

    #[test]
    fn tradebooks_give_the_same_portfolio_in_any_order_or_combined() {
        let y2023 = vec![
            trade("TCS.NS", "2023-02-01", false, 4.0, 3300.0),
            trade("INFY.NS", "2023-03-01", false, 20.0, 1400.0),
        ];
        let y2024 = vec![
            trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0),
            trade("TCS.NS", "2024-06-03", true, 6.0, 3900.0),
            trade("INFY.NS", "2024-07-01", true, 5.0, 1500.0),
        ];
        let y2025 = vec![
            trade("TCS.NS", "2025-02-01", false, 5.0, 4000.0),
            trade("TCS.NS", "2025-03-01", true, 3.0, 4100.0),
            trade("INFY.NS", "2025-03-05", true, 15.0, 1700.0),
        ];
        let import = |files: &[&Vec<ImportRow>]| {
            let mut p = Portfolio::default();
            for f in files {
                p.apply_import(f, tradebook_opts("import:kite"));
            }
            state(&p)
        };
        let combined: Vec<ImportRow> = [&y2023, &y2024, &y2025]
            .iter()
            .flat_map(|f| f.iter().cloned())
            .collect();
        let want = import(&[&combined]);
        assert_eq!(want.1.len(), 5, "{:?}", want);
        for order in [
            [&y2023, &y2024, &y2025],
            [&y2025, &y2024, &y2023],
            [&y2024, &y2025, &y2023],
            [&y2025, &y2023, &y2024],
        ] {
            assert_eq!(import(&order), want);
        }
        // Importing every file twice changes nothing.
        assert_eq!(
            import(&[&y2023, &y2024, &y2025, &y2023, &y2025, &y2024]),
            want
        );
    }

    #[test]
    fn a_tradebook_that_sells_before_its_buys_waits_for_the_earlier_file() {
        let early = vec![trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0)];
        let late = vec![trade("TCS.NS", "2025-03-01", true, 3.0, 4100.0)];
        let mut p = Portfolio::default();
        let out = p.apply_import(&late, tradebook_opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::Unmatched { unmatched: 3.0 });
        assert!(p.holding("TCS.NS").is_none() && p.realized.is_empty());
        let out = p.apply_import(&early, tradebook_opts("import:kite"));
        assert!(matches!(out[0].1, ImportOutcome::Lots { sales: 1, .. }));
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 7.0);
    }

    #[test]
    fn overlapping_tradebooks_count_a_trade_once_but_keep_equal_fills() {
        let fills = vec![
            trade("TCS.NS", "2024-01-10", false, 5.0, 3500.0),
            trade("TCS.NS", "2024-01-10", false, 5.0, 3500.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&fills, tradebook_opts("import:kite"));
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 10.0);
        // A later download that repeats those two fills and adds one more.
        let mut overlap = fills.clone();
        overlap.push(trade("TCS.NS", "2024-02-01", false, 1.0, 3600.0));
        p.apply_import(&overlap, tradebook_opts("import:kite"));
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 11.0);
        p.apply_import(&fills, tradebook_opts("import:kite"));
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 11.0);
    }

    #[test]
    fn replace_rebuilds_from_the_file_alone() {
        let y2024 = vec![trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0)];
        let y2025 = vec![trade("TCS.NS", "2025-02-01", false, 5.0, 4000.0)];
        let mut p = Portfolio::default();
        p.apply_import(&y2024, tradebook_opts("import:kite"));
        let replace = ImportOptions {
            replace: true,
            ..tradebook_opts("import:kite")
        };
        p.apply_import(&y2025, replace);
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 5.0);
        // The earlier trades are gone from the kept list too.
        p.apply_import(&y2025, tradebook_opts("import:kite"));
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 5.0);
    }

    #[test]
    fn a_lots_only_file_does_not_delete_tradebook_sales_unless_replacing() {
        let book = vec![
            trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0),
            trade("TCS.NS", "2024-06-01", true, 4.0, 3900.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&book, opts("import:kite"));
        assert_eq!(p.realized.len(), 1);
        assert_eq!(p.imported.len(), 2);
        let before = state(&p);

        // Dated buys, no sell rows, no tradebook: a lot list for the symbol.
        let lots_only = vec![trade("TCS.NS", "2024-01-10", false, 6.0, 3500.0)];
        let out = p.apply_import(&lots_only, opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::WouldReplace);
        assert_eq!(state(&p), before, "nothing changed");
        assert_eq!(p.imported.len(), 2, "kept trades survive");

        let replace = ImportOptions {
            replace: true,
            ..opts("import:kite")
        };
        let out = p.apply_import(&lots_only, replace);
        assert!(matches!(out[0].1, ImportOutcome::Lots { sales: 0, .. }));
        assert!(p.realized.is_empty());
        assert!(p.imported.is_empty());
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 6.0);
    }

    #[test]
    fn a_lots_only_file_is_still_idempotent() {
        let lots_only = vec![trade("TCS.NS", "2024-01-10", false, 6.0, 3500.0)];
        let mut p = Portfolio::default();
        p.apply_import(&lots_only, opts("import:csv"));
        let before = state(&p);
        let out = p.apply_import(&lots_only, opts("import:csv"));
        assert!(matches!(out[0].1, ImportOutcome::Lots { .. }), "{:?}", out);
        assert_eq!(state(&p), before);
    }

    #[test]
    fn an_import_made_before_trades_were_kept_is_not_overwritten() {
        // Lots and a sale from an earlier version's import of the 2024 file.
        let y2024 = vec![
            trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0),
            trade("TCS.NS", "2024-06-01", true, 4.0, 3900.0),
        ];
        let mut old = Portfolio::default();
        old.apply_import(&y2024, tradebook_opts("import:kite"));
        old.imported.clear();
        let y2025 = vec![trade("TCS.NS", "2025-02-01", false, 5.0, 4000.0)];

        // A different file can't be told apart from a wrong one: refuse.
        let mut p = old.clone();
        let out = p.apply_import(&y2025, tradebook_opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::WouldReplace);
        assert_eq!(state(&p), state(&old));

        // The same file is recognised and its trades kept, so the next file merges.
        let out = p.apply_import(&y2024, tradebook_opts("import:kite"));
        assert!(matches!(out[0].1, ImportOutcome::Lots { .. }));
        assert_eq!(state(&p), state(&old));
        p.apply_import(&y2025, tradebook_opts("import:kite"));
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 11.0);
        assert_eq!(p.realized.len(), 1);
    }

    #[test]
    fn old_files_without_kept_trades_still_load() {
        let p: Portfolio = serde_json::from_str(r#"{"version":2,"holdings":[]}"#).unwrap();
        assert!(p.imported.is_empty());
        let mut q = Portfolio::default();
        q.apply_import(
            &[trade("TCS.NS", "2024-01-10", false, 1.0, 1.0)],
            tradebook_opts("import:kite"),
        );
        let json = serde_json::to_string(&q).unwrap();
        let back: Portfolio = serde_json::from_str(&json).unwrap();
        assert_eq!(back.imported, q.imported);
    }

    #[test]
    fn same_day_buy_and_sell_is_intraday_and_leaves_old_lots_alone() {
        // The repro from the issue.
        let rows = vec![
            trade("TCS.NS", "2023-01-10", false, 100.0, 3000.0),
            trade("TCS.NS", "2025-06-02", false, 50.0, 4000.0),
            trade("TCS.NS", "2025-06-02", true, 50.0, 4010.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&rows, tradebook_opts("import:kite"));
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots.len(), 1);
        assert_eq!(
            (h.shares, h.avg_cost, h.lots[0].date.as_str()),
            (100.0, 3000.0, "2023-01-10")
        );
        assert_eq!(p.realized.len(), 1);
        let s = &p.realized[0];
        assert!(s.intraday && s.is_intraday());
        assert_eq!(
            (s.qty, s.buy_date.as_str(), s.sell_date.as_str()),
            (50.0, "2025-06-02", "2025-06-02")
        );
        // 50 × (4010 − 4000) = ₹500, not 50 × (4010 − 3000) = ₹50,500.
        assert_eq!(s.gain(), 500.0);
        // Re-importing changes nothing.
        let before = state(&p);
        p.apply_import(&rows, tradebook_opts("import:kite"));
        assert_eq!(state(&p), before);
    }

    #[test]
    fn partial_same_day_netting_keeps_the_rest_as_a_new_lot() {
        let rows = vec![
            trade("TCS.NS", "2023-01-10", false, 10.0, 3000.0),
            trade("TCS.NS", "2025-06-02", false, 100.0, 4000.0),
            trade("TCS.NS", "2025-06-02", true, 60.0, 4020.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&rows, tradebook_opts("import:kite"));
        // 60 intraday; 40 left from the day's buy become a lot; the 2023 lot is whole.
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots.len(), 2);
        assert_eq!((h.lots[0].qty, h.lots[0].price), (10.0, 3000.0));
        assert_eq!(
            (h.lots[1].qty, h.lots[1].price, h.lots[1].date.as_str()),
            (40.0, 4000.0, "2025-06-02")
        );
        assert_eq!(p.realized.len(), 1);
        assert_eq!((p.realized[0].qty, p.realized[0].gain()), (60.0, 1200.0));
    }

    #[test]
    fn a_sell_larger_than_the_days_buy_takes_the_rest_from_old_lots() {
        // Buy 20 @ 100 and sell 50 @ 110 on one day, holding 100 @ 90 from before.
        let rows = vec![
            trade("X.NS", "2024-01-02", false, 100.0, 90.0),
            trade("X.NS", "2025-06-02", false, 10.0, 100.0),
            trade("X.NS", "2025-06-02", false, 10.0, 104.0),
            trade("X.NS", "2025-06-02", true, 50.0, 110.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&rows, tradebook_opts("import:kite"));
        assert_eq!(p.realized.len(), 2);
        let intraday = p.realized.iter().find(|s| s.intraday).unwrap();
        // Average buy of the day 102; 20 matched.
        assert_eq!(
            (intraday.qty, intraday.buy_price, intraday.sell_price),
            (20.0, 102.0, 110.0)
        );
        let old = p.realized.iter().find(|s| !s.intraday).unwrap();
        assert_eq!(
            (old.qty, old.buy_date.as_str(), old.buy_price),
            (30.0, "2024-01-02", 90.0)
        );
        assert_eq!(p.holding("X.NS").unwrap().shares, 70.0);
        // Proceeds are all accounted for: 50 × 110.
        let proceeds: f64 = p.realized.iter().map(|s| s.qty * s.sell_price).sum();
        assert_eq!(proceeds, 5500.0);
    }

    #[test]
    fn intraday_days_do_not_change_other_days() {
        // Buy on one day, sell on another: ordinary FIFO, not intraday.
        let rows = vec![
            trade("X.NS", "2025-06-02", false, 10.0, 100.0),
            trade("X.NS", "2025-06-03", true, 10.0, 110.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&rows, tradebook_opts("import:kite"));
        assert_eq!(p.realized.len(), 1);
        assert!(!p.realized[0].is_intraday());
    }

    #[test]
    fn broker_fills_net_same_day_buys_and_sells_before_old_lots() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(100.0, 1000.0, "2023-01-10"));
        // The sell is listed (and timed) first; the day's buy still nets with it.
        let fills = vec![
            fill("S1", "T2", true, 30.0, 1520.0, "2026-10-20 10:00:00"),
            fill("B1", "T1", false, 50.0, 1500.0, "2026-10-20 11:00:00"),
        ];
        let s = p.apply_broker_fills(&fills);
        assert_eq!(s.orders.len(), 2);
        let FillEffect::Sold { sales } = &s.orders[1].effect else {
            panic!("expected a sale: {:?}", s.orders[1].effect);
        };
        assert_eq!(sales.len(), 1);
        assert!(sales[0].intraday);
        assert_eq!(
            (sales[0].qty, sales[0].buy_price, sales[0].buy_date.as_str()),
            (30.0, 1500.0, "2026-10-20")
        );
        let h = p.holding("INFY.NS").unwrap();
        // 2023 lot untouched; 20 of the day's 50 remain.
        assert_eq!(h.lots.len(), 2);
        assert_eq!((h.lots[0].qty, h.lots[1].qty), (100.0, 20.0));
        assert!(p.realized[0].intraday);
        // Syncing again changes nothing.
        assert!(p.apply_broker_fills(&fills).orders.is_empty());
        assert_eq!(p.realized.len(), 1);
    }

    #[test]
    fn broker_sell_without_a_same_day_buy_is_not_intraday() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(10.0, 1000.0, "2024-01-10"));
        p.apply_broker_fills(&[fill("S1", "T1", true, 4.0, 1500.0, "2026-10-20 10:00:00")]);
        assert!(!p.realized[0].is_intraday());
    }

    #[test]
    fn estimated_purchase_date_is_not_intraday() {
        let mut p = Portfolio::default();
        let mut l = lot(5.0, 100.0, "2026-10-20");
        l.date_estimated = true;
        p.add_lot("X.NS", l);
        let sales = p
            .sell_fifo("X.NS", 5.0, 110.0, "2026-10-20", "manual")
            .unwrap();
        assert!(!sales[0].is_intraday());
    }

    #[test]
    fn broker_sell_does_not_net_against_an_estimated_lot_dated_today() {
        // A holdings snapshot dated 50 shares today (date_estimated): they
        // may have been bought any day, so a sale today is not intraday and
        // comes out of the oldest lot first.
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(100.0, 1000.0, "2023-01-10"));
        let mut est = lot(50.0, 1500.0, "2026-10-20");
        est.date_estimated = true;
        p.add_lot("INFY.NS", est);
        let fills = vec![fill("S1", "T1", true, 30.0, 1520.0, "2026-10-20 10:00:00")];
        p.apply_broker_fills(&fills);
        assert_eq!(p.realized.len(), 1);
        let s = &p.realized[0];
        assert!(!s.intraday && !s.is_intraday());
        assert_eq!((s.qty, s.buy_date.as_str()), (30.0, "2023-01-10"));
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!((h.lots[0].qty, h.lots[1].qty), (70.0, 50.0));

        // With only the estimated lot held, it is sold, still not intraday.
        let mut q = Portfolio::default();
        let mut est = lot(50.0, 1500.0, "2026-10-20");
        est.date_estimated = true;
        q.add_lot("INFY.NS", est);
        q.apply_broker_fills(&fills);
        let s = &q.realized[0];
        assert!(s.buy_date_estimated && !s.is_intraday());
    }

    #[test]
    fn tradebook_buy_without_a_date_does_not_net_with_a_sale_today() {
        let today = today();
        let mut buy = trade("TCS.NS", &today, false, 10.0, 100.0);
        buy.date = None;
        let rows = vec![buy, trade("TCS.NS", &today, true, 10.0, 110.0)];
        let mut p = Portfolio::default();
        p.apply_import(&rows, tradebook_opts("import:kite"));
        assert_eq!(p.realized.len(), 1);
        let s = &p.realized[0];
        assert!(
            !s.intraday && !s.is_intraday(),
            "taxed as delivery, not intraday"
        );
        assert!(s.buy_date_estimated);
        assert!(p.holding("TCS.NS").is_none());
        // A dated buy the same day still nets.
        let rows = vec![
            trade("TCS.NS", &today, false, 10.0, 100.0),
            trade("TCS.NS", &today, true, 10.0, 110.0),
        ];
        let mut q = Portfolio::default();
        q.apply_import(&rows, tradebook_opts("import:kite"));
        assert!(q.realized[0].intraday);
    }

    fn snapshot(sym: &str, qty: f64, price: f64) -> ImportRow {
        ImportRow {
            symbol: sym.into(),
            qty,
            price,
            date: None,
            sell: false,
            units_as_of: None,
        }
    }

    fn by_account(p: &Portfolio, sym: &str) -> Vec<(String, f64)> {
        let mut out: Vec<(String, f64)> = Vec::new();
        for l in &p.holding(sym).unwrap().lots {
            let key = l.account_key().unwrap_or("-").to_string();
            match out.iter_mut().find(|(a, _)| *a == key) {
                Some((_, q)) => *q += l.qty,
                None => out.push((key, l.qty)),
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    #[test]
    fn a_holdings_file_from_a_second_broker_adds_to_the_first() {
        // The repro from the issue: 10 INFY at Zerodha, 5 at Groww.
        let kite = vec![snapshot("INFY.NS", 10.0, 1500.0)];
        let groww = vec![snapshot("INFY.NS", 5.0, 1600.0)];
        let mut p = Portfolio::default();
        p.apply_import(&kite, opts("import:kite"));
        let out = p.apply_import(&groww, opts("import:groww"));
        assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Added(5.0)));
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!(h.shares, 15.0);
        assert!((h.cost_basis() - (10.0 * 1500.0 + 5.0 * 1600.0)).abs() < 1e-6);
        assert_eq!(
            by_account(&p, "INFY.NS"),
            [("groww".to_string(), 5.0), ("zerodha".to_string(), 10.0)]
        );
        // Re-importing either file changes nothing, whatever the order.
        for (rows, source) in [
            (&kite, "import:kite"),
            (&groww, "import:groww"),
            (&kite, "import:kite"),
        ] {
            let out = p.apply_import(rows, opts(source));
            assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Unchanged));
            assert_eq!(p.holding("INFY.NS").unwrap().shares, 15.0);
        }
        // Either order gives the same two accounts.
        let mut q = Portfolio::default();
        q.apply_import(&groww, opts("import:groww"));
        q.apply_import(&kite, opts("import:kite"));
        assert_eq!(by_account(&q, "INFY.NS"), by_account(&p, "INFY.NS"));
        // A smaller snapshot from one broker only reduces that broker's lots.
        p.apply_import(&[snapshot("INFY.NS", 8.0, 1500.0)], opts("import:kite"));
        assert_eq!(
            by_account(&p, "INFY.NS"),
            [("groww".to_string(), 5.0), ("zerodha".to_string(), 8.0)]
        );
    }

    #[test]
    fn tradebooks_from_two_brokers_are_kept_apart() {
        let kite = vec![
            trade("TCS.NS", "2024-01-10", false, 10.0, 3500.0),
            trade("TCS.NS", "2024-06-03", true, 4.0, 3900.0),
        ];
        let groww = vec![
            trade("TCS.NS", "2024-02-01", false, 5.0, 3600.0),
            trade("TCS.NS", "2024-07-01", true, 2.0, 3950.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&kite, tradebook_opts("import:kite"));
        let out = p.apply_import(&groww, tradebook_opts("import:groww"));
        assert!(
            matches!(
                out[0].1,
                ImportOutcome::Lots {
                    lots: 1,
                    sales: 1,
                    ..
                }
            ),
            "{:?}",
            out
        );
        // 6 left at Zerodha, 3 at Groww; each sale came out of its own account's lot.
        assert_eq!(
            by_account(&p, "TCS.NS"),
            [("groww".to_string(), 3.0), ("zerodha".to_string(), 6.0)]
        );
        assert_eq!(p.realized.len(), 2);
        let sale = |acct: &str| {
            p.realized
                .iter()
                .find(|s| s.account_key() == Some(acct))
                .unwrap()
        };
        assert_eq!(
            (sale("zerodha").buy_date.as_str(), sale("zerodha").buy_price),
            ("2024-01-10", 3500.0)
        );
        assert_eq!(
            (sale("groww").buy_date.as_str(), sale("groww").buy_price),
            ("2024-02-01", 3600.0)
        );
        // Re-importing either file, in either order, changes nothing.
        let before = state(&p);
        p.apply_import(&groww, tradebook_opts("import:groww"));
        p.apply_import(&kite, tradebook_opts("import:kite"));
        assert_eq!(state(&p), before);
        // Replacing one broker's lots leaves the other's.
        let replace = ImportOptions {
            replace: true,
            ..tradebook_opts("import:kite")
        };
        p.apply_import(&kite[..1], replace);
        assert_eq!(
            by_account(&p, "TCS.NS"),
            [("groww".to_string(), 3.0), ("zerodha".to_string(), 10.0)]
        );
        assert_eq!(p.realized.len(), 1);
    }

    #[test]
    fn a_broker_fill_sale_never_takes_another_accounts_lots() {
        let mut p = Portfolio::default();
        let mut groww = lot(10.0, 1000.0, "2024-01-10");
        groww.source = "import:groww".into();
        p.add_lot("INFY.NS", groww);
        // Zerodha holds nothing: its sell is left unsynced, Groww's lot stays whole.
        let s = p.apply_broker_fills(&[fill("S1", "T1", true, 5.0, 1500.0, "2026-10-20 10:00:00")]);
        assert_eq!(s.orders[0].effect, FillEffect::Unmatched { held: 0.0 });
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 10.0);
        assert!(p.realized.is_empty());
        // A Zerodha buy then sell works inside Zerodha's own lots.
        p.apply_broker_fills(&[
            fill("B1", "T2", false, 6.0, 1400.0, "2026-10-01 10:00:00"),
            fill("S1", "T1", true, 5.0, 1500.0, "2026-10-20 10:00:00"),
        ]);
        assert_eq!(
            by_account(&p, "INFY.NS"),
            [("groww".to_string(), 10.0), ("zerodha".to_string(), 1.0)]
        );
        assert_eq!(p.realized[0].buy_price, 1400.0);
    }

    #[test]
    fn a_manual_sell_still_spans_every_account() {
        let mut p = Portfolio::default();
        p.apply_import(&[snapshot("INFY.NS", 10.0, 1500.0)], opts("import:kite"));
        p.apply_import(&[snapshot("INFY.NS", 5.0, 1600.0)], opts("import:groww"));
        // No account named: `portfolio sell` works on the whole holding, oldest first.
        let sales = p
            .sell_fifo("INFY.NS", 12.0, 1700.0, &today(), "manual")
            .unwrap();
        assert_eq!(sales.iter().map(|s| s.qty).sum::<f64>(), 12.0);
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 3.0);
    }

    #[test]
    fn lots_without_an_account_join_the_first_broker_that_syncs() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(10.0, 1500.0, "2024-01-10"));
        // The user's manual 10 shares are Zerodha's: the same snapshot adds nothing.
        let out = p.apply_import(&[snapshot("INFY.NS", 10.0, 1500.0)], opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Unchanged));
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 10.0);
        // A second broker then adds its own shares instead of reducing them.
        let out = p.apply_import(&[snapshot("INFY.NS", 5.0, 1600.0)], opts("import:groww"));
        assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Added(5.0)));
        assert_eq!(
            by_account(&p, "INFY.NS"),
            [("groww".to_string(), 5.0), ("zerodha".to_string(), 10.0)]
        );
    }

    #[test]
    fn lots_saved_before_accounts_belong_to_the_account_their_source_names() {
        // A file written before `account` existed: only `source` says Zerodha.
        let json = r#"{"version":2,"holdings":[{"symbol":"INFY.NS","shares":10.0,"avg_cost":1500.0,
            "lots":[{"qty":10.0,"price":1500.0,"date":"2024-01-10","source":"import:kite"}]}]}"#;
        let mut p: Portfolio = serde_json::from_str(json).unwrap();
        p.normalize();
        assert_eq!(p.holdings[0].lots[0].account, None);
        assert_eq!(p.holdings[0].lots[0].account_key(), Some("zerodha"));
        // Importing the same Kite file again is a no-op, not a second account.
        let out = p.apply_import(&[snapshot("INFY.NS", 10.0, 1500.0)], opts("import:kite"));
        assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Unchanged));
        let out = p.apply_import(&[snapshot("INFY.NS", 5.0, 1600.0)], opts("import:groww"));
        assert_eq!(out[0].1, ImportOutcome::Synced(SyncChange::Added(5.0)));
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 15.0);
        // The file saves back without an account on lots that had none.
        let saved = serde_json::to_string(&p.holdings[0].lots[0]).unwrap();
        assert!(!saved.contains("account"), "{saved}");
    }

    #[test]
    fn account_names_follow_the_source() {
        assert_eq!(account_of_source("import:kite"), Some("zerodha"));
        assert_eq!(account_of_source("broker:zerodha"), Some("zerodha"));
        assert_eq!(account_of_source("broker:angel"), Some("angel"));
        assert_eq!(account_of_source("import:groww"), Some("groww"));
        for none in [
            "manual",
            "sync",
            "migrated",
            "import:csv",
            "import:cas",
            "broker:",
        ] {
            assert_eq!(account_of_source(none), None, "{none}");
        }
    }

    #[test]
    fn hdfc_trades_before_the_merger_become_hdfc_bank_shares_at_the_same_cost() {
        // The repro from the issue: 25 HDFC at 2600 on 2023-03-01.
        let swap = merger_swap("HDFC", Some("2023-03-01"), 25.0, 2600.0).unwrap();
        assert_eq!(swap.symbol, "HDFCBANK");
        assert_eq!(swap.qty, 42.0);
        assert!(
            (swap.price - 65_000.0 / 42.0).abs() < 1e-9 && (swap.price - 1547.62).abs() < 0.005
        );
        assert!((swap.qty * swap.price - 65_000.0).abs() < 1e-6);
        // The record date itself is still old shares; the day after is not a trade of HDFC.
        assert!(merger_swap("HDFC.NS", Some("2023-07-13"), 25.0, 2600.0).is_some());
        assert_eq!(
            merger_swap("hdfc.ns", Some("2023-03-01"), 1.0, 1.0)
                .unwrap()
                .symbol,
            "HDFCBANK.NS"
        );
        assert!(merger_swap("HDFC", Some("2023-07-14"), 25.0, 2600.0).is_none());
        // Without a date there is nothing to convert at.
        assert!(merger_swap("HDFC", None, 25.0, 2600.0).is_none());
        // HDFC Bank itself and look-alikes are untouched.
        assert!(merger_swap("HDFCBANK", Some("2023-03-01"), 1.0, 1.0).is_none());
        assert!(merger_swap("HDFCLIFE.NS", Some("2023-03-01"), 1.0, 1.0).is_none());

        // Through an import: one lot of 42 HDFCBANK, same total cost, same date.
        let mut p = Portfolio::default();
        let rows = [trade(
            &swap.symbol,
            "2023-03-01",
            false,
            swap.qty,
            swap.price,
        )];
        p.apply_import(&rows, tradebook_opts("import:kite"));
        let h = p.holding("HDFCBANK").unwrap();
        assert_eq!((h.shares, h.lots[0].date.as_str()), (42.0, "2023-03-01"));
        assert!((h.cost_basis() - 65_000.0).abs() < 1e-6);
    }

    #[test]
    fn mindtree_converts_at_73_for_100() {
        let swap = merger_swap("MINDTREE", Some("2022-06-01"), 100.0, 3000.0).unwrap();
        assert_eq!(swap.symbol, "LTIM");
        assert_eq!(swap.qty, 73.0);
        assert!((swap.price - 3000.0 * 100.0 / 73.0).abs() < 1e-9);
        assert!((swap.qty * swap.price - 300_000.0).abs() < 1e-6);
        assert!(merger_swap("MINDTREE.NS", Some("2022-11-24"), 1.0, 1.0).is_some());
        assert!(merger_swap("MINDTREE.NS", Some("2022-11-25"), 1.0, 1.0).is_none());
    }

    #[test]
    fn a_sell_after_the_merger_matches_the_converted_lot() {
        // Bought 25 HDFC in 2023-03 (→ 42 HDFCBANK); sold 10 HDFCBANK in 2024.
        let buy = merger_swap("HDFC", Some("2023-03-01"), 25.0, 2600.0).unwrap();
        let rows = [
            trade("HDFCBANK", "2023-03-01", false, buy.qty, buy.price),
            trade("HDFCBANK", "2024-02-01", true, 10.0, 1500.0),
        ];
        let mut p = Portfolio::default();
        let out = p.apply_import(&rows, tradebook_opts("import:kite"));
        assert!(
            matches!(out[0].1, ImportOutcome::Lots { sales: 1, .. }),
            "{:?}",
            out
        );
        assert_eq!(p.holding("HDFCBANK").unwrap().shares, 32.0);
        assert_eq!(p.realized[0].buy_date, "2023-03-01");
    }

    fn hdfc_split_2019() -> Split {
        Split {
            date: "2019-09-19".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        }
    }

    fn hdfcbank_bonus_2025() -> Split {
        Split {
            date: "2025-08-26".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        }
    }

    /// "HDFC buy 25 at 2000 on 2018-05-10" the way an import restates it.
    fn merged_hdfc_row() -> ImportRow {
        let swap = merger_swap("HDFC", Some("2018-05-10"), 25.0, 2000.0).unwrap();
        ImportRow {
            units_as_of: Some(swap.merger.through.to_string()),
            ..trade("HDFCBANK", "2018-05-10", false, swap.qty, swap.price)
        }
    }

    #[test]
    fn merged_hdfc_lot_does_not_get_the_pre_merger_split_again() {
        // 25 HDFC → 42 HDFCBANK at 2000 × 25 / 42 = 1190.476, cost 50,000.
        // The 42 are already in HDFC Bank's share count after the 2019 2:1
        // split, so only a split after the 2023-07-13 merger applies.
        let mut p = Portfolio::default();
        let splits = vec![(
            "HDFCBANK".to_string(),
            vec![hdfc_split_2019(), hdfcbank_bonus_2025()],
        )];
        let opts = ImportOptions {
            splits: &splits,
            ..tradebook_opts("import:kite")
        };
        // The sale (after all splits) makes this a tradebook.
        let rows = [
            merged_hdfc_row(),
            trade("HDFCBANK", "2025-09-10", true, 2.0, 900.0),
        ];
        let out = p.apply_import(&rows, opts);
        assert!(
            matches!(out[0].1, ImportOutcome::Lots { splits: 1, .. }),
            "{:?}",
            out
        );
        let l = &p.holding("HDFCBANK").unwrap().lots[0];
        // 42 × 2 (2025 bonus only) − 2 sold; cost per share 1190.476 / 2.
        assert!((l.qty - 82.0).abs() < 1e-9, "{}", l.qty);
        assert!((l.price - 50_000.0 / 84.0).abs() < 1e-6, "{}", l.price);
        assert_eq!(l.adjustments, vec!["2025-08-26:2:1".to_string()]);
        assert_eq!(l.date, "2018-05-10");
    }

    #[test]
    fn merged_hdfc_lot_with_only_old_splits_stays_42_at_1190() {
        let mut p = Portfolio::default();
        let splits = vec![("HDFCBANK".to_string(), vec![hdfc_split_2019()])];
        let opts = ImportOptions {
            splits: &splits,
            ..tradebook_opts("import:kite")
        };
        let rows = [merged_hdfc_row()];
        p.apply_import(&rows, opts);
        let l = &p.holding("HDFCBANK").unwrap().lots[0];
        assert_eq!(l.qty, 42.0);
        assert!((l.price - 1_190.476_190).abs() < 1e-5, "{}", l.price);
        assert!(l.adjustments.is_empty());
        // The same row without the marker is the bug: 84 shares at half the price.
        let mut q = Portfolio::default();
        let unmarked = [ImportRow {
            units_as_of: None,
            ..merged_hdfc_row()
        }];
        let opts = ImportOptions {
            splits: &splits,
            ..tradebook_opts("import:kite")
        };
        q.apply_import(&unmarked, opts);
        assert_eq!(q.holding("HDFCBANK").unwrap().lots[0].qty, 84.0);
    }

    #[test]
    fn merged_trades_keep_their_marker_across_reimports() {
        let mut p = Portfolio::default();
        let splits = vec![("HDFCBANK".to_string(), vec![hdfc_split_2019()])];
        for _ in 0..2 {
            let opts = ImportOptions {
                splits: &splits,
                ..tradebook_opts("import:kite")
            };
            p.apply_import(&[merged_hdfc_row()], opts);
        }
        assert_eq!(p.imported.len(), 1);
        assert_eq!(p.imported[0].units_as_of.as_deref(), Some("2023-07-13"));
        assert_eq!(p.holding("HDFCBANK").unwrap().lots[0].qty, 42.0);
    }

    #[test]
    fn apply_split_skips_a_merged_lot_for_splits_up_to_the_merger() {
        // A lot added by `portfolio add HDFC --date 2018-05-10`.
        let mut p = Portfolio::default();
        let mut merged = lot(42.0, 50_000.0 / 42.0, "2018-05-10");
        merged.units_as_of = Some("2023-07-13".into());
        p.add_lot("HDFCBANK.NS", merged);
        assert_eq!(p.apply_split("HDFCBANK.NS", &hdfc_split_2019()), 0);
        assert_eq!(p.apply_split("HDFCBANK.NS", &hdfcbank_bonus_2025()), 1);
        let l = &p.holding("HDFCBANK.NS").unwrap().lots[0];
        assert_eq!(l.qty, 84.0);
        assert!((l.cost() - 50_000.0).abs() < 1e-6);
        // On the merger's own record date the split is still not applied.
        let on_through = Split {
            date: "2023-07-13".into(),
            ..hdfc_split_2019()
        };
        assert!(!on_through.pending_for(&lot_with_units("2018-05-10", "2023-07-13")));
    }

    fn lot_with_units(date: &str, units_as_of: &str) -> Lot {
        Lot {
            units_as_of: Some(units_as_of.into()),
            ..lot(1.0, 1.0, date)
        }
    }

    #[test]
    fn partial_tradebook_leaves_the_holding_alone() {
        let rows = vec![ImportRow {
            symbol: "X".into(),
            qty: 2.0,
            price: 10.0,
            date: Some("2024-01-01".into()),
            sell: true,
            units_as_of: None,
        }];
        let mut p = Portfolio::default();
        p.add_lot("X", lot(5.0, 8.0, "2023-01-01"));
        let replace = ImportOptions {
            replace: true,
            ..opts("import:csv")
        };
        let out = p.apply_import(&rows, replace);
        assert_eq!(out[0].1, ImportOutcome::Unmatched { unmatched: 2.0 });
        assert_eq!(p.holding("X").unwrap().shares, 5.0);
        assert!(p.realized.is_empty());
    }

    #[test]
    fn trade_dates_parse_in_broker_formats() {
        let want = d("2024-03-15");
        for s in [
            "2024-03-15",
            "2024-03-15 09:30:00",
            "2024-03-15T09:30:00",
            "15-03-2024",
            "15/03/2024",
            "15-Mar-2024",
            "2024/03/15",
            "\"15 Mar 2024\"",
        ] {
            assert_eq!(parse_trade_date(s, false), Some(want), "{s}");
        }
        assert_eq!(parse_trade_date("03/15/2024", false), Some(want));
        assert_eq!(parse_trade_date("03/04/2024", true), Some(d("2024-03-04")));
        assert_eq!(parse_trade_date("03/04/2024", false), Some(d("2024-04-03")));
        assert_eq!(parse_trade_date("yesterday", false), None);
        assert_eq!(parse_trade_date("31/02/2024", false), None);
    }

    #[test]
    fn command_line_dates_are_strict_and_not_in_the_future() {
        assert!(parse_date("2024-01-31").is_ok());
        assert!(parse_date("31-01-2024").is_err());
        assert!(parse_date("2999-01-01").is_err());
    }

    // ── Corporate actions ──

    fn split_2_for_1() -> Split {
        Split {
            date: "2024-10-28".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        }
    }

    #[test]
    fn split_adjusts_older_lots_once_and_keeps_cost_and_date() {
        let mut p = Portfolio::default();
        p.add_lot("RELIANCE.NS", lot(10.0, 2800.0, "2023-05-02"));
        p.add_lot("RELIANCE.NS", lot(4.0, 1300.0, "2024-11-15"));
        let cost_before = p.holdings[0].cost_basis();

        assert_eq!(p.apply_split("RELIANCE.NS", &split_2_for_1()), 1);
        let h = p.holding("RELIANCE.NS").unwrap();
        assert_eq!((h.lots[0].qty, h.lots[0].price), (20.0, 1400.0));
        assert_eq!(h.lots[0].date, "2023-05-02");
        assert_eq!((h.lots[1].qty, h.lots[1].price), (4.0, 1300.0));
        assert_eq!(h.shares, 24.0);
        assert!((h.cost_basis() - cost_before).abs() < 1e-9);

        // Idempotent.
        assert_eq!(p.apply_split("RELIANCE.NS", &split_2_for_1()), 0);
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 24.0);
        assert_eq!(p.apply_split("NOPE", &split_2_for_1()), 0);
    }

    /// A 1:1 bonus (Yahoo: 2:1 split) with ex-date and allotment 2026-06-15.
    fn bonus_1_for_1() -> Split {
        Split {
            bonus: Some("2026-06-15".into()),
            ..Split::new("2026-06-15", 2.0, 1.0)
        }
    }

    #[test]
    fn indian_bonus_adds_a_zero_cost_lot_and_keeps_the_old_one() {
        // Issue #95: 100 bought at 1,000; a 1:1 bonus allotted on 2026-06-15.
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        assert_eq!(p.apply_split("TCS.NS", &bonus_1_for_1()), 1);
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots.len(), 2);
        let (old, new) = (&h.lots[0], &h.lots[1]);
        assert_eq!(
            (old.qty, old.price, old.date.as_str()),
            (100.0, 1000.0, "2023-05-01")
        );
        assert_eq!(old.adjustments, vec!["bonus:2026-06-15:2:1".to_string()]);
        assert_eq!(
            (new.qty, new.price, new.date.as_str()),
            (100.0, 0.0, "2026-06-15")
        );
        assert_eq!(new.source, BONUS_SOURCE);
        assert_eq!(new.adjustments, vec!["2026-06-15:2:1".to_string()]);
        assert_eq!((h.shares, h.cost_basis()), (200.0, 100_000.0));

        // Applying it again (as a bonus or as a plain split) changes nothing.
        assert_eq!(p.apply_split("TCS.NS", &bonus_1_for_1()), 0);
        assert_eq!(
            p.apply_split("TCS.NS", &Split::new("2026-06-15", 2.0, 1.0)),
            0
        );
        assert_eq!(p.holding("TCS.NS").unwrap().lots.len(), 2);

        // FIFO sells the 2023 shares first: a long-term loss of 40,000.
        let sales = p
            .sell_fifo("TCS.NS", 100.0, 600.0, "2026-07-01", "manual")
            .unwrap();
        assert_eq!(sales.len(), 1);
        assert_eq!(sales[0].buy_date, "2023-05-01");
        assert_eq!(sales[0].gain(), -40_000.0);
        assert!(crate::tax::Country::India.is_long_term(d(&sales[0].buy_date), d("2026-07-01")));
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots.len(), 1);
        assert_eq!((h.lots[0].qty, h.lots[0].price), (100.0, 0.0));
    }

    #[test]
    fn bonus_shares_follow_each_lots_account() {
        let mut p = Portfolio::default();
        let mut a = lot(10.0, 1000.0, "2023-05-01");
        a.account = Some("zerodha".into());
        let mut b = lot(30.0, 1100.0, "2024-01-01");
        b.account = Some("zerodha".into());
        p.add_lot("TCS.NS", a);
        p.add_lot("TCS.NS", b);
        p.add_lot(
            "TCS.NS",
            Lot::new(5.0, 1200.0, "2024-02-01", "import:groww"),
        );
        assert_eq!(p.apply_split("TCS.NS", &bonus_1_for_1()), 3);
        let bonus: Vec<(f64, Option<&str>)> = p
            .holding("TCS.NS")
            .unwrap()
            .lots
            .iter()
            .filter(|l| l.source == BONUS_SOURCE)
            .map(|l| (l.qty, l.account.as_deref()))
            .collect();
        assert_eq!(bonus, vec![(40.0, Some("zerodha")), (5.0, Some("groww"))]);
    }

    #[test]
    fn a_real_split_after_a_bonus_adjusts_both_lots() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        p.apply_split("TCS.NS", &bonus_1_for_1());
        // A 5:1 split (no `bonus`) later multiplies both lots.
        let split = Split::new("2026-09-01", 5.0, 1.0);
        assert_eq!(p.apply_split("TCS.NS", &split), 2);
        let h = p.holding("TCS.NS").unwrap();
        let lots: Vec<(f64, f64, &str)> = h
            .lots
            .iter()
            .map(|l| (l.qty, l.price, l.date.as_str()))
            .collect();
        assert_eq!(
            lots,
            vec![(500.0, 200.0, "2023-05-01"), (500.0, 0.0, "2026-06-15")]
        );
        assert_eq!(p.apply_split("TCS.NS", &split), 0);
    }

    #[test]
    fn a_bonus_without_nse_or_outside_india_stays_a_split() {
        // `bonus: None` is what a non-Indian symbol, or NSE being
        // unreachable, leaves: the old behaviour.
        let mut p = Portfolio::default();
        p.add_lot("AAPL", lot(10.0, 150.0, "2023-05-01"));
        assert_eq!(
            p.apply_split("AAPL", &Split::new("2026-06-15", 2.0, 1.0)),
            1
        );
        let h = p.holding("AAPL").unwrap();
        assert_eq!(h.lots.len(), 1);
        assert_eq!((h.lots[0].qty, h.lots[0].price), (20.0, 75.0));
    }

    #[test]
    fn dividends_count_bonus_shares_in_todays_share_count() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        let splits = vec![bonus_1_for_1()];
        p.apply_split("TCS.NS", &splits[0]);
        // Before the bonus: Yahoo's per-share amount is in today's count, so
        // the 100 shares held then count as 200.
        let before = DividendEvent {
            ex_date: "2025-06-01".into(),
            per_share: 5.0,
        };
        assert_eq!(p.dividend_due("TCS.NS", &before, &splits), Some(200.0));
        // After it: the old lot and the bonus lot, 200 in all.
        let after = DividendEvent {
            ex_date: "2026-08-01".into(),
            per_share: 5.0,
        };
        assert_eq!(p.dividend_due("TCS.NS", &after, &splits), Some(200.0));
    }

    #[test]
    fn dividends_before_a_bonus_count_the_bonus_shares_of_sold_lots() {
        let splits = vec![bonus_1_for_1()];
        let before = DividendEvent {
            ex_date: "2025-06-01".into(),
            per_share: 5.0,
        };
        // All 100 older shares sold after the bonus: still 200 on 2025-06-01.
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        p.apply_split("TCS.NS", &splits[0]);
        p.sell_fifo("TCS.NS", 100.0, 600.0, "2026-07-01", "manual")
            .unwrap();
        assert_eq!(p.realized[0].bonuses, vec![splits[0].bonus_id()]);
        assert_eq!(p.dividend_due("TCS.NS", &before, &splits), Some(200.0));
        // A partial sale: 60 × 2 open plus 40 × 2 sold.
        let mut q = Portfolio::default();
        q.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        q.apply_split("TCS.NS", &splits[0]);
        q.sell_fifo("TCS.NS", 40.0, 600.0, "2026-07-01", "manual")
            .unwrap();
        assert_eq!(q.dividend_due("TCS.NS", &before, &splits), Some(200.0));
        // A bonus lot sold is not counted twice.
        q.sell_fifo("TCS.NS", 100.0, 600.0, "2026-08-01", "manual")
            .unwrap();
        assert_eq!(q.dividend_due("TCS.NS", &before, &splits), Some(200.0));
    }

    #[test]
    fn a_sale_on_the_allotment_date_does_not_net_against_bonus_shares() {
        let mut lots = vec![lot(100.0, 1000.0, "2023-05-01")];
        apply_split_to(&mut lots, &bonus_1_for_1());
        let (taken, left) = take_netting(&mut lots, 50.0, "2026-06-15");
        assert_eq!(left, 0.0);
        assert_eq!(taken.len(), 1);
        let (from, qty, intraday) = &taken[0];
        assert_eq!(
            (from.date.as_str(), *qty, *intraday),
            ("2023-05-01", 50.0, false)
        );

        // Selling into the bonus shares on that day: still not intraday.
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        p.apply_split("TCS.NS", &bonus_1_for_1());
        let sales = p
            .sell_fifo("TCS.NS", 150.0, 600.0, "2026-06-15", "manual")
            .unwrap();
        assert_eq!(sales.len(), 2);
        assert!(sales[1].bonus_shares);
        assert!(sales.iter().all(|s| !s.is_intraday()));
    }

    #[test]
    fn a_sale_backdated_before_a_bonus_counts_it_once() {
        let splits = vec![bonus_1_for_1()];
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", lot(100.0, 1000.0, "2023-05-01"));
        p.apply_split("TCS.NS", &splits[0]);
        p.sell_fifo("TCS.NS", 100.0, 600.0, "2026-06-01", "manual")
            .unwrap();
        let before = DividendEvent {
            ex_date: "2025-06-01".into(),
            per_share: 5.0,
        };
        assert_eq!(p.dividend_due("TCS.NS", &before, &splits), Some(200.0));
    }

    #[test]
    fn tradebook_replay_records_an_indian_bonus_as_a_zero_cost_lot() {
        let rows = vec![
            trade("TCS.NS", "2023-05-01", false, 100.0, 1000.0),
            trade("TCS.NS", "2026-07-01", true, 100.0, 600.0),
        ];
        let splits = vec![("TCS.NS".to_string(), vec![bonus_1_for_1()])];
        let tradebook = ImportOptions {
            splits: &splits,
            ..tradebook_opts("import:kite")
        };
        let mut p = Portfolio::default();
        let out = p.apply_import(&rows, tradebook);
        assert_eq!(
            out[0].1,
            ImportOutcome::Lots {
                lots: 1,
                sales: 1,
                splits: 1
            }
        );
        assert_eq!(p.realized[0].buy_date, "2023-05-01");
        assert_eq!(p.realized[0].gain(), -40_000.0);
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.lots.len(), 1);
        let l = &h.lots[0];
        assert_eq!(
            (l.qty, l.price, l.date.as_str()),
            (100.0, 0.0, "2026-06-15")
        );
        assert_eq!(
            (l.source.as_str(), l.account.as_deref()),
            (BONUS_SOURCE, Some("zerodha"))
        );
        // `actions --apply` has nothing left to do, and re-importing the
        // file keeps the bonus lot instead of refusing to replace it.
        assert_eq!(p.apply_split("TCS.NS", &bonus_1_for_1()), 0);
        let before = state(&p);
        let again = p.apply_import(&rows, tradebook);
        assert!(
            matches!(again[0].1, ImportOutcome::Lots { .. }),
            "{again:?}"
        );
        assert_eq!(state(&p), before);
    }

    #[test]
    fn correcting_an_estimated_date_does_not_reapply_earlier_splits() {
        // Upgraded lot recorded on 2025-06-01 with 20 shares — already the
        // post-bonus count. The real purchase was before the 2024 bonus.
        let mut p = Portfolio::default();
        let mut l = lot(20.0, 1400.0, "2025-06-01");
        l.date_estimated = true;
        p.add_lot("RELIANCE.NS", l);
        p.set_lot_date("RELIANCE.NS", 1, "2023-05-02").unwrap();
        let h = p.holding("RELIANCE.NS").unwrap();
        assert_eq!(h.lots[0].units_date(), "2025-06-01");
        assert_eq!(p.apply_split("RELIANCE.NS", &split_2_for_1()), 0);
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 20.0);
        // A real (non-estimated) date correction keeps splits applicable.
        let mut q = Portfolio::default();
        q.add_lot("RELIANCE.NS", lot(10.0, 2800.0, "2025-06-01"));
        q.set_lot_date("RELIANCE.NS", 1, "2023-05-02").unwrap();
        assert_eq!(q.apply_split("RELIANCE.NS", &split_2_for_1()), 1);
    }

    #[test]
    fn dividends_count_shares_held_at_ex_date_in_todays_share_count() {
        let mut p = Portfolio::default();
        p.add_lot("RELIANCE.NS", lot(10.0, 2800.0, "2023-05-02"));
        p.add_lot("RELIANCE.NS", lot(6.0, 2900.0, "2023-09-01"));
        p.sell_fifo("RELIANCE.NS", 6.0, 3000.0, "2024-01-10", "manual")
            .unwrap();
        let splits = vec![split_2_for_1()];
        // 10 shares were held on 2023-08-21 (6 of them sold later), which is
        // 20 in today's share count after the 2:1 bonus.
        let aug = DividendEvent {
            ex_date: "2023-08-21".into(),
            per_share: 4.5,
        };
        assert_eq!(p.dividend_due("RELIANCE.NS", &aug, &splits), Some(20.0));
        // After the bonus is applied the answer is the same.
        p.apply_split("RELIANCE.NS", &splits[0]);
        assert_eq!(p.dividend_due("RELIANCE.NS", &aug, &splits), Some(20.0));

        let d = p
            .record_dividend("RELIANCE.NS", &aug, &splits, Some("INR"))
            .unwrap();
        assert!((d.amount - 90.0).abs() < 1e-9);
        // Recorded once.
        assert!(
            p.record_dividend("RELIANCE.NS", &aug, &splits, None)
                .is_none()
        );

        // Nothing held before the first purchase.
        let early = DividendEvent {
            ex_date: "2022-08-01".into(),
            per_share: 4.0,
        };
        assert_eq!(p.dividend_due("RELIANCE.NS", &early, &splits), None);
    }

    #[test]
    fn shares_bought_on_the_ex_date_miss_the_dividend_and_sold_on_it_get_it() {
        let mut p = Portfolio::default();
        p.add_lot("X", lot(5.0, 10.0, "2024-01-01"));
        p.add_lot("X", lot(7.0, 10.0, "2024-03-01"));
        p.sell_fifo("X", 5.0, 11.0, "2024-03-01", "manual").unwrap();
        assert_eq!(p.shares_on("X", "2024-03-01", &[]), 5.0);
        assert_eq!(p.shares_on("X", "2024-03-02", &[]), 7.0);
    }

    // ── XIRR ──

    #[test]
    fn xirr_of_one_lot_up_ten_percent_in_a_year() {
        let mut p = Portfolio::default();
        p.add_lot("X", lot(10.0, 100.0, "2024-01-01"));
        let flows = p.cash_flows("X");
        let r = xirr(&flows, 1100.0, d("2024-12-31")).unwrap();
        assert!((r - 0.10).abs() < 1e-3, "{r}");
    }

    #[test]
    fn xirr_includes_sales_and_dividends() {
        let mut p = Portfolio::default();
        p.add_lot("X", lot(20.0, 100.0, "2023-01-01"));
        p.sell_fifo("X", 10.0, 110.0, "2024-01-01", "manual")
            .unwrap();
        p.dividends.push(DividendReceived {
            symbol: "X".into(),
            ex_date: "2024-01-01".into(),
            per_share: 1.0,
            qty: 10.0,
            amount: 10.0,
            currency: None,
        });
        let flows = p.cash_flows("X");
        // -2000 on day 0; +1100 sale and +10 dividend after a year.
        assert_eq!(flows.len(), 4);
        assert_eq!(
            flows.iter().map(|f| f.1).sum::<f64>(),
            -2000.0 + 1100.0 + 10.0
        );
        // Holding 10 worth 1100 on 2024-01-01 → 2000 grew to 2210: 10.5%.
        let r = xirr(&flows, 1100.0, d("2024-01-01")).unwrap();
        assert!((r - 0.105).abs() < 2e-3, "{r}");
    }

    #[test]
    fn xirr_needs_a_month_of_history() {
        let flows = vec![(d("2024-01-01"), -100.0)];
        assert!(xirr(&flows, 101.0, d("2024-01-15")).is_none());
        assert!(xirr(&[], 101.0, d("2024-01-15")).is_none());
    }

    // ── Currencies ──

    #[test]
    fn currency_from_symbol_suffix() {
        assert_eq!(symbol_currency("TCS.NS"), Some("INR"));
        assert_eq!(symbol_currency("RELIANCE.BO"), Some("INR"));
        assert_eq!(symbol_currency("AAPL"), Some("USD"));
        assert_eq!(symbol_currency("BTC-USD"), Some("USD"));
        assert_eq!(symbol_currency("VOD.L"), Some("GBp"));
        assert_eq!(symbol_currency("SAP.DE"), Some("EUR"));
        assert_eq!(symbol_currency("7203.T"), Some("JPY"));
        assert_eq!(symbol_currency("0005.HK"), Some("HKD"));
        assert_eq!(symbol_currency("BHP.AX"), Some("AUD"));
        assert_eq!(symbol_currency("MC.PA"), None); // Paris is not in the market table
        assert_eq!(symbol_currency("MF:118834"), Some("INR"));
    }

    #[test]
    fn fx_converts_into_the_base_and_refuses_unknown_currencies() {
        let mut fx = Fx::new("INR");
        fx.set_rate("USD", 84.0);
        assert_eq!(fx.convert(100.0, "INR"), Some(100.0));
        assert_eq!(fx.convert(10.0, "USD"), Some(840.0));
        assert_eq!(fx.convert(1.0, "EUR"), None);
        fx.set_rate("GBP", 110.0);
        assert!((fx.convert(250.0, "GBp").unwrap() - 275.0).abs() < 1e-9);
        fx.set_rate("JPY", f64::NAN);
        assert_eq!(fx.convert(1.0, "JPY"), None);
    }

    // ── Named portfolios ──

    #[test]
    fn portfolio_names_are_validated_and_mapped_to_files() {
        assert_eq!(normalize_name(None).unwrap(), None);
        assert_eq!(normalize_name(Some("default")).unwrap(), None);
        assert_eq!(
            normalize_name(Some("Long-Term")).unwrap(),
            Some("long-term".into())
        );
        for bad in ["../x", "a/b", "a.b", "has space", &"x".repeat(41)] {
            assert!(normalize_name(Some(bad)).is_err(), "{bad}");
        }
        assert_eq!(file_name(None), "portfolio.json");
        assert_eq!(file_name(Some("us")), "portfolios/us.json");
    }

    #[test]
    fn named_portfolio_saves_into_its_own_directory() {
        let dir = temp_dir("named");
        let path = dir.join(file_name(Some("us")));
        let mut p = Portfolio::default();
        p.add("AAPL", 1.0, 200.0);
        p.save_to(&path).unwrap();
        assert!(dir.join("portfolios/us.json").exists());
    }

    fn fill(order: &str, id: &str, sell: bool, qty: f64, price: f64, at: &str) -> BrokerFill {
        let (date, time) = at.split_once(' ').unwrap_or((at, ""));
        BrokerFill {
            broker: "zerodha".into(),
            order_id: order.into(),
            fill_id: id.into(),
            symbol: "INFY.NS".into(),
            sell,
            qty,
            price,
            date: date.into(),
            time: (!time.is_empty()).then(|| time.to_string()),
        }
    }

    #[test]
    fn broker_fills_become_dated_lots_once() {
        let mut p = Portfolio::default();
        let fills = vec![fill("O1", "T1", false, 10.0, 1500.0, "2026-10-20 09:30:00")];
        let s = p.apply_broker_fills(&fills);
        assert_eq!(s.orders.len(), 1);
        assert_eq!(s.orders[0].effect, FillEffect::Lot);
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!(h.shares, 10.0);
        let l = &h.lots[0];
        assert_eq!(
            (l.date.as_str(), l.price, l.source.as_str()),
            ("2026-10-20", 1500.0, "broker:zerodha")
        );
        assert_eq!(l.order_id.as_deref(), Some("O1"));
        assert!(!l.date_estimated);

        // A second run with the same trade book changes nothing.
        let again = p.apply_broker_fills(&fills);
        assert!(again.orders.is_empty());
        assert_eq!(again.already_synced, 1);
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 10.0);
        // The same trade id on another order (another day) is a new fill.
        let other = fill("O2", "T1", false, 2.0, 1510.0, "2026-10-21 09:30:00");
        assert_eq!(p.apply_broker_fills(&[other]).orders.len(), 1);
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 12.0);
        let again = p.apply_broker_fills(&fills);
        assert!(again.orders.is_empty());
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 12.0);
        assert_eq!(p.broker_fills.len(), 2);
    }

    #[test]
    fn partial_fills_merge_per_order_and_later_parts_add_on() {
        let mut p = Portfolio::default();
        let mut fills = vec![
            fill("O1", "T1", false, 4.0, 100.0, "2026-10-20 09:30:00"),
            fill("O1", "T2", false, 6.0, 110.0, "2026-10-20 09:31:00"),
        ];
        let s = p.apply_broker_fills(&fills);
        assert_eq!(s.orders.len(), 1);
        assert_eq!((s.orders[0].qty, s.orders[0].fills), (10.0, 2));
        assert!((s.orders[0].price - 106.0).abs() < 1e-9);
        assert_eq!(p.holding("INFY.NS").unwrap().lots.len(), 1);

        // The rest of the order fills later the same day.
        fills.push(fill("O1", "T3", false, 3.0, 120.0, "2026-10-20 10:05:00"));
        let s = p.apply_broker_fills(&fills);
        assert_eq!(s.already_synced, 2);
        assert_eq!(s.orders.len(), 1);
        assert_eq!((s.orders[0].qty, s.orders[0].price), (3.0, 120.0));
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!(h.shares, 13.0);
        assert!(h.lots.iter().all(|l| l.order_id.as_deref() == Some("O1")));
    }

    #[test]
    fn broker_sells_consume_lots_fifo_and_keep_the_order_id() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(10.0, 1000.0, "2024-01-10"));
        p.add_lot("INFY.NS", lot(5.0, 1200.0, "2025-06-01"));
        let fills = vec![fill("S9", "T9", true, 12.0, 1500.0, "2026-10-20 11:00:00")];
        let s = p.apply_broker_fills(&fills);
        let FillEffect::Sold { sales } = &s.orders[0].effect else {
            panic!("expected a sale: {:?}", s.orders[0].effect);
        };
        assert_eq!(sales.len(), 2);
        assert_eq!((sales[0].qty, sales[0].buy_price), (10.0, 1000.0));
        assert_eq!((sales[1].qty, sales[1].buy_price), (2.0, 1200.0));
        assert!(
            p.realized
                .iter()
                .all(|s| s.order_id.as_deref() == Some("S9") && s.source == "broker:zerodha")
        );
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!((h.shares, h.lots[0].price), (3.0, 1200.0));

        assert!(p.apply_broker_fills(&fills).orders.is_empty());
        assert_eq!(p.realized.len(), 2);
    }

    #[test]
    fn same_day_fills_apply_in_time_order() {
        let mut p = Portfolio::default();
        // Listed sell first, but the buy came earlier in the day.
        let fills = vec![
            fill("S1", "T2", true, 5.0, 1510.0, "2026-10-20 10:00:00"),
            fill("B1", "T1", false, 5.0, 1500.0, "2026-10-20 09:30:00"),
        ];
        let s = p.apply_broker_fills(&fills);
        assert_eq!(s.orders[0].side, "BUY");
        assert!(matches!(s.orders[1].effect, FillEffect::Sold { .. }));
        assert!(p.holding("INFY.NS").is_none());
        assert_eq!(p.realized.len(), 1);
        assert!((p.realized[0].gain() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn a_sell_without_enough_shares_is_left_unsynced() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(3.0, 1000.0, "2024-01-10"));
        let fills = vec![fill("S1", "T1", true, 5.0, 1500.0, "2026-10-20")];
        let s = p.apply_broker_fills(&fills);
        assert_eq!(s.orders[0].effect, FillEffect::Unmatched { held: 3.0 });
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 3.0);
        assert!(p.realized.is_empty() && p.broker_fills.is_empty());

        // Once the missing lot is recorded, the same sell applies.
        p.add_lot("INFY.NS", lot(2.0, 1100.0, "2025-01-10"));
        let s = p.apply_broker_fills(&fills);
        assert!(matches!(s.orders[0].effect, FillEffect::Sold { .. }));
        assert!(p.holding("INFY.NS").is_none());
    }

    #[test]
    fn order_fill_json_names_its_effect() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", lot(1.0, 1000.0, "2024-01-10"));
        let s = p.apply_broker_fills(&[
            fill("B1", "T1", false, 1.0, 10.0, "2026-10-20 09:00:00"),
            fill("S1", "T2", true, 1.0, 1500.0, "2026-10-20 10:00:00"),
            fill("S2", "T3", true, 9.0, 1500.0, "2026-10-20 11:00:00"),
        ]);
        let v = serde_json::to_value(&s.orders).unwrap();
        assert_eq!(v[0]["result"], "lot");
        assert_eq!(v[1]["result"], "sold");
        assert_eq!(v[1]["sales"][0]["order_id"], "S1");
        assert_eq!(v[2]["result"], "unmatched");
        assert_eq!(v[2]["held"], 1.0);
        assert_eq!(v[0]["side"], "BUY");
    }

    #[test]
    fn synced_fills_round_trip_and_old_files_still_load() {
        let mut p = Portfolio::default();
        p.apply_broker_fills(&[fill("O1", "T1", false, 1.0, 10.0, "2026-10-20")]);
        let json = serde_json::to_string(&p).unwrap();
        let back: Portfolio = serde_json::from_str(&json).unwrap();
        assert_eq!(back.broker_fills, p.broker_fills);
        assert_eq!(back.holdings[0].lots[0].order_id.as_deref(), Some("O1"));
        let old: Portfolio = serde_json::from_str(
            r#"{"version":2,"holdings":[{"symbol":"TCS.NS","shares":1,"avg_cost":5,"lots":[{"qty":1,"price":5,"date":"2024-01-01","source":"manual"}]}]}"#,
        )
        .unwrap();
        assert!(old.broker_fills.is_empty());
        assert_eq!(old.holdings[0].lots[0].order_id, None);
        // Manual lots don't grow an order_id key on save.
        assert!(!serde_json::to_string(&old).unwrap().contains("order_id"));
    }

    #[test]
    fn targets_and_drift_tolerance_methods_and_serialization() {
        let mut p = Portfolio::default();
        p.set_target("TCS.NS", 25.0);
        p.set_target("Technology", 35.0);
        p.set_drift_tolerance(Some(2.5));

        assert_eq!(p.targets.len(), 2);
        assert_eq!(p.targets.get("TCS.NS"), Some(&25.0));
        assert_eq!(p.targets.get("Technology"), Some(&35.0));
        assert_eq!(p.drift_tolerance, Some(2.5));

        // Serialize and deserialize round-trip
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("targets"));
        assert!(json.contains("drift_tolerance"));

        let mut back: Portfolio = serde_json::from_str(&json).unwrap();
        assert_eq!(back.targets, p.targets);
        assert_eq!(back.drift_tolerance, Some(2.5));

        // Case-insensitive removal
        assert!(back.remove_target("tcs.ns"));
        assert_eq!(back.targets.len(), 1);
        assert!(!back.remove_target("UNKNOWN"));

        // Clear targets
        back.clear_targets();
        assert!(back.targets.is_empty());

        // Old JSON without targets loads with empty targets and None drift
        let old: Portfolio = serde_json::from_str(r#"{"version":2,"holdings":[]}"#).unwrap();
        assert!(old.targets.is_empty());
        assert_eq!(old.drift_tolerance, None);

        // When empty, targets and drift_tolerance are omitted from JSON
        let empty_json = serde_json::to_string(&old).unwrap();
        assert!(!empty_json.contains("targets"));
        assert!(!empty_json.contains("drift_tolerance"));
    }

    #[test]
    fn edit_lot_changes_qty_and_price_and_refreshes_the_average() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", Lot::new(10.0, 3000.0, "2024-01-01", "manual"));
        p.add_lot("TCS.NS", Lot::new(5.0, 3600.0, "2024-06-01", "manual"));
        p.edit_lot("TCS.NS", 2, Some(10.0), Some(3300.0)).unwrap();
        let h = p.holding("TCS.NS").unwrap();
        assert_eq!(h.shares, 20.0);
        assert!((h.avg_cost - 3150.0).abs() < 1e-9);
        assert!(p.edit_lot("TCS.NS", 1, Some(0.0), None).is_err());
        assert!(p.edit_lot("TCS.NS", 1, None, Some(-1.0)).is_err());
        assert!(p.edit_lot("TCS.NS", 3, Some(1.0), None).is_err());
    }

    #[test]
    fn delete_lot_drops_one_lot_and_the_holding_with_the_last() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", Lot::new(10.0, 1500.0, "2024-01-01", "manual"));
        p.add_lot("INFY.NS", Lot::new(10.0, 1500.0, "2024-01-01", "manual"));
        assert_eq!(p.holding("INFY.NS").unwrap().lots.len(), 2);
        p.delete_lot("INFY.NS", 1).unwrap();
        assert_eq!(p.holding("INFY.NS").unwrap().shares, 10.0);
        assert!(p.delete_lot("INFY.NS", 2).is_err());
        p.delete_lot("INFY.NS", 1).unwrap();
        assert!(p.holding("INFY.NS").is_none());
        assert!(p.realized.is_empty(), "deleting is not a sale");
    }

    #[test]
    fn undo_sale_restores_the_exact_lot() {
        let mut p = Portfolio::default();
        let mut lot = Lot::new(100.0, 1000.0, "2023-05-01", "import:kite");
        lot.adjustments.push("2024-01-01:2:1".into());
        lot.order_id = Some("A1".into());
        p.add_lot("RELIANCE.NS", lot.clone());
        let before = p.holding("RELIANCE.NS").unwrap().lots.clone();

        p.sell_fifo("RELIANCE.NS", 100.0, 1200.0, "2025-01-10", "manual")
            .unwrap();
        assert!(p.holding("RELIANCE.NS").is_none());
        let sale = p.undo_sale("RELIANCE.NS", 1).unwrap();
        assert_eq!(sale.qty, 100.0);
        assert!(p.realized.is_empty());
        assert_eq!(p.holding("RELIANCE.NS").unwrap().lots, before);
        // The restored lot keeps its split record, so it is not split again.
        let split = Split {
            date: "2024-01-01".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        };
        assert_eq!(p.apply_split("RELIANCE.NS", &split), 0);
    }

    #[test]
    fn undo_sale_merges_into_the_rest_of_its_lot() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", Lot::new(100.0, 400.0, "2023-01-01", "manual"));
        p.add_lot("ITC.NS", Lot::new(50.0, 450.0, "2024-01-01", "manual"));
        p.sell_fifo("ITC.NS", 30.0, 480.0, "2025-02-01", "manual")
            .unwrap();
        p.sell_fifo("ITC.NS", 10.0, 490.0, "2025-03-01", "manual")
            .unwrap();
        assert_eq!(p.sales_of("ITC.NS").len(), 2);
        // Undo the first sale only; the second stays.
        p.undo_sale("itc.ns", 1).unwrap();
        let h = p.holding("ITC.NS").unwrap();
        assert_eq!(h.lots.len(), 2);
        assert_eq!(h.lots[0].qty, 90.0);
        assert_eq!(p.sales_of("ITC.NS").len(), 1);
        assert_eq!(p.realized[0].sell_date, "2025-03-01");
        assert!(p.undo_sale("ITC.NS", 2).is_err());
    }

    #[test]
    fn undoing_an_older_sale_only_takes_splits_after_it() {
        let mut p = Portfolio::default();
        let mut old = Sale::from_lot(
            "TCS.NS",
            &Lot::new(10.0, 3000.0, "2020-01-01", "manual"),
            10.0,
            "2023-01-01",
            3500.0,
            "manual",
        );
        old.origin = None;
        p.realized.push(old);
        p.undo_sale("TCS.NS", 1).unwrap();
        let lot = &p.holding("TCS.NS").unwrap().lots[0];
        assert_eq!(lot.units_as_of.as_deref(), Some("2023-01-01"));
        let before = Split {
            date: "2022-01-01".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        };
        assert_eq!(p.apply_split("TCS.NS", &before), 0);
    }

    #[test]
    fn saves_keep_five_backups_and_restore_brings_one_back() {
        let dir = temp_dir("backups");
        let path = dir.join("portfolio.json");
        let mut p = Portfolio::default();
        for i in 1..=7 {
            p.add_lot(
                "TCS.NS",
                Lot::new(1.0, 100.0 * i as f64, "2024-01-01", "manual"),
            );
            p.save_to(&path).unwrap();
        }
        // 7 saves: the first wrote the file, the next 6 each kept one; only 5 stay.
        assert_eq!(backups_of(&path).len(), BACKUPS_KEPT);
        assert!(!numbered_backup(&path, 6).exists());
        let one = Portfolio::load_from(&numbered_backup(&path, 1)).unwrap();
        assert_eq!(one.holding("TCS.NS").unwrap().lots.len(), 6);

        // A save that changes nothing rotates nothing.
        let newest = std::fs::read(numbered_backup(&path, 1)).unwrap();
        p.save_to(&path).unwrap();
        assert_eq!(std::fs::read(numbered_backup(&path, 1)).unwrap(), newest);

        let restored = Portfolio::restore_backup(&path, 1).unwrap();
        assert_eq!(restored.holding("TCS.NS").unwrap().lots.len(), 6);
        let now = Portfolio::load_from(&path).unwrap();
        assert_eq!(now.holding("TCS.NS").unwrap().lots.len(), 6);
        // The replaced version became backup 1, so the restore can be undone.
        let undo = Portfolio::load_from(&numbered_backup(&path, 1)).unwrap();
        assert_eq!(undo.holding("TCS.NS").unwrap().lots.len(), 7);
        assert!(Portfolio::restore_backup(&path, 6).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_sale_catches_up_a_split_after_the_sale() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", Lot::new(10.0, 1000.0, "2023-01-01", "manual"));
        p.sell_fifo("INFY.NS", 4.0, 1200.0, "2024-01-01", "manual")
            .unwrap();
        let split = Split {
            date: "2024-06-01".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        };
        assert_eq!(p.apply_split("INFY.NS", &split), 1);
        p.undo_sale("INFY.NS", 1).unwrap();
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!(h.shares, 20.0);
        assert!((h.cost_basis() - 10_000.0).abs() < 1e-6);
        assert_eq!(p.apply_split("INFY.NS", &split), 0);
    }

    #[test]
    fn undoing_a_legacy_sale_keeps_the_account_its_source_names() {
        let mut p = Portfolio::default();
        let mut sale = Sale::from_lot(
            "TCS.NS",
            &Lot::new(5.0, 3000.0, "2022-01-01", "import:kite"),
            5.0,
            "2023-01-01",
            3500.0,
            "import:kite",
        );
        sale.origin = None;
        sale.account = None;
        p.realized.push(sale);
        p.undo_sale("TCS.NS", 1).unwrap();
        let lot = &p.holding("TCS.NS").unwrap().lots[0];
        assert_eq!(lot.account_key(), Some("zerodha"));
    }

    #[test]
    fn a_failed_backup_copy_leaves_the_history_alone() {
        let dir = temp_dir("backup-fail");
        let path = dir.join("portfolio.json");
        std::fs::write(numbered_backup(&path, 1), "one").unwrap();
        // No portfolio file: the copy fails before anything is renamed.
        assert!(rotate_backups(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(numbered_backup(&path, 1)).unwrap(),
            "one"
        );
        assert!(!numbered_backup(&path, 2).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_rotation_retried_after_a_partial_failure_loses_no_version() {
        let dir = temp_dir("backup-partial");
        let path = dir.join("portfolio.json");
        std::fs::write(&path, "now").unwrap();
        // A rotation that moved .4 to .5 and then failed: .4 is missing.
        for (n, body) in [(1, "v1"), (2, "v2"), (3, "v3"), (5, "v4")] {
            std::fs::write(numbered_backup(&path, n), body).unwrap();
        }
        rotate_backups(&path).unwrap();
        let read = |n| std::fs::read_to_string(numbered_backup(&path, n)).unwrap();
        assert_eq!(
            [read(1), read(2), read(3), read(4), read(5)],
            ["now", "v1", "v2", "v3", "v4"]
        );
        // With every slot full, the oldest goes.
        std::fs::write(&path, "next").unwrap();
        rotate_backups(&path).unwrap();
        assert_eq!(read(1), "next");
        assert_eq!(read(5), "v3");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reading_a_backup_never_writes_it() {
        let dir = temp_dir("backup-readonly");
        let path = dir.join("portfolio.json.1");
        // An old-format file that `load_from` would upgrade in place.
        let old = r#"{"holdings":[{"symbol":"TCS.NS","shares":5.0,"avg_cost":3000.0}]}"#;
        std::fs::write(&path, old).unwrap();
        let p = Portfolio::read_only(&path).unwrap();
        assert_eq!(p.holding("TCS.NS").unwrap().shares, 5.0);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), old);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn undo_sale_finds_a_split_on_a_later_sale_when_the_lot_is_gone() {
        let mut p = Portfolio::default();
        p.add_lot("INFY.NS", Lot::new(10.0, 1000.0, "2023-01-01", "manual"));
        p.sell_fifo("INFY.NS", 4.0, 1200.0, "2024-01-01", "manual")
            .unwrap();
        let split = Split {
            date: "2024-06-01".into(),
            numerator: 2.0,
            denominator: 1.0,
            bonus: None,
        };
        p.apply_split("INFY.NS", &split);
        p.sell_fifo("INFY.NS", 12.0, 700.0, "2024-08-01", "manual")
            .unwrap();
        p.add_lot("INFY.NS", Lot::new(5.0, 800.0, "2024-09-01", "manual"));
        p.undo_sale("INFY.NS", 1).unwrap();
        let h = p.holding("INFY.NS").unwrap();
        assert_eq!(h.shares, 13.0);
        let restored = h.lots.iter().find(|l| l.date == "2023-01-01").unwrap();
        assert_eq!(restored.qty, 8.0);
        assert!((restored.price - 500.0).abs() < 1e-9);
    }

    #[test]
    fn a_save_retried_after_a_failed_write_does_not_rotate_again() {
        let dir = temp_dir("backup-retry");
        let path = dir.join("portfolio.json");
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", Lot::new(1.0, 100.0, "2024-01-01", "manual"));
        p.save_to(&path).unwrap();
        for n in 2..=BACKUPS_KEPT {
            std::fs::write(numbered_backup(&path, n), format!("v{n}")).unwrap();
        }
        // The state a save leaves when rotation succeeded and the write failed:
        // backup 1 is a copy of the unchanged current file.
        std::fs::copy(&path, numbered_backup(&path, 1)).unwrap();
        p.add_lot("INFY.NS", Lot::new(1.0, 100.0, "2024-01-01", "manual"));
        p.save_to(&path).unwrap();
        for n in 2..=BACKUPS_KEPT {
            let body = std::fs::read_to_string(numbered_backup(&path, n)).unwrap();
            assert_eq!(body, format!("v{n}"), "backup {n} moved");
        }
        assert!(
            Portfolio::read_only(&path)
                .unwrap()
                .holding("INFY.NS")
                .is_some()
        );
        let one = Portfolio::read_only(&numbered_backup(&path, 1)).unwrap();
        assert!(one.holding("INFY.NS").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A portfolio file plus backups `.1..=.5` holding `v1`..`v5`.
    fn full_history(name: &str) -> (PathBuf, PathBuf, Portfolio) {
        let dir = temp_dir(name);
        let path = dir.join("portfolio.json");
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", Lot::new(1.0, 100.0, "2024-01-01", "manual"));
        crate::store::save_to(&path, &p, false).unwrap();
        for n in 1..=BACKUPS_KEPT {
            std::fs::write(numbered_backup(&path, n), format!("v{n}")).unwrap();
        }
        (dir, path, p)
    }

    fn history(path: &Path) -> Vec<String> {
        (1..=BACKUPS_KEPT)
            .map(|n| std::fs::read_to_string(numbered_backup(path, n)).unwrap_or_default())
            .collect()
    }

    #[test]
    fn a_failed_portfolio_write_then_retry_drops_one_version_only() {
        let (dir, path, mut p) = full_history("inject-write");
        let current = std::fs::read_to_string(&path).unwrap();
        p.add_lot("INFY.NS", Lot::new(1.0, 100.0, "2024-01-01", "manual"));
        let failed = p.save_with(
            &path,
            |a: &Path, b: &Path| std::fs::rename(a, b),
            |_: &Path| anyhow::bail!("disk full"),
        );
        assert!(failed.is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        let after_failure = history(&path);
        assert_eq!(
            after_failure,
            [
                current.clone(),
                "v1".into(),
                "v2".into(),
                "v3".into(),
                "v4".into()
            ]
        );
        // Retry for real: the history stays as the failed save left it.
        p.save_to(&path).unwrap();
        assert_eq!(history(&path), after_failure);
        assert!(
            Portfolio::read_only(&path)
                .unwrap()
                .holding("INFY.NS")
                .is_some()
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_rename_then_retry_drops_one_version_only() {
        let (dir, path, mut p) = full_history("inject-rename");
        let current = std::fs::read_to_string(&path).unwrap();
        p.add_lot("INFY.NS", Lot::new(1.0, 100.0, "2024-01-01", "manual"));
        // Fail the third rename (.2 -> .3), after .4 -> .5 and .3 -> .4 moved.
        let calls = std::cell::Cell::new(0);
        let flaky = |from: &Path, to: &Path| -> std::io::Result<()> {
            calls.set(calls.get() + 1);
            if calls.get() == 3 {
                Err(std::io::Error::other("injected"))
            } else {
                std::fs::rename(from, to)
            }
        };
        assert!(
            p.save_with(&path, flaky, |_: &Path| unreachable!())
                .is_err()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), current);
        p.save_to(&path).unwrap();
        assert_eq!(
            history(&path),
            [current, "v1".into(), "v2".into(), "v3".into(), "v4".into()]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn demerger(child: &str) -> &'static Demerger {
        DEMERGERS.iter().find(|d| d.child == child).unwrap()
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    #[test]
    fn every_demerger_row_cites_a_source_and_has_sane_figures() {
        for d in DEMERGERS {
            assert!(d.source.starts_with("https://"), "{d:?}");
            assert!(date_of(d.record_date).is_some(), "{d:?}");
            assert!(d.child_cost_pct > 0.0 && d.child_cost_pct < 100.0, "{d:?}");
            assert!(d.child_shares > 0.0 && d.parent_shares > 0.0, "{d:?}");
            assert!(!d.parents.is_empty(), "{d:?}");
        }
        assert_eq!(demergers_for("reliance.ns"), vec![demerger("JIOFIN")]);
        assert_eq!(demergers_for("TATAMOTORS.NS"), vec![demerger("TMCV")]);
        assert_eq!(demergers_for("TMPV.BO"), vec![demerger("TMCV")]);
        assert!(demergers_for("ITCHOTELS.NS").is_empty());
        assert_eq!(demerger("TMCV").child_symbol("TATAMOTORS.NS"), "TMCV.NS");
    }

    #[test]
    fn a_lot_bought_before_the_record_date_becomes_two_lots_with_split_cost_and_its_date() {
        let mut p = Portfolio::default();
        let mut old = Lot::new(10.0, 2500.0, "2022-05-02", "import:kite");
        old.charges = Some(20.0);
        p.add_lot("RELIANCE.NS", old);
        // Bought on the record date (= ex-date under T+1): not entitled.
        p.add_lot("RELIANCE.NS", Lot::new(5.0, 2600.0, "2023-07-20", "manual"));
        assert_eq!(p.pending_demergers().len(), 1);

        let out = p.apply_demerger("RELIANCE.NS", demerger("JIOFIN"));
        assert_eq!(out.lots, 1);
        assert!(p.pending_demergers().is_empty());

        let ril = p.holding("RELIANCE.NS").unwrap();
        assert_eq!(ril.shares, 15.0);
        assert_eq!(ril.lots[0].qty, 10.0);
        assert_eq!(ril.lots[0].date, "2022-05-02");
        assert!(close(ril.lots[0].cost(), 25_000.0 * 0.9532));
        // Charges split to the paisa: 0.94 to the child, the rest stays.
        assert!(close(ril.lots[0].charges.unwrap(), 20.0 - 0.94));
        assert_eq!(ril.lots[1].price, 2600.0, "the later lot is untouched");

        let jio = p.holding("JIOFIN.NS").unwrap();
        assert_eq!(jio.lots.len(), 1);
        let lot = &jio.lots[0];
        assert_eq!(lot.qty, 10.0);
        assert_eq!(lot.date, "2022-05-02", "keeps the parent's purchase date");
        assert_eq!(lot.source, DEMERGER_SOURCE);
        assert_eq!(lot.account_key(), Some("zerodha"));
        assert!(close(lot.cost(), 25_000.0 * 0.0468));
        assert!(close(lot.charges.unwrap(), 0.94));
        // The cost is only moved, never lost.
        assert!(close(ril.lots[0].cost() + lot.cost(), 25_000.0));
    }

    #[test]
    fn applying_a_demerger_twice_changes_nothing() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", Lot::new(1000.0, 400.0, "2020-03-02", "manual"));
        p.add_lot("ITC.NS", Lot::new(500.0, 450.0, "2024-08-01", "manual"));
        let d = demerger("ITCHOTELS");
        assert_eq!(p.apply_demerger("ITC.NS", d).lots, 2);
        let once = serde_json::to_string(&p).unwrap();
        assert_eq!(p.apply_demerger("ITC.NS", d), DemergerOutcome::default());
        assert_eq!(serde_json::to_string(&p).unwrap(), once);
        // ITC's own worked example: 1,000 shares at 400 → 100 ITCHL at
        // 54,040 and 1,000 ITC at 3,45,960.
        let hotels = p.holding("ITCHOTELS.NS").unwrap();
        assert_eq!(hotels.shares, 150.0);
        assert!(close(hotels.lots[0].qty, 100.0));
        assert!(close(hotels.lots[0].cost(), 54_040.0));
        assert!(close(
            p.holding("ITC.NS").unwrap().lots[0].cost(),
            345_960.0
        ));
    }

    #[test]
    fn fractional_entitlements_are_rounded_down_per_account() {
        let mut p = Portfolio::default();
        let mut a = Lot::new(9.0, 400.0, "2023-01-02", "manual");
        a.account = Some("zerodha".into());
        let mut b = Lot::new(6.0, 420.0, "2023-06-02", "manual");
        b.account = Some("zerodha".into());
        let mut c = Lot::new(7.0, 410.0, "2023-06-02", "manual");
        c.account = Some("groww".into());
        for l in [a, b, c] {
            p.add_lot("ITC.NS", l);
        }
        let out = p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        assert_eq!(out.lots, 3);
        // zerodha: 15 ITC → 1.5 → 1 share; groww: 7 ITC → 0.7 → none.
        assert!(close(out.fraction_dropped, 0.5 + 0.7));
        let hotels = p.holding("ITCHOTELS.NS").unwrap();
        assert!(close(hotels.shares, 1.0));
        assert!(
            hotels
                .lots
                .iter()
                .all(|l| l.account.as_deref() == Some("zerodha"))
        );
        // The groww lot got no shares, so it keeps its whole cost.
        let itc = p.holding("ITC.NS").unwrap();
        let groww = itc
            .lots
            .iter()
            .find(|l| l.account.as_deref() == Some("groww"))
            .unwrap();
        assert_eq!(groww.price, 410.0);
        assert!(groww.adjustments.contains(&demerger("ITCHOTELS").id()));
    }

    #[test]
    fn a_split_after_the_record_date_does_not_inflate_the_new_shares() {
        let mut p = Portfolio::default();
        p.add_lot(
            "RELIANCE.NS",
            Lot::new(10.0, 2400.0, "2022-01-03", "manual"),
        );
        // RIL's 1:1 bonus of Oct 2024, applied as a split (NSE offline).
        p.apply_split("RELIANCE.NS", &Split::new("2024-10-28", 2.0, 1.0));
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 20.0);
        p.apply_demerger("RELIANCE.NS", demerger("JIOFIN"));
        assert_eq!(p.holding("JIOFIN.NS").unwrap().shares, 10.0);
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 20.0);
    }

    #[test]
    fn tata_motors_under_its_old_ticker_gets_tmcv() {
        let mut p = Portfolio::default();
        p.add_lot(
            "TATAMOTORS.NS",
            Lot::new(1000.0, 400.0, "2021-02-01", "manual"),
        );
        p.add_lot(
            "TATAMOTORS.NS",
            Lot::new(10.0, 700.0, "2025-10-14", "manual"),
        );
        let pending = p.pending_demergers();
        assert_eq!(
            pending,
            vec![("TATAMOTORS.NS".to_string(), demerger("TMCV"))]
        );
        p.apply_demerger("TATAMOTORS.NS", demerger("TMCV"));
        // TMPV's worked example: 1,24,600 to TMCV, 2,75,400 left.
        let tmcv = p.holding("TMCV.NS").unwrap();
        assert_eq!(tmcv.shares, 1000.0);
        assert!(close(tmcv.cost_basis(), 124_600.0));
        assert_eq!(tmcv.lots[0].date, "2021-02-01");
        let tm = p.holding("TATAMOTORS.NS").unwrap();
        assert!(close(tm.lots[0].cost(), 275_400.0));
        assert_eq!(tm.lots[1].price, 700.0);
    }

    #[test]
    fn an_old_portfolio_file_with_a_pre_demerger_lot_still_loads_and_applies() {
        // Written before demergers existed: no adjustments on the lot.
        let json = r#"{
            "version": 2,
            "holdings": [{"symbol": "RELIANCE.NS", "shares": 10.0, "avg_cost": 2500.0,
                "added_at": "2022-05-02",
                "lots": [{"qty": 10.0, "price": 2500.0, "date": "2022-05-02",
                    "source": "manual", "adjustments": ["2017-09-07:2:1"]}]}]
        }"#;
        let path = temp_dir("old-demerger-file").join("portfolio.json");
        std::fs::write(&path, json).unwrap();
        let mut p = Portfolio::read_only(&path).unwrap();
        assert_eq!(p.holding("RELIANCE.NS").unwrap().shares, 10.0);
        assert_eq!(p.pending_demergers().len(), 1);
        p.apply_demerger("RELIANCE.NS", demerger("JIOFIN"));
        assert_eq!(p.holding("JIOFIN.NS").unwrap().shares, 10.0);
        // Saved and read back, the demerger stays applied.
        let back: Portfolio = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert!(back.pending_demergers().is_empty());
        assert_eq!(
            back.holding("JIOFIN.NS").unwrap().lots[0].date,
            "2022-05-02"
        );
    }

    fn snapshot_lot(qty: f64, account: &str) -> Lot {
        // A broker holdings snapshot: no purchase date, no order behind it.
        let mut l = Lot::new(qty, 300.0, "2025-03-01", "broker:zerodha");
        l.date_estimated = true;
        l.account = Some(account.into());
        l
    }

    fn itc_lot(qty: f64, date: &str, account: &str) -> Lot {
        let mut l = Lot::new(qty, 400.0, date, "manual");
        l.account = Some(account.into());
        l
    }

    #[test]
    fn a_split_before_the_record_date_counts_in_the_entitlement() {
        // 10 RIL bought in 2016, the 2017 1:1 bonus still pending: on the
        // 2023 record date the lot was 20 shares, so 20 JIOFIN.
        let mut p = Portfolio::default();
        p.add_lot(
            "RELIANCE.NS",
            Lot::new(10.0, 1000.0, "2016-05-02", "manual"),
        );
        let bonus_2017 = Split::new("2017-09-07", 2.0, 1.0);
        let mut applied = p.clone();
        applied.apply_in_order(
            "RELIANCE.NS",
            std::slice::from_ref(&bonus_2017),
            &[demerger("JIOFIN")],
        );
        assert_eq!(applied.holding("JIOFIN.NS").unwrap().shares, 20.0);
        assert_eq!(applied.holding("RELIANCE.NS").unwrap().shares, 20.0);
        // The read path's copy agrees.
        let pending = crate::corp_actions::PendingSplits {
            by_symbol: vec![("RELIANCE.NS".into(), vec![bonus_2017])],
            demergers: p.pending_demergers(),
            ..Default::default()
        };
        let adj = pending.adjusted(&p);
        assert_eq!(adj.holding("JIOFIN.NS").unwrap().shares, 20.0);
    }

    #[test]
    fn child_shares_from_a_broker_snapshot_are_taken_not_added_twice() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", itc_lot(1000.0, "2020-03-02", "zerodha"));
        p.add_lot("ITCHOTELS.NS", snapshot_lot(130.0, "zerodha"));
        let out = p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        assert_eq!(out.reconciled, 100.0);
        assert!(out.conflicts.is_empty());
        let hotels = p.holding("ITCHOTELS.NS").unwrap();
        // 100 from the demerger (2020 date, its cost) + 30 snapshot left.
        assert_eq!(hotels.shares, 130.0);
        let ours = hotels
            .lots
            .iter()
            .find(|l| l.source == DEMERGER_SOURCE)
            .unwrap();
        assert_eq!((ours.qty, ours.date.as_str()), (100.0, "2020-03-02"));
        assert!(close(ours.cost(), 54_040.0));
    }

    #[test]
    fn too_few_unexplained_child_shares_is_a_conflict_and_changes_nothing() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", itc_lot(1000.0, "2020-03-02", "zerodha"));
        p.add_lot("ITCHOTELS.NS", snapshot_lot(60.0, "zerodha"));
        let before = serde_json::to_string(&p).unwrap();
        let out = p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        assert_eq!(out.conflicts.len(), 1, "{out:?}");
        assert!(out.conflicts[0].contains("zerodha"), "{}", out.conflicts[0]);
        assert_eq!(serde_json::to_string(&p).unwrap(), before);
        // Still pending, and the read path says why instead of adding 100.
        let pending = crate::corp_actions::PendingSplits::offline(&p);
        assert_eq!(
            pending.adjusted(&p).holding("ITCHOTELS.NS").unwrap().shares,
            60.0
        );
        assert!(
            pending
                .warnings()
                .iter()
                .any(|w| w.contains("left unchanged"))
        );
    }

    #[test]
    fn a_later_market_purchase_of_the_child_is_not_taken_as_the_allotment() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", itc_lot(1000.0, "2020-03-02", "zerodha"));
        let mut bought = Lot::new(50.0, 450.0, "2025-02-10", "broker:zerodha");
        bought.order_id = Some("ord-1".into());
        p.add_lot("ITCHOTELS.NS", bought);
        p.add_lot("ITCHOTELS.NS", Lot::new(5.0, 460.0, "2025-02-12", "manual"));
        let out = p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        assert_eq!(out.reconciled, 0.0);
        assert_eq!(p.holding("ITCHOTELS.NS").unwrap().shares, 155.0);
    }

    #[test]
    fn demerged_shares_are_not_held_or_sellable_before_the_record_date() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", itc_lot(1000.0, "2020-03-02", "zerodha"));
        p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        let lot = &p.holding("ITCHOTELS.NS").unwrap().lots[0];
        assert_eq!(lot.date, "2020-03-02", "the tax date stays the parent's");
        assert_eq!(lot.held_from(), "2025-01-06");
        assert_eq!(p.shares_on("ITCHOTELS.NS", "2024-12-31", &[]), 0.0);
        assert_eq!(p.shares_on("ITCHOTELS.NS", "2025-01-07", &[]), 100.0);
        assert!(
            p.sell_fifo("ITCHOTELS.NS", 10.0, 500.0, "2024-12-20", "manual")
                .is_err()
        );
        let sale = p
            .sell_fifo("ITCHOTELS.NS", 10.0, 500.0, "2025-02-03", "manual")
            .unwrap();
        assert_eq!(sale[0].buy_date, "2020-03-02");
        assert_eq!(sale[0].held_from(), "2025-01-06");
        // Sold after 7 Jan: not held on 2024-12-31, held on 2025-01-07.
        assert_eq!(p.shares_on("ITCHOTELS.NS", "2024-12-31", &[]), 0.0);
        assert_eq!(p.shares_on("ITCHOTELS.NS", "2025-01-07", &[]), 100.0);
    }

    #[test]
    fn an_unchanged_tradebook_re_import_keeps_the_demerger() {
        let rows = vec![
            trade("ITC.NS", "2020-03-02", false, 1000.0, 400.0),
            trade("ITC.NS", "2025-03-03", true, 200.0, 420.0),
        ];
        let mut p = Portfolio::default();
        p.apply_import(&rows, tradebook_opts("import:kite"));
        // The replay applied the demerger as it happened: the sale after the
        // record date carries the reduced cost, and the child is there.
        assert!(close(p.realized[0].buy_price, 400.0 * 0.8649));
        assert_eq!(p.holding("ITCHOTELS.NS").unwrap().shares, 100.0);
        assert!(p.pending_demergers().is_empty());
        let before = state(&p);
        p.apply_import(&rows, tradebook_opts("import:kite"));
        assert_eq!(state(&p), before);
        // And after the child was partly sold, still unchanged.
        p.sell_fifo("ITCHOTELS.NS", 40.0, 500.0, "2025-04-01", "manual")
            .unwrap();
        let before = state(&p);
        p.apply_import(&rows, tradebook_opts("import:kite"));
        assert_eq!(state(&p), before);
        assert_eq!(p.holding("ITCHOTELS.NS").unwrap().shares, 60.0);
        assert_eq!(
            p.apply_demerger("ITC.NS", demerger("ITCHOTELS")),
            DemergerOutcome::default()
        );
    }

    #[test]
    fn the_entitlement_is_rounded_once_per_account_across_listings() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", itc_lot(6.0, "2023-01-02", "zerodha"));
        p.add_lot("ITC.BO", itc_lot(4.0, "2023-02-02", "zerodha"));
        let out = p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        assert_eq!(out.lots, 2);
        let total: f64 = ["ITCHOTELS.NS", "ITCHOTELS.BO"]
            .iter()
            .filter_map(|s| p.holding(s))
            .map(|h| h.shares)
            .sum();
        assert!(close(total, 1.0), "{total}");
        assert!(out.fraction_dropped.abs() < 1e-9);
        // The .BO listing was applied in the same pass.
        assert!(p.pending_demergers().is_empty());
    }

    #[test]
    fn the_cost_split_is_in_whole_paise_and_adds_up() {
        let mut p = Portfolio::default();
        p.add_lot("RELIANCE.NS", Lot::new(1.0, 12.5, "2022-05-02", "manual"));
        p.apply_demerger("RELIANCE.NS", demerger("JIOFIN"));
        let child = p.holding("JIOFIN.NS").unwrap().cost_basis();
        let parent = p.holding("RELIANCE.NS").unwrap().cost_basis();
        // 4.68% of 12.50 is 0.585: 0.59 to the child, 11.91 stays.
        assert!(close(child, 0.59), "{child}");
        assert!(close(parent, 11.91), "{parent}");
        assert!(close(child + parent, 12.5));
    }

    #[test]
    fn shares_sold_after_the_record_date_get_their_child_shares_and_a_split_cost() {
        let mut p = Portfolio::default();
        p.add_lot("ITC.NS", itc_lot(1000.0, "2020-03-02", "zerodha"));
        p.add_lot("ITC.NS", itc_lot(500.0, "2021-03-02", "zerodha"));
        // Sold everything after the record date, before applying.
        p.sell_fifo("ITC.NS", 1500.0, 420.0, "2025-03-03", "manual")
            .unwrap();
        // Sold before the record date: not entitled.
        p.add_lot("ITC.NS", itc_lot(100.0, "2022-01-03", "zerodha"));
        p.sell_fifo("ITC.NS", 100.0, 300.0, "2024-06-03", "manual")
            .unwrap();
        assert!(p.holding("ITC.NS").is_none());
        assert_eq!(
            p.pending_demergers().len(),
            1,
            "a fully sold parent is pending"
        );
        let out = p.apply_demerger("ITC.NS", demerger("ITCHOTELS"));
        assert_eq!((out.lots, out.sales), (0, 2));
        assert_eq!(p.holding("ITCHOTELS.NS").unwrap().shares, 150.0);
        let after: Vec<&Sale> = p
            .realized
            .iter()
            .filter(|s| s.sell_date == "2025-03-03")
            .collect();
        assert!(after.iter().all(|s| close(s.buy_price, 400.0 * 0.8649)));
        let early = p
            .realized
            .iter()
            .find(|s| s.sell_date == "2024-06-03")
            .unwrap();
        assert_eq!(early.buy_price, 400.0);
        // Applying again changes nothing.
        let once = serde_json::to_string(&p).unwrap();
        assert_eq!(
            p.apply_demerger("ITC.NS", demerger("ITCHOTELS")),
            DemergerOutcome::default()
        );
        assert_eq!(serde_json::to_string(&p).unwrap(), once);
    }
}
