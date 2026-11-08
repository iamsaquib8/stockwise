//! Planning and reporting: `tax export` (capital-gains schedules from the
//! realized lots), `portfolio benchmark` (time-weighted return against an
//! index, with attribution by holding and sector) and `goal` (SIP,
//! retirement and education calculators). The math lives in `tax_export.rs`,
//! `benchmark.rs` and `goals.rs`.

use super::portfolio::{fx_pair, market_currency, portfolio_title, signed_money};
use super::*;
use crate::api::{ChartData, ChartResponse};
use crate::benchmark::{self, Analysis, HoldingDays};
use crate::goals::{self, SipPlan};
use crate::i18n::{Lang, tf_in};
use crate::portfolio::{self as pf, Split};
use crate::store;
use crate::tax::{self, Country};
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
                if m.taxable_stcg_before_23_jul_2024 > 0.0
                    || m.taxable_ltcg_before_23_jul_2024 > 0.0
                {
                    print_kv(
                        "  sales before 23 Jul 2024",
                        &format!(
                            "STCG {} at 15%, LTCG {} at 10%",
                            inr(m.taxable_stcg_before_23_jul_2024),
                            inr(m.taxable_ltcg_before_23_jul_2024)
                        ),
                    );
                }
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
            let marks = sale_marks(&views::LiveSource::new(), &sold).await;
            let old = tax_export::grandfathered_symbols(&portfolio.realized, start);
            let fmvs = fmv_31_jan_2018(&old).await;
            Schedule::India(tax_export::india_schedule_with_fmv(
                &portfolio.realized,
                start,
                |sym| isins.iter().find(|(s, _)| s == sym).map(|(_, i)| i.clone()),
                &marks,
                |sym| fmvs.iter().find(|(s, _)| s == sym).map(|(_, v)| *v),
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

/// The 31 Jan 2018 price of each symbol (Section 112A grandfathering): the
/// day's highest price for a listed share (Yahoo), the NAV for a mutual fund
/// (AMFI via mfapi). A price never changes, so it is cached for good; a
/// symbol without one is left out and keeps its actual cost.
pub(crate) async fn fmv_31_jan_2018(symbols: &[String]) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    for sym in symbols {
        let cache = format!(
            "fmv2018_{}.txt",
            sym.replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        );
        let forever = std::time::Duration::from_secs(u64::MAX / 4);
        let valid = |t: &str| t.trim().parse::<f64>().is_ok_and(|v| v > 0.0);
        if let Some(page) = crate::pagecache::read(&cache, forever, valid)
            && let Ok(v) = page.text.trim().parse::<f64>()
        {
            out.push((sym.clone(), v));
            continue;
        }
        let price = if let Some(id) = crate::offmarket::fund_id(sym) {
            fund_nav_31_jan_2018(id).await
        } else {
            let (from, to) = tax_export::fmv_window();
            match views::DataSource::yahoo(&views::LiveSource::new()).await {
                Ok(y) => y
                    .get_chart_between(sym, from, to, "1d")
                    .await
                    .ok()
                    .map(|c| views::history_from_chart(c, sym, "", "1d"))
                    .and_then(|h| tax_export::fmv_from_bars(&h.bars)),
                Err(_) => None,
            }
        };
        if let Some(v) = price {
            crate::pagecache::write(&cache, &v.to_string());
            out.push((sym.clone(), v));
        }
    }
    out
}

/// The NAV of AMFI scheme `id` on 31 Jan 2018 (the last one on or before it).
/// None for an ISIN id or when the history cannot be read.
async fn fund_nav_31_jan_2018(id: &str) -> Option<f64> {
    let code: u32 = id.parse().ok()?;
    let client = crate::pagecache::client(std::time::Duration::from_secs(20)).ok()?;
    let hist = crate::funds::load_history(&client, code).await.ok()?;
    let day = NaiveDate::from_ymd_opt(2018, 1, 31)?;
    crate::funds::nav_on_or_before(&hist.navs, day)
        .filter(|(d, _)| (day - *d).num_days() <= 7)
        .map(|(_, nav)| nav)
}

/// The 31 Jan 2018 prices `advance_tax_schedule` needs: one per rupee symbol
/// sold from a lot bought on or before that day, whatever the tax year.
pub(crate) async fn advance_fmvs(portfolio: &Portfolio) -> Vec<(String, f64)> {
    let mut symbols: Vec<String> = portfolio
        .realized
        .iter()
        .filter(|s| {
            tax_export::is_grandfathered_date(&s.buy_date)
                && pf::symbol_currency(&s.symbol) == Some("INR")
                && !s.symbol.to_uppercase().ends_with("-INR")
        })
        .map(|s| s.symbol.clone())
        .collect();
    symbols.sort();
    symbols.dedup();
    fmv_31_jan_2018(&symbols).await
}

/// AMFI marks (categories) for the funds and SGBs among `symbols`, so the tax
/// commands that have no valuation can tell equity-oriented funds from the
/// rest. Never fails: a symbol without a mark is classified as unknown.
pub(crate) async fn sale_marks(
    src: &impl views::DataSource,
    symbols: &[String],
) -> crate::offmarket::OffMarket {
    let mut off: Vec<String> = symbols
        .iter()
        .filter(|s| crate::offmarket::is_off_market(s))
        .cloned()
        .collect();
    off.sort();
    off.dedup();
    if off.is_empty() {
        return crate::offmarket::OffMarket::default();
    }
    crate::offmarket::price(src, &off, &[]).await
}

/// Compute the Indian advance tax schedule for `portfolio`'s realized sales.
pub(crate) fn advance_tax_schedule(
    portfolio: &Portfolio,
    fy: Option<&str>,
    payments: &[tax::AdvancePayment],
    marks: &crate::offmarket::OffMarket,
    fmvs: &[(String, f64)],
) -> Result<tax::AdvanceTaxSchedule> {
    if payments.iter().any(|p| p.amount < 0.0) {
        bail!("Advance tax paid amount cannot be negative");
    }

    let today = chrono::Local::now().date_naive();
    let start = match fy {
        Some(label) => tax_export::parse_tax_year(Country::India, label).with_context(|| {
            format!(
                "Invalid financial year '{}': use a format like 2025-26 or AY2026-27",
                label
            )
        })?,
        None => Country::India.tax_year(today).0,
    };
    if start > today {
        bail!(
            "{} has not started yet",
            Country::India.tax_year_label(start)
        );
    }

    let (from, to) = Country::India.tax_year(start);

    let mut advance_sales = Vec::new();
    for s in &portfolio.realized {
        let Some(sell_d) = NaiveDate::parse_from_str(&s.sell_date, "%Y-%m-%d").ok() else {
            continue;
        };
        if sell_d < from || sell_d > to {
            continue;
        }
        let Some(scale) =
            pf::symbol_currency(&s.symbol).and_then(|c| Country::India.currency_scale(c))
        else {
            continue;
        };
        // Same-day sales are speculative income at slab rates; crypto, debt and
        // other funds, SGBs and gold, silver, liquid, debt and international
        // ETFs are not 111A/112A. Same rule as `tax in` and `tax export in`.
        if s.is_intraday()
            || !matches!(
                crate::offmarket::india_class(&s.symbol, marks.mark(&s.symbol)),
                crate::offmarket::IndiaClass::Listed | crate::offmarket::IndiaClass::EquityFund
            )
        {
            continue;
        }
        let is_lt = match NaiveDate::parse_from_str(&s.buy_date, "%Y-%m-%d").ok() {
            Some(buy_d) => Country::India.is_long_term(buy_d, sell_d),
            None => false,
        };
        // Long-term lots bought by 31 Jan 2018 use the grandfathered cost.
        let fmv = fmvs.iter().find(|(f, _)| *f == s.symbol).map(|(_, v)| *v);
        let unit_cost =
            tax_export::lot_unit_cost(&s.buy_date, is_lt, s.buy_price, s.sell_price, fmv);
        let gain = s.qty * (s.sell_price - unit_cost);
        advance_sales.push(tax::AdvanceTaxSale::new(sell_d, gain * scale, is_lt));
    }

    Ok(tax::compute_advance_tax(
        &advance_sales,
        start,
        today,
        payments,
    ))
}

/// Advance tax quarterly instalment estimates under Section 208 and 211 with
/// Section 234C(1) capital-gains proviso protections.
pub async fn cmd_tax_advance(fy: Option<&str>, paid: &[String], json: bool) -> Result<()> {
    let payments = paid
        .iter()
        .map(|p| tax::AdvancePayment::parse(p).map_err(|e| anyhow::anyhow!("--paid {p}: {e}")))
        .collect::<Result<Vec<_>>>()?;
    let portfolio = Portfolio::load()?;
    let year_symbols: Vec<String> = portfolio
        .realized
        .iter()
        .map(|s| s.symbol.clone())
        .collect();
    let marks = sale_marks(&views::LiveSource::new(), &year_symbols).await;
    let fmvs = advance_fmvs(&portfolio).await;
    let schedule = advance_tax_schedule(&portfolio, fy, &payments, &marks, &fmvs)?;

    if json {
        return output::print_json(&schedule);
    }

    let inr = |amount: f64| {
        if amount < 0.0 {
            format!("-₹{}", goals::indian_grouping(-amount))
        } else {
            format!("₹{}", goals::indian_grouping(amount))
        }
    };

    print_header(&portfolio_title(&format!(
        "Advance Tax Schedule — {}",
        schedule.fy_label
    )));
    println!(
        "  {}\n",
        "Statutory quarterly instalments under Section 208/211 with Section 234C(1) capital-gains proviso."
            .dimmed()
    );

    if schedule.total_stcg == 0.0 && schedule.total_ltcg == 0.0 {
        println!(
            "  {}\n",
            "No sales recorded for this financial year. Record one with `stockwise portfolio sell` or import a tradebook."
                .dimmed()
        );
    }

    println!(
        "  {:<7} {:<11} {:>12} {:>12} {:>12} {:>6} {:>13} {:>13}  {}",
        "Quarter".bold(),
        "Due Date".bold(),
        "Cum. STCG".bold(),
        "Cum. LTCG".bold(),
        "Cum. Tax".bold(),
        "Min %".bold(),
        "Min Cum. Due".bold(),
        "Quarter Due".bold(),
        "Status".bold(),
    );

    for q in &schedule.quarters {
        let status_str = match q.status {
            tax::AdvanceTaxStatus::Paid => "Paid".green(),
            tax::AdvanceTaxStatus::Pending => "Pending".yellow(),
            tax::AdvanceTaxStatus::Upcoming => "Upcoming".dimmed(),
            tax::AdvanceTaxStatus::Overdue => "Overdue".red().bold(),
            tax::AdvanceTaxStatus::NothingDue => "Nothing due".dimmed(),
        };
        println!(
            "  {:<7} {:<11} {:>12} {:>12} {:>12} {:>5}% {:>13} {:>13}  {}",
            q.quarter.cyan(),
            q.due_date.format("%d %b %Y"),
            inr(q.stcg_cumulative),
            inr(q.ltcg_cumulative),
            inr(q.cumulative_tax_estimated),
            q.required_cumulative_pct as i64,
            inr(q.min_cumulative_tax_required),
            inr(q.quarter_tax_payable),
            status_str,
        );
    }

    println!();
    print_section("Summary & Section 234 Compliance");
    println!(
        "  • Realized Capital Gains: {} STCG, {} LTCG",
        inr(schedule.total_stcg),
        inr(schedule.total_ltcg)
    );
    println!(
        "  • Total Estimated Tax: {}",
        inr(schedule.total_tax_estimated)
    );
    if schedule.sec_208_applicable {
        println!(
            "  • Section 208: {} (estimated tax ≥ ₹10,000 threshold)",
            "Applicable".green()
        );
    } else {
        println!(
            "  • Section 208: {} (estimated tax < ₹10,000 threshold — advance tax not mandatory)",
            "Exempt".cyan()
        );
    }
    println!(
        "  • Section 234C Proviso: Shortfalls from capital gains realized later in the year are not penalized."
    );

    if !payments.is_empty() {
        println!(
            "  • Advance Tax Paid Recorded: {}",
            inr(schedule.total_paid)
        );
        if schedule.ignored_before_year {
            println!(
                "  • {}",
                format!(
                    "A payment dated before {} was made for an earlier year and is not counted.",
                    schedule.fy_start.format("%d %b %Y")
                )
                .yellow()
            );
        }
        if schedule.assumed_on_time {
            println!(
                "  • {}",
                "A payment without a date is assumed to be paid before every due date. Use --paid DATE:AMOUNT for exact interest."
                    .yellow()
            );
        }
        for q in schedule
            .quarters
            .iter()
            .filter(|q| q.shortfall > 0.0 && q.sec_234c_interest > 0.0)
        {
            println!(
                "  • {}: {} paid by {} against {} due, shortfall {}, interest {}",
                q.quarter,
                inr(q.paid),
                q.due_date.format("%d %b %Y"),
                inr(q.min_cumulative_tax_required),
                inr(q.shortfall),
                inr(q.sec_234c_interest)
            );
        }
        if schedule.total_sec_234c_interest > 0.0 {
            println!(
                "  • Estimated Section 234C Deferment Interest: {}",
                inr(schedule.total_sec_234c_interest).red().bold()
            );
        } else {
            println!(
                "  • Estimated Section 234C Interest: {}",
                "₹0 (no shortfall subject to interest)".green()
            );
        }
        if schedule.sec_234b_applicable {
            println!(
                "  • Section 234B: {} (advance tax paid < 90% of assessed tax after 31 March — 1%/month interest applies)",
                "Applicable".red().bold()
            );
        }
    } else {
        println!(
            "  • Section 234B/C Interest: Pass {} to track shortfall and interest.",
            "`--paid [DATE:]AMOUNT`".cyan()
        );
    }

    println!("\n  {}\n", tax_export::DISCLAIMER.yellow().italic());
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

// ── Public Market Equivalent (PME) / Index Twin ──

#[derive(Debug, Clone, Serialize)]
pub struct PmeRow {
    pub symbol: String,
    pub actual_value: f64,
    pub twin_value: f64,
    pub value_added: f64,
    pub pme_ratio: Option<f64>,
    pub direct_alpha_percent: Option<f64>,
    pub actual_xirr_percent: Option<f64>,
    pub twin_xirr_percent: Option<f64>,
}

impl CsvRow for PmeRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "actual_value",
        "twin_value",
        "value_added",
        "pme_ratio",
        "direct_alpha_percent",
        "actual_xirr_percent",
        "twin_xirr_percent",
    ];
}

/// Compute the Public Market Equivalent (Index Twin) analysis for `portfolio`.
pub async fn portfolio_pme_report(
    src: &impl views::DataSource,
    portfolio: &Portfolio,
    bench: Option<&str>,
    market: Market,
    base: Option<&str>,
) -> Result<crate::pme::PmeReport> {
    let (bench_symbol, bench_name) = resolve_benchmark(bench, market);
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

    let today = chrono::Local::now().date_naive();
    if symbols.is_empty() {
        return Ok(crate::pme::PmeReport {
            summary: crate::pme::PmeSummary {
                benchmark_symbol: bench_symbol,
                benchmark_name: bench_name,
                base_currency: base,
                start_date: today.to_string(),
                end_date: today.to_string(),
                total_invested: 0.0,
                total_distributed: 0.0,
                actual_value: 0.0,
                twin_value: 0.0,
                value_added: 0.0,
                kaplan_schoar_pme: None,
                direct_alpha_percent: None,
                portfolio_xirr_percent: None,
                twin_xirr_percent: None,
                twin_shares: 0.0,
                benchmark_end_price: 0.0,
            },
            holdings: Vec::new(),
        });
    }

    let val = super::portfolio::value_holdings(src, &symbols, market, Some(&base)).await?;

    let mut all_flows: Vec<(NaiveDate, f64)> = Vec::new();
    let mut holding_flows: Vec<(String, Vec<(NaiveDate, f64)>)> = Vec::new();

    for sym in &symbols {
        let raw_flows = portfolio.cash_flows(sym);
        let mut base_flows = Vec::new();
        for (d, amt) in raw_flows {
            if let Some(converted) = val.to_base(sym, amt) {
                base_flows.push((d, converted));
                all_flows.push((d, converted));
            }
        }
        holding_flows.push((sym.clone(), base_flows));
    }
    all_flows.sort_by_key(|f| f.0);

    let start_date = match all_flows.first() {
        Some(f) => f.0,
        None => today,
    };

    let client = src.yahoo().await?;
    let bench_hist = daily_history(client, &bench_symbol, Span::Since(start_date))
        .await
        .with_context(|| format!("No price history for the benchmark {}", bench_symbol))?;
    if bench_hist.closes.len() < 2 {
        bail!(
            "Not enough price history for the benchmark {}",
            bench_symbol
        );
    }

    let calendar: Vec<NaiveDate> = bench_hist.closes.iter().map(|c| c.0).collect();
    let bench_currency = bench_hist.currency.clone();

    let mut bench_closes = bench_hist.closes.clone();
    if let Some(cur) = bench_currency.as_deref().filter(|c| *c != base) {
        if let Some(fx) = fx_series(client, cur, &base, &calendar).await {
            bench_closes = bench_closes
                .into_iter()
                .zip(fx)
                .filter_map(|((d, p), r)| Some((d, p * r?)))
                .collect();
        } else {
            bail!(
                "No {}→{} exchange-rate history to compare with {}",
                cur,
                base,
                bench_name
            );
        }
    }

    let actual_portfolio_value = val.totals(portfolio).value;
    let pme_res = crate::pme::compute_pme(&all_flows, actual_portfolio_value, &bench_closes, today)
        .context("Could not compute PME for portfolio")?;

    let (twin_shares, _) =
        crate::pme::twin_shares_and_value(&all_flows, &bench_closes, today).unwrap_or((0.0, 0.0));
    let bench_end_price = crate::pme::benchmark_price_on(&bench_closes, today).unwrap_or(0.0);

    let mut total_invested = 0.0;
    let mut total_distributed = 0.0;
    for &(_, amt) in &all_flows {
        if amt < 0.0 {
            total_invested += -amt;
        } else {
            total_distributed += amt;
        }
    }

    let summary = crate::pme::PmeSummary {
        benchmark_symbol: bench_symbol,
        benchmark_name: bench_name,
        base_currency: base,
        start_date: start_date.to_string(),
        end_date: today.to_string(),
        total_invested: round(total_invested, 2),
        total_distributed: round(total_distributed, 2),
        actual_value: round(actual_portfolio_value, 2),
        twin_value: round(pme_res.twin_value, 2),
        value_added: round(pme_res.value_added, 2),
        kaplan_schoar_pme: pme_res.pme_ratio.map(|r| round(r, 4)),
        direct_alpha_percent: pme_res.direct_alpha_percent.map(|a| round(a, 2)),
        portfolio_xirr_percent: pme_res.actual_xirr_percent.map(|x| round(x, 2)),
        twin_xirr_percent: pme_res.twin_xirr_percent.map(|x| round(x, 2)),
        twin_shares: round(twin_shares, 4),
        benchmark_end_price: round(bench_end_price, 2),
    };

    let mut holdings = Vec::new();
    for (sym, flows) in holding_flows {
        if flows.is_empty() {
            continue;
        }
        let h_val = portfolio.holding(&sym).map_or(0.0, |h| {
            val.price(&sym)
                .map_or(0.0, |p| val.to_base(&sym, p * h.shares).unwrap_or(0.0))
        });
        if let Some(res) = crate::pme::compute_pme(&flows, h_val, &bench_closes, today) {
            holdings.push(crate::pme::HoldingPme {
                symbol: sym,
                actual_value: round(res.actual_value, 2),
                twin_value: round(res.twin_value, 2),
                value_added: round(res.value_added, 2),
                pme_ratio: res.pme_ratio.map(|r| round(r, 4)),
                direct_alpha_percent: res.direct_alpha_percent.map(|a| round(a, 2)),
                actual_xirr_percent: res.actual_xirr_percent.map(|x| round(x, 2)),
                twin_xirr_percent: res.twin_xirr_percent.map(|x| round(x, 2)),
                cashflows_count: flows.len(),
            });
        }
    }
    holdings.sort_by(|a, b| b.value_added.total_cmp(&a.value_added));

    Ok(crate::pme::PmeReport { summary, holdings })
}

/// Run `stockwise portfolio pme`: mirror historical cash flows into a benchmark index.
pub async fn cmd_portfolio_pme(
    bench: Option<&str>,
    market: Market,
    base: Option<&str>,
    fmt: Format,
) -> Result<()> {
    let portfolio = Portfolio::load()?;
    let report =
        portfolio_pme_report(&views::LiveSource::new(), &portfolio, bench, market, base).await?;

    let s = &report.summary;
    let rows: Vec<PmeRow> = report
        .holdings
        .iter()
        .map(|h| PmeRow {
            symbol: h.symbol.clone(),
            actual_value: h.actual_value,
            twin_value: h.twin_value,
            value_added: h.value_added,
            pme_ratio: h.pme_ratio,
            direct_alpha_percent: h.direct_alpha_percent,
            actual_xirr_percent: h.actual_xirr_percent,
            twin_xirr_percent: h.twin_xirr_percent,
        })
        .collect();

    match fmt {
        Format::Json => return output::print_json(&report),
        Format::Csv => return output::print_csv(&rows),
        Format::Table => {}
    }

    if report.holdings.is_empty() && s.actual_value == 0.0 {
        println!("\n  {}", t(Msg::PortfolioEmpty).dimmed());
        return Ok(());
    }

    let base_cur = Some(s.base_currency.as_str());
    print_header(&portfolio_title(&format!(
        "Public Market Equivalent (Index Twin) vs {}",
        s.benchmark_name
    )));

    print_kv(
        "Benchmark",
        &format!("{} ({})", s.benchmark_name, s.benchmark_symbol),
    );
    print_kv("Period", &format!("{} → {}", s.start_date, s.end_date));
    print_kv("Base Currency", &s.base_currency);
    print_kv("Total Invested", &format_price(s.total_invested, base_cur));
    print_kv(
        "Total Distributed",
        &format_price(s.total_distributed, base_cur),
    );
    println!();
    print_kv(
        "Portfolio Value",
        &format_price(s.actual_value, base_cur).bold().to_string(),
    );
    print_kv("Index Twin Value", &format_price(s.twin_value, base_cur));
    let va_str = signed_money(s.value_added, base_cur);
    let va_styled = if s.value_added >= 0.0 {
        va_str.green().bold()
    } else {
        va_str.red().bold()
    };
    print_kv("Value Added / Alpha", &va_styled.to_string());
    println!();

    if let Some(pme) = s.kaplan_schoar_pme {
        let pme_str = format!("{:.4}", pme);
        let styled = if pme >= 1.0 {
            pme_str.green().bold()
        } else {
            pme_str.red().bold()
        };
        print_kv(
            "Kaplan-Schoar PME",
            &format!("{} (> 1.0000 = beat the index)", styled),
        );
    }
    if let Some(alpha) = s.direct_alpha_percent {
        let alpha_str = format!("{:+.2}% p.a.", alpha);
        let styled = if alpha >= 0.0 {
            alpha_str.green().bold()
        } else {
            alpha_str.red().bold()
        };
        print_kv("Direct Alpha", &styled.to_string());
    }
    if let Some(px) = s.portfolio_xirr_percent {
        let tx_str = s
            .twin_xirr_percent
            .map_or("N/A".into(), |tx| format!("{:.2}%", tx));
        print_kv(
            "Portfolio XIRR",
            &format!("{:.2}% (vs Twin XIRR {})", px, tx_str),
        );
    }

    if !report.holdings.is_empty() {
        print_section("Holdings vs Benchmark Twin");
        println!(
            "  {} {} {} {} {} {}",
            pad_end("Symbol", 14).bold(),
            pad_start("Actual Value", 14).bold(),
            pad_start("Twin Value", 14).bold(),
            pad_start("Value Added", 15).bold(),
            pad_start("KS-PME", 10).bold(),
            pad_start("Direct Alpha", 14).bold(),
        );
        for h in &report.holdings {
            let va = signed_money(h.value_added, base_cur);
            let va_cell = if h.value_added >= 0.0 {
                va.green()
            } else {
                va.red()
            };
            let pme_cell = h.pme_ratio.map_or("-".to_string(), |r| format!("{:.4}", r));
            let alpha_cell = h.direct_alpha_percent.map_or("-".to_string(), |a| {
                let s = format!("{:+.2}%", a);
                if a >= 0.0 {
                    s.green().to_string()
                } else {
                    s.red().to_string()
                }
            });
            println!(
                "  {} {} {} {} {} {}",
                pad_end(&h.symbol, 14).cyan(),
                pad_start(&format_price(h.actual_value, base_cur), 14),
                pad_start(&format_price(h.twin_value, base_cur), 14),
                pad_start(&va_cell.to_string(), 15),
                pad_start(&pme_cell, 10),
                pad_start(&alpha_cell, 14),
            );
        }
    }
    println!();
    Ok(())
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

/// "1 year", "18 months", "2 years 6 months".
fn duration_label(months: u32) -> String {
    if months == 0 {
        return tf(Msg::DurMonths, &[&0]);
    }
    let (y, m) = goals::duration_parts(months);
    let mut parts = Vec::new();
    match y {
        0 => {}
        1 => parts.push(t(Msg::DurYear1).to_string()),
        n => parts.push(tf(Msg::DurYears, &[&n])),
    }
    match m {
        0 => {}
        1 => parts.push(t(Msg::DurMonth1).to_string()),
        n => parts.push(tf(Msg::DurMonths, &[&n])),
    }
    parts.join(" ")
}

/// Equity and arbitrage tax rules exist for India and the US only.
pub(crate) fn check_tax_region(tax: Option<goals::TaxKind>, market: Market) -> Result<()> {
    if goal_region(market) == goals::Region::Other
        && matches!(
            tax,
            Some(goals::TaxKind::Equity | goals::TaxKind::Arbitrage)
        )
    {
        bail!(
            "--tax equity and arbitrage have rules for India (-m in) and the US (-m us) only; use --tax debt with --slab, or --tax none"
        );
    }
    Ok(())
}

pub(crate) fn goal_region(market: Market) -> goals::Region {
    match market {
        Market::In => goals::Region::India,
        Market::Us => goals::Region::Us,
        _ => goals::Region::Other,
    }
}

/// `goal <kind> --json`: the plan rounded to paise/cents, with its kind and
/// currency.
pub(crate) fn goal_json(
    kind: &str,
    currency: &str,
    plan: &impl Serialize,
) -> Result<serde_json::Value> {
    let mut v = serde_json::to_value(plan)?;
    round_floats(&mut v, 2);
    if let Some(o) = v.as_object_mut() {
        o.insert("goal".into(), serde_json::json!(kind));
        o.insert("currency".into(), serde_json::json!(currency));
    }
    Ok(v)
}

fn print_goal_json(kind: &str, currency: &str, plan: &impl Serialize) -> Result<()> {
    output::print_json(&goal_json(kind, currency, plan)?)
}

/// The report for a SIP request: region and the cached T-bill yield filled
/// in, and the English warnings `--json` carries.
pub(crate) fn goal_sip_report(mut req: goals::SipRequest, market: Market) -> goals::SipReport {
    req.region = goal_region(market);
    if req.region == goals::Region::India && req.tbill_pct.is_none() {
        req.tbill_pct = crate::gsec::cached_tbill_364();
    }
    let mut report = goals::report_sip(&req);
    let currency = market_currency(market);
    if let Some(w) = mismatch_text(&report, currency, Lang::En) {
        report.warnings.push(w);
    }
    report
}

fn mismatch_text(r: &goals::SipReport, currency: &str, lang: Lang) -> Option<String> {
    let w = r.mismatch.as_ref()?;
    let m = |x: f64| goal_money(x, currency);
    Some(tf_in(
        lang,
        Msg::GoalWarnMismatch,
        &[
            &format!("{}", (w.return_pct * 100.0).round() / 100.0),
            &m(w.p10),
            &format!("{:.0}", w.shortfall_prob_pct),
            &format!("{}", (w.safe_return_pct * 100.0).round() / 100.0),
            &m(w.safe_monthly_sip),
        ],
    ))
}

fn per_month(x: f64, currency: &str) -> String {
    tf(Msg::GoalPerMonth, &[&goal_money(x, currency)])
}

/// The SIP part every goal shares: savings, return, what to invest and the
/// totals.
fn print_sip_core(p: &SipPlan, currency: &str) {
    let m = |x: f64| goal_money(x, currency);
    if p.savings > 0.0 {
        print_kv(t(Msg::GoalSavings), &m(p.savings));
        print_kv(t(Msg::GoalSavingsGrowTo), &m(p.savings_future_value));
    }
    print_kv(t(Msg::GoalExpectedReturn), &pct_label(p.annual_return_pct));
    if p.annual_return_pct > 0.0 {
        print_kv(t(Msg::GoalEffective), &pct_label(p.effective_annual_pct));
    }
    if p.monthly_sip <= 0.0 {
        println!("\n  {}", t(Msg::GoalCovered).green());
        return;
    }
    print_kv(
        t(Msg::GoalMonthlySip),
        &per_month(p.monthly_sip, currency)
            .bold()
            .green()
            .to_string(),
    );
    if p.step_up_pct > 0.0 {
        print_kv(t(Msg::GoalStepUp), &pct_label(p.step_up_pct));
        print_kv(
            t(Msg::GoalFinalSip),
            &per_month(p.final_monthly_sip, currency),
        );
    }
    print_kv(t(Msg::GoalTotalInvested), &m(p.total_invested));
    print_kv(t(Msg::GoalWealthGained), &m(p.gains));
    print_kv(t(Msg::GoalLumpSum), &m(p.lump_sum_today));
}

/// Month by month up to 24 months, quarter by quarter up to 36, then year by
/// year (every fifth year above 20).
fn print_schedule(p: &SipPlan, currency: &str) {
    if p.monthly_sip <= 0.0 {
        return;
    }
    let m = |x: f64| goal_money(x, currency);
    if p.months <= 36 {
        print_section(t(if p.months <= 24 {
            Msg::GoalMonthByMonth
        } else {
            Msg::GoalQuarterly
        }));
        println!(
            "  {} {} {} {} {}",
            pad_start(t(Msg::GoalMonth), 6).bold(),
            pad_start(t(Msg::GoalSipColumn), 18).bold(),
            pad_start(t(Msg::GoalInvested), 14).bold(),
            pad_start(t(Msg::Value), 14).bold(),
            pad_start(t(Msg::GoalLongLots), 10).bold(),
        );
        let step = if p.months <= 24 { 1 } else { 3 };
        for r in p
            .periods
            .iter()
            .filter(|r| r.month % step == 0 || r.month == p.months)
        {
            println!(
                "  {} {} {} {} {}",
                pad_start(&r.month.to_string(), 6),
                pad_start(&m(r.monthly_sip), 18),
                pad_start(&m(r.invested), 14),
                pad_start(&m(r.value), 14).cyan(),
                pad_start(&r.lots_long_term.to_string(), 10),
            );
        }
        return;
    }
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

fn band_text(b: &goals::Band, currency: &str) -> String {
    format!(
        "{} / {} / {}",
        goal_money(b.p10, currency),
        goal_money(b.p50, currency),
        goal_money(b.p90, currency)
    )
}

/// A full SIP report: plan, return basis, tax, bands, sensitivity, schedule.
fn print_sip_report(r: &goals::SipReport, currency: &str) {
    let m = |x: f64| goal_money(x, currency);
    let p = &r.plan;
    if r.target_today.is_some() {
        print_kv(t(Msg::GoalInflation), &pct_label(r.inflation_pct));
        print_kv(
            t(Msg::GoalTargetToday),
            &m(r.target_today.unwrap_or(p.target)),
        );
        print_kv(
            t(Msg::GoalTargetFuture),
            &m(p.target).bold().yellow().to_string(),
        );
    }
    let basis = if r.regime.source == "user" {
        pct_label(r.regime.return_pct)
    } else {
        format!("{} — {}", r.regime.name, pct_label(r.regime.return_pct))
    };
    print_kv(t(Msg::GoalRegime), &basis);
    if let Some(g) = &r.glide {
        print_kv(
            t(Msg::GoalGlide),
            &tf(Msg::GoalGlideValue, &[&g.to_pct, &duration_label(g.months)]),
        );
    }
    print_sip_core(p, currency);
    if p.monthly_sip <= 0.0 {
        println!();
        return;
    }
    if let Some(tax) = &r.tax {
        print_section(&tf(Msg::GoalTaxLine, &[&tax.kind.label()]));
        let gross = p.periods.last().map_or(0.0, |x| x.value);
        print_kv(t(Msg::GoalGrossCorpus), &m(gross));
        print_kv(
            t(Msg::GoalTaxAndLoad),
            &format!("{} + {}", m(tax.tax), m(tax.exit_load)),
        );
        print_kv(
            t(Msg::GoalNetInHand),
            &m(r.net_after_tax.unwrap_or(gross))
                .bold()
                .green()
                .to_string(),
        );
        print_kv(
            t(Msg::GoalLotsLine),
            &format!("{} / {}", tax.short_term_lots, tax.long_term_lots),
        );
        if let Some(net_sip) = r.monthly_sip_for_net_target {
            print_kv(t(Msg::GoalPostTaxSip), &per_month(net_sip, currency));
        }
        if !r.tax_compare.is_empty() {
            let parts: Vec<String> = r.tax_compare.iter().map(|c| m(c.net_after_tax)).collect();
            print_kv(t(Msg::GoalCompare), &parts.join(" vs "));
        }
    }
    print_section(t(Msg::GoalBands));
    let bands = &r.percentiles;
    print_kv(t(Msg::Value), &band_text(&bands.gross, currency));
    if let Some(n) = &bands.net {
        print_kv(t(Msg::GoalBandsNet), &band_text(n, currency));
    }
    print_kv(
        t(Msg::GoalSuccess),
        &format!("{:.0}%", bands.success_prob_pct),
    );
    let s = &r.sensitivity;
    if s.len() == 3 {
        print_kv(
            &tf(
                Msg::GoalSensitivity,
                &[&s[0].return_pct, &s[1].return_pct, &s[2].return_pct],
            ),
            &format!(
                "{} / {} / {}",
                m(s[0].monthly_sip),
                m(s[1].monthly_sip),
                m(s[2].monthly_sip)
            ),
        );
    }
    if let Some(real) = r.real_value_today {
        print_kv(&tf(Msg::GoalPurchasing, &[&r.real_value_cpi_pct]), &m(real));
    }
    if let Some(w) = mismatch_text(r, currency, crate::i18n::lang()) {
        println!("\n  {} {}", "!".yellow(), w.yellow());
    }
    print_schedule(p, currency);
}

fn print_assumptions() {
    println!("\n  {}\n", t(Msg::GoalAssumptions).dimmed().italic());
}

/// `goal sip`: the monthly SIP that reaches `target`, tax-aware and matched
/// to the horizon.
pub fn cmd_goal_sip(req: goals::SipRequest, market: Market, json: bool) -> Result<()> {
    let currency = market_currency(market);
    let report = goal_sip_report(req, market);
    if json {
        return print_goal_json("sip", currency, &report);
    }
    print_header(&tf(
        Msg::GoalSipTitle,
        &[
            &goal_money(report.target_today.unwrap_or(report.plan.target), currency),
            &duration_label(report.plan.months),
        ],
    ));
    print_sip_report(&report, currency);
    print_assumptions();
    Ok(())
}

pub struct RetireArgs {
    pub monthly_expense: f64,
    pub years_to_retire: u32,
    pub years_in_retirement: u32,
    pub inflation_pct: f64,
    pub return_pre_pct: f64,
    pub return_post_pct: f64,
    pub step_up_pct: f64,
    pub savings: f64,
    pub monthly_income: f64,
    pub buffer_years: u32,
    pub legacy: f64,
}

pub(crate) fn retire_plan(a: &RetireArgs, market: Market) -> goals::RetirePlan {
    goals::plan_retirement_with(
        a.monthly_expense,
        a.years_to_retire,
        a.years_in_retirement,
        a.inflation_pct,
        a.return_pre_pct,
        a.return_post_pct,
        a.step_up_pct,
        a.savings,
        &goals::RetireOpts {
            monthly_income: a.monthly_income,
            buffer_years: a.buffer_years,
            legacy: a.legacy,
            region: goal_region(market),
            slab_pct: goal_region(market).default_slab_pct(),
        },
    )
}

/// `goal retire`: the corpus that pays today's monthly expense, grown by
/// inflation, through retirement, and the SIP that builds it.
pub fn cmd_goal_retire(a: RetireArgs, market: Market, json: bool) -> Result<()> {
    let currency = market_currency(market);
    let plan = retire_plan(&a, market);
    if json {
        return print_goal_json("retire", currency, &plan);
    }
    let m = |x: f64| goal_money(x, currency);
    print_header(&tf(
        Msg::GoalRetireTitle,
        &[&duration_label(a.years_to_retire * 12)],
    ));
    print_kv(
        t(Msg::GoalExpenseToday),
        &per_month(a.monthly_expense, currency),
    );
    if a.monthly_income > 0.0 {
        print_kv(
            t(Msg::GoalIncomeOffset),
            &per_month(a.monthly_income, currency),
        );
    }
    print_kv(t(Msg::GoalInflation), &pct_label(a.inflation_pct));
    print_kv(
        t(Msg::GoalExpenseAtRetirement),
        &per_month(plan.monthly_expense_at_retirement, currency),
    );
    if a.monthly_income > 0.0 {
        print_kv(
            t(Msg::GoalNeedAtRetirement),
            &per_month(plan.monthly_need_at_retirement, currency),
        );
    }
    print_kv(
        t(Msg::GoalYearsInRetirement),
        &a.years_in_retirement.to_string(),
    );
    if a.buffer_years > 0 {
        print_kv(t(Msg::GoalBufferYears), &a.buffer_years.to_string());
    }
    if a.legacy > 0.0 {
        print_kv(t(Msg::GoalLegacy), &m(a.legacy));
    }
    print_kv(t(Msg::GoalReturnAfter), &pct_label(a.return_post_pct));
    print_kv(
        t(Msg::GoalCorpusNeeded),
        &m(plan.corpus).bold().yellow().to_string(),
    );
    print_kv(
        t(Msg::GoalWithdrawalRate),
        &format!("{:.2}%", plan.initial_withdrawal_rate_pct),
    );
    print_kv(
        t(Msg::GoalSurvives),
        &duration_label((plan.survival_years * 12.0).round() as u32),
    );
    print_kv(
        t(Msg::GoalCrashSurvives),
        &duration_label((plan.crash_survival_years * 12.0).round() as u32),
    );
    if let Some(d) = &plan.swp_drag {
        print_kv(
            t(Msg::GoalSwpDrag),
            &format!("{} vs {}", m(d.swp_tax), m(d.fd_tax)),
        );
    }
    print_section(t(Msg::GoalReturnBefore));
    print_sip_core(&plan.sip, currency);
    print_schedule(&plan.sip, currency);
    print_assumptions();
    Ok(())
}

/// `goal education`: today's cost grown by education inflation, and the SIP
/// that pays it.
pub fn cmd_goal_education(
    cost: f64,
    inflation_pct: f64,
    req: goals::SipRequest,
    market: Market,
    json: bool,
) -> Result<()> {
    let currency = market_currency(market);
    let plan = education_plan(cost, inflation_pct, req, market);
    if json {
        return print_goal_json("education", currency, &plan);
    }
    let m = |x: f64| goal_money(x, currency);
    let label = duration_label(plan.months);
    print_header(&tf(Msg::GoalEducationTitle, &[&label]));
    print_kv(t(Msg::GoalCostToday), &m(cost));
    print_kv(t(Msg::GoalInflation), &pct_label(inflation_pct));
    print_kv(
        &tf(Msg::GoalFutureCost, &[&label]),
        &m(plan.future_cost).bold().yellow().to_string(),
    );
    print_sip_report(&plan.sip, currency);
    print_assumptions();
    Ok(())
}

pub(crate) fn education_plan(
    cost: f64,
    inflation_pct: f64,
    mut req: goals::SipRequest,
    market: Market,
) -> goals::EducationPlan {
    req.region = goal_region(market);
    if req.region == goals::Region::India && req.tbill_pct.is_none() {
        req.tbill_pct = crate::gsec::cached_tbill_364();
    }
    let mut plan = goals::plan_education(cost, req.months, inflation_pct, req);
    if let Some(w) = mismatch_text(&plan.sip, market_currency(market), Lang::En) {
        plan.sip.warnings.push(w);
    }
    plan
}

// ── Linked holdings ──

/// Whether `symbol` (`NIFTYBEES`, `MF:122639`) names the holding.
fn names_holding(h: &pf::Holding, symbol: &str) -> bool {
    let (have, want) = (h.symbol.to_uppercase(), symbol.trim().to_uppercase());
    have == want
        || have
            .strip_prefix(&want)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// What linked holdings are worth, and what they cost.
pub struct Linked {
    pub value: f64,
    pub cost: f64,
    /// Prices were live; otherwise `value` is the cost basis.
    pub live: bool,
}

/// The portfolio's holdings named in `symbols` (all when empty), valued at
/// live prices when `src` is there, else at cost.
///
/// With `strict`, a named holding missing from the portfolio is an error
/// (explicit `--holdings`); saved goals skip holdings that were removed.
/// Values are in the market's currency; offline, holdings in another
/// currency are left out and named.
pub(crate) async fn linked_value(
    src: Option<&impl views::DataSource>,
    symbols: &[String],
    market: Market,
    strict: bool,
) -> Result<Linked> {
    let portfolio = Portfolio::load()?;
    if strict && portfolio.holdings.is_empty() {
        bail!("The portfolio is empty: add holdings with `stockwise portfolio add`");
    }
    let mut chosen = portfolio.clone();
    if !symbols.is_empty() {
        if strict {
            for s in symbols {
                if !portfolio.holdings.iter().any(|h| names_holding(h, s)) {
                    bail!("No holding named `{s}` in the portfolio");
                }
            }
        }
        chosen
            .holdings
            .retain(|h| symbols.iter().any(|s| names_holding(h, s)));
    }
    if chosen.holdings.is_empty() {
        return Ok(Linked {
            value: 0.0,
            cost: 0.0,
            live: true,
        });
    }
    let names: Vec<String> = chosen.holdings.iter().map(|h| h.symbol.clone()).collect();
    let base = market_currency(market);
    let (val, live) = value_or_offline(src, &names, market, Some(base)).await;
    if !live {
        let (same, other): (Vec<_>, Vec<_>) = chosen
            .holdings
            .iter()
            .partition(|h| pf::symbol_currency(&h.symbol).is_none_or(|c| c == base));
        if !other.is_empty() {
            eprintln!(
                "  ! Left out of the linked value (not in {base}, no exchange rate offline): {}",
                other
                    .iter()
                    .map(|h| h.symbol.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let cost: f64 = same.iter().map(|h| h.cost_basis()).sum();
        return Ok(Linked {
            value: cost,
            cost,
            live: false,
        });
    }
    let totals = val.totals(&chosen);
    if !totals.unpriced.is_empty() || !totals.unconverted.is_empty() {
        eprintln!(
            "  ! Left out of the linked value (no price or exchange rate): {}",
            [totals.unpriced.clone(), totals.unconverted.clone()]
                .concat()
                .join(", ")
        );
    }
    Ok(Linked {
        value: totals.value,
        cost: totals.cost,
        live: true,
    })
}

/// Savings from `--from-portfolio` / `--holdings`, when either is given.
pub async fn goal_linked(
    from_portfolio: bool,
    holdings: &[String],
    market: Market,
) -> Result<Option<Linked>> {
    if !from_portfolio && holdings.is_empty() {
        return Ok(None);
    }
    let client = YahooClient::new().await.ok();
    let linked = linked_value(client.as_ref(), holdings, market, true).await?;
    if !linked.live {
        eprintln!("  ! {}", t(Msg::GoalLinkedOffline));
    }
    Ok(Some(linked))
}

// ── Saved goals ──

const GOALS_FILE: &str = "goals.json";

#[derive(Serialize)]
pub(crate) struct GoalRow {
    name: String,
    target: f64,
    target_future: f64,
    by: String,
    months_left: u32,
    holdings: Vec<String>,
    linked_value: f64,
    /// `market` (live prices), `cost` (offline) or `none` (no holdings linked).
    valued_at: &'static str,
    funded_pct: f64,
    monthly_sip: f64,
    status: &'static str,
}

fn goal_due(by: &str) -> Option<NaiveDate> {
    let mut parts = by.trim().split('-');
    let y = parts.next()?.parse().ok()?;
    let mo = parts.next()?.parse().ok()?;
    let d = parts.next().map_or(Some(1), |d| d.parse().ok())?;
    NaiveDate::from_ymd_opt(y, mo, d)
}

/// The target on the due date. With inflation the saved target is in the
/// money of the day the goal was created, so it grows from then, not from
/// today.
fn saved_target_future(g: &goals::SavedGoal, due: Option<NaiveDate>, months_left: u32) -> f64 {
    let Some(infl) = g.inflation_pct.filter(|i| *i > 0.0) else {
        return g.target;
    };
    let from_creation = goal_due(&g.created)
        .zip(due)
        .and_then(|(created, due)| goals::months_between(created, due));
    goals::inflate_months(g.target, infl, from_creation.unwrap_or(months_left))
}

/// Saved goals are in the currency they were saved in.
fn check_goal_currency(g: &goals::SavedGoal, market: Market) -> Result<()> {
    match &g.currency {
        Some(c) if c != market_currency(market) => bail!(
            "Goal `{}` is in {c}: use the market that matches (e.g. -m {})",
            g.name,
            if c == "INR" { "in" } else { "us" }
        ),
        _ => Ok(()),
    }
}

fn saved_request(
    g: &goals::SavedGoal,
    months: u32,
    savings: f64,
    market: Market,
) -> goals::SipRequest {
    let due = goal_due(&g.by);
    let mut req = goals::SipRequest::new(
        saved_target_future(g, due, months),
        months,
        goal_region(market),
    );
    req.return_pct = g.return_pct;
    req.savings = savings;
    req
}

/// Every saved goal with its live linked value, funded share and the SIP
/// still needed.
pub(crate) async fn goal_rows(
    src: Option<&impl views::DataSource>,
    market: Market,
) -> Result<Vec<GoalRow>> {
    let store: goals::GoalStore = store::load(GOALS_FILE)?;
    let today = chrono::Local::now().date_naive();
    let mut rows = Vec::new();
    for g in &store.goals {
        if let Err(e) = check_goal_currency(g, market) {
            eprintln!("  ! Skipped: {e}");
            continue;
        }
        let due = goal_due(&g.by);
        let months_left = due
            .and_then(|d| goals::months_between(today, d))
            .unwrap_or(0);
        let (linked, valued_at) = if g.holdings.is_empty() {
            (0.0, "none")
        } else {
            let l = linked_value(src, &g.holdings, market, false).await?;
            (l.value, if l.live { "market" } else { "cost" })
        };
        let target_future = saved_target_future(g, due, months_left);
        let region = goal_region(market);
        let ret = g
            .return_pct
            .unwrap_or(goals::baseline(months_left.max(1), region, None).return_pct);
        let sip = if months_left == 0 {
            (target_future - linked).max(0.0)
        } else {
            goals::plan_sip_months(target_future, months_left, ret, 0.0, linked, None).monthly_sip
        };
        let status = if sip <= 0.0 {
            "funded"
        } else if months_left == 0 && due.is_none_or(|d| d <= today) {
            "overdue"
        } else {
            "needs_sip"
        };
        rows.push(GoalRow {
            name: g.name.clone(),
            target: g.target,
            target_future,
            by: g.by.clone(),
            months_left,
            holdings: g.holdings.clone(),
            linked_value: linked,
            valued_at,
            funded_pct: if target_future > 0.0 {
                linked / target_future * 100.0
            } else {
                0.0
            },
            monthly_sip: sip,
            status,
        });
    }
    Ok(rows)
}

pub struct GoalAdd {
    pub currency: String,
    pub name: String,
    pub target: f64,
    pub by: String,
    pub inflation_pct: Option<f64>,
    pub holdings: Vec<String>,
    pub return_pct: Option<f64>,
}

pub fn cmd_goal_add(a: GoalAdd) -> Result<()> {
    let today = chrono::Local::now().date_naive();
    goals::months_until(today, &a.by).map_err(anyhow::Error::msg)?;
    if a.name.trim().is_empty() {
        bail!("The goal needs a name");
    }
    let saved = goals::SavedGoal {
        name: a.name.trim().to_string(),
        target: a.target,
        by: a.by.trim().to_string(),
        inflation_pct: a.inflation_pct,
        holdings: a.holdings,
        return_pct: a.return_pct,
        created: today.to_string(),
        currency: Some(a.currency),
    };
    store::update(GOALS_FILE, |s: &mut goals::GoalStore| {
        if s.find(&saved.name).is_some() {
            bail!("A goal named `{}` exists: remove it first", saved.name);
        }
        s.goals.push(saved.clone());
        Ok(())
    })?;
    println!("\n  {}\n", tf(Msg::GoalAddedSaved, &[&saved.name]).green());
    Ok(())
}

pub fn cmd_goal_remove(name: &str) -> Result<()> {
    store::update(GOALS_FILE, |s: &mut goals::GoalStore| {
        let before = s.goals.len();
        s.goals.retain(|g| !g.name.eq_ignore_ascii_case(name));
        if s.goals.len() == before {
            bail!("No saved goal named `{name}`");
        }
        Ok(())
    })?;
    println!("\n  {}\n", tf(Msg::GoalRemovedSaved, &[&name]));
    Ok(())
}

pub async fn cmd_goal_list(market: Market, json: bool) -> Result<()> {
    let currency = market_currency(market);
    let store: goals::GoalStore = store::load(GOALS_FILE)?;
    let client = if store.goals.iter().any(|g| !g.holdings.is_empty()) {
        YahooClient::new().await.ok()
    } else {
        None
    };
    let rows = goal_rows(client.as_ref(), market).await?;
    if json {
        return print_goal_json_list(currency, &rows);
    }
    if rows.is_empty() {
        println!("\n  {}\n", t(Msg::GoalNone).dimmed());
        return Ok(());
    }
    print_header(t(Msg::GoalsTitle));
    println!(
        "  {} {} {} {} {} {} {} {}",
        pad_end(t(Msg::GoalColName), 14).bold(),
        pad_start(t(Msg::GoalColTarget), 12).bold(),
        pad_start(t(Msg::GoalColDue), 10).bold(),
        pad_start(t(Msg::GoalColLeft), 6).bold(),
        pad_start(t(Msg::GoalColLinked), 13).bold(),
        pad_start(t(Msg::GoalColFunded), 8).bold(),
        pad_start(t(Msg::GoalColSip), 14).bold(),
        t(Msg::GoalColStatus).bold(),
    );
    for r in &rows {
        let status = match r.status {
            "funded" => t(Msg::GoalFunded).green(),
            "overdue" => t(Msg::GoalOverdue).red(),
            _ => t(Msg::GoalNeedsSip).yellow(),
        };
        println!(
            "  {} {} {} {} {} {} {} {}",
            pad_end(&r.name, 14).cyan(),
            pad_start(&goal_money(r.target_future, currency), 12),
            pad_start(&r.by, 10),
            pad_start(&r.months_left.to_string(), 6),
            pad_start(&goal_money(r.linked_value, currency), 13),
            pad_start(&format!("{:.0}%", r.funded_pct), 8),
            pad_start(&goal_money(r.monthly_sip, currency), 14),
            status,
        );
    }
    println!();
    Ok(())
}

fn print_goal_json_list(currency: &str, rows: &[GoalRow]) -> Result<()> {
    let mut v = serde_json::to_value(rows)?;
    round_floats(&mut v, 2);
    output::print_json(&serde_json::json!({"goal": "list", "currency": currency, "goals": v}))
}

pub async fn cmd_goal_show(name: &str, market: Market, json: bool) -> Result<()> {
    let store: goals::GoalStore = store::load(GOALS_FILE)?;
    let g = store
        .find(name)
        .ok_or_else(|| anyhow::anyhow!("No saved goal named `{name}`"))?
        .clone();
    check_goal_currency(&g, market)?;
    let today = chrono::Local::now().date_naive();
    let months = goals::months_until(today, &g.by).map_err(anyhow::Error::msg)?;
    let linked = if g.holdings.is_empty() {
        0.0
    } else {
        let client = YahooClient::new().await.ok();
        linked_value(client.as_ref(), &g.holdings, market, false)
            .await?
            .value
    };
    let report = goal_sip_report(saved_request(&g, months, linked, market), market);
    let currency = market_currency(market);
    if json {
        return print_goal_json(
            "show",
            currency,
            &serde_json::json!({
                "name": g.name, "by": g.by, "holdings": g.holdings, "linked_value": linked, "plan": report,
            }),
        );
    }
    print_header(&format!("{} — {}", g.name, g.by));
    if linked > 0.0 {
        print_kv(t(Msg::GoalColLinked), &goal_money(linked, currency));
    }
    print_sip_report(&report, currency);
    print_assumptions();
    Ok(())
}

// ── SWP, emergency fund, prepayment ──

pub struct SwpArgs {
    pub corpus: f64,
    pub cost: Option<f64>,
    pub withdrawal: f64,
    pub inflation_pct: f64,
    pub return_pct: f64,
    pub years: u32,
    pub fd_return_pct: f64,
    pub slab_pct: f64,
}

pub(crate) fn swp_plan(a: &SwpArgs, market: Market) -> goals::SwpPlan {
    goals::plan_swp(
        a.corpus,
        a.cost,
        a.withdrawal,
        a.inflation_pct,
        a.return_pct,
        a.years,
        a.fd_return_pct,
        a.slab_pct,
        goal_region(market),
    )
}

pub fn cmd_goal_swp(a: SwpArgs, market: Market, json: bool) -> Result<()> {
    let currency = market_currency(market);
    let plan = swp_plan(&a, market);
    if json {
        return print_goal_json("swp", currency, &plan);
    }
    let m = |x: f64| goal_money(x, currency);
    print_header(t(Msg::SwpTitle));
    print_kv(t(Msg::SwpCorpus), &m(a.corpus));
    print_kv(t(Msg::SwpWithdrawal), &per_month(a.withdrawal, currency));
    print_kv(t(Msg::GoalInflation), &pct_label(a.inflation_pct));
    print_kv(
        t(Msg::GoalWithdrawalRate),
        &format!("{:.2}%", plan.initial_withdrawal_rate_pct),
    );
    for (label, path, rate) in [
        (Msg::SwpEquity, &plan.swp, a.return_pct),
        (Msg::SwpFd, &plan.fd, a.fd_return_pct),
    ] {
        print_section(&format!("{} — {}", t(label), pct_label(rate)));
        let when = duration_label(path.runway_months);
        let line = if path.lasts {
            tf(Msg::SwpLasts, &[&duration_label(a.years * 12)])
                .green()
                .to_string()
        } else {
            tf(Msg::SwpRunsOut, &[&when]).red().to_string()
        };
        println!("  {line}");
        println!(
            "  {}",
            tf(Msg::SwpLeft, &[&m(path.final_value), &m(path.total_tax)])
        );
    }
    println!();
    Ok(())
}

pub(crate) fn emergency_plan(
    monthly_expense: f64,
    months_cover: u32,
    liquid: f64,
    horizon_months: u32,
    return_pct: Option<f64>,
    market: Market,
) -> goals::EmergencyPlan {
    let region = goal_region(market);
    let tbill = (region == goals::Region::India)
        .then(crate::gsec::cached_tbill_364)
        .flatten();
    let ret =
        return_pct.unwrap_or_else(|| goals::baseline(horizon_months, region, tbill).return_pct);
    goals::plan_emergency(monthly_expense, months_cover, liquid, horizon_months, ret)
}

pub fn cmd_goal_emergency(plan: goals::EmergencyPlan, market: Market, json: bool) -> Result<()> {
    let currency = market_currency(market);
    if json {
        return print_goal_json("emergency", currency, &plan);
    }
    let m = |x: f64| goal_money(x, currency);
    print_header(t(Msg::EmergencyTitle));
    print_kv(
        t(Msg::GoalExpenseToday),
        &per_month(plan.monthly_expense, currency),
    );
    print_kv(
        t(Msg::EmergencyTarget),
        &format!("{} ({})", m(plan.target), duration_label(plan.months_cover)),
    );
    print_kv(
        t(Msg::EmergencyHave),
        &format!("{} ({:.0}%)", m(plan.liquid), plan.funded_pct),
    );
    if plan.build.monthly_sip <= 0.0 {
        println!("\n  {}\n", t(Msg::GoalCovered).green());
        return Ok(());
    }
    print_kv(
        t(Msg::GoalExpectedReturn),
        &pct_label(plan.build.annual_return_pct),
    );
    print_kv(
        t(Msg::EmergencyBuild),
        &per_month(plan.build.monthly_sip, currency)
            .bold()
            .green()
            .to_string(),
    );
    print_schedule(&plan.build, currency);
    println!();
    Ok(())
}

pub struct PrepayArgs {
    pub principal: f64,
    pub loan_rate_pct: f64,
    pub remaining_months: u32,
    pub extra: f64,
    pub lump_sum: f64,
    pub return_pct: Option<f64>,
    pub tax: Option<goals::TaxKind>,
    pub slab_pct: Option<f64>,
}

pub(crate) fn prepay_plan(a: &PrepayArgs, market: Market) -> goals::PrepayPlan {
    let region = goal_region(market);
    let regime = goals::regime_for(a.remaining_months, region, None, a.return_pct);
    let kind = a
        .tax
        .unwrap_or_else(|| goals::default_tax_kind(regime.kind, region));
    let slab = a.slab_pct.unwrap_or_else(|| region.default_slab_pct());
    let rules = goals::TaxRules::new(
        region,
        kind,
        slab,
        goals::default_exit_load_pct(kind, region),
    );
    goals::plan_prepay(
        a.principal,
        a.loan_rate_pct,
        a.remaining_months,
        a.extra,
        a.lump_sum,
        regime.return_pct,
        regime.vol_pct,
        &rules,
    )
}

pub fn cmd_goal_prepay(a: PrepayArgs, market: Market, json: bool) -> Result<()> {
    let currency = market_currency(market);
    let p = prepay_plan(&a, market);
    if json {
        return print_goal_json("prepay", currency, &p);
    }
    let m = |x: f64| goal_money(x, currency);
    print_header(t(Msg::PrepayTitle));
    print_kv(t(Msg::PrepayEmi), &per_month(p.emi, currency));
    print_kv(t(Msg::PrepayTenure), &duration_label(p.new_tenure_months));
    print_kv(
        t(Msg::PrepayTenureSaved),
        &duration_label(p.tenure_saved_months),
    );
    print_kv(
        t(Msg::PrepayInterestSaved),
        &m(p.interest_saved).bold().green().to_string(),
    );
    print_section(&pct_label(p.sip_return_pct));
    print_kv(t(Msg::PrepaySipNet), &m(p.sip_net));
    print_kv(t(Msg::PrepayEquivalent), &m(p.prepay_equivalent));
    print_kv(
        t(Msg::GoalBands),
        &band_text(
            p.sip_percentiles
                .net
                .as_ref()
                .unwrap_or(&p.sip_percentiles.gross),
            currency,
        ),
    );
    print_kv(
        t(Msg::PrepayChance),
        &format!("{:.0}%", p.sip_percentiles.success_prob_pct),
    );
    print_kv(
        t(Msg::PrepayBreakeven),
        &p.breakeven_return_pct
            .map_or_else(|| t(Msg::PrepayNoBreakeven).to_string(), pct_label),
    );
    print_assumptions();
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
    fn linked_holdings_match_by_symbol_or_exchange_suffix() {
        let h = |s: &str| pf::Holding {
            symbol: s.into(),
            shares: 1.0,
            avg_cost: 1.0,
            added_at: String::new(),
            lots: Vec::new(),
        };
        assert!(names_holding(&h("NIFTYBEES.NS"), "niftybees"));
        assert!(names_holding(&h("MF:122639"), "MF:122639"));
        assert!(!names_holding(&h("NIFTYBEES.NS"), "NIFTY"));
        assert!(!names_holding(&h("TCS.NS"), "INFY"));
    }

    fn saved(infl: Option<f64>, created: &str, currency: Option<&str>) -> goals::SavedGoal {
        goals::SavedGoal {
            name: "g".into(),
            target: 1_000_000.0,
            by: " 2031-06 ".into(),
            inflation_pct: infl,
            holdings: Vec::new(),
            return_pct: None,
            created: created.into(),
            currency: currency.map(String::from),
        }
    }

    #[test]
    fn saved_goal_dates_are_trimmed_and_inflated_from_creation() {
        assert_eq!(goal_due(" 2031-06 "), NaiveDate::from_ymd_opt(2031, 6, 1));
        let due = goal_due("2031-06");
        // Created 5 years (60 months) before the due date; 4 months left now.
        let g = saved(Some(6.0), "2026-06-01", None);
        let want = 1_000_000.0 * 1.06f64.powi(5);
        assert!((saved_target_future(&g, due, 4) - want).abs() < 1.0);
        // No inflation: the target as saved.
        assert_eq!(
            saved_target_future(&saved(None, "2026-06-01", None), due, 4),
            1e6
        );
    }

    #[test]
    fn saved_goals_stay_in_their_currency() {
        let g = saved(None, "2026-06-01", Some("INR"));
        assert!(check_goal_currency(&g, Market::In).is_ok());
        assert!(check_goal_currency(&g, Market::Us).is_err());
        assert!(check_goal_currency(&saved(None, "2026-06-01", None), Market::Us).is_ok());
    }

    #[test]
    fn equity_tax_needs_india_or_us() {
        use goals::TaxKind::*;
        assert!(check_tax_region(Some(Equity), Market::In).is_ok());
        assert!(check_tax_region(Some(Arbitrage), Market::Us).is_ok());
        assert!(check_tax_region(Some(Debt), Market::Crypto).is_ok());
        assert!(check_tax_region(Option::None, Market::Crypto).is_ok());
        assert!(check_tax_region(Some(Equity), Market::Crypto).is_err());
    }

    #[test]
    fn durations_use_singular_and_plural_forms() {
        assert_eq!(duration_label(0), "0 months");
        assert_eq!(duration_label(1), "1 month");
        assert_eq!(duration_label(6), "6 months");
        assert_eq!(duration_label(12), "1 year");
        assert_eq!(duration_label(18), "1 year 6 months");
        assert_eq!(duration_label(24), "2 years");
        assert_eq!(duration_label(25), "2 years 1 month");
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
