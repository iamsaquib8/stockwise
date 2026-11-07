//! Holdings imports beyond the Kite / INDmoney / generic CSV parsers in
//! `holdings.rs`: Groww and Upstox holdings exports, and CAMS / KFintech
//! consolidated account statements (CAS) in casparser's JSON or CSV form.
//!
//! Parsers are pure (text in, rows out) so they can be tested with fixtures;
//! `import_rows` turns them into `portfolio::ImportRow`s and applies them
//! through the same lot-based path as the other imports: dated transactions
//! (a CAS that covers a scheme's whole history) rebuild its lots, snapshots
//! only sync the quantity. Re-importing changes nothing.

use super::holdings::{find_col, parse_csv_header, parse_csv_row};
use super::*;
use crate::portfolio::ImportRow;

/// One dated transaction from a CAS, for lot-level import.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Txn {
    /// `YYYY-MM-DD`.
    pub date: String,
    pub qty: f64,
    /// Price per unit including charges (cash paid / units).
    pub price: f64,
    pub sell: bool,
}

/// One holding read from an import file.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct ParsedRow {
    /// Portfolio symbol (`RELIANCE.NS`, `MF:118955`), or empty when only an
    /// ISIN is known and it still has to be looked up.
    pub symbol: String,
    pub isin: Option<String>,
    /// Display name (company or scheme), when the file has one.
    pub name: Option<String>,
    pub qty: f64,
    /// Average cost per unit. `None` when the file has no cost (a demat or
    /// summary CAS): an existing holding then gets its quantity updated and
    /// keeps its cost; a new one is skipped.
    pub avg_cost: Option<f64>,
    /// "shares" or "units".
    pub unit: &'static str,
    /// Every transaction behind `qty`, when the file has them all (a CAS
    /// from the first purchase). Empty for snapshots.
    pub history: Vec<Txn>,
}

/// A row that could not be imported, and why.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Skipped {
    pub what: String,
    pub why: String,
}

#[derive(Debug, Default)]
pub(super) struct Parsed {
    pub rows: Vec<ParsedRow>,
    pub skipped: Vec<Skipped>,
}

/// Parse a number as exported by Indian brokers: `1,234.50`, `₹ 1,234.50`,
/// `"(12.5)"`, `-`.
fn number(s: &str) -> Option<f64> {
    let t: String = s
        .trim()
        .trim_matches('"')
        .chars()
        .filter(|c| !matches!(c, ',' | '₹' | ' ' | '\u{a0}'))
        .collect();
    let (neg, t) = match t.strip_prefix('(').and_then(|x| x.strip_suffix(')')) {
        Some(inner) => (true, inner.to_string()),
        None => (false, t),
    };
    let v: f64 = t.parse().ok()?;
    v.is_finite().then_some(if neg { -v } else { v })
}

fn is_isin(s: &str) -> bool {
    let s = s.trim();
    // Check characters, not byte offsets: a cell such as `₹12,34,567` is 12
    // bytes long but has no character boundary at byte 2.
    s.len() == 12
        && s.is_ascii()
        && s.chars().take(2).all(|c| c.is_ascii_uppercase())
        && s.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Find the header row: exports often start with a few lines of account
/// details (name, client code, statement date) before the table.
fn find_header(lines: &[&str], groups: &[&[&str]]) -> Option<(usize, Vec<String>)> {
    lines.iter().take(40).enumerate().find_map(|(i, line)| {
        let cols = parse_csv_header(line);
        groups
            .iter()
            .all(|names| find_col(&cols, names).is_some())
            .then_some((i, cols))
    })
}

/// Column layout of a stock holdings export.
struct StockLayout {
    broker: &'static str,
    /// Expected columns, for the error message.
    expected: &'static str,
    symbol: &'static [&'static str],
    isin: &'static [&'static str],
    name: &'static [&'static str],
    qty: &'static [&'static str],
    avg: &'static [&'static str],
}

/// Groww → Stocks → Holdings → Download, then save the sheet as CSV.
const GROWW: StockLayout = StockLayout {
    broker: "Groww",
    expected: "Stock Name, ISIN, Quantity, Average buy price",
    symbol: &["symbol", "nse symbol", "trading symbol"],
    isin: &["isin"],
    name: &["stock name", "company name", "name"],
    qty: &["quantity", "qty", "shares"],
    avg: &[
        "average buy price",
        "avg buy price",
        "average price",
        "avg. buy price",
        "avg price",
        "buy price",
    ],
};

/// Upstox → Portfolio → Holdings → Download.
const UPSTOX: StockLayout = StockLayout {
    broker: "Upstox",
    expected: "Symbol / Scrip Name / ISIN, Qty, Avg. Price",
    symbol: &["symbol", "trading symbol", "tradingsymbol", "scrip code"],
    isin: &["isin"],
    name: &[
        "scrip name",
        "company name",
        "instrument",
        "stock name",
        "name",
    ],
    qty: &["quantity", "qty", "net qty", "shares"],
    avg: &[
        "avg. price",
        "avg price",
        "average price",
        "avg. cost",
        "avg cost",
        "average cost",
        "buy avg",
    ],
};

fn parse_stock_export(content: &str, layout: &StockLayout, market: Market) -> Result<Parsed> {
    let lines: Vec<&str> = content
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}'))
        .collect();
    let any_symbol: Vec<&str> = layout
        .symbol
        .iter()
        .chain(layout.isin)
        .chain(layout.name)
        .copied()
        .collect();
    let Some((h, cols)) = find_header(&lines, &[&any_symbol, layout.qty, layout.avg]) else {
        bail!(
            "Could not detect {} CSV/Excel columns. Expected: {}.",
            layout.broker,
            layout.expected
        );
    };
    let sym_col = find_col(&cols, layout.symbol);
    let isin_col = find_col(&cols, layout.isin);
    let name_col = find_col(&cols, layout.name);
    let qty_col = find_col(&cols, layout.qty).expect("checked by find_header");
    let avg_col = find_col(&cols, layout.avg).expect("checked by find_header");

    let mut out = Parsed::default();
    for line in &lines[h + 1..] {
        let fields = parse_csv_row(line);
        let get = |c: Option<usize>| {
            c.and_then(|i| fields.get(i))
                .map(|f| f.trim().trim_matches('"').trim().to_string())
                .filter(|f| !f.is_empty() && f != "-")
        };
        let name = get(name_col);
        let isin = get(isin_col).filter(|s| is_isin(s));
        // A name column that holds a ticker (no spaces) is a symbol too.
        let symbol = get(sym_col).or_else(|| {
            name.clone()
                .filter(|n| isin.is_none() && !n.contains(' ') && n.len() <= 20)
        });
        if symbol.is_none() && isin.is_none() && name.is_none() {
            continue; // blank or footer line
        }
        let label = name
            .clone()
            .or_else(|| symbol.clone())
            .or_else(|| isin.clone())
            .unwrap_or_default();
        // Totals, and Groww's "NA" filler rows.
        if label.to_lowercase().starts_with("total") || label.eq_ignore_ascii_case("NA") {
            continue;
        }
        let qty = get(Some(qty_col)).and_then(|q| number(&q)).unwrap_or(0.0);
        let avg = get(Some(avg_col)).and_then(|a| number(&a)).unwrap_or(0.0);
        if qty <= 0.0 || avg <= 0.0 {
            out.skipped.push(Skipped {
                what: label,
                why: "no positive quantity and average price".into(),
            });
            continue;
        }
        if symbol.is_none() && isin.is_none() {
            out.skipped.push(Skipped {
                what: label,
                why: "no symbol or ISIN — add it with `portfolio add`".into(),
            });
            continue;
        }
        out.rows.push(ParsedRow {
            symbol: symbol
                .map(|s| market::resolve_symbol(&crate::brokers::plain_symbol(&s), market))
                .unwrap_or_default(),
            isin,
            name,
            qty,
            avg_cost: Some(avg),
            unit: "shares",
            history: Vec::new(),
        });
    }
    Ok(out)
}

pub(super) fn parse_groww(content: &str, market: Market) -> Result<Parsed> {
    parse_stock_export(content, &GROWW, market)
}

pub(super) fn parse_upstox(content: &str, market: Market) -> Result<Parsed> {
    parse_stock_export(content, &UPSTOX, market)
}

// ── CAS (CAMS / KFintech) ──

/// Portfolio symbol for a mutual-fund scheme: `MF:<AMFI code>`, else
/// `MF:<ISIN>`. Never a Yahoo ticker, so it can't be confused with a stock.
fn mf_symbol(amfi: Option<&str>, isin: Option<&str>) -> Option<String> {
    fn pick(s: Option<&str>) -> Option<&str> {
        s.map(str::trim).filter(|s| !s.is_empty())
    }
    pick(amfi)
        .or_else(|| pick(isin))
        .map(|id| format!("MF:{}", id.to_uppercase()))
}

/// Running average-cost book for one scheme: purchases add units and cost,
/// redemptions remove units at the current average cost. It also keeps the
/// dated transactions, for lot-level import.
#[derive(Debug, Default, Clone)]
struct CostBook {
    units: f64,
    cost: f64,
    txns: Vec<Txn>,
    /// A transaction had units but no usable date or price.
    incomplete: bool,
    /// A redemption was larger than the units the transactions had bought:
    /// the rest came from before the statement period.
    oversold: bool,
}

/// `YYYY-MM-DD` from casparser's ISO dates (or `DD-MMM-YYYY`).
fn iso_date(raw: &str) -> Option<String> {
    let raw = raw.trim();
    chrono::NaiveDate::parse_from_str(raw, "%Y-%m-%d")
        .or_else(|_| chrono::NaiveDate::parse_from_str(raw, "%d-%b-%Y"))
        .ok()
        .map(|d| d.format("%Y-%m-%d").to_string())
}

impl CostBook {
    /// Apply one transaction. `units` is signed (negative for redemptions
    /// and switch-outs); `amount` is the cash value. Rows without units
    /// (stamp duty, STT, TDS, dividend payouts) only adjust cost when they are
    /// stamp duty, which is part of the purchase cost.
    fn apply(
        &mut self,
        date: Option<&str>,
        kind: &str,
        units: Option<f64>,
        amount: Option<f64>,
        nav: Option<f64>,
    ) {
        let date = date.and_then(iso_date);
        match units {
            Some(u) if u > 0.0 => {
                let value = amount
                    .map(f64::abs)
                    .filter(|a| *a > 0.0)
                    .or_else(|| nav.map(|n| n * u))
                    .unwrap_or(0.0);
                self.units += u;
                self.cost += value;
                match date {
                    Some(date) if value > 0.0 => self.txns.push(Txn {
                        date,
                        qty: u,
                        price: value / u,
                        sell: false,
                    }),
                    _ => self.incomplete = true,
                }
            }
            Some(u) if u < 0.0 => {
                let sold = -u;
                if sold > self.units + 0.002_f64.max(self.units * 1e-4) {
                    self.oversold = true;
                }
                let u = sold.min(self.units);
                if self.units > 0.0 {
                    self.cost -= self.cost / self.units * u;
                }
                self.units -= u;
                if self.units < 1e-6 {
                    self.units = 0.0;
                    self.cost = 0.0;
                }
                let price = amount
                    .map(f64::abs)
                    .filter(|a| *a > 0.0)
                    .map(|a| a / sold)
                    .or(nav)
                    .filter(|p| *p > 0.0);
                match (date, price) {
                    (Some(date), Some(price)) => self.txns.push(Txn {
                        date,
                        qty: sold,
                        price,
                        sell: true,
                    }),
                    _ => self.incomplete = true,
                }
            }
            _ => {
                if kind.to_ascii_uppercase().contains("STAMP")
                    && let Some(a) = amount
                {
                    self.cost += a.abs();
                    // Stamp duty belongs to that day's purchase.
                    if let Some(buy) = self
                        .txns
                        .iter_mut()
                        .rev()
                        .find(|t| !t.sell && Some(&t.date) == date.as_ref())
                    {
                        buy.price += a.abs() / buy.qty;
                    }
                }
            }
        }
    }

    /// The dated transactions, when they explain the whole closing balance.
    fn history_for(&self, closing_units: f64) -> Vec<Txn> {
        if self.incomplete || self.txns.is_empty() || self.avg_for(closing_units).is_none() {
            return Vec::new();
        }
        self.txns.clone()
    }

    /// The dated transactions of a scheme that ended at zero units, when they
    /// cover its whole life: every unit sold was bought in them, so the
    /// sales have a cost. Empty when units bought earlier are missing.
    fn redeemed_history(&self) -> Vec<Txn> {
        if self.incomplete
            || self.oversold
            || self.units > 1e-6
            || !self.txns.iter().any(|t| t.sell)
        {
            return Vec::new();
        }
        self.txns.clone()
    }

    fn avg(&self) -> Option<f64> {
        (self.units > 0.0 && self.cost > 0.0).then(|| self.cost / self.units)
    }

    /// Average cost, but only if the transactions explain the whole closing
    /// balance. A statement that starts with units bought before its period
    /// has no cost for them, and averaging only the later purchases would be
    /// wrong.
    fn avg_for(&self, closing_units: f64) -> Option<f64> {
        let tolerance = 0.002_f64.max(closing_units * 1e-4);
        ((self.units - closing_units).abs() <= tolerance)
            .then(|| self.avg())
            .flatten()
    }
}

/// A scheme whose closing balance is zero. When its transactions cover its
/// whole life, it becomes a row with no units left and the dated buys and
/// sells, so the redemptions are recorded as realized sales for `tax`. When
/// units bought before the statement period are missing, it is skipped with
/// a reason; a scheme with no unit activity at all is ignored.
fn redeemed_scheme(
    out: &mut Parsed,
    book: &CostBook,
    opening_zero: bool,
    what: String,
    symbol: Option<String>,
    isin: Option<String>,
    name: String,
) {
    if !book.txns.iter().any(|t| t.sell) && !book.incomplete && !book.oversold {
        return;
    }
    let history = if opening_zero {
        book.redeemed_history()
    } else {
        Vec::new()
    };
    if history.is_empty() {
        out.skipped.push(Skipped {
            what,
            why: "fully redeemed, but the statement does not cover every unit's purchase (or a \
                  transaction has no date or amount), so the gain cannot be worked out — \
                  import a statement that starts before the first purchase"
                .into(),
        });
        return;
    }
    let Some(symbol) = symbol else {
        out.skipped.push(Skipped {
            what,
            why: "no AMFI code or ISIN".into(),
        });
        return;
    };
    out.rows.push(ParsedRow {
        symbol,
        isin,
        name: Some(name),
        qty: 0.0,
        avg_cost: None,
        unit: "units",
        history,
    });
}

fn json_num(v: Option<&serde_json::Value>) -> Option<f64> {
    match v? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => number(s),
        _ => None,
    }
}

fn json_str(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// casparser JSON (`casparser CAS.pdf -p PASSWORD -o cas.json`).
pub(super) fn parse_cas_json(content: &str) -> Result<Parsed> {
    let doc: serde_json::Value =
        serde_json::from_str(content).context("Not valid JSON — is this casparser output?")?;
    if let Some(accounts) = doc.get("accounts").and_then(|a| a.as_array()) {
        return Ok(parse_demat_accounts(accounts));
    }
    let Some(folios) = doc.get("folios").and_then(|f| f.as_array()) else {
        bail!(
            "No `folios` or `accounts` in this JSON. Expected casparser output for a CAMS / \
             KFintech / NSDL / CDSL CAS (`casparser CAS.pdf -o cas.json`)."
        );
    };
    let mut out = Parsed::default();
    for folio in folios {
        let folio_no = json_str(folio.get("folio")).unwrap_or_default();
        let schemes = folio
            .get("schemes")
            .and_then(|s| s.as_array())
            .map(Vec::as_slice)
            .unwrap_or_default();
        for scheme in schemes {
            let name = json_str(scheme.get("scheme")).unwrap_or_else(|| "Unknown scheme".into());
            let what = format!("{name} (folio {folio_no})");
            let isin = json_str(scheme.get("isin"));
            let amfi = json_str(scheme.get("amfi"));
            let units = json_num(scheme.get("close"))
                .or_else(|| json_num(scheme.get("close_calculated")))
                .unwrap_or(0.0);
            let mut book = CostBook::default();
            for t in scheme
                .get("transactions")
                .and_then(|t| t.as_array())
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                book.apply(
                    json_str(t.get("date")).as_deref(),
                    &json_str(t.get("type")).unwrap_or_default(),
                    json_num(t.get("units")),
                    json_num(t.get("amount")),
                    json_num(t.get("nav")),
                );
            }
            let symbol = mf_symbol(amfi.as_deref(), isin.as_deref());
            if units <= 1e-6 {
                // Fully redeemed: its sales still count, if the history is whole.
                let opening_zero = json_num(scheme.get("open")).is_none_or(|o| o <= 1e-6);
                redeemed_scheme(&mut out, &book, opening_zero, what, symbol, isin, name);
                continue;
            }
            let Some(symbol) = symbol else {
                out.skipped.push(Skipped {
                    what,
                    why: "no AMFI code or ISIN".into(),
                });
                continue;
            };
            let valuation = scheme.get("valuation");
            let stated_cost = json_num(valuation.and_then(|v| v.get("cost"))).filter(|c| *c > 0.0);
            // Dated lots when the transactions cover the whole balance; else
            // the statement's cost value; else quantity only (a fund already
            // tracked keeps its cost).
            let avg = stated_cost
                .map(|c| c / units)
                .or_else(|| book.avg_for(units));
            out.rows.push(ParsedRow {
                symbol,
                isin,
                name: Some(name),
                qty: units,
                avg_cost: avg,
                unit: "units",
                history: book.history_for(units),
            });
        }
    }
    Ok(out)
}

/// The Yahoo ticker for a demat CAS equity: any `.NS` / `.BO` suffix the
/// statement already carries is dropped first, then a BSE listing (`exchange`
/// BSE, or a `.BO` suffix) gets `.BO`, a numeric BSE scrip code (`500325`)
/// gets `.BO`, and everything else is an NSE ticker (`.NS`).
fn demat_symbol(symbol: &str, exchange: Option<&str>) -> String {
    let upper = symbol.trim().to_uppercase();
    let (plain, has_bo) = match upper.strip_suffix(".BO") {
        Some(base) => (base, true),
        None => (upper.as_str(), false),
    };
    let plain = crate::brokers::plain_symbol(plain);
    if plain.is_empty() {
        return String::new();
    }
    if has_bo || exchange.is_some_and(|x| x.trim().eq_ignore_ascii_case("BSE")) {
        format!("{plain}.BO")
    } else {
        market::resolve_symbol(&plain, Market::In)
    }
}

/// NSDL / CDSL (demat) CAS from casparser: `accounts[].equities[]` and
/// `accounts[].mutual_funds[]`. Equities carry no purchase cost, so they
/// only update quantities of holdings you already track.
fn parse_demat_accounts(accounts: &[serde_json::Value]) -> Parsed {
    let mut out = Parsed::default();
    let items = |a: &serde_json::Value, key: &str| {
        a.get(key)
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
    };
    for account in accounts {
        for e in items(account, "equities") {
            let isin = json_str(e.get("isin"));
            let name = json_str(e.get("name"));
            let qty = json_num(e.get("num_shares")).unwrap_or(0.0);
            if qty <= 0.0 {
                continue;
            }
            // casparser back-fills the ticker when it knows it.
            let symbol = json_str(e.get("symbol"))
                .map(|s| demat_symbol(&s, json_str(e.get("exchange")).as_deref()))
                .unwrap_or_default();
            if symbol.is_empty() && isin.is_none() {
                continue;
            }
            out.rows.push(ParsedRow {
                symbol,
                isin,
                name,
                qty,
                avg_cost: None,
                unit: "shares",
                history: Vec::new(),
            });
        }
        for mf in items(account, "mutual_funds") {
            let isin = json_str(mf.get("isin"));
            let amfi = json_str(mf.get("amfi"));
            let units = json_num(mf.get("balance")).unwrap_or(0.0);
            let Some(symbol) = mf_symbol(amfi.as_deref(), isin.as_deref()) else {
                continue;
            };
            if units <= 1e-6 {
                continue;
            }
            let avg = json_num(mf.get("avg_cost"))
                .filter(|c| *c > 0.0)
                .or_else(|| {
                    json_num(mf.get("total_cost"))
                        .filter(|c| *c > 0.0)
                        .map(|c| c / units)
                });
            out.rows.push(ParsedRow {
                symbol,
                isin,
                name: json_str(mf.get("name")),
                qty: units,
                avg_cost: avg,
                unit: "units",
                history: Vec::new(),
            });
        }
    }
    out
}

/// casparser summary CSV (`-s`): one row per scheme with closing units and
/// no cost.
fn parse_cas_summary_csv(lines: &[&str], h: usize, cols: &[String]) -> Parsed {
    let col = |name: &str| cols.iter().position(|c| c == name);
    let (scheme_c, isin_c, amfi_c, close_c, folio_c) = (
        col("scheme"),
        col("isin"),
        col("amfi"),
        col("close"),
        col("folio"),
    );
    let mut out = Parsed::default();
    for line in &lines[h + 1..] {
        let f = parse_csv_row(line);
        let get = |c: Option<usize>| {
            c.and_then(|i| f.get(i))
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(scheme) = get(scheme_c) else {
            continue;
        };
        let units = get(close_c).and_then(|v| number(&v)).unwrap_or(0.0);
        if units <= 1e-6 {
            continue;
        }
        let (isin, amfi) = (get(isin_c), get(amfi_c));
        let Some(symbol) = mf_symbol(amfi.as_deref(), isin.as_deref()) else {
            out.skipped.push(Skipped {
                what: format!("{scheme} (folio {})", get(folio_c).unwrap_or_default()),
                why: "no AMFI code or ISIN".into(),
            });
            continue;
        };
        out.rows.push(ParsedRow {
            symbol,
            isin,
            name: Some(scheme),
            qty: units,
            avg_cost: None,
            unit: "units",
            history: Vec::new(),
        });
    }
    out
}

/// casparser CSV (`casparser CAS.pdf -p PASSWORD -o cas.csv`): one row per
/// transaction; holdings are rebuilt per folio and scheme. The summary
/// layout (`-s`) is accepted too.
pub(super) fn parse_cas_csv(content: &str) -> Result<Parsed> {
    let lines: Vec<&str> = content
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}'))
        .collect();
    if let Some((h, cols)) = find_header(&lines, &[&["scheme"], &["close"], &["transactions"]]) {
        return Ok(parse_cas_summary_csv(&lines, h, &cols));
    }
    let Some((h, cols)) = find_header(
        &lines,
        &[&["scheme"], &["units"], &["balance"], &["amount"]],
    ) else {
        bail!(
            "Could not detect casparser CSV columns (scheme, isin, amfi, units, balance, amount). \
             Export with `casparser CAS.pdf -o cas.csv`, or use JSON."
        );
    };
    let col = |name: &str| cols.iter().position(|c| c == name);
    let (folio_c, scheme_c, isin_c, amfi_c) =
        (col("folio"), col("scheme"), col("isin"), col("amfi"));
    let date_c = col("date");
    let (units_c, balance_c, amount_c, nav_c, type_c) = (
        col("units"),
        col("balance"),
        col("amount"),
        col("nav"),
        col("type"),
    );

    // Keyed by folio + scheme, in first-seen order.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut books: std::collections::HashMap<
        (String, String),
        (CostBook, Option<f64>, Option<String>, Option<String>),
    > = Default::default();
    for line in &lines[h + 1..] {
        let f = parse_csv_row(line);
        let get = |c: Option<usize>| {
            c.and_then(|i| f.get(i))
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let Some(scheme) = get(scheme_c) else {
            continue;
        };
        let key = (get(folio_c).unwrap_or_default(), scheme);
        let entry = books.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            (CostBook::default(), None, get(isin_c), get(amfi_c))
        });
        let num = |c| get(c).and_then(|v| number(&v));
        entry.0.apply(
            get(date_c).as_deref(),
            &get(type_c).unwrap_or_default(),
            num(units_c),
            num(amount_c),
            num(nav_c),
        );
        if let Some(b) = num(balance_c) {
            entry.1 = Some(b);
        }
    }

    let mut out = Parsed::default();
    for key in order {
        let (book, balance, isin, amfi) = &books[&key];
        let what = format!("{} (folio {})", key.1, key.0);
        let units = balance.unwrap_or(book.units);
        let symbol = mf_symbol(amfi.as_deref(), isin.as_deref());
        if units <= 1e-6 {
            // Fully redeemed: the CSV has no opening balance, but a sale of
            // units the file never bought marks the book as incomplete.
            redeemed_scheme(
                &mut out,
                book,
                true,
                what,
                symbol,
                isin.clone(),
                key.1.clone(),
            );
            continue;
        }
        let Some(symbol) = symbol else {
            out.skipped.push(Skipped {
                what,
                why: "no AMFI code or ISIN".into(),
            });
            continue;
        };
        // Opening units of unknown cost (a period statement): quantity only.
        out.rows.push(ParsedRow {
            symbol,
            isin: isin.clone(),
            name: Some(key.1.clone()),
            qty: units,
            avg_cost: book.avg_for(units),
            unit: "units",
            history: book.history_for(units),
        });
    }
    Ok(out)
}

/// casparser JSON or CSV, by content.
pub(super) fn parse_cas(content: &str) -> Result<Parsed> {
    let trimmed = content.trim_start_matches('\u{feff}').trim_start();
    if trimmed.starts_with("%PDF") {
        bail!(
            "This is the CAS PDF itself. StockWise reads casparser's output: install it \
             (`pip install casparser`), run `casparser CAS.pdf -p <password> -o cas.json`, \
             then `stockwise import cas cas.json`."
        );
    }
    if trimmed.starts_with('{') {
        parse_cas_json(trimmed)
    } else {
        parse_cas_csv(trimmed)
    }
}

/// Look up the NSE (else BSE) ticker for an ISIN via Yahoo search. Only a
/// single unambiguous equity result is accepted. `Ok(None)` means the search
/// worked and found no listing; a failed search is an error, so a network
/// blip never changes what a row is imported as.
async fn ticker_for_isin(yahoo: &YahooClient, isin: &str) -> Result<Option<String>> {
    let hits = yahoo.search(isin).await?;
    let pick = |suffix: &str| {
        let mut found = hits.iter().filter_map(|h| {
            let s = h.symbol.as_deref()?;
            let equity = h
                .quote_type
                .as_deref()
                .is_none_or(|t| t.eq_ignore_ascii_case("EQUITY") || t.eq_ignore_ascii_case("ETF"));
            (equity && s.ends_with(suffix)).then(|| s.to_uppercase())
        });
        let first = found.next()?;
        found.all(|s| s == first).then_some(first)
    };
    Ok(pick(".NS").or_else(|| pick(".BO")))
}

/// Turn parsed rows into portfolio import rows. A row with a full history
/// becomes its dated buys and sells (the symbol's lots are rebuilt); a
/// snapshot row becomes one undated row (quantity sync). A snapshot without
/// a cost uses the cost of the holding already tracked, or is skipped.
pub(super) fn to_import_rows(
    portfolio: &Portfolio,
    parsed: &[ParsedRow],
) -> (Vec<ImportRow>, Vec<Skipped>) {
    let mut rows = Vec::new();
    let mut skipped = Vec::new();
    for r in parsed {
        let sym = r.symbol.to_uppercase();
        if !r.history.is_empty() {
            rows.extend(r.history.iter().map(|t| ImportRow {
                symbol: sym.clone(),
                qty: t.qty,
                price: t.price,
                date: Some(t.date.clone()),
                sell: t.sell,
            }));
            continue;
        }
        let existing = portfolio.holding(&sym).map(|h| h.avg_cost);
        let Some(price) = r.avg_cost.or(existing) else {
            skipped.push(Skipped {
                what: r.name.clone().unwrap_or_else(|| sym.clone()),
                why: format!(
                    "the statement has no complete purchase cost (units bought before its period, \
                     or a summary/demat statement) — add it once with `portfolio add {sym} <qty> <cost>`; \
                     later imports keep its quantity in sync"
                ),
            });
            continue;
        };
        rows.push(ImportRow {
            symbol: sym,
            qty: r.qty,
            price,
            date: None,
            sell: false,
        });
    }
    (rows, skipped)
}

/// Resolve ISIN-only rows to tickers, then apply everything through the
/// shared lot-based import (`holdings::apply_import_rows`).
pub(super) async fn import_rows(
    source: &str,
    file: &str,
    replace: bool,
    parsed: Parsed,
) -> Result<()> {
    let Parsed {
        mut rows,
        mut skipped,
    } = parsed;

    // Exports that only carry ISINs (Groww) need a ticker lookup.
    if rows.iter().any(|r| r.symbol.is_empty()) {
        let yahoo = YahooClient::new().await;
        for r in rows.iter_mut().filter(|r| r.symbol.is_empty()) {
            let isin = r.isin.clone().unwrap_or_default();
            let what = r.name.clone().unwrap_or(isin.clone());
            let lookup = match &yahoo {
                Ok(y) => ticker_for_isin(y, &isin).await,
                Err(e) => Err(anyhow::anyhow!("{e:#}")),
            };
            match lookup {
                Ok(Some(t)) => r.symbol = t,
                // INF… ISINs with no exchange listing are mutual funds.
                Ok(None) if isin.starts_with("INF") => {
                    r.symbol = mf_symbol(None, Some(&isin)).unwrap_or_default();
                    r.unit = "units";
                }
                Ok(None) => skipped.push(Skipped {
                    what,
                    why: format!(
                        "could not find one NSE/BSE ticker for ISIN {isin} — add it with `portfolio add`"
                    ),
                }),
                Err(e) => skipped.push(Skipped {
                    what,
                    why: format!("ticker lookup failed ({e:#}) — run the import again later"),
                }),
            }
        }
        rows.retain(|r| !r.symbol.is_empty());
    }

    let portfolio = Portfolio::load()?;
    let (import, not_applied) = to_import_rows(&portfolio, &rows);
    skipped.extend(not_applied);
    for s in &skipped {
        println!("  {} {} — {}", "!".yellow(), s.what, s.why.dimmed());
    }
    if import.iter().any(|r| r.symbol.starts_with("MF:")) {
        println!(
            "  {}",
            "Mutual funds are stored as MF:<AMFI code or ISIN> (units, not shares).".dimmed()
        );
    }
    let redeemed = rows
        .iter()
        .filter(|r| r.qty <= 1e-6 && !r.history.is_empty())
        .count();
    if redeemed > 0 {
        println!(
            "  {}",
            format!("{redeemed} fully redeemed scheme(s) imported as sales (no holding left).")
                .dimmed()
        );
    }
    // CAS rows are already split-free units; nothing to replay.
    super::holdings::apply_import_rows(source, file, import, false, replace).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio as pf;

    #[test]
    fn numbers_in_indian_exports() {
        assert_eq!(number("1,234.50"), Some(1234.5));
        assert_eq!(number("₹ 2,00,000"), Some(200000.0));
        assert_eq!(number("\"12\""), Some(12.0));
        assert_eq!(number("(5.5)"), Some(-5.5));
        assert_eq!(number("-"), None);
        assert_eq!(number(""), None);
    }

    #[test]
    fn isin_shape() {
        assert!(is_isin("INE002A01018"));
        assert!(is_isin("INF179K01UT0"));
        assert!(!is_isin("RELIANCE"));
        assert!(!is_isin("ine002a01018"));
    }

    #[test]
    fn is_isin_does_not_panic_on_non_ascii_cells() {
        // 12 bytes, but byte 2 is inside `₹`: a byte slice would panic.
        assert_eq!("₹12,34,567".len(), 12);
        assert!(!is_isin("₹12,34,567"));
        assert!(!is_isin("ÉÉ123456789"));
        assert!(!is_isin("IN\u{e9}002A01018"));
        assert!(!is_isin(""));
    }

    #[test]
    fn cost_book_average_cost_method() {
        let mut b = CostBook::default();
        b.apply(
            Some("2024-01-02"),
            "PURCHASE",
            Some(10.0),
            Some(1000.0),
            Some(100.0),
        );
        b.apply(
            Some("2024-02-02"),
            "PURCHASE_SIP",
            Some(10.0),
            Some(2000.0),
            Some(200.0),
        );
        b.apply(Some("2024-02-02"), "STAMP_DUTY_TAX", None, Some(0.15), None);
        // avg = 3000.15 / 20; selling 5 keeps the average.
        b.apply(
            Some("2024-03-01"),
            "REDEMPTION",
            Some(-5.0),
            Some(-1250.0),
            Some(250.0),
        );
        assert!((b.units - 15.0).abs() < 1e-9);
        assert!((b.avg().unwrap() - 3000.15 / 20.0).abs() < 1e-9);
        // Stamp duty lands on that day's purchase lot.
        assert!((b.txns[1].price - 2000.15 / 10.0).abs() < 1e-9);
        assert_eq!(b.history_for(15.0).len(), 3);
        assert!(b.history_for(16.0).is_empty());
        b.apply(
            Some("2024-04-01"),
            "REDEMPTION",
            Some(-15.0),
            Some(-4000.0),
            Some(266.0),
        );
        assert_eq!(b.avg(), None);
    }

    #[test]
    fn mf_symbols_prefer_amfi_code() {
        assert_eq!(
            mf_symbol(Some("118955"), Some("INF179K01UT0")).as_deref(),
            Some("MF:118955")
        );
        assert_eq!(
            mf_symbol(None, Some("inf179k01ut0")).as_deref(),
            Some("MF:INF179K01UT0")
        );
        assert_eq!(mf_symbol(Some(" "), None), None);
    }

    fn row(symbol: &str, qty: f64, avg: Option<f64>) -> ParsedRow {
        ParsedRow {
            symbol: symbol.into(),
            isin: None,
            name: None,
            qty,
            avg_cost: avg,
            unit: "shares",
            history: Vec::new(),
        }
    }

    fn apply(portfolio: &mut Portfolio, parsed: &[ParsedRow]) -> Vec<Skipped> {
        let (rows, skipped) = to_import_rows(portfolio, parsed);
        portfolio.apply_import(
            &rows,
            pf::ImportOptions {
                source: "import:cas",
                replace: false,
                tradebook: false,
                splits: &[],
                account: None,
            },
        );
        skipped
    }

    fn holding(portfolio: &Portfolio, sym: &str) -> (f64, f64, usize) {
        let h = portfolio.holding(sym).expect("holding");
        (h.shares, h.avg_cost, h.lots.len())
    }

    #[test]
    fn groww_export_skips_preamble_and_na_rows() {
        let p = parse_groww(include_str!("fixtures/groww_holdings.csv"), Market::In).unwrap();
        assert_eq!(p.rows.len(), 2, "{:?}", p.rows);
        assert!(p.skipped.is_empty(), "{:?}", p.skipped);
        let asian = &p.rows[0];
        // Groww has no ticker column: the ISIN is looked up later.
        assert_eq!(asian.symbol, "");
        assert_eq!(asian.isin.as_deref(), Some("INE021A01026"));
        assert_eq!((asian.qty, asian.avg_cost), (100.0, Some(2410.04)));
        assert_eq!(p.rows[1].isin.as_deref(), Some("INF204KB14I2"));
    }

    #[test]
    fn upstox_export_reads_symbols_and_indian_number_format() {
        let p = parse_upstox(include_str!("fixtures/upstox_holdings.csv"), Market::In).unwrap();
        assert_eq!(p.rows.len(), 2, "{:?}", p.rows);
        assert_eq!(p.rows[0].symbol, "YESBANK.NS");
        assert_eq!(p.rows[1].symbol, "BAJAJ-AUTO.NS");
        assert_eq!((p.rows[1].qty, p.rows[1].avg_cost), (1000.0, Some(8912.4)));
    }

    #[test]
    fn unknown_layout_is_a_clear_error() {
        let err = parse_groww("a,b,c\n1,2,3\n", Market::In)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Groww") && err.contains("CSV"), "{err}");
    }

    #[test]
    fn cas_json_uses_stated_cost_then_transactions() {
        let p = parse_cas(include_str!("fixtures/cas_cams.json")).unwrap();
        // Two holdings, and the fund redeemed in full (kept for its sale).
        assert_eq!(p.rows.len(), 3, "{:?}", p.rows);
        let hdfc = &p.rows[0];
        assert_eq!(hdfc.symbol, "MF:118834");
        assert_eq!(hdfc.qty, 23.412);
        assert!((hdfc.avg_cost.unwrap() - 3499.75 / 23.412).abs() < 1e-9);
        // No valuation.cost and numbers instead of strings (older casparser).
        let ppfas = &p.rows[1];
        assert_eq!(ppfas.symbol, "MF:122639");
        assert!((ppfas.avg_cost.unwrap() - 6000.0 / 100.5).abs() < 1e-9);
        // The liquid fund with units from before the statement is skipped, with a reason.
        assert!(p.rows.iter().all(|r| r.symbol != "MF:119091"));
        assert_eq!(p.skipped.len(), 1, "{:?}", p.skipped);
        assert!(p.skipped[0].what.contains("HDFC Liquid"), "{:?}", p.skipped);
        assert!(
            p.skipped[0].why.contains("fully redeemed"),
            "{:?}",
            p.skipped
        );
        // The transactions cover each balance, so they become dated lots.
        assert_eq!(hdfc.history.len(), 2);
        assert!(hdfc.history[1].sell);
        assert_eq!(ppfas.history[0].date, "2023-05-02");

        let mut portfolio = Portfolio::default();
        apply(&mut portfolio, &p.rows);
        let (units, _, lots) = holding(&portfolio, "MF:118834");
        assert!((units - 23.412).abs() < 1e-9);
        assert_eq!(lots, 1);
        assert_eq!(
            portfolio.holding("MF:118834").unwrap().lots[0].date,
            "2024-04-02"
        );
        // The redemption is a realized sale, for `tax`.
        assert_eq!(portfolio.realized.len(), 2);
        apply(&mut portfolio, &p.rows);
        assert_eq!(portfolio.realized.len(), 2);
        assert!((holding(&portfolio, "MF:118834").0 - 23.412).abs() < 1e-9);
    }

    #[test]
    fn fully_redeemed_scheme_records_its_sale_and_no_holding() {
        let p = parse_cas(include_str!("fixtures/cas_cams.json")).unwrap();
        let axis = p.rows.iter().find(|r| r.symbol == "MF:120465").unwrap();
        assert_eq!((axis.qty, axis.history.len()), (0.0, 2));

        let mut portfolio = Portfolio::default();
        apply(&mut portfolio, &p.rows);
        assert!(portfolio.holding("MF:120465").is_none());
        let sales: Vec<_> = portfolio
            .realized
            .iter()
            .filter(|s| s.symbol == "MF:120465")
            .collect();
        assert_eq!(sales.len(), 1);
        assert_eq!(sales[0].sell_date, "2025-06-10");
        // 100 units: bought for 5,000, sold for 7,000.
        assert!((sales[0].qty - 100.0).abs() < 1e-9);
        assert!((sales[0].sell_price * sales[0].qty - 7000.0).abs() < 1e-6);
        assert!((sales[0].buy_price * sales[0].qty - 5000.0).abs() < 1e-6);
        // Importing again changes nothing.
        apply(&mut portfolio, &p.rows);
        assert_eq!(
            portfolio
                .realized
                .iter()
                .filter(|s| s.symbol == "MF:120465")
                .count(),
            1
        );
        assert!(portfolio.holding("MF:120465").is_none());
    }

    #[test]
    fn fully_redeemed_csv_scheme_records_its_sale() {
        let csv = "amc,folio,pan,scheme,advisor,isin,amfi,date,description,amount,units,nav,balance,type,dividend\n\
X AMC,1,P,X Fund,,INF000000001,1,2024-04-02,Purchase,5000,100,50,100,PURCHASE,\n\
X AMC,1,P,X Fund,,INF000000001,1,2025-06-10,Redemption,7000,-100,70,0,REDEMPTION,\n\
X AMC,1,P,Old Fund,,INF000000002,2,2024-06-01,Redemption,4900,-1,4900,0,REDEMPTION,\n";
        let p = parse_cas(csv).unwrap();
        // Old Fund sold units the file never bought: skipped with a reason.
        assert_eq!(p.rows.len(), 1, "{:?}", p.rows);
        assert_eq!((p.rows[0].symbol.as_str(), p.rows[0].qty), ("MF:1", 0.0));
        assert_eq!(p.skipped.len(), 1, "{:?}", p.skipped);
        assert!(p.skipped[0].why.contains("fully redeemed"));

        let mut portfolio = Portfolio::default();
        apply(&mut portfolio, &p.rows);
        assert_eq!(portfolio.realized.len(), 1);
        assert!(portfolio.holdings.is_empty());
    }

    #[test]
    fn cas_csv_rebuilds_holdings_per_folio() {
        let p = parse_cas(include_str!("fixtures/cas_cams.csv")).unwrap();
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].qty, 23.412);
        let expected = 5000.0 * (23.412 / 33.412);
        assert!((p.rows[0].avg_cost.unwrap() * 23.412 - expected).abs() < 1e-6);
        assert_eq!(
            (p.rows[1].qty, p.rows[1].avg_cost),
            (6.5, Some(1000.0 / 6.5))
        );

        // Two folios of one scheme add up; re-importing never doubles them.
        let mut portfolio = Portfolio::default();
        apply(&mut portfolio, &p.rows);
        apply(&mut portfolio, &p.rows);
        assert_eq!(portfolio.holdings.len(), 1);
        let (units, _, lots) = holding(&portfolio, "MF:118834");
        assert!((units - 29.912).abs() < 1e-9);
        assert_eq!(lots, 2);
    }

    #[test]
    fn units_bought_before_the_statement_period_have_no_cost() {
        // 100 units carried in, 10 bought in the period: averaging only the
        // 10 would misstate the cost of all 110.
        let json = r#"{"folios":[{"folio":"1","schemes":[{"scheme":"X Fund","amfi":"1","isin":"INF000000001",
            "open":"100","close":"110","close_calculated":"110","valuation":{"date":"2025-03-31","nav":"100","value":"11000"},
            "transactions":[{"date":"2024-05-01","description":"Purchase","amount":"1000","units":"10","nav":"100","balance":"110","type":"PURCHASE"}]}]}]}"#;
        let p = parse_cas(json).unwrap();
        assert_eq!((p.rows[0].qty, p.rows[0].avg_cost), (110.0, None));
        assert!(p.rows[0].history.is_empty());

        let csv = "amc,folio,pan,scheme,advisor,isin,amfi,date,description,amount,units,nav,balance,type,dividend\n\
X AMC,1,P,X Fund,,INF000000001,1,2024-05-01,Purchase,1000,10,100,110,PURCHASE,\n";
        let p = parse_cas(csv).unwrap();
        assert_eq!((p.rows[0].qty, p.rows[0].avg_cost), (110.0, None));
    }

    #[test]
    fn cas_summary_csv_has_no_cost() {
        let p = parse_cas(include_str!("fixtures/cas_summary.csv")).unwrap();
        assert_eq!(p.rows.len(), 1);
        assert_eq!((p.rows[0].qty, p.rows[0].avg_cost), (23.412, None));
    }

    #[test]
    fn demat_cas_equities_update_quantities_only() {
        let p = parse_cas(include_str!("fixtures/cas_nsdl.json")).unwrap();
        assert_eq!(p.rows.len(), 3);
        assert_eq!(p.rows[0].symbol, "HDFCBANK.NS");
        assert_eq!(p.rows[0].avg_cost, None);
        assert_eq!(p.rows[2].symbol, "MF:122639");
        assert_eq!(p.rows[2].avg_cost, Some(60.0));

        let mut portfolio = Portfolio::default();
        portfolio.add("HDFCBANK.NS", 90.0, 1500.0);
        let skipped = apply(&mut portfolio, &p.rows);
        // HDFCBANK: quantity synced, cost kept. RELIANCE: no cost, not held → skipped.
        assert_eq!(skipped.len(), 1);
        assert!(
            skipped[0].why.contains("portfolio add RELIANCE.NS"),
            "{:?}",
            skipped
        );
        let (shares, avg, _) = holding(&portfolio, "HDFCBANK.NS");
        assert!((shares - 100.0).abs() < 1e-9);
        assert!((avg - 1500.0).abs() < 1e-9);
        assert!(portfolio.holding("RELIANCE.NS").is_none());
        assert!(portfolio.holding("MF:122639").is_some());
    }

    #[test]
    fn demat_cas_tickers_get_one_correct_suffix() {
        let sym = |s: &str, x: Option<&str>| demat_symbol(s, x);
        // Plain tickers are NSE; an explicit BSE exchange is BSE.
        assert_eq!(sym("tcs", Some("NSE")), "TCS.NS");
        assert_eq!(sym("TCS", None), "TCS.NS");
        assert_eq!(sym("ABCBSE", Some("bse")), "ABCBSE.BO");
        // Numeric scrip codes are BSE, whatever the exchange field says.
        assert_eq!(sym("500325", None), "500325.BO");
        assert_eq!(sym("500325", Some("NSE")), "500325.BO");
        assert_eq!(sym("500325", Some("BSE")), "500325.BO");
        // An existing suffix is not doubled.
        assert_eq!(sym("TCS.NS", None), "TCS.NS");
        assert_eq!(sym("TCS.NS", Some("NSE")), "TCS.NS");
        assert_eq!(sym("500325.BO", None), "500325.BO");
        assert_eq!(sym("500325.NS", None), "500325.BO");
        assert_eq!(sym("RELIANCE.BO", Some("NSE")), "RELIANCE.BO");
        assert_eq!(sym("M&M", None), "M&M.NS");
        assert_eq!(sym("  ", None), "");
    }

    #[test]
    fn demat_cas_json_with_bse_codes_and_suffixed_tickers() {
        let json = r#"{"accounts":[{"equities":[
            {"isin":"INE002A01018","name":"RELIANCE","num_shares":"8","symbol":"500325"},
            {"isin":"INE467B01029","name":"TCS","num_shares":"3","symbol":"TCS.NS","exchange":"NSE"},
            {"isin":"INE000000001","name":"BSE ONLY","num_shares":"5","symbol":"532540.BO"},
            {"isin":"INE000000002","name":"EXPLICIT BSE","num_shares":"2","symbol":"ABCD","exchange":"BSE"}
        ],"mutual_funds":[]}]}"#;
        let p = parse_cas(json).unwrap();
        let symbols: Vec<&str> = p.rows.iter().map(|r| r.symbol.as_str()).collect();
        assert_eq!(symbols, ["500325.BO", "TCS.NS", "532540.BO", "ABCD.BO"]);
    }

    #[test]
    fn reimport_is_idempotent() {
        let rows = vec![
            row("TCS.NS", 10.0, Some(3500.0)),
            row("INFY.NS", 5.0, Some(1500.0)),
        ];
        let mut portfolio = Portfolio::default();
        apply(&mut portfolio, &rows);
        let snapshot = |p: &Portfolio| -> Vec<_> {
            p.holdings
                .iter()
                .map(|h| (h.symbol.clone(), h.shares, h.avg_cost, h.lots.len()))
                .collect()
        };
        let first = snapshot(&portfolio);
        apply(&mut portfolio, &rows);
        assert_eq!(first, snapshot(&portfolio));
    }

    #[test]
    fn pdf_input_explains_casparser() {
        let err = parse_cas("%PDF-1.7 ...").unwrap_err().to_string();
        assert!(err.contains("casparser"), "{err}");
    }

    #[test]
    fn excel_serial_dates_parse_in_tradebooks() {
        // 45366 in Excel's 1900 epoch (1899-12-30 + 45366) = 2024-03-15.
        let d = pf::parse_trade_date("45366", false).unwrap();
        assert_eq!(d.to_string(), "2024-03-15");
    }

    fn mock_yahoo(server_url: &str) -> YahooClient {
        let base = server_url.to_string();
        YahooClient::for_tests_with_rewrite(reqwest::Client::new(), "c", move |url: &str| {
            let parsed = reqwest::Url::parse(url).unwrap();
            format!("{}{}?{}", base, parsed.path(), parsed.query().unwrap_or(""))
        })
    }

    fn dated_buy(symbol: &str) -> ImportRow {
        ImportRow {
            symbol: symbol.into(),
            qty: 73.0,
            price: 4109.58,
            date: Some("2022-06-01".into()),
            sell: false,
        }
    }

    #[tokio::test]
    async fn split_check_skips_a_symbol_yahoo_has_no_chart_for() {
        // Mindtree trades become LTIM.NS, which Yahoo answers with a 404.
        let server = crate::testutil::MockServer::start(vec![(404, "Not Found".into())]).await;
        let client = mock_yahoo(&server.url);
        let rows = vec![dated_buy("LTIM.NS")];
        let sym = "LTIM.NS".to_string();
        let (found, unchecked) =
            super::super::holdings::fetch_import_splits(&client, &rows, &[&sym])
                .await
                .unwrap();
        assert!(found.is_empty());
        assert_eq!(unchecked, vec!["LTIM.NS".to_string()]);
    }

    #[tokio::test]
    async fn split_check_still_fails_on_other_errors() {
        let server = crate::testutil::MockServer::start(vec![(403, "Forbidden".into())]).await;
        let client = mock_yahoo(&server.url);
        let rows = vec![dated_buy("LTIM.NS")];
        let sym = "LTIM.NS".to_string();
        let err = super::super::holdings::fetch_import_splits(&client, &rows, &[&sym])
            .await
            .unwrap_err();
        assert!(
            format!("{err:#}").contains("nothing was changed"),
            "{err:#}"
        );
    }

    #[test]
    fn two_sheet_workbook_imports_only_the_holdings_sheet() {
        // Tab 1 is a mutual-fund sheet (Fund, Units, NAV, ISIN), tab 2 the stocks.
        let sheets =
            crate::xlsx::read_workbook(include_bytes!("../fixtures/holdings_two_sheets.xlsx"))
                .unwrap();
        for source in ["kite", "groww", "upstox", "indmoney", "csv"] {
            let chosen = crate::xlsx::pick_sheet(&sheets, None, |csv| {
                super::holdings::sheet_has_header(source, csv)
            })
            .unwrap();
            assert_eq!(chosen.name, "Equity", "{source}");
        }
        let csv = crate::xlsx::pick_sheet(&sheets, None, |c| {
            super::holdings::sheet_has_header("groww", c)
        })
        .unwrap()
        .to_csv();
        let parsed = parse_groww(&csv, Market::In).unwrap();
        // INFY only: the fund on the other sheet is not read with these columns.
        assert_eq!(parsed.rows.len(), 1, "{:?}", parsed.rows);
        assert_eq!(parsed.rows[0].symbol, "INFY.NS");
        assert_eq!(
            (parsed.rows[0].qty, parsed.rows[0].avg_cost),
            (10.0, Some(1500.0))
        );
        // `--sheet` picks the other one by name; the importer then reports its own error.
        let funds = crate::xlsx::pick_sheet(&sheets, Some("mutual funds"), |_| false).unwrap();
        assert!(parse_groww(&funds.to_csv(), Market::In).is_err());
    }
}
