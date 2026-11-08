//! Quarterly and annual financial results for one symbol, and the
//! earnings-season scoreboard across many.
//!
//! Sources (the first that answers wins, later ones only add fields):
//! - Yahoo `fundamentals-timeseries`, asked with explicit period timestamps
//!   (`quoteSummary` carries only four quarters),
//! - Yahoo `earningsHistory` for the EPS surprise,
//! - NSE financial-results filings for the bank asset-quality rows
//!   (best effort, India only),
//! - `events.rs` for the result date and the price reaction around it.
//!
//! Every fetched period is kept in the `financial_results` table of
//! `cache.db`, so a repeat call within [`CACHE_TTL_SECS`] sends no request.
//! Nothing here gives a view on whether to buy or sell.

use crate::api::YahooClient;
use crate::db::{CacheDb, L2};
use crate::events::{self, Kind};
use crate::market::{self, Market};
use crate::output::{CsvRow, round_opt};
use crate::providers::edgar;
use crate::research::{self, Needs, Series};
use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

/// How long cached results are served without asking the sources again.
pub const CACHE_TTL_SECS: i64 = 6 * 3600;

/// Surprise within this fraction of the estimate is reported as in-line.
pub const INLINE_BAND: f64 = 0.02;

/// How far back (days) the result-date and price-reaction lookup reaches.
const REACTION_DAYS: i64 = 730;

/// Quarter-end dates may drift by this many days between years (52/53-week
/// calendars).
const DATE_TOLERANCE: i64 = 20;

// ───────────────────────────────────────────────────────────────
// Data
// ───────────────────────────────────────────────────────────────

/// EPS against the consensus the quarter was reported into.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EpsSurprise {
    pub actual: f64,
    pub estimate: f64,
    /// Fraction of the estimate (0.05 = 5% above).
    pub surprise: f64,
    /// `beat`, `in-line` or `miss`.
    pub outcome: String,
}

/// Move around the result date, stock and excess over the benchmark.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ResultReaction {
    pub session: NaiveDate,
    pub one_day: f64,
    pub one_day_excess: Option<f64>,
    pub five_day: Option<f64>,
    pub five_day_excess: Option<f64>,
}

/// One quarter or fiscal year. Money is in the statement currency.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ResultPeriod {
    pub end: NaiveDate,
    pub revenue: Option<f64>,
    pub operating_profit: Option<f64>,
    pub other_income: Option<f64>,
    pub interest: Option<f64>,
    pub depreciation: Option<f64>,
    pub pbt: Option<f64>,
    pub tax: Option<f64>,
    pub pat: Option<f64>,
    pub eps: Option<f64>,
    /// Net interest income (banks and lenders).
    pub nii: Option<f64>,
    /// Gross / net NPA, percent of advances (banks, from the NSE filing).
    pub gnpa_pct: Option<f64>,
    pub nnpa_pct: Option<f64>,
    pub ocf: Option<f64>,
    pub capex: Option<f64>,
    pub fcf: Option<f64>,
    pub equity: Option<f64>,
    pub total_assets: Option<f64>,
    pub current_liabilities: Option<f64>,
    pub ebit: Option<f64>,
    pub surprise: Option<EpsSurprise>,
    pub reaction: Option<ResultReaction>,
    /// Day the result was announced, when `events` found it (unix seconds).
    pub result_ts: Option<i64>,
    /// Currency the amounts are reported in.
    #[serde(default)]
    pub currency: Option<String>,
    /// The result-date and reaction lookup ran (found or not).
    #[serde(default)]
    pub reaction_checked: bool,
    /// Filled from SEC EDGAR filings (the rest is Yahoo).
    #[serde(default)]
    pub edgar: bool,
    /// EDGAR was asked for this symbol (found or not), so a cached copy
    /// is complete for requests that use it.
    #[serde(default)]
    pub edgar_checked: bool,
    #[serde(skip)]
    ebitda: Option<f64>,
    #[serde(skip)]
    op_income: Option<f64>,
}

impl ResultPeriod {
    /// An empty period ending on `end`.
    fn new(end: NaiveDate) -> Self {
        Self {
            end,
            ..Default::default()
        }
    }

    /// Operating profit over revenue, as a fraction.
    pub fn opm(&self) -> Option<f64> {
        ratio(self.operating_profit, self.revenue)
    }

    /// Tax over pre-tax profit, as a fraction; none for a loss.
    pub fn tax_rate(&self) -> Option<f64> {
        ratio(self.tax, self.pbt).filter(|_| self.pbt.is_some_and(|p| p > 0.0))
    }

    /// Derive operating profit and free cash flow once all fields are read.
    fn finalize(&mut self) {
        // Operating profit before other income, as Screener shows it.
        self.operating_profit = match (self.op_income, self.depreciation) {
            (Some(o), Some(d)) => Some(o + d),
            _ => self.ebitda.or(self.op_income),
        };
        if self.fcf.is_none()
            && let (Some(o), Some(c)) = (self.ocf, self.capex)
        {
            self.fcf = Some(o - c.abs());
        }
    }
}

/// `n / d`, none unless `d` is positive.
fn ratio(n: Option<f64>, d: Option<f64>) -> Option<f64> {
    match (n, d) {
        (Some(n), Some(d)) if d > 0.0 => Some(n / d),
        _ => None,
    }
}

/// `cur / prev - 1`; none when `prev` is not positive (a swing from a loss
/// has no meaningful percentage).
pub fn growth(cur: Option<f64>, prev: Option<f64>) -> Option<f64> {
    match (cur, prev) {
        (Some(c), Some(p)) if p > 0.0 => Some(c / p - 1.0),
        _ => None,
    }
}

// ───────────────────────────────────────────────────────────────
// Yahoo timeseries
// ───────────────────────────────────────────────────────────────

/// Timeseries fields (each asked as `quarterly…` and `annual…`).
pub const FIELDS: &[&str] = &[
    "TotalRevenue",
    "EBITDA",
    "OperatingIncome",
    "OtherIncomeExpense",
    "InterestExpense",
    "ReconciledDepreciation",
    "PretaxIncome",
    "TaxProvision",
    "NetIncome",
    "DilutedEPS",
    "NetInterestIncome",
    "OperatingCashFlow",
    "CapitalExpenditure",
    "FreeCashFlow",
    "StockholdersEquity",
    "TotalAssets",
    "CurrentLiabilities",
    "EBIT",
];

/// Store one Yahoo timeseries value on the field it names.
fn set_field(p: &mut ResultPeriod, field: &str, v: f64) {
    match field {
        "TotalRevenue" => p.revenue = Some(v),
        "EBITDA" => p.ebitda = Some(v),
        "OperatingIncome" => p.op_income = Some(v),
        "OtherIncomeExpense" => p.other_income = Some(v),
        "InterestExpense" => p.interest = Some(v),
        "ReconciledDepreciation" => p.depreciation = Some(v),
        "PretaxIncome" => p.pbt = Some(v),
        "TaxProvision" => p.tax = Some(v),
        "NetIncome" => p.pat = Some(v),
        "DilutedEPS" => p.eps = Some(v),
        "NetInterestIncome" => p.nii = Some(v),
        "OperatingCashFlow" => p.ocf = Some(v),
        "CapitalExpenditure" => p.capex = Some(v),
        "FreeCashFlow" => p.fcf = Some(v),
        "StockholdersEquity" => p.equity = Some(v),
        "TotalAssets" => p.total_assets = Some(v),
        "CurrentLiabilities" => p.current_liabilities = Some(v),
        "EBIT" => p.ebit = Some(v),
        _ => {}
    }
}

/// Quarters and fiscal years (oldest first) with the statement currency.
#[derive(Debug, Default)]
pub struct Statements {
    pub quarterly: Vec<ResultPeriod>,
    pub annual: Vec<ResultPeriod>,
    pub currency: Option<String>,
}

/// Parse `fundamentals-timeseries` results. Amounts stay in the currency
/// they were reported in; that currency is returned, not converted.
pub fn parse_statements(results: &[Value]) -> Statements {
    let mut q: BTreeMap<(i32, u32), ResultPeriod> = BTreeMap::new();
    let mut a: BTreeMap<(i32, u32), ResultPeriod> = BTreeMap::new();
    let mut ccy: BTreeMap<String, usize> = BTreeMap::new();
    for r in results {
        let Some(t) = r
            .pointer("/meta/type/0")
            .and_then(|t| t.as_str())
            .filter(|t| !t.is_empty())
        else {
            continue;
        };
        let (map, field) = if let Some(f) = t.strip_prefix("quarterly") {
            (&mut q, f)
        } else if let Some(f) = t.strip_prefix("annual") {
            (&mut a, f)
        } else {
            continue;
        };
        for item in r.get(t).and_then(|x| x.as_array()).into_iter().flatten() {
            let Some(date) = item
                .get("asOfDate")
                .and_then(|d| d.as_str())
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let Some(v) = item
                .pointer("/reportedValue/raw")
                .and_then(|x| x.as_f64())
                .filter(|v| v.is_finite())
            else {
                continue;
            };
            if field != "DilutedEPS"
                && let Some(c) = item.get("currencyCode").and_then(|c| c.as_str())
            {
                *ccy.entry(c.to_string()).or_default() += 1;
            }
            let p = map
                .entry((date.year(), date.month()))
                .or_insert_with(|| ResultPeriod::new(date));
            set_field(p, field, v);
            if field != "DilutedEPS"
                && p.currency.is_none()
                && let Some(c) = item.get("currencyCode").and_then(|c| c.as_str())
            {
                p.currency = Some(c.to_string());
            }
        }
    }
    let fin = |m: BTreeMap<(i32, u32), ResultPeriod>| {
        let mut v: Vec<ResultPeriod> = m.into_values().collect();
        v.iter_mut().for_each(ResultPeriod::finalize);
        // A row with no revenue and no profit is a balance-sheet-only stub.
        v.retain(|p| p.revenue.is_some() || p.pat.is_some());
        v
    };
    Statements {
        quarterly: fin(q),
        annual: fin(a),
        currency: ccy.into_iter().max_by_key(|(_, n)| *n).map(|(c, _)| c),
    }
}

// ───────────────────────────────────────────────────────────────
// Surprise and reaction
// ───────────────────────────────────────────────────────────────

/// Whether two quarter ends are the same quarter, allowing calendar drift.
fn near(a: NaiveDate, b: NaiveDate) -> bool {
    (a - b).num_days().abs() <= DATE_TOLERANCE
}

/// `beat` / `in-line` / `miss` for a surprise given as a fraction.
pub fn outcome(surprise: f64) -> &'static str {
    if surprise > INLINE_BAND {
        "beat"
    } else if surprise < -INLINE_BAND {
        "miss"
    } else {
        "in-line"
    }
}

/// Put each EPS surprise on the quarter it belongs to.
pub fn attach_surprises(quarters: &mut [ResultPeriod], surprises: &[research::Surprise]) {
    for s in surprises {
        if let Some(p) = quarters.iter_mut().find(|p| near(p.end, s.quarter)) {
            p.surprise = Some(EpsSurprise {
                actual: s.actual,
                estimate: s.estimate,
                surprise: s.surprise_pct,
                outcome: outcome(s.surprise_pct).to_string(),
            });
        }
    }
}

/// Return from the close before `session` to the close `sessions - 1`
/// sessions later.
fn window_return(s: &Series, session: NaiveDate, sessions: usize) -> Option<f64> {
    let i = s.index_of(session).filter(|&i| i > 0)?;
    let j = i + sessions - 1;
    (j < s.len()).then(|| s.close[j] / s.close[i - 1] - 1.0)
}

/// 1-day and 5-day reaction (stock, excess over benchmark) around `ts`,
/// measured as `events` measures a result: the session that trades on it.
pub fn reaction_at(
    ts: i64,
    prices: &Series,
    bench: &Series,
    is_india: bool,
) -> Option<ResultReaction> {
    let r = events::reaction_for(ts, prices, bench, is_india)?;
    let five = window_return(prices, r.session, 5);
    let five_bench = window_return(bench, r.session, 5);
    Some(ResultReaction {
        session: r.session,
        one_day: r.stock,
        one_day_excess: r.excess,
        five_day: five,
        five_day_excess: five.zip(five_bench).map(|(s, b)| s - b),
    })
}

/// Match `Results` events to the quarter they report (announced within 80
/// days after the quarter end) and measure the reaction.
pub fn attach_reactions(quarters: &mut [ResultPeriod], r: &research::Research) {
    let evts = events::build_events(r);
    for p in quarters.iter_mut() {
        let Some(e) = evts
            .iter()
            .filter(|e| e.kind == Kind::Results)
            .filter(|e| {
                chrono::DateTime::from_timestamp(e.ts, 0).is_some_and(|d| {
                    let days = (d.date_naive() - p.end).num_days();
                    days > 0 && days <= 80
                })
            })
            .min_by_key(|e| e.ts)
        else {
            continue;
        };
        p.result_ts = Some(e.ts);
        p.reaction = reaction_at(e.ts, &r.prices, &r.bench, r.is_india);
    }
}

// ───────────────────────────────────────────────────────────────
// NSE filings (bank asset quality)
// ───────────────────────────────────────────────────────────────

/// One quarter of NSE's `corporates-financial-results` list: the
/// consolidated filing, else the standalone one, and the standalone one
/// as a second source when the consolidated filing leaves the ratios empty.
#[derive(Clone, Debug, PartialEq)]
pub struct NseFiling {
    pub to_date: NaiveDate,
    pub xbrl: String,
    pub consolidated: bool,
    pub standalone: Option<String>,
}

/// Parse NSE's `30-Sep-2026` dates, in any letter case.
fn nse_date(s: &str) -> Option<NaiveDate> {
    let s = s.split_whitespace().next()?;
    let mut norm = s.to_lowercase();
    if let Some(i) = norm.find('-') {
        let (a, b) = norm.split_at(i + 1);
        let mut c = b.chars();
        if let Some(f) = c.next() {
            norm = format!("{}{}{}", a, f.to_ascii_uppercase(), c.as_str());
        }
    }
    NaiveDate::parse_from_str(&norm, "%d-%b-%Y").ok()
}

/// Quarterly filings with an XBRL file, newest first. NSE lists every
/// filing newest first and gives rows without a file an `.../xbrl/-` link;
/// of several revisions of one filing the first (latest) one is used.
pub fn parse_nse_filings(v: &Value) -> Vec<NseFiling> {
    let rows = v
        .as_array()
        .or_else(|| v.get("data").and_then(|d| d.as_array()));
    // (consolidated, standalone) per quarter end.
    let mut by_date: BTreeMap<NaiveDate, (Option<String>, Option<String>)> = BTreeMap::new();
    for e in rows.into_iter().flatten() {
        let get = |keys: &[&str]| {
            keys.iter()
                .find_map(|k| e.get(*k).and_then(|x| x.as_str()))
                .map(str::trim)
                .filter(|s| !s.is_empty() && *s != "-")
        };
        let Some(to_date) = get(&["toDate", "to_date", "qe_Date"]).and_then(nse_date) else {
            continue;
        };
        let Some(xbrl) = get(&["xbrl", "xbrlLink"]).filter(|x| x.to_lowercase().ends_with(".xml"))
        else {
            continue;
        };
        let consolidated = get(&["consolidated"])
            .map(|c| c.eq_ignore_ascii_case("consolidated"))
            .unwrap_or(false);
        let slot = by_date.entry(to_date).or_default();
        let slot = if consolidated {
            &mut slot.0
        } else {
            &mut slot.1
        };
        slot.get_or_insert_with(|| xbrl.to_string());
    }
    by_date
        .into_iter()
        .rev()
        .filter_map(|(to_date, (cons, solo))| match (cons, solo) {
            (Some(c), solo) => Some(NseFiling {
                to_date,
                xbrl: c,
                consolidated: true,
                standalone: solo,
            }),
            (None, Some(s)) => Some(NseFiling {
                to_date,
                xbrl: s,
                consolidated: false,
                standalone: None,
            }),
            (None, None) => None,
        })
        .collect()
}

/// The first value of the XBRL fact `local_name` in the current-quarter
/// context (`OneD`), whatever its namespace prefix.
fn xbrl_fact(xml: &str, local_name: &str) -> Option<f64> {
    let mut rest = xml;
    while let Some(i) = rest.find('<') {
        rest = &rest[i + 1..];
        let Some(gt) = rest.find('>') else { break };
        let tag = &rest[..gt];
        let name = tag
            .split(|c: char| c.is_whitespace() || c == '/')
            .next()
            .unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or("");
        if local.eq_ignore_ascii_case(local_name)
            && !name.starts_with('/')
            && !tag.ends_with('/')
            && tag.contains("contextRef=\"OneD\"")
        {
            let body = &rest[gt + 1..];
            if let Some(lt) = body.find('<')
                && let Ok(v) = body[..lt].trim().replace(',', "").parse::<f64>()
            {
                return Some(v);
            }
        }
    }
    None
}

/// Gross and net NPA (percent) from a bank's result XBRL.
///
/// NSE's banking format (`in-bse-fin`) files them as fractions in
/// `PercentageOfGrossNpa` and `PercentageOfNpa` (the net ratio): `0.0142`
/// is 1.42%. A consolidated filing can carry `0.00` placeholders instead
/// of the ratios; a bank never has none, so a zero reads as "not filed".
pub fn parse_bank_xbrl(xml: &str) -> (Option<f64>, Option<f64>) {
    let get = |tag: &str| {
        xbrl_fact(xml, tag)
            .filter(|v| *v > 0.0)
            .map(|v| (v * 1e6).round() / 1e4)
    };
    (get("PercentageOfGrossNpa"), get("PercentageOfNpa"))
}

/// Fill GNPA/NNPA on `quarters` from the NSE filings; returns how many were filled.
async fn nse_asset_quality(
    client: &YahooClient,
    base: &str,
    quarters: &mut [ResultPeriod],
) -> Result<usize> {
    let url = format!(
        "https://www.nseindia.com/api/corporates-financial-results?index=equities&symbol={}&period=Quarterly",
        research::url_encode(base)
    );
    let list = tokio::time::timeout(
        Duration::from_secs(4),
        client.nse_get_json(&url, Duration::from_secs(6 * 3600)),
    )
    .await
    .context("NSE results list timed out")??;
    let mut filled = 0;
    for f in parse_nse_filings(&list).into_iter().take(6) {
        let Some(p) = quarters.iter_mut().find(|p| near(p.end, f.to_date)) else {
            continue;
        };
        for url in std::iter::once(&f.xbrl).chain(f.standalone.as_ref()) {
            let xml = match tokio::time::timeout(
                Duration::from_secs(4),
                client.nse_get_text(url, Duration::from_secs(7 * 86_400)),
            )
            .await
            {
                Ok(Ok(x)) => x,
                _ => continue,
            };
            let (g, n) = parse_bank_xbrl(&xml);
            if g.is_some() || n.is_some() {
                p.gnpa_pct = g;
                p.nnpa_pct = n;
                filled += 1;
                break;
            }
        }
    }
    Ok(filled)
}

// ───────────────────────────────────────────────────────────────
// SEC EDGAR merge
// ───────────────────────────────────────────────────────────────

/// Copy what EDGAR reports onto `p`; fields EDGAR lacks stay as they are.
fn apply_edgar(p: &mut ResultPeriod, e: &edgar::Period) {
    let set = |dst: &mut Option<f64>, v: Option<f64>| {
        if v.is_some() {
            *dst = v;
        }
    };
    set(&mut p.revenue, e.revenue);
    set(&mut p.operating_profit, e.operating_income);
    set(&mut p.pat, e.net_income);
    set(&mut p.eps, e.diluted_eps);
    set(&mut p.ocf, e.operating_cash_flow);
    set(&mut p.capex, e.capex);
    // Yahoo's free cash flow belonged to its own cash-flow figures.
    // (`finalize` is not rerun: it would replace the EDGAR operating profit.)
    if let (Some(o), Some(c)) = (e.operating_cash_flow, e.capex) {
        p.fcf = Some(o - c.abs());
    }
    p.edgar = true;
    if p.currency.is_none() {
        p.currency = Some("USD".into());
    }
}

/// Merge EDGAR periods into `rows` (oldest first): EDGAR wins the fields it
/// has, Yahoo keeps the rest, and quarters only EDGAR has are added.
fn merge_edgar_rows(rows: &mut Vec<ResultPeriod>, edgar: &[edgar::Period]) {
    for e in edgar {
        match rows.iter_mut().find(|p| near(p.end, e.end)) {
            Some(p) => apply_edgar(p, e),
            None => {
                let mut p = ResultPeriod::new(e.end);
                apply_edgar(&mut p, e);
                rows.push(p);
            }
        }
    }
    rows.sort_by_key(|p| p.end);
}

fn merge_edgar(st: &mut Statements, e: &edgar::Statements) {
    merge_edgar_rows(&mut st.quarterly, &e.quarterly);
    merge_edgar_rows(&mut st.annual, &e.annual);
    st.currency.get_or_insert_with(|| "USD".into());
}

// ───────────────────────────────────────────────────────────────
// Cache (`financial_results`)
// ───────────────────────────────────────────────────────────────

/// The shared `cache.db`, never opened from unit tests (they inject one).
pub fn default_store() -> Option<L2> {
    #[cfg(test)]
    {
        None
    }
    #[cfg(not(test))]
    {
        crate::db::l2()
    }
}

/// Write every period of `symbol` to `financial_results`.
pub fn store_periods(db: L2, symbol: &str, quarterly: &[ResultPeriod], annual: &[ResultPeriod]) {
    let Ok(db) = db.lock() else { return };
    for (rows, is_annual) in [(quarterly, false), (annual, true)] {
        for p in rows {
            if let Ok(json) = serde_json::to_string(p) {
                let source = if p.edgar { "edgar" } else { "yahoo" };
                let _ = db.put_result(symbol, &p.end.to_string(), is_annual, source, &json);
            }
        }
    }
}

/// Cached periods (oldest first) when the newest write is under `max_age`
/// seconds old and there is at least one quarter.
pub fn load_periods(
    db: &CacheDb,
    symbol: &str,
    max_age: i64,
) -> Option<(Vec<ResultPeriod>, Vec<ResultPeriod>)> {
    let age = db.results_age(symbol).ok().flatten()?;
    if age > max_age {
        return None;
    }
    let (mut q, mut a) = (Vec::new(), Vec::new());
    for (_, is_annual, json) in db.get_results(symbol).ok()? {
        let p: ResultPeriod = serde_json::from_str(&json).ok()?;
        if is_annual { &mut a } else { &mut q }.push(p);
    }
    q.sort_by_key(|p| p.end);
    a.sort_by_key(|p| p.end);
    (!q.is_empty()).then_some((q, a))
}

// ───────────────────────────────────────────────────────────────
// Collection
// ───────────────────────────────────────────────────────────────

/// What the sources gave for one symbol.
#[derive(Debug)]
pub struct Collected {
    pub symbol: String,
    pub name: String,
    /// Currency of the statements (not necessarily of the share price).
    pub currency: Option<String>,
    pub is_india: bool,
    pub is_financial: bool,
    pub quarterly: Vec<ResultPeriod>,
    pub annual: Vec<ResultPeriod>,
    pub sources: Vec<String>,
    pub warnings: Vec<String>,
    pub from_cache: bool,
}

/// Banks, lenders and insurers, where revenue and margin mean something else.
fn financial_sector(sector: &str, industry: &str) -> bool {
    let s = format!("{sector} {industry}").to_lowercase();
    s.contains("financial") || s.contains("bank") || s.contains("insurance") || s.contains("credit")
}

/// Fetch (or read from `store`) the results of one symbol. `reaction`
/// also reads prices, news and exchange filings to date each result and
/// measure the move around it.
pub async fn collect(
    client: &YahooClient,
    symbol: &str,
    market: Market,
    store: Option<L2>,
    reaction: bool,
    resolve: bool,
) -> Result<Collected> {
    collect_with(client, symbol, market, store, reaction, resolve, true).await
}

/// [`collect`], with SEC EDGAR optional: the scoreboard reads only the
/// latest quarter of many symbols, where a multi-megabyte filing history
/// each would cost more than it adds.
async fn collect_with(
    client: &YahooClient,
    symbol: &str,
    market: Market,
    store: Option<L2>,
    reaction: bool,
    resolve: bool,
    use_edgar: bool,
) -> Result<Collected> {
    // Saved holdings and index members are already full symbols (a US
    // holding `AAPL` must not become `AAPL.NS`): only typed ones resolve.
    let symbol = if resolve {
        market::resolve_symbol(symbol, market)
    } else {
        symbol.to_string()
    };
    let base = symbol.split('.').next().unwrap_or(&symbol).to_string();
    let is_india = symbol.ends_with(".NS") || symbol.ends_with(".BO");

    let quote = client
        .get_quote(&[symbol.as_str()])
        .await?
        .into_iter()
        .find(|q| q.is_usable())
        .with_context(|| format!("Symbol '{symbol}' not found"))?;
    let name = quote
        .long_name
        .clone()
        .or(quote.short_name.clone())
        .unwrap_or_else(|| symbol.clone());
    let is_financial = financial_sector(
        quote.sector.as_deref().unwrap_or(""),
        quote.industry.as_deref().unwrap_or(""),
    );
    let is_india = is_india || market::is_inr(quote.currency.as_deref());

    let want_edgar = use_edgar && market == Market::Us && !is_india;
    if let Some(db) = store
        && let Ok(guard) = db.lock()
        && let Some((quarterly, annual)) = load_periods(&guard, &symbol, CACHE_TTL_SECS)
        && (!reaction || quarterly.iter().any(|p| p.reaction_checked))
        && (!want_edgar || quarterly.iter().any(|p| p.edgar_checked))
    {
        let source = if quarterly.iter().any(|p| p.edgar) {
            "cache.db (Yahoo Finance, SEC EDGAR)"
        } else {
            "cache.db (Yahoo Finance)"
        };
        return Ok(Collected {
            currency: quarterly
                .iter()
                .rev()
                .find_map(|p| p.currency.clone())
                .or(quote.currency.clone()),
            symbol,
            name,
            is_india,
            is_financial,
            quarterly,
            annual,
            sources: vec![source.into()],
            warnings: vec![],
            from_cache: true,
        });
    }

    let mut warnings = Vec::new();
    let types: Vec<String> = FIELDS
        .iter()
        .flat_map(|f| [format!("quarterly{f}"), format!("annual{f}")])
        .collect();
    let type_refs: Vec<&str> = types.iter().map(String::as_str).collect();
    let (series, summary, filings) = tokio::join!(
        // Explicit period timestamps: eleven years back, not Yahoo's default
        // handful of quarters.
        client.fundamentals_timeseries(&symbol, &type_refs, 365 * 11),
        client.quote_summary(&symbol, &["earningsHistory"]),
        async {
            if want_edgar {
                edgar::fetch(client, &symbol).await
            } else {
                Ok(None)
            }
        },
    );
    // A source that failed leaves a partial result that is not cached,
    // so a retry can fill it in.
    let mut partial = false;
    let filings = match filings {
        Ok(f) => f,
        Err(e) => {
            warnings.push(format!("SEC EDGAR unavailable: {e:#}"));
            partial = true;
            None
        }
    };
    let mut sources = Vec::new();
    let mut st = match series {
        Ok(series) => {
            sources.push("Yahoo Finance fundamentals-timeseries".to_string());
            parse_statements(&series)
        }
        // EDGAR alone still gives a full history.
        Err(e) if filings.is_some() => {
            warnings.push(format!("Yahoo financial statements unavailable: {e:#}"));
            partial = true;
            Statements::default()
        }
        Err(e) => return Err(e.context("Yahoo financial statements unavailable")),
    };
    if let Some(f) = &filings {
        merge_edgar(&mut st, f);
        sources.push("SEC EDGAR companyfacts (XBRL, 10-K/10-Q)".into());
    }
    match summary {
        Ok(s) => {
            let surprises = research::parse_surprises(&s);
            if !surprises.is_empty() {
                attach_surprises(&mut st.quarterly, &surprises);
                sources.push("Yahoo Finance earningsHistory".into());
            }
        }
        Err(e) => warnings.push(format!("Consensus estimates unavailable: {e:#}")),
    }

    if reaction {
        let needs = Needs {
            statements: false,
            prices: true,
            ownership: false,
            news: true,
            filings: true,
            corporate_actions: false,
        };
        match research::fetch(client, &symbol, market, REACTION_DAYS, needs).await {
            Ok(r) => {
                attach_reactions(&mut st.quarterly, &r);
                st.quarterly
                    .iter_mut()
                    .for_each(|p| p.reaction_checked = true);
            }
            Err(e) => warnings.push(format!(
                "Result dates and price reaction unavailable: {e:#}"
            )),
        }
    }

    if is_india && symbol.ends_with(".NS") && is_financial {
        match nse_asset_quality(client, &base, &mut st.quarterly).await {
            Ok(n) if n > 0 => sources.push("NSE financial-results XBRL (asset quality)".into()),
            Ok(_) => {}
            Err(e) => warnings.push(format!("NSE asset quality unavailable: {e:#}")),
        }
    }

    if st.quarterly.is_empty() {
        anyhow::bail!("No quarterly results found for {symbol}");
    }
    if want_edgar && !partial {
        st.quarterly
            .iter_mut()
            .chain(st.annual.iter_mut())
            .for_each(|p| p.edgar_checked = true);
    }
    if let Some(db) = store
        && !partial
    {
        store_periods(db, &symbol, &st.quarterly, &st.annual);
    }
    Ok(Collected {
        currency: st.currency.or(quote.currency.clone()),
        symbol,
        name,
        is_india,
        is_financial,
        quarterly: st.quarterly,
        annual: st.annual,
        sources,
        warnings,
        from_cache: false,
    })
}

/// Symbols read in parallel for the scoreboard.
const PARALLEL: usize = 6;

/// Collate the latest quarter of every symbol (no price reaction: that
/// needs news and filings per symbol). Symbols that fail are listed, not fatal.
pub async fn season(
    client: &YahooClient,
    symbols: &[String],
    title: &str,
    market: Market,
    store: Option<L2>,
    sort: Sort,
) -> Scoreboard {
    use futures_util::StreamExt;
    let outcomes: Vec<(String, Result<Collected>)> =
        futures_util::stream::iter(symbols.iter().map(|s| async move {
            (
                s.clone(),
                collect_with(client, s, market, store, false, false, false).await,
            )
        }))
        .buffered(PARALLEL)
        .collect()
        .await;
    let (mut ok, mut failed) = (Vec::new(), Vec::new());
    for (s, r) in outcomes {
        match r {
            Ok(c) => ok.push(c),
            Err(_) => failed.push(s),
        }
    }
    scoreboard(title, symbols.len(), failed, &ok, sort)
}

// ───────────────────────────────────────────────────────────────
// Quarterly table
// ───────────────────────────────────────────────────────────────

/// Percent changes, as fractions.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Growth {
    pub revenue: Option<f64>,
    pub operating_profit: Option<f64>,
    pub pat: Option<f64>,
    pub eps: Option<f64>,
}

/// Growth of each headline row of `cur` over `prev`.
fn growth_between(cur: &ResultPeriod, prev: Option<&ResultPeriod>) -> Growth {
    let Some(p) = prev else {
        return Growth::default();
    };
    Growth {
        revenue: growth(cur.revenue, p.revenue),
        operating_profit: growth(cur.operating_profit, p.operating_profit),
        pat: growth(cur.pat, p.pat),
        eps: growth(cur.eps, p.eps),
    }
}

/// The period about `days` before `end`, if one is on hand.
fn find_back(all: &[ResultPeriod], end: NaiveDate, days: i64) -> Option<&ResultPeriod> {
    let target = end - chrono::Duration::days(days);
    all.iter()
        .filter(|p| near(p.end, target))
        .min_by_key(|p| (p.end - target).num_days().abs())
}

/// Difference of two fractions in basis points.
fn bps(cur: Option<f64>, prev: Option<f64>) -> Option<f64> {
    Some((cur? - prev?) * 10_000.0)
}

/// One column of the quarterly table with everything derived.
#[derive(Clone, Debug, Serialize)]
pub struct QuarterView {
    pub quarter: String,
    #[serde(flatten)]
    pub period: ResultPeriod,
    pub opm_pct: Option<f64>,
    pub tax_pct: Option<f64>,
    pub yoy: Growth,
    pub qoq: Growth,
    pub opm_bps_yoy: Option<f64>,
    pub opm_bps_qoq: Option<f64>,
}

/// Trailing twelve months: the last four consecutive quarters summed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Ttm {
    pub through: NaiveDate,
    pub revenue: Option<f64>,
    pub operating_profit: Option<f64>,
    pub opm_pct: Option<f64>,
    pub other_income: Option<f64>,
    pub interest: Option<f64>,
    pub depreciation: Option<f64>,
    pub pbt: Option<f64>,
    pub tax_pct: Option<f64>,
    pub pat: Option<f64>,
    pub eps: Option<f64>,
    pub nii: Option<f64>,
}

/// Sum of `f` over the quarters; none if any is missing.
fn sum4(q: &[ResultPeriod], f: impl Fn(&ResultPeriod) -> Option<f64>) -> Option<f64> {
    q.iter().map(f).sum::<Option<f64>>()
}

/// `None` when fewer than four quarters, or when they are not consecutive.
pub fn ttm(quarters: &[ResultPeriod]) -> Option<Ttm> {
    let n = quarters.len();
    if n < 4 {
        return None;
    }
    let last4 = &quarters[n - 4..];
    if last4
        .windows(2)
        .any(|w| !(70..=110).contains(&(w[1].end - w[0].end).num_days()))
    {
        return None;
    }
    let revenue = sum4(last4, |p| p.revenue);
    let operating_profit = sum4(last4, |p| p.operating_profit);
    let pbt = sum4(last4, |p| p.pbt);
    let tax = sum4(last4, |p| p.tax);
    Some(Ttm {
        through: last4[3].end,
        revenue,
        operating_profit,
        opm_pct: ratio(operating_profit, revenue).map(|r| r * 100.0),
        other_income: sum4(last4, |p| p.other_income),
        interest: sum4(last4, |p| p.interest),
        depreciation: sum4(last4, |p| p.depreciation),
        pbt,
        tax_pct: ratio(tax, pbt).map(|r| r * 100.0),
        pat: sum4(last4, |p| p.pat),
        eps: sum4(last4, |p| p.eps),
        nii: sum4(last4, |p| p.nii),
    })
}

/// The last `n` quarters (oldest first) with growth and margin changes
/// computed against all quarters on hand.
pub fn quarter_views(all: &[ResultPeriod], n: usize, is_india: bool) -> Vec<QuarterView> {
    let start = all.len().saturating_sub(n);
    all[start..]
        .iter()
        .map(|p| {
            let yago = find_back(all, p.end, 365);
            let prev = find_back(all, p.end, 91);
            QuarterView {
                quarter: research::quarter_label(p.end, is_india),
                opm_pct: round_opt(p.opm().map(|r| r * 100.0), 2),
                tax_pct: round_opt(p.tax_rate().map(|r| r * 100.0), 2),
                yoy: pct_growth(growth_between(p, yago)),
                qoq: pct_growth(growth_between(p, prev)),
                opm_bps_yoy: round_opt(bps(p.opm(), yago.and_then(|y| y.opm())), 0),
                opm_bps_qoq: round_opt(bps(p.opm(), prev.and_then(|y| y.opm())), 0),
                period: p.clone(),
            }
        })
        .collect()
}

/// Growth fractions as rounded percents.
fn pct_growth(g: Growth) -> Growth {
    let f = |v: Option<f64>| round_opt(v.map(|x| x * 100.0), 2);
    Growth {
        revenue: f(g.revenue),
        operating_profit: f(g.operating_profit),
        pat: f(g.pat),
        eps: f(g.eps),
    }
}

/// Round TTM figures for output.
pub fn ttm_rounded(t: Ttm) -> Ttm {
    Ttm {
        opm_pct: round_opt(t.opm_pct, 2),
        tax_pct: round_opt(t.tax_pct, 2),
        ..t
    }
}

/// The quarterly report as JSON.
pub fn quarterly_json(c: &Collected, n: usize) -> Value {
    let views = quarter_views(&c.quarterly, n, c.is_india);
    serde_json::json!({
        "symbol": c.symbol,
        "name": c.name,
        "currency": c.currency,
        "basis": "as reported by the source (consolidated where the company files it)",
        "sources": c.sources,
        "from_cache": c.from_cache,
        "is_financial": c.is_financial,
        "quarters": views,
        "ttm": ttm(&c.quarterly).map(ttm_rounded),
        "notes": c.warnings,
    })
}

/// Flat CSV row of one quarter.
#[derive(Debug, Serialize)]
pub struct QuarterCsv {
    pub quarter: String,
    pub quarter_end: String,
    pub currency: String,
    pub revenue: Option<f64>,
    pub operating_profit: Option<f64>,
    pub opm_pct: Option<f64>,
    pub other_income: Option<f64>,
    pub interest: Option<f64>,
    pub depreciation: Option<f64>,
    pub pbt: Option<f64>,
    pub tax_pct: Option<f64>,
    pub pat: Option<f64>,
    pub eps: Option<f64>,
    pub revenue_yoy_pct: Option<f64>,
    pub revenue_qoq_pct: Option<f64>,
    pub pat_yoy_pct: Option<f64>,
    pub pat_qoq_pct: Option<f64>,
    pub opm_bps_yoy: Option<f64>,
    pub opm_bps_qoq: Option<f64>,
    pub nii: Option<f64>,
    pub gnpa_pct: Option<f64>,
    pub nnpa_pct: Option<f64>,
    pub eps_estimate: Option<f64>,
    pub eps_surprise_pct: Option<f64>,
    pub outcome: Option<String>,
    pub reaction_1d_excess_pct: Option<f64>,
    pub reaction_5d_excess_pct: Option<f64>,
}

impl CsvRow for QuarterCsv {
    const COLUMNS: &'static [&'static str] = &[
        "quarter",
        "quarter_end",
        "currency",
        "revenue",
        "operating_profit",
        "opm_pct",
        "other_income",
        "interest",
        "depreciation",
        "pbt",
        "tax_pct",
        "pat",
        "eps",
        "revenue_yoy_pct",
        "revenue_qoq_pct",
        "pat_yoy_pct",
        "pat_qoq_pct",
        "opm_bps_yoy",
        "opm_bps_qoq",
        "nii",
        "gnpa_pct",
        "nnpa_pct",
        "eps_estimate",
        "eps_surprise_pct",
        "outcome",
        "reaction_1d_excess_pct",
        "reaction_5d_excess_pct",
    ];
}

/// One flat CSV row per displayed quarter.
pub fn quarterly_csv(c: &Collected, n: usize) -> Vec<QuarterCsv> {
    quarter_views(&c.quarterly, n, c.is_india)
        .into_iter()
        .map(|v| {
            let p = &v.period;
            QuarterCsv {
                quarter: v.quarter.clone(),
                quarter_end: p.end.to_string(),
                currency: c.currency.clone().unwrap_or_default(),
                revenue: p.revenue,
                operating_profit: p.operating_profit,
                opm_pct: v.opm_pct,
                other_income: p.other_income,
                interest: p.interest,
                depreciation: p.depreciation,
                pbt: p.pbt,
                tax_pct: v.tax_pct,
                pat: p.pat,
                eps: p.eps,
                revenue_yoy_pct: v.yoy.revenue,
                revenue_qoq_pct: v.qoq.revenue,
                pat_yoy_pct: v.yoy.pat,
                pat_qoq_pct: v.qoq.pat,
                opm_bps_yoy: v.opm_bps_yoy,
                opm_bps_qoq: v.opm_bps_qoq,
                nii: p.nii,
                gnpa_pct: p.gnpa_pct,
                nnpa_pct: p.nnpa_pct,
                eps_estimate: p.surprise.as_ref().map(|s| s.estimate),
                eps_surprise_pct: round_opt(p.surprise.as_ref().map(|s| s.surprise * 100.0), 2),
                outcome: p.surprise.as_ref().map(|s| s.outcome.clone()),
                reaction_1d_excess_pct: round_opt(
                    p.reaction
                        .as_ref()
                        .and_then(|r| r.one_day_excess)
                        .map(|x| x * 100.0),
                    2,
                ),
                reaction_5d_excess_pct: round_opt(
                    p.reaction
                        .as_ref()
                        .and_then(|r| r.five_day_excess)
                        .map(|x| x * 100.0),
                    2,
                ),
            }
        })
        .collect()
}

// ───────────────────────────────────────────────────────────────
// Annual table
// ───────────────────────────────────────────────────────────────

/// One fiscal year with cash conversion and returns.
#[derive(Clone, Debug, Serialize)]
pub struct AnnualView {
    pub fiscal_year: String,
    #[serde(flatten)]
    pub period: ResultPeriod,
    pub opm_pct: Option<f64>,
    pub tax_pct: Option<f64>,
    pub revenue_yoy_pct: Option<f64>,
    pub pat_yoy_pct: Option<f64>,
    /// Operating cash flow over net profit.
    pub cfo_to_pat_pct: Option<f64>,
    /// Net profit over average equity (year-end when the prior year is missing).
    pub roe_pct: Option<f64>,
    /// EBIT over total assets less current liabilities.
    pub roce_pct: Option<f64>,
}

/// Compound annual growth over `years`, from the fiscal years on hand.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Cagr {
    pub years: u32,
    pub revenue_pct: Option<f64>,
    pub pat_pct: Option<f64>,
    pub eps_pct: Option<f64>,
}

/// `FY26` (India, Apr-Mar) or `FY2026` (calendar year end).
fn fiscal_year_label(end: NaiveDate, is_india: bool) -> String {
    if is_india {
        // FY27 = Apr 2026 to Mar 2027
        let fy = if end.month() <= 3 {
            end.year()
        } else {
            end.year() + 1
        };
        format!("FY{:02}", fy % 100)
    } else {
        format!("FY{}", end.year())
    }
}

/// The last `n` fiscal years with growth, cash conversion and returns.
pub fn annual_views(all: &[ResultPeriod], n: usize, is_india: bool) -> Vec<AnnualView> {
    let start = all.len().saturating_sub(n);
    (start..all.len())
        .map(|i| {
            let p = &all[i];
            let prev = i.checked_sub(1).map(|j| &all[j]);
            let equity = match prev.and_then(|q| q.equity) {
                Some(e0) => p.equity.map(|e1| (e0 + e1) / 2.0),
                None => p.equity,
            };
            let capital = p
                .total_assets
                .zip(p.current_liabilities)
                .map(|(a, l)| a - l);
            AnnualView {
                fiscal_year: fiscal_year_label(p.end, is_india),
                opm_pct: round_opt(p.opm().map(|r| r * 100.0), 2),
                tax_pct: round_opt(p.tax_rate().map(|r| r * 100.0), 2),
                revenue_yoy_pct: round_opt(
                    growth(p.revenue, prev.and_then(|q| q.revenue)).map(|g| g * 100.0),
                    2,
                ),
                pat_yoy_pct: round_opt(
                    growth(p.pat, prev.and_then(|q| q.pat)).map(|g| g * 100.0),
                    2,
                ),
                cfo_to_pat_pct: round_opt(ratio(p.ocf, p.pat).map(|r| r * 100.0), 1),
                roe_pct: round_opt(ratio(p.pat, equity).map(|r| r * 100.0), 2),
                roce_pct: round_opt(ratio(p.ebit, capital).map(|r| r * 100.0), 2),
                period: p.clone(),
            }
        })
        .collect()
}

/// Compound annual growth in percent; none unless both ends are positive.
fn cagr_of(cur: Option<f64>, base: Option<f64>, years: u32) -> Option<f64> {
    match (cur, base) {
        (Some(c), Some(b)) if c > 0.0 && b > 0.0 => {
            Some(((c / b).powf(1.0 / years as f64) - 1.0) * 100.0)
        }
        _ => None,
    }
}

/// 3, 5 and 10 year CAGR ending at the latest fiscal year; years the data
/// does not reach are left out.
pub fn cagrs(all: &[ResultPeriod]) -> Vec<Cagr> {
    let Some(last) = all.last() else {
        return vec![];
    };
    [3u32, 5, 10]
        .into_iter()
        .filter_map(|y| {
            let target = last.end - chrono::Duration::days(365 * y as i64 + (y as i64) / 4);
            let base = all
                .iter()
                .find(|p| (p.end - target).num_days().abs() <= 45)?;
            Some(Cagr {
                years: y,
                revenue_pct: round_opt(cagr_of(last.revenue, base.revenue, y), 2),
                pat_pct: round_opt(cagr_of(last.pat, base.pat, y), 2),
                eps_pct: round_opt(cagr_of(last.eps, base.eps, y), 2),
            })
        })
        .collect()
}

/// The annual report as JSON.
pub fn annual_json(c: &Collected, n: usize) -> Value {
    serde_json::json!({
        "symbol": c.symbol,
        "name": c.name,
        "currency": c.currency,
        "basis": "as reported by the source (consolidated where the company files it)",
        "sources": c.sources,
        "from_cache": c.from_cache,
        "years": annual_views(&c.annual, n, c.is_india),
        "cagr": cagrs(&c.annual),
        "notes": c.warnings,
    })
}

/// Flat CSV row of one fiscal year.
#[derive(Debug, Serialize)]
pub struct AnnualCsv {
    pub fiscal_year: String,
    pub year_end: String,
    pub currency: String,
    pub revenue: Option<f64>,
    pub operating_profit: Option<f64>,
    pub opm_pct: Option<f64>,
    pub pat: Option<f64>,
    pub eps: Option<f64>,
    pub revenue_yoy_pct: Option<f64>,
    pub pat_yoy_pct: Option<f64>,
    pub ocf: Option<f64>,
    pub capex: Option<f64>,
    pub fcf: Option<f64>,
    pub cfo_to_pat_pct: Option<f64>,
    pub roe_pct: Option<f64>,
    pub roce_pct: Option<f64>,
}

impl CsvRow for AnnualCsv {
    const COLUMNS: &'static [&'static str] = &[
        "fiscal_year",
        "year_end",
        "currency",
        "revenue",
        "operating_profit",
        "opm_pct",
        "pat",
        "eps",
        "revenue_yoy_pct",
        "pat_yoy_pct",
        "ocf",
        "capex",
        "fcf",
        "cfo_to_pat_pct",
        "roe_pct",
        "roce_pct",
    ];
}

/// One flat CSV row per displayed fiscal year.
pub fn annual_csv(c: &Collected, n: usize) -> Vec<AnnualCsv> {
    annual_views(&c.annual, n, c.is_india)
        .into_iter()
        .map(|v| AnnualCsv {
            fiscal_year: v.fiscal_year,
            year_end: v.period.end.to_string(),
            currency: c.currency.clone().unwrap_or_default(),
            revenue: v.period.revenue,
            operating_profit: v.period.operating_profit,
            opm_pct: v.opm_pct,
            pat: v.period.pat,
            eps: v.period.eps,
            revenue_yoy_pct: v.revenue_yoy_pct,
            pat_yoy_pct: v.pat_yoy_pct,
            ocf: v.period.ocf,
            capex: v.period.capex,
            fcf: v.period.fcf,
            cfo_to_pat_pct: v.cfo_to_pat_pct,
            roe_pct: v.roe_pct,
            roce_pct: v.roce_pct,
        })
        .collect()
}

// ───────────────────────────────────────────────────────────────
// Season scoreboard
// ───────────────────────────────────────────────────────────────

/// One company in the scoreboard: its latest quarter against a year ago.
#[derive(Clone, Debug, Serialize)]
pub struct Reporter {
    pub symbol: String,
    pub name: String,
    pub quarter: String,
    pub quarter_end: NaiveDate,
    pub revenue_yoy_pct: Option<f64>,
    pub operating_profit_yoy_pct: Option<f64>,
    pub pat_yoy_pct: Option<f64>,
    pub opm_bps_yoy: Option<f64>,
    pub eps_surprise_pct: Option<f64>,
    pub outcome: Option<String>,
}

/// Sum-based growth across companies that reported both periods.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Aggregate {
    pub revenue_yoy_pct: Option<f64>,
    pub operating_profit_yoy_pct: Option<f64>,
    pub pat_yoy_pct: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Scoreboard {
    pub title: String,
    /// The quarter most companies in the set have reported.
    pub quarter_end: Option<NaiveDate>,
    pub quarter: Option<String>,
    pub total: usize,
    pub reported: usize,
    pub pending: Vec<String>,
    pub failed: Vec<String>,
    pub aggregate: Aggregate,
    pub beats: usize,
    pub in_line: usize,
    pub misses: usize,
    pub reporters: Vec<Reporter>,
}

/// Collate the reporters for the quarter that most of them have reached.
/// `total` is the number of symbols asked for; `failed` those that errored.
pub fn scoreboard(
    title: &str,
    total: usize,
    failed: Vec<String>,
    results: &[Collected],
    sort: Sort,
) -> Scoreboard {
    // The active quarter is the latest quarter end shared by at least two
    // symbols (one) when the set is a single symbol.
    let mut ends: Vec<NaiveDate> = results
        .iter()
        .filter_map(|c| c.quarterly.last().map(|p| p.end))
        .collect();
    ends.sort();
    let active = ends
        .iter()
        .rev()
        .find(|e| ends.iter().filter(|o| near(**o, **e)).count() >= 2.min(ends.len()))
        // No shared quarter yet: the newest one, the rest are pending.
        .or(ends.last());
    let mut board = Scoreboard {
        title: title.to_string(),
        total,
        failed,
        ..Default::default()
    };
    let Some(&active) = active else {
        return board;
    };
    board.quarter_end = Some(active);
    let (mut rev, mut op, mut pat) = (Agg::default(), Agg::default(), Agg::default());
    for c in results {
        let Some(cur) = c.quarterly.last() else {
            continue;
        };
        if !near(cur.end, active) {
            board.pending.push(c.symbol.clone());
            continue;
        }
        board.quarter = Some(research::quarter_label(cur.end, c.is_india));
        let yago = find_back(&c.quarterly, cur.end, 365);
        rev.add(cur.revenue, yago.and_then(|y| y.revenue));
        op.add(cur.operating_profit, yago.and_then(|y| y.operating_profit));
        pat.add(cur.pat, yago.and_then(|y| y.pat));
        match cur.surprise.as_ref().map(|s| s.outcome.as_str()) {
            Some("beat") => board.beats += 1,
            Some("miss") => board.misses += 1,
            Some(_) => board.in_line += 1,
            None => {}
        }
        let g = growth_between(cur, yago);
        board.reporters.push(Reporter {
            symbol: c.symbol.clone(),
            name: c.name.clone(),
            quarter: research::quarter_label(cur.end, c.is_india),
            quarter_end: cur.end,
            revenue_yoy_pct: round_opt(g.revenue.map(|x| x * 100.0), 2),
            operating_profit_yoy_pct: round_opt(g.operating_profit.map(|x| x * 100.0), 2),
            pat_yoy_pct: round_opt(g.pat.map(|x| x * 100.0), 2),
            opm_bps_yoy: round_opt(bps(cur.opm(), yago.and_then(|y| y.opm())), 0),
            eps_surprise_pct: round_opt(cur.surprise.as_ref().map(|s| s.surprise * 100.0), 2),
            outcome: cur.surprise.as_ref().map(|s| s.outcome.clone()),
        });
    }
    board.reported = board.reporters.len();
    board.aggregate = Aggregate {
        revenue_yoy_pct: round_opt(rev.growth(), 2),
        operating_profit_yoy_pct: round_opt(op.growth(), 2),
        pat_yoy_pct: round_opt(pat.growth(), 2),
    };
    board.pending.sort();
    sort_reporters(&mut board.reporters, sort);
    board
}

#[derive(Default)]
struct Agg {
    cur: f64,
    prev: f64,
    n: usize,
}

impl Agg {
    /// Only companies with both periods positive in the base count, so the
    /// total is comparable on both sides.
    fn add(&mut self, cur: Option<f64>, prev: Option<f64>) {
        if let (Some(c), Some(p)) = (cur, prev)
            && p > 0.0
        {
            self.cur += c;
            self.prev += p;
            self.n += 1;
        }
    }
    /// Percent growth of the summed current over the summed base.
    fn growth(&self) -> Option<f64> {
        (self.n > 0).then(|| (self.cur / self.prev - 1.0) * 100.0)
    }
}

/// Order of the reporters table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum Sort {
    /// Result date (quarter end), newest first
    Date,
    /// Net profit growth year on year, highest first
    Pat,
    /// Operating margin change year on year, highest first
    Margin,
    /// EPS surprise against consensus, highest first
    Surprise,
}

/// Order the reporters table (newest first, then by the chosen key).
fn sort_reporters(rows: &mut [Reporter], sort: Sort) {
    let key = |r: &Reporter| match sort {
        Sort::Date => None,
        Sort::Pat => r.pat_yoy_pct,
        Sort::Margin => r.opm_bps_yoy,
        Sort::Surprise => r.eps_surprise_pct,
    };
    rows.sort_by(|a, b| {
        let by_date = b.quarter_end.cmp(&a.quarter_end);
        let by_key = match sort {
            Sort::Date => std::cmp::Ordering::Equal,
            _ => key(b)
                .unwrap_or(f64::NEG_INFINITY)
                .total_cmp(&key(a).unwrap_or(f64::NEG_INFINITY)),
        };
        by_key.then(by_date).then(a.symbol.cmp(&b.symbol))
    });
}

/// Flat CSV row of one reporter.
#[derive(Debug, Serialize)]
pub struct ReporterCsv {
    pub symbol: String,
    pub name: String,
    pub quarter: String,
    pub quarter_end: String,
    pub revenue_yoy_pct: Option<f64>,
    pub operating_profit_yoy_pct: Option<f64>,
    pub pat_yoy_pct: Option<f64>,
    pub opm_bps_yoy: Option<f64>,
    pub eps_surprise_pct: Option<f64>,
    pub outcome: Option<String>,
}

impl CsvRow for ReporterCsv {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "quarter",
        "quarter_end",
        "revenue_yoy_pct",
        "operating_profit_yoy_pct",
        "pat_yoy_pct",
        "opm_bps_yoy",
        "eps_surprise_pct",
        "outcome",
    ];
}

/// One flat CSV row per reporter.
pub fn reporters_csv(b: &Scoreboard) -> Vec<ReporterCsv> {
    b.reporters
        .iter()
        .map(|r| ReporterCsv {
            symbol: r.symbol.clone(),
            name: r.name.clone(),
            quarter: r.quarter.clone(),
            quarter_end: r.quarter_end.to_string(),
            revenue_yoy_pct: r.revenue_yoy_pct,
            operating_profit_yoy_pct: r.operating_profit_yoy_pct,
            pat_yoy_pct: r.pat_yoy_pct,
            opm_bps_yoy: r.opm_bps_yoy,
            eps_surprise_pct: r.eps_surprise_pct,
            outcome: r.outcome.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests;
