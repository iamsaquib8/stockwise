//! Indian tax-season commands: `tax reconcile` (against a broker's tax
//! P&L), `tax income` (dividends and interest with TDS, Schedule OS),
//! `tax foreign` (Schedule FA and Form 67), `tax advance` (instalments,
//! Sec. 234C, the reminder hook), the `tax export --schedule os|fa|form67`
//! files and `tax export --audit-pack`, plus the Sec. 50AA / 112 / 115BBH
//! section of `tax in`. Every figure is an estimate to verify with a CA or
//! the broker.

use super::*;
use crate::offmarket::Mark;
use crate::portfolio::{self as pf, Sale};
use crate::tax::{self, Country};
use crate::tax_export;
use crate::tax_foreign;
use crate::tax_income;
use crate::tax_reconcile::{self, Broker, OurSale, Reason, Term};
use chrono::{Datelike, NaiveDate};
use std::path::Path;

/// Shown under every tax-season report.
pub(crate) const ESTIMATE_NOTE: &str =
    "Estimate — verify with your CA or your broker's tax statements before filing.";

fn inr(amount: f64) -> String {
    let rounded = (amount * 100.0).round() / 100.0;
    if rounded < 0.0 {
        format!("-₹{}", crate::goals::indian_grouping(-rounded))
    } else {
        format!("₹{}", crate::goals::indian_grouping(rounded))
    }
}

fn date_of(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The first day of the Indian financial year `fy` names (2025-26,
/// AY2026-27), or of the current one; never one that has not started.
pub(crate) fn india_fy(fy: Option<&str>) -> Result<NaiveDate> {
    let today = chrono::Local::now().date_naive();
    let start = match fy {
        Some(label) => tax_export::parse_tax_year(Country::India, label).with_context(|| {
            format!("Invalid financial year '{label}': use a format like 2025-26 or AY2026-27")
        })?,
        None => Country::India.tax_year(today).0,
    };
    if start > today {
        bail!(
            "{} has not started yet",
            Country::India.tax_year_label(start)
        );
    }
    Ok(start)
}

// ── Rupee rates ──

/// Rupees per unit of other currencies, by day, from the ECB reference
/// rates (Frankfurter). A day without a rate takes the nearest earlier one
/// up to ten days back.
///
/// TODO(0.9 F): switch to `crate::fx_history::FxHistory` (cached in
/// `cache.db`) once the portfolio-math stream is merged; this is the thin
/// stand-in it asked for.
#[derive(Debug, Default, Clone)]
pub(crate) struct RupeeRates {
    series: Vec<(String, Vec<(NaiveDate, f64)>)>,
}

impl RupeeRates {
    /// Load the rates each `(currency, earliest day)` needs, up to today.
    /// A currency the ECB does not publish, or a network failure, leaves it
    /// without rates (callers name what they left out).
    pub(crate) async fn load(needs: &[(String, NaiveDate)]) -> Self {
        let mut wanted: Vec<(String, NaiveDate)> = Vec::new();
        for (cur, from) in needs {
            let (_, major) = crate::market::to_major(1.0, cur);
            let major = major.to_uppercase();
            if major == "INR" {
                continue;
            }
            match wanted.iter_mut().find(|(c, _)| *c == major) {
                Some((_, d)) => *d = (*d).min(*from),
                None => wanted.push((major, *from)),
            }
        }
        let mut rates = Self::default();
        if wanted.is_empty() {
            return rates;
        }
        use crate::providers::{Provider, ProviderKind};
        let Provider::Frankfurter(f) =
            Provider::new(ProviderKind::Frankfurter, String::new(), None)
        else {
            return rates;
        };
        for (cur, from) in wanted {
            let symbol = format!("{cur}INR=X");
            match f
                .daily(&symbol, Some(from - chrono::Duration::days(10)))
                .await
            {
                Ok(series) => {
                    let mut points: Vec<(NaiveDate, f64)> = series
                        .bars
                        .iter()
                        .filter_map(|b| Some((b.date, b.close?)))
                        .collect();
                    points.sort_by_key(|p| p.0);
                    rates.series.push((cur, points));
                }
                Err(e) => eprintln!("  Warning: no {cur}/INR rates ({e:#})"),
            }
        }
        rates
    }

    /// Rupees per unit of `currency` on `day` (pence and other minor units
    /// are converted by the caller: this is the major currency's rate).
    pub(crate) fn rate(&self, currency: &str, day: NaiveDate) -> Option<f64> {
        let (_, major) = crate::market::to_major(1.0, currency);
        let major = major.to_uppercase();
        if major == "INR" {
            return Some(1.0);
        }
        let points = &self.series.iter().find(|(c, _)| *c == major)?.1;
        let idx = points.partition_point(|(d, _)| *d <= day);
        let (d, r) = *points.get(idx.checked_sub(1)?)?;
        ((day - d).num_days() <= 10 && r > 0.0).then_some(r)
    }
}

// ── tax reconcile ──

/// StockWise's rupee sales of the year starting `start`, as reconcile
/// compares them. `account` keeps only sales from that demat account (and
/// sales saved without one).
async fn our_sales(
    portfolio: &pf::Portfolio,
    start: NaiveDate,
    account: Option<&str>,
) -> Vec<OurSale> {
    let (from, to) = Country::India.tax_year(start);
    let sales: Vec<&Sale> = portfolio
        .realized
        .iter()
        .filter(|s| date_of(&s.sell_date).is_some_and(|d| d >= from && d <= to))
        .filter(|s| pf::symbol_currency(&s.symbol) == Some("INR"))
        .filter(|s| match (account, s.account_key()) {
            (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
            _ => true,
        })
        .collect();
    let mut symbols: Vec<String> = sales.iter().map(|s| s.symbol.clone()).collect();
    symbols.sort();
    symbols.dedup();
    let isins = super::planning::lookup_isins(&symbols).await;
    let old = tax_export::grandfathered_symbols(&portfolio.realized, start);
    let fmvs = super::planning::fmv_31_jan_2018(&old).await;
    sales
        .into_iter()
        .filter_map(|s| {
            let (b, d) = (date_of(&s.buy_date)?, date_of(&s.sell_date)?);
            let intraday = s.is_intraday();
            let long = !intraday && Country::India.is_long_term(b, d);
            let fmv = fmvs.iter().find(|(f, _)| *f == s.symbol).map(|(_, v)| *v);
            let unit = tax_export::lot_unit_cost(&s.buy_date, long, s.buy_price, s.sell_price, fmv);
            Some(OurSale {
                symbol: s.symbol.clone(),
                isin: isins
                    .iter()
                    .find(|(x, _)| *x == s.symbol)
                    .map(|(_, i)| i.clone()),
                qty: s.qty,
                buy_date: b,
                sell_date: d,
                cost: s.qty * s.buy_price,
                proceeds: s.qty * s.sell_price,
                gain: s.qty * (s.sell_price - unit),
                term: if intraday {
                    Term::Intraday
                } else if long {
                    Term::Long
                } else {
                    Term::Short
                },
                grandfathered: unit != s.buy_price,
                // TODO(0.9 F): the sale's recorded charges once `Sale` has them.
                charges: 0.0,
            })
        })
        .collect()
}

/// `tax reconcile FILE`: StockWise's realised gains against a broker's tax
/// P&L, sale by sale. Read-only.
pub async fn cmd_tax_reconcile(
    file: &Path,
    broker: Option<Broker>,
    fy: Option<&str>,
    all_accounts: bool,
    json: bool,
) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("Cannot read {}", file.display()))?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (detected, rows) = tax_reconcile::parse_file(&bytes, &name)?;
    let broker = broker.unwrap_or(detected);
    // The year: as given, else the one most of the file's sales are in.
    let start = match fy {
        Some(_) => india_fy(fy)?,
        None => {
            let mut years: Vec<NaiveDate> = rows
                .iter()
                .map(|r| Country::India.tax_year(r.sell_date).0)
                .collect();
            years.sort();
            let mut best = (years[0], 0);
            for y in &years {
                let n = years.iter().filter(|x| *x == y).count();
                if n > best.1 {
                    best = (*y, n);
                }
            }
            best.0
        }
    };
    let (from, to) = Country::India.tax_year(start);
    let fy_label = Country::India.tax_year_label(start);
    let outside = rows
        .iter()
        .filter(|r| r.sell_date < from || r.sell_date > to)
        .count();
    let theirs: Vec<_> = rows
        .into_iter()
        .filter(|r| r.sell_date >= from && r.sell_date <= to)
        .collect();
    let portfolio = pf::Portfolio::load()?;
    let account = if all_accounts { None } else { broker.account() };
    let ours = our_sales(&portfolio, start, account).await;
    let r = tax_reconcile::reconcile(broker, &fy_label, &ours, &theirs);

    if json {
        let mut v = serde_json::to_value(&r)?;
        v["agreeing"] = r.agreeing().into();
        v["outside_year"] = outside.into();
        v["account"] = account.into();
        v["disclaimer"] = ESTIMATE_NOTE.into();
        return output::print_json(&v);
    }

    print_header(&format!(
        "Tax Reconcile — StockWise vs {} — {}",
        broker.name(),
        fy_label
    ));
    println!(
        "  {} matched ({} agree, {} differ) · {} only in StockWise · {} only in {}",
        r.matched.len(),
        r.agreeing().to_string().green(),
        (r.matched.len() - r.agreeing()).to_string().yellow(),
        r.only_ours.len(),
        r.only_theirs.len(),
        broker.name()
    );
    if let Some(a) = account {
        println!(
            "  {}",
            format!(
                "StockWise sales from the {a} account (and sales saved without one); --all-accounts compares every sale."
            )
            .dimmed()
        );
    }
    if outside > 0 {
        println!(
            "  {}",
            format!("{outside} row(s) of the file are outside {fy_label} and were skipped.")
                .dimmed()
        );
    }

    let differ: Vec<&tax_reconcile::Matched> = r.matched.iter().filter(|m| !m.agrees()).collect();
    if !differ.is_empty() {
        print_section("Differences");
        println!(
            "  {:<12} {:<11} {:>9} {:>13} {:>13} {:>11}  {}",
            "Symbol".bold(),
            "Sold".bold(),
            "Qty".bold(),
            "StockWise".bold(),
            broker.name().bold(),
            "Diff".bold(),
            "Likely reason".bold()
        );
        for m in &differ {
            let qty = if (m.qty_ours - m.qty_theirs).abs() > 1e-6 {
                format!("{}/{}", m.qty_ours, m.qty_theirs)
            } else {
                format!("{}", m.qty_ours)
            };
            let reasons: Vec<&str> = m.reasons.iter().map(|x| x.label()).collect();
            println!(
                "  {:<12} {:<11} {:>9} {:>13} {:>13} {:>11}  {}",
                m.symbol.cyan(),
                m.sell_date.format("%Y-%m-%d"),
                qty,
                inr(m.gain_ours),
                inr(m.gain_theirs),
                inr(m.difference),
                reasons.join("; ").yellow()
            );
            if m.reasons.contains(&Reason::BuyDate) {
                println!(
                    "  {:<12} {}",
                    "",
                    format!(
                        "bought {} (StockWise) vs {} ({})",
                        m.buy_date_ours
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "several days".into()),
                        m.buy_date_theirs
                            .map(|d| d.to_string())
                            .unwrap_or_else(|| "several days".into()),
                        broker.name()
                    )
                    .dimmed()
                );
            }
        }
    }
    if !r.only_ours.is_empty() {
        print_section(&format!(
            "Only in StockWise (not in the {} file)",
            broker.name()
        ));
        for o in &r.only_ours {
            println!(
                "  {:<12} {:<11} {:>9} {:>13}  {}",
                o.symbol.cyan(),
                o.sell_date.format("%Y-%m-%d"),
                o.qty,
                inr(o.gain),
                o.term.label().dimmed()
            );
        }
    }
    if !r.only_theirs.is_empty() {
        print_section(&format!(
            "Only in {} (not recorded in StockWise)",
            broker.name()
        ));
        for t in &r.only_theirs {
            println!(
                "  {:<12} {:<11} {:>9} {:>13}  {}",
                t.symbol.cyan(),
                t.sell_date.format("%Y-%m-%d"),
                t.qty,
                inr(t.gain()),
                t.term.map(|x| x.label()).unwrap_or("").dimmed()
            );
        }
    }

    print_section("Totals");
    println!(
        "  {:<10} {:>14} {:>14} {:>14}",
        "",
        "StockWise".bold(),
        broker.name().bold(),
        "Difference".bold()
    );
    for (label, a, b) in [
        ("STCG", r.ours.stcg, r.theirs.stcg),
        ("LTCG", r.ours.ltcg, r.theirs.ltcg),
        ("Intraday", r.ours.intraday, r.theirs.intraday),
    ] {
        let diff = a - b;
        let d = if diff.abs() < 0.5 {
            inr(0.0).green()
        } else {
            inr(diff).yellow()
        };
        println!("  {:<10} {:>14} {:>14} {:>14}", label, inr(a), inr(b), d);
    }

    let mut hints = Vec::new();
    let seen = |x: Reason| r.matched.iter().any(|m| m.reasons.contains(&x));
    if seen(Reason::Quantity) {
        hints.push("Quantity gaps: check `stockwise portfolio actions` for a split or bonus not yet applied, or a missing buy.");
    }
    if seen(Reason::Grandfathering) {
        hints.push("Grandfathering: pre-2018 lots use the 31 Jan 2018 price (Sec. 112A); StockWise takes the day's high from Yahoo, brokers may use another source.");
    }
    if seen(Reason::Charges) {
        hints.push("Charges: the broker nets STT, brokerage and stamp duty; STT is not deductible for listed shares (Sec. 48), other charges are.");
    }
    if !r.only_theirs.is_empty() {
        hints.push("Sales only in the broker file: record them with `stockwise portfolio sell` or import the tradebook.");
    }
    if !hints.is_empty() {
        println!();
        for h in hints {
            println!("  {} {}", "→".cyan(), h);
        }
    }
    println!("\n  {}\n", ESTIMATE_NOTE.yellow().italic());
    Ok(())
}

// ── tax income ──

/// The foreign currencies of the portfolio's dividends with the earliest
/// day each is needed from.
fn dividend_currencies(portfolio: &pf::Portfolio, from: NaiveDate) -> Vec<(String, NaiveDate)> {
    let mut out: Vec<(String, NaiveDate)> = Vec::new();
    for d in &portfolio.dividends {
        let Some(cur) = d
            .currency
            .clone()
            .or_else(|| pf::symbol_currency(&d.symbol).map(str::to_string))
        else {
            continue;
        };
        let day = date_of(&d.ex_date).unwrap_or(from);
        if cur.eq_ignore_ascii_case("INR") || day < from {
            continue;
        }
        // Form 67 takes the rate of the previous month's last day.
        let need = tax_foreign::form67_rate_date(day).min(day);
        match out.iter_mut().find(|(c, _)| c.eq_ignore_ascii_case(&cur)) {
            Some((_, d)) => *d = (*d).min(need),
            None => out.push((cur, need)),
        }
    }
    out
}

async fn income_for(portfolio: &pf::Portfolio, start: NaiveDate) -> tax_income::IncomeReport {
    let rates = RupeeRates::load(&dividend_currencies(portfolio, start)).await;
    tax_income::income_report(portfolio, start, |c, d| rates.rate(c, d))
}

/// `tax income`: dividends and SGB interest of a financial year with TDS,
/// by Sec. 234C period, for ITR Schedule OS.
pub async fn cmd_tax_income(fy: Option<&str>, fmt: Format, out: Option<&Path>) -> Result<()> {
    let start = india_fy(fy)?;
    let portfolio = pf::Portfolio::load()?;
    let r = income_for(&portfolio, start).await;
    if let Some(path) = out {
        std::fs::write(path, tax_income::schedule_os_csv(&r))
            .with_context(|| format!("Cannot write {}", path.display()))?;
    }
    match fmt {
        Format::Json => return output::print_json(&r),
        Format::Csv if out.is_none() => {
            print!("{}", tax_income::schedule_os_csv(&r));
            return Ok(());
        }
        _ => {}
    }
    print_header(&portfolio_title_tax(&format!(
        "Dividend & Interest Income — {}",
        r.fy_label
    )));
    if r.rows.is_empty() {
        println!(
            "  {}",
            "No dividends or SGB interest recorded for this year. `stockwise portfolio actions --apply` records the dividends your holdings received."
                .dimmed()
        );
    } else {
        println!(
            "  {:<11} {:<26} {:<17} {:<16} {:>12} {:>10} {:>12}",
            "Date".bold(),
            "Payer".bold(),
            "Nature".bold(),
            "ITR period".bold(),
            "Gross".bold(),
            "TDS".bold(),
            "Net".bold()
        );
        for row in &r.rows {
            let tds = if row.foreign_tax > 0.0 {
                format!("{} WHT", inr(row.foreign_tax))
            } else {
                inr(row.tds)
            };
            println!(
                "  {:<11} {:<26} {:<17} {:<16} {:>12} {:>10} {:>12}",
                row.date.format("%Y-%m-%d"),
                row.payer.cyan(),
                row.kind.label(),
                tax_income::QUARTER_LABELS[row.quarter],
                inr(row.gross),
                tds,
                inr(row.net)
            );
        }
        print_section("Totals");
        print_kv("Dividends (Indian)", &inr(r.dividends));
        if r.foreign_dividends > 0.0 {
            print_kv("Dividends (foreign)", &inr(r.foreign_dividends));
        }
        if r.interest > 0.0 {
            print_kv("SGB interest", &inr(r.interest));
        }
        print_kv("Gross income", &inr(r.gross));
        print_kv("TDS deducted (estimate)", &inr(r.tds));
        if r.foreign_tax > 0.0 {
            print_kv("Tax withheld abroad", &inr(r.foreign_tax));
        }
        print_kv("Net received", &inr(r.net));

        print_section("Quarterly breakup (Schedule OS, Sec. 234C periods)");
        for (i, label) in tax_income::QUARTER_LABELS.iter().enumerate() {
            println!(
                "  {:<18} {:>12} dividends {:>12} all income",
                label,
                inr(r.dividend_by_quarter[i]),
                inr(r.income_by_quarter[i])
            );
        }
    }
    println!();
    println!(
        "  {} Dividends are taxed at your slab rate. TDS is 10% once one company's or fund's dividends pass {} this year (Sec. 194/194K); SGB interest has no TDS.",
        "·".dimmed(),
        inr(r.tds_threshold)
    );
    println!(
        "  {} Dated by ex-date; the payment usually follows within a month. Match TDS with Form 26AS / AIS.",
        "·".dimmed()
    );
    if r.foreign_tax > 0.0 {
        println!(
            "  {} Foreign dividends: claim the tax withheld abroad with Form 67 — see `stockwise tax foreign`.",
            "→".cyan()
        );
    }
    if !r.no_rate.is_empty() {
        println!(
            "  {} No rupee rate for: {} (left out).",
            "!".yellow(),
            r.no_rate.join(", ")
        );
    }
    if !r.unknown_sgbs.is_empty() {
        println!(
            "  {} SGB tranche not known, no interest computed: {}.",
            "!".yellow(),
            r.unknown_sgbs.join(", ")
        );
    }
    if let Some(path) = out {
        println!(
            "\n  {}",
            format!("Schedule OS CSV written to {}", path.display()).green()
        );
    }
    println!("  {}\n", ESTIMATE_NOTE.yellow().italic());
    Ok(())
}

fn portfolio_title_tax(title: &str) -> String {
    super::portfolio::portfolio_title(title)
}

// ── tax foreign (Schedule FA, Form 67) ──

/// Daily closes of `symbols` in calendar year `year`, from Yahoo.
async fn year_closes(symbols: &[String], year: i32) -> Vec<(String, Vec<(NaiveDate, f64)>)> {
    let at = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .map(|t| t.and_utc().timestamp())
            .unwrap_or(0)
    };
    let from = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date");
    let to = NaiveDate::from_ymd_opt(year + 1, 1, 1).expect("valid date");
    let mut out = Vec::new();
    let src = views::LiveSource::new();
    let Ok(y) = views::DataSource::yahoo(&src).await else {
        return out;
    };
    for sym in symbols {
        let Ok(chart) = y.get_chart_between(sym, at(from), at(to), "1d").await else {
            continue;
        };
        let h = views::history_from_chart(chart, sym, "", "1d");
        let points: Vec<(NaiveDate, f64)> = h
            .bars
            .iter()
            .filter_map(|b| {
                let day = chrono::DateTime::from_timestamp(b.ts, 0)?.date_naive();
                Some((day, b.close?))
            })
            .filter(|(_, c)| c.is_finite() && *c > 0.0)
            .collect();
        out.push((sym.clone(), points));
    }
    out
}

/// Schedule FA for calendar year `year` and Form 67 for the financial year
/// starting `start`, with the prices and rates they need.
async fn foreign_reports(
    portfolio: &pf::Portfolio,
    year: i32,
    start: NaiveDate,
    slab: f64,
) -> (tax_foreign::ScheduleFa, tax_foreign::Form67) {
    let mut symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .chain(portfolio.realized.iter().map(|s| s.symbol.clone()))
        .filter(|s| tax_foreign::is_foreign(s))
        .collect();
    symbols.sort();
    symbols.dedup();
    let year_start = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date");
    let mut needs: Vec<(String, NaiveDate)> = Vec::new();
    for s in &symbols {
        let cur = pf::symbol_currency(s).unwrap_or("USD").to_string();
        let earliest = portfolio
            .holdings
            .iter()
            .filter(|h| &h.symbol == s)
            .flat_map(|h| h.lots.iter().map(|l| l.date.clone()))
            .chain(
                portfolio
                    .realized
                    .iter()
                    .filter(|x| &x.symbol == s)
                    .map(|x| x.buy_date.clone()),
            )
            .filter_map(|d| date_of(&d))
            .min()
            .unwrap_or(year_start)
            .min(year_start);
        needs.push((cur, earliest));
    }
    needs.extend(dividend_currencies(portfolio, start));
    let rates = RupeeRates::load(&needs).await;
    let closes = if symbols.is_empty() {
        Vec::new()
    } else {
        year_closes(&symbols, year).await
    };
    let fa = tax_foreign::schedule_fa(
        portfolio,
        year,
        |s| {
            closes
                .iter()
                .find(|(x, _)| x == s)
                .map(|(_, p)| p.as_slice())
        },
        |c, d| rates.rate(c, d),
    );
    let f67 = tax_foreign::form67(portfolio, start, slab, |c, d| rates.rate(c, d));
    (fa, f67)
}

/// The calendar year and financial year `tax foreign` reports: `--year`
/// names the calendar year (Schedule FA), whose financial year starts in
/// April of it; `--fy` names the financial year, whose Schedule FA covers
/// the calendar year it starts in.
fn foreign_years(fy: Option<&str>, year: Option<i32>) -> Result<(i32, NaiveDate)> {
    match year {
        Some(y) => {
            let start = NaiveDate::from_ymd_opt(y, 4, 1).context("Invalid year")?;
            let start = start.min(Country::India.tax_year(chrono::Local::now().date_naive()).0);
            Ok((y, start))
        }
        None => {
            let start = india_fy(fy)?;
            Ok((start.year(), start))
        }
    }
}

/// `tax foreign`: Schedule FA (foreign assets, calendar year) and Form 67
/// (foreign tax credit) inputs.
pub async fn cmd_tax_foreign(
    fy: Option<&str>,
    year: Option<i32>,
    slab: f64,
    json: bool,
) -> Result<()> {
    let (year, start) = foreign_years(fy, year)?;
    let portfolio = pf::Portfolio::load()?;
    let (fa, f67) = foreign_reports(&portfolio, year, start, slab / 100.0).await;
    let today = chrono::Local::now().date_naive();
    let in_progress = today.year() <= year;
    if json {
        return output::print_json(&serde_json::json!({
            "schedule_fa": fa,
            "form_67": f67,
            "calendar_year_in_progress": in_progress,
            "rate_source": "ECB reference rate (Frankfurter); the rules ask for the SBI TT buying rate",
            "disclaimer": ESTIMATE_NOTE,
        }));
    }
    print_header(&portfolio_title_tax(&format!(
        "Foreign Assets — Schedule FA {year} · Form 67 {}",
        f67.fy_label
    )));
    print_section(&format!("Schedule FA, Table A3 — 1 Jan to 31 Dec {year}"));
    if fa.rows.is_empty() {
        println!(
            "  {}",
            "No foreign shares held during this calendar year.".dimmed()
        );
    } else {
        println!(
            "  {:<10} {:<11} {:>9} {:>13} {:>13} {:>13} {:>11} {:>13}",
            "Entity".bold(),
            "Acquired".bold(),
            "Qty".bold(),
            "Initial".bold(),
            "Peak".bold(),
            "Closing".bold(),
            "Dividends".bold(),
            "Sale proceeds".bold()
        );
        for r in &fa.rows {
            println!(
                "  {:<10} {:<11} {:>9} {:>13} {:>13} {:>13} {:>11} {:>13}",
                r.entity.cyan(),
                r.acquired.format("%Y-%m-%d"),
                r.quantity,
                inr(r.initial_value),
                inr(r.peak_value),
                inr(r.closing_value),
                inr(r.dividends),
                inr(r.sale_proceeds)
            );
        }
        println!(
            "  {:<10} {:<11} {:>9} {:>13} {:>13} {:>13} {:>11} {:>13}",
            "Total".bold(),
            "",
            "",
            inr(fa.initial_value),
            inr(fa.peak_value),
            inr(fa.closing_value),
            inr(fa.dividends),
            inr(fa.sale_proceeds)
        );
        if in_progress {
            println!(
                "  {}",
                format!("{year} is not over: peak and closing values are to date.").yellow()
            );
        }
    }
    for (what, list) in [
        ("no price history", &fa.no_prices),
        ("no rupee rate", &fa.no_rate),
    ] {
        if !list.is_empty() {
            println!("  {} Left out ({what}): {}", "!".yellow(), list.join(", "));
        }
    }

    print_section(&format!(
        "Form 67 — foreign tax credit, {} (slab {}%)",
        f67.fy_label, slab
    ));
    if f67.rows.is_empty() {
        println!(
            "  {}",
            "No foreign dividends recorded for this year.".dimmed()
        );
    } else {
        println!(
            "  {:<11} {:<10} {:>10} {:>10} {:>12} {:>12} {:>12}",
            "Date".bold(),
            "Payer".bold(),
            "Gross".bold(),
            "Withheld".bold(),
            "Gross ₹".bold(),
            "Withheld ₹".bold(),
            "Credit ₹".bold()
        );
        for r in &f67.rows {
            println!(
                "  {:<11} {:<10} {:>10} {:>10} {:>12} {:>12} {:>12}",
                r.date.format("%Y-%m-%d"),
                r.payer.cyan(),
                format!("{:.2} {}", r.gross, r.currency),
                format!("{:.2}", r.tax_withheld),
                inr(r.gross_inr),
                inr(r.tax_withheld_inr),
                inr(r.credit_inr)
            );
        }
        print_kv("Foreign tax credit (estimate)", &inr(f67.credit_inr));
        println!(
            "  {}",
            "File Form 67 before the ITR. Rates: last day of the month before the tax was withheld (Rule 128)."
                .dimmed()
        );
    }
    println!(
        "\n  {} Rupee values use the ECB reference rate for the day; the rules ask for the SBI TT buying rate, which differs slightly.",
        "·".dimmed()
    );
    println!(
        "  {} `stockwise tax export in --schedule fa` / `--schedule form67` write these as CSV.",
        "→".cyan()
    );
    println!("  {}\n", ESTIMATE_NOTE.yellow().italic());
    Ok(())
}

// ── tax export --schedule os|fa|form67, --audit-pack ──

/// The Indian schedules `tax export in --schedule` writes besides Schedule CG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ExportSchedule {
    /// Schedule CG / 112A (capital gains)
    Cg,
    /// Schedule OS: dividends and interest with TDS, quarterly breakup
    Os,
    /// Schedule FA Table A3: foreign assets, calendar year
    Fa,
    /// Form 67: foreign tax credit on foreign dividends
    Form67,
}

/// `tax export in --schedule os|fa|form67`: the schedule as CSV (or JSON),
/// on stdout or into `out`.
pub async fn cmd_tax_export_schedule(
    schedule: ExportSchedule,
    fy: Option<&str>,
    slab: f64,
    out: Option<&Path>,
    json: bool,
) -> Result<()> {
    let start = india_fy(fy)?;
    let portfolio = pf::Portfolio::load()?;
    let (body, what) = match schedule {
        ExportSchedule::Cg => bail!("Schedule CG is `tax export in` without --schedule"),
        ExportSchedule::Os => {
            let r = income_for(&portfolio, start).await;
            let body = if json {
                serde_json::to_string_pretty(&r)? + "\n"
            } else {
                tax_income::schedule_os_csv(&r)
            };
            (
                body,
                format!("Schedule OS {}: {} rows", r.fy_label, r.rows.len()),
            )
        }
        ExportSchedule::Fa | ExportSchedule::Form67 => {
            let (fa, f67) = foreign_reports(&portfolio, start.year(), start, slab / 100.0).await;
            if schedule == ExportSchedule::Fa {
                let body = if json {
                    serde_json::to_string_pretty(&fa)? + "\n"
                } else {
                    tax_foreign::schedule_fa_csv(&fa)
                };
                (
                    body,
                    format!("Schedule FA {}: {} rows", fa.calendar_year, fa.rows.len()),
                )
            } else {
                let body = if json {
                    serde_json::to_string_pretty(&f67)? + "\n"
                } else {
                    tax_foreign::form67_csv(&f67)
                };
                (
                    body,
                    format!("Form 67 {}: {} rows", f67.fy_label, f67.rows.len()),
                )
            }
        }
    };
    match out {
        Some(path) => {
            std::fs::write(path, &body)
                .with_context(|| format!("Cannot write {}", path.display()))?;
            println!(
                "  {}",
                format!("{what} written to {}", path.display()).green()
            );
            println!("  {}", ESTIMATE_NOTE.yellow().italic());
        }
        None => print!("{body}"),
    }
    Ok(())
}

/// `tax export in --audit-pack DIR`: one workpaper CSV per sale of the year
/// (lot, dates, cost, 31 Jan 2018 FMV where it applies, gain, rule), a
/// `summary.csv` and the Schedule CG CSV, in `dir`.
pub async fn cmd_tax_audit_pack(fy: Option<&str>, dir: &Path) -> Result<()> {
    let start = india_fy(fy)?;
    let portfolio = pf::Portfolio::load()?;
    let (from, to) = Country::India.tax_year(start);
    let mut sold: Vec<String> = portfolio
        .realized
        .iter()
        .filter(|s| date_of(&s.sell_date).is_some_and(|d| d >= from && d <= to))
        .map(|s| s.symbol.clone())
        .collect();
    sold.sort();
    sold.dedup();
    let isins = super::planning::lookup_isins(&sold).await;
    let marks = super::planning::sale_marks(&views::LiveSource::new(), &sold).await;
    let old = tax_export::grandfathered_symbols(&portfolio.realized, start);
    let fmvs = super::planning::fmv_31_jan_2018(&old).await;
    let isin = |sym: &str| isins.iter().find(|(s, _)| s == sym).map(|(_, i)| i.clone());
    let fmv = |sym: &str| fmvs.iter().find(|(s, _)| s == sym).map(|(_, v)| *v);
    let rows = tax_export::india_audit_rows(&portfolio.realized, start, isin, &marks, fmv);
    let year = Country::India.tax_year_label(start);

    std::fs::create_dir_all(dir).with_context(|| format!("Cannot create {}", dir.display()))?;
    for r in &rows {
        let path = dir.join(&r.file);
        std::fs::write(&path, r.workpaper_csv(&year))
            .with_context(|| format!("Cannot write {}", path.display()))?;
    }
    std::fs::write(
        dir.join("summary.csv"),
        tax_export::audit_summary_csv(&rows, &year),
    )
    .with_context(|| format!("Cannot write {}", dir.join("summary.csv").display()))?;
    let schedule =
        tax_export::india_schedule_with_fmv(&portfolio.realized, start, isin, &marks, fmv);
    std::fs::write(
        dir.join("schedule-cg.csv"),
        tax_export::india_csv(&schedule),
    )
    .with_context(|| format!("Cannot write {}", dir.join("schedule-cg.csv").display()))?;

    print_header(&portfolio_title_tax(&format!("Audit Pack — {year}")));
    print_kv("Sales", &rows.len().to_string());
    let mut rules: Vec<&str> = rows.iter().map(|r| r.rule.as_str()).collect();
    rules.sort();
    rules.dedup();
    for rule in rules {
        let gain: f64 = rows.iter().filter(|r| r.rule == rule).map(|r| r.gain).sum();
        println!("  {:<64} {:>14}", rule, inr(gain));
    }
    println!(
        "\n  {}",
        format!(
            "Wrote {} workpaper(s), summary.csv and schedule-cg.csv to {}",
            rows.len(),
            dir.display()
        )
        .green()
    );
    println!("  {}\n", ESTIMATE_NOTE.yellow().italic());
    Ok(())
}

// ── tax in: gains outside Sec. 111A/112A ──

/// One sale under Sec. 50AA, 112 or 115BBH in `tax in`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct OtherGainRow {
    pub(crate) symbol: String,
    pub(crate) sold: String,
    pub(crate) gain: f64,
    pub(crate) rule: tax::IndiaRule,
    pub(crate) rule_label: &'static str,
}

/// The year's sales `tax in` leaves out of Sec. 111A/112A that have a rule
/// of their own (debt and other funds, gold and international ETFs, SGBs,
/// market-linked debentures, crypto), with their totals.
pub(crate) fn other_gains(
    portfolio: &pf::Portfolio,
    mark: impl Fn(&str) -> Option<Mark>,
    today: NaiveDate,
) -> (Vec<OtherGainRow>, tax::OtherGains) {
    let (from, to) = Country::India.tax_year(today);
    let mut rows = Vec::new();
    let mut gains = Vec::new();
    for s in &portfolio.realized {
        let (Some(b), Some(d)) = (date_of(&s.buy_date), date_of(&s.sell_date)) else {
            continue;
        };
        if d < from || d > to || pf::symbol_currency(&s.symbol) != Some("INR") || s.is_intraday() {
            continue;
        }
        let m = mark(&s.symbol);
        let Some(asset) = crate::offmarket::india_asset(&s.symbol, m.as_ref()) else {
            continue;
        };
        let rule = tax::india_rule(asset, b, d);
        let gain = s.gain();
        gains.push(tax::OtherGain {
            rule,
            gain,
            proceeds: s.qty * s.sell_price,
            before_rate_change: d < tax::india_rate_change_date(),
        });
        rows.push(OtherGainRow {
            symbol: s.symbol.clone(),
            sold: s.sell_date.clone(),
            gain: round(gain, 2),
            rule,
            rule_label: rule.label(),
        });
    }
    (rows, tax::india_other_gains(&gains))
}

/// The `tax in` section for [`other_gains`].
pub(crate) fn print_other_gains(rows: &[OtherGainRow], o: &tax::OtherGains) {
    if rows.is_empty() {
        return;
    }
    print_section("Other capital gains — Sec. 50AA, 112 and 115BBH");
    for r in rows {
        println!(
            "  {:<16} {:<11} {:>12}  {}",
            r.symbol.cyan(),
            r.sold,
            inr(r.gain),
            r.rule_label.dimmed()
        );
    }
    if o.slab_income().abs() > 0.005 {
        println!(
            "  • Taxed at your slab rate (Sec. 50AA {} + other short-term {}): add {} to your income.",
            inr(o.sec_50aa),
            inr(o.short_term_slab),
            inr(o.slab_income())
        );
    }
    if o.long_term_112.abs() > 0.005 {
        println!(
            "  • Sec. 112 long-term (no ₹1.25 lakh exemption, no indexation): {} — tax {}",
            inr(o.long_term_112),
            inr(o.long_term_112_tax)
        );
    }
    if o.vda_gains > 0.0 || o.vda_losses_ignored > 0.0 {
        println!(
            "  • Crypto (Sec. 115BBH, 30% flat): gains {} — tax {}; losses {} cannot be set off or carried forward. TDS of 1% (Sec. 194S) on sales: about {}.",
            inr(o.vda_gains),
            inr(o.vda_tax),
            inr(o.vda_losses_ignored),
            inr(o.vda_tds)
        );
    }
    if o.flat_tax() > 0.005 {
        println!(
            "  • Sec. 112 + 115BBH tax with 4% cess: {}",
            inr(o.flat_tax())
        );
    }
}

// ── tax advance ──

/// Advance-tax payments recorded with `tax advance --paid DATE:AMOUNT
/// --record`, kept in the data directory so later runs (and the reminder)
/// count them.
#[derive(Debug, Default, Serialize, serde::Deserialize)]
pub(crate) struct AdvanceLedger {
    #[serde(default)]
    pub(crate) payments: Vec<tax::AdvancePayment>,
}

const ADVANCE_LEDGER: &str = "advance_tax.json";

fn load_ledger() -> AdvanceLedger {
    crate::store::load(ADVANCE_LEDGER).unwrap_or_default()
}

/// The recorded payments plus `extra`, without duplicates.
fn with_recorded(extra: &[tax::AdvancePayment]) -> Vec<tax::AdvancePayment> {
    let mut all = load_ledger().payments;
    for p in extra {
        if !all.contains(p) {
            all.push(*p);
        }
    }
    all
}

/// The schedule `tax advance` and the reminder work from: capital gains,
/// plus dividends and interest at `slab` when given.
async fn advance_schedule(
    portfolio: &pf::Portfolio,
    fy: Option<&str>,
    payments: &[tax::AdvancePayment],
    slab: Option<f64>,
) -> Result<(tax::AdvanceTaxSchedule, Option<tax_income::IncomeReport>)> {
    let symbols: Vec<String> = portfolio
        .realized
        .iter()
        .map(|s| s.symbol.clone())
        .collect();
    let marks = super::planning::sale_marks(&views::LiveSource::new(), &symbols).await;
    let fmvs = super::planning::advance_fmvs(portfolio).await;
    let (income, report) = match slab {
        Some(pct) => {
            let start = india_fy(fy)?;
            let r = income_for(portfolio, start).await;
            (tax_income::advance_income(&r, pct / 100.0), Some(r))
        }
        None => (Vec::new(), None),
    };
    let s = super::planning::advance_tax_schedule_with_income(
        portfolio, fy, payments, &marks, &fmvs, &income,
    )?;
    Ok((s, report))
}

/// The advance-tax reminder due today, for the notifier: set within
/// [`tax::ADVANCE_REMINDER_DAYS`] days of a due date with something still
/// to pay, counting payments recorded with `tax advance --record`.
/// Capital gains only (the slab rate for dividends is not known here).
pub async fn advance_tax_reminder_today() -> Option<tax::AdvanceReminder> {
    let portfolio = pf::Portfolio::load().ok()?;
    if portfolio.realized.is_empty() {
        return None;
    }
    let payments = load_ledger().payments;
    let (s, _) = advance_schedule(&portfolio, None, &payments, None)
        .await
        .ok()?;
    let today = chrono::Local::now().date_naive();
    tax::advance_tax_reminder(&s, today, tax::ADVANCE_REMINDER_DAYS)
}

/// `tax advance`: the year's instalments under Sec. 208/211 with Sec. 234C,
/// the next instalment due and the interest if it is missed.
pub async fn cmd_tax_advance(
    fy: Option<&str>,
    paid: &[String],
    slab: Option<f64>,
    record: bool,
    json: bool,
) -> Result<()> {
    let given = paid
        .iter()
        .map(|p| tax::AdvancePayment::parse(p).map_err(|e| anyhow::anyhow!("--paid {p}: {e}")))
        .collect::<Result<Vec<_>>>()?;
    if let Some(pct) = slab
        && !(0.0..=50.0).contains(&pct)
    {
        bail!("--slab is your marginal rate in percent, 0 to 50");
    }
    if record {
        if given.is_empty() {
            bail!("--record saves the --paid payments: give at least one --paid DATE:AMOUNT");
        }
        if given.iter().any(|p| p.date.is_none()) {
            bail!("--record needs dated payments: --paid DATE:AMOUNT (2026-12-10:25000)");
        }
        let mut ledger = load_ledger();
        for p in &given {
            if !ledger.payments.contains(p) {
                ledger.payments.push(*p);
            }
        }
        ledger.payments.sort_by_key(|p| p.date);
        crate::store::save(ADVANCE_LEDGER, &ledger, true)?;
    }
    let payments = with_recorded(&given);
    let portfolio = pf::Portfolio::load()?;
    let (schedule, income) = advance_schedule(&portfolio, fy, &payments, slab).await?;
    let today = chrono::Local::now().date_naive();
    let next = tax::next_instalment(&schedule, today);

    if json {
        let mut v = serde_json::to_value(&schedule)?;
        v["next_instalment"] = serde_json::to_value(&next)?;
        v["slab_rate_pct"] = slab.into();
        v["disclaimer"] = ESTIMATE_NOTE.into();
        return output::print_json(&v);
    }

    super::planning::print_advance_tax(&schedule, &payments);
    if let Some(r) = &income {
        println!(
            "  • Dividends and interest included at your {}% slab: {} income, {} tax with cess (each in the quarter received).",
            slab.unwrap_or(0.0),
            inr(r.gross),
            inr(schedule.other_income_tax)
        );
    } else {
        println!(
            "  • {}",
            "Dividends and interest are left out: pass --slab 30 (your rate) to add them in the quarter received."
                .dimmed()
        );
    }
    if record {
        println!(
            "  • {}",
            format!(
                "Recorded {} payment(s); later runs and the reminder count them.",
                given.len()
            )
            .green()
        );
    }

    if let Some(n) = next {
        print_section(&format!(
            "Next instalment — {} due {}",
            n.quarter,
            n.due_date.format("%d %b %Y")
        ));
        let when = match n.days_left {
            0 => "today".to_string(),
            1 => "tomorrow".to_string(),
            d => format!("in {d} days"),
        };
        print_kv("Due", &when);
        print_kv(
            &format!("Cumulative by {}", n.due_date.format("%d %b")),
            &inr(n.required_by_due),
        );
        print_kv("Paid so far", &inr(n.paid));
        let due = if n.amount_due > 0.5 {
            inr(n.amount_due).yellow().bold().to_string()
        } else {
            "nothing — covered".green().to_string()
        };
        print_kv("To pay", &due);
        if n.interest_if_missed > 0.0 {
            print_kv(
                "Sec. 234C interest if missed",
                &inr(n.interest_if_missed).red().to_string(),
            );
        }
        if !schedule.sec_208_applicable {
            println!(
                "  {}",
                "Tax this year is under ₹10,000 so far: advance tax is not required (Sec. 208)."
                    .dimmed()
            );
        }
    }
    println!("\n  {}\n", ESTIMATE_NOTE.yellow().italic());
    Ok(())
}
