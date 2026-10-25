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
        }
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
}

impl Sale {
    fn from_lot(symbol: &str, lot: &Lot, qty: f64, date: &str, price: f64, source: &str) -> Self {
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
        }
    }

    pub fn gain(&self) -> f64 {
        self.qty * (self.sell_price - self.buy_price)
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

/// A stock split or bonus issue: `numerator` shares for every `denominator`
/// held. Yahoo reports Indian bonus issues as splits (1:1 bonus = 2:1).
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    pub date: String,
    pub numerator: f64,
    pub denominator: f64,
}

impl Split {
    pub fn ratio(&self) -> f64 {
        self.numerator / self.denominator
    }

    pub fn id(&self) -> String {
        format!("{}:{}:{}", self.date, self.numerator, self.denominator)
    }

    /// Whether `lot`'s quantity predates this split and was not yet adjusted for it.
    pub fn pending_for(&self, lot: &Lot) -> bool {
        lot.units_date() < self.date.as_str() && !lot.adjustments.contains(&self.id())
    }
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
}

#[derive(Debug, Clone, PartialEq)]
pub enum ImportOutcome {
    /// No dates in the file: existing lots kept, quantity matched.
    Synced(SyncChange),
    /// Dated rows: the symbol's lots rebuilt from the file.
    Lots {
        lots: usize,
        sales: usize,
        /// Splits applied while replaying a tradebook.
        splits: usize,
    },
    /// Left unchanged: sells of `unmatched` shares had no earlier buy in the
    /// file, so it is not the symbol's whole history.
    Unmatched { unmatched: f64 },
    /// Left unchanged: the holding has lots or sales from elsewhere that
    /// rebuilding it from this file would replace (allowed with `replace`).
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
}

impl Default for Portfolio {
    fn default() -> Self {
        Self {
            version: SCHEMA_VERSION,
            holdings: Vec::new(),
            realized: Vec::new(),
            dividends: Vec::new(),
            broker_fills: Vec::new(),
        }
    }
}

/// Take `qty` shares from the oldest lots bought on or before `date` (lots
/// must be sorted oldest first). Returns the slices taken and any quantity
/// that could not be matched.
fn take_fifo(lots: &mut Vec<Lot>, qty: f64, date: &str) -> (Vec<(Lot, f64)>, f64) {
    let mut remaining = qty;
    let mut taken = Vec::new();
    for lot in lots.iter_mut().filter(|l| l.date.as_str() <= date) {
        if remaining <= EPS {
            break;
        }
        let take = if remaining >= lot.qty - EPS {
            lot.qty
        } else {
            remaining
        };
        taken.push((lot.clone(), take));
        lot.qty -= take;
        remaining -= take;
    }
    lots.retain(|l| l.qty > EPS);
    (taken, if remaining > EPS { remaining } else { 0.0 })
}

impl Portfolio {
    /// Load the active portfolio (see [`select`]).
    pub fn load() -> Result<Self> {
        Self::load_named(active())
    }

    pub fn load_named(name: Option<&str>) -> Result<Self> {
        Self::load_from(&crate::store::path(&file_name(name))?)
    }

    /// Save to the *active* portfolio's file — not necessarily the one a
    /// `load_named` call read.
    pub fn save(&self) -> Result<()> {
        self.save_to(&crate::store::path(&file_name(active()))?)
    }

    /// Load `path`, upgrading an old-format file in place: the original is
    /// copied to `<file>.bak` (only if no backup exists yet), then the new
    /// format is written.
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
        if upgrade || (repaired && path.exists()) {
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

    pub fn save_to(&self, path: &Path) -> Result<()> {
        crate::store::save_to(path, self, false)
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
    pub fn sync_quantity(&mut self, symbol: &str, qty: f64, avg: f64, source: &str) -> SyncChange {
        let mut lot = Lot::new(qty, avg, today(), source);
        lot.date_estimated = true;
        let Some(h) = self.holding_mut(symbol) else {
            self.add_lot(symbol, lot);
            return SyncChange::New;
        };
        let diff = qty - h.shares;
        if diff > EPS {
            let implied = (qty * avg - h.cost_basis()) / diff;
            lot.qty = diff;
            if implied.is_finite() && implied > 0.0 {
                lot.price = implied;
            }
            h.lots.push(lot);
            h.recompute();
            SyncChange::Added(diff)
        } else if diff < -EPS {
            take_fifo(&mut h.lots, -diff, "9999-12-31");
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
        if qty <= EPS || !qty.is_finite() {
            bail!("Quantity to sell must be positive");
        }
        if price < 0.0 || !price.is_finite() {
            bail!("Sale price can't be negative");
        }
        let sym = symbol.to_uppercase();
        let Some(h) = self.holding_mut(&sym) else {
            bail!("{} is not in this portfolio", sym);
        };
        let held_then: f64 = h
            .lots
            .iter()
            .filter(|l| l.date.as_str() <= date)
            .map(|l| l.qty)
            .sum();
        if qty > held_then + EPS {
            if held_then + EPS < h.shares {
                bail!(
                    "Cannot sell {} {} on {}: only {} were bought by then",
                    qty,
                    sym,
                    date,
                    held_then
                );
            }
            bail!("Cannot sell {} {}: you hold {}", qty, sym, h.shares);
        }
        let (taken, _) = take_fifo(&mut h.lots, qty, date);
        h.recompute();
        if h.lots.is_empty() {
            self.holdings.retain(|h| h.symbol != sym);
        }
        let sales: Vec<Sale> = taken
            .iter()
            .map(|(lot, q)| Sale::from_lot(&sym, lot, *q, date, price, source))
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

    pub fn remove(&mut self, symbol: &str) -> bool {
        let before = self.holdings.len();
        self.holdings.retain(|h| h.symbol != symbol.to_uppercase());
        self.holdings.len() < before
    }

    /// Apply import rows symbol by symbol. A symbol whose rows have no dates
    /// (a holdings snapshot) keeps its lots and only syncs the quantity. A
    /// symbol with dated rows or sells (a lot list or tradebook) is rebuilt
    /// from the file: buys become lots, sells come out FIFO and are recorded
    /// as realized; for a tradebook, splits are applied as they happen. A
    /// symbol is left alone when a sell has no earlier buy in the file, or
    /// (without `replace`) when the rebuild would drop lots or sales from
    /// another source. Re-importing the same file changes nothing.
    pub fn apply_import(
        &mut self,
        rows: &[ImportRow],
        opts: ImportOptions,
    ) -> Vec<(String, ImportOutcome)> {
        let source = opts.source;
        let mut order: Vec<String> = Vec::new();
        for r in rows {
            let sym = r.symbol.to_uppercase();
            if !order.contains(&sym) {
                order.push(sym);
            }
        }
        let mut out = Vec::new();
        for sym in order {
            let mut group: Vec<&ImportRow> = rows
                .iter()
                .filter(|r| r.symbol.to_uppercase() == sym)
                .collect();
            let lot_mode = group.iter().any(|r| r.date.is_some() || r.sell);
            if !lot_mode {
                let qty: f64 = group.iter().map(|r| r.qty).sum();
                let cost: f64 = group.iter().map(|r| r.qty * r.price).sum();
                if qty > EPS {
                    let change = self.sync_quantity(&sym, qty, cost / qty, source);
                    out.push((sym, ImportOutcome::Synced(change)));
                }
                continue;
            }
            let today = today();
            // Buys before sells on the same day.
            group.sort_by(|a, b| {
                let da = a.date.as_deref().unwrap_or(&today);
                let db = b.date.as_deref().unwrap_or(&today);
                da.cmp(db).then(a.sell.cmp(&b.sell))
            });
            let first = group[0].date.clone().unwrap_or_else(|| today.clone());
            let foreign_lots = self
                .holding(&sym)
                .is_some_and(|h| h.lots.iter().any(|l| l.source != source));
            let foreign_sales = self
                .realized
                .iter()
                .any(|s| s.symbol == sym && s.source != source && s.sell_date >= first);
            if (foreign_lots || foreign_sales) && !opts.replace {
                out.push((sym, ImportOutcome::WouldReplace));
                continue;
            }

            let splits: &[Split] = if opts.tradebook {
                opts.splits
                    .iter()
                    .find(|(s, _)| s.to_uppercase() == sym)
                    .map_or(&[], |(_, v)| v.as_slice())
            } else {
                &[]
            };
            let mut applied = Vec::new();
            let apply_until = |lots: &mut Vec<Lot>, date: &str, applied: &mut Vec<String>| {
                for split in splits.iter().filter(|s| s.date.as_str() <= date) {
                    for lot in lots.iter_mut().filter(|l| split.pending_for(l)) {
                        lot.qty *= split.ratio();
                        lot.price /= split.ratio();
                        lot.adjustments.push(split.id());
                        if !applied.contains(&split.id()) {
                            applied.push(split.id());
                        }
                    }
                }
            };
            let mut lots: Vec<Lot> = Vec::new();
            let mut sales = Vec::new();
            let mut unmatched = 0.0;
            for r in group {
                let date = r.date.clone().unwrap_or_else(|| today.clone());
                apply_until(&mut lots, &date, &mut applied);
                if r.sell {
                    let (taken, left) = take_fifo(&mut lots, r.qty, &date);
                    sales.extend(
                        taken
                            .iter()
                            .map(|(lot, q)| Sale::from_lot(&sym, lot, *q, &date, r.price, source)),
                    );
                    unmatched += left;
                } else {
                    let mut lot = Lot::new(r.qty, r.price, date, source);
                    lot.date_estimated = r.date.is_none();
                    lots.push(lot);
                    lots.sort_by(|a, b| a.date.cmp(&b.date));
                }
            }
            // Bring the remaining lots up to today's share count.
            apply_until(&mut lots, "9999-12-31", &mut applied);
            // A sell with no earlier buy means the file doesn't cover this
            // symbol's whole history: leave the holding as it is.
            if unmatched > EPS {
                out.push((sym, ImportOutcome::Unmatched { unmatched }));
                continue;
            }
            self.realized
                .retain(|s| s.symbol != sym || (s.source != source && s.sell_date < first));
            let (n, n_sales) = (lots.len(), sales.len());
            self.realized.extend(sales);
            self.replace_lots(&sym, lots);
            out.push((
                sym,
                ImportOutcome::Lots {
                    lots: n,
                    sales: n_sales,
                    splits: applied.len(),
                },
            ));
        }
        out
    }

    /// Apply fills from a broker's trade book. Fills already synced (by
    /// broker, order id and fill id) are skipped. The new fills of one order on one
    /// day are merged at their average price; orders are applied in time
    /// order: a buy becomes a lot (dated, `broker:<id>`, with its order id),
    /// a sell comes out of the oldest lots held on that date and is
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
        groups.sort_by(|a, b| {
            (a[0].date.as_str(), first_time(a), a[0].sell).cmp(&(
                b[0].date.as_str(),
                first_time(b),
                b[0].sell,
            ))
        });

        for g in groups {
            let head = g[0];
            let qty: f64 = g.iter().map(|f| f.qty).sum();
            let price = g.iter().map(|f| f.qty * f.price).sum::<f64>() / qty;
            // Round the average to the paisa / cent it can't be finer than.
            let price = (price * 10_000.0).round() / 10_000.0;
            let source = format!("broker:{}", head.broker);
            let effect = if head.sell {
                let held: f64 = self.holding(&head.symbol).map_or(0.0, |h| {
                    h.lots
                        .iter()
                        .filter(|l| l.date.as_str() <= head.date.as_str())
                        .map(|l| l.qty)
                        .sum()
                });
                let sold = (qty <= held + EPS)
                    .then(|| self.sell_fifo(&head.symbol, qty, price, &head.date, &source))
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

    /// Apply a split or bonus to the lots bought before it: quantity × ratio,
    /// price ÷ ratio, so cost and purchase date are unchanged. Lots already
    /// adjusted for it are skipped. Returns how many lots changed.
    pub fn apply_split(&mut self, symbol: &str, split: &Split) -> usize {
        let r = split.ratio();
        if !r.is_finite() || r <= 0.0 {
            return 0;
        }
        let Some(h) = self.holding_mut(symbol) else {
            return 0;
        };
        let id = split.id();
        let mut changed = 0;
        for lot in h.lots.iter_mut().filter(|l| split.pending_for(l)) {
            lot.qty *= r;
            lot.price /= r;
            lot.adjustments.push(id.clone());
            changed += 1;
        }
        if changed > 0 {
            h.recompute();
        }
        changed
    }

    /// Shares of `symbol` held at the start of `date` (bought before it, not
    /// yet sold), in today's share count: `splits` not yet applied to a lot,
    /// and splits after a sale, are multiplied in.
    pub fn shares_on(&self, symbol: &str, date: &str, splits: &[Split]) -> f64 {
        let sym = symbol.to_uppercase();
        let later = |after: &str, lot: Option<&Lot>| -> f64 {
            splits
                .iter()
                .filter(|s| s.date.as_str() > after && lot.is_none_or(|l| s.pending_for(l)))
                .map(Split::ratio)
                .product()
        };
        let open: f64 = self.holding(&sym).map_or(0.0, |h| {
            h.lots
                .iter()
                .filter(|l| l.date.as_str() < date)
                .map(|l| l.qty * later(&l.date, Some(l)))
                .sum()
        });
        let sold: f64 = self
            .realized
            .iter()
            .filter(|s| {
                s.symbol == sym && s.buy_date.as_str() < date && s.sell_date.as_str() >= date
            })
            .map(|s| s.qty * later(&s.sell_date, None))
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
        let sym = symbol.to_uppercase();
        let mut flows = Vec::new();
        if let Some(h) = self.holding(&sym) {
            for l in &h.lots {
                if let Some(d) = date_of(&l.date) {
                    flows.push((d, -l.cost()));
                }
            }
        }
        for s in self.realized.iter().filter(|s| s.symbol == sym) {
            if let (Some(b), Some(d)) = (date_of(&s.buy_date), date_of(&s.sell_date)) {
                flows.push((b, -s.qty * s.buy_price));
                flows.push((d, s.qty * s.sell_price));
            }
        }
        for d in self.dividends.iter().filter(|d| d.symbol == sym) {
            if let Some(x) = date_of(&d.ex_date) {
                flows.push((x, d.amount));
            }
        }
        flows.sort_by_key(|f| f.0);
        flows
    }
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

fn backup_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".bak");
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let rows = vec![
            ImportRow {
                symbol: "RELIANCE.NS".into(),
                qty: 10.0,
                price: 2800.0,
                date: Some("2023-05-02".into()),
                sell: false,
            },
            ImportRow {
                symbol: "RELIANCE.NS".into(),
                qty: 15.0,
                price: 1500.0,
                date: Some("2025-01-10".into()),
                sell: true,
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

    #[test]
    fn partial_tradebook_leaves_the_holding_alone() {
        let rows = vec![ImportRow {
            symbol: "X".into(),
            qty: 2.0,
            price: 10.0,
            date: Some("2024-01-01".into()),
            sell: true,
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
}
