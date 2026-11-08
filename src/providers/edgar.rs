//! SEC EDGAR (<https://www.sec.gov/edgar>): keyless US filings data.
//!
//! Two files give the financial statements of a US filer: `company_tickers.json`
//! maps a ticker to its CIK, and the XBRL `companyfacts` document holds every
//! fact the company ever tagged in a 10-K or 10-Q. The facts carry a period
//! (`start`..`end`) and the filing they came from, not a "quarter", so the
//! quarters are rebuilt here:
//!
//! - a 10-Q reports the three-month figure and, for cash flow, only the
//!   year-to-date one (six and nine months), so a quarter is the direct
//!   three-month fact or the difference of two year-to-date facts;
//! - Q4 is never reported on its own: it is the fiscal year minus Q1..Q3;
//! - a fact filed again later (a restatement, or the prior-year column of a
//!   newer filing) replaces the older one, so the latest `filed` date wins;
//! - fiscal years are found from the annual facts themselves, so a
//!   September or 52/53-week year end needs no special handling.
//!
//! EDGAR asks for a descriptive `User-Agent` and at most 10 requests a
//! second. Requests go through the shared client, which spaces them 200 ms
//! apart (5 a second) and caches the bodies.

use crate::api::YahooClient;
use anyhow::{Context, Result};
use chrono::{Duration as Days, NaiveDate};
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::Duration;

const TICKERS_URL: &str = "https://www.sec.gov/files/company_tickers.json";
const FACTS_URL: &str = "https://data.sec.gov/api/xbrl/companyfacts/CIK";

/// A generic client name: EDGAR wants to know the tool, and nothing about
/// the person running it is sent.
const USER_AGENT: &str = concat!(
    "stockwise/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/iamsaquib8/stockwise)"
);

/// The ticker list changes rarely.
const TICKERS_TTL: Duration = Duration::from_secs(24 * 3600);
/// Facts change when a filing lands.
const FACTS_TTL: Duration = Duration::from_secs(6 * 3600);

/// Newest quarters and fiscal years kept.
const QUARTERS_KEPT: usize = 24;
const YEARS_KEPT: usize = 10;

/// Days a three-month, six-month, nine-month and twelve-month period may
/// span (52/53-week years drift a few days either way).
const QUARTER_DAYS: (i64, i64) = (80, 100);
const HALF_DAYS: (i64, i64) = (170, 195);
const NINE_MONTH_DAYS: (i64, i64) = (260, 285);
const YEAR_DAYS: (i64, i64) = (350, 380);
/// How far a period start may be from the fiscal-year start.
const START_SLOP: i64 = 5;
/// How far a quarter end may be from where the calendar puts it.
const END_SLOP: i64 = 20;

// Concepts per metric, best first. A company moves between them over the
// years, so every one is read and the better one wins per period.
const REVENUE: &[&str] = &[
    "Revenues",
    "RevenueFromContractWithCustomerExcludingAssessedTax",
    "SalesRevenueNet",
    "RevenueFromContractWithCustomerIncludingAssessedTax",
];
const OPERATING_INCOME: &[&str] = &["OperatingIncomeLoss"];
const NET_INCOME: &[&str] = &["NetIncomeLoss", "ProfitLoss"];
const EPS: &[&str] = &["EarningsPerShareDiluted", "EarningsPerShareBasicAndDiluted"];
const OPERATING_CASH: &[&str] = &[
    "NetCashProvidedByUsedInOperatingActivities",
    "NetCashProvidedByUsedInOperatingActivitiesContinuingOperations",
];
const CAPEX: &[&str] = &[
    "PaymentsToAcquirePropertyPlantAndEquipment",
    "PaymentsToAcquireProductiveAssets",
];

/// The EDGAR ticker of a Yahoo symbol (`AAPL`, `BRK-B`); none for anything
/// that is not a plain US stock (exchange suffixes, indices, crypto, FX).
pub fn ticker(symbol: &str) -> Option<String> {
    use crate::market::AssetClass;
    let upper = symbol.trim().to_ascii_uppercase();
    if upper.is_empty()
        || upper.contains('.')
        || !matches!(AssetClass::of(&upper), AssetClass::Stock)
    {
        return None;
    }
    Some(upper)
}

/// One quarter or fiscal year. Cash paid for capex is negative, as in the
/// Yahoo statements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Period {
    pub end: NaiveDate,
    pub revenue: Option<f64>,
    pub operating_income: Option<f64>,
    pub net_income: Option<f64>,
    pub diluted_eps: Option<f64>,
    pub operating_cash_flow: Option<f64>,
    pub capex: Option<f64>,
}

/// Quarters and fiscal years of one filer, oldest first.
#[derive(Debug, Default, PartialEq)]
pub struct Statements {
    pub company: String,
    pub cik: u64,
    pub quarterly: Vec<Period>,
    pub annual: Vec<Period>,
}

// ───────────────────────────────────────────────────────────────
// Wire format
// ───────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct TickerRow {
    cik_str: u64,
    ticker: String,
}

/// The CIK of `ticker` in `company_tickers.json`.
pub fn find_cik(json: &str, ticker: &str) -> Option<u64> {
    let rows: HashMap<String, TickerRow> = serde_json::from_str(json).ok()?;
    rows.into_values()
        .find(|r| r.ticker.eq_ignore_ascii_case(ticker))
        .map(|r| r.cik_str)
}

#[derive(Deserialize)]
struct CompanyFacts {
    #[serde(rename = "entityName", default)]
    entity_name: String,
    #[serde(default)]
    cik: u64,
    #[serde(default)]
    facts: Taxonomies,
}

#[derive(Deserialize, Default)]
struct Taxonomies {
    #[serde(rename = "us-gaap", default)]
    us_gaap: HashMap<String, Concept>,
}

#[derive(Deserialize)]
struct Concept {
    #[serde(default)]
    units: HashMap<String, Vec<Fact>>,
}

#[derive(Deserialize)]
struct Fact {
    start: Option<NaiveDate>,
    end: NaiveDate,
    val: f64,
    form: String,
    filed: NaiveDate,
}

/// A flow fact over `start..=end`.
#[derive(Clone, Copy, Debug)]
struct Flow {
    start: NaiveDate,
    end: NaiveDate,
    val: f64,
}

impl Flow {
    fn days(&self) -> i64 {
        (self.end - self.start).num_days()
    }
}

fn within(days: i64, (lo, hi): (i64, i64)) -> bool {
    (lo..=hi).contains(&days)
}

/// Annual and quarterly reports only (8-Ks and amendments of other forms
/// carry no statements).
fn is_periodic(form: &str) -> bool {
    form.starts_with("10-K") || form.starts_with("10-Q")
}

/// The flow facts of a metric, one per period. Of several filings of the
/// same period the latest `filed` wins; of several concepts the first in
/// `names` that has the period wins.
fn flows(gaap: &HashMap<String, Concept>, names: &[&str], unit: &str) -> Vec<Flow> {
    let mut chosen: BTreeMap<(NaiveDate, NaiveDate), Flow> = BTreeMap::new();
    for name in names {
        let Some(facts) = gaap.get(*name).and_then(|c| c.units.get(unit)) else {
            continue;
        };
        let mut latest: BTreeMap<(NaiveDate, NaiveDate), (NaiveDate, f64)> = BTreeMap::new();
        for f in facts {
            let Some(start) = f.start else { continue };
            if !is_periodic(&f.form) || !f.val.is_finite() || f.end <= start {
                continue;
            }
            let slot = latest.entry((start, f.end)).or_insert((f.filed, f.val));
            if f.filed >= slot.0 {
                *slot = (f.filed, f.val);
            }
        }
        for ((start, end), (_, val)) in latest {
            chosen
                .entry((start, end))
                .or_insert(Flow { start, end, val });
        }
    }
    chosen.into_values().collect()
}

/// Quarter and fiscal-year values of one metric, keyed by period end.
#[derive(Default)]
struct Series {
    quarters: BTreeMap<NaiveDate, f64>,
    years: BTreeMap<NaiveDate, f64>,
}

/// Rebuild the quarters and years of one metric from its flow facts.
fn series(facts: &[Flow]) -> Series {
    let mut out = Series::default();
    let years: Vec<&Flow> = facts
        .iter()
        .filter(|f| within(f.days(), YEAR_DAYS))
        .collect();
    for y in &years {
        out.years.insert(y.end, y.val);
    }
    // Fiscal-year starts: each annual fact's own, and the day after its end
    // (the current year has no annual fact yet).
    let mut starts: BTreeSet<NaiveDate> = BTreeSet::new();
    for y in &years {
        starts.insert(y.start);
        starts.insert(y.end + Days::days(1));
    }
    let mut last: Option<NaiveDate> = None;
    for s in starts {
        if last.is_some_and(|l| (s - l).num_days().abs() <= START_SLOP) {
            continue;
        }
        last = Some(s);
        let year = years
            .iter()
            .find(|y| (y.start - s).num_days().abs() <= START_SLOP);
        quarters_of_year(facts, s, year.map(|y| (y.end, y.val)), &mut out.quarters);
    }
    out
}

/// The quarters of the fiscal year starting `s`.
fn quarters_of_year(
    facts: &[Flow],
    s: NaiveDate,
    year: Option<(NaiveDate, f64)>,
    out: &mut BTreeMap<NaiveDate, f64>,
) {
    let from_start = |range: (i64, i64)| {
        facts
            .iter()
            .find(|f| (f.start - s).num_days().abs() <= START_SLOP && within(f.days(), range))
    };
    // Year-to-date facts at three, six and nine months.
    let cum = [
        from_start(QUARTER_DAYS),
        from_start(HALF_DAYS),
        from_start(NINE_MONTH_DAYS),
    ];
    // A three-month fact ending near where the k-th quarter ends.
    let direct = |expected_end: NaiveDate| {
        facts.iter().find(|f| {
            within(f.days(), QUARTER_DAYS) && (f.end - expected_end).num_days().abs() <= END_SLOP
        })
    };
    let mut quarter: [Option<(NaiveDate, f64)>; 3] = [None; 3];
    for k in 0..3 {
        let expected = s + Days::days(91 * (k as i64 + 1));
        let end = cum[k]
            .map(|c| c.end)
            .or_else(|| direct(expected).map(|d| d.end));
        let value = direct(expected).map(|d| d.val).or_else(|| {
            let now = cum[k]?.val;
            let before = if k == 0 { 0.0 } else { cum[k - 1]?.val };
            Some(now - before)
        });
        quarter[k] = end.zip(value);
    }
    for (end, v) in quarter.iter().flatten() {
        out.insert(*end, *v);
    }
    let Some((fy_end, fy)) = year else { return };
    // A fourth quarter a filer did report beats the derived one.
    if let Some(d) = facts
        .iter()
        .find(|f| within(f.days(), QUARTER_DAYS) && (f.end - fy_end).num_days().abs() <= START_SLOP)
    {
        out.insert(fy_end, d.val);
        return;
    }
    let nine_months = if quarter.iter().all(Option::is_some) {
        Some(quarter.iter().flatten().map(|(_, v)| v).sum::<f64>())
    } else {
        cum[2].map(|c| c.val)
    };
    if let Some(n) = nine_months {
        out.insert(fy_end, fy - n);
    }
}

/// Statements from a `companyfacts` document.
pub fn parse_company_facts(json: &str) -> Result<Statements> {
    let doc: CompanyFacts =
        serde_json::from_str(json).context("Failed to parse EDGAR company facts")?;
    let gaap = &doc.facts.us_gaap;
    let metric = |names: &[&str], unit: &str| series(&flows(gaap, names, unit));
    let revenue = metric(REVENUE, "USD");
    let operating = metric(OPERATING_INCOME, "USD");
    let net = metric(NET_INCOME, "USD");
    let eps = metric(EPS, "USD/shares");
    let cash = metric(OPERATING_CASH, "USD");
    let capex = metric(CAPEX, "USD");

    let build = |pick: fn(&Series) -> &BTreeMap<NaiveDate, f64>| {
        let sets = [&revenue, &operating, &net, &eps, &cash, &capex];
        let ends: BTreeSet<NaiveDate> = sets.iter().flat_map(|s| pick(s).keys().copied()).collect();
        let mut rows: Vec<Period> = ends
            .into_iter()
            .map(|end| {
                let at = |s: &Series| pick(s).get(&end).copied();
                Period {
                    end,
                    revenue: at(&revenue),
                    operating_income: at(&operating),
                    net_income: at(&net),
                    diluted_eps: at(&eps),
                    operating_cash_flow: at(&cash),
                    capex: at(&capex).map(|v| -v.abs()),
                }
            })
            .collect();
        // A period with neither revenue nor profit is a stray fact.
        rows.retain(|p| p.revenue.is_some() || p.net_income.is_some());
        rows
    };
    let mut quarterly = build(|s| &s.quarters);
    let mut annual = build(|s| &s.years);
    quarterly.drain(..quarterly.len().saturating_sub(QUARTERS_KEPT));
    annual.drain(..annual.len().saturating_sub(YEARS_KEPT));
    Ok(Statements {
        company: doc.entity_name,
        cik: doc.cik,
        quarterly,
        annual,
    })
}

// ───────────────────────────────────────────────────────────────
// Fetch
// ───────────────────────────────────────────────────────────────

const HEADERS: &[(&str, &str)] = &[("User-Agent", USER_AGENT), ("Accept", "application/json")];

/// US statements of `symbol` from EDGAR. `None` for a symbol that is not a
/// US stock, is not in the ticker list, or has no US GAAP facts (foreign
/// filers report on other forms).
pub async fn fetch(client: &YahooClient, symbol: &str) -> Result<Option<Statements>> {
    let Some(ticker) = ticker(symbol) else {
        return Ok(None);
    };
    let list = client
        .fetch_with(TICKERS_URL, TICKERS_TTL, HEADERS)
        .await
        .context("EDGAR ticker list unavailable")?;
    let Some(cik) = find_cik(&list, &ticker) else {
        return Ok(None);
    };
    let url = format!("{FACTS_URL}{cik:010}.json");
    let body = client
        .fetch_with(&url, FACTS_TTL, HEADERS)
        .await
        .context("EDGAR company facts unavailable")?;
    let st = parse_company_facts(&body)?;
    Ok((!st.quarterly.is_empty() || !st.annual.is_empty()).then_some(st))
}

#[cfg(test)]
pub(crate) mod tests;
