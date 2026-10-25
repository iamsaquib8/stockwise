//! Planning and reporting: `tax export` (capital-gains schedules from the
//! realized lots), `portfolio benchmark` (time-weighted return against an
//! index, with attribution by holding and sector) and `goal` (SIP,
//! retirement and education calculators). The math lives in `tax_export.rs`,
//! `benchmark.rs` and `goals.rs`.

use super::portfolio::{fx_pair, market_currency, portfolio_title};
use super::*;
use crate::api::{ChartData, ChartResponse};
use crate::benchmark::{self, Analysis, HoldingDays};
use crate::goals::{self, SipPlan};
use crate::portfolio::{self as pf, Split};
use crate::tax::Country;
use crate::tax_export::{self, IndiaSchedule, UsSchedule};
use chrono::NaiveDate;
use std::path::Path;

/// Round every number in a JSON value to `places` decimals (plans are built
/// from raw floats).
fn round_floats(v: &mut serde_json::Value, places: i32) {
    match v {
        serde_json::Value::Number(n) => {
            if let Some(x) = n.as_f64().filter(|_| !n.is_i64() && !n.is_u64()) {
                *v = serde_json::json!(round(x, places));
            }
        }
        serde_json::Value::Array(a) => a.iter_mut().for_each(|x| round_floats(x, places)),
        serde_json::Value::Object(o) => o.values_mut().for_each(|x| round_floats(x, places)),
        _ => {}
    }
}

// ── Tax export ──

const NSE_EQUITY_LIST: &str = "https://nsearchives.nseindia.com/content/equities/EQUITY_L.csv";

/// ISINs of `symbols`: NSE's equity list for `.NS`/`.BO` shares, AMFI's NAV
/// file for `MF:<code>` funds (both cached for 12 hours). Best effort — a
/// failed download leaves those ISINs blank with a warning on stderr.
async fn lookup_isins(symbols: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let shares: Vec<&String> = symbols
        .iter()
        .filter(|s| s.ends_with(".NS") || s.ends_with(".BO"))
        .collect();
    if !shares.is_empty() {
        let list = async {
            let client = crate::funds::http_client()?;
            crate::funds::cached_text(
                &client,
                "nse_equity_list.csv",
                NSE_EQUITY_LIST,
                "NSE",
                |t| t.trim_start_matches('\u{feff}').starts_with("SYMBOL"),
            )
            .await
        }
        .await;
        match list {
            Ok(text) => {
                let all = tax_export::parse_nse_equity_list(&text);
                for sym in shares {
                    let base = sym
                        .strip_suffix(".NS")
                        .or_else(|| sym.strip_suffix(".BO"))
                        .unwrap_or(sym);
                    if let Some((_, isin)) = all.iter().find(|(s, _)| s == base) {
                        out.push((sym.clone(), isin.clone()));
                    }
                }
            }
            Err(e) => eprintln!("  Warning: share ISINs left blank ({e:#})"),
        }
    }
    // `MF:<ISIN>` (a CAS or Groww import) already is the ISIN.
    for sym in symbols {
        if let Some(id) = crate::offmarket::fund_id(sym)
            && id.parse::<u32>().is_err()
        {
            out.push((sym.clone(), id.to_string()));
        }
    }
    let funds: Vec<(&String, u32)> = symbols
        .iter()
        .filter_map(|s| Some((s, crate::offmarket::fund_id(s)?.parse().ok()?)))
        .collect();
    if !funds.is_empty() {
        match crate::funds::load_amfi().await {
            Ok(schemes) => {
                for (sym, code) in funds {
                    if let Some(isin) = schemes
                        .iter()
                        .find(|x| x.code == code)
                        .and_then(|x| x.isin_growth.clone().or_else(|| x.isin_reinvest.clone()))
                    {
                        out.push((sym.clone(), isin));
                    }
                }
            }
            Err(e) => eprintln!("  Warning: fund ISINs left blank ({e:#})"),
        }
    }
    out
}

enum Schedule {
    India(IndiaSchedule),
    Us(UsSchedule),
}

impl Schedule {
    fn csv(&self) -> String {
        match self {
            Schedule::India(s) => tax_export::india_csv(s),
            Schedule::Us(s) => tax_export::us_csv(s),
        }
    }

    fn json(&self) -> Result<String> {
        Ok(match self {
            Schedule::India(s) => serde_json::to_string_pretty(s)?,
            Schedule::Us(s) => serde_json::to_string_pretty(s)?,
        })
    }

    fn sales(&self) -> usize {
        match self {
            Schedule::India(s) => s.sales.len(),
            Schedule::Us(s) => s.sales.len(),
        }
    }

    fn tax_year(&self) -> &str {
        match self {
            Schedule::India(s) => &s.tax_year,
            Schedule::Us(s) => &s.tax_year,
        }
    }

    /// Symbols without an ISIN (Indian schedule only).
    fn missing_isins(&self) -> Vec<String> {
        let Schedule::India(s) = self else {
            return Vec::new();
        };
        let mut v: Vec<String> = s
            .sales
            .iter()
            .filter(|r| r.isin.is_empty())
            .map(|r| r.scrip.clone())
            .collect();
        v.sort();
        v.dedup();
        v
    }

    fn print_summary(&self) {
        let inr = |x: f64| format!("₹{}", goals::indian_grouping(x));
        let usd = |x: f64| {
            let s = format!("${}", goals::western_grouping(x.abs()));
            if x < 0.0 { format!("-{s}") } else { s }
        };
        match self {
            Schedule::India(s) => {
                let m = &s.summary;
                print_kv(
                    "STCG 111A gains / losses",
                    &format!("{} / {}", inr(m.stcg_gains), inr(m.stcg_losses)),
                );
                print_kv(
                    "LTCG 112A gains / losses",
                    &format!("{} / {}", inr(m.ltcg_gains), inr(m.ltcg_losses)),
                );
                if m.st_loss_set_off_against_ltcg > 0.0 {
                    print_kv(
                        "ST loss set off vs LTCG",
                        &inr(m.st_loss_set_off_against_ltcg),
                    );
                }
                print_kv("Taxable STCG (111A)", &inr(m.taxable_stcg));
                print_kv("LTCG exemption used", &inr(m.ltcg_exemption));
                print_kv("Taxable LTCG (112A)", &inr(m.taxable_ltcg));
                print_kv("Estimated tax", &inr(m.total_tax).bold().to_string());
                if m.st_loss_carried_forward + m.lt_loss_carried_forward > 0.0 {
                    print_kv(
                        "Loss to carry forward",
                        &format!(
                            "short-term {}, long-term {}",
                            inr(m.st_loss_carried_forward),
                            inr(m.lt_loss_carried_forward)
                        ),
                    );
                }
            }
            Schedule::Us(s) => {
                let m = &s.summary;
                print_kv("Short-term gain or (loss)", &usd(m.short_term_gain));
                print_kv("Long-term gain or (loss)", &usd(m.long_term_gain));
                print_kv("Net gain or (loss)", &usd(m.net_gain));
                print_kv("Estimated tax", &usd(m.total_tax).bold().to_string());
                if m.loss_deductible_this_year > 0.0 {
                    print_kv(
                        "Loss deductible this year",
                        &usd(m.loss_deductible_this_year),
                    );
                    print_kv("Loss carryover", &usd(m.loss_carryover));
                }
            }
        }
    }
}

/// `tax export`: the capital-gains schedule of a tax year's realized sales,
/// as CSV (ITR Schedule CG / 112A for India, Form 8949 for the US) or JSON —
/// on stdout, or into `out` with a summary on screen.
pub async fn cmd_tax_export(
    country: &str,
    fy: Option<&str>,
    out: Option<&Path>,
    json: bool,
) -> Result<()> {
    let country = match Country::parse(country) {
        Some(c @ (Country::India | Country::Us)) => c,
        _ => bail!("`tax export` covers in (ITR Schedule CG) and us (Form 8949)"),
    };
    let today = chrono::Local::now().date_naive();
    let start = match fy {
        Some(label) => tax_export::parse_tax_year(country, label).with_context(|| {
            format!(
                "Invalid tax year '{}': use {}",
                label,
                if country == Country::Us {
                    "a year such as 2025"
                } else {
                    "a financial year such as 2025-26"
                }
            )
        })?,
        None => country.tax_year(today).0,
    };
    if start > today {
        bail!("{} has not started yet", country.tax_year_label(start));
    }
    let portfolio = Portfolio::load()?;
    let schedule = match country {
        Country::India => {
            let (from, to) = country.tax_year(start);
            let mut sold: Vec<String> = portfolio
                .realized
                .iter()
                .filter(|s| {
                    NaiveDate::parse_from_str(&s.sell_date, "%Y-%m-%d")
                        .is_ok_and(|d| d >= from && d <= to)
                })
                .map(|s| s.symbol.clone())
                .collect();
            sold.sort();
            sold.dedup();
            let isins = lookup_isins(&sold).await;
            Schedule::India(tax_export::india_schedule(
                &portfolio.realized,
                start,
                |sym| isins.iter().find(|(s, _)| s == sym).map(|(_, i)| i.clone()),
            ))
        }
        _ => Schedule::Us(tax_export::us_schedule(&portfolio.realized, start)),
    };

    let body = if json {
        let mut s = schedule.json()?;
        s.push('\n');
        s
    } else {
        schedule.csv()
    };
    let missing = schedule.missing_isins();
    let Some(path) = out else {
        if !missing.is_empty() {
            eprintln!(
                "  {}",
                tf(Msg::TaxExportIsinMissing, &[&missing.join(", ")])
            );
        }
        print!("{}", body);
        return Ok(());
    };
    std::fs::write(path, &body).with_context(|| format!("Cannot write {}", path.display()))?;

    print_header(&portfolio_title(&tf(
        Msg::TaxExportTitle,
        &[&schedule.tax_year()],
    )));
    if schedule.sales() == 0 {
        println!(
            "  {}",
            tf(Msg::TaxExportNoSales, &[&schedule.tax_year()]).dimmed()
        );
    } else {
        print_kv(t(Msg::TaxExportSales), &schedule.sales().to_string());
        schedule.print_summary();
    }
    if !missing.is_empty() {
        println!(
            "\n  {} {}",
            "!".yellow(),
            tf(Msg::TaxExportIsinMissing, &[&missing.join(", ")])
        );
    }
    println!(
        "\n  {}",
        tf(Msg::TaxExportWrote, &[&schedule.sales(), &path.display()]).green()
    );
    println!("  {}\n", t(Msg::TaxExportEstimate).yellow().italic());
    Ok(())
}

// ── Benchmark ──

/// Benchmarks by name: (names, Yahoo symbol, display name).
const BENCHMARKS: &[(&[&str], &str, &str)] = &[
    (&["NIFTY", "NIFTY50"], "^NSEI", "NIFTY 50"),
    (&["SENSEX"], "^BSESN", "SENSEX"),
    (&["BANKNIFTY", "NIFTYBANK"], "^NSEBANK", "NIFTY Bank"),
    (&["NIFTY500"], "^CRSLDX", "NIFTY 500"),
    (&["SP500", "S&P500", "SPX"], "^GSPC", "S&P 500"),
    (&["NASDAQ"], "^IXIC", "NASDAQ Composite"),
    (&["NASDAQ100", "NDX"], "^NDX", "NASDAQ 100"),
    (&["DOW", "DJIA"], "^DJI", "Dow Jones"),
];

/// The benchmark's Yahoo symbol and display name: a name from
/// [`BENCHMARKS`], an index symbol (`^NSEI`), or any ticker resolved for the
/// market (`NIFTYBEES` → `NIFTYBEES.NS`). Default: NIFTY 50 for India, the
/// S&P 500 otherwise.
pub(crate) fn resolve_benchmark(name: Option<&str>, market: Market) -> (String, String) {
    let name = name
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(match market {
            Market::In => "NIFTY",
            // A global exchange's main index (`^FTSE`, `^N225`).
            Market::Global(x) => x.info().benchmark().0,
            _ => "SP500",
        });
    let upper = name.to_uppercase();
    let key: String = upper
        .chars()
        .filter(|c| !c.is_whitespace() && !matches!(c, '_' | '-'))
        .collect();
    for (names, symbol, label) in BENCHMARKS {
        if names.contains(&key.as_str()) || *symbol == upper {
            return (symbol.to_string(), label.to_string());
        }
    }
    let symbol = market::resolve_symbol(name, market);
    (symbol.clone(), symbol)
}

/// Daily closes, splits and currency of one symbol.
struct History {
    closes: Vec<(NaiveDate, f64)>,
    splits: Vec<Split>,
    currency: Option<String>,
}

/// Turn a Yahoo chart response (daily bars, `events=splits`) into dated
/// closes (one per day, the last bar of a day winning) and splits. Bars are
/// dated in the exchange's time zone (`gmtoffset`, seconds east of UTC):
/// FX bars start at midnight London time, which is the previous day in UTC.
/// Three hours of slack absorb a daylight-saving change since `gmtoffset`
/// was reported without moving any session start across midnight.
fn history_from_chart(data: ChartData, gmtoffset: i64) -> History {
    let to_date = |ts: i64| {
        chrono::DateTime::from_timestamp(ts + gmtoffset + 3 * 3600, 0).map(|d| d.date_naive())
    };
    let stamps = data.timestamp.unwrap_or_default();
    let closes_raw = data
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.clone())
        .unwrap_or_default();
    let mut closes: Vec<(NaiveDate, f64)> = Vec::new();
    for (ts, close) in stamps.iter().zip(closes_raw) {
        let (Some(day), Some(c)) = (to_date(*ts), close) else {
            continue;
        };
        if !c.is_finite() || c <= 0.0 {
            continue;
        }
        match closes.last_mut() {
            Some(last) if last.0 == day => last.1 = c,
            _ => closes.push((day, c)),
        }
    }
    let mut splits: Vec<Split> = data
        .events
        .map(|e| {
            e.splits
                .values()
                .filter(|s| s.numerator > 0.0 && s.denominator > 0.0)
                .filter_map(|s| {
                    Some(Split {
                        date: to_date(s.date)?.to_string(),
                        numerator: s.numerator,
                        denominator: s.denominator,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    splits.sort_by(|a, b| a.date.cmp(&b.date));
    History {
        closes,
        splits,
        currency: data.meta.and_then(|m| m.currency),
    }
}

/// How much daily history to fetch.
#[derive(Clone, Copy)]
enum Span<'a> {
    /// A Yahoo range such as `1y` (the benchmark sets the calendar).
    Range(&'a str),
    /// From this day to today (holdings and exchange rates, so the first
    /// calendar day has a price even when markets' holidays differ).
    Since(NaiveDate),
}

/// Daily history of `symbol` from Yahoo, with splits: share counts are in
/// today's units and Yahoo's closes are split-adjusted.
async fn daily_history(client: &YahooClient, symbol: &str, span: Span<'_>) -> Result<History> {
    let (from, to);
    let mut query = vec![("interval", "1d"), ("events", "splits")];
    match span {
        Span::Range(range) => query.push(("range", range)),
        Span::Since(day) => {
            // End at the next midnight so the URL (and its cache entry) is stable for the day.
            let midnight = |d: NaiveDate| {
                d.and_hms_opt(0, 0, 0)
                    .expect("midnight")
                    .and_utc()
                    .timestamp()
            };
            from = midnight(day).to_string();
            to = midnight(chrono::Utc::now().date_naive() + chrono::Days::new(1)).to_string();
            query.push(("period1", &from));
            query.push(("period2", &to));
        }
    }
    let url = crate::api::build_url(
        "https://query1.finance.yahoo.com/v8/finance/chart",
        Some(symbol),
        &query,
    );
    let v = client.raw_get(&url).await?;
    let gmtoffset = v
        .pointer("/chart/result/0/meta/gmtoffset")
        .and_then(|o| o.as_i64())
        .unwrap_or(0);
    let resp: ChartResponse =
        serde_json::from_value(v).context("Failed to parse chart response")?;
    let data = resp
        .chart
        .result
        .and_then(|r| r.into_iter().next())
        .with_context(|| format!("No price history for {}", symbol))?;
    Ok(history_from_chart(data, gmtoffset))
}

/// A fund's NAV history from api.mfapi.in, for `MF:<code>` holdings.
/// The AMFI scheme code for a fund id: the code itself, or the scheme an
/// ISIN belongs to (looked up in AMFI's NAV file).
async fn fund_code(id: &str) -> Option<u32> {
    if let Ok(code) = id.parse() {
        return Some(code);
    }
    let schemes = crate::funds::load_amfi_shared().await.ok()?;
    crate::offmarket::find_scheme(&schemes, id).map(|s| s.code)
}

async fn fund_history(code: u32) -> Result<History> {
    let client = crate::funds::http_client()?;
    let h = crate::funds::load_history(&client, code).await?;
    Ok(History {
        closes: h.navs,
        splits: Vec::new(),
        currency: Some("INR".into()),
    })
}

/// Daily rate from `currency` into `base` on `calendar`, or None when it
/// can't be fetched. `GBp` (pence) becomes pounds.
async fn fx_series(
    client: &YahooClient,
    currency: &str,
    base: &str,
    calendar: &[NaiveDate],
) -> Option<Vec<Option<f64>>> {
    let (cur, scale) = if currency == "GBp" {
        ("GBP", 0.01)
    } else {
        (currency, 1.0)
    };
    if cur == base {
        return Some(vec![Some(scale); calendar.len()]);
    }
    let (symbol, invert) = fx_pair(cur, base);
    let h = daily_history(client, &symbol, Span::Since(lead_in(calendar)))
        .await
        .ok()?;
    if h.closes.is_empty() {
        return None;
    }
    Some(
        benchmark::align(calendar, &h.closes)
            .into_iter()
            .map(|r| r.map(|r| if invert { 1.0 / r } else { r } * scale))
            .collect(),
    )
}

/// A week before the calendar's first day: history fetched from here has a
/// price on that day.
fn lead_in(calendar: &[NaiveDate]) -> NaiveDate {
    calendar[0] - chrono::Days::new(7)
}

/// A holding's sector from Yahoo's asset profile.
fn parse_sector(v: &serde_json::Value) -> Option<String> {
    v.pointer("/quoteSummary/result/0/assetProfile/sector")
        .and_then(|s| s.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

async fn sector_of(client: &YahooClient, symbol: &str) -> String {
    if symbol.starts_with("MF:") {
        return "Mutual fund".into();
    }
    match market::AssetClass::of(symbol) {
        market::AssetClass::Crypto => return "Crypto".into(),
        market::AssetClass::Fx => return "Currency".into(),
        _ => {}
    }
    let url = crate::api::build_url(
        "https://query2.finance.yahoo.com/v10/finance/quoteSummary",
        Some(symbol),
        &[("modules", "assetProfile"), ("crumb", client.crumb())],
    );
    match client.raw_get(&url).await {
        Ok(v) => parse_sector(&v).unwrap_or_else(|| "Unknown".into()),
        Err(_) => "Unknown".into(),
    }
}

/// Whether `symbol` was held at any close from `start` to `end` (YYYY-MM-DD).
fn held_between(portfolio: &Portfolio, symbol: &str, start: &str, end: &str) -> bool {
    portfolio
        .holding(symbol)
        .is_some_and(|h| h.lots.iter().any(|l| l.date.as_str() <= end))
        || portfolio.realized.iter().any(|s| {
            s.symbol == symbol && s.buy_date.as_str() <= end && s.sell_date.as_str() > start
        })
}

/// The first purchase date of `symbol` among its open lots and sales.
fn earliest_purchase<'a>(portfolio: &'a Portfolio, symbol: &str) -> Option<&'a str> {
    let open = portfolio
        .holding(symbol)
        .into_iter()
        .flat_map(|h| h.lots.iter().map(|l| l.date.as_str()));
    let sold = portfolio
        .realized
        .iter()
        .filter(|s| s.symbol == symbol)
        .map(|s| s.buy_date.as_str());
    open.chain(sold).min()
}

/// Whether a purchase of `symbol` inside the period has an estimated date
/// (the shares may have been held before it).
fn estimated_in_period(portfolio: &Portfolio, symbol: &str, start: &str, end: &str) -> bool {
    let inside = |d: &str| d > start && d <= end;
    portfolio
        .holding(symbol)
        .is_some_and(|h| h.lots.iter().any(|l| l.date_estimated && inside(&l.date)))
        || portfolio
            .realized
            .iter()
            .any(|s| s.symbol == symbol && s.buy_date_estimated && inside(&s.buy_date))
}

/// One holding in `portfolio benchmark --json/--csv`. Percentages; the
/// contributions are percentage points.
#[derive(Debug, Serialize)]
pub(crate) struct BenchmarkRow {
    pub(crate) symbol: String,
    pub(crate) sector: String,
    pub(crate) avg_weight: f64,
    pub(crate) end_weight: f64,
    pub(crate) return_pct: f64,
    pub(crate) contribution: f64,
    pub(crate) active_contribution: f64,
    pub(crate) days_held: usize,
    pub(crate) days_unpriced: usize,
    /// Why the holding's history is partial, if it is.
    pub(crate) flag: Option<String>,
}

impl CsvRow for BenchmarkRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "sector",
        "avg_weight",
        "end_weight",
        "return_pct",
        "contribution",
        "active_contribution",
        "days_held",
        "days_unpriced",
        "flag",
    ];
}

#[derive(Debug, Serialize)]
pub(crate) struct Flagged {
    pub(crate) symbol: String,
    pub(crate) reason: String,
}

/// The benchmark comparison as `--json` prints it (rows are `BenchmarkRow`).
pub(crate) fn benchmark_json(
    a: Option<&Analysis>,
    bench: (&str, &str, Option<&str>),
    base: &str,
    period: &str,
    rows: &[BenchmarkRow],
    flagged: &[Flagged],
) -> serde_json::Value {
    let (symbol, name, currency) = bench;
    let r = |x: f64| round(x, 4);
    serde_json::json!({
        "benchmark": {
            "symbol": symbol,
            "name": name,
            "currency": currency,
            "converted_to_base": currency.is_some_and(|c| c != base),
        },
        "base_currency": base,
        "period": period,
        "start": a.map(|a| a.start().to_string()),
        "end": a.map(|a| a.end().to_string()),
        "trading_days": a.map(|a| a.portfolio_daily.len()),
        "portfolio_return": a.map(|a| r(a.portfolio_return)),
        "benchmark_return": a.map(|a| r(a.benchmark_return)),
        "excess_return": a.map(|a| r(a.excess_return())),
        "portfolio_annualized": a.and_then(|a| a.annualized()).map(|x| r(x.0)),
        "benchmark_annualized": a.and_then(|a| a.annualized()).map(|x| r(x.1)),
        "metrics": a.map(|a| serde_json::json!({
            "observations": a.metrics.observations,
            "alpha": round_opt(a.metrics.alpha.map(|x| x * 100.0), 4),
            "beta": round_opt(a.metrics.beta, 4),
            "tracking_error": round_opt(a.metrics.tracking_error.map(|x| x * 100.0), 4),
            "information_ratio": round_opt(a.metrics.information_ratio, 4),
            "correlation": round_opt(a.metrics.correlation, 4),
        })),
        "holdings": rows,
        "sectors": a.map(|a| a.sectors.iter().map(|s| serde_json::json!({
            "sector": s.sector,
            "holdings": s.holdings,
            "avg_weight": r(s.avg_weight),
            "end_weight": r(s.end_weight),
            "return_pct": r(s.return_pct),
            "contribution": r(s.contribution),
            "active_contribution": r(s.active_contribution),
        })).collect::<Vec<_>>()).unwrap_or_default(),
        "flagged": flagged,
        "series": a.map(|a| a.days.iter().zip(&a.portfolio_index).zip(&a.benchmark_index)
            .map(|((d, p), b)| serde_json::json!({
                "date": d.to_string(),
                "portfolio": r(*p),
                "benchmark": r(*b),
            })).collect::<Vec<_>>()).unwrap_or_default(),
    })
}

fn signed_pct(x: f64) -> String {
    let s = format!("{:+.2}%", x);
    if x >= 0.0 {
        s.green().to_string()
    } else {
        s.red().to_string()
    }
}

fn opt_num(x: Option<f64>, f: impl Fn(f64) -> String) -> String {
    x.map(f).unwrap_or_else(|| "—".dimmed().to_string())
}

/// `portfolio benchmark`: the portfolio's time-weighted return against an
/// index over `period`, built from the lots held at each close, with alpha,
/// beta, tracking error and attribution by holding and sector.
pub async fn cmd_portfolio_benchmark(
    bench: Option<&str>,
    period: &str,
    market: Market,
    base: Option<&str>,
    fmt: Format,
) -> Result<()> {
    let (bench_symbol, bench_name) = resolve_benchmark(bench, market);
    let portfolio = Portfolio::load()?;
    let symbols = portfolio.symbols_with_history();
    let base = match base {
        Some(b) => b.to_uppercase(),
        None => {
            let mut cur: Vec<&str> = symbols
                .iter()
                .filter_map(|s| pf::symbol_currency(s))
                .map(|c| if c == "GBp" { "GBP" } else { c })
                .collect();
            cur.sort();
            cur.dedup();
            match cur.as_slice() {
                [one] => one.to_string(),
                _ => market_currency(market).to_string(),
            }
        }
    };
    if symbols.is_empty() {
        return match fmt {
            Format::Table => {
                println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
                Ok(())
            }
            Format::Json => output::print_json(&benchmark_json(
                None,
                (&bench_symbol, &bench_name, None),
                &base,
                period,
                &[],
                &[],
            )),
            Format::Csv => output::print_csv::<BenchmarkRow>(&[]),
        };
    }

    let client = YahooClient::new().await?;
    let bench_hist = daily_history(&client, &bench_symbol, Span::Range(period))
        .await
        .with_context(|| format!("No price history for the benchmark {}", bench_symbol))?;
    if bench_hist.closes.len() < 2 {
        bail!(
            "Not enough price history for the benchmark {}",
            bench_symbol
        );
    }
    let calendar: Vec<NaiveDate> = bench_hist.closes.iter().map(|c| c.0).collect();
    let (first, last) = (
        calendar[0].to_string(),
        calendar[calendar.len() - 1].to_string(),
    );
    let bench_currency = bench_hist.currency.clone();
    let mut fx_cache: Vec<(String, Option<Vec<Option<f64>>>)> = Vec::new();
    let mut fx_for = async |cur: &str| -> Option<Vec<Option<f64>>> {
        if let Some((_, s)) = fx_cache.iter().find(|(c, _)| c == cur) {
            return s.clone();
        }
        let s = fx_series(&client, cur, &base, &calendar).await;
        fx_cache.push((cur.to_string(), s.clone()));
        s
    };
    let mut bench_prices = benchmark::align(&calendar, &bench_hist.closes);
    if let Some(cur) = bench_currency.as_deref().filter(|c| *c != base) {
        let Some(fx) = fx_for(cur).await else {
            let hint = if matches!(cur, "INR" | "USD") {
                format!("; try --base {}", cur.to_lowercase())
            } else {
                String::new()
            };
            bail!(
                "No {}→{} exchange-rate history to compare with {}{}",
                cur,
                base,
                bench_name,
                hint
            );
        };
        bench_prices = bench_prices
            .iter()
            .zip(&fx)
            .map(|(p, r)| Some((*p)? * (*r)?))
            .collect();
    }

    let mut holdings: Vec<HoldingDays> = Vec::new();
    let mut flagged: Vec<Flagged> = Vec::new();
    let flag = |flagged: &mut Vec<Flagged>, symbol: &str, reason: String| {
        flagged.push(Flagged {
            symbol: symbol.to_string(),
            reason,
        })
    };
    for sym in &symbols {
        if !held_between(&portfolio, sym, &first, &last) {
            continue;
        }
        let hist = match crate::offmarket::fund_id(sym) {
            Some(id) => match fund_code(id).await {
                Some(code) => fund_history(code).await,
                None => Err(anyhow::anyhow!("not in AMFI's NAV file")),
            },
            None => daily_history(&client, sym, Span::Since(lead_in(&calendar))).await,
        };
        let hist = match hist {
            Ok(h) if !h.closes.is_empty() => h,
            _ => {
                flag(&mut flagged, sym, "no price history — left out".into());
                continue;
            }
        };
        // Splits before the period still matter for lots bought before them
        // that `portfolio actions --apply` hasn't adjusted.
        let mut splits = hist.splits.clone();
        if let Some(since) = earliest_purchase(&portfolio, sym).filter(|d| *d < first.as_str())
            && !crate::offmarket::is_fund(sym)
            && let Ok((earlier, _, _)) = super::portfolio::fetch_actions(&client, sym, since).await
        {
            for split in earlier {
                if !splits.iter().any(|s| s.id() == split.id()) {
                    splits.push(split);
                }
            }
        }
        let mut price = benchmark::align(&calendar, &hist.closes);
        let currency = hist
            .currency
            .clone()
            .or_else(|| pf::symbol_currency(sym).map(str::to_string));
        match currency.as_deref() {
            Some(cur) if cur != base => match fx_for(cur).await {
                Some(fx) => {
                    price = price
                        .iter()
                        .zip(&fx)
                        .map(|(p, r)| Some((*p)? * (*r)?))
                        .collect()
                }
                None => {
                    flag(
                        &mut flagged,
                        sym,
                        format!("no {}→{} exchange-rate history — left out", cur, base),
                    );
                    continue;
                }
            },
            _ => {}
        }
        let qty: Vec<f64> = calendar
            .iter()
            .map(|d| {
                let next = (*d + chrono::Days::new(1)).to_string();
                portfolio.shares_on(sym, &next, &splits)
            })
            .collect();
        if estimated_in_period(&portfolio, sym, &first, &last) {
            flag(
                &mut flagged,
                sym,
                "estimated purchase date — the shares may have been held earlier".into(),
            );
        }
        holdings.push(HoldingDays {
            symbol: sym.clone(),
            sector: sector_of(&client, sym).await,
            qty,
            price,
        });
    }

    let analysis = benchmark::analyze(&calendar, &bench_prices, &holdings);
    if let Some(a) = &analysis {
        for h in a.holdings.iter().filter(|h| h.days_unpriced > 0) {
            flag(
                &mut flagged,
                &h.symbol,
                format!(
                    "no price for {} of the days held — those days left out",
                    h.days_unpriced
                ),
            );
        }
    }
    let rows: Vec<BenchmarkRow> = analysis
        .as_ref()
        .map(|a| {
            a.holdings
                .iter()
                .map(|h| BenchmarkRow {
                    symbol: h.symbol.clone(),
                    sector: h.sector.clone(),
                    avg_weight: round(h.avg_weight, 4),
                    end_weight: round(h.end_weight, 4),
                    return_pct: round(h.return_pct, 4),
                    contribution: round(h.contribution, 4),
                    active_contribution: round(h.active_contribution, 4),
                    days_held: h.days_held,
                    days_unpriced: h.days_unpriced,
                    flag: {
                        let reasons: Vec<&str> = flagged
                            .iter()
                            .filter(|f| f.symbol == h.symbol)
                            .map(|f| f.reason.as_str())
                            .collect();
                        (!reasons.is_empty()).then(|| reasons.join("; "))
                    },
                })
                .collect()
        })
        .unwrap_or_default();

    match fmt {
        Format::Json => {
            return output::print_json(&benchmark_json(
                analysis.as_ref(),
                (&bench_symbol, &bench_name, bench_currency.as_deref()),
                &base,
                period,
                &rows,
                &flagged,
            ));
        }
        Format::Csv => return output::print_csv(&rows),
        Format::Table => {}
    }

    print_header(&portfolio_title(&tf(Msg::BenchTitle, &[&bench_name])));
    let Some(a) = analysis else {
        println!("  {}", tf(Msg::BenchNothingHeld, &[&period]).dimmed());
        print_flagged(&flagged);
        println!();
        return Ok(());
    };
    print_kv(
        t(Msg::BenchPeriod),
        &format!(
            "{} → {} ({})",
            a.start().format("%d %b %Y"),
            a.end().format("%d %b %Y"),
            tf(Msg::BenchTradingDays, &[&a.portfolio_daily.len()])
        ),
    );
    print_kv(
        t(Msg::BenchPortfolioReturn),
        &signed_pct(a.portfolio_return),
    );
    print_kv(
        &tf(Msg::BenchIndexReturn, &[&bench_name]),
        &signed_pct(a.benchmark_return),
    );
    print_kv(
        t(Msg::BenchExcess),
        &signed_pct(a.excess_return()).bold().to_string(),
    );
    if let Some((p, b)) = a.annualized() {
        print_kv(
            t(Msg::BenchAnnualized),
            &format!("{} / {}", signed_pct(p), signed_pct(b)),
        );
    }
    let m = &a.metrics;
    if m.beta.is_some() {
        print_kv(
            t(Msg::BenchAlpha),
            &opt_num(m.alpha, |x| signed_pct(x * 100.0)),
        );
        print_kv(t(Msg::BenchBeta), &opt_num(m.beta, |x| format!("{:.2}", x)));
        print_kv(
            t(Msg::BenchTrackingError),
            &opt_num(m.tracking_error, |x| format!("{:.2}%", x * 100.0)),
        );
        print_kv(
            t(Msg::BenchInfoRatio),
            &opt_num(m.information_ratio, |x| format!("{:.2}", x)),
        );
        print_kv(
            t(Msg::BenchCorrelation),
            &opt_num(m.correlation, |x| format!("{:.2}", x)),
        );
    } else {
        println!(
            "  {}",
            tf(Msg::BenchTooFewDays, &[&benchmark::MIN_OBSERVATIONS]).dimmed()
        );
    }
    if let Some(cur) = bench_currency.as_deref().filter(|c| *c != base) {
        println!(
            "  {}",
            tf(
                Msg::BenchConverted,
                &[&format!("{} ({})", bench_name, cur), &base]
            )
            .dimmed()
        );
    }

    println!();
    for line in charts::dual_line_chart(
        &a.portfolio_index,
        &a.benchmark_index,
        60,
        12,
        t(Msg::BenchGrowth),
        t(Msg::BenchPortfolio),
        &bench_name,
    ) {
        println!("{}", line);
    }

    print_section(t(Msg::BenchByHolding));
    let header = |first: &str, second: &str| {
        println!(
            "  {} {} {} {} {} {}",
            pad_end(first, 14).bold(),
            pad_end(second, 22).bold(),
            pad_start(t(Msg::BenchAvgWeight), 8).bold(),
            pad_start(t(Msg::BenchReturn), 9).bold(),
            pad_start(t(Msg::BenchContribution), 9).bold(),
            pad_start(t(Msg::BenchVsIndex), 9).bold(),
        );
    };
    let pp = |x: f64| {
        let s = format!("{:+.2}", x);
        if x >= 0.0 { s.green() } else { s.red() }
    };
    header(t(Msg::Symbol), t(Msg::BenchSector));
    for h in &a.holdings {
        let mark = if flagged.iter().any(|f| f.symbol == h.symbol) {
            "*"
        } else {
            ""
        };
        println!(
            "  {} {} {:>7.1}% {:>8.1}% {:>9} {:>9}",
            pad_end(&format!("{}{}", h.symbol, mark), 14).cyan(),
            pad_end(&truncate_text(&h.sector, 22), 22),
            h.avg_weight,
            h.return_pct,
            pp(h.contribution),
            pp(h.active_contribution),
        );
    }

    print_section(t(Msg::BenchBySector));
    header(t(Msg::BenchSector), "");
    for s in &a.sectors {
        println!(
            "  {} {:>7.1}% {:>8.1}% {:>9} {:>9}",
            pad_end(&truncate_text(&s.sector, 37), 37).cyan(),
            s.avg_weight,
            s.return_pct,
            pp(s.contribution),
            pp(s.active_contribution),
        );
    }
    print_flagged(&flagged);
    println!("\n  {}\n", t(Msg::BenchMethod).dimmed().italic());
    Ok(())
}

fn print_flagged(flagged: &[Flagged]) {
    if flagged.is_empty() {
        return;
    }
    print_section(t(Msg::BenchFlagged));
    for f in flagged {
        println!("  {} {} — {}", "*".yellow(), f.symbol.cyan(), f.reason);
    }
}

/// `text` cut to `width` characters with an ellipsis.
fn truncate_text(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        text.to_string()
    } else {
        let cut: String = text.chars().take(width.saturating_sub(1)).collect();
        format!("{}…", cut)
    }
}

// ── Goals ──

/// An amount for goal output: Indian grouping below a lakh, then lakhs and
/// crores (₹19,819 · ₹25.00L · ₹1.00 Cr); US-style for other currencies.
fn goal_money(amount: f64, currency: &str) -> String {
    // Whole units, so ₹99,99,999.99 of float noise reads as ₹1.00 Cr.
    let amount = amount.round();
    if currency == "INR" {
        if amount.abs() >= 100_000.0 {
            format_large_number(amount, Some("INR"))
        } else {
            format!("₹{}", goals::indian_grouping(amount))
        }
    } else if amount.abs() >= 1_000_000.0 {
        format_large_number(amount, Some(currency))
    } else {
        format!(
            "{}{}",
            market::currency_symbol(Some(currency)),
            goals::western_grouping(amount)
        )
    }
}

fn pct_label(x: f64) -> String {
    let s = format!("{:.2}", x);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    tf(Msg::GoalPerYear, &[&s])
}

fn print_goal_json(kind: &str, currency: &str, plan: &impl Serialize) -> Result<()> {
    let mut v = serde_json::to_value(plan)?;
    round_floats(&mut v, 2);
    if let Some(o) = v.as_object_mut() {
        o.insert("goal".into(), serde_json::json!(kind));
        o.insert("currency".into(), serde_json::json!(currency));
    }
    output::print_json(&v)
}

/// The SIP part shared by every goal: what to invest, the totals, and the
/// year-by-year schedule.
fn print_sip_plan(p: &SipPlan, currency: &str) {
    let m = |x: f64| goal_money(x, currency);
    if p.savings > 0.0 {
        print_kv(t(Msg::GoalSavings), &m(p.savings));
        print_kv(t(Msg::GoalSavingsGrowTo), &m(p.savings_future_value));
    }
    print_kv(t(Msg::GoalExpectedReturn), &pct_label(p.annual_return_pct));
    if p.monthly_sip <= 0.0 {
        println!("\n  {}", t(Msg::GoalCovered).green());
        return;
    }
    print_kv(
        t(Msg::GoalMonthlySip),
        &tf(Msg::GoalPerMonth, &[&m(p.monthly_sip)])
            .bold()
            .green()
            .to_string(),
    );
    if p.step_up_pct > 0.0 {
        print_kv(t(Msg::GoalStepUp), &pct_label(p.step_up_pct));
        print_kv(
            t(Msg::GoalFinalSip),
            &tf(Msg::GoalPerMonth, &[&m(p.final_monthly_sip)]),
        );
    }
    print_kv(t(Msg::GoalTotalInvested), &m(p.total_invested));
    print_kv(t(Msg::GoalWealthGained), &m(p.gains));
    print_kv(t(Msg::GoalLumpSum), &m(p.lump_sum_today));

    print_section(t(Msg::GoalYearByYear));
    println!(
        "  {} {} {} {}",
        pad_start(t(Msg::GoalYear), 5).bold(),
        pad_start(t(Msg::GoalSipColumn), 20).bold(),
        pad_start(t(Msg::GoalInvested), 14).bold(),
        pad_start(t(Msg::Value), 14).bold(),
    );
    // Long plans show every fifth year and the last one.
    let long = p.schedule.len() > 20;
    for y in p.schedule.iter().filter(|y| {
        !long || y.year.is_multiple_of(5) || y.year == 1 || y.year as usize == p.schedule.len()
    }) {
        println!(
            "  {} {} {} {}",
            pad_start(&y.year.to_string(), 5),
            pad_start(&m(y.monthly_sip), 20),
            pad_start(&m(y.invested), 14),
            pad_start(&m(y.value), 14).cyan(),
        );
    }
}

/// `goal sip`: the monthly SIP that reaches `target` in `years`.
pub fn cmd_goal_sip(
    target: f64,
    years: u32,
    return_pct: f64,
    step_up_pct: f64,
    savings: f64,
    market: Market,
    json: bool,
) -> Result<()> {
    let currency = market_currency(market);
    let plan = goals::plan_sip(target, years, return_pct, step_up_pct, savings);
    if json {
        return print_goal_json("sip", currency, &plan);
    }
    print_header(&tf(
        Msg::GoalSipTitle,
        &[&goal_money(target, currency), &years],
    ));
    print_sip_plan(&plan, currency);
    println!("\n  {}\n", t(Msg::GoalAssumptions).dimmed().italic());
    Ok(())
}

/// `goal retire`: the corpus that pays today's monthly expense, grown by
/// inflation, through retirement, and the SIP that builds it.
pub fn cmd_goal_retire(
    monthly_expense: f64,
    years_to_retire: u32,
    years_in_retirement: u32,
    inflation_pct: f64,
    return_pre_pct: f64,
    return_post_pct: f64,
    step_up_pct: f64,
    savings: f64,
    market: Market,
    json: bool,
) -> Result<()> {
    let currency = market_currency(market);
    let plan = goals::plan_retirement(
        monthly_expense,
        years_to_retire,
        years_in_retirement,
        inflation_pct,
        return_pre_pct,
        return_post_pct,
        step_up_pct,
        savings,
    );
    if json {
        return print_goal_json("retire", currency, &plan);
    }
    let m = |x: f64| goal_money(x, currency);
    print_header(&tf(Msg::GoalRetireTitle, &[&years_to_retire]));
    let monthly = |x: f64| tf(Msg::GoalPerMonth, &[&m(x)]);
    print_kv(t(Msg::GoalExpenseToday), &monthly(monthly_expense));
    print_kv(t(Msg::GoalInflation), &pct_label(inflation_pct));
    print_kv(
        t(Msg::GoalExpenseAtRetirement),
        &monthly(plan.monthly_expense_at_retirement),
    );
    print_kv(
        t(Msg::GoalYearsInRetirement),
        &years_in_retirement.to_string(),
    );
    print_kv(t(Msg::GoalReturnAfter), &pct_label(return_post_pct));
    print_kv(
        t(Msg::GoalCorpusNeeded),
        &m(plan.corpus).bold().yellow().to_string(),
    );
    print_section(t(Msg::GoalReturnBefore));
    print_sip_plan(&plan.sip, currency);
    println!("\n  {}\n", t(Msg::GoalAssumptions).dimmed().italic());
    Ok(())
}

/// `goal education`: today's cost grown by education inflation, and the SIP
/// that pays it.
pub fn cmd_goal_education(
    cost: f64,
    years: u32,
    inflation_pct: f64,
    return_pct: f64,
    step_up_pct: f64,
    savings: f64,
    market: Market,
    json: bool,
) -> Result<()> {
    let currency = market_currency(market);
    let plan = goals::plan_education(cost, years, inflation_pct, return_pct, step_up_pct, savings);
    if json {
        return print_goal_json("education", currency, &plan);
    }
    let m = |x: f64| goal_money(x, currency);
    print_header(&tf(Msg::GoalEducationTitle, &[&years]));
    print_kv(t(Msg::GoalCostToday), &m(cost));
    print_kv(t(Msg::GoalInflation), &pct_label(inflation_pct));
    print_kv(
        &tf(Msg::GoalFutureCost, &[&years]),
        &m(plan.future_cost).bold().yellow().to_string(),
    );
    print_sip_plan(&plan.sip, currency);
    println!("\n  {}\n", t(Msg::GoalAssumptions).dimmed().italic());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::Lot;

    #[test]
    fn benchmark_names_resolve() {
        assert_eq!(
            resolve_benchmark(None, Market::In),
            ("^NSEI".into(), "NIFTY 50".into())
        );
        assert_eq!(resolve_benchmark(None, Market::Us).0, "^GSPC");
        assert_eq!(
            resolve_benchmark(None, Market::Global(market::Exchange::Jp)).0,
            "^N225"
        );
        assert_eq!(resolve_benchmark(Some("sensex"), Market::Us).0, "^BSESN");
        assert_eq!(resolve_benchmark(Some("S&P 500"), Market::In).0, "^GSPC");
        assert_eq!(resolve_benchmark(Some("nifty-50"), Market::Us).0, "^NSEI");
        assert_eq!(resolve_benchmark(Some("^nsei"), Market::Us).1, "NIFTY 50");
        assert_eq!(
            resolve_benchmark(Some("NIFTYBEES"), Market::In),
            ("NIFTYBEES.NS".into(), "NIFTYBEES.NS".into())
        );
        assert_eq!(resolve_benchmark(Some("SPY"), Market::Us).0, "SPY");
    }

    #[test]
    fn chart_history_keeps_one_close_per_day_and_splits() {
        let json = r#"{"chart":{"result":[{
            "meta":{"currency":"INR"},
            "timestamp":[1735702200,1735788600,1735875000,1735880000],
            "indicators":{"quote":[{"close":[100.0,null,102.0,103.0]}]},
            "events":{"splits":{"1735788600":{"date":1735788600,"numerator":2,"denominator":1}}}
        }]}}"#;
        let resp: ChartResponse = serde_json::from_str(json).unwrap();
        let h = history_from_chart(resp.chart.result.unwrap().remove(0), 19_800);
        assert_eq!(h.currency.as_deref(), Some("INR"));
        let d = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        // The null close is skipped; two bars on 3 Jan keep the later one.
        assert_eq!(
            h.closes,
            vec![(d("2025-01-01"), 100.0), (d("2025-01-03"), 103.0)]
        );
        assert_eq!(h.splits.len(), 1);
        assert_eq!(h.splits[0].date, "2025-01-02");

        // An FX bar for 2 Jul 2025 starts at midnight London time (23:00 UTC
        // on 1 Jul), whether gmtoffset says BST or, after the clocks change, GMT.
        let fx = r#"{"chart":{"result":[{"meta":{"currency":"INR"},
            "timestamp":[1751410800],
            "indicators":{"quote":[{"close":[85.5]}]}}]}}"#;
        for offset in [3600, 0] {
            let resp: ChartResponse = serde_json::from_str(fx).unwrap();
            let h = history_from_chart(resp.chart.result.unwrap().remove(0), offset);
            assert_eq!(
                h.closes,
                vec![(d("2025-07-02"), 85.5)],
                "gmtoffset {offset}"
            );
        }
        assert_eq!(h.splits[0].ratio(), 2.0);
    }

    #[test]
    fn sector_comes_from_the_asset_profile() {
        let v = serde_json::json!({"quoteSummary": {"result": [{"assetProfile": {"sector": "Technology"}}]}});
        assert_eq!(parse_sector(&v).as_deref(), Some("Technology"));
        let v = serde_json::json!({"quoteSummary": {"result": [{"assetProfile": {}}]}});
        assert_eq!(parse_sector(&v), None);
    }

    #[test]
    fn held_between_and_estimated_dates() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", Lot::new(10.0, 3000.0, "2025-06-01", "manual"));
        let mut lot = Lot::new(5.0, 1500.0, "2025-09-01", "sync");
        lot.date_estimated = true;
        p.add_lot("INFY.NS", lot);
        p.add_lot("OLD.NS", Lot::new(1.0, 10.0, "2020-01-01", "manual"));
        p.sell_fifo("OLD.NS", 1.0, 12.0, "2021-01-01", "manual")
            .unwrap();
        assert!(held_between(&p, "TCS.NS", "2025-01-01", "2025-12-31"));
        assert!(!held_between(&p, "TCS.NS", "2024-01-01", "2025-05-31"));
        assert!(!held_between(&p, "OLD.NS", "2025-01-01", "2025-12-31"));
        assert!(held_between(&p, "OLD.NS", "2020-06-01", "2025-12-31"));
        assert!(estimated_in_period(
            &p,
            "INFY.NS",
            "2025-01-01",
            "2025-12-31"
        ));
        assert!(!estimated_in_period(
            &p,
            "TCS.NS",
            "2025-01-01",
            "2025-12-31"
        ));
    }

    #[test]
    fn goal_money_uses_lakhs_and_crores() {
        assert_eq!(goal_money(19_818.6, "INR"), "₹19,819");
        assert_eq!(goal_money(2_500_000.0, "INR"), "₹25.00L");
        assert_eq!(goal_money(10_000_000.0, "INR"), "₹1.00 Cr");
        assert_eq!(goal_money(1_234.0, "USD"), "$1,234");
        assert_eq!(pct_label(12.0), "12% a year");
        assert_eq!(pct_label(6.5), "6.5% a year");
    }

    #[test]
    fn floats_are_rounded_in_goal_json() {
        let mut v = serde_json::json!({"a": 1.23456, "b": [2.0001, 3], "c": {"d": 0.125}});
        round_floats(&mut v, 2);
        assert_eq!(
            v,
            serde_json::json!({"a": 1.23, "b": [2.0, 3], "c": {"d": 0.13}})
        );
    }
}
