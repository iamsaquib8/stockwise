//! Mutual funds: AMFI scheme search and NAVs with expense ratios (TER), NAV
//! history with returns, SIP simulation with XIRR, category screening, and
//! ETF/fund overlap.

use super::*;
use crate::funds::{self, Scheme, SchemeTer, TerTable};

/// How `mf screen` filters plans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PlanFilter {
    Direct,
    Regular,
    All,
}

/// How `mf screen` ranks funds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreenSort {
    /// Trailing return over this many months, highest first.
    Return(u32),
    /// Expense ratio, lowest first.
    Ter,
}

/// The TER table, or `None` with a note on stderr: expense ratios are extra
/// information and never block NAVs or returns.
fn ter_or_note(ter: Result<TerTable>) -> Option<TerTable> {
    match ter {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("  Note: expense ratios (TER) unavailable — {e:#}");
            None
        }
    }
}

/// "0.67% direct plan · regular 2.19% · as of 23 Oct 2026".
fn ter_line(t: &SchemeTer) -> String {
    let other = match t.plan {
        "direct" => t
            .regular
            .map(|r| format!(" · regular plan {}", pct_plain(r))),
        _ => t.direct.map(|d| format!(" · direct plan {}", pct_plain(d))),
    };
    format!(
        "{} {} plan{} · as of {}",
        pct_plain(t.costs.total).bold(),
        t.plan,
        other.unwrap_or_default(),
        t.date.format("%d %b %Y")
    )
}

/// TER cell for `mf nav` / `mf history`: the value, or why there is none.
fn ter_cell(table: Option<&TerTable>, ter: Option<&SchemeTer>) -> String {
    match (table, ter) {
        (_, Some(t)) => ter_line(t),
        (Some(_), None) => "not in AMFI's TER disclosure for this plan"
            .dimmed()
            .to_string(),
        (None, None) => "unavailable".dimmed().to_string(),
    }
}

const INR: Option<&str> = Some("INR");

/// Right-aligned percentage, padded before colouring so columns line up.
fn pct_cell(v: Option<f64>, width: usize) -> String {
    match v {
        Some(v) => {
            let s = format!("{:>width$}", format!("{:+.2}%", v * 100.0));
            if v >= 0.0 {
                s.green().to_string()
            } else {
                s.red().to_string()
            }
        }
        None => format!("{:>width$}", "N/A").dimmed().to_string(),
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn months_label(months: Option<u32>) -> String {
    match months {
        None => "max".into(),
        Some(m) if m % 12 == 0 => format!("{}y", m / 12),
        Some(m) => format!("{m}mo"),
    }
}

fn period_months(period: &str) -> Result<Option<u32>> {
    funds::parse_period(period).map_err(|e| anyhow::anyhow!(e))
}

/// At most `max` points, evenly spaced and keeping the last, for charts.
fn downsample(values: &[f64], max: usize) -> Vec<f64> {
    if values.len() <= max || max < 2 {
        return values.to_vec();
    }
    let step = (values.len() - 1) as f64 / (max - 1) as f64;
    (0..max)
        .map(|i| values[((i as f64 * step).round() as usize).min(values.len() - 1)])
        .collect()
}

/// `mf search` — find schemes in AMFI's list.
pub async fn cmd_mf_search(query: &str, limit: usize, json: bool) -> Result<()> {
    let schemes = funds::load_amfi().await?;
    let hits = funds::search(&schemes, query);
    let shown: Vec<&Scheme> = hits.iter().take(limit.max(1)).copied().collect();
    if json {
        println!("{}", serde_json::to_string_pretty(&shown)?);
        return Ok(());
    }
    if hits.is_empty() {
        bail!(
            "No mutual fund scheme matches '{}'. Try fewer words, e.g. a fund house or \"flexi cap\"",
            query
        );
    }
    print_header(&format!("Mutual funds: {}", query));
    println!(
        "  {:>7}  {:<62} {:>11} {:>11}",
        "Code".bold(),
        "Scheme".bold(),
        "NAV".bold(),
        "Date".bold()
    );
    println!("  {}", "─".repeat(95).dimmed());
    for s in &shown {
        println!(
            "  {:>7}  {:<62} {:>11} {:>11}",
            s.code.to_string().cyan(),
            truncate(&s.full_name(), 62),
            s.nav
                .map(|n| format!("{:.4}", n))
                .unwrap_or_else(|| "N/A".into()),
            s.date
                .map(|d| d.format("%d %b %Y").to_string())
                .unwrap_or_default()
                .dimmed()
        );
    }
    if hits.len() > shown.len() {
        println!(
            "\n  {}",
            format!(
                "{} more — add words (\"direct growth\", a fund house) or raise --limit",
                hits.len() - shown.len()
            )
            .dimmed()
        );
    }
    println!(
        "\n  {}",
        "Use the code: stockwise mf nav <code> · mf history <code> · mf sip <code> <amount>"
            .dimmed()
    );
    println!();
    Ok(())
}

/// One scheme in `mf nav --json`: AMFI's row and its expense ratio.
#[derive(serde::Serialize)]
pub(crate) struct NavRow<'a> {
    #[serde(flatten)]
    pub(crate) scheme: &'a Scheme,
    pub(crate) ter: &'a Option<SchemeTer>,
}

/// `mf nav` — latest NAV from AMFI for one or more scheme codes.
pub async fn cmd_mf_nav(codes: &[u32], json: bool) -> Result<()> {
    let (schemes, ter) = tokio::join!(funds::load_amfi(), funds::load_ter());
    let schemes = schemes?;
    let ter = ter_or_note(ter);
    let found: Vec<&Scheme> = codes
        .iter()
        .filter_map(|c| schemes.iter().find(|s| s.code == *c))
        .collect();
    let missing: Vec<String> = codes
        .iter()
        .filter(|c| !found.iter().any(|s| s.code == **c))
        .map(|c| c.to_string())
        .collect();
    let missing_err = || {
        anyhow::anyhow!(
            "No scheme with code {} in AMFI's NAV list (find codes with `stockwise mf search <name>`)",
            missing.join(", ")
        )
    };
    if found.is_empty() {
        return Err(missing_err());
    }
    let ters: Vec<Option<SchemeTer>> = found
        .iter()
        .map(|s| ter.as_ref().and_then(|t| t.for_scheme(s)))
        .collect();
    if json {
        let rows: Vec<NavRow> = found
            .iter()
            .zip(&ters)
            .map(|(scheme, ter)| NavRow { scheme, ter })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for (s, t) in found.iter().zip(&ters) {
            print_header(&s.full_name());
            print_kv(
                "NAV",
                &s.nav
                    .map(|n| format!("₹{:.4}", n).bold().to_string())
                    .unwrap_or_else(|| "N/A".dimmed().to_string()),
            );
            print_kv(
                "As of",
                &s.date
                    .map(|d| d.format("%d %b %Y").to_string())
                    .unwrap_or_else(|| "—".into()),
            );
            print_kv("Scheme code", &s.code.to_string());
            print_kv("Fund house", s.amc.as_deref().unwrap_or("—"));
            print_kv("Category", s.category.as_deref().unwrap_or("—"));
            print_kv("Type", s.scheme_type.as_deref().unwrap_or("—"));
            print_kv("Expense ratio (TER)", &ter_cell(ter.as_ref(), t.as_ref()));
            if let Some(isin) = &s.isin_growth {
                print_kv("ISIN", isin);
            }
            if let Some(isin) = &s.isin_reinvest {
                print_kv("ISIN (reinvestment)", isin);
            }
        }
        println!(
            "\n  {}",
            "Source: AMFI NAVAll.txt (cached up to 12 hours); TER: AMFI's monthly TER disclosure (cached a day)"
                .dimmed()
        );
        println!();
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing_err())
    }
}

/// `mf history` — NAV chart and trailing returns for one scheme.
pub async fn cmd_mf_history(code: u32, period: &str, json: bool) -> Result<()> {
    let months = period_months(period)?;
    let client = funds::http_client()?;
    let (hist, schemes, ter) = tokio::join!(
        funds::load_history(&client, code),
        funds::load_amfi(),
        funds::load_ter()
    );
    let hist = hist?;
    let ter_table = ter_or_note(ter);
    // Join on AMFI's own scheme name when the NAV file has the code; the
    // NAV-history name ("X Fund - Direct Plan - Growth") is the fallback.
    let scheme_ter = ter_table.as_ref().and_then(|t| {
        match schemes
            .ok()
            .and_then(|all| all.into_iter().find(|s| s.code == code))
        {
            Some(s) => t.for_scheme(&s),
            None => t.for_name(
                &hist.meta.name,
                hist.meta.name.to_lowercase().contains("direct"),
            ),
        }
    });
    let navs = &hist.navs;
    let window = funds::slice_period(navs, months);
    let returns: Vec<(&str, Option<f64>)> = funds::HORIZONS
        .iter()
        .map(|(label, m)| (*label, funds::trailing_return(navs, *m)))
        .collect();
    let since_inception = funds::since_inception(navs);
    let drawdown = funds::max_drawdown(window);
    let &(last_date, last_nav) = navs.last().context("empty NAV history")?;

    if json {
        let ret: serde_json::Map<String, serde_json::Value> = returns
            .iter()
            .map(|(k, v)| (k.to_lowercase(), serde_json::json!(v)))
            .chain([("since_inception".into(), serde_json::json!(since_inception))])
            .collect();
        let out = serde_json::json!({
            "code": code,
            "name": hist.meta.name,
            "fund_house": hist.meta.fund_house,
            "category": hist.meta.category,
            "period": months_label(months),
            "nav": last_nav,
            "date": last_date,
            "returns": ret,
            "max_drawdown": drawdown,
            "ter": scheme_ter,
            "navs": window.iter().map(|(d, n)| serde_json::json!({"date": d, "nav": n})).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    print_header(&format!("{} ({})", hist.meta.name, months_label(months)));
    print_kv(
        "NAV",
        &format!(
            "{}  {}",
            format_price(last_nav, INR).bold(),
            last_date.format("%d %b %Y").to_string().dimmed()
        ),
    );
    print_kv("Fund house", hist.meta.fund_house.as_deref().unwrap_or("—"));
    print_kv("Category", hist.meta.category.as_deref().unwrap_or("—"));
    print_kv(
        "Expense ratio (TER)",
        &ter_cell(ter_table.as_ref(), scheme_ter.as_ref()),
    );

    let values: Vec<f64> = window.iter().map(|(_, n)| *n).collect();
    if values.len() >= 2 {
        let color = if values.last() >= values.first() {
            "green"
        } else {
            "red"
        };
        println!();
        for line in charts::line_chart(&downsample(&values, 110), 55, 10, color, "NAV") {
            println!("{}", line);
        }
        if let (Some(first), Some(last)) = (window.first(), window.last()) {
            println!(
                "  {:>10} {}{:>width$}",
                "",
                first.0.format("%b %Y").to_string().dimmed(),
                last.0.format("%b %Y").to_string().dimmed(),
                width = 48
            );
        }
    }

    print_section("Trailing returns (1Y+ annualized)");
    for (label, r) in &returns {
        print_kv(label, &pct_cell(*r, 9));
    }
    print_kv("Since inception (CAGR)", &pct_cell(since_inception, 9));
    if let Some(dd) = drawdown {
        print_kv(
            &format!("Max drawdown ({})", months_label(months)),
            &format!("{:>9}", format!("-{:.2}%", dd * 100.0))
                .red()
                .to_string(),
        );
    }
    println!(
        "\n  {}",
        format!(
            "NAV history: api.mfapi.in (AMFI data), {} → {}. Past returns do not predict future returns.",
            navs[0].0.format("%b %Y"),
            last_date.format("%b %Y")
        )
        .dimmed()
    );
    println!();
    Ok(())
}

/// A monthly SIP replayed on a scheme's NAV history.
pub(crate) struct SipRun {
    pub(crate) hist: funds::NavHistory,
    pub(crate) months: Option<u32>,
    /// First instalment date; later than asked when NAV history is shorter.
    pub(crate) start: chrono::NaiveDate,
    /// The history began after the requested start.
    pub(crate) clipped: bool,
    pub(crate) result: funds::SipResult,
}

/// Replay a monthly SIP of `amount` into scheme `code` over `period`
/// (`5y`, `18mo`, `max`).
pub(crate) async fn run_sip(code: u32, amount: f64, period: &str) -> Result<SipRun> {
    if !(amount.is_finite() && amount > 0.0) {
        bail!("SIP amount must be a positive number");
    }
    let months = period_months(period)?;
    let client = funds::http_client()?;
    let hist = funds::load_history(&client, code).await?;
    let navs = &hist.navs;
    let (first_date, _) = navs[0];
    let (last_date, _) = *navs.last().context("empty NAV history")?;
    let wanted_start = match months {
        Some(m) => last_date
            .checked_sub_months(chrono::Months::new(m))
            .unwrap_or(first_date),
        None => first_date,
    };
    let start = wanted_start.max(first_date);
    let result = funds::simulate_sip(navs, amount, start)
        .context("Not enough NAV history for a SIP over this period")?;
    Ok(SipRun {
        hist,
        months,
        start,
        clipped: start > wanted_start,
        result,
    })
}

impl SipRun {
    /// `mf sip --json`.
    pub(crate) fn json(&self, code: u32, amount: f64) -> serde_json::Value {
        let r = &self.result;
        serde_json::json!({
            "code": code,
            "name": self.hist.meta.name,
            "monthly_amount": amount,
            "period": months_label(self.months),
            "start": r.instalments.first().map(|i| i.date),
            "end": r.end_date,
            "instalments": r.instalments.len(),
            "invested": r.invested,
            "units": r.units,
            "nav": r.end_nav,
            "value": r.value,
            "gain": r.value - r.invested,
            "xirr": r.xirr,
            "history": r.instalments,
        })
    }
}

/// `mf sip` — monthly SIP into a scheme, with XIRR.
pub async fn cmd_mf_sip(code: u32, amount: f64, period: &str, json: bool) -> Result<()> {
    let run = run_sip(code, amount, period).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&run.json(code, amount))?);
        return Ok(());
    }
    let SipRun {
        hist,
        months,
        start,
        clipped,
        result: r,
    } = run;

    print_header(&format!(
        "SIP: {} — ₹{}/month ({})",
        hist.meta.name,
        amount,
        months_label(months)
    ));
    if clipped {
        println!(
            "  {}",
            format!(
                "NAV history starts {}; the SIP starts there.",
                start.format("%d %b %Y")
            )
            .yellow()
        );
    }
    let value_curve: Vec<f64> = r.instalments.iter().map(|i| i.value).collect();
    let invested_curve: Vec<f64> = r.instalments.iter().map(|i| i.invested).collect();
    if value_curve.len() >= 2 {
        for line in charts::dual_line_chart(
            &value_curve,
            &invested_curve,
            55,
            8,
            "Value vs Invested",
            "Value",
            "Invested",
        ) {
            println!("{}", line);
        }
    }

    print_section("Results");
    let first = r.instalments.first().map(|i| i.date).unwrap_or(start);
    print_kv(
        "Instalments",
        &format!(
            "{} ({} → {})",
            r.instalments.len(),
            first.format("%b %Y"),
            r.instalments
                .last()
                .map(|i| i.date)
                .unwrap_or(first)
                .format("%b %Y")
        ),
    );
    print_kv("Total invested", &format_price(r.invested, INR));
    print_kv(
        &format!("Value on {}", r.end_date.format("%d %b %Y")),
        &format_price(r.value, INR).bold().to_string(),
    );
    let gain = r.value - r.invested;
    let gain_str = format!(
        "{}{} ({:+.2}%)",
        if gain >= 0.0 { "+" } else { "-" },
        format_price(gain.abs(), INR),
        gain / r.invested * 100.0
    );
    print_kv(
        "Gain",
        &if gain >= 0.0 {
            gain_str.green().bold().to_string()
        } else {
            gain_str.red().bold().to_string()
        },
    );
    print_kv("XIRR (annualized)", pct_cell(r.xirr, 0).trim());
    print_kv("Units", &format!("{:.3}", r.units));
    print_kv(
        "Average cost / unit",
        &format_price(r.invested / r.units, INR),
    );
    print_kv("Latest NAV", &format_price(r.end_nav, INR));
    println!(
        "\n  {}",
        "Bought at the first NAV on or after each due date; no stamp duty, exit load or tax modelled."
            .dimmed()
    );
    println!();
    Ok(())
}

/// One fund's row in `mf screen`.
#[derive(serde::Serialize)]
struct ScreenRow {
    code: u32,
    name: String,
    category: Option<String>,
    nav: Option<f64>,
    date: Option<chrono::NaiveDate>,
    return_1y: Option<f64>,
    return_3y: Option<f64>,
    return_5y: Option<f64>,
    since_inception: Option<f64>,
    /// Total expense ratio of the scheme's plan (fraction), from AMFI.
    ter: Option<f64>,
    ter_date: Option<chrono::NaiveDate>,
}

/// `mf screen` — rank a category's growth-option schemes by trailing
/// returns or expense ratio, optionally capped by `max_ter_pct`. Without a
/// category, list the categories.
pub async fn cmd_mf_screen(
    category: &str,
    plan: PlanFilter,
    sort: ScreenSort,
    max_ter_pct: Option<f64>,
    limit: usize,
    max_funds: usize,
    json: bool,
) -> Result<()> {
    let category = category.trim();
    let (all, ter) = if category.is_empty() {
        (funds::load_amfi().await?, None)
    } else {
        let (all, ter) = tokio::join!(funds::load_amfi(), funds::load_ter());
        (all?, Some(ter))
    };
    let schemes = funds::active(&all);
    if category.is_empty() {
        let cats = funds::categories(schemes.iter().copied());
        if json {
            let out: Vec<serde_json::Value> = cats
                .iter()
                .map(|(c, n)| serde_json::json!({"category": c, "schemes": n}))
                .collect();
            println!("{}", serde_json::to_string_pretty(&out)?);
            return Ok(());
        }
        print_header("Mutual fund categories (AMFI)");
        for (c, n) in &cats {
            println!("  {:<78} {:>5}", c, n.to_string().dimmed());
        }
        println!(
            "\n  {}",
            "Screen one: stockwise mf screen flexi cap · mf screen elss --sort 5y".dimmed()
        );
        println!();
        return Ok(());
    }

    let key = funds::category_key(category);
    let mut candidates: Vec<&Scheme> = schemes
        .into_iter()
        .filter(|s| {
            s.category
                .as_deref()
                .is_some_and(|c| funds::category_key(c).contains(&key))
        })
        .filter(|s| s.is_growth())
        .filter(|s| match plan {
            PlanFilter::Direct => s.is_direct(),
            PlanFilter::Regular => !s.is_direct(),
            PlanFilter::All => true,
        })
        .collect();
    if candidates.is_empty() {
        bail!(
            "No active growth-option schemes in a category matching '{}'. Run `stockwise mf screen` to list categories",
            category
        );
    }
    let ter = match ter {
        Some(Err(e)) if sort == ScreenSort::Ter || max_ter_pct.is_some() => {
            return Err(e.context(
                "--sort ter and --max-ter need AMFI's TER disclosure, which could not be loaded",
            ));
        }
        Some(t) => ter_or_note(t),
        None => None,
    };
    let ter_of = |s: &Scheme| ter.as_ref().and_then(|t| t.for_scheme(s));
    let mut no_ter = 0;
    if let Some(max) = max_ter_pct {
        let before = candidates.len();
        candidates.retain(|s| ter_of(s).is_some_and(|t| t.costs.total * 100.0 <= max + 1e-9));
        no_ter = before - candidates.len();
        if candidates.is_empty() {
            bail!(
                "No scheme in '{}' has a disclosed TER at or below {}%",
                category,
                max
            );
        }
    }
    candidates.sort_by_key(|s| s.full_name());
    if sort == ScreenSort::Ter {
        // Cheapest first before --max cuts the list, so the cap keeps the
        // funds the ranking is about. Schemes without a TER go last.
        candidates.sort_by(|a, b| {
            let (ta, tb) = (ter_of(a), ter_of(b));
            match (ta.map(|t| t.costs.total), tb.map(|t| t.costs.total)) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (None, None) => std::cmp::Ordering::Equal,
            }
        });
    }
    let total = candidates.len();
    if total > max_funds {
        eprintln!(
            "  {} funds match; screening the first {} {}. Narrow the category or raise --max.",
            total,
            max_funds,
            if sort == ScreenSort::Ter {
                "by expense ratio"
            } else {
                "by name"
            }
        );
        candidates.truncate(max_funds);
    }
    eprintln!(
        "  Fetching NAV history for {} funds (cached for 12 hours)...",
        candidates.len()
    );

    let client = funds::http_client()?;
    let mut set = tokio::task::JoinSet::new();
    let gate = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    for s in &candidates {
        let (client, gate, code) = (client.clone(), gate.clone(), s.code);
        set.spawn(async move {
            let _permit = gate.acquire_owned().await;
            (code, funds::load_history(&client, code).await)
        });
    }
    let mut histories = std::collections::HashMap::new();
    let mut failed = 0;
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok((code, Ok(h))) => {
                histories.insert(code, h);
            }
            _ => failed += 1,
        }
    }

    let mut rows: Vec<ScreenRow> = candidates
        .iter()
        .filter_map(|s| {
            let h = histories.get(&s.code)?;
            let t = ter_of(s);
            Some(ScreenRow {
                code: s.code,
                name: s.full_name(),
                category: s.category.clone(),
                nav: s.nav,
                date: s.date,
                return_1y: funds::trailing_return(&h.navs, 12),
                return_3y: funds::trailing_return(&h.navs, 36),
                return_5y: funds::trailing_return(&h.navs, 60),
                since_inception: funds::since_inception(&h.navs),
                ter: t.as_ref().map(|t| t.costs.total),
                ter_date: t.as_ref().map(|t| t.date),
            })
        })
        .collect();
    let pick = |r: &ScreenRow| match sort {
        ScreenSort::Return(12) => r.return_1y,
        ScreenSort::Return(60) => r.return_5y,
        ScreenSort::Return(_) => r.return_3y,
        // Lowest TER first: rank on the negated value.
        ScreenSort::Ter => r.ter.map(|t| -t),
    };
    // Best first; funds without a value (too young, no TER) go last.
    rows.sort_by(|a, b| match (pick(a), pick(b)) {
        (Some(x), Some(y)) => y.total_cmp(&x),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    });
    if rows.is_empty() {
        bail!("Could not fetch NAV history for any matching fund; try again later");
    }
    rows.truncate(limit.max(1));

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    let sort_label = match sort {
        ScreenSort::Return(m) => format!("{} return", months_label(Some(m)).to_uppercase()),
        ScreenSort::Ter => "expense ratio (lowest first)".into(),
    };
    print_header(&format!(
        "Fund screen: '{}' — top {} by {}{}",
        category,
        rows.len(),
        sort_label,
        max_ter_pct
            .map(|m| format!(", TER ≤ {m}%"))
            .unwrap_or_default()
    ));
    println!(
        "  {:>3} {:>7}  {:<52} {:>7} {:>9} {:>9} {:>9}",
        "#".bold(),
        "Code".bold(),
        "Scheme".bold(),
        "TER".bold(),
        "1Y".bold(),
        "3Y CAGR".bold(),
        "5Y CAGR".bold()
    );
    println!("  {}", "─".repeat(104).dimmed());
    for (i, r) in rows.iter().enumerate() {
        let ter_cell = match r.ter {
            Some(t) => format!("{:>7}", pct_plain(t)),
            None => format!("{:>7}", "N/A").dimmed().to_string(),
        };
        println!(
            "  {:>3} {:>7}  {:<52} {} {} {} {}",
            i + 1,
            r.code.to_string().cyan(),
            truncate(&r.name, 52),
            ter_cell,
            pct_cell(r.return_1y, 9),
            pct_cell(r.return_3y, 9),
            pct_cell(r.return_5y, 9)
        );
    }
    if failed > 0 {
        println!(
            "\n  {}",
            format!("{} fund(s) skipped: NAV history unavailable", failed).yellow()
        );
    }
    if no_ter > 0 {
        println!(
            "  {}",
            format!(
                "{} fund(s) left out: TER above the cap or not in AMFI's disclosure",
                no_ter
            )
            .dimmed()
        );
    }
    let ter_note = match &ter {
        Some(t) => format!(
            "TER: total expense ratio of each plan from AMFI's {} disclosure (latest day).",
            t.month
        ),
        None => "TER: AMFI's disclosure could not be loaded this time.".into(),
    };
    println!(
        "\n  {}",
        format!(
            "Growth options only. {}\n  Direct plans cost less than regular ones. Past returns do not predict future returns.",
            ter_note
        )
        .dimmed()
    );
    println!();
    Ok(())
}

/// Format a fraction as a plain percentage.
fn pct_plain(v: f64) -> String {
    format!("{:.2}%", v * 100.0)
}

async fn fund_profile(
    client: &YahooClient,
    symbol: &str,
    market: Market,
) -> Result<funds::FundProfile> {
    if !symbol.is_empty() && symbol.chars().all(|c| c.is_ascii_digit()) {
        bail!(
            "{} looks like an AMFI scheme code. AMFI publishes NAVs, not portfolios, so StockWise \
             has no holdings for Indian mutual funds. Overlap works for funds Yahoo Finance has \
             holdings for — mainly US ETFs and mutual funds (e.g. `mf overlap SPY QQQ -m us`)",
            symbol
        );
    }
    let resolved = market::resolve_symbol(symbol, market);
    let url = crate::api::build_url(
        "https://query2.finance.yahoo.com/v10/finance/quoteSummary",
        Some(&resolved),
        &[
            ("modules", "topHoldings,fundProfile,price"),
            ("crumb", client.crumb()),
        ],
    );
    let no_data = || {
        format!(
            "No fund holdings for {} on Yahoo Finance. Holdings exist for US ETFs and mutual funds \
             (e.g. SPY, QQQ, VFIAX with -m us); NSE-listed ETFs such as NIFTYBEES have none{}",
            resolved,
            if market == Market::In {
                " — for US funds add `-m us`"
            } else {
                ""
            }
        )
    };
    let v = client.raw_get(&url).await.with_context(no_data)?;
    funds::parse_fund_profile(&resolved, &v).with_context(no_data)
}

/// `mf overlap` — shared top holdings and sector overlap of two funds/ETFs.
pub async fn cmd_mf_overlap(a: &str, b: &str, market: Market, json: bool) -> Result<()> {
    let client = YahooClient::new().await?;
    let pa = fund_profile(&client, a, market).await?;
    let pb = fund_profile(&client, b, market).await?;
    let o = funds::overlap(&pa, &pb);

    if json {
        let out = serde_json::json!({ "a": pa, "b": pb, "overlap": o });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    print_header(&format!("Overlap: {} vs {}", pa.symbol, pb.symbol));
    println!(
        "  {:<24} {:<30} {:<30}",
        "",
        pa.symbol.bold().cyan(),
        pb.symbol.bold().cyan()
    );
    let row = |label: &str, x: String, y: String| {
        println!(
            "  {:<24} {:<30} {:<30}",
            label.dimmed(),
            truncate(&x, 30),
            truncate(&y, 30)
        );
    };
    let or_dash = |s: &Option<String>| s.clone().unwrap_or_else(|| "—".into());
    row("Name", or_dash(&pa.name), or_dash(&pb.name));
    row("Family", or_dash(&pa.family), or_dash(&pb.family));
    row("Category", or_dash(&pa.category), or_dash(&pb.category));
    let er = |p: &funds::FundProfile| {
        p.expense_ratio
            .map(pct_plain)
            .unwrap_or_else(|| "N/A".into())
    };
    row("Expense ratio", er(&pa), er(&pb));
    row(
        "Top holdings cover",
        format!("{} ({} names)", pct_plain(o.coverage_a), pa.holdings.len()),
        format!("{} ({} names)", pct_plain(o.coverage_b), pb.holdings.len()),
    );

    print_section("Shared top holdings");
    if o.common.is_empty() {
        println!(
            "  {}",
            "None of the published top holdings are shared.".dimmed()
        );
    } else {
        println!(
            "  {:<10} {:<28} {:>9} {:>9} {:>9}",
            "Symbol".bold(),
            "Name".bold(),
            pa.symbol.bold(),
            pb.symbol.bold(),
            "Overlap".bold()
        );
        for c in &o.common {
            println!(
                "  {:<10} {:<28} {:>9} {:>9} {:>9}",
                c.symbol.as_deref().unwrap_or("—").cyan(),
                truncate(c.name.as_deref().unwrap_or("—"), 28),
                pct_plain(c.weight_a),
                pct_plain(c.weight_b),
                pct_plain(c.weight_a.min(c.weight_b))
            );
        }
    }

    print_section("Overlap");
    print_kv(
        "Top-holdings overlap",
        &format!("{} (at least)", pct_plain(o.top_holdings_overlap))
            .bold()
            .to_string(),
    );
    if let Some(s) = o.sector_overlap {
        print_kv("Sector overlap", &pct_plain(s));
    }
    println!(
        "\n  {}",
        "Overlap = Σ min(weight in A, weight in B). Yahoo publishes only each fund's top 10 holdings,\n  so the true holdings overlap is at least the figure above; sector weights cover the whole fund."
            .dimmed()
    );
    println!();
    Ok(())
}
