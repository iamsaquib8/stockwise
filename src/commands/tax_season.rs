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

/// Paise as `₹1,234.50`.
fn rp(paise: i64) -> String {
    inr(tax::paise::to_rupees(paise))
}

/// `--slab` and `--withholding` are percentages from 0 to 50.
fn check_pct(flag: &str, pct: f64) -> Result<()> {
    if !(0.0..=50.0).contains(&pct) {
        bail!("{flag} is a rate in percent, 0 to 50");
    }
    Ok(())
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

/// Rupees per unit of other currencies, by day: the ECB reference rates
/// of [`crate::fx_history`] (cached in `cache.db`). A day without a rate
/// takes the nearest earlier one, up to ten days back.
#[derive(Debug, Default, Clone)]
pub(crate) struct RupeeRates {
    hist: crate::fx_history::FxHistory,
}

impl RupeeRates {
    /// Load the rates each `(currency, earliest day)` needs, up to today.
    /// A currency the ECB does not publish, or a network failure with
    /// nothing cached, leaves it without rates (callers name what they left
    /// out).
    pub(crate) async fn load(needs: &[(String, NaiveDate)]) -> Self {
        Self {
            hist: crate::fx_history::FxHistory::load("INR", needs).await,
        }
    }

    /// Rupees per unit of `currency`'s major unit on `day` (pence and other
    /// minor units are scaled by the caller).
    pub(crate) fn rate(&self, currency: &str, day: NaiveDate) -> Option<f64> {
        let (_, major) = crate::market::to_major(1.0, currency);
        self.hist.rate_on(&major, day)
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
        // Crypto is never in a stock broker's tax P&L (Schedule VDA).
        .filter(|s| crate::market::AssetClass::of(&s.symbol) != crate::market::AssetClass::Crypto)
        .filter(|s| match (account, s.account_key()) {
            (Some(want), Some(have)) => want.eq_ignore_ascii_case(have),
            _ => true,
        })
        .collect();
    let mut symbols: Vec<String> = sales.iter().map(|s| s.symbol.clone()).collect();
    symbols.sort();
    symbols.dedup();
    let isins = super::planning::lookup_isins(&symbols).await;
    let marks = super::planning::sale_marks(&views::LiveSource::new(), &symbols).await;
    let old = tax_export::grandfathered_symbols(&portfolio.realized, start);
    let fmvs = super::planning::fmv_31_jan_2018(&old).await;
    sales
        .into_iter()
        .filter_map(|s| {
            let (b, d) = (date_of(&s.buy_date)?, date_of(&s.sell_date)?);
            let intraday = s.is_intraday();
            // Debt funds, gold ETFs, SGBs: their own holding periods (Sec. 50AA, 112).
            let long = !intraday
                && match crate::offmarket::india_asset(&s.symbol, marks.mark(&s.symbol)) {
                    Some(asset) => tax::india_rule(asset, b, d) == tax::IndiaRule::LongTerm112,
                    None => Country::India.is_long_term(b, d),
                };
            // Sec. 112A grandfathering is for listed shares and
            // equity-oriented funds only; other assets keep their cost.
            let equity = matches!(
                crate::offmarket::india_class(&s.symbol, marks.mark(&s.symbol)),
                crate::offmarket::IndiaClass::Listed | crate::offmarket::IndiaClass::EquityFund
            );
            let fmv = fmvs
                .iter()
                .find(|(f, _)| equity && *f == s.symbol)
                .map(|(_, v)| *v);
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
                charges: s.charges(),
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
    fmt: Format,
) -> Result<()> {
    let bytes = std::fs::read(file).with_context(|| format!("Cannot read {}", file.display()))?;
    let name = file
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let parsed = tax_reconcile::parse_file(&bytes, &name)?;
    let (rows, skipped) = (parsed.rows, parsed.skipped);
    let broker = broker.unwrap_or(parsed.broker);
    // The year: as given, else the one most of the file's sales are in (the
    // earliest on a tie), counted in one pass.
    let start = match fy {
        Some(_) => india_fy(fy)?,
        None => {
            let mut counts: std::collections::BTreeMap<NaiveDate, usize> = Default::default();
            for r in &rows {
                *counts
                    .entry(Country::India.tax_year(r.sell_date).0)
                    .or_default() += 1;
            }
            let most = counts.values().copied().max().unwrap_or(0);
            counts
                .into_iter()
                .find(|(_, n)| *n == most)
                .map(|(y, _)| y)
                .context("no sales in the file")?
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

    match fmt {
        Format::Json => {
            let mut v = serde_json::to_value(&r)?;
            v["agreeing"] = r.agreeing().into();
            v["outside_year"] = outside.into();
            v["skipped_rows"] = skipped.clone().into();
            v["account"] = account.into();
            v["disclaimer"] = ESTIMATE_NOTE.into();
            return output::print_json(&v);
        }
        Format::Csv => {
            if outside > 0 {
                eprintln!("  note: {outside} row(s) outside {fy_label} excluded.");
            }
            if !skipped.is_empty() {
                eprintln!(
                    "  warning: {} broker row(s) skipped: {}",
                    skipped.len(),
                    skipped.join("; ")
                );
            }
            print!("{}", reconcile_csv(&r));
            return Ok(());
        }
        Format::Table => {}
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
    for line in &skipped {
        println!("  {} Not compared — {line}", "!".yellow());
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

/// `tax reconcile --csv`: one line per matched sale and per sale on one
/// side only.
fn reconcile_csv(r: &tax_reconcile::Reconciliation) -> String {
    let m2 = |x: f64| format!("{:.2}", x + 0.0);
    let date = |d: Option<NaiveDate>| d.map(|d| d.to_string()).unwrap_or_default();
    let mut out = output::csv_line(&[
        "status",
        "symbol",
        "sell_date",
        "buy_date_stockwise",
        "buy_date_broker",
        "qty_stockwise",
        "qty_broker",
        "gain_stockwise",
        "gain_broker",
        "difference",
        "term_stockwise",
        "term_broker",
        "reasons",
    ]);
    for m in &r.matched {
        let reasons: Vec<&str> = m.reasons.iter().map(|x| x.label()).collect();
        out.push_str(&output::csv_line(&[
            if m.agrees() { "agrees" } else { "differs" }.to_string(),
            m.symbol.clone(),
            m.sell_date.to_string(),
            date(m.buy_date_ours),
            date(m.buy_date_theirs),
            m.qty_ours.to_string(),
            m.qty_theirs.to_string(),
            m2(m.gain_ours),
            m2(m.gain_theirs),
            m2(m.difference),
            m.term_ours.label().to_string(),
            m.term_theirs.map(|t| t.label()).unwrap_or("").to_string(),
            reasons.join("; "),
        ]));
    }
    for o in &r.only_ours {
        out.push_str(&output::csv_line(&[
            "only_stockwise".to_string(),
            tax_reconcile::bare_symbol(&o.symbol),
            o.sell_date.to_string(),
            o.buy_date.to_string(),
            String::new(),
            o.qty.to_string(),
            String::new(),
            m2(o.gain),
            String::new(),
            m2(o.gain),
            o.term.label().to_string(),
            String::new(),
            String::new(),
        ]));
    }
    for t in &r.only_theirs {
        out.push_str(&output::csv_line(&[
            "only_broker".to_string(),
            t.symbol.clone(),
            t.sell_date.to_string(),
            String::new(),
            date(t.buy_date),
            String::new(),
            t.qty.to_string(),
            String::new(),
            m2(t.gain()),
            m2(-t.gain()),
            String::new(),
            t.term.map(|x| x.label()).unwrap_or("").to_string(),
            String::new(),
        ]));
    }
    out
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

async fn income_for(
    portfolio: &pf::Portfolio,
    start: NaiveDate,
    withholding: Option<f64>,
) -> tax_income::IncomeReport {
    let rates = RupeeRates::load(&dividend_currencies(portfolio, start)).await;
    tax_income::income_report(portfolio, start, withholding, |c, d| rates.rate(c, d))
}

/// Say what the income figures leave out.
fn print_income_omissions(r: &tax_income::IncomeReport) {
    if !r.no_rate.is_empty() {
        println!(
            "  {} Left out (no rupee rate): {}. The totals are incomplete.",
            "!".yellow(),
            r.no_rate.join(", ")
        );
    }
    if !r.unknown_sgbs.is_empty() {
        println!(
            "  {} SGB tranche not known, no interest counted: {}. The totals are incomplete.",
            "!".yellow(),
            r.unknown_sgbs.join(", ")
        );
    }
    if !r.withholding_unknown.is_empty() {
        println!(
            "  {} Tax withheld abroad not known for {}: pass --withholding 25 (or the rate on your statement).",
            "!".yellow(),
            r.withholding_unknown.join(", ")
        );
    }
}

/// `tax income`: dividends and SGB interest of a financial year with TDS,
/// by Sec. 234C period, for ITR Schedule OS.
pub async fn cmd_tax_income(
    fy: Option<&str>,
    fmt: Format,
    out: Option<&Path>,
    withholding: Option<f64>,
) -> Result<()> {
    if let Some(w) = withholding {
        check_pct("--withholding", w)?;
    }
    let start = india_fy(fy)?;
    let portfolio = pf::Portfolio::load()?;
    let r = income_for(&portfolio, start, withholding.map(|w| w / 100.0)).await;
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
            let tds = match row.foreign_tax {
                _ if row.kind != tax_income::IncomeKind::ForeignDividend => rp(row.tds),
                Some(t) => format!("{} WHT", rp(t)),
                None => "WHT ?".to_string(),
            };
            println!(
                "  {:<11} {:<26} {:<17} {:<16} {:>12} {:>10} {:>12}",
                row.date.format("%Y-%m-%d"),
                row.payer.cyan(),
                row.kind.label(),
                tax_income::QUARTER_LABELS[row.quarter],
                rp(row.gross),
                tds,
                rp(row.net)
            );
        }
        print_section("Totals");
        print_kv("Dividends (Indian)", &rp(r.dividends));
        if r.foreign_dividends > 0 {
            print_kv("Dividends (foreign)", &rp(r.foreign_dividends));
        }
        if r.interest > 0 {
            print_kv("SGB interest", &rp(r.interest));
        }
        print_kv("Gross income", &rp(r.gross));
        print_kv("TDS (estimate)", &rp(r.tds));
        if r.foreign_tax > 0 {
            print_kv("Tax withheld abroad", &rp(r.foreign_tax));
        }
        print_kv("Net received", &rp(r.net));

        print_section("Quarterly breakup (Schedule OS, Sec. 234C periods)");
        for (i, label) in tax_income::QUARTER_LABELS.iter().enumerate() {
            println!(
                "  {:<18} {:>12} dividends {:>12} all income",
                label,
                rp(r.dividend_by_quarter[i]),
                rp(r.income_by_quarter[i])
            );
        }
    }
    println!();
    println!(
        "  {} Dividends are taxed at your slab rate. TDS (estimate) is 10% of a company's or fund's dividends once they pass {} in the year (Sec. 194/194K; ₹5,000 up to FY 2024-25); SGB interest has no TDS.",
        "·".dimmed(),
        inr(r.tds_threshold)
    );
    println!(
        "  {} Dividends are dated by ex-date (the payment date is not recorded), so one paid after a period's end can sit in the earlier period: check the payment dates. Match TDS with Form 26AS / AIS.",
        "·".dimmed()
    );
    if r.foreign_dividends > 0 {
        println!(
            "  {} Foreign dividends: claim the tax withheld abroad with Form 67 — see `stockwise tax foreign`.",
            "→".cyan()
        );
    }
    print_income_omissions(&r);
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
/// starting `start`, with the prices and rates they need. `slab` and
/// `withholding` are fractions.
async fn foreign_reports(
    portfolio: &pf::Portfolio,
    year: i32,
    start: NaiveDate,
    slab: f64,
    withholding: Option<f64>,
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
    let year_start = NaiveDate::from_ymd_opt(year, 1, 1).expect("year checked by the caller");
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
    needs.extend(dividend_currencies(portfolio, start.min(year_start)));
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
    let f67 = tax_foreign::form67(portfolio, start, slab, withholding, |c, d| rates.rate(c, d));
    (fa, f67)
}

/// The calendar year and financial year `tax foreign` reports: `--year`
/// names the calendar year (Schedule FA), whose financial year starts in
/// April of it; `--fy` names the financial year, whose Schedule FA covers
/// the calendar year it starts in. A year whose financial year has not
/// started is refused rather than paired with another one.
fn foreign_years(fy: Option<&str>, year: Option<i32>) -> Result<(i32, NaiveDate)> {
    match year {
        Some(y) => {
            let today = chrono::Local::now().date_naive();
            if !(1999..=today.year()).contains(&y) {
                bail!(
                    "--year {y}: give a calendar year from 1999 to {}",
                    today.year()
                );
            }
            let start = NaiveDate::from_ymd_opt(y, 4, 1).context("Invalid year")?;
            if start > today {
                bail!(
                    "FY {y}-{:02} (for Form 67) has not started yet: use --fy {}-{:02} for the current year",
                    (y + 1) % 100,
                    y - 1,
                    y % 100
                );
            }
            Ok((y, start))
        }
        None => {
            let start = india_fy(fy)?;
            Ok((start.year(), start))
        }
    }
}

/// Where Form 67 goes, in words (Rule 128(9)).
const FORM67_DEADLINE: &str = "Form 67 for a return filed under Sec. 139(1) or 139(4) is due by the end of the assessment year \
(Rule 128(9)); for an updated return (Sec. 139(8A)), by the day that return is filed. File it with or before the return \
to avoid a mismatch. Rates: last day of the month before the tax was withheld (Rule 128).";

/// `tax foreign`: Schedule FA (foreign assets, calendar year) and Form 67
/// (foreign tax credit) inputs. `slab` and `withholding` are percentages.
pub async fn cmd_tax_foreign(
    fy: Option<&str>,
    year: Option<i32>,
    slab: f64,
    withholding: Option<f64>,
    json: bool,
) -> Result<()> {
    check_pct("--slab", slab)?;
    if let Some(w) = withholding {
        check_pct("--withholding", w)?;
    }
    let (year, start) = foreign_years(fy, year)?;
    let portfolio = pf::Portfolio::load()?;
    let (fa, f67) = foreign_reports(
        &portfolio,
        year,
        start,
        slab / 100.0,
        withholding.map(|w| w / 100.0),
    )
    .await;
    let today = chrono::Local::now().date_naive();
    let in_progress = today.year() <= year;
    if json {
        return output::print_json(&serde_json::json!({
            "schedule_fa": fa,
            "schedule_fa_incomplete": fa.is_incomplete(),
            "form_67": f67,
            "form_67_incomplete": f67.is_incomplete(),
            "calendar_year_in_progress": in_progress,
            "rate_source": "ECB reference rate (Frankfurter); the rules ask for the SBI TT buying rate",
            "dates": "dividends are dated by ex-date (payment and withholding dates are not recorded)",
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
                rp(r.initial_value),
                rp(r.peak_value),
                rp(r.closing_value),
                rp(r.dividends),
                rp(r.sale_proceeds)
            );
            if !r.missing_rates.is_empty() {
                println!(
                    "  {:<10} {}",
                    "",
                    format!(
                        "no rupee rate for some days: {} incomplete",
                        r.missing_rates.join(", ")
                    )
                    .yellow()
                );
            }
        }
        println!(
            "  {:<10} {:<11} {:>9} {:>13} {:>13} {:>13} {:>11} {:>13}",
            "Total".bold(),
            "",
            "",
            rp(fa.initial_value),
            rp(fa.peak_value),
            rp(fa.closing_value),
            rp(fa.dividends),
            rp(fa.sale_proceeds)
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
            println!(
                "  {} Left out ({what}): {}. The totals are incomplete.",
                "!".yellow(),
                list.join(", ")
            );
        }
    }

    print_section(&format!(
        "Form 67 — foreign tax credit, {} (slab {}%)",
        f67.fy_label, slab
    ));
    if f67.rows.is_empty() && f67.no_rate.is_empty() {
        println!(
            "  {}",
            "No foreign dividends recorded for this year.".dimmed()
        );
    } else {
        if !f67.rows.is_empty() {
            println!(
                "  {:<11} {:<10} {:>12} {:>10} {:>12} {:>12} {:>12}",
                "Ex-date".bold(),
                "Payer".bold(),
                "Gross".bold(),
                "Withheld".bold(),
                "Gross ₹".bold(),
                "Withheld ₹".bold(),
                "Credit ₹".bold()
            );
            let unknown = || "?".to_string();
            for r in &f67.rows {
                println!(
                    "  {:<11} {:<10} {:>12} {:>10} {:>12} {:>12} {:>12}",
                    r.date.format("%Y-%m-%d"),
                    r.payer.cyan(),
                    format!("{:.2} {}", tax::paise::to_rupees(r.gross), r.currency),
                    r.tax_withheld
                        .map(|t| format!("{:.2}", tax::paise::to_rupees(t)))
                        .unwrap_or_else(unknown),
                    rp(r.gross_inr),
                    r.tax_withheld_inr.map(rp).unwrap_or_else(unknown),
                    r.credit_inr.map(rp).unwrap_or_else(unknown)
                );
            }
            print_kv("Foreign tax credit (estimate)", &rp(f67.credit_inr));
        }
        if !f67.no_rate.is_empty() {
            println!(
                "  {} Left out (no rupee rate for the Rule 128 date): {}. The form is incomplete.",
                "!".yellow(),
                f67.no_rate.join(", ")
            );
        }
        if f67.withholding_rate.is_none() && !f67.rows.is_empty() {
            println!(
                "  {} The tax withheld abroad is not recorded: pass --withholding 25 (or the rate on your broker's statement) for the credit.",
                "!".yellow()
            );
        }
        println!("  {}", FORM67_DEADLINE.dimmed());
    }
    println!(
        "\n  {} Rupee values use the ECB reference rate for the day; the rules ask for the SBI TT buying rate, which differs slightly. Dividends are dated by ex-date.",
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
/// on stdout or into `out`. `slab` and `withholding` are percentages.
pub async fn cmd_tax_export_schedule(
    schedule: ExportSchedule,
    fy: Option<&str>,
    slab: f64,
    withholding: Option<f64>,
    out: Option<&Path>,
    json: bool,
) -> Result<()> {
    check_pct("--slab", slab)?;
    if let Some(w) = withholding {
        check_pct("--withholding", w)?;
    }
    let withholding = withholding.map(|w| w / 100.0);
    let start = india_fy(fy)?;
    let portfolio = pf::Portfolio::load()?;
    let (body, what, incomplete) = match schedule {
        ExportSchedule::Cg => bail!("Schedule CG is `tax export in` without --schedule"),
        ExportSchedule::Os => {
            let r = income_for(&portfolio, start, withholding).await;
            let body = if json {
                serde_json::to_string_pretty(&r)? + "\n"
            } else {
                tax_income::schedule_os_csv(&r)
            };
            (
                body,
                format!("Schedule OS {}: {} rows", r.fy_label, r.rows.len()),
                r.is_incomplete(),
            )
        }
        ExportSchedule::Fa | ExportSchedule::Form67 => {
            let (fa, f67) =
                foreign_reports(&portfolio, start.year(), start, slab / 100.0, withholding).await;
            if schedule == ExportSchedule::Fa {
                let body = if json {
                    serde_json::to_string_pretty(&fa)? + "\n"
                } else {
                    tax_foreign::schedule_fa_csv(&fa)
                };
                (
                    body,
                    format!("Schedule FA {}: {} rows", fa.calendar_year, fa.rows.len()),
                    fa.is_incomplete(),
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
                    f67.is_incomplete(),
                )
            }
        }
    };
    if incomplete {
        eprintln!(
            "  {} Incomplete: some income or holdings were left out or lack a rate (listed at the end of the file).",
            "!".yellow()
        );
    }
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

    let schedule =
        tax_export::india_schedule_with_fmv(&portfolio.realized, start, isin, &marks, fmv);
    let mut files: Vec<(String, String)> = rows
        .iter()
        .map(|r| (r.file.clone(), r.workpaper_csv(&year)))
        .collect();
    files.push((
        "summary.csv".into(),
        tax_export::audit_summary_csv(&rows, &year),
    ));
    files.push(("schedule-cg.csv".into(), tax_export::india_csv(&schedule)));
    publish_pack(dir, &files)?;

    print_header(&portfolio_title_tax(&format!("Audit Pack — {year}")));
    print_kv("Sales", &rows.len().to_string());
    let mut rules: Vec<&str> = rows.iter().map(|r| r.rule.as_str()).collect();
    rules.sort();
    rules.dedup();
    for rule in rules {
        let gain: i64 = rows.iter().filter(|r| r.rule == rule).map(|r| r.gain).sum();
        println!("  {:<64} {:>14}", rule, rp(gain));
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

/// Whether `dir` holds only an earlier audit pack (its own file names and
/// a `summary.csv` that says so), so a new run may replace it.
fn is_audit_pack(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    let ours = |n: &str| {
        n == "summary.csv"
            || n == "schedule-cg.csv"
            || (n.starts_with("sale-") && n.ends_with(".csv"))
    };
    let mut names = Vec::new();
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !ours(&name) || !e.path().is_file() {
            return false;
        }
        names.push(name);
    }
    names.iter().any(|n| n == "summary.csv")
        && std::fs::read_to_string(dir.join("summary.csv"))
            .is_ok_and(|t| t.starts_with("StockWise audit pack"))
}

/// Write `files` as the audit pack `dir`, all or nothing: into a staging
/// folder beside it first, then moved into place. An earlier pack in `dir`
/// is replaced whole (no workpaper of a removed sale is left behind); a
/// folder holding anything else is refused. A failed write leaves `dir` as
/// it was and removes the staging folder.
pub(crate) fn publish_pack(dir: &Path, files: &[(String, String)]) -> Result<()> {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .context("The audit pack needs a folder name")?;
    let parent = match dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => std::path::PathBuf::from("."),
    };
    let exists = dir.exists();
    let empty = exists && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none());
    if exists && !empty && !is_audit_pack(dir) {
        bail!(
            "{} is not empty and is not an earlier audit pack: choose a new folder",
            dir.display()
        );
    }
    std::fs::create_dir_all(&parent)
        .with_context(|| format!("Cannot create {}", parent.display()))?;
    let pid = std::process::id();
    let staging = parent.join(format!(".{name}.staging-{pid}"));
    let _ = std::fs::remove_dir_all(&staging);
    let written = (|| -> Result<()> {
        std::fs::create_dir(&staging)
            .with_context(|| format!("Cannot create {}", staging.display()))?;
        for (file, body) in files {
            let path = staging.join(file);
            std::fs::write(&path, body)
                .with_context(|| format!("Cannot write {}", path.display()))?;
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }
    let old = parent.join(format!(".{name}.previous-{pid}"));
    if exists && let Err(e) = std::fs::rename(dir, &old) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e).with_context(|| format!("Cannot replace {}", dir.display()));
    }
    if let Err(e) = std::fs::rename(&staging, dir) {
        // Put the earlier pack back.
        let mut rollback_note = String::new();
        if exists && let Err(re) = std::fs::rename(&old, dir) {
            rollback_note = format!(
                "; rollback failed ({re}), previous pack preserved at {}",
                old.display()
            );
        }
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e).with_context(|| format!("Cannot write {}{rollback_note}", dir.display()));
    }
    if exists {
        let _ = std::fs::remove_dir_all(&old);
    }
    Ok(())
}

// ── tax in: gains outside Sec. 111A/112A ──

/// One sale under Sec. 50AA, 112 or 115BBH in `tax in` (gain in paise,
/// shown as rupees).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct OtherGainRow {
    pub(crate) symbol: String,
    pub(crate) sold: String,
    #[serde(serialize_with = "tax::paise::serialize")]
    pub(crate) gain: i64,
    pub(crate) rule: tax::IndiaRule,
    pub(crate) rule_label: &'static str,
}

/// The year's sales `tax in` leaves out of Sec. 111A/112A that have a rule
/// of their own (debt and other funds, gold and international ETFs, SGBs,
/// market-linked debentures, crypto), with their totals. A same-day crypto
/// sale is here too: Sec. 115BBH applies to every crypto gain (`tax in`
/// never counts crypto as intraday income).
pub(crate) fn other_gains(
    portfolio: &pf::Portfolio,
    mark: impl Fn(&str) -> Option<Mark>,
    today: NaiveDate,
) -> (Vec<OtherGainRow>, tax::OtherGains) {
    use tax::paise;
    let (from, to) = Country::India.tax_year(today);
    let mut rows = Vec::new();
    let mut gains = Vec::new();
    for s in &portfolio.realized {
        let (Some(b), Some(d)) = (date_of(&s.buy_date), date_of(&s.sell_date)) else {
            continue;
        };
        if d < from || d > to || pf::symbol_currency(&s.symbol) != Some("INR") {
            continue;
        }
        let m = mark(&s.symbol);
        let Some(asset) = crate::offmarket::india_asset(&s.symbol, m.as_ref()) else {
            continue;
        };
        if s.is_intraday() && asset != tax::IndiaAsset::Vda {
            continue;
        }
        let rule = tax::india_rule(asset, b, d);
        // Before charges, like the rest of `tax` (Sec. 115BBH allows no
        // deduction but the cost anyway).
        let gain =
            paise::from_rupees(s.qty * s.sell_price) - paise::from_rupees(s.qty * s.buy_price);
        gains.push(tax::OtherGain {
            rule,
            gain,
            proceeds: paise::from_rupees(s.qty * s.sell_price),
            before_rate_change: d < tax::india_rate_change_date(),
        });
        rows.push(OtherGainRow {
            symbol: s.symbol.clone(),
            sold: s.sell_date.clone(),
            gain,
            rule,
            rule_label: rule.label_on(d),
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
            rp(r.gain),
            r.rule_label.dimmed()
        );
    }
    if o.slab_income() > 0 {
        println!(
            "  • Taxed at your slab rate (Sec. 50AA {} + other short-term {}): add {} to your income.",
            rp(o.sec_50aa),
            rp(o.short_term_slab),
            rp(o.slab_income())
        );
    }
    if o.st_loss_for_set_off > 0 {
        println!(
            "  • Net short-term loss {} (Sec. 50AA and other short-term) is set off against the listed-share gains above.",
            rp(o.st_loss_for_set_off)
        );
    }
    if o.long_term_112 > 0 {
        println!(
            "  • Sec. 112 long-term (no ₹1.25 lakh exemption, no indexation): {} — tax {}",
            rp(o.long_term_112),
            rp(o.long_term_112_tax)
        );
    }
    if o.lt_loss_for_set_off > 0 {
        println!(
            "  • Net Sec. 112 loss {} is set off against the listed-share long-term gains above.",
            rp(o.lt_loss_for_set_off)
        );
    }
    if o.vda_gains > 0 || o.vda_losses_ignored > 0 {
        println!(
            "  • Crypto (Sec. 115BBH, 30% flat): gains {} — tax {}; losses {} cannot be set off or carried forward. TDS of 1% (Sec. 194S) on sales: about {}.",
            rp(o.vda_gains),
            rp(o.vda_tax),
            rp(o.vda_losses_ignored),
            rp(o.vda_tds)
        );
    }
    if o.flat_tax() > 0 {
        println!(
            "  • Sec. 112 + 115BBH tax with 4% cess: {}",
            rp(o.flat_tax())
        );
    }
    println!(
        "  {}",
        "Estimate limits: gains here are not set off against losses on listed shares, and positive Sec. 112 gains are not offset by Sec. 112A losses."
            .dimmed()
    );
}

// ── tax advance ──

/// One advance-tax payment recorded with `tax advance --paid DATE:AMOUNT
/// --record`, for the financial year it was paid for.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, serde::Deserialize)]
pub(crate) struct RecordedPayment {
    #[serde(flatten)]
    pub(crate) payment: tax::AdvancePayment,
    /// The first calendar year of the financial year it is for (2026 = FY
    /// 2026-27): `--fy` when recorded, else the year its date falls in.
    /// Entries saved without one are assigned by their date.
    #[serde(default)]
    pub(crate) fy: Option<i32>,
}

impl RecordedPayment {
    /// The financial year (its first calendar year) this payment counts for.
    fn year(&self) -> Option<i32> {
        self.fy.or_else(|| {
            self.payment
                .date
                .map(|d| Country::India.tax_year(d).0.year())
        })
    }
}

/// Advance-tax payments recorded with `--record`, kept in the data
/// directory so later runs (and the reminder) count them.
#[derive(Debug, Default, Serialize, serde::Deserialize)]
pub(crate) struct AdvanceLedger {
    #[serde(default)]
    pub(crate) payments: Vec<RecordedPayment>,
}

impl AdvanceLedger {
    /// The recorded payments for the financial year starting in `year`.
    fn for_year(&self, year: i32) -> Vec<tax::AdvancePayment> {
        self.payments
            .iter()
            .filter(|p| p.year() == Some(year))
            .map(|p| p.payment)
            .collect()
    }

    /// Add `payments` for the financial year starting in `year`, skipping
    /// ones already recorded for it. Returns how many were new.
    fn record(&mut self, payments: &[tax::AdvancePayment], year: i32) -> usize {
        let mut added = 0;
        for p in payments {
            let entry = RecordedPayment {
                payment: *p,
                fy: Some(year),
            };
            if !self
                .payments
                .iter()
                .any(|x| x.payment == *p && x.year() == Some(year))
            {
                self.payments.push(entry);
                added += 1;
            }
        }
        self.payments.sort_by_key(|p| (p.year(), p.payment.date));
        added
    }
}

const ADVANCE_LEDGER: &str = "advance_tax.json";

/// The ledger: empty when the file does not exist; an unreadable or
/// corrupt file is an error, so it is never overwritten with less.
fn load_ledger() -> Result<AdvanceLedger> {
    crate::store::load(ADVANCE_LEDGER).with_context(|| {
        format!("Cannot read {ADVANCE_LEDGER} in the data directory (left as it is)")
    })
}

/// Record `payments` for the financial year starting in `year`, holding the
/// data-file lock across the read, merge and save (owner-only file).
fn record_payments(payments: &[tax::AdvancePayment], year: i32) -> Result<(usize, AdvanceLedger)> {
    let path = crate::store::path(ADVANCE_LEDGER)?;
    let _lock = crate::store::lock(&path)?;
    let mut ledger: AdvanceLedger = crate::store::load_from(&path)
        .with_context(|| format!("Cannot read {} (left as it is)", path.display()))?;
    let added = ledger.record(payments, year);
    crate::store::save_to(&path, &ledger, true)?;
    Ok((added, ledger))
}

/// The schedule `tax advance` and the reminder work from: capital gains,
/// plus dividends and interest at `slab` (a percentage) when given.
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
            let r = income_for(portfolio, start, None).await;
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
/// to pay, counting this year's payments recorded with `tax advance
/// --record`. Capital gains only: dividends and SGB interest are left out
/// (their slab rate is not known here), so a portfolio with income but no
/// sales gets no reminder.
pub async fn advance_tax_reminder_today() -> Option<tax::AdvanceReminder> {
    let portfolio = pf::Portfolio::load().ok()?;
    if portfolio.realized.is_empty() {
        return None;
    }
    let today = chrono::Local::now().date_naive();
    let year = Country::India.tax_year(today).0.year();
    let payments = load_ledger().ok()?.for_year(year);
    let (s, _) = advance_schedule(&portfolio, None, &payments, None)
        .await
        .ok()?;
    tax::advance_tax_reminder(&s, today, tax::ADVANCE_REMINDER_DAYS)
}

/// `tax advance`: the year's instalments under Sec. 208/211 with Sec. 234C,
/// the next instalment due and the interest if it is missed.
///
/// Payments: the ones recorded for this financial year with `--record`,
/// plus the `--paid` ones of this run (which count for the year asked for,
/// as given, and are saved only with `--record`).
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
    if let Some(pct) = slab {
        check_pct("--slab", pct)?;
    }
    if record {
        if given.is_empty() {
            bail!("--record saves the --paid payments: give at least one --paid DATE:AMOUNT");
        }
        if given.iter().any(|p| p.date.is_none()) {
            bail!("--record needs dated payments: --paid DATE:AMOUNT (2026-12-10:25000)");
        }
    }
    // Everything that can fail before saving: the year, the portfolio and
    // the ledger itself.
    let start = india_fy(fy)?;
    let year = start.year();
    let portfolio = pf::Portfolio::load()?;
    let (ledger, added) = if record {
        let (added, committed) = record_payments(&given, year)?;
        eprintln!(
            "  {}",
            format!(
                "Recorded {added} payment(s) for {} in {ADVANCE_LEDGER}.",
                Country::India.tax_year_label(start)
            )
            .green()
        );
        (committed, Some(added))
    } else {
        (load_ledger()?, None)
    };
    let mut payments = ledger.for_year(year);
    for p in &given {
        if !payments.contains(p) {
            payments.push(*p);
        }
    }
    let (schedule, income) = advance_schedule(&portfolio, fy, &payments, slab)
        .await
        .with_context(|| {
            if added.is_some() {
                "the --paid payments were recorded; the schedule could not be worked out"
                    .to_string()
            } else {
                "the schedule could not be worked out".to_string()
            }
        })?;
    let today = chrono::Local::now().date_naive();
    let next = tax::next_instalment(&schedule, today);

    if json {
        let mut v = serde_json::to_value(&schedule)?;
        v["next_instalment"] = serde_json::to_value(&next)?;
        v["slab_rate_pct"] = slab.into();
        if let Some(r) = &income {
            v["income_incomplete"] = r.is_incomplete().into();
            v["income_omitted"] = serde_json::json!({
                "no_rupee_rate": r.no_rate,
                "unknown_sgb_tranche": r.unknown_sgbs,
            });
        }
        v["disclaimer"] = ESTIMATE_NOTE.into();
        return output::print_json(&v);
    }

    super::planning::print_advance_tax(&schedule, &payments);
    if let Some(r) = &income {
        println!(
            "  • Dividends and interest included at your {}% slab: {} income, {} tax with cess (dividends in the quarter of their ex-date, interest in every quarter).",
            slab.unwrap_or(0.0),
            rp(r.gross),
            inr(schedule.other_income_tax)
        );
        if r.is_incomplete() {
            print_income_omissions(r);
        }
    } else {
        println!(
            "  • {}",
            "Dividends and interest are left out: pass --slab 30 (your rate) to add them.".dimmed()
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
        if schedule.sec_208_applicable {
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
        } else {
            println!(
                "  {}",
                "Tax this year is under ₹10,000 so far: advance tax is not required (Sec. 208), so nothing is payable."
                    .dimmed()
            );
        }
    }
    println!("\n  {}\n", ESTIMATE_NOTE.yellow().italic());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn recorded_payments_count_only_for_their_financial_year() {
        let pay = |date: &str, amount: f64| tax::AdvancePayment {
            date: Some(d(date)),
            amount,
        };
        let mut ledger = AdvanceLedger::default();
        assert_eq!(ledger.record(&[pay("2026-06-14", 1_000.0)], 2026), 1);
        // A late payment deliberately for FY 2025-26.
        assert_eq!(ledger.record(&[pay("2026-04-10", 500.0)], 2025), 1);
        // The same payment again: not added twice.
        assert_eq!(ledger.record(&[pay("2026-06-14", 1_000.0)], 2026), 0);
        assert_eq!(ledger.for_year(2026), vec![pay("2026-06-14", 1_000.0)]);
        assert_eq!(ledger.for_year(2025), vec![pay("2026-04-10", 500.0)]);
        // An entry saved without a year goes by its date.
        let legacy: AdvanceLedger =
            serde_json::from_str(r#"{"payments":[{"date":"2026-12-10","amount":300.0}]}"#).unwrap();
        assert_eq!(legacy.for_year(2026).len(), 1);
        assert!(legacy.for_year(2025).is_empty());
        // Round trip keeps the year.
        let text = serde_json::to_string(&ledger).unwrap();
        let back: AdvanceLedger = serde_json::from_str(&text).unwrap();
        assert_eq!(back.for_year(2025), vec![pay("2026-04-10", 500.0)]);
    }

    #[test]
    fn an_audit_pack_is_published_whole_and_replaces_only_an_earlier_pack() {
        let base = std::env::temp_dir().join(format!("sw-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let dir = base.join("pack");
        let summary = (
            "summary.csv".to_string(),
            "StockWise audit pack,FY\n".to_string(),
        );
        let first = vec![
            ("sale-001-A-2026-05-01.csv".to_string(), "a".to_string()),
            ("sale-002-B-2026-06-01.csv".to_string(), "b".to_string()),
            summary.clone(),
        ];
        publish_pack(&dir, &first).unwrap();
        // A rerun with one sale fewer leaves no stale workpaper behind.
        let second = vec![
            ("sale-001-A-2026-05-01.csv".to_string(), "a".to_string()),
            summary.clone(),
        ];
        publish_pack(&dir, &second).unwrap();
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(names, vec!["sale-001-A-2026-05-01.csv", "summary.csv"]);
        // A folder with other files is refused and left alone.
        let other = base.join("mine");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("notes.txt"), "keep").unwrap();
        assert!(publish_pack(&other, &second).is_err());
        assert_eq!(
            std::fs::read_to_string(other.join("notes.txt")).unwrap(),
            "keep"
        );
        // No staging folder is left over.
        let leftovers = std::fs::read_dir(&base)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with('.')
            })
            .count();
        assert_eq!(leftovers, 0);
        let _ = std::fs::remove_dir_all(&base);
    }
}
