//! Research data layer for the `events`, `insights` and `verdict` commands.
//!
//! Gathers everything we know about one stock into a single [`Research`]
//! snapshot, fetched concurrently from:
//!   - Yahoo Finance: quote, quoteSummary modules (financials, estimates,
//!     analyst actions, insiders, calendar), fundamentals time series,
//!     daily prices for the stock and its benchmark index, RSS headlines
//!   - Google News RSS: recent headlines (multi-publisher coverage)
//!   - NSE India (Indian stocks only): official exchange filings, quarterly
//!     shareholding pattern (promoter / FII / DII / MF / retail), insider
//!     (PIT) trades and corporate actions
//!
//! Every source is optional — failures are recorded in `warnings` and the
//! analysis degrades gracefully.

use crate::api::{ChartData, Quote, YahooClient};
use crate::market::{self, Market};
use anyhow::{Context, Result};
use chrono::{Datelike, NaiveDate, NaiveDateTime, Utc};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

// ───────────────────────────────────────────────────────────────
// JSON helpers
// ───────────────────────────────────────────────────────────────

/// Read a Yahoo numeric field that may be `{"raw": x, "fmt": ".."}` or a bare number.
pub fn raw(v: &Value, key: &str) -> Option<f64> {
    let x = v.get(key)?;
    let n = match x.get("raw") {
        Some(r) => r.as_f64(),
        None => x.as_f64(),
    };
    n.filter(|f| f.is_finite())
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(|x| x.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "-")
}

/// Parse NSE numeric strings like "3294168" or "1,234.5"
fn nse_num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::String(s) => s.replace(',', "").trim().parse().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    }
}

/// Percent-encode a query-string component.
pub fn url_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 3);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// "Larsen & Toubro Limited" → "Larsen & Toubro", "Apple Inc." → "Apple"
pub fn clean_company_name(name: &str) -> String {
    let mut s = name.trim().to_string();
    let suffixes = [
        " limited",
        " ltd.",
        " ltd",
        " inc.",
        " inc",
        ", inc.",
        ", inc",
        " corporation",
        " corp.",
        " corp",
        " plc",
        " co.",
        " company",
        " holdings",
        " (the)",
    ];
    loop {
        let lower = s.to_lowercase();
        let mut changed = false;
        for suf in suffixes {
            if lower.ends_with(suf) {
                s.truncate(s.len() - suf.len());
                s = s.trim_end_matches([',', ' ']).to_string();
                changed = true;
                break;
            }
        }
        if !changed {
            break;
        }
    }
    if s.is_empty() { name.to_string() } else { s }
}

// ───────────────────────────────────────────────────────────────
// Data model
// ───────────────────────────────────────────────────────────────

/// One reporting period (quarter or fiscal year).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Period {
    pub end: NaiveDate,
    pub revenue: Option<f64>,
    pub net_income: Option<f64>,
    pub operating_income: Option<f64>,
    pub ebitda: Option<f64>,
    pub eps: Option<f64>,
    pub ocf: Option<f64>,
    pub fcf: Option<f64>,
    pub equity: Option<f64>,
    pub total_debt: Option<f64>,
    pub cash: Option<f64>,
    pub interest_expense: Option<f64>,
    pub ebit: Option<f64>,
    pub total_assets: Option<f64>,
    pub gross_profit: Option<f64>,
    pub current_assets: Option<f64>,
    pub current_liabilities: Option<f64>,
    pub retained_earnings: Option<f64>,
    pub working_capital: Option<f64>,
    pub shares_outstanding: Option<f64>,
    pub total_liabilities: Option<f64>,
}

impl Period {
    pub fn net_margin(&self) -> Option<f64> {
        match (self.net_income, self.revenue) {
            (Some(n), Some(r)) if r > 0.0 => Some(n / r),
            _ => None,
        }
    }
    pub fn op_margin(&self) -> Option<f64> {
        match (self.operating_income, self.revenue) {
            (Some(o), Some(r)) if r > 0.0 => Some(o / r),
            _ => None,
        }
    }
    pub fn gross_margin(&self) -> Option<f64> {
        match (self.gross_profit, self.revenue) {
            (Some(gp), Some(r)) if r > 0.0 => Some(gp / r),
            _ => None,
        }
    }
    pub fn current_ratio(&self) -> Option<f64> {
        match (self.current_assets, self.current_liabilities) {
            (Some(ca), Some(cl)) if cl > 0.0 => Some(ca / cl),
            _ => None,
        }
    }
    pub fn working_capital(&self) -> Option<f64> {
        self.working_capital
            .or(match (self.current_assets, self.current_liabilities) {
                (Some(ca), Some(cl)) => Some(ca - cl),
                _ => None,
            })
    }
    pub fn asset_turnover(&self) -> Option<f64> {
        match (self.revenue, self.total_assets) {
            (Some(rev), Some(assets)) if assets > 0.0 => Some(rev / assets),
            _ => None,
        }
    }
    pub fn roa(&self) -> Option<f64> {
        match (self.net_income, self.total_assets) {
            (Some(ni), Some(assets)) if assets > 0.0 => Some(ni / assets),
            _ => None,
        }
    }
}

/// Merged `financialData` + `defaultKeyStatistics` + `summaryDetail`.
#[derive(Clone, Debug, Default)]
pub struct Fin {
    pub target_low: Option<f64>,
    pub target_mean: Option<f64>,
    pub target_median: Option<f64>,
    pub target_high: Option<f64>,
    pub rec_mean: Option<f64>,
    pub rec_key: Option<String>,
    pub n_analysts: Option<f64>,
    pub total_cash: Option<f64>,
    pub total_debt: Option<f64>,
    pub ebitda: Option<f64>,
    pub revenue_growth: Option<f64>,
    pub earnings_growth: Option<f64>,
    pub gross_margin: Option<f64>,
    pub op_margin: Option<f64>,
    pub profit_margin: Option<f64>,
    pub roe: Option<f64>,
    pub roa: Option<f64>,
    pub fcf: Option<f64>,
    pub ocf: Option<f64>,
    pub current_ratio: Option<f64>,
    pub de: Option<f64>,
    pub shares: Option<f64>,
    pub insiders_pct: Option<f64>,
    pub inst_pct: Option<f64>,
    pub short_pct_float: Option<f64>,
    pub short_ratio: Option<f64>,
    pub peg: Option<f64>,
    pub book_value: Option<f64>,
    pub forward_eps: Option<f64>,
    pub trailing_eps: Option<f64>,
    pub pb: Option<f64>,
    pub beta: Option<f64>,
    pub trailing_pe: Option<f64>,
    pub forward_pe: Option<f64>,
    pub ev_ebitda: Option<f64>,
    pub div_yield: Option<f64>,
    pub payout: Option<f64>,
    /// Net institutional buying over the last 6 months, as a fraction of holdings
    pub inst_net_buy_pct: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct Surprise {
    pub quarter: NaiveDate,
    pub actual: f64,
    pub estimate: f64,
    pub surprise_pct: f64,
}

#[derive(Clone, Debug, Default)]
pub struct EstimateTrend {
    pub period: String, // "0q", "+1q", "0y", "+1y"
    pub end: Option<String>,
    pub growth: Option<f64>,
    pub eps_avg: Option<f64>,
    pub revenue_avg: Option<f64>,
    pub eps_now: Option<f64>,
    pub eps_30d: Option<f64>,
    pub eps_90d: Option<f64>,
    pub up_30d: Option<f64>,
    pub down_30d: Option<f64>,
}

#[derive(Clone, Debug, Default)]
pub struct RecTrend {
    pub period: String, // "0m", "-1m", ...
    pub strong_buy: f64,
    pub buy: f64,
    pub hold: f64,
    pub sell: f64,
    pub strong_sell: f64,
}

impl RecTrend {
    pub fn total(&self) -> f64 {
        self.strong_buy + self.buy + self.hold + self.sell + self.strong_sell
    }
    /// Share of analysts at Buy or Strong Buy
    pub fn bullish_share(&self) -> Option<f64> {
        let t = self.total();
        (t > 0.0).then(|| (self.strong_buy + self.buy) / t)
    }
}

#[derive(Clone, Debug)]
pub struct AnalystAction {
    pub ts: i64,
    pub firm: String,
    pub from_grade: String,
    pub to_grade: String,
    pub action: String, // up / down / main / init / reit
    pub pt_action: Option<String>,
    pub target: Option<f64>,
    pub prior_target: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct InsiderTxn {
    pub ts: i64,
    pub name: String,
    pub relation: String,
    pub is_buy: bool,
    pub shares: f64,
    pub value: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Calendar {
    pub next_earnings: Option<i64>,
    pub earnings_is_estimate: bool,
    pub eps_estimate: Option<f64>,
    pub revenue_estimate: Option<f64>,
    pub ex_dividend: Option<i64>,
}

/// Quarterly shareholding pattern (percent, 0–100). Indian stocks only.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Holding {
    pub date: NaiveDate,
    pub promoter: Option<f64>,
    pub fii: Option<f64>,
    pub dii: Option<f64>,
    pub mf: Option<f64>,
    pub retail: Option<f64>,
    pub pledged: Option<bool>,
}

#[derive(Clone, Debug)]
pub struct CorpAction {
    pub ex_date: NaiveDate,
    pub subject: String,
}

/// A headline or official filing.
#[derive(Clone, Debug)]
pub struct Headline {
    pub ts: i64,
    pub title: String,
    pub summary: Option<String>,
    pub publisher: String,
    pub link: Option<String>,
    /// True for exchange filings (NSE corporate announcements)
    pub official: bool,
    /// Filing category for official items (e.g. "Outcome of Board Meeting")
    pub category: Option<String>,
}

/// Daily OHLCV aligned on sessions with a valid close.
#[derive(Clone, Debug, Default)]
pub struct Series {
    pub dates: Vec<NaiveDate>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
    pub volume: Vec<f64>,
}

impl Series {
    pub fn from_chart(c: &ChartData) -> Self {
        let mut s = Series::default();
        let Some(ts) = c.timestamp.as_ref() else {
            return s;
        };
        let Some(q) = c.indicators.quote.first() else {
            return s;
        };
        let get = |v: &Option<Vec<Option<f64>>>, i: usize| {
            v.as_ref().and_then(|x| x.get(i).copied().flatten())
        };
        for (i, &t) in ts.iter().enumerate() {
            let Some(close) = get(&q.close, i).filter(|c| *c > 0.0) else {
                continue;
            };
            let Some(date) = chrono::DateTime::from_timestamp(t, 0).map(|d| d.date_naive()) else {
                continue;
            };
            // Yahoo occasionally emits a duplicate final row for the live session
            if s.dates.last() == Some(&date) {
                s.dates.pop();
                s.open.pop();
                s.high.pop();
                s.low.pop();
                s.close.pop();
                s.volume.pop();
            }
            s.dates.push(date);
            s.close.push(close);
            s.open.push(get(&q.open, i).unwrap_or(close));
            s.high.push(get(&q.high, i).unwrap_or(close));
            s.low.push(get(&q.low, i).unwrap_or(close));
            s.volume.push(
                q.volume
                    .as_ref()
                    .and_then(|v| v.get(i).copied().flatten())
                    .unwrap_or(0) as f64,
            );
        }
        s
    }

    pub fn len(&self) -> usize {
        self.close.len()
    }

    pub fn is_empty(&self) -> bool {
        self.close.is_empty()
    }

    pub fn last(&self) -> Option<f64> {
        self.close.last().copied()
    }

    /// Return over the last `sessions` sessions (fraction).
    pub fn ret(&self, sessions: usize) -> Option<f64> {
        let n = self.len();
        if n <= sessions || sessions == 0 {
            return None;
        }
        Some(self.close[n - 1] / self.close[n - 1 - sessions] - 1.0)
    }

    /// Index of the first session on or after `d`.
    pub fn index_on_or_after(&self, d: NaiveDate) -> Option<usize> {
        let i = self.dates.partition_point(|x| *x < d);
        (i < self.len()).then_some(i)
    }

    pub fn index_of(&self, d: NaiveDate) -> Option<usize> {
        self.dates.binary_search(&d).ok()
    }
}

/// Granular specification of what data sources are needed for a [`Research`] snapshot.
///
/// Many commands only need a subset of the full snapshot (e.g. `score` only reads financial
/// statements; `verdict` and `insights` only read statements, prices, and shareholding).
/// Selective fetching avoids slow and unused network calls (such as NSE announcements/filings
/// and Google/Yahoo RSS feeds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    /// Financial statements, fundamentals time series, and quote summary modules.
    pub statements: bool,
    /// Historical price series and chart data for the target stock.
    pub prices: bool,
    /// Shareholding pattern (NSE) and insider transactions (Yahoo & NSE PIT).
    pub ownership: bool,
    /// Google News and Yahoo RSS headlines.
    pub news: bool,
    /// Official exchange announcements and filings (NSE).
    pub filings: bool,
    /// Exchange corporate actions and dividends (NSE).
    pub corporate_actions: bool,
}

impl Needs {
    /// Fetch all sources (used for `events`, `deep`, and full web research).
    pub const ALL: Self = Self {
        statements: true,
        prices: true,
        ownership: true,
        news: true,
        filings: true,
        corporate_actions: true,
    };

    /// Only financial statements and fundamentals time series (used for `score` / `scorecard`).
    pub const STATEMENTS: Self = Self {
        statements: true,
        prices: false,
        ownership: false,
        news: false,
        filings: false,
        corporate_actions: false,
    };

    /// Statements, price history, and ownership data (used for `verdict`, `insights`, and bare snapshot).
    pub const VERDICT: Self = Self {
        statements: true,
        prices: true,
        ownership: true,
        news: false,
        filings: false,
        corporate_actions: false,
    };

    /// Event timeline sources (price action, ownership, news, filings, corporate actions).
    pub const EVENTS: Self = Self {
        statements: false,
        prices: true,
        ownership: true,
        news: true,
        filings: true,
        corporate_actions: true,
    };

    /// Empty set of needs (only resolves quote).
    pub const NONE: Self = Self {
        statements: false,
        prices: false,
        ownership: false,
        news: false,
        filings: false,
        corporate_actions: false,
    };

    pub fn with_statements(mut self, val: bool) -> Self {
        self.statements = val;
        self
    }

    pub fn with_prices(mut self, val: bool) -> Self {
        self.prices = val;
        self
    }

    pub fn with_ownership(mut self, val: bool) -> Self {
        self.ownership = val;
        self
    }

    pub fn with_news(mut self, val: bool) -> Self {
        self.news = val;
        self
    }

    pub fn with_filings(mut self, val: bool) -> Self {
        self.filings = val;
        self
    }

    pub fn with_corporate_actions(mut self, val: bool) -> Self {
        self.corporate_actions = val;
        self
    }
}

impl Default for Needs {
    fn default() -> Self {
        Self::ALL
    }
}

impl std::ops::BitOr for Needs {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self {
            statements: self.statements || rhs.statements,
            prices: self.prices || rhs.prices,
            ownership: self.ownership || rhs.ownership,
            news: self.news || rhs.news,
            filings: self.filings || rhs.filings,
            corporate_actions: self.corporate_actions || rhs.corporate_actions,
        }
    }
}

impl std::ops::BitOrAssign for Needs {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = *self | rhs;
    }
}

/// Everything we know about one stock.
#[derive(Clone, Debug, Default)]
pub struct Research {
    pub symbol: String,
    /// Ticker without exchange suffix ("LT" for "LT.NS")
    pub base: String,
    pub is_india: bool,
    pub name: String,
    /// Name without legal suffix, used for news search and display
    pub short_name: String,
    pub currency: Option<String>,
    pub quote: Quote,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub fin: Fin,
    /// Quarterly results, oldest first
    pub quarters: Vec<Period>,
    /// Annual results, oldest first
    pub annual: Vec<Period>,
    /// EPS surprises, oldest first
    pub surprises: Vec<Surprise>,
    pub estimates: Vec<EstimateTrend>,
    /// Recommendation trend, current month first
    pub rec_trend: Vec<RecTrend>,
    /// Analyst actions, newest first
    pub actions: Vec<AnalystAction>,
    /// Insider transactions (open-market buys/sells only), newest first
    pub insiders: Vec<InsiderTxn>,
    pub calendar: Calendar,
    /// Shareholding pattern, oldest first
    pub holdings: Vec<Holding>,
    pub corp_actions: Vec<CorpAction>,
    /// Exchange filings, newest first
    pub filings: Vec<Headline>,
    /// News headlines, newest first
    pub news: Vec<Headline>,
    pub prices: Series,
    pub bench: Series,
    pub bench_name: String,
    pub warnings: Vec<String>,
    /// Lookback window for news/events, in days
    pub days: i64,
    /// Snapshot time (unix seconds)
    pub now: i64,
}

impl Research {
    pub fn price(&self) -> Option<f64> {
        self.quote.regular_market_price.or(self.prices.last())
    }

    pub fn csym(&self) -> &'static str {
        market::currency_symbol(self.currency.as_deref())
    }

    /// Banks/NBFCs/insurers: leverage and revenue metrics mean different things
    pub fn is_financial(&self) -> bool {
        let s = format!(
            "{} {}",
            self.sector.as_deref().unwrap_or(""),
            self.industry.as_deref().unwrap_or("")
        )
        .to_lowercase();
        s.contains("financial")
            || s.contains("bank")
            || s.contains("insurance")
            || s.contains("credit")
    }

    /// Fiscal quarter label: India "Q2 FY27", US "Q3 2026"
    pub fn quarter_label(&self, end: NaiveDate) -> String {
        quarter_label(end, self.is_india)
    }
}

pub fn quarter_label(end: NaiveDate, is_india: bool) -> String {
    // Quarter ends that fall in the first days of a month (e.g. 52/53-week
    // fiscal calendars ending Oct 1) belong to the previous month.
    let d = if end.day() <= 7 {
        end - chrono::Duration::days(8)
    } else {
        end
    };
    let (m, y) = (d.month(), d.year());
    if is_india {
        // FY runs Apr–Mar; FY27 = Apr 2026 – Mar 2027
        let (q, fy) = match m {
            4..=6 => (1, y + 1),
            7..=9 => (2, y + 1),
            10..=12 => (3, y + 1),
            _ => (4, y),
        };
        format!("Q{} FY{:02}", q, fy % 100)
    } else {
        format!("Q{} {}", m.div_ceil(3), y)
    }
}

// ───────────────────────────────────────────────────────────────
// Parsers (pure — unit tested)
// ───────────────────────────────────────────────────────────────

fn set_period_field(p: &mut Period, field: &str, v: f64) {
    match field {
        "TotalRevenue" => p.revenue = Some(v),
        "NetIncome" => p.net_income = Some(v),
        "OperatingIncome" => p.operating_income = Some(v),
        "EBITDA" => p.ebitda = Some(v),
        "DilutedEPS" => p.eps = Some(v),
        "OperatingCashFlow" => p.ocf = Some(v),
        "FreeCashFlow" => p.fcf = Some(v),
        "StockholdersEquity" => p.equity = Some(v),
        "TotalDebt" => p.total_debt = Some(v),
        "CashAndCashEquivalents" => p.cash = Some(v),
        "InterestExpense" => p.interest_expense = Some(v),
        "EBIT" => p.ebit = Some(v),
        "TotalAssets" => p.total_assets = Some(v),
        "GrossProfit" => p.gross_profit = Some(v),
        "CurrentAssets" => p.current_assets = Some(v),
        "CurrentLiabilities" => p.current_liabilities = Some(v),
        "RetainedEarnings" => p.retained_earnings = Some(v),
        "WorkingCapital" => p.working_capital = Some(v),
        "OrdinarySharesNumber" | "ShareIssued" => p.shares_outstanding = Some(v),
        "TotalLiabilitiesNetMinorityInterest" | "TotalLiabilities" => p.total_liabilities = Some(v),
        _ => {}
    }
}

pub const TIMESERIES_FIELDS: &[&str] = &[
    "TotalRevenue",
    "NetIncome",
    "OperatingIncome",
    "EBITDA",
    "DilutedEPS",
    "OperatingCashFlow",
    "FreeCashFlow",
    "StockholdersEquity",
    "TotalDebt",
    "CashAndCashEquivalents",
    "InterestExpense",
    "EBIT",
    "TotalAssets",
    "GrossProfit",
    "CurrentAssets",
    "CurrentLiabilities",
    "RetainedEarnings",
    "WorkingCapital",
    "OrdinarySharesNumber",
    "ShareIssued",
    "TotalLiabilitiesNetMinorityInterest",
];

/// Turns statement amounts into the quote currency. Some NSE companies
/// (Infosys among them) report in US dollars while the share trades in rupees;
/// Yahoo marks each statement value with its `currencyCode`.
#[derive(Debug, Clone, Default)]
pub struct StatementFx {
    /// Quote currency; `None` means "take amounts as they are".
    to: Option<String>,
    /// Units of `to` per one unit of the key currency.
    rates: BTreeMap<String, f64>,
}

impl StatementFx {
    pub fn new(to: Option<&str>, rates: impl IntoIterator<Item = (String, f64)>) -> Self {
        Self {
            // Pence count as pounds: statements are never in pence.
            to: to.map(|c| market::to_major(1.0, c).1),
            rates: rates.into_iter().collect(),
        }
    }

    /// Multiplier from `from` into the quote currency, or `None` when the
    /// currencies differ and no rate is known.
    pub fn factor(&self, from: &str) -> Option<f64> {
        match &self.to {
            None => Some(1.0),
            Some(to) if to.eq_ignore_ascii_case(from) => Some(1.0),
            Some(_) => self.rates.get(&from.to_ascii_uppercase()).copied(),
        }
    }
}

/// Currencies the statement values are reported in (`currencyCode`).
pub fn statement_currencies(results: &[Value]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for r in results {
        let Some(t) = r
            .get("meta")
            .and_then(|m| m.get("type"))
            .and_then(|t| t.get(0))
            .and_then(|t| t.as_str())
        else {
            continue;
        };
        for item in r.get(t).and_then(|x| x.as_array()).into_iter().flatten() {
            if let Some(c) = item.get("currencyCode").and_then(|c| c.as_str()) {
                out.insert(c.to_ascii_uppercase());
            }
        }
    }
    out
}

/// Currency of `financialData` (`financialCurrency`), when Yahoo says.
pub fn financial_currency(summary: &Value) -> Option<String> {
    text(summary.get("financialData")?, "financialCurrency").map(|c| c.to_ascii_uppercase())
}

/// Per-share and total amounts convert; share counts do not.
fn is_monetary(field: &str) -> bool {
    !matches!(field, "OrdinarySharesNumber" | "ShareIssued")
}

/// Convert the money fields that `financialData` reports in the company's
/// financial currency. With no rate they are dropped rather than shown in
/// the wrong currency.
pub fn convert_fin(fin: &mut Fin, factor: Option<f64>) {
    for v in [
        &mut fin.total_cash,
        &mut fin.total_debt,
        &mut fin.ebitda,
        &mut fin.fcf,
        &mut fin.ocf,
    ] {
        *v = match (*v, factor) {
            (Some(x), Some(f)) => Some(x * f),
            _ => None,
        };
    }
}

/// Rates from each foreign currency to `to`, from Yahoo's `USDINR=X` style pairs.
/// A rate that cannot be fetched is left out, with a warning.
async fn statement_fx(
    client: &YahooClient,
    to: Option<&str>,
    foreign: &BTreeSet<String>,
    warnings: &mut Vec<String>,
) -> StatementFx {
    let Some(to) = to else {
        return StatementFx::default();
    };
    let to = market::to_major(1.0, to).1;
    let to = to.as_str();
    let mut rates = Vec::new();
    for from in foreign.iter().filter(|c| !c.eq_ignore_ascii_case(to)) {
        let pair = format!("{from}{}=X", to.to_ascii_uppercase());
        let rate = client
            .get_quote(&[pair.as_str()])
            .await
            .ok()
            .and_then(|q| q.into_iter().find_map(|q| q.regular_market_price))
            .filter(|r| *r > 0.0);
        match rate {
            Some(r) => rates.push((from.clone(), r)),
            None => warnings.push(format!(
                "Statements are reported in {from} and the {from}/{to} rate is unavailable, so amounts from the statements are left out"
            )),
        }
    }
    StatementFx::new(Some(to), rates)
}

/// Parse fundamentals-timeseries results into (quarterly, annual) periods,
/// oldest first, with amounts converted to the quote currency by `fx`.
pub fn parse_timeseries(results: &[Value], fx: &StatementFx) -> (Vec<Period>, Vec<Period>) {
    let mut q: BTreeMap<(i32, u32), Period> = BTreeMap::new();
    let mut a: BTreeMap<(i32, u32), Period> = BTreeMap::new();
    for r in results {
        let Some(t) = r
            .get("meta")
            .and_then(|m| m.get("type"))
            .and_then(|t| t.get(0))
            .and_then(|t| t.as_str())
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
        let Some(items) = r.get(t).and_then(|x| x.as_array()) else {
            continue;
        };
        for item in items {
            let Some(date) = item
                .get("asOfDate")
                .and_then(|d| d.as_str())
                .and_then(|d| NaiveDate::parse_from_str(d, "%Y-%m-%d").ok())
            else {
                continue;
            };
            let Some(v) = item
                .get("reportedValue")
                .and_then(|rv| rv.get("raw"))
                .and_then(|x| x.as_f64())
            else {
                continue;
            };
            let v = if is_monetary(field) {
                let from = item.get("currencyCode").and_then(|c| c.as_str());
                match from.map_or(Some(1.0), |c| fx.factor(c)) {
                    Some(f) => v * f,
                    None => continue,
                }
            } else {
                v
            };
            let p = map
                .entry((date.year(), date.month()))
                .or_insert_with(|| Period {
                    end: date,
                    ..Default::default()
                });
            set_period_field(p, field, v);
        }
    }
    (q.into_values().collect(), a.into_values().collect())
}

/// Merge `incomeStatementHistoryQuarterly` into timeseries quarters (fills gaps,
/// usually the most recent quarter which the timeseries lags on).
pub fn merge_income_statement(quarters: &mut Vec<Period>, summary: &Value) {
    let Some(hist) = summary
        .get("incomeStatementHistoryQuarterly")
        .and_then(|m| m.get("incomeStatementHistory"))
        .and_then(|h| h.as_array())
    else {
        return;
    };
    let nz = |v: Option<f64>| v.filter(|x| *x != 0.0);
    for item in hist {
        let Some(end) =
            raw(item, "endDate").and_then(|t| chrono::DateTime::from_timestamp(t as i64, 0))
        else {
            continue;
        };
        let end = end.date_naive();
        let rev = nz(raw(item, "totalRevenue"));
        let ni = nz(raw(item, "netIncome"));
        let oi = nz(raw(item, "operatingIncome"));
        match quarters
            .iter_mut()
            .find(|p| p.end.year() == end.year() && p.end.month() == end.month())
        {
            Some(p) => {
                p.revenue = p.revenue.or(rev);
                p.net_income = p.net_income.or(ni);
                p.operating_income = p.operating_income.or(oi);
            }
            None => quarters.push(Period {
                end,
                revenue: rev,
                net_income: ni,
                operating_income: oi,
                ..Default::default()
            }),
        }
    }
    quarters.sort_by_key(|p| p.end);
}

/// Merge `balanceSheetHistory` into annual periods (fills gaps in assets, liabilities, working capital, retained earnings).
pub fn merge_balance_sheet(annual: &mut Vec<Period>, summary: &Value) {
    let Some(hist) = summary
        .get("balanceSheetHistory")
        .and_then(|m| m.get("balanceSheetStatements"))
        .and_then(|h| h.as_array())
    else {
        return;
    };
    let nz = |v: Option<f64>| v.filter(|x| *x != 0.0);
    for item in hist {
        let Some(end) =
            raw(item, "endDate").and_then(|t| chrono::DateTime::from_timestamp(t as i64, 0))
        else {
            continue;
        };
        let end = end.date_naive();
        let assets = nz(raw(item, "totalAssets"));
        let liab =
            nz(raw(item, "totalLiab").or_else(|| raw(item, "totalLiabilitiesNetMinorityInterest")));
        let ca = nz(raw(item, "totalCurrentAssets"));
        let cl = nz(raw(item, "totalCurrentLiabilities"));
        let re = nz(raw(item, "retainedEarnings"));
        let eq = nz(raw(item, "totalStockholderEquity"));
        let debt = nz(raw(item, "shortLongTermDebt").or_else(|| raw(item, "longTermDebt")));

        match annual.iter_mut().find(|p| p.end.year() == end.year()) {
            Some(p) => {
                p.total_assets = p.total_assets.or(assets);
                p.total_liabilities = p.total_liabilities.or(liab);
                p.current_assets = p.current_assets.or(ca);
                p.current_liabilities = p.current_liabilities.or(cl);
                p.retained_earnings = p.retained_earnings.or(re);
                p.equity = p.equity.or(eq);
                p.total_debt = p.total_debt.or(debt);
            }
            None => annual.push(Period {
                end,
                total_assets: assets,
                total_liabilities: liab,
                current_assets: ca,
                current_liabilities: cl,
                retained_earnings: re,
                equity: eq,
                total_debt: debt,
                ..Default::default()
            }),
        }
    }
    annual.sort_by_key(|p| p.end);
}

pub fn parse_fin(summary: &Value, quote: &Quote) -> Fin {
    let empty = Value::Null;
    let fd = summary.get("financialData").unwrap_or(&empty);
    let ks = summary.get("defaultKeyStatistics").unwrap_or(&empty);
    let sd = summary.get("summaryDetail").unwrap_or(&empty);
    let nsp = summary.get("netSharePurchaseActivity").unwrap_or(&empty);
    let mh = summary.get("majorHoldersBreakdown").unwrap_or(&empty);
    Fin {
        target_low: raw(fd, "targetLowPrice"),
        target_mean: raw(fd, "targetMeanPrice").or(quote.target_mean_price),
        target_median: raw(fd, "targetMedianPrice"),
        target_high: raw(fd, "targetHighPrice"),
        rec_mean: raw(fd, "recommendationMean").or(quote.recommendation_mean),
        rec_key: text(fd, "recommendationKey").or(quote.recommendation_key.clone()),
        n_analysts: raw(fd, "numberOfAnalystOpinions")
            .or(quote.number_of_analyst_opinions.map(|n| n as f64)),
        total_cash: raw(fd, "totalCash"),
        total_debt: raw(fd, "totalDebt"),
        ebitda: raw(fd, "ebitda"),
        revenue_growth: raw(fd, "revenueGrowth").or(quote.revenue_growth),
        earnings_growth: raw(fd, "earningsGrowth").or(quote.earnings_quarterly_growth),
        gross_margin: raw(fd, "grossMargins"),
        op_margin: raw(fd, "operatingMargins"),
        profit_margin: raw(fd, "profitMargins").or(quote.profit_margins),
        roe: raw(fd, "returnOnEquity").or(quote.return_on_equity),
        roa: raw(fd, "returnOnAssets"),
        fcf: raw(fd, "freeCashflow"),
        ocf: raw(fd, "operatingCashflow"),
        current_ratio: raw(fd, "currentRatio").or(quote.current_ratio),
        de: raw(fd, "debtToEquity").or(quote.debt_to_equity),
        shares: raw(ks, "sharesOutstanding"),
        insiders_pct: raw(ks, "heldPercentInsiders").or(raw(mh, "insidersPercentHeld")),
        inst_pct: raw(ks, "heldPercentInstitutions").or(raw(mh, "institutionsPercentHeld")),
        short_pct_float: raw(ks, "shortPercentOfFloat"),
        short_ratio: raw(ks, "shortRatio"),
        peg: raw(ks, "pegRatio").or(raw(ks, "trailingPegRatio")),
        book_value: raw(ks, "bookValue").or(quote.book_value),
        forward_eps: raw(ks, "forwardEps").or(quote.eps_forward),
        trailing_eps: raw(ks, "trailingEps").or(quote.eps_trailing_twelve_months),
        pb: raw(ks, "priceToBook").or(quote.price_to_book),
        beta: raw(sd, "beta").or(raw(ks, "beta")).or(quote.beta),
        trailing_pe: raw(sd, "trailingPE").or(quote.trailing_pe),
        forward_pe: raw(sd, "forwardPE")
            .or(raw(ks, "forwardPE"))
            .or(quote.forward_pe),
        ev_ebitda: raw(ks, "enterpriseToEbitda").or(quote.enterprise_to_ebitda),
        div_yield: raw(sd, "dividendYield")
            .or(raw(sd, "trailingAnnualDividendYield"))
            .or(quote.dividend_yield_fraction()),
        payout: raw(sd, "payoutRatio"),
        inst_net_buy_pct: raw(nsp, "netInstBuyingPercent"),
    }
}

pub fn parse_surprises(summary: &Value) -> Vec<Surprise> {
    let mut out: Vec<Surprise> = summary
        .get("earningsHistory")
        .and_then(|e| e.get("history"))
        .and_then(|h| h.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    let quarter = chrono::DateTime::from_timestamp(raw(h, "quarter")? as i64, 0)?
                        .date_naive();
                    let actual = raw(h, "epsActual")?;
                    let estimate = raw(h, "epsEstimate")?;
                    let surprise_pct = raw(h, "surprisePercent").unwrap_or_else(|| {
                        if estimate.abs() > 1e-9 {
                            (actual - estimate) / estimate.abs()
                        } else {
                            0.0
                        }
                    });
                    Some(Surprise {
                        quarter,
                        actual,
                        estimate,
                        surprise_pct,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|s| s.quarter);
    out
}

pub fn parse_estimates(summary: &Value) -> Vec<EstimateTrend> {
    summary
        .get("earningsTrend")
        .and_then(|e| e.get("trend"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let empty = Value::Null;
                    let ee = t.get("earningsEstimate").unwrap_or(&empty);
                    let re = t.get("revenueEstimate").unwrap_or(&empty);
                    let et = t.get("epsTrend").unwrap_or(&empty);
                    let er = t.get("epsRevisions").unwrap_or(&empty);
                    Some(EstimateTrend {
                        period: text(t, "period")?,
                        end: text(t, "endDate"),
                        growth: raw(t, "growth"),
                        eps_avg: raw(ee, "avg"),
                        revenue_avg: raw(re, "avg"),
                        eps_now: raw(et, "current"),
                        eps_30d: raw(et, "30daysAgo"),
                        eps_90d: raw(et, "90daysAgo"),
                        up_30d: raw(er, "upLast30days"),
                        down_30d: raw(er, "downLast30days"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_rec_trend(summary: &Value) -> Vec<RecTrend> {
    summary
        .get("recommendationTrend")
        .and_then(|e| e.get("trend"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let r = RecTrend {
                        period: text(t, "period")?,
                        strong_buy: raw(t, "strongBuy").unwrap_or(0.0),
                        buy: raw(t, "buy").unwrap_or(0.0),
                        hold: raw(t, "hold").unwrap_or(0.0),
                        sell: raw(t, "sell").unwrap_or(0.0),
                        strong_sell: raw(t, "strongSell").unwrap_or(0.0),
                    };
                    (r.total() > 0.0).then_some(r)
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_actions(summary: &Value) -> Vec<AnalystAction> {
    let mut out: Vec<AnalystAction> = summary
        .get("upgradeDowngradeHistory")
        .and_then(|e| e.get("history"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|h| {
                    Some(AnalystAction {
                        ts: raw(h, "epochGradeDate")? as i64,
                        firm: text(h, "firm")?,
                        from_grade: text(h, "fromGrade").unwrap_or_default(),
                        to_grade: text(h, "toGrade").unwrap_or_default(),
                        action: text(h, "action").unwrap_or_default(),
                        pt_action: text(h, "priceTargetAction"),
                        target: raw(h, "currentPriceTarget").filter(|t| *t > 0.0),
                        prior_target: raw(h, "priorPriceTarget").filter(|t| *t > 0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by_key(|a| std::cmp::Reverse(a.ts));
    out
}

/// Yahoo insider transactions → open-market buys/sells only
/// (gifts, option exercises and awards are not conviction signals).
pub fn parse_yahoo_insiders(summary: &Value) -> Vec<InsiderTxn> {
    summary
        .get("insiderTransactions")
        .and_then(|e| e.get("transactions"))
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let txt = text(t, "transactionText")
                        .unwrap_or_default()
                        .to_lowercase();
                    let is_buy = if txt.contains("purchase") || txt.starts_with("buy") {
                        true
                    } else if txt.contains("sale") {
                        false
                    } else {
                        return None;
                    };
                    Some(InsiderTxn {
                        ts: raw(t, "startDate")? as i64,
                        name: text(t, "filerName").unwrap_or_default(),
                        relation: text(t, "filerRelation").unwrap_or_default(),
                        is_buy,
                        shares: raw(t, "shares").unwrap_or(0.0),
                        value: raw(t, "value").unwrap_or(0.0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_calendar(summary: &Value) -> Calendar {
    let empty = Value::Null;
    let ce = summary.get("calendarEvents").unwrap_or(&empty);
    let e = ce.get("earnings").unwrap_or(&empty);
    let next_earnings = e
        .get("earningsDate")
        .and_then(|d| d.as_array())
        .and_then(|a| a.first())
        .and_then(|d| d.get("raw").and_then(|r| r.as_f64()))
        .map(|t| t as i64);
    Calendar {
        next_earnings,
        earnings_is_estimate: e
            .get("isEarningsDateEstimate")
            .and_then(|b| b.as_bool())
            .unwrap_or(false),
        eps_estimate: raw(e, "earningsAverage"),
        revenue_estimate: raw(e, "revenueAverage"),
        ex_dividend: raw(ce, "exDividendDate").map(|t| t as i64),
    }
}

/// Extract shareholding percentages from an NSE/BSE SHP XBRL document.
/// Returns (context → fraction) for `ShareholdingAsAPercentageOfTotalNumberOfShares`.
pub fn parse_shp_xbrl(xml: &str) -> BTreeMap<String, f64> {
    let tag = "<in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares";
    let mut out = BTreeMap::new();
    for chunk in xml.split(tag).skip(1) {
        let Some(ctx_start) = chunk.find("contextRef=\"") else {
            continue;
        };
        let rest = &chunk[ctx_start + 12..];
        let Some(ctx_end) = rest.find('"') else {
            continue;
        };
        let ctx = &rest[..ctx_end];
        let Some(gt) = chunk.find('>') else { continue };
        let val = &chunk[gt + 1..];
        let Some(lt) = val.find('<') else { continue };
        if let Ok(v) = val[..lt].trim().parse::<f64>() {
            out.entry(ctx.to_string()).or_insert(v);
        }
    }
    out
}

pub fn holding_from_xbrl(
    xml: &str,
    date: NaiveDate,
    promoter_hint: Option<f64>,
) -> Option<Holding> {
    let m = parse_shp_xbrl(xml);
    if m.is_empty() {
        return None;
    }
    let pct = |k: &str| m.get(&format!("{}_ContextI", k)).map(|v| v * 100.0);
    let retail = match (
        pct("ResidentIndividualShareholdersHoldingNominalShareCapitalUpToRsTwoLakh"),
        pct("ResidentIndividualShareholdersHoldingNominalShareCapitalInExcessOfRsTwoLakh"),
    ) {
        (Some(a), Some(b)) => Some(a + b),
        (a, b) => a.or(b),
    };
    let pledged = xml
        .split("<in-bse-shp:WhetherAnySharesHeldByPromotersAreEncumberedUnderPledged ")
        .nth(1)
        .and_then(|c| c.split('>').nth(1))
        .and_then(|c| c.split('<').next())
        .map(|v| v.trim().eq_ignore_ascii_case("true") || v.trim().eq_ignore_ascii_case("yes"));
    Some(Holding {
        date,
        promoter: pct("ShareholdingOfPromoterAndPromoterGroup").or(promoter_hint),
        fii: pct("InstitutionsForeign"),
        dii: pct("InstitutionsDomestic"),
        mf: pct("MutualFundsOrUTI"),
        retail,
        pledged,
    })
}

fn parse_nse_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    let first = s.split_whitespace().next()?;
    // NSE mixes "30-SEP-2026" and "22-May-2026"; normalise case for chrono.
    let mut norm = first.to_lowercase();
    if let Some(i) = norm.find('-') {
        let (a, b) = norm.split_at(i + 1);
        let mut c = b.chars();
        if let Some(f) = c.next() {
            norm = format!("{}{}{}", a, f.to_ascii_uppercase(), c.as_str());
        }
    }
    NaiveDate::parse_from_str(&norm, "%d-%b-%Y").ok()
}

fn parse_nse_datetime_ist(s: &str) -> Option<i64> {
    // "27-Oct-2026 22:13:16" or "18-Feb-2026 19:06" in IST
    let date = parse_nse_date(s)?;
    let time = s.split_whitespace().nth(1).unwrap_or("12:00");
    let t = chrono::NaiveTime::parse_from_str(time, "%H:%M:%S")
        .or_else(|_| chrono::NaiveTime::parse_from_str(time, "%H:%M"))
        .unwrap_or_else(|_| chrono::NaiveTime::from_hms_opt(12, 0, 0).unwrap());
    let ist = NaiveDateTime::new(date, t);
    Some(ist.and_utc().timestamp() - 19_800) // IST = UTC+5:30
}

pub fn parse_nse_filings(v: &Value) -> Vec<Headline> {
    let arr = v.as_array().cloned().unwrap_or_default();
    let mut out: Vec<Headline> = arr
        .iter()
        .filter_map(|f| {
            let ts = text(f, "an_dt")
                .or_else(|| text(f, "sort_date"))
                .and_then(|d| parse_nse_datetime_ist(&d))?;
            let category = text(f, "desc");
            let body = text(f, "attchmntText").unwrap_or_default();
            let title = if body.is_empty() {
                category.clone()?
            } else {
                body
            };
            Some(Headline {
                ts,
                title,
                summary: None,
                publisher: "NSE filing".into(),
                link: text(f, "attchmntFile"),
                official: true,
                category,
            })
        })
        .collect();
    out.sort_by_key(|h| std::cmp::Reverse(h.ts));
    out
}

/// NSE PIT (insider trading) disclosures → promoter/director/KMP open-market trades
pub fn parse_nse_pit(v: &Value) -> Vec<InsiderTxn> {
    let arr = v
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    arr.iter()
        .filter_map(|t| {
            let category = text(t, "personCategory").unwrap_or_default();
            let cat_l = category.to_lowercase();
            if !(cat_l.contains("promoter")
                || cat_l.contains("director")
                || cat_l.contains("key managerial")
                || cat_l.contains("kmp"))
            {
                return None;
            }
            let mode = text(t, "acqMode").unwrap_or_default().to_lowercase();
            if !mode.contains("market") || mode.contains("off market") {
                return None;
            }
            let tt = text(t, "tdpTransactionType")
                .unwrap_or_default()
                .to_lowercase();
            let is_buy = if tt.contains("buy") || tt.contains("acq") {
                true
            } else if tt.contains("sell") || tt.contains("disp") {
                false
            } else {
                return None;
            };
            let ts = text(t, "date")
                .and_then(|d| parse_nse_datetime_ist(&d))
                .or_else(|| text(t, "acqtoDt").and_then(|d| parse_nse_datetime_ist(&d)))?;
            Some(InsiderTxn {
                ts,
                name: text(t, "acqName").unwrap_or_default(),
                relation: category,
                is_buy,
                shares: nse_num(t, "secAcq").unwrap_or(0.0),
                value: nse_num(t, "secVal").unwrap_or(0.0),
            })
        })
        .collect()
}

pub fn parse_nse_corp_actions(v: &Value) -> Vec<CorpAction> {
    v.as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    Some(CorpAction {
                        ex_date: parse_nse_date(&text(c, "exDate")?)?,
                        subject: text(c, "subject")?,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── RSS ──

fn xml_unescape(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(s);
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)?;
    Some(&s[start..start + end])
}

/// Strip HTML tags (for RSS descriptions)
fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Parse an RSS feed. Google News titles end in " - Publisher", which is stripped.
pub fn parse_rss(xml: &str, default_publisher: &str) -> Vec<Headline> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let item = &chunk[..chunk.find("</item>").unwrap_or(chunk.len())];
            let mut title = xml_unescape(between(item, "<title>", "</title>")?);
            let publisher = between(item, "<source", "</source>")
                .and_then(|s| s.split_once('>').map(|(_, name)| xml_unescape(name)))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| default_publisher.to_string());
            let suffix = format!(" - {}", publisher);
            if title.ends_with(&suffix) {
                title.truncate(title.len() - suffix.len());
            }
            if title.is_empty() {
                return None;
            }
            let ts = between(item, "<pubDate>", "</pubDate>")
                .and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok())
                .map(|d| d.timestamp())?;
            let summary = between(item, "<description>", "</description>")
                .map(|d| strip_tags(&xml_unescape(d)))
                .filter(|d| d.len() > 40 && !d.contains(&title));
            let link = between(item, "<link>", "</link>").map(xml_unescape);
            Some(Headline {
                ts,
                title,
                summary,
                publisher,
                link,
                official: false,
                category: None,
            })
        })
        .collect()
}

/// Names/aliases used to decide whether a headline is really about this company.
pub fn company_aliases(short_name: &str, base: &str) -> Vec<String> {
    let mut a = vec![short_name.to_lowercase()];
    let words: Vec<&str> = short_name.split_whitespace().collect();
    // "Larsen & Toubro" → "l&t"
    if words.len() == 3 && words[1] == "&" {
        let i1 = words[0].chars().next().unwrap_or_default();
        let i2 = words[2].chars().next().unwrap_or_default();
        a.push(format!("{}&{}", i1, i2).to_lowercase());
    }
    if let Some(first) = words.first()
        && first.len() >= 5
    {
        a.push(first.to_lowercase());
    }
    if base.len() >= 3 {
        a.push(base.to_lowercase());
    }
    a
}

pub fn mentions_company(h: &Headline, aliases: &[String]) -> bool {
    let hay = format!("{} {}", h.title, h.summary.as_deref().unwrap_or("")).to_lowercase();
    aliases.iter().any(|a| {
        hay.match_indices(a.as_str()).any(|(i, _)| {
            // word boundary check (so "LT" doesn't match "result")
            let before = hay[..i].chars().last().is_none_or(|c| !c.is_alphanumeric());
            let after = hay[i + a.len()..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric());
            before && after
        })
    })
}

/// Listing pages / live blogs that carry no event information
pub fn is_noise_headline(title: &str) -> bool {
    let t = title.to_lowercase();
    [
        "stock price, news, quote",
        "share price today live",
        "share price live",
        "live updates",
        "stock price today",
        "share price today",
        "stock price live",
        "price, news",
        "stocks to watch",
        "stocks to buy",
        "top gainers",
        "top losers",
        "market live",
        "sensex today",
        "nifty today",
    ]
    .iter()
    .any(|p| t.contains(p))
}

// ───────────────────────────────────────────────────────────────
// Fetch
// ───────────────────────────────────────────────────────────────

const SUMMARY_MODULES: &[&str] = &[
    "financialData",
    "defaultKeyStatistics",
    "summaryDetail",
    "assetProfile",
    "earningsHistory",
    "earningsTrend",
    "recommendationTrend",
    "upgradeDowngradeHistory",
    "calendarEvents",
    "insiderTransactions",
    "netSharePurchaseActivity",
    "majorHoldersBreakdown",
    "incomeStatementHistoryQuarterly",
    "balanceSheetHistory",
    "balanceSheetHistoryQuarterly",
];

async fn fetch_google_news(
    client: &YahooClient,
    short_name: &str,
    is_india: bool,
    days: i64,
) -> Result<Vec<Headline>> {
    let q = if is_india {
        format!("\"{}\" when:{}d", short_name, days)
    } else {
        format!("\"{}\" stock when:{}d", short_name, days)
    };
    let locale = if is_india {
        "hl=en-IN&gl=IN&ceid=IN:en"
    } else {
        "hl=en-US&gl=US&ceid=US:en"
    };
    let url = format!(
        "https://news.google.com/rss/search?q={}&{}",
        url_encode(&q),
        locale
    );
    let xml = client.raw_get_text(&url).await?;
    Ok(parse_rss(&xml, "Google News"))
}

async fn fetch_yahoo_rss(client: &YahooClient, symbol: &str) -> Result<Vec<Headline>> {
    let url = format!(
        "https://feeds.finance.yahoo.com/rss/2.0/headline?s={}&region=US&lang=en-US",
        symbol
    );
    let xml = client.raw_get_text(&url).await?;
    Ok(parse_rss(&xml, "Yahoo Finance"))
}

async fn fetch_nse_filings(client: &YahooClient, sym: &str, days: i64) -> Result<Vec<Headline>> {
    let to = Utc::now().date_naive();
    let from = to - chrono::Duration::days(days);
    let url = format!(
        "https://www.nseindia.com/api/corporate-announcements?index=equities&symbol={}&from_date={}&to_date={}",
        url_encode(sym),
        from.format("%d-%m-%Y"),
        to.format("%d-%m-%Y")
    );
    let v = client.nse_get_json(&url, Duration::from_secs(900)).await?;
    Ok(parse_nse_filings(&v))
}

async fn fetch_nse_holdings(client: &YahooClient, sym: &str) -> Result<Vec<Holding>> {
    let url = format!(
        "https://www.nseindia.com/api/corporate-share-holdings-master?index=equities&symbol={}",
        url_encode(sym)
    );
    let master = client
        .nse_get_json(&url, Duration::from_secs(6 * 3600))
        .await?;
    let entries = master.as_array().cloned().unwrap_or_default();
    // Latest 5 quarters → QoQ and YoY changes. Revised filings repeat a date;
    // keep the first (most recent) per quarter.
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for e in entries {
        if out.len() >= 5 {
            break;
        }
        let Some(date) = text(&e, "date").and_then(|d| parse_nse_date(&d)) else {
            continue;
        };
        let Some(xbrl) = text(&e, "xbrl") else {
            continue;
        };
        if !seen.insert(date) {
            continue;
        }
        let promoter_hint = nse_num(&e, "pr_and_prgrp");
        // Filed XBRL never changes — cache for a week.
        if let Ok(xml) = client
            .nse_get_text(&xbrl, Duration::from_secs(7 * 86_400))
            .await
            && let Some(h) = holding_from_xbrl(&xml, date, promoter_hint)
        {
            out.push(h);
        }
    }
    out.sort_by_key(|h| h.date);
    Ok(out)
}

async fn fetch_nse_pit(client: &YahooClient, sym: &str) -> Result<Vec<InsiderTxn>> {
    let url = format!(
        "https://www.nseindia.com/api/corporates-pit?index=equities&symbol={}",
        url_encode(sym)
    );
    let v = client.nse_get_json(&url, Duration::from_secs(3600)).await?;
    Ok(parse_nse_pit(&v))
}

async fn fetch_nse_corp_actions(client: &YahooClient, sym: &str) -> Result<Vec<CorpAction>> {
    let url = format!(
        "https://www.nseindia.com/api/corporates-corporateActions?index=equities&symbol={}",
        url_encode(sym)
    );
    let v = client
        .nse_get_json(&url, Duration::from_secs(6 * 3600))
        .await?;
    Ok(parse_nse_corp_actions(&v))
}

/// Unwrap an optional source, recording a warning on failure.
fn soft<T: Default>(r: Result<T>, what: &str, warnings: &mut Vec<String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => {
            warnings.push(format!("{} unavailable ({})", what, e));
            T::default()
        }
    }
}

/// Fetch a full research snapshot (`Needs::ALL`). `days` is the news/event lookback window.
pub async fn fetch_all(
    client: &YahooClient,
    symbol: &str,
    market: Market,
    days: i64,
) -> Result<Research> {
    fetch(client, symbol, market, days, Needs::ALL).await
}

/// Fetch a research snapshot according to the specified data [`Needs`].
/// `days` is the news/event lookback window.
/// Error text for a symbol with no usable quote.
fn not_found_message(symbol: &str) -> String {
    if market::is_bse_scrip_code(symbol) {
        format!("Symbol '{symbol}' not found. {}", market::BSE_CODE_HINT)
    } else {
        format!("Symbol '{symbol}' not found")
    }
}

pub async fn fetch(
    client: &YahooClient,
    symbol: &str,
    market: Market,
    days: i64,
    needs: Needs,
) -> Result<Research> {
    let symbol = market::resolve_symbol(symbol, market);
    let quote = client
        .get_quote(&[symbol.as_str()])
        .await?
        .into_iter()
        .find(Quote::is_usable)
        .with_context(|| not_found_message(&symbol))?;

    let name = quote
        .long_name
        .clone()
        .or(quote.short_name.clone())
        .unwrap_or_else(|| symbol.clone());
    let short_name = clean_company_name(&name);
    let base = symbol.split('.').next().unwrap_or(&symbol).to_string();
    let is_india = symbol.ends_with(".NS")
        || symbol.ends_with(".BO")
        || market::is_inr(quote.currency.as_deref());
    let nse_sym = symbol.ends_with(".NS").then(|| base.clone());
    let (bench_sym, bench_name) = if is_india {
        ("^NSEI", "NIFTY 50")
    } else {
        ("^GSPC", "S&P 500")
    };
    let ts_types: Vec<String> = TIMESERIES_FIELDS
        .iter()
        .flat_map(|f| [format!("quarterly{}", f), format!("annual{}", f)])
        .collect();
    let ts_refs: Vec<&str> = ts_types.iter().map(|s| s.as_str()).collect();

    let nse = nse_sym.as_deref();
    let (summary, timeseries, prices, bench, gnews, yrss, filings, holdings, pit, corp) = tokio::join!(
        async {
            if needs.statements || needs.ownership || needs.prices {
                client.quote_summary(&symbol, SUMMARY_MODULES).await
            } else {
                Ok(Value::Null)
            }
        },
        async {
            if needs.statements {
                client
                    .fundamentals_timeseries(&symbol, &ts_refs, 365 * 5)
                    .await
            } else {
                Ok(Vec::new())
            }
        },
        async {
            if needs.prices {
                client
                    .get_chart(&symbol, "2y", "1d")
                    .await
                    .map(|c| Series::from_chart(&c))
            } else {
                Ok(Series::default())
            }
        },
        async {
            if needs.prices && (needs.news || needs.filings || needs.corporate_actions) {
                client
                    .get_chart(bench_sym, "2y", "1d")
                    .await
                    .map(|c| Series::from_chart(&c))
            } else {
                Ok(Series::default())
            }
        },
        async {
            if needs.news {
                fetch_google_news(client, &short_name, is_india, days).await
            } else {
                Ok(vec![])
            }
        },
        async {
            if needs.news {
                fetch_yahoo_rss(client, &symbol).await
            } else {
                Ok(vec![])
            }
        },
        async {
            if !needs.filings {
                return Ok(vec![]);
            }
            match nse {
                Some(s) => {
                    match tokio::time::timeout(
                        Duration::from_secs(3),
                        fetch_nse_filings(client, s, days),
                    )
                    .await
                    {
                        Ok(res) => res,
                        Err(_) => anyhow::bail!("timed out after 3s"),
                    }
                }
                None => Ok(vec![]),
            }
        },
        async {
            if !needs.ownership {
                return Ok(vec![]);
            }
            match nse {
                Some(s) => {
                    match tokio::time::timeout(
                        Duration::from_secs(3),
                        fetch_nse_holdings(client, s),
                    )
                    .await
                    {
                        Ok(res) => res,
                        Err(_) => anyhow::bail!("timed out after 3s"),
                    }
                }
                None => Ok(vec![]),
            }
        },
        async {
            if !needs.ownership {
                return Ok(vec![]);
            }
            match nse {
                Some(s) => {
                    match tokio::time::timeout(Duration::from_secs(3), fetch_nse_pit(client, s))
                        .await
                    {
                        Ok(res) => res,
                        Err(_) => anyhow::bail!("timed out after 3s"),
                    }
                }
                None => Ok(vec![]),
            }
        },
        async {
            if !needs.corporate_actions {
                return Ok(vec![]);
            }
            match nse {
                Some(s) => {
                    match tokio::time::timeout(
                        Duration::from_secs(3),
                        fetch_nse_corp_actions(client, s),
                    )
                    .await
                    {
                        Ok(res) => res,
                        Err(_) => anyhow::bail!("timed out after 3s"),
                    }
                }
                None => Ok(vec![]),
            }
        },
    );

    let mut warnings = Vec::new();
    let summary = soft(summary, "Yahoo fundamentals", &mut warnings);
    let timeseries = soft(timeseries, "Yahoo financial statements", &mut warnings);
    let prices = soft(prices, "Price history", &mut warnings);
    let bench = soft(bench, "Benchmark history", &mut warnings);
    let gnews = soft(gnews, "Google News", &mut warnings);
    let yrss = soft(yrss, "Yahoo RSS", &mut warnings);
    let filings = soft(filings, "NSE filings", &mut warnings);
    let holdings = soft(holdings, "NSE shareholding", &mut warnings);
    let pit = soft(pit, "NSE insider trades", &mut warnings);
    let corp_actions = soft(corp, "NSE corporate actions", &mut warnings);

    // Statements may be reported in another currency than the share trades in.
    let fin_ccy = financial_currency(&summary);
    let mut foreign = statement_currencies(&timeseries);
    foreign.extend(fin_ccy.clone());
    let fx = statement_fx(client, quote.currency.as_deref(), &foreign, &mut warnings).await;

    let (quarters, annual) = if needs.statements {
        let (mut q, mut a) = parse_timeseries(&timeseries, &fx);
        merge_income_statement(&mut q, &summary);
        merge_balance_sheet(&mut a, &summary);
        (q, a)
    } else {
        (vec![], vec![])
    };

    let now = Utc::now().timestamp();
    let cutoff = now - days * 86_400;
    let aliases = company_aliases(&short_name, &base);
    let news: Vec<Headline> = if needs.news {
        let mut n: Vec<Headline> = gnews
            .into_iter()
            .chain(yrss.into_iter().filter(|h| mentions_company(h, &aliases)))
            .filter(|h| h.ts >= cutoff && h.ts <= now + 3600 && !is_noise_headline(&h.title))
            .collect();
        n.sort_by_key(|h| std::cmp::Reverse(h.ts));
        n
    } else {
        vec![]
    };

    let insiders = if needs.ownership {
        let mut ins = parse_yahoo_insiders(&summary);
        ins.extend(pit);
        ins.sort_by_key(|t| std::cmp::Reverse(t.ts));
        ins
    } else {
        vec![]
    };

    let profile = summary.get("assetProfile");
    let sector = profile
        .and_then(|p| text(p, "sector"))
        .or(quote.sector.clone());
    let industry = profile
        .and_then(|p| text(p, "industry"))
        .or(quote.industry.clone());

    let mut fin = parse_fin(&summary, &quote);
    if let Some(c) = &fin_ccy {
        convert_fin(&mut fin, fx.factor(c));
    }

    Ok(Research {
        fin,
        surprises: parse_surprises(&summary),
        estimates: parse_estimates(&summary),
        rec_trend: parse_rec_trend(&summary),
        actions: parse_actions(&summary),
        calendar: parse_calendar(&summary),
        currency: quote.currency.clone(),
        symbol,
        base,
        is_india,
        name,
        short_name,
        quote,
        sector,
        industry,
        quarters,
        annual,
        insiders,
        holdings,
        corp_actions,
        filings,
        news,
        prices,
        bench,
        bench_name: bench_name.to_string(),
        warnings,
        days,
        now,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_clean_company_name() {
        assert_eq!(
            clean_company_name("Larsen & Toubro Limited"),
            "Larsen & Toubro"
        );
        assert_eq!(clean_company_name("Apple Inc."), "Apple");
        assert_eq!(clean_company_name("Tata Motors Ltd"), "Tata Motors");
        assert_eq!(clean_company_name("Microsoft Corporation"), "Microsoft");
    }

    #[test]
    fn test_url_encode() {
        assert_eq!(
            url_encode("\"L & T\" when:30d"),
            "%22L%20%26%20T%22%20when%3A30d"
        );
    }

    #[test]
    fn test_quarter_label_india_and_us() {
        let d = |y, m, dd| NaiveDate::from_ymd_opt(y, m, dd).unwrap();
        assert_eq!(quarter_label(d(2026, 9, 30), true), "Q2 FY27");
        assert_eq!(quarter_label(d(2026, 3, 31), true), "Q4 FY26");
        assert_eq!(quarter_label(d(2026, 12, 31), true), "Q3 FY27");
        assert_eq!(quarter_label(d(2026, 9, 27), false), "Q3 2026");
        // 52/53-week calendar ending in the first days of a month
        assert_eq!(quarter_label(d(2026, 10, 2), false), "Q3 2026");
    }

    #[test]
    fn test_parse_timeseries_and_merge() {
        let ts = vec![
            json!({"meta": {"type": ["quarterlyTotalRevenue"]}, "quarterlyTotalRevenue": [
                {"asOfDate": "2026-03-31", "reportedValue": {"raw": 100.0}},
                null,
                {"asOfDate": "2026-06-30", "reportedValue": {"raw": 110.0}}
            ]}),
            json!({"meta": {"type": ["annualNetIncome"]}, "annualNetIncome": [
                {"asOfDate": "2026-03-31", "reportedValue": {"raw": 40.0}}
            ]}),
        ];
        let (mut q, a) = parse_timeseries(&ts, &StatementFx::default());
        assert_eq!(q.len(), 2);
        assert_eq!(q[1].revenue, Some(110.0));
        assert_eq!(a[0].net_income, Some(40.0));

        let summary = json!({"incomeStatementHistoryQuarterly": {"incomeStatementHistory": [
            {"endDate": {"raw": 1790726400}, "totalRevenue": {"raw": 120.0}, "netIncome": {"raw": 12.0}},
            {"endDate": {"raw": 1782777600}, "totalRevenue": {"raw": 999.0}, "netIncome": {"raw": 11.0}}
        ]}});
        merge_income_statement(&mut q, &summary);
        assert_eq!(q.len(), 3);
        assert_eq!(q[2].revenue, Some(120.0)); // new latest quarter appended
        assert_eq!(q[1].revenue, Some(110.0)); // existing value preserved
        assert_eq!(q[1].net_income, Some(11.0)); // gap filled
    }

    fn usd_item(date: &str, raw: f64) -> Value {
        json!({"asOfDate": date, "currencyCode": "USD", "reportedValue": {"raw": raw}})
    }

    fn usd_statements() -> Vec<Value> {
        vec![
            json!({"meta": {"type": ["quarterlyTotalRevenue"]},
                   "quarterlyTotalRevenue": [usd_item("2026-06-30", 5_082_000_000.0)]}),
            json!({"meta": {"type": ["annualDilutedEPS"]},
                   "annualDilutedEPS": [usd_item("2026-03-31", 0.8)]}),
            json!({"meta": {"type": ["annualOrdinarySharesNumber"]},
                   "annualOrdinarySharesNumber": [
                       {"asOfDate": "2026-03-31", "reportedValue": {"raw": 4_050_000_000.0}}]}),
        ]
    }

    #[test]
    fn usd_statements_convert_to_the_quote_currency() {
        let ts = usd_statements();
        assert_eq!(
            statement_currencies(&ts),
            BTreeSet::from(["USD".to_string()])
        );
        let fx = StatementFx::new(Some("INR"), [("USD".to_string(), 90.0)]);
        let (q, a) = parse_timeseries(&ts, &fx);
        assert_eq!(q[0].revenue, Some(5_082_000_000.0 * 90.0));
        assert!((a[0].eps.unwrap() - 72.0).abs() < 1e-9);
        // Share counts are not money.
        assert_eq!(a[0].shares_outstanding, Some(4_050_000_000.0));
    }

    #[test]
    fn statements_in_the_quote_currency_are_not_touched() {
        let ts = usd_statements();
        let fx = StatementFx::new(Some("USD"), []);
        let (q, _) = parse_timeseries(&ts, &fx);
        assert_eq!(q[0].revenue, Some(5_082_000_000.0));
        // Pence quotes: statements in pounds are not converted.
        let gbp = StatementFx::new(Some("GBp"), []);
        assert_eq!(gbp.factor("GBP"), Some(1.0));
    }

    #[test]
    fn statements_without_a_rate_are_left_out_not_mislabelled() {
        let (q, a) = parse_timeseries(&usd_statements(), &StatementFx::new(Some("INR"), []));
        assert!(q.is_empty(), "no period is made from skipped values");
        assert_eq!(a[0].eps, None);
        assert_eq!(a[0].shares_outstanding, Some(4_050_000_000.0));
    }

    #[test]
    fn financial_data_money_converts_or_goes() {
        let summary = json!({"financialData": {"financialCurrency": "usd",
            "totalCash": {"raw": 3.0e9}, "totalDebt": {"raw": 1.0e9},
            "ebitda": {"raw": 4.0e9}, "freeCashflow": {"raw": 2.0e9},
            "operatingCashflow": {"raw": 3.5e9}, "debtToEquity": {"raw": 8.0}}});
        assert_eq!(financial_currency(&summary).as_deref(), Some("USD"));
        let mut fin = parse_fin(&summary, &Quote::default());
        convert_fin(&mut fin, Some(90.0));
        assert_eq!(fin.total_cash, Some(2.7e11));
        assert_eq!(fin.ocf, Some(3.15e11));
        assert_eq!(fin.de, Some(8.0), "ratios stay");
        convert_fin(&mut fin, None);
        assert_eq!(fin.total_debt, None);
        assert_eq!(fin.de, Some(8.0));
    }

    #[test]
    fn test_parse_shp_xbrl() {
        let xml = r#"<in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares contextRef="InstitutionsForeign_ContextI" decimals="INF" unitRef="pure">0.1921</in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares>
<in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares contextRef="InstitutionsDomestic_ContextI" decimals="INF" unitRef="pure">0.4286</in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares>
<in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares contextRef="ResidentIndividualShareholdersHoldingNominalShareCapitalUpToRsTwoLakh_ContextI" decimals="INF" unitRef="pure">0.1896</in-bse-shp:ShareholdingAsAPercentageOfTotalNumberOfShares>
<in-bse-shp:WhetherAnySharesHeldByPromotersAreEncumberedUnderPledged contextRef="MainI">false</in-bse-shp:WhetherAnySharesHeldByPromotersAreEncumberedUnderPledged>"#;
        let d = NaiveDate::from_ymd_opt(2026, 9, 30).unwrap();
        let h = holding_from_xbrl(xml, d, Some(0.0)).unwrap();
        assert!((h.fii.unwrap() - 19.21).abs() < 1e-6);
        assert!((h.dii.unwrap() - 42.86).abs() < 1e-6);
        assert!((h.retail.unwrap() - 18.96).abs() < 1e-6);
        assert_eq!(h.promoter, Some(0.0));
        assert_eq!(h.pledged, Some(false));
    }

    #[test]
    fn test_parse_nse_dates() {
        assert_eq!(
            parse_nse_date("30-SEP-2026"),
            NaiveDate::from_ymd_opt(2026, 9, 30)
        );
        assert_eq!(
            parse_nse_date("22-May-2026"),
            NaiveDate::from_ymd_opt(2026, 5, 22)
        );
        // 27-Oct-2026 22:13:16 IST = 16:43:16 UTC
        let ts = parse_nse_datetime_ist("27-Oct-2026 22:13:16").unwrap();
        let dt = chrono::DateTime::from_timestamp(ts, 0).unwrap();
        assert_eq!(dt.format("%Y-%m-%d %H:%M").to_string(), "2026-10-27 16:43");
    }

    #[test]
    fn test_parse_nse_pit_filters_noise() {
        let v = json!({"data": [
            {"personCategory": "Promoter Group", "acqMode": "Market Purchase", "tdpTransactionType": "Buy",
             "date": "18-Feb-2026 19:06", "acqName": "A", "secAcq": "1000", "secVal": "250000"},
            {"personCategory": "Promoters", "acqMode": "Off Market", "tdpTransactionType": "Sell",
             "date": "18-Feb-2026 19:06", "acqName": "B", "secAcq": "1000", "secVal": "250000"},
            {"personCategory": "Other", "acqMode": "Market Sale", "tdpTransactionType": "Sell",
             "date": "18-Feb-2026 19:06", "acqName": "C", "secAcq": "1000", "secVal": "250000"}
        ]});
        let t = parse_nse_pit(&v);
        assert_eq!(t.len(), 1);
        assert!(t[0].is_buy);
        assert_eq!(t[0].value, 250000.0);
    }

    #[test]
    fn test_parse_rss_google_strips_publisher() {
        let xml = r#"<rss><channel><item><title>L&amp;T wins mega order - Economic Times</title>
<link>https://x</link><pubDate>Mon, 05 Oct 2026 07:00:00 GMT</pubDate>
<description>&lt;a href="x"&gt;L&amp;T wins mega order&lt;/a&gt;</description>
<source url="https://e.com">Economic Times</source></item></channel></rss>"#;
        let items = parse_rss(xml, "Google News");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "L&T wins mega order");
        assert_eq!(items[0].publisher, "Economic Times");
        assert!(items[0].summary.is_none()); // description only repeats the title
    }

    #[test]
    fn test_mentions_company_word_boundary() {
        let aliases = company_aliases("Larsen & Toubro", "LT");
        let h = |t: &str| Headline {
            ts: 0,
            title: t.into(),
            summary: None,
            publisher: String::new(),
            link: None,
            official: false,
            category: None,
        };
        assert!(mentions_company(&h("L&T bags order"), &aliases));
        assert!(mentions_company(&h("Larsen shares rise"), &aliases));
        assert!(!mentions_company(
            &h("Global results beat estimates"),
            &aliases
        ));
    }

    #[test]
    fn test_noise_headlines() {
        assert!(is_noise_headline("L&T Share Price Today Live Updates"));
        assert!(!is_noise_headline("L&T Q2 profit jumps 33%"));
    }

    #[test]
    fn test_series_returns() {
        let s = Series {
            dates: (1..=5)
                .map(|d| NaiveDate::from_ymd_opt(2026, 1, d).unwrap())
                .collect(),
            close: vec![100.0, 101.0, 102.0, 103.0, 110.0],
            ..Default::default()
        };
        assert!((s.ret(4).unwrap() - 0.10).abs() < 1e-9);
        assert_eq!(s.ret(5), None);
        assert_eq!(
            s.index_on_or_after(NaiveDate::from_ymd_opt(2026, 1, 3).unwrap()),
            Some(2)
        );
    }

    #[test]
    fn test_needs_struct_and_bitor() {
        assert_eq!(
            Needs::STATEMENTS | Needs::EVENTS,
            Needs::ALL,
            "STATEMENTS | EVENTS should cover all needs"
        );
        let st = Needs::STATEMENTS;
        assert!(!st.prices);
        assert!(!st.news);
        assert!(!st.filings);
        assert!(!st.ownership);
        assert!(!st.corporate_actions);
        assert!(st.statements);

        let custom = Needs::NONE.with_prices(true).with_filings(true);
        assert!(custom.prices);
        assert!(custom.filings);
        assert!(!custom.statements);
        assert!(!custom.news);
        assert!(!custom.ownership);
        assert!(!custom.corporate_actions);

        let mut a = Needs::STATEMENTS;
        a |= Needs::NONE.with_news(true);
        assert!(a.statements);
        assert!(a.news);
        assert!(!a.prices);
    }

    #[tokio::test]
    async fn test_score_needs_makes_no_news_or_nse_calls() {
        let server = crate::testutil::MockServer::start(vec![
            // 1. Quote response for TCS.NS
            (
                200,
                serde_json::json!({
                    "quoteResponse": {
                        "result": [{
                            "symbol": "TCS.NS",
                            "longName": "Tata Consultancy Services Limited",
                            "currency": "INR",
                            "regularMarketPrice": 3800.0,
                            "marketCap": 13800000000000.0
                        }]
                    }
                })
                .to_string(),
            ),
            // 2. quoteSummary
            (
                200,
                serde_json::json!({
                    "quoteSummary": {
                        "result": [{
                            "financialData": {
                                "currentPrice": { "raw": 3800.0 },
                                "profitMargins": { "raw": 0.18 }
                            },
                            "defaultKeyStatistics": {
                                "sharesOutstanding": { "raw": 3610000000.0 }
                            }
                        }]
                    }
                })
                .to_string(),
            ),
            // 3. fundamentals-timeseries
            (
                200,
                serde_json::json!({
                    "timeseries": {
                        "result": []
                    }
                })
                .to_string(),
            ),
        ])
        .await;

        let server_url = server.url.clone();
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "test_crumb",
            move |url: &str| {
                if let Ok(parsed) = reqwest::Url::parse(url) {
                    if let Some(q) = parsed.query() {
                        format!("{}{}?{}", server_url, parsed.path(), q)
                    } else {
                        format!("{}{}", server_url, parsed.path())
                    }
                } else {
                    url.to_string()
                }
            },
        );

        let res = fetch(&client, "TCS", Market::In, 60, Needs::STATEMENTS)
            .await
            .expect("fetch with Needs::STATEMENTS must succeed");

        assert_eq!(res.symbol, "TCS.NS");
        assert_eq!(res.short_name, "Tata Consultancy Services");

        let requests = server.requests();
        // Exactly 3 calls: quote, quoteSummary, fundamentals timeseries.
        assert_eq!(
            requests.len(),
            3,
            "Expected exactly 3 calls for Needs::STATEMENTS (quote, quoteSummary, timeseries)"
        );

        // Verify none of the calls were for chart, RSS, news, or NSE
        for req in &requests {
            assert!(
                !req.target.contains("rss"),
                "Should not make RSS requests: {}",
                req.target
            );
            assert!(
                !req.target.contains("chart"),
                "Should not make chart requests: {}",
                req.target
            );
            assert!(
                !req.target.contains("corporate"),
                "Should not make NSE corporate requests: {}",
                req.target
            );
            assert!(
                !req.target.contains("news.google"),
                "Should not make Google news requests: {}",
                req.target
            );
        }
    }

    #[tokio::test]
    async fn test_verdict_needs_makes_no_news_or_filings_calls() {
        let server = crate::testutil::MockServer::start(vec![
            // 1. Quote
            (
                200,
                serde_json::json!({
                    "quoteResponse": {
                        "result": [{
                            "symbol": "TCS.NS",
                            "longName": "Tata Consultancy Services Limited",
                            "currency": "INR",
                            "regularMarketPrice": 3800.0
                        }]
                    }
                })
                .to_string(),
            ),
            // 2. quoteSummary
            (
                200,
                serde_json::json!({
                    "quoteSummary": {
                        "result": [{
                            "financialData": { "currentPrice": { "raw": 3800.0 } }
                        }]
                    }
                })
                .to_string(),
            ),
            // 3. fundamentals-timeseries
            (
                200,
                serde_json::json!({ "timeseries": { "result": [] } }).to_string(),
            ),
            // 4. chart for TCS.NS
            (
                200,
                serde_json::json!({
                    "chart": {
                        "result": [{
                            "meta": { "currency": "INR", "symbol": "TCS.NS" },
                            "timestamp": [1700000000],
                            "indicators": { "quote": [{ "close": [3800.0] }] }
                        }]
                    }
                })
                .to_string(),
            ),
            // 5. NSE holdings
            (200, "[]".to_string()),
            // 6. NSE pit
            (200, "[]".to_string()),
        ])
        .await;

        let server_url = server.url.clone();
        let client = YahooClient::for_tests_with_rewrite(
            reqwest::Client::new(),
            "test_crumb",
            move |url: &str| {
                if let Ok(parsed) = reqwest::Url::parse(url) {
                    if let Some(q) = parsed.query() {
                        format!("{}{}?{}", server_url, parsed.path(), q)
                    } else {
                        format!("{}{}", server_url, parsed.path())
                    }
                } else {
                    url.to_string()
                }
            },
        );

        let res = fetch(&client, "TCS", Market::In, 60, Needs::VERDICT)
            .await
            .expect("fetch with Needs::VERDICT must succeed");

        assert_eq!(res.symbol, "TCS.NS");
        let requests = server.requests();

        // Verify no bench, news, filings, or corp actions were requested
        for req in &requests {
            assert!(
                !req.target.contains("%5ENSEI") && !req.target.contains("^NSEI"),
                "Should not request benchmark chart: {}",
                req.target
            );
            assert!(
                !req.target.contains("rss"),
                "Should not request RSS: {}",
                req.target
            );
            assert!(
                !req.target.contains("corporate-announcements"),
                "Should not request NSE filings: {}",
                req.target
            );
            assert!(
                !req.target.contains("corporates-corporateActions"),
                "Should not request NSE corp actions: {}",
                req.target
            );
        }
    }

    #[tokio::test]
    async fn fetch_refuses_a_symbol_without_a_quote() {
        let none = r#"{"quoteResponse":{"result":[{"symbol":"500325.BO","quoteType":"NONE"}],"error":null}}"#;
        let server = crate::testutil::MockServer::start(vec![(200, none.into())]).await;
        let base = server.url.clone();
        let client =
            YahooClient::for_tests_with_rewrite(reqwest::Client::new(), "c", move |url: &str| {
                let u = reqwest::Url::parse(url).unwrap();
                format!("{}{}?{}", base, u.path(), u.query().unwrap_or(""))
            });
        let err = fetch(&client, "500325", Market::In, 60, Needs::VERDICT)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'500325.BO' not found"), "{msg}");
        assert!(msg.contains("BSE scrip codes"), "{msg}");
        assert_eq!(server.requests().len(), 1, "stops after the quote");
    }
}
