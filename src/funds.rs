//! Mutual funds and ETFs.
//!
//! - Latest NAVs for every Indian scheme come from AMFI's public
//!   `NAVAll.txt` (one file, ~1.5 MB), cached in the data directory.
//! - NAV history comes from the free community API at api.mfapi.in, which
//!   republishes AMFI data per scheme code.
//! - Expense ratios (TER) come from AMFI's monthly TER disclosure, which it
//!   publishes as a spreadsheet export of daily TERs per scheme (direct and
//!   regular plan). It is keyed by scheme name and NSDL code, not by AMFI
//!   scheme code, so schemes are joined on their normalized name.
//! - Fund holdings (for overlap) come from Yahoo Finance's `topHoldings`
//!   module: the top 10 positions and sector weights, published for US ETFs
//!   and mutual funds (and some funds listed under Yahoo's `0P…` IDs). AMFI
//!   publishes NAVs, not portfolios, so Indian schemes have no holdings here.

use crate::store;
use anyhow::{Context, Result, bail};
use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const AMFI_NAV_URL: &str = "https://portal.amfiindia.com/spages/NAVAll.txt";
pub const MFAPI_URL: &str = "https://api.mfapi.in/mf";
/// AMFI's TER disclosure API behind amfiindia.com/ter-of-mf-schemes; with
/// `excel=true` it returns the whole month as one `.xlsx` (about 3 MB).
pub const AMFI_TER_URL: &str = "https://www.amfiindia.com/api/populate-te-rdata-revised";
/// Cached AMFI and mfapi files are re-downloaded after this long.
pub const CACHE_MAX_AGE: Duration = Duration::from_secs(12 * 3600);
/// The TER table is re-downloaded once a day.
pub const TER_CACHE_MAX_AGE: Duration = Duration::from_secs(24 * 3600);

// ── AMFI NAVAll.txt ──

/// One scheme (plan + option) from AMFI's NAV file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Scheme {
    pub code: u32,
    pub name: String,
    pub plan: Option<String>,
    pub option: Option<String>,
    pub nav: Option<f64>,
    pub date: Option<NaiveDate>,
    pub isin_growth: Option<String>,
    pub isin_reinvest: Option<String>,
    pub amc: Option<String>,
    /// e.g. "Equity Scheme - Flexi Cap Fund"
    pub category: Option<String>,
    /// e.g. "Open Ended Schemes"
    pub scheme_type: Option<String>,
}

impl Scheme {
    /// Name with plan and option, e.g. "X Fund - Direct Plan - Growth".
    pub fn full_name(&self) -> String {
        let mut parts = vec![self.name.as_str()];
        for extra in [&self.plan, &self.option].into_iter().flatten() {
            if !self.name.to_lowercase().contains(&extra.to_lowercase()) {
                parts.push(extra);
            }
        }
        parts.join(" - ")
    }

    pub fn is_direct(&self) -> bool {
        self.plan
            .as_deref()
            .map(|p| p.to_lowercase().contains("direct"))
            .unwrap_or_else(|| self.name.to_lowercase().contains("direct"))
    }

    /// Growth option (IDCW/dividend options pay out, so their NAV returns
    /// understate what an investor earned).
    pub fn is_growth(&self) -> bool {
        let text = format!(
            "{} {}",
            self.name.to_lowercase(),
            self.option.as_deref().unwrap_or("").to_lowercase()
        );
        text.contains("growth") && !text.contains("idcw") && !text.contains("dividend")
    }
}

fn dash_none(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty() && s != "-").then(|| s.to_string())
}

/// AMFI's `dd-Mon-yyyy` dates.
pub fn parse_amfi_date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%d-%b-%Y").ok()
}

/// Column positions, read from the header so both the current 8-column
/// layout (with Plan and Option) and the older 6-column one parse.
struct Columns {
    code: usize,
    isin_growth: Option<usize>,
    isin_reinvest: Option<usize>,
    name: usize,
    plan: Option<usize>,
    option: Option<usize>,
    nav: usize,
    date: usize,
}

impl Columns {
    fn from_header(line: &str) -> Option<Self> {
        let cols: Vec<String> = line.split(';').map(|c| c.trim().to_lowercase()).collect();
        let find = |pred: &dyn Fn(&str) -> bool| cols.iter().position(|c| pred(c));
        Some(Self {
            code: find(&|c| c == "scheme code")?,
            isin_growth: find(&|c| c.contains("payout") || c.contains("isin growth")),
            isin_reinvest: find(&|c| c.contains("reinvestment")),
            name: find(&|c| c == "scheme name")?,
            plan: find(&|c| c == "plan"),
            option: find(&|c| c == "option"),
            nav: find(&|c| c.contains("net asset value"))?,
            date: find(&|c| c == "date")?,
        })
    }

    fn legacy() -> Self {
        Self {
            code: 0,
            isin_growth: Some(1),
            isin_reinvest: Some(2),
            name: 3,
            plan: None,
            option: None,
            nav: 4,
            date: 5,
        }
    }
}

/// `Open Ended Schemes(Equity Scheme - Large Cap Fund)` → (type, category).
fn section_heading(line: &str) -> Option<(String, String)> {
    let open = line.find('(')?;
    let kind = line[..open].trim();
    if !kind.to_lowercase().contains("scheme") || !line.ends_with(')') {
        return None;
    }
    let category = line[open + 1..line.len() - 1].trim();
    Some((kind.to_string(), category.to_string()))
}

/// Parse AMFI's `NAVAll.txt`. Section headings set the scheme type and
/// category, bare lines name the fund house, and `;`-separated lines are
/// schemes. Unparseable NAVs (`N.A.`) become `None`.
pub fn parse_navall(text: &str) -> Vec<Scheme> {
    let mut cols: Option<Columns> = None;
    let (mut scheme_type, mut category, mut amc) = (None, None, None);
    let mut out = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if !line.contains(';') {
            if let Some((kind, cat)) = section_heading(line) {
                scheme_type = Some(kind);
                category = Some(cat);
                amc = None;
            } else {
                amc = Some(line.to_string());
            }
            continue;
        }
        if cols.is_none() && line.to_lowercase().starts_with("scheme code") {
            cols = Columns::from_header(line);
            continue;
        }
        let c = cols.get_or_insert_with(Columns::legacy);
        let fields: Vec<&str> = line.split(';').collect();
        let get = |i: usize| fields.get(i).map(|s| s.trim()).unwrap_or("");
        let Ok(code) = get(c.code).parse::<u32>() else {
            continue;
        };
        out.push(Scheme {
            code,
            name: get(c.name).to_string(),
            plan: c.plan.and_then(|i| dash_none(get(i))),
            option: c.option.and_then(|i| dash_none(get(i))),
            nav: get(c.nav)
                .parse::<f64>()
                .ok()
                .filter(|v| v.is_finite() && *v > 0.0),
            date: parse_amfi_date(get(c.date)),
            isin_growth: c.isin_growth.and_then(|i| dash_none(get(i))),
            isin_reinvest: c.isin_reinvest.and_then(|i| dash_none(get(i))),
            amc: amc.clone(),
            category: category.clone(),
            scheme_type: scheme_type.clone(),
        });
    }
    out
}

/// Case-insensitive: every word of `query` appears in the scheme's name,
/// fund house, or category — or `query` is the scheme code.
pub fn search<'a>(schemes: &'a [Scheme], query: &str) -> Vec<&'a Scheme> {
    let words: Vec<String> = query.split_whitespace().map(|w| w.to_lowercase()).collect();
    if words.is_empty() {
        return Vec::new();
    }
    schemes
        .iter()
        .filter(|s| {
            if query.trim() == s.code.to_string() {
                return true;
            }
            let hay = format!(
                "{} {} {} {}",
                s.full_name(),
                s.amc.as_deref().unwrap_or(""),
                s.category.as_deref().unwrap_or(""),
                s.isin_growth.as_deref().unwrap_or("")
            )
            .to_lowercase();
            words.iter().all(|w| hay.contains(w.as_str()))
        })
        .collect()
}

/// Category key that treats AMFI's "Equity Scheme" and "Equity Schemes"
/// spellings as one.
pub fn category_key(category: &str) -> String {
    category
        .to_lowercase()
        .replace("schemes", "scheme")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Schemes with a NAV within 30 days of the newest one in the file. AMFI
/// keeps matured and wound-up schemes in the list with their last NAV.
pub fn active(schemes: &[Scheme]) -> Vec<&Scheme> {
    let newest = schemes.iter().filter_map(|s| s.date).max();
    schemes
        .iter()
        .filter(|s| match (s.date, newest, s.nav) {
            (Some(d), Some(n), Some(_)) => (n - d).num_days() <= 30,
            _ => false,
        })
        .collect()
}

/// Categories with their scheme counts, most schemes first.
pub fn categories<'a>(schemes: impl IntoIterator<Item = &'a Scheme>) -> Vec<(String, usize)> {
    let mut map: std::collections::BTreeMap<String, (String, usize)> = Default::default();
    for s in schemes {
        if let Some(cat) = &s.category {
            map.entry(category_key(cat))
                .or_insert_with(|| (cat.replace("Schemes - ", "Scheme - "), 0))
                .1 += 1;
        }
    }
    let mut out: Vec<(String, usize)> = map.into_values().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

// ── Expense ratios (AMFI TER disclosure) ──

/// One plan's expense ratio, as fractions of assets a year (0.0062 = 0.62%).
/// From April 2026 AMFI splits the total into a base expense ratio,
/// brokerage, transaction costs and statutory levies; before that only a
/// base TER (where shown) and the total.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerCosts {
    pub total: f64,
    pub base: Option<f64>,
    pub brokerage: Option<f64>,
    pub transaction: Option<f64>,
    pub statutory_levies: Option<f64>,
}

/// A scheme's latest disclosed TER for its regular and direct plans. A plan
/// the scheme does not offer (most ETFs have no direct plan) is `None`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TerRecord {
    pub name: String,
    pub nsdl_code: Option<String>,
    pub category: Option<String>,
    pub date: NaiveDate,
    pub regular: Option<TerCosts>,
    pub direct: Option<TerCosts>,
}

/// A scheme's TER for its own plan, with the other plan's for comparison.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SchemeTer {
    /// `direct` or `regular`: the plan `total` and the breakdown are for.
    pub plan: &'static str,
    #[serde(flatten)]
    pub costs: TerCosts,
    pub direct: Option<f64>,
    pub regular: Option<f64>,
    /// Date of the disclosure row used.
    pub date: NaiveDate,
}

/// Every scheme's latest TER in one disclosure month.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TerTable {
    /// Disclosure month as AMFI names it, `MM-YYYY`.
    pub month: String,
    pub records: Vec<TerRecord>,
    #[serde(skip)]
    index: HashMap<String, usize>,
}

/// Words that make up a plan/option suffix such as "Direct Plan - Growth".
const PLAN_WORDS: &[&str] = &[
    "direct",
    "regular",
    "plan",
    "growth",
    "idcw",
    "dividend",
    "payout",
    "reinvestment",
    "reinvest",
    "option",
    "bonus",
    "daily",
    "weekly",
    "fortnightly",
    "monthly",
    "quarterly",
    "half",
    "yearly",
    "annual",
];

/// Join key for scheme names: lower case, `&` as "and", punctuation and
/// repeated spaces collapsed. AMFI's TER sheet and NAV file differ only in
/// case and spacing ("360 ONE FLEXICAP FUND" vs "360 ONE Flexicap Fund").
pub fn ter_key(name: &str) -> String {
    let lower = name.to_lowercase().replace('&', " and ");
    lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Scheme name without trailing plan/option segments:
/// "X Fund - Direct Plan - Growth" → "X Fund". Segments with any other word
/// ("Monthly Interval Plan", "Series 2") are part of the name and stay.
pub fn base_scheme_name(name: &str) -> &str {
    let mut end = name.len();
    while let Some(i) = name[..end].rfind('-') {
        let words: Vec<String> = name[i + 1..end]
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        if words.is_empty() || !words.iter().all(|w| PLAN_WORDS.contains(&w.as_str())) {
            break;
        }
        end = i;
    }
    name[..end].trim_end()
}

/// A TER cell (in percent) as a fraction; blank and dashes are `None`.
fn ter_component(s: &str) -> Option<f64> {
    s.trim()
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v / 100.0)
}

/// A total-TER cell as a fraction. A total of 0 is AMFI's marker for a
/// plan the scheme does not offer, so it is `None` too.
fn ter_value(s: &str) -> Option<f64> {
    ter_component(s).filter(|v| *v > 0.0)
}

/// Excel serial day numbers (the export's format) or ISO / `dd-mm-yyyy` /
/// `dd-Mon-yyyy` text (the JSON API's).
fn ter_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    if let Ok(serial) = s.parse::<f64>() {
        // Excel's day 0 is 1899-12-30 (it counts a 29 Feb 1900 that never was).
        return (20_000.0..90_000.0).contains(&serial).then(|| {
            NaiveDate::from_ymd_opt(1899, 12, 30).expect("valid date")
                + chrono::Days::new(serial as u64)
        });
    }
    let day = s.get(..10).unwrap_or(s);
    NaiveDate::parse_from_str(day, "%Y-%m-%d")
        .or_else(|_| NaiveDate::parse_from_str(day, "%d-%m-%Y"))
        .or_else(|_| NaiveDate::parse_from_str(s, "%d-%b-%Y"))
        .ok()
}

/// Column positions in a TER sheet, found from the header text so the
/// pre-April-2026 layout ("Base TER") and the current one ("BER", costs
/// split out) both parse.
struct TerColumns {
    name: usize,
    nsdl: Option<usize>,
    category: Option<usize>,
    date: usize,
    plans: [PlanColumns; 2],
}

struct PlanColumns {
    total: usize,
    base: Option<usize>,
    brokerage: Option<usize>,
    transaction: Option<usize>,
    levies: Option<usize>,
}

impl PlanColumns {
    fn find(header: &[String], plan: &str) -> Option<Self> {
        let col = |words: &[&str]| {
            header.iter().position(|h| {
                let h = h.to_lowercase();
                h.starts_with(plan) && words.iter().any(|w| h.contains(w))
            })
        };
        Some(Self {
            total: col(&["total ter"])?,
            base: col(&["base expense", "base ter"]),
            brokerage: col(&["brokerage"]),
            transaction: col(&["transaction"]),
            levies: col(&["statutory"]),
        })
    }

    fn costs(&self, row: &[String]) -> Option<TerCosts> {
        let get = |i: Option<usize>| i.and_then(|i| row.get(i)).and_then(|s| ter_component(s));
        Some(TerCosts {
            total: row.get(self.total).and_then(|s| ter_value(s))?,
            base: get(self.base),
            brokerage: get(self.brokerage),
            transaction: get(self.transaction),
            statutory_levies: get(self.levies),
        })
    }
}

impl TerColumns {
    fn from_header(header: &[String]) -> Option<Self> {
        let find = |pred: &dyn Fn(&str) -> bool| {
            header
                .iter()
                .position(|h| pred(h.trim().to_lowercase().as_str()))
        };
        Some(Self {
            name: find(&|h| h == "scheme name")?,
            nsdl: find(&|h| h.contains("nsdl")),
            category: find(&|h| h == "scheme category"),
            date: find(&|h| h == "ter date")?,
            plans: [
                PlanColumns::find(header, "regular plan")?,
                PlanColumns::find(header, "direct plan")?,
            ],
        })
    }
}

/// Parse the rows of AMFI's TER export: daily rows per scheme, reduced to
/// each scheme's latest date. Notes under the table are skipped.
pub fn parse_ter_rows(rows: &[Vec<String>]) -> Result<Vec<TerRecord>> {
    let (header_at, cols) = rows
        .iter()
        .enumerate()
        .take(20)
        .find_map(|(i, r)| Some((i, TerColumns::from_header(r)?)))
        .context("AMFI's TER sheet has an unexpected layout (no Scheme Name / TER Date / Total TER columns)")?;
    let mut latest: HashMap<String, TerRecord> = HashMap::new();
    for row in &rows[header_at + 1..] {
        let cell = |i: usize| row.get(i).map(|s| s.trim()).unwrap_or("");
        let name = cell(cols.name);
        let Some(date) = ter_date(cell(cols.date)) else {
            continue;
        };
        if name.is_empty() {
            continue;
        }
        let record = TerRecord {
            name: name.to_string(),
            nsdl_code: cols
                .nsdl
                .map(cell)
                .filter(|s| !s.is_empty())
                .map(String::from),
            category: cols
                .category
                .map(cell)
                .filter(|s| !s.is_empty())
                .map(String::from),
            date,
            regular: cols.plans[0].costs(row),
            direct: cols.plans[1].costs(row),
        };
        if record.regular.is_none() && record.direct.is_none() {
            continue;
        }
        let key = ter_key(name);
        if latest.get(&key).is_none_or(|old| old.date <= date) {
            latest.insert(key, record);
        }
    }
    let mut out: Vec<TerRecord> = latest.into_values().collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

impl TerTable {
    pub fn new(month: String, records: Vec<TerRecord>) -> Self {
        let mut t = Self {
            month,
            records,
            index: HashMap::new(),
        };
        t.reindex();
        t
    }

    fn reindex(&mut self) {
        self.index = self
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| (ter_key(&r.name), i))
            .collect();
    }

    /// The record for a scheme name, with or without a plan/option suffix.
    pub fn find(&self, name: &str) -> Option<&TerRecord> {
        [name, base_scheme_name(name)]
            .iter()
            .find_map(|n| self.index.get(&ter_key(n)))
            .map(|&i| &self.records[i])
    }

    /// TER of `scheme`'s own plan (direct or regular).
    pub fn for_scheme(&self, scheme: &Scheme) -> Option<SchemeTer> {
        self.for_name(&scheme.name, scheme.is_direct())
    }

    /// TER of the named scheme's direct or regular plan; `None` when the
    /// scheme is not in the disclosure or the plan has no TER there.
    pub fn for_name(&self, name: &str, direct: bool) -> Option<SchemeTer> {
        let r = self.find(name)?;
        let costs = if direct { &r.direct } else { &r.regular };
        Some(SchemeTer {
            plan: if direct { "direct" } else { "regular" },
            costs: costs.clone()?,
            direct: r.direct.as_ref().map(|c| c.total),
            regular: r.regular.as_ref().map(|c| c.total),
            date: r.date,
        })
    }
}

/// `MM-YYYY` for AMFI's `Month` parameter.
fn ter_month(d: NaiveDate) -> String {
    d.format("%m-%Y").to_string()
}

pub fn ter_export_url(month: &str) -> String {
    format!("{AMFI_TER_URL}?MF_ID=All&Month={month}&strCat=-1&strType=-1&excel=true")
}

async fn download_ter(client: &reqwest::Client) -> Result<TerTable> {
    let today = chrono::Utc::now()
        .with_timezone(&crate::clock::ist())
        .date_naive();
    let this_month = today.with_day(1).expect("valid date");
    let last_month = this_month - chrono::Days::new(1);
    let mut tried = Vec::new();
    // Early in a month AMFI may not have this month's sheet yet.
    for month in [ter_month(this_month), ter_month(last_month)] {
        let bytes = get_bytes(client, &ter_export_url(&month), "AMFI").await?;
        let rows = crate::xlsx::read_first_sheet(&bytes)
            .context("AMFI's TER export is not a readable spreadsheet")?;
        let records = parse_ter_rows(&rows)?;
        if !records.is_empty() {
            return Ok(TerTable::new(month, records));
        }
        tried.push(month);
    }
    bail!("AMFI published no TER data for {}", tried.join(" or "))
}

/// Latest TER of every scheme (AMFI's disclosure, cached for a day). The
/// first download of the day takes a while: AMFI builds a ~3 MB export.
pub async fn load_ter() -> Result<TerTable> {
    let path = cache_path("amfi_ter.json")?;
    let cached = || -> Option<TerTable> {
        let mut t: TerTable = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
        (!t.records.is_empty()).then(|| {
            t.reindex();
            t
        })
    };
    if is_fresh(&path, TER_CACHE_MAX_AGE)
        && let Some(t) = cached()
    {
        return Ok(t);
    }
    eprintln!("  Downloading AMFI's expense-ratio (TER) disclosure (once a day, ~3 MB)...");
    match download_ter(&http_client()?).await {
        Ok(t) => {
            if let Ok(json) = serde_json::to_string(&t) {
                let _ = write_atomic(&path, &json);
            }
            Ok(t)
        }
        Err(e) => match cached() {
            Some(t) => {
                eprintln!(
                    "  Warning: {e:#}; using TERs cached from {}",
                    path.display()
                );
                Ok(t)
            }
            None => Err(e),
        },
    }
}

// ── NAV history (api.mfapi.in) ──

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SchemeMeta {
    pub code: u32,
    pub name: String,
    pub fund_house: Option<String>,
    pub category: Option<String>,
    pub scheme_type: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NavHistory {
    pub meta: SchemeMeta,
    /// Oldest first.
    pub navs: Vec<(NaiveDate, f64)>,
}

/// Parse an api.mfapi.in `/mf/<code>` response. Unknown codes come back
/// with empty data, which is an error here.
pub fn parse_mfapi(body: &str, code: u32) -> Result<NavHistory> {
    let v: Value = serde_json::from_str(body).context("NAV history is not valid JSON")?;
    let meta = v.get("meta");
    let text = |key: &str| {
        meta.and_then(|m| m.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    let mut navs: Vec<(NaiveDate, f64)> = v
        .get("data")
        .and_then(Value::as_array)
        .map(|rows| {
            rows.iter()
                .filter_map(|r| {
                    let date =
                        NaiveDate::parse_from_str(r.get("date")?.as_str()?, "%d-%m-%Y").ok()?;
                    let nav: f64 = r.get("nav")?.as_str()?.parse().ok()?;
                    (nav.is_finite() && nav > 0.0).then_some((date, nav))
                })
                .collect()
        })
        .unwrap_or_default();
    if navs.is_empty() {
        bail!("No NAV history for scheme {code} (check the code with `stockwise mf search`)");
    }
    navs.sort_by_key(|(d, _)| *d);
    navs.dedup_by_key(|(d, _)| *d);
    Ok(NavHistory {
        meta: SchemeMeta {
            code,
            name: text("scheme_name").unwrap_or_else(|| format!("Scheme {code}")),
            fund_house: text("fund_house"),
            category: text("scheme_category"),
            scheme_type: text("scheme_type"),
        },
        navs,
    })
}

/// Last NAV on or before `date`.
pub fn nav_on_or_before(navs: &[(NaiveDate, f64)], date: NaiveDate) -> Option<(NaiveDate, f64)> {
    let i = navs.partition_point(|(d, _)| *d <= date);
    i.checked_sub(1).map(|i| navs[i])
}

/// First NAV on or after `date`.
pub fn nav_on_or_after(navs: &[(NaiveDate, f64)], date: NaiveDate) -> Option<(NaiveDate, f64)> {
    let i = navs.partition_point(|(d, _)| *d < date);
    navs.get(i).copied()
}

/// Return over the trailing `months`, ending at the latest NAV: absolute
/// below a year, CAGR (annualized) from 12 months up. `None` when the
/// history does not go back that far.
pub fn trailing_return(navs: &[(NaiveDate, f64)], months: u32) -> Option<f64> {
    let &(end_date, end_nav) = navs.last()?;
    let target = end_date.checked_sub_months(Months::new(months))?;
    if navs.first()?.0 > target {
        return None;
    }
    let (start_date, start_nav) = nav_on_or_before(navs, target)?;
    let growth = end_nav / start_nav;
    if months < 12 {
        Some(growth - 1.0)
    } else {
        let years = (end_date - start_date).num_days() as f64 / 365.0;
        (years > 0.0).then(|| growth.powf(1.0 / years) - 1.0)
    }
}

/// CAGR from the first to the last NAV (absolute return if under a year).
pub fn since_inception(navs: &[(NaiveDate, f64)]) -> Option<f64> {
    let (&(d0, n0), &(d1, n1)) = (navs.first()?, navs.last()?);
    let years = (d1 - d0).num_days() as f64 / 365.0;
    if years <= 0.0 {
        return None;
    }
    Some(if years < 1.0 {
        n1 / n0 - 1.0
    } else {
        (n1 / n0).powf(1.0 / years) - 1.0
    })
}

/// Standard return horizons: label and months.
pub const HORIZONS: [(&str, u32); 7] = [
    ("1M", 1),
    ("3M", 3),
    ("6M", 6),
    ("1Y", 12),
    ("3Y", 36),
    ("5Y", 60),
    ("10Y", 120),
];

/// Parse an MF period: `<n>mo`, `<n>y`, or `max` → months (`None` = all).
pub fn parse_period(s: &str) -> Result<Option<u32>, String> {
    let s = s.trim().to_lowercase();
    if s == "max" {
        return Ok(None);
    }
    let (num, mult) = if let Some(n) = s.strip_suffix("mo") {
        (n, 1)
    } else if let Some(n) = s.strip_suffix('y') {
        (n, 12)
    } else {
        return Err(format!("`{s}`: use months (6mo), years (3y) or max"));
    };
    match num.parse::<u32>() {
        Ok(n) if n > 0 && n <= 50 => Ok(Some(n * mult)),
        _ => Err(format!("`{s}`: use months (6mo), years (3y) or max")),
    }
}

/// NAVs from `months` before the latest NAV onwards (all for `None`).
pub fn slice_period(navs: &[(NaiveDate, f64)], months: Option<u32>) -> &[(NaiveDate, f64)] {
    let (Some(m), Some(&(end, _))) = (months, navs.last()) else {
        return navs;
    };
    let Some(start) = end.checked_sub_months(Months::new(m)) else {
        return navs;
    };
    let i = navs.partition_point(|(d, _)| *d < start);
    &navs[i..]
}

/// Largest peak-to-trough fall, as a positive fraction.
pub fn max_drawdown(navs: &[(NaiveDate, f64)]) -> Option<f64> {
    let mut peak = navs.first()?.1;
    let mut worst: f64 = 0.0;
    for &(_, v) in navs {
        peak = peak.max(v);
        worst = worst.max(1.0 - v / peak);
    }
    Some(worst)
}

// ── SIP ──

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SipInstalment {
    pub date: NaiveDate,
    pub nav: f64,
    pub units: f64,
    /// Invested and portfolio value right after this instalment.
    pub invested: f64,
    pub value: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SipResult {
    pub instalments: Vec<SipInstalment>,
    pub invested: f64,
    pub units: f64,
    pub end_date: NaiveDate,
    pub end_nav: f64,
    pub value: f64,
    /// Annualized money-weighted return.
    pub xirr: Option<f64>,
}

/// Invest `amount` monthly from `start` (same day each month, bought at the
/// first NAV on or after it) and value the units at the latest NAV. XIRR
/// uses the actual purchase dates.
pub fn simulate_sip(navs: &[(NaiveDate, f64)], amount: f64, start: NaiveDate) -> Option<SipResult> {
    let &(end_date, end_nav) = navs.last()?;
    let mut instalments: Vec<SipInstalment> = Vec::new();
    let (mut units, mut invested) = (0.0, 0.0);
    for k in 0.. {
        let due = start.checked_add_months(Months::new(k))?;
        if due > end_date {
            break;
        }
        let Some((date, nav)) = nav_on_or_after(navs, due) else {
            break;
        };
        // Two due dates can map to one NAV across a long gap; buy once.
        if instalments.last().is_some_and(|i| i.date == date) {
            continue;
        }
        units += amount / nav;
        invested += amount;
        instalments.push(SipInstalment {
            date,
            nav,
            units: amount / nav,
            invested,
            value: units * nav,
        });
    }
    let first = instalments.first()?.date;
    let years = |d: NaiveDate| (d - first).num_days() as f64 / 365.0;
    let value = units * end_nav;
    let mut flows: Vec<(f64, f64)> = instalments
        .iter()
        .map(|i| (years(i.date), -amount))
        .collect();
    flows.push((years(end_date), value));
    let xirr = if end_date > first {
        crate::technical::xirr(&flows)
    } else {
        None
    };
    Some(SipResult {
        instalments,
        invested,
        units,
        end_date,
        end_nav,
        value,
        xirr,
    })
}

// ── Holdings overlap (Yahoo topHoldings) ──

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Holding {
    pub symbol: Option<String>,
    pub name: Option<String>,
    /// Fraction of the fund (0.08 = 8%).
    pub weight: f64,
}

impl Holding {
    fn key(&self) -> Option<String> {
        self.symbol
            .as_deref()
            .or(self.name.as_deref())
            .map(|k| k.trim().to_uppercase())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FundProfile {
    pub symbol: String,
    pub name: Option<String>,
    pub family: Option<String>,
    pub category: Option<String>,
    pub expense_ratio: Option<f64>,
    pub currency: Option<String>,
    /// Top holdings only (Yahoo publishes 10).
    pub holdings: Vec<Holding>,
    /// Sector name → weight; these cover the whole fund.
    pub sectors: Vec<(String, f64)>,
}

fn raw(v: Option<&Value>) -> Option<f64> {
    v.and_then(|x| x.get("raw").and_then(Value::as_f64).or_else(|| x.as_f64()))
        .filter(|x| x.is_finite())
}

/// Parse a Yahoo `quoteSummary` response with the `topHoldings`,
/// `fundProfile` and `price` modules.
pub fn parse_fund_profile(symbol: &str, v: &Value) -> Result<FundProfile> {
    let r = v
        .pointer("/quoteSummary/result/0")
        .with_context(|| format!("Yahoo Finance has no fund data for {symbol}"))?;
    let th = r.get("topHoldings");
    let holdings: Vec<Holding> = th
        .and_then(|t| t.get("holdings"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|h| {
                    let text = |k: &str| {
                        h.get(k)
                            .and_then(Value::as_str)
                            .filter(|s| !s.trim().is_empty())
                            .map(String::from)
                    };
                    Some(Holding {
                        symbol: text("symbol"),
                        name: text("holdingName"),
                        weight: raw(h.get("holdingPercent")).filter(|w| *w > 0.0)?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let sectors: Vec<(String, f64)> = th
        .and_then(|t| t.get("sectorWeightings"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_object)
                .flat_map(|o| o.iter())
                .filter_map(|(k, w)| Some((k.clone(), raw(Some(w))?)))
                .filter(|(_, w)| *w > 0.0)
                .collect()
        })
        .unwrap_or_default();
    if holdings.is_empty() && sectors.is_empty() {
        bail!("Yahoo Finance publishes no holdings for {symbol}");
    }
    let fp = r.get("fundProfile");
    let price = r.get("price");
    let text = |obj: Option<&Value>, key: &str| {
        obj.and_then(|o| o.get(key))
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(String::from)
    };
    Ok(FundProfile {
        symbol: symbol.to_string(),
        name: text(price, "longName").or_else(|| text(price, "shortName")),
        family: text(fp, "family"),
        category: text(fp, "categoryName"),
        expense_ratio: raw(
            fp.and_then(|f| f.pointer("/feesExpensesInvestment/annualReportExpenseRatio"))
        ),
        currency: text(price, "currency"),
        holdings,
        sectors,
    })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CommonHolding {
    pub symbol: Option<String>,
    pub name: Option<String>,
    pub weight_a: f64,
    pub weight_b: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Overlap {
    pub common: Vec<CommonHolding>,
    /// Σ min(weight in A, weight in B) over shared top holdings — a lower
    /// bound on true overlap, since only top holdings are known.
    pub top_holdings_overlap: f64,
    /// Share of each fund the known holdings cover.
    pub coverage_a: f64,
    pub coverage_b: f64,
    /// Σ min over sector weights (whole-fund data), if both have sectors.
    pub sector_overlap: Option<f64>,
}

pub fn overlap(a: &FundProfile, b: &FundProfile) -> Overlap {
    let mut common: Vec<CommonHolding> = a
        .holdings
        .iter()
        .filter_map(|ha| {
            let key = ha.key()?;
            let hb = b
                .holdings
                .iter()
                .find(|hb| hb.key().as_deref() == Some(&key))?;
            Some(CommonHolding {
                symbol: ha.symbol.clone().or(hb.symbol.clone()),
                name: ha.name.clone().or(hb.name.clone()),
                weight_a: ha.weight,
                weight_b: hb.weight,
            })
        })
        .collect();
    common.sort_by(|x, y| {
        x.weight_a
            .min(x.weight_b)
            .total_cmp(&y.weight_a.min(y.weight_b))
            .reverse()
    });
    let sector_overlap = (!a.sectors.is_empty() && !b.sectors.is_empty()).then(|| {
        a.sectors
            .iter()
            .filter_map(|(k, wa)| {
                let wb = b.sectors.iter().find(|(kb, _)| kb == k)?.1;
                Some(wa.min(wb))
            })
            .sum()
    });
    Overlap {
        top_holdings_overlap: common.iter().map(|c| c.weight_a.min(c.weight_b)).sum(),
        coverage_a: a.holdings.iter().map(|h| h.weight).sum(),
        coverage_b: b.holdings.iter().map(|h| h.weight).sum(),
        common,
        sector_overlap,
    }
}

// ── Fetching and caching ──

/// HTTP client for AMFI and api.mfapi.in.
pub fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("stockwise/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(60))
        .build()
        .context("Failed to build HTTP client")
}

/// GET `url`, retrying network errors and 5xx twice.
async fn fetch(client: &reqwest::Client, url: &str, source: &str) -> Result<reqwest::Response> {
    let mut last_err = None;
    for attempt in 0..3u32 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
        }
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => return Ok(resp),
            Ok(resp) if resp.status().is_server_error() => {
                last_err = Some(anyhow::anyhow!("{source} returned HTTP {}", resp.status()));
            }
            Ok(resp) => bail!("{source} returned HTTP {}", resp.status()),
            Err(e) => last_err = Some(anyhow::anyhow!("Cannot reach {source} ({e})")),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("Cannot reach {source}")))
}

/// GET `url` as text (see [`fetch`]).
async fn get_text(client: &reqwest::Client, url: &str, source: &str) -> Result<String> {
    fetch(client, url, source)
        .await?
        .text()
        .await
        .with_context(|| format!("Failed to read the {source} response"))
}

/// GET `url` as bytes (see [`fetch`]).
async fn get_bytes(client: &reqwest::Client, url: &str, source: &str) -> Result<Vec<u8>> {
    Ok(fetch(client, url, source)
        .await?
        .bytes()
        .await
        .with_context(|| format!("Failed to read the {source} response"))?
        .to_vec())
}

pub(crate) fn cache_path(name: &str) -> Result<PathBuf> {
    let dir = store::data_dir()?.join("cache");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Cannot create cache directory {}", dir.display()))?;
    Ok(dir.join(name))
}

pub(crate) fn is_fresh(path: &Path, max_age: Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.elapsed().ok())
        .is_some_and(|age| age < max_age)
}

/// Write via a temp file and rename, so a crash never leaves half a cache.
pub(crate) fn write_atomic(path: &Path, data: &str) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&tmp, data).with_context(|| format!("Cannot write {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("Cannot replace {}", path.display()))
}

/// Cached copy of `url` if younger than `max_age`; otherwise download it.
/// When the download fails, a stale copy is used with a warning on stderr.
pub(crate) async fn cached_text(
    client: &reqwest::Client,
    name: &str,
    url: &str,
    source: &str,
    validate: impl Fn(&str) -> bool,
) -> Result<String> {
    let path = cache_path(name)?;
    if is_fresh(&path, CACHE_MAX_AGE)
        && let Ok(text) = std::fs::read_to_string(&path)
        && validate(&text)
    {
        return Ok(text);
    }
    match get_text(client, url, source).await {
        Ok(text) if validate(&text) => {
            // Caching is best-effort: a read-only data dir still works.
            let _ = write_atomic(&path, &text);
            Ok(text)
        }
        Ok(_) => bail!("{source} returned an unexpected response"),
        Err(e) => match std::fs::read_to_string(&path) {
            Ok(text) if validate(&text) => {
                eprintln!(
                    "  Warning: {e:#}; using cached data from {}",
                    path.display()
                );
                Ok(text)
            }
            _ => Err(e),
        },
    }
}

/// Every scheme in AMFI's latest NAV file (cached for 12 hours).
pub async fn load_amfi() -> Result<Vec<Scheme>> {
    let client = http_client()?;
    let text = cached_text(&client, "amfi_navall.txt", AMFI_NAV_URL, "AMFI", |t| {
        t.to_lowercase().contains("scheme code")
    })
    .await?;
    let schemes = parse_navall(&text);
    if schemes.is_empty() {
        bail!("AMFI's NAV file had no schemes; try again later");
    }
    Ok(schemes)
}

/// [`load_amfi`], parsed once and shared: valuing `MF:` holdings on every
/// `serve` request or TUI refresh reuses it for up to an hour.
pub async fn load_amfi_shared() -> Result<std::sync::Arc<Vec<Scheme>>> {
    use std::sync::{Arc, Mutex};
    use std::time::Instant;
    static PARSED: Mutex<Option<(Instant, Arc<Vec<Scheme>>)>> = Mutex::new(None);
    const KEEP: Duration = Duration::from_secs(3600);
    if let Some((at, schemes)) = PARSED.lock().ok().and_then(|p| p.clone())
        && at.elapsed() < KEEP
    {
        return Ok(schemes);
    }
    let schemes = Arc::new(load_amfi().await?);
    if let Ok(mut p) = PARSED.lock() {
        *p = Some((Instant::now(), schemes.clone()));
    }
    Ok(schemes)
}

/// NAV history for one scheme (cached for 12 hours).
pub async fn load_history(client: &reqwest::Client, code: u32) -> Result<NavHistory> {
    let url = format!("{MFAPI_URL}/{code}");
    let name = format!("mfapi_{code}.json");
    let text = cached_text(client, &name, &url, "api.mfapi.in", |t| {
        t.trim_start().starts_with('{')
    })
    .await?;
    let parsed = parse_mfapi(&text, code);
    if parsed.is_err() {
        // Don't keep "unknown scheme" answers around for 12 hours.
        let _ = cache_path(&name).map(std::fs::remove_file);
    }
    parsed
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const NAVALL: &str = "Scheme Code;ISIN Div Payout/ ISIN Growth;ISIN Div Reinvestment;Scheme Name;Plan;Option;Net Asset Value;Date\r
 \r
Open Ended Schemes(Equity Scheme - Flexi Cap Fund)\r
 \r
PPFAS Mutual Fund\r
 \r
122639;INF879O01027;-;Parag Parikh Flexi Cap Fund;Direct Plan;Growth;90.3546;19-Oct-2026\r
153964;-;INF879O01308;Parag Parikh Flexi Cap Fund;Direct Plan;Monthly IDCW Payout;90.3546;19-Oct-2026\r
122640;INF879O01019;-;Parag Parikh Flexi Cap Fund;Regular Plan;Growth;82.2629;19-Oct-2026\r
 \r
Open Ended Schemes(Equity Schemes - Large Cap Fund)\r
 \r
Axis Mutual Fund\r
 \r
120465;INF846K01DP8;-;Axis Large Cap Fund;Direct Plan;Growth;N.A.;19-Oct-2026\r
";

    const LEGACY: &str = "Scheme Code;ISIN Div Payout/ ISIN Growth;ISIN Div Reinvestment;Scheme Name;Net Asset Value;Date
Open Ended Schemes(Debt Scheme - Liquid Fund)
HDFC Mutual Fund
119091;INF179KB1HP9;-;HDFC Liquid Fund - Direct Plan - Growth Option;4521.1234;01-Apr-2024
";

    #[test]
    fn parses_amfi_navall_fixture() {
        let s = parse_navall(NAVALL);
        assert_eq!(s.len(), 4);
        let ppfas = &s[0];
        assert_eq!(ppfas.code, 122639);
        assert_eq!(ppfas.nav, Some(90.3546));
        assert_eq!(ppfas.date, NaiveDate::from_ymd_opt(2026, 10, 19));
        assert_eq!(ppfas.amc.as_deref(), Some("PPFAS Mutual Fund"));
        assert_eq!(
            ppfas.category.as_deref(),
            Some("Equity Scheme - Flexi Cap Fund")
        );
        assert_eq!(ppfas.scheme_type.as_deref(), Some("Open Ended Schemes"));
        assert_eq!(ppfas.isin_growth.as_deref(), Some("INF879O01027"));
        assert_eq!(ppfas.isin_reinvest, None);
        assert_eq!(
            ppfas.full_name(),
            "Parag Parikh Flexi Cap Fund - Direct Plan - Growth"
        );
        assert!(ppfas.is_direct() && ppfas.is_growth());
        assert!(!s[1].is_growth(), "IDCW payout is not growth");
        assert!(!s[2].is_direct());
        // N.A. NAV is missing, not zero; the AMC resets per section.
        assert_eq!(s[3].nav, None);
        assert_eq!(s[3].amc.as_deref(), Some("Axis Mutual Fund"));
    }

    #[test]
    fn parses_legacy_six_column_layout() {
        let s = parse_navall(LEGACY);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].nav, Some(4521.1234));
        assert_eq!(s[0].plan, None);
        assert!(s[0].is_direct() && s[0].is_growth());
        assert_eq!(
            s[0].full_name(),
            "HDFC Liquid Fund - Direct Plan - Growth Option"
        );
    }

    #[test]
    fn search_and_categories() {
        let s = parse_navall(NAVALL);
        let hits: Vec<u32> = search(&s, "parag direct").iter().map(|x| x.code).collect();
        assert_eq!(hits, vec![122639, 153964]);
        assert_eq!(search(&s, "120465").len(), 1);
        assert_eq!(search(&s, "PPFAS growth regular").len(), 1);
        assert!(search(&s, "  ").is_empty());
        let cats = categories(&s);
        // The N.A. scheme is not active; the rest share the newest date.
        assert_eq!(active(&s).len(), 3);
        let mut old = s[0].clone();
        old.date = NaiveDate::from_ymd_opt(2019, 3, 29);
        assert_eq!(active(&[s[0].clone(), old]).len(), 1);
        assert_eq!(cats[0], ("Equity Scheme - Flexi Cap Fund".into(), 3));
        assert_eq!(cats[1], ("Equity Scheme - Large Cap Fund".into(), 1));
        assert_eq!(
            category_key("Equity Schemes - Large Cap Fund"),
            category_key("Equity Scheme -  Large Cap Fund")
        );
    }

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn parses_mfapi_and_rejects_unknown_codes() {
        let body = r#"{"meta":{"fund_house":"PPFAS Mutual Fund","scheme_type":"Open Ended Schemes",
            "scheme_category":"Equity Scheme - Flexi Cap Fund","scheme_code":122639,
            "scheme_name":"Parag Parikh Flexi Cap Fund - Direct Plan - Growth"},
            "data":[{"date":"19-10-2026","nav":"90.35460"},{"date":"16-10-2026","nav":"90.88440"},
                    {"date":"16-10-2026","nav":"90.88440"},{"date":"15-10-2026","nav":"bad"}],
            "status":"SUCCESS"}"#;
        let h = parse_mfapi(body, 122639).unwrap();
        assert_eq!(
            h.navs,
            vec![(d(2026, 10, 16), 90.8844), (d(2026, 10, 19), 90.3546)]
        );
        assert_eq!(h.meta.fund_house.as_deref(), Some("PPFAS Mutual Fund"));

        let empty = r#"{"meta":{"scheme_name":""},"data":[],"status":"SUCCESS"}"#;
        assert!(parse_mfapi(empty, 1).is_err());
    }

    /// NAV starting at 100 on 2020-01-01 and growing 1% a month.
    fn monthly_navs(months: u32) -> Vec<(NaiveDate, f64)> {
        (0..=months)
            .map(|m| {
                (
                    d(2020, 1, 1).checked_add_months(Months::new(m)).unwrap(),
                    100.0 * 1.01f64.powi(m as i32),
                )
            })
            .collect()
    }

    #[test]
    fn trailing_returns_and_cagr() {
        let navs = monthly_navs(72);
        let one_y = trailing_return(&navs, 12).unwrap();
        assert!((one_y - (1.01f64.powi(12) - 1.0)).abs() < 1e-3, "{one_y}");
        let three_m = trailing_return(&navs, 3).unwrap();
        assert!((three_m - (1.01f64.powi(3) - 1.0)).abs() < 1e-12);
        assert!(trailing_return(&navs, 120).is_none(), "history too short");
        let si = since_inception(&navs).unwrap();
        assert!((si - (1.01f64.powi(12) - 1.0)).abs() < 2e-3, "{si}");
        assert_eq!(
            nav_on_or_before(&navs, d(2020, 1, 15)).unwrap().0,
            d(2020, 1, 1)
        );
        assert_eq!(
            nav_on_or_after(&navs, d(2020, 1, 15)).unwrap().0,
            d(2020, 2, 1)
        );
        assert_eq!(slice_period(&navs, Some(12)).len(), 13);
        assert_eq!(slice_period(&navs, None).len(), 73);
    }

    #[test]
    fn sip_fixture_xirr() {
        // Flat NAV: SIP XIRR is 0%.
        let flat: Vec<(NaiveDate, f64)> = (0..=24)
            .map(|m| {
                (
                    d(2020, 1, 1).checked_add_months(Months::new(m)).unwrap(),
                    50.0,
                )
            })
            .collect();
        let r = simulate_sip(&flat, 1000.0, d(2020, 1, 1)).unwrap();
        assert_eq!(r.instalments.len(), 25);
        assert_eq!(r.invested, 25_000.0);
        assert!((r.value - 25_000.0).abs() < 1e-9);
        assert!(r.xirr.unwrap().abs() < 1e-6);

        // NAV growing 1% a month: every instalment earns ~12.68% a year, so
        // the money-weighted return is the same.
        let navs = monthly_navs(36);
        let r = simulate_sip(&navs, 5000.0, d(2021, 1, 1)).unwrap();
        assert_eq!(r.instalments.len(), 25);
        assert_eq!(r.instalments[0].nav, navs[12].1);
        let xirr = r.xirr.unwrap();
        assert!((xirr - (1.01f64.powi(12) - 1.0)).abs() < 5e-3, "{xirr}");
        let expected_value: f64 = (12..=36).map(|m| 5000.0 / navs[m].1 * navs[36].1).sum();
        assert!((r.value - expected_value).abs() < 1e-6);

        // Instalment due on a holiday buys at the next NAV.
        let gappy = vec![
            (d(2024, 1, 2), 10.0),
            (d(2024, 2, 1), 11.0),
            (d(2024, 3, 4), 12.0),
        ];
        let r = simulate_sip(&gappy, 100.0, d(2024, 1, 1)).unwrap();
        let dates: Vec<NaiveDate> = r.instalments.iter().map(|i| i.date).collect();
        assert_eq!(dates, vec![d(2024, 1, 2), d(2024, 2, 1), d(2024, 3, 4)]);
    }

    #[test]
    fn period_parsing() {
        assert_eq!(parse_period("3y"), Ok(Some(36)));
        assert_eq!(parse_period("6MO"), Ok(Some(6)));
        assert_eq!(parse_period("max"), Ok(None));
        assert!(parse_period("3x").is_err());
        assert!(parse_period("0y").is_err());
    }

    #[test]
    fn drawdown() {
        let navs = vec![
            (d(2024, 1, 1), 10.0),
            (d(2024, 1, 2), 12.0),
            (d(2024, 1, 3), 9.0),
            (d(2024, 1, 4), 13.0),
        ];
        assert!((max_drawdown(&navs).unwrap() - 0.25).abs() < 1e-12);
    }

    fn profile(symbol: &str, holdings: &[(&str, f64)], sectors: &[(&str, f64)]) -> FundProfile {
        FundProfile {
            symbol: symbol.into(),
            name: None,
            family: None,
            category: None,
            expense_ratio: None,
            currency: None,
            holdings: holdings
                .iter()
                .map(|(s, w)| Holding {
                    symbol: Some(s.to_string()),
                    name: None,
                    weight: *w,
                })
                .collect(),
            sectors: sectors.iter().map(|(s, w)| (s.to_string(), *w)).collect(),
        }
    }

    #[test]
    fn overlap_of_top_holdings_and_sectors() {
        let a = profile(
            "SPY",
            &[("NVDA", 0.08), ("AAPL", 0.07), ("JPM", 0.02)],
            &[("technology", 0.40), ("financial_services", 0.11)],
        );
        let b = profile(
            "QQQ",
            &[("AAPL", 0.09), ("NVDA", 0.05), ("AMD", 0.04)],
            &[("technology", 0.60), ("healthcare", 0.04)],
        );
        let o = overlap(&a, &b);
        assert_eq!(o.common.len(), 2);
        assert_eq!(o.common[0].symbol.as_deref(), Some("AAPL"));
        assert!((o.top_holdings_overlap - 0.12).abs() < 1e-12);
        assert!((o.coverage_a - 0.17).abs() < 1e-12);
        assert!((o.sector_overlap.unwrap() - 0.40).abs() < 1e-12);
    }

    #[test]
    fn parses_yahoo_fund_profile() {
        let v: Value = serde_json::from_str(
            r#"{"quoteSummary":{"result":[{
                "price":{"longName":"SPDR S&P 500 ETF Trust","currency":"USD"},
                "fundProfile":{"family":"State Street","categoryName":"Large Blend",
                  "feesExpensesInvestment":{"annualReportExpenseRatio":{"raw":0.000945,"fmt":"0.09%"}}},
                "topHoldings":{"holdings":[
                  {"symbol":"NVDA","holdingName":"NVIDIA Corp","holdingPercent":{"raw":0.0836}},
                  {"symbol":"","holdingName":"Cash","holdingPercent":{"raw":0.0}}],
                  "sectorWeightings":[{"technology":{"raw":0.4045}},{"realestate":{"raw":0.0}}]}
            }]}}"#,
        )
        .unwrap();
        let p = parse_fund_profile("SPY", &v).unwrap();
        assert_eq!(p.name.as_deref(), Some("SPDR S&P 500 ETF Trust"));
        assert_eq!(p.expense_ratio, Some(0.000945));
        assert_eq!(p.holdings.len(), 1, "zero-weight rows are dropped");
        assert_eq!(p.sectors, vec![("technology".to_string(), 0.4045)]);

        let none: Value = serde_json::from_str(
            r#"{"quoteSummary":{"result":[{"topHoldings":{"holdings":[]}}]}}"#,
        )
        .unwrap();
        assert!(parse_fund_profile("X", &none).is_err());
    }

    fn ter_rows(rows: &[&[&str]]) -> Vec<Vec<String>> {
        rows.iter()
            .map(|r| r.iter().map(|c| c.to_string()).collect())
            .collect()
    }

    #[test]
    fn parses_amfi_ter_export_fixture() {
        let rows =
            crate::xlsx::read_first_sheet(include_bytes!("fixtures/amfi_ter_360one.xlsx")).unwrap();
        let recs = parse_ter_rows(&rows).unwrap();
        assert_eq!(recs.len(), 12, "daily rows reduce to one per scheme");
        let bhf = recs
            .iter()
            .find(|r| r.name == "360 ONE Balanced Hybrid Fund")
            .unwrap();
        // The latest day in the sheet, not the first (0.62% on 1 Oct).
        assert_eq!(bhf.date, d(2026, 10, 23));
        let direct = bhf.direct.as_ref().unwrap();
        assert!((direct.total - 0.0067).abs() < 1e-12, "{direct:?}");
        assert!((direct.base.unwrap() - 0.0049).abs() < 1e-12);
        assert_eq!(direct.transaction, Some(0.0), "zero costs are kept");
        assert!((bhf.regular.as_ref().unwrap().total - 0.0219).abs() < 1e-12);
        assert_eq!(bhf.nsdl_code.as_deref(), Some("360O/O/H/BHF/23/07/0007"));
        assert_eq!(
            bhf.category.as_deref(),
            Some("Hybrid Schemes - Balanced Hybrid Fund")
        );
        // ETFs have no direct plan: AMFI writes 0, which is not a TER.
        let gold = recs.iter().find(|r| r.name == "360 ONE GOLD ETF").unwrap();
        assert!(gold.direct.is_none());
        assert!((gold.regular.as_ref().unwrap().total - 0.0047).abs() < 1e-12);
    }

    #[test]
    fn parses_older_ter_layout_and_keeps_the_latest_row() {
        let rows = ter_rows(&[
            &["Total Expense Ratio of Mutual Fund Schemes"],
            &[
                "Scheme Name",
                "Scheme Type",
                "Scheme Category",
                "TER Date",
                "Regular Plan - Base TER (%)",
                "Regular Plan - Total TER (%)",
                "Direct Plan - Base TER (%)",
                "Direct Plan - Total TER (%)",
            ],
            &[
                "Axis Large Cap Fund",
                "Open Ended",
                "Equity Scheme - Large Cap Fund",
                "2025-03-03T00:00:00.000Z",
                "1.40",
                "1.60",
                "0.50",
                "0.70",
            ],
            &[
                "AXIS  LARGE CAP FUND",
                "Open Ended",
                "Equity Scheme - Large Cap Fund",
                "04-03-2025",
                "1.40",
                "1.58",
                "0.50",
                "0.68",
            ],
            &[
                "Axis Large Cap Fund",
                "Open Ended",
                "Equity Scheme - Large Cap Fund",
                "2025-03-01",
                "1.40",
                "1.99",
                "0.50",
                "0.99",
            ],
            &[
                "Direct Only Index Fund",
                "Open Ended",
                "Index Funds",
                "2025-03-04",
                "-",
                "0.00",
                "",
                "0.20",
            ],
            &[
                "No Plans Fund",
                "Open Ended",
                "Other",
                "2025-03-04",
                "",
                "",
                "",
                "",
            ],
            &["", "", "", "2025-03-04", "1", "1", "1", "1"],
            &["Reference period: April 1 to March 31"],
        ]);
        let recs = parse_ter_rows(&rows).unwrap();
        assert_eq!(recs.len(), 2);
        let axis = &recs[0];
        assert_eq!(axis.name, "AXIS  LARGE CAP FUND");
        assert_eq!(axis.date, d(2025, 3, 4));
        assert!((axis.direct.as_ref().unwrap().total - 0.0068).abs() < 1e-12);
        assert!((axis.direct.as_ref().unwrap().base.unwrap() - 0.005).abs() < 1e-12);
        assert_eq!(axis.direct.as_ref().unwrap().brokerage, None);
        assert!(recs[1].regular.is_none(), "0.00 and '-' are not TERs");

        let bad = ter_rows(&[&["Scheme Code", "Scheme Name", "NAV"]]);
        assert!(parse_ter_rows(&bad).is_err());
    }

    #[test]
    fn ter_dates_and_values() {
        assert_eq!(ter_date("46296"), Some(d(2026, 10, 1)));
        assert_eq!(ter_date("2026-10-01T00:00:00.000Z"), Some(d(2026, 10, 1)));
        assert_eq!(ter_date("01-10-2026"), Some(d(2026, 10, 1)));
        assert_eq!(ter_date("01-Oct-2026"), Some(d(2026, 10, 1)));
        assert_eq!(ter_date("12"), None);
        assert_eq!(ter_date("soon"), None);
        assert_eq!(ter_value("0.62"), Some(0.0062));
        assert_eq!(ter_value("2.5%"), Some(0.025));
        assert_eq!(ter_value("0"), None);
        assert_eq!(ter_component("0"), Some(0.0), "a zero cost is a value");
        assert_eq!(ter_value("NA"), None);
        assert_eq!(ter_month(d(2026, 1, 15)), "01-2026");
        assert!(
            ter_export_url("10-2026").ends_with("Month=10-2026&strCat=-1&strType=-1&excel=true")
        );
    }

    #[test]
    fn scheme_names_join_the_ter_table() {
        assert_eq!(
            ter_key("360 ONE FLEXICAP  FUND"),
            ter_key("360 ONE Flexicap Fund")
        );
        assert_eq!(
            ter_key("ICICI Pru. Banking & PSU"),
            "icici pru banking and psu"
        );
        assert_eq!(
            base_scheme_name("Parag Parikh Flexi Cap Fund - Direct Plan - Growth"),
            "Parag Parikh Flexi Cap Fund"
        );
        assert_eq!(
            base_scheme_name("HDFC Top 100 Fund -Direct Plan - Growth Option"),
            "HDFC Top 100 Fund"
        );
        assert_eq!(
            base_scheme_name("ICICI Prudential Interval Fund - Monthly Interval Plan - Direct"),
            "ICICI Prudential Interval Fund - Monthly Interval Plan"
        );
        assert_eq!(
            base_scheme_name("Nippon India ETF Nifty 50 BeES"),
            "Nippon India ETF Nifty 50 BeES"
        );
        assert_eq!(
            base_scheme_name("ABSL CRISIL-IBX 60:40 SDL Index Fund - Regular - IDCW"),
            "ABSL CRISIL-IBX 60:40 SDL Index Fund"
        );

        let costs = |total: f64| TerCosts {
            total,
            base: None,
            brokerage: None,
            transaction: None,
            statutory_levies: None,
        };
        let table = TerTable::new(
            "10-2026".into(),
            vec![TerRecord {
                name: "PARAG PARIKH FLEXI CAP FUND".into(),
                nsdl_code: None,
                category: None,
                date: d(2026, 10, 23),
                regular: Some(costs(0.0128)),
                direct: Some(costs(0.0063)),
            }],
        );
        let schemes = parse_navall(NAVALL);
        let direct = table.for_scheme(&schemes[0]).unwrap();
        assert_eq!((direct.plan, direct.costs.total), ("direct", 0.0063));
        assert_eq!(
            (direct.direct, direct.regular),
            (Some(0.0063), Some(0.0128))
        );
        assert_eq!(table.for_scheme(&schemes[2]).unwrap().plan, "regular");
        assert!(
            table.for_scheme(&schemes[3]).is_none(),
            "Axis is not in the table"
        );
        // NAV-history names carry the plan; the base name still joins.
        let h = table
            .for_name("Parag Parikh Flexi Cap Fund - Direct Plan - Growth", true)
            .unwrap();
        assert_eq!(h.costs.total, 0.0063);

        // Cached tables round-trip through JSON and are re-indexed.
        let json = serde_json::to_string(&table).unwrap();
        let mut back: TerTable = serde_json::from_str(&json).unwrap();
        assert!(
            back.find("Parag Parikh Flexi Cap Fund").is_none(),
            "index is not serialized"
        );
        back.reindex();
        assert!(back.find("Parag Parikh Flexi Cap Fund").is_some());
    }
}
