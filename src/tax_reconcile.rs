//! `tax reconcile`: compare StockWise's realised capital gains with a
//! broker's tax P&L (Zerodha Console, Groww, Upstox; CSV or `.xlsx`), sale
//! by sale, and say why they differ.
//!
//! The broker files share a shape: optional section headings ("Equity -
//! Short Term", "Long Term", "Intraday"), then a header row and one row per
//! sale (or per lot sold). Columns are found by name, with each broker's
//! wording, so a reordered or extended export still reads.
//!
//! Matching works in two passes on the same share (ISIN when both sides
//! have it, else the NSE symbol) and the same sale date: first on the same
//! purchase date too, then — for what is left — on the sale date alone,
//! summing the lots (the two sides split a sale into lots differently, or
//! disagree on the purchase date). What is still left is missing on one
//! side. Read-only: the portfolio is never changed.

use anyhow::{Result, bail};
use chrono::NaiveDate;
use serde::Serialize;

/// Whose tax P&L the file is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Broker {
    Zerodha,
    Groww,
    Upstox,
    /// Any file with the usual columns.
    Other,
}

impl Broker {
    pub fn name(self) -> &'static str {
        match self {
            Self::Zerodha => "Zerodha",
            Self::Groww => "Groww",
            Self::Upstox => "Upstox",
            Self::Other => "broker",
        }
    }

    /// The demat account StockWise records this broker's lots under.
    pub fn account(self) -> Option<&'static str> {
        match self {
            Self::Zerodha => Some("zerodha"),
            Self::Groww => Some("groww"),
            Self::Upstox => Some("upstox"),
            Self::Other => None,
        }
    }
}

/// Holding period as a tax P&L states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Term {
    Short,
    Long,
    /// Bought and sold the same day: speculative income.
    Intraday,
}

impl Term {
    fn parse(text: &str) -> Option<Self> {
        let t = text.to_lowercase();
        if t.contains("intraday") || t.contains("speculat") {
            Some(Self::Intraday)
        } else if t.contains("short") || t.trim() == "st" || t.contains("stcg") {
            Some(Self::Short)
        } else if t.contains("long") || t.trim() == "lt" || t.contains("ltcg") {
            Some(Self::Long)
        } else {
            None
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Short => "ST",
            Self::Long => "LT",
            Self::Intraday => "Intraday",
        }
    }
}

/// One sale (or lot sold) in a broker's tax P&L.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BrokerRow {
    pub symbol: String,
    pub isin: Option<String>,
    pub qty: f64,
    pub buy_date: Option<NaiveDate>,
    pub sell_date: NaiveDate,
    pub buy_value: f64,
    pub sell_value: f64,
    #[serde(skip)]
    pub has_buy_value: bool,
    #[serde(skip)]
    pub has_sell_value: bool,
    /// The profit the broker reports.
    pub profit: f64,
    /// Profit after Sec. 112A grandfathering, where the file has it.
    pub taxable_profit: Option<f64>,
    /// 31 Jan 2018 fair market value (total), where the file has it.
    pub fmv: Option<f64>,
    pub term: Option<Term>,
    /// Brokerage, STT, exchange, SEBI, stamp duty and GST on the trade.
    pub charges: f64,
}

impl BrokerRow {
    /// The gain the broker taxes: grandfathered when given.
    pub fn gain(&self) -> f64 {
        self.taxable_profit.unwrap_or(self.profit)
    }
}

/// Lower-case a header and keep only letters, digits, `&` and `/`.
fn norm(h: &str) -> String {
    let base = h.split('(').next().unwrap_or(h);
    base.to_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '&' | '/' | ' '))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

const SYMBOL: &[&str] = &[
    "symbol",
    "scrip name",
    "scrip",
    "stock name",
    "stock",
    "security name",
    "instrument",
    "name",
];
const ISIN: &[&str] = &["isin", "isin code"];
const QTY: &[&str] = &["quantity", "qty", "sell qty", "sold qty", "units"];
const BUY_DATE: &[&str] = &[
    "buy date",
    "entry date",
    "purchase date",
    "date of purchase",
    "buy trade date",
    "acquisition date",
    "date of acquisition",
];
const SELL_DATE: &[&str] = &[
    "sell date",
    "exit date",
    "sale date",
    "date of sale",
    "sell trade date",
    "date of transfer",
];
const BUY_VALUE: &[&str] = &[
    "buy value",
    "purchase value",
    "buy amount",
    "cost of acquisition",
    "total buy value",
    "buy cost",
];
const BUY_PRICE: &[&str] = &["buy price", "buy rate", "purchase price", "avg buy price"];
const SELL_VALUE: &[&str] = &[
    "sell value",
    "sale value",
    "sell amount",
    "sale consideration",
    "full value of consideration",
    "total sell value",
];
const SELL_PRICE: &[&str] = &["sell price", "sell rate", "sale price"];
const PROFIT: &[&str] = &[
    "profit",
    "realised p&l",
    "realized p&l",
    "p&l",
    "pnl",
    "profit/loss",
    "realised profit",
    "gain/loss",
    "capital gain",
];
const TAXABLE: &[&str] = &["taxable profit", "taxable gain", "taxable p&l"];
const FMV: &[&str] = &[
    "fair market value",
    "fmv",
    "fmv as on 31 jan 2018",
    "fmv on 31/01/2018",
];
const TERM: &[&str] = &[
    "period of holding",
    "term",
    "type",
    "remark",
    "remarks",
    "short term/long term",
    "capital gain type",
    "holding type",
];
const CHARGES: &[&str] = &[
    "brokerage",
    "stt",
    "exchange transaction charges",
    "exchange charges",
    "sebi charges",
    "sebi turnover fees",
    "stamp duty",
    "gst",
    "cgst",
    "sgst",
    "igst",
    "ipft",
    "charges",
    "other charges",
    "total charges",
];

/// Column positions of one header row.
#[derive(Debug, Default)]
struct Columns {
    symbol: Option<usize>,
    isin: Option<usize>,
    qty: Option<usize>,
    buy_date: Option<usize>,
    sell_date: Option<usize>,
    buy_value: Option<usize>,
    buy_price: Option<usize>,
    sell_value: Option<usize>,
    sell_price: Option<usize>,
    profit: Option<usize>,
    taxable: Option<usize>,
    fmv: Option<usize>,
    term: Option<usize>,
    charges: Vec<usize>,
    total_charges: Option<usize>,
}

impl Columns {
    /// The header's columns, when it names at least a share, a quantity and
    /// a sale date.
    fn of(row: &[String]) -> Option<Self> {
        let cells: Vec<String> = row.iter().map(|c| norm(c)).collect();
        let find = |names: &[&str]| cells.iter().position(|c| names.contains(&c.as_str()));
        let mut c = Self {
            symbol: find(SYMBOL),
            isin: find(ISIN),
            qty: find(QTY),
            buy_date: find(BUY_DATE),
            sell_date: find(SELL_DATE),
            buy_value: find(BUY_VALUE),
            buy_price: find(BUY_PRICE),
            sell_value: find(SELL_VALUE),
            sell_price: find(SELL_PRICE),
            profit: find(PROFIT),
            taxable: find(TAXABLE),
            fmv: find(FMV),
            term: find(TERM),
            ..Self::default()
        };
        for (i, cell) in cells.iter().enumerate() {
            if cell == "total charges" {
                c.total_charges = Some(i);
            } else if CHARGES.contains(&cell.as_str()) {
                c.charges.push(i);
            }
        }
        (c.symbol.or(c.isin).is_some() && c.qty.is_some() && c.sell_date.is_some()).then_some(c)
    }
}

/// `1,234.50`, `₹ 1,234.50`, `(1,234.50)` or `-1234.5` as a number.
fn amount(text: &str) -> Option<f64> {
    let t = text.trim();
    let negative = t.starts_with('(') && t.ends_with(')');
    let cleaned: String = t
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | 'e' | 'E'))
        .collect();
    if cleaned.is_empty() || cleaned == "-" {
        return None;
    }
    let v: f64 = cleaned.parse().ok()?;
    v.is_finite().then_some(if negative { -v.abs() } else { v })
}

/// The bare NSE symbol of a broker's scrip: `RELIANCE-EQ`, `RELIANCE.NS` and
/// `reliance` are all `RELIANCE`.
pub fn bare_symbol(symbol: &str) -> String {
    let up = symbol.trim().to_ascii_uppercase();
    let up = up
        .strip_suffix(".NS")
        .or_else(|| up.strip_suffix(".BO"))
        .unwrap_or(&up)
        .to_string();
    for suffix in ["-EQ", "-BE", "-BZ", " EQ"] {
        if let Some(s) = up.strip_suffix(suffix) {
            return s.to_string();
        }
    }
    up
}

/// Which broker a file comes from: named in it, or by its columns.
fn detect(rows: &[Vec<String>]) -> Broker {
    let text: String = rows
        .iter()
        .take(40)
        .flat_map(|r| r.iter())
        .map(|c| c.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    if text.contains("zerodha") || text.contains("console") {
        Broker::Zerodha
    } else if text.contains("groww") {
        Broker::Groww
    } else if text.contains("upstox") || text.contains("rksv") {
        Broker::Upstox
    } else if text.contains("exit date") && text.contains("fair market value") {
        Broker::Zerodha
    } else if text.contains("stock name") && text.contains("realised p&l") {
        Broker::Groww
    } else if text.contains("scrip name") && text.contains("buy rate") {
        Broker::Upstox
    } else {
        Broker::Other
    }
}

/// A broker file read: its sales, and the rows that looked like sales but
/// had no readable gain (shown, never counted as zero).
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedPnl {
    pub broker: Broker,
    pub rows: Vec<BrokerRow>,
    /// "sheet row N (SYMBOL): no profit, taxable profit or buy and sell value".
    pub skipped: Vec<String>,
}

/// The sales in one sheet (or CSV) of a tax P&L given as rows of cells.
/// Section headings set the term of the rows under them when there is no
/// term column; each sheet starts with no section and no header.
fn parse_sheet(
    rows: &[Vec<String>],
    sheet: &str,
    out: &mut Vec<BrokerRow>,
    skipped: &mut Vec<String>,
) {
    let mut cols: Option<Columns> = None;
    let mut section: Option<Term> = None;
    for (n, row) in rows.iter().enumerate() {
        let filled: Vec<&String> = row.iter().filter(|c| !c.trim().is_empty()).collect();
        if filled.is_empty() {
            continue;
        }
        if let Some(c) = Columns::of(row) {
            cols = Some(c);
            continue;
        }
        // A heading: one or two cells of text and no numbers.
        if filled.len() <= 2 && filled.iter().all(|c| amount(c).is_none()) {
            let t = filled
                .iter()
                .map(|c| c.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            if let Some(term) = Term::parse(&t) {
                section = Some(term);
            } else if t.to_lowercase().contains("equity") || t.to_lowercase().contains("mutual") {
                section = None;
            }
            continue;
        }
        let Some(c) = &cols else { continue };
        let cell = |i: Option<usize>| i.and_then(|i| row.get(i)).map(|s| s.trim());
        let Some(sell_date) =
            cell(c.sell_date).and_then(|d| crate::portfolio::parse_trade_date(d, false))
        else {
            continue; // a total or a blank line
        };
        let symbol = cell(c.symbol).unwrap_or("").to_string();
        let isin = cell(c.isin)
            .filter(|s| s.len() == 12)
            .map(|s| s.to_ascii_uppercase());
        if symbol.is_empty() && isin.is_none() {
            continue;
        }
        let Some(qty) = cell(c.qty).and_then(amount).map(f64::abs) else {
            continue;
        };
        let value = |v: Option<usize>, p: Option<usize>| {
            cell(v)
                .and_then(amount)
                .or_else(|| cell(p).and_then(amount).map(|px| px * qty))
        };
        let buy_value = value(c.buy_value, c.buy_price);
        let sell_value = value(c.sell_value, c.sell_price);
        let taxable_profit = cell(c.taxable).and_then(amount);
        // The gain must be read, or worked out from both values: a row with
        // neither is reported, not counted as a zero gain.
        let profit = match (
            cell(c.profit).and_then(amount),
            taxable_profit,
            buy_value,
            sell_value,
        ) {
            (Some(p), _, _, _) => p,
            (None, Some(t), _, _) => t,
            (None, None, Some(b), Some(s)) => s - b,
            _ => {
                let label = if symbol.is_empty() {
                    isin.clone().unwrap_or_default()
                } else {
                    symbol.clone()
                };
                skipped.push(format!(
                    "{sheet} row {} ({label}): no profit, taxable profit or buy and sell value",
                    n + 1
                ));
                continue;
            }
        };
        let charges = match c
            .total_charges
            .and_then(|i| row.get(i))
            .and_then(|s| amount(s))
        {
            Some(total) => total,
            None => c
                .charges
                .iter()
                .filter_map(|i| row.get(*i).and_then(|s| amount(s)))
                .sum(),
        };
        out.push(BrokerRow {
            symbol: bare_symbol(&symbol),
            isin,
            qty,
            buy_date: cell(c.buy_date).and_then(|d| crate::portfolio::parse_trade_date(d, false)),
            sell_date,
            buy_value: buy_value.unwrap_or(0.0),
            sell_value: sell_value.unwrap_or(0.0),
            has_buy_value: buy_value.is_some(),
            has_sell_value: sell_value.is_some(),
            profit,
            taxable_profit,
            fmv: cell(c.fmv).and_then(amount).filter(|v| *v > 0.0),
            term: cell(c.term).and_then(Term::parse).or(section),
            charges,
        });
    }
}

/// The sales in a tax P&L given as rows of cells (a CSV).
pub fn parse_rows(rows: &[Vec<String>]) -> ParsedPnl {
    parse_sheets(&[("CSV".to_string(), rows.to_vec())])
}

/// The sales of every sheet, each parsed on its own.
fn parse_sheets(sheets: &[(String, Vec<Vec<String>>)]) -> ParsedPnl {
    let all: Vec<Vec<String>> = sheets.iter().flat_map(|(_, r)| r.clone()).collect();
    let broker = detect(&all);
    let (mut rows, mut skipped) = (Vec::new(), Vec::new());
    for (name, sheet) in sheets {
        parse_sheet(sheet, name, &mut rows, &mut skipped);
    }
    ParsedPnl {
        broker,
        rows,
        skipped,
    }
}

/// Read a tax P&L file: `.xlsx` (every sheet) or CSV.
pub fn parse_file(bytes: &[u8], name: &str) -> Result<ParsedPnl> {
    let parsed = if name.to_lowercase().ends_with(".xlsx") || bytes.starts_with(b"PK") {
        let sheets: Vec<(String, Vec<Vec<String>>)> = crate::xlsx::read_workbook(bytes)?
            .into_iter()
            .map(|s| (format!("sheet \"{}\"", s.name), s.rows))
            .collect();
        parse_sheets(&sheets)
    } else {
        let rows: Vec<Vec<String>> = String::from_utf8_lossy(bytes)
            .trim_start_matches('\u{feff}')
            .lines()
            .map(crate::brokers::csv_fields)
            .collect();
        parse_rows(&rows)
    };
    if parsed.rows.is_empty() {
        if parsed.skipped.is_empty() {
            bail!(
                "No sales found in {name}: expected a tax P&L with columns such as Symbol, ISIN, \
                 Quantity, Buy Date, Sell Date, Buy Value, Sell Value and Profit"
            );
        }
        bail!(
            "No usable sales found in {name} ({} skipped): {}",
            parsed.skipped.len(),
            parsed.skipped.join("; ")
        );
    }
    Ok(parsed)
}

/// One StockWise sale as reconcile sees it (rupee amounts).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OurSale {
    pub symbol: String,
    pub isin: Option<String>,
    pub qty: f64,
    pub buy_date: NaiveDate,
    pub sell_date: NaiveDate,
    pub cost: f64,
    pub proceeds: f64,
    /// After Sec. 112A grandfathering where it applies.
    pub gain: f64,
    pub term: Term,
    /// The 31 Jan 2018 price was used for the cost.
    pub grandfathered: bool,
    pub charges: f64,
}

/// Why a matched sale differs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Reason {
    /// Quantities differ: a lot is missing, or a split or bonus is not
    /// applied (`portfolio actions --apply`).
    Quantity,
    /// The purchase dates differ (FIFO across accounts, a transfer-in, an
    /// estimated date).
    BuyDate,
    /// Pre-2018 shares: one side uses the 31 Jan 2018 price as cost.
    Grandfathering,
    /// The difference is the trade's charges (STT, brokerage, stamp duty).
    Charges,
    /// The cost differs for another reason.
    Cost,
    /// The sale value differs.
    SaleValue,
    /// One side calls it long-term, the other short-term or intraday.
    Term,
}

impl Reason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Quantity => "quantity differs — a missing lot, or a split/bonus not applied",
            Self::BuyDate => "buy date differs",
            Self::Grandfathering => "grandfathering (31 Jan 2018 FMV) applied differently",
            Self::Charges => "charges counted on one side only",
            Self::Cost => "buy cost differs",
            Self::SaleValue => "sale value differs",
            Self::Term => "short/long-term differs",
        }
    }
}

/// A sale both sides have (possibly several lots on each side).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Matched {
    pub symbol: String,
    pub sell_date: NaiveDate,
    pub buy_date_ours: Option<NaiveDate>,
    pub buy_date_theirs: Option<NaiveDate>,
    pub qty_ours: f64,
    pub qty_theirs: f64,
    pub cost_ours: f64,
    pub cost_theirs: f64,
    pub proceeds_ours: f64,
    pub proceeds_theirs: f64,
    pub gain_ours: f64,
    pub gain_theirs: f64,
    pub difference: f64,
    pub term_ours: Term,
    pub term_theirs: Option<Term>,
    pub reasons: Vec<Reason>,
}

impl Matched {
    pub fn agrees(&self) -> bool {
        self.reasons.is_empty()
    }
}

/// Totals by term, for one side.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize)]
pub struct Totals {
    pub stcg: f64,
    pub ltcg: f64,
    pub intraday: f64,
}

impl Totals {
    fn add(&mut self, term: Option<Term>, gain: f64) {
        match term {
            Some(Term::Long) => self.ltcg += gain,
            Some(Term::Intraday) => self.intraday += gain,
            _ => self.stcg += gain,
        }
    }
}

/// Per-symbol view: each side's gains and the reasons seen.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolSummary {
    pub symbol: String,
    pub ours: Totals,
    pub theirs: Totals,
    pub difference: f64,
    pub reasons: Vec<Reason>,
}

/// The reconciliation.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Reconciliation {
    pub broker: Broker,
    pub fy_label: String,
    pub matched: Vec<Matched>,
    /// Sales StockWise has that the broker file does not.
    pub only_ours: Vec<OurSale>,
    /// Sales in the broker file that StockWise has not recorded.
    pub only_theirs: Vec<BrokerRow>,
    pub ours: Totals,
    pub theirs: Totals,
    pub by_symbol: Vec<SymbolSummary>,
}

impl Reconciliation {
    pub fn agreeing(&self) -> usize {
        self.matched.iter().filter(|m| m.agrees()).count()
    }
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// The share both rows are of: the ISIN when both have one, else the symbol.
fn same_share(a_sym: &str, a_isin: Option<&str>, b_sym: &str, b_isin: Option<&str>) -> bool {
    match (a_isin, b_isin) {
        (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
        _ => bare_symbol(a_sym) == bare_symbol(b_sym),
    }
}

/// Compare `ours` with `theirs` (both for the same financial year).
pub fn reconcile(
    broker: Broker,
    fy_label: &str,
    ours: &[OurSale],
    theirs: &[BrokerRow],
) -> Reconciliation {
    let mut our_left: Vec<bool> = vec![true; ours.len()];
    let mut their_left: Vec<bool> = vec![true; theirs.len()];
    // The StockWise symbol a broker row was matched to (it may name the
    // company or give only an ISIN), so both sides total under one name.
    let mut their_key: Vec<Option<String>> = vec![None; theirs.len()];
    let mut matched = Vec::new();

    // Pass 1: same share, sale date and purchase date. Pass 2: same share
    // and sale date (lots summed).
    for pass in 0..2 {
        for i in 0..ours.len() {
            if !our_left[i] {
                continue;
            }
            let o = &ours[i];
            let key_ours = |s: &OurSale| {
                same_share(&o.symbol, o.isin.as_deref(), &s.symbol, s.isin.as_deref())
                    && s.sell_date == o.sell_date
                    && (pass == 1 || s.buy_date == o.buy_date)
            };
            let key_theirs = |t: &BrokerRow| {
                same_share(&o.symbol, o.isin.as_deref(), &t.symbol, t.isin.as_deref())
                    && t.sell_date == o.sell_date
                    && (pass == 1 || t.buy_date == Some(o.buy_date))
            };
            let theirs_idx: Vec<usize> = (0..theirs.len())
                .filter(|j| their_left[*j] && key_theirs(&theirs[*j]))
                .collect();
            if theirs_idx.is_empty() {
                continue;
            }
            let ours_idx: Vec<usize> = (0..ours.len())
                .filter(|k| our_left[*k] && key_ours(&ours[*k]))
                .collect();
            let os: Vec<&OurSale> = ours_idx.iter().map(|k| &ours[*k]).collect();
            let ts: Vec<&BrokerRow> = theirs_idx.iter().map(|j| &theirs[*j]).collect();
            // An exact-date group that does not account for the whole
            // quantity waits for the sale-date pass, which sums every lot.
            let qty = |a: f64, b: f64| (a - b).abs() <= 1e-6;
            if pass == 0
                && !qty(
                    os.iter().map(|o| o.qty).sum(),
                    ts.iter().map(|t| t.qty).sum(),
                )
            {
                continue;
            }
            matched.push(compare(&os, &ts));
            for k in ours_idx {
                our_left[k] = false;
            }
            for j in theirs_idx {
                their_left[j] = false;
                their_key[j] = Some(bare_symbol(&o.symbol));
            }
        }
    }

    let only_ours: Vec<OurSale> = ours
        .iter()
        .zip(&our_left)
        .filter(|(_, l)| **l)
        .map(|(o, _)| o.clone())
        .collect();
    let only_theirs: Vec<BrokerRow> = theirs
        .iter()
        .zip(&their_left)
        .filter(|(_, l)| **l)
        .map(|(t, _)| t.clone())
        .collect();

    let (mut ot, mut tt) = (Totals::default(), Totals::default());
    let mut by_symbol: Vec<SymbolSummary> = Vec::new();
    let mut entry = |sym: &str| -> usize {
        match by_symbol.iter().position(|s| s.symbol == sym) {
            Some(i) => i,
            None => {
                by_symbol.push(SymbolSummary {
                    symbol: sym.to_string(),
                    ours: Totals::default(),
                    theirs: Totals::default(),
                    difference: 0.0,
                    reasons: Vec::new(),
                });
                by_symbol.len() - 1
            }
        }
    };
    let mut ix = Vec::new();
    for o in ours {
        ot.add(Some(o.term), o.gain);
        let sym_key = bare_symbol(&o.symbol);
        ix.push((entry(&sym_key), true, Some(o.term), o.gain));
    }
    for (j, t) in theirs.iter().enumerate() {
        tt.add(t.term, t.gain());
        // Matched: the StockWise symbol; else a StockWise sale with the same
        // ISIN names it; else the broker's own name.
        let key = their_key[j].clone().unwrap_or_else(|| {
            t.isin
                .as_deref()
                .and_then(|i| {
                    ours.iter()
                        .find(|o| o.isin.as_deref().is_some_and(|x| x.eq_ignore_ascii_case(i)))
                })
                .map(|o| bare_symbol(&o.symbol))
                .unwrap_or_else(|| bare_symbol(&t.symbol))
        });
        ix.push((entry(&key), false, t.term, t.gain()));
    }
    let reason_ix: Vec<(usize, Reason)> = matched
        .iter()
        .flat_map(|m| {
            let canonical = ours
                .iter()
                .find(|o| bare_symbol(&o.symbol) == bare_symbol(&m.symbol))
                .map(|o| bare_symbol(&o.symbol))
                .unwrap_or_else(|| bare_symbol(&m.symbol));
            let i = entry(&canonical);
            m.reasons.iter().map(move |r| (i, *r))
        })
        .collect();
    for (i, is_ours, term, gain) in ix {
        let s = &mut by_symbol[i];
        if is_ours {
            s.ours.add(term, gain);
        } else {
            s.theirs.add(term, gain);
        }
    }
    for (i, r) in reason_ix {
        if !by_symbol[i].reasons.contains(&r) {
            by_symbol[i].reasons.push(r);
        }
    }
    for s in &mut by_symbol {
        let sum = |t: Totals| t.stcg + t.ltcg + t.intraday;
        s.difference = round2(sum(s.ours) - sum(s.theirs));
        for t in [&mut s.ours, &mut s.theirs] {
            *t = Totals {
                stcg: round2(t.stcg),
                ltcg: round2(t.ltcg),
                intraday: round2(t.intraday),
            };
        }
    }
    by_symbol.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    let r2 = |t: Totals| Totals {
        stcg: round2(t.stcg),
        ltcg: round2(t.ltcg),
        intraday: round2(t.intraday),
    };
    matched.sort_by(|a: &Matched, b: &Matched| {
        (a.sell_date, &a.symbol).cmp(&(b.sell_date, &b.symbol))
    });
    Reconciliation {
        broker,
        fy_label: fy_label.to_string(),
        matched,
        only_ours,
        only_theirs,
        ours: r2(ot),
        theirs: r2(tt),
        by_symbol,
    }
}

/// One matched group, with the reasons it differs.
fn compare(os: &[&OurSale], ts: &[&BrokerRow]) -> Matched {
    let sum_o = |f: fn(&OurSale) -> f64| os.iter().map(|o| f(o)).sum::<f64>();
    let sum_t = |f: fn(&BrokerRow) -> f64| ts.iter().map(|t| f(t)).sum::<f64>();
    let one_date = |mut v: Vec<NaiveDate>| {
        v.sort();
        v.dedup();
        (v.len() == 1).then(|| v[0])
    };
    let buy_ours = one_date(os.iter().map(|o| o.buy_date).collect());
    let buy_theirs = one_date(ts.iter().filter_map(|t| t.buy_date).collect());
    let (qty_o, qty_t) = (sum_o(|o| o.qty), sum_t(|t| t.qty));
    let (cost_o, cost_t) = (sum_o(|o| o.cost), sum_t(|t| t.buy_value));
    let (proc_o, proc_t) = (sum_o(|o| o.proceeds), sum_t(|t| t.sell_value));
    let (gain_o, gain_t) = (sum_o(|o| o.gain), sum_t(|t| t.gain()));
    let charges = sum_t(|t| t.charges);
    let term_ours = if os.iter().any(|o| o.term == Term::Intraday) {
        Term::Intraday
    } else if os.iter().all(|o| o.term == Term::Long) {
        Term::Long
    } else {
        Term::Short
    };
    // The term each side gives, compared by quantity per term so the order
    // of the rows does not matter. Broker rows without a term are left out
    // of the comparison.
    let by_term = |pairs: &mut dyn Iterator<Item = (Term, f64)>| {
        let mut q = [0.0f64; 3];
        for (t, qty) in pairs {
            q[t as usize] += qty;
        }
        q
    };
    let ours_q = by_term(&mut os.iter().map(|o| (o.term, o.qty)));
    let theirs_q = by_term(&mut ts.iter().filter_map(|t| Some((t.term?, t.qty))));
    let unknown_q: f64 = ts.iter().filter(|t| t.term.is_none()).map(|t| t.qty).sum();
    let mut known: Vec<Term> = Vec::new();
    for t in ts.iter().filter_map(|t| t.term) {
        if !known.contains(&t) {
            known.push(t);
        }
    }
    let term_theirs = (known.len() == 1 && unknown_q <= 1e-9).then(|| known[0]);
    // With equal quantities the split by term must match; when quantities
    // differ (a missing lot, a split) only which terms appear can be compared.
    let same_qty = (qty_o - qty_t).abs() <= 1e-6;
    let term_differs = (0..3).any(|i| {
        let (o, t) = (ours_q[i] > 1e-9, theirs_q[i] > 1e-9);
        if !same_qty {
            (t && !o) || (unknown_q <= 1e-9 && o && !t)
        } else if unknown_q <= 1e-9 {
            (ours_q[i] - theirs_q[i]).abs() > 1e-6
        } else {
            theirs_q[i] > ours_q[i] + 1e-6
        }
    });
    let tol = |scale: f64| (scale.abs() * 0.0005).max(1.0);
    let close = |a: f64, b: f64, scale: f64| (a - b).abs() <= tol(scale);
    let old = os.iter().any(|o| o.grandfathered)
        || ts.iter().any(|t| {
            t.fmv.is_some() || t.taxable_profit.is_some_and(|x| (x - t.profit).abs() > 0.5)
        })
        || buy_ours.is_some_and(|d| d <= NaiveDate::from_ymd_opt(2018, 1, 31).expect("date"));

    let mut reasons: Vec<Reason> = Vec::new();
    let mut add = |r: Reason| {
        if !reasons.contains(&r) {
            reasons.push(r);
        }
    };
    let qty_differs = (qty_o - qty_t).abs() > 1e-6;
    if qty_differs {
        add(Reason::Quantity);
    }
    let our_dates: std::collections::BTreeSet<NaiveDate> = os.iter().map(|o| o.buy_date).collect();
    let their_dates: std::collections::BTreeSet<NaiveDate> =
        ts.iter().filter_map(|t| t.buy_date).collect();
    if !their_dates.is_empty() && our_dates != their_dates {
        add(Reason::BuyDate);
    }
    // Cost, sale value and gain are each checked: equal gains can hide a
    // cost and a sale value that both differ. A quantity gap explains all
    // three on its own.
    if !qty_differs {
        let grandfathered = old && term_ours == Term::Long;
        let by_charges = |diff: f64| charges > 0.5 && close(diff, charges, proc_o);
        let check_cost = ts.iter().all(|t| t.has_buy_value);
        let check_proc = ts.iter().all(|t| t.has_sell_value);
        let cost_differs = check_cost && !close(cost_o, cost_t, cost_o.max(cost_t));
        let proc_differs = check_proc && !close(proc_o, proc_t, proc_o.max(proc_t));
        if cost_differs {
            add(if by_charges(cost_t - cost_o) {
                Reason::Charges
            } else if grandfathered {
                Reason::Grandfathering
            } else {
                Reason::Cost
            });
        }
        if proc_differs {
            add(if by_charges(proc_o - proc_t) {
                Reason::Charges
            } else {
                Reason::SaleValue
            });
        }
        if !close(gain_o, gain_t, proc_o) && !cost_differs && !proc_differs {
            add(if grandfathered {
                Reason::Grandfathering
            } else if by_charges(gain_o - gain_t) {
                Reason::Charges
            } else {
                // Same cost and value but another gain: the broker's own
                // adjustment, shown as a cost difference.
                Reason::Cost
            });
        }
    }
    if term_differs {
        add(Reason::Term);
    }
    Matched {
        symbol: os
            .first()
            .map(|o| bare_symbol(&o.symbol))
            .unwrap_or_default(),
        sell_date: os.first().map(|o| o.sell_date).unwrap_or_default(),
        buy_date_ours: buy_ours,
        buy_date_theirs: buy_theirs,
        qty_ours: qty_o,
        qty_theirs: qty_t,
        cost_ours: round2(cost_o),
        cost_theirs: round2(cost_t),
        proceeds_ours: round2(proc_o),
        proceeds_theirs: round2(proc_t),
        gain_ours: round2(gain_o),
        gain_theirs: round2(gain_t),
        difference: round2(gain_o - gain_t),
        term_ours,
        term_theirs,
        reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ZERODHA: &str = include_str!("fixtures/tax_pnl/zerodha.csv");
    const GROWW: &str = include_str!("fixtures/tax_pnl/groww.csv");
    const UPSTOX: &str = include_str!("fixtures/tax_pnl/upstox.csv");

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn ours(sym: &str, qty: f64, buy: &str, sell: &str, bp: f64, sp: f64) -> OurSale {
        let (b, s) = (d(buy), d(sell));
        OurSale {
            symbol: format!("{sym}.NS"),
            isin: None,
            qty,
            buy_date: b,
            sell_date: s,
            cost: qty * bp,
            proceeds: qty * sp,
            gain: qty * (sp - bp),
            term: if b == s {
                Term::Intraday
            } else if crate::tax::is_long_term(b, s, 12) {
                Term::Long
            } else {
                Term::Short
            },
            grandfathered: false,
            charges: 0.0,
        }
    }

    #[test]
    fn zerodha_console_sections_and_columns_are_read() {
        let p = parse_file(ZERODHA.as_bytes(), "taxpnl.csv").unwrap();
        let (broker, rows) = (p.broker, p.rows);
        assert!(p.skipped.is_empty());
        assert_eq!(broker, Broker::Zerodha);
        assert_eq!(rows.len(), 4);
        let infy = &rows[0];
        assert_eq!(infy.symbol, "INFY");
        assert_eq!(infy.isin.as_deref(), Some("INE009A01021"));
        assert_eq!(infy.qty, 10.0);
        assert_eq!(infy.buy_date, Some(d("2025-11-03")));
        assert_eq!(infy.sell_date, d("2026-05-12"));
        assert_eq!(infy.term, Some(Term::Short));
        // Console's profit here is net of the trade's charges.
        assert_eq!(infy.profit, 1_468.8);
        assert!(infy.charges > 0.0);
        let old = rows.iter().find(|r| r.symbol == "HDFCBANK").unwrap();
        assert_eq!(old.term, Some(Term::Long));
        assert_eq!(old.fmv, Some(9_480.0));
        assert_eq!(old.taxable_profit, Some(8_520.0));
        assert_eq!(old.gain(), 8_520.0);
    }

    #[test]
    fn groww_and_upstox_layouts_are_read() {
        let p = parse_file(GROWW.as_bytes(), "groww.csv").unwrap();
        let (broker, rows) = (p.broker, p.rows);
        assert_eq!(broker, Broker::Groww);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].symbol, "TATA MOTORS");
        // Groww names the company; its ISIN is what matches StockWise's ticker.
        assert_eq!(rows[0].isin.as_deref(), Some("INE155A01022"));
        assert_eq!(rows[0].term, Some(Term::Short));
        assert_eq!(rows[1].term, Some(Term::Long));
        assert_eq!(rows[1].profit, -1_200.0);

        let p = parse_file(UPSTOX.as_bytes(), "upstox.csv").unwrap();
        let (broker, rows) = (p.broker, p.rows);
        assert_eq!(broker, Broker::Upstox);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].symbol, "ITC");
        // Buy Amount is missing: the rate × quantity.
        assert_eq!(rows[0].buy_value, 50.0 * 410.0);
        assert_eq!(rows[1].term, Some(Term::Intraday));
    }

    #[test]
    fn a_file_without_sales_is_an_error() {
        assert!(parse_file(b"hello,world\n1,2\n", "x.csv").is_err());
    }

    #[test]
    fn matching_sales_agree_and_differences_get_a_reason() {
        let p = parse_file(ZERODHA.as_bytes(), "taxpnl.csv").unwrap();
        let (broker, theirs) = (p.broker, p.rows);
        let ours = vec![
            // Same as Zerodha's INFY row, but Zerodha nets ₹31.20 of charges.
            ours("INFY", 10.0, "2025-11-03", "2026-05-12", 1_500.0, 1_650.0),
            // TCS: bought on another day (an estimated date here).
            ours("TCS", 5.0, "2025-01-15", "2026-06-02", 3_000.0, 3_400.0),
            // HDFCBANK from 2017: StockWise without the 2018 price.
            ours("HDFCBANK", 10.0, "2017-03-01", "2026-07-20", 700.0, 1_800.0),
            // A sale Zerodha does not have (another broker).
            ours("WIPRO", 20.0, "2025-02-01", "2026-08-01", 250.0, 260.0),
        ];
        let r = reconcile(broker, "FY 2026-27", &ours, &theirs);
        assert_eq!(r.matched.len(), 3);
        let by = |s: &str| r.matched.iter().find(|m| m.symbol == s).unwrap();
        assert_eq!(by("INFY").reasons, vec![Reason::Charges]);
        assert!(by("TCS").reasons.contains(&Reason::BuyDate));
        assert_eq!(by("HDFCBANK").reasons, vec![Reason::Grandfathering]);
        assert_eq!(r.only_ours.len(), 1);
        assert_eq!(r.only_ours[0].symbol, "WIPRO.NS");
        // Zerodha's RELIANCE sale is not in StockWise.
        assert_eq!(r.only_theirs.len(), 1);
        assert_eq!(r.only_theirs[0].symbol, "RELIANCE");
        // Totals by term on both sides.
        assert_eq!(r.ours.ltcg, 2_000.0 + 11_000.0 + 200.0);
        assert_eq!(r.ours.stcg, 1_500.0);
        assert!(r.theirs.ltcg > 0.0 && r.theirs.stcg > 0.0);
        let infy = r.by_symbol.iter().find(|s| s.symbol == "INFY").unwrap();
        assert_eq!(infy.reasons, vec![Reason::Charges]);
    }

    #[test]
    fn identical_sales_agree_and_split_lots_are_summed() {
        let theirs = vec![BrokerRow {
            symbol: "ITC".into(),
            isin: None,
            qty: 30.0,
            buy_date: None,
            sell_date: d("2026-06-01"),
            buy_value: 30.0 * 400.0,
            sell_value: 30.0 * 450.0,
            has_buy_value: true,
            has_sell_value: true,
            profit: 30.0 * 50.0,
            taxable_profit: None,
            fmv: None,
            term: Some(Term::Short),
            charges: 0.0,
        }];
        // StockWise sold the 30 out of two lots.
        let ours = vec![
            ours("ITC", 10.0, "2026-01-02", "2026-06-01", 400.0, 450.0),
            ours("ITC", 20.0, "2026-02-02", "2026-06-01", 400.0, 450.0),
        ];
        let r = reconcile(Broker::Other, "FY 2026-27", &ours, &theirs);
        assert_eq!(r.matched.len(), 1);
        assert!(r.matched[0].agrees(), "{:?}", r.matched[0].reasons);
        assert_eq!(r.agreeing(), 1);
        assert!(r.only_ours.is_empty() && r.only_theirs.is_empty());
    }

    #[test]
    fn a_quantity_gap_points_at_a_missing_lot_or_split() {
        let theirs = vec![BrokerRow {
            symbol: "NESTLEIND".into(),
            isin: None,
            qty: 20.0,
            buy_date: Some(d("2023-01-02")),
            sell_date: d("2026-06-01"),
            buy_value: 20.0 * 1_000.0,
            sell_value: 20.0 * 1_250.0,
            has_buy_value: true,
            has_sell_value: true,
            profit: 5_000.0,
            taxable_profit: None,
            fmv: None,
            term: Some(Term::Long),
            charges: 0.0,
        }];
        // A 10:1 split StockWise has not applied: 2 shares at ₹10,000.
        let ours = vec![ours(
            "NESTLEIND",
            2.0,
            "2023-01-02",
            "2026-06-01",
            10_000.0,
            12_500.0,
        )];
        let r = reconcile(Broker::Other, "FY 2026-27", &ours, &theirs);
        assert_eq!(r.matched[0].reasons, vec![Reason::Quantity]);
    }

    #[test]
    fn a_term_disagreement_is_flagged() {
        let theirs = vec![BrokerRow {
            symbol: "SBIN".into(),
            isin: None,
            qty: 1.0,
            buy_date: Some(d("2025-06-01")),
            sell_date: d("2026-06-01"),
            buy_value: 800.0,
            sell_value: 900.0,
            has_buy_value: true,
            has_sell_value: true,
            profit: 100.0,
            taxable_profit: None,
            fmv: None,
            term: Some(Term::Long),
            charges: 0.0,
        }];
        let ours = vec![ours("SBIN", 1.0, "2025-06-01", "2026-06-01", 800.0, 900.0)];
        let r = reconcile(Broker::Other, "FY 2026-27", &ours, &theirs);
        // Held exactly 12 months is short-term: more than 12 are needed.
        assert_eq!(r.matched[0].reasons, vec![Reason::Term]);
    }

    fn row(
        sym: &str,
        qty: f64,
        buy: Option<&str>,
        sell: &str,
        bv: f64,
        sv: f64,
        term: Option<Term>,
    ) -> BrokerRow {
        BrokerRow {
            symbol: sym.into(),
            isin: None,
            qty,
            buy_date: buy.map(d),
            sell_date: d(sell),
            buy_value: bv,
            sell_value: sv,
            has_buy_value: true,
            has_sell_value: true,
            profit: sv - bv,
            taxable_profit: None,
            fmv: None,
            term,
            charges: 0.0,
        }
    }

    #[test]
    fn each_sheet_starts_without_the_previous_sections_term() {
        let header = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cols = [
            "Symbol",
            "Quantity",
            "Buy Date",
            "Sell Date",
            "Buy Value",
            "Sell Value",
        ];
        let long = vec![
            header(&["Equity - Long Term"]),
            header(&cols),
            header(&["TCS", "1", "2024-01-02", "2026-06-02", "3000", "3400"]),
        ];
        let plain = vec![
            header(&cols),
            header(&["ITC", "1", "2026-01-02", "2026-06-02", "400", "450"]),
        ];
        let p = parse_sheets(&[("a".into(), long), ("b".into(), plain)]);
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].term, Some(Term::Long));
        assert_eq!(p.rows[1].term, None);
    }

    #[test]
    fn a_row_without_a_readable_gain_is_reported_not_zero() {
        let csv = "Symbol,Quantity,Sell Date,Profit,Buy Value,Sell Value\n\
                   ITC,10,2026-06-02,,,\n\
                   TCS,1,2026-06-02,0,,\n\
                   INFY,1,2026-06-02,,1500,1600\n";
        let p = parse_file(csv.as_bytes(), "x.csv").unwrap();
        assert_eq!(p.rows.len(), 2);
        assert_eq!(p.rows[0].profit, 0.0, "a reported zero is kept");
        assert_eq!(p.rows[1].profit, 100.0);
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].contains("row 2 (ITC)"), "{:?}", p.skipped);
    }

    #[test]
    fn a_partial_exact_date_match_waits_for_the_whole_sale() {
        // The broker reports one 30-share row with the first buy date.
        let theirs = vec![row(
            "ITC",
            30.0,
            Some("2026-01-02"),
            "2026-06-01",
            12_000.0,
            13_500.0,
            Some(Term::Short),
        )];
        let ours = vec![
            ours("ITC", 10.0, "2026-01-02", "2026-06-01", 400.0, 450.0),
            ours("ITC", 20.0, "2026-02-02", "2026-06-01", 400.0, 450.0),
        ];
        let r = reconcile(Broker::Other, "FY 2026-27", &ours, &theirs);
        assert_eq!(r.matched.len(), 1);
        assert_eq!(r.matched[0].qty_ours, 30.0);
        assert_eq!(r.matched[0].reasons, vec![Reason::BuyDate]);
        assert!(r.only_ours.is_empty());
    }

    #[test]
    fn equal_gains_with_different_cost_and_value_still_differ() {
        let theirs = vec![row(
            "ITC",
            1.0,
            None,
            "2026-06-01",
            200.0,
            250.0,
            Some(Term::Short),
        )];
        let ours = vec![ours("ITC", 1.0, "2026-01-02", "2026-06-01", 100.0, 150.0)];
        let r = reconcile(Broker::Other, "FY 2026-27", &ours, &theirs);
        assert!(!r.matched[0].agrees());
        assert_eq!(r.matched[0].reasons, vec![Reason::Cost, Reason::SaleValue]);
    }

    #[test]
    fn term_comparison_does_not_depend_on_row_order() {
        let ours = vec![
            ours("ITC", 10.0, "2024-01-02", "2026-06-01", 400.0, 450.0),
            ours("ITC", 10.0, "2026-01-02", "2026-06-01", 400.0, 450.0),
        ];
        let lt = row(
            "ITC",
            10.0,
            None,
            "2026-06-01",
            4_000.0,
            4_500.0,
            Some(Term::Long),
        );
        let st = row(
            "ITC",
            10.0,
            None,
            "2026-06-01",
            4_000.0,
            4_500.0,
            Some(Term::Short),
        );
        for theirs in [vec![lt.clone(), st.clone()], vec![st, lt]] {
            let r = reconcile(Broker::Other, "FY 2026-27", &ours, &theirs);
            assert!(r.matched[0].agrees(), "{:?}", r.matched[0].reasons);
        }
    }

    #[test]
    fn a_company_name_matched_by_isin_totals_under_the_ticker() {
        let mut t = row(
            "TATA MOTORS",
            15.0,
            None,
            "2026-06-15",
            13_500.0,
            14_250.0,
            Some(Term::Short),
        );
        t.isin = Some("INE155A01022".into());
        let mut o = ours("TATAMOTORS", 15.0, "2026-01-10", "2026-06-15", 900.0, 950.0);
        o.isin = Some("INE155A01022".into());
        let r = reconcile(Broker::Groww, "FY 2026-27", &[o], &[t]);
        assert_eq!(r.matched.len(), 1);
        assert_eq!(r.by_symbol.len(), 1);
        assert_eq!(r.by_symbol[0].symbol, "TATAMOTORS");
        assert_eq!(r.by_symbol[0].difference, 0.0);
    }

    #[test]
    fn missing_buy_or_sell_value_with_profit_does_not_flag_false_cost_or_proceeds_mismatch() {
        let rows = vec![
            vec![
                "Symbol".into(),
                "Quantity".into(),
                "Sell Date".into(),
                "Profit".into(),
                "Term".into(),
            ],
            vec![
                "TCS".into(),
                "10".into(),
                "2026-06-15".into(),
                "500".into(),
                "Short Term".into(),
            ],
        ];
        let parsed = parse_rows(&rows);
        assert!(parsed.skipped.is_empty());
        assert!(!parsed.rows[0].has_buy_value && !parsed.rows[0].has_sell_value);
        let ours = vec![ours("TCS", 10.0, "2026-01-10", "2026-06-15", 300.0, 350.0)];
        let r = reconcile(Broker::Other, "FY 2026-27", &ours, &parsed.rows);
        assert!(r.matched[0].agrees(), "{:?}", r.matched[0].reasons);
    }
}
