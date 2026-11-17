//! `portfolio analyze`: gathers the inputs of [`crate::portfolio_analysis`]
//! (prices, a year of closes, sectors, scores, tax rules, the profile's
//! limits) and prints the dashboard. The TUI's portfolio screen uses the
//! same loader.

use super::*;
use crate::offmarket;
use crate::portfolio as pf;
use crate::portfolio_analysis::{self as pa, Analysis, Level, LotInput, PositionInput};
use crate::tax::{self, Country};
use futures_util::StreamExt;

/// Most holdings that get a year of closes (one request each, spaced out).
const CHART_LIMIT: usize = 80;
/// Most holdings without a stored sector that get one looked up.
const SECTOR_LIMIT: usize = 40;

/// What `portfolio analyze` prints with `--json`: the analysis and the
/// tax a sale of everything today would bring.
#[derive(Debug, Serialize)]
pub struct AnalyzeReport {
    #[serde(flatten)]
    pub analysis: Analysis,
    pub tax_if_sold_today: Option<TaxIfSold>,
    pub disclaimer: &'static str,
}

/// Estimated tax on every unrealised gain if all holdings were sold today,
/// at the standard rates, before any other gain or loss of the year.
#[derive(Debug, Serialize)]
pub struct TaxIfSold {
    pub country: &'static str,
    pub currency: String,
    pub short_term_tax: f64,
    pub long_term_tax: f64,
    pub cess: f64,
    pub total: f64,
    pub exemption_used: f64,
}

/// One holding in `portfolio analyze --csv`, flat; amounts in the holding's
/// own currency unless named `_base`.
#[derive(Debug, Serialize)]
pub(crate) struct PositionCsv {
    symbol: String,
    name: Option<String>,
    market: String,
    currency: Option<String>,
    cap_tier: &'static str,
    sector: Option<String>,
    shares: f64,
    cost: f64,
    value: Option<f64>,
    value_base: Option<f64>,
    weight_percent: Option<f64>,
    pnl: Option<f64>,
    pnl_percent: Option<f64>,
    day_change_percent: Option<f64>,
    xirr_percent: Option<f64>,
    short_term_gain: Option<f64>,
    long_term_gain: Option<f64>,
    next_long_term_date: Option<chrono::NaiveDate>,
    next_long_term_days: Option<i64>,
    vs_200_day_percent: Option<f64>,
    range_position_percent: Option<f64>,
    volatility_percent: Option<f64>,
    max_drawdown_percent: Option<f64>,
    beta: Option<f64>,
    score: Option<f64>,
    observations: usize,
}

impl CsvRow for PositionCsv {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "market",
        "currency",
        "cap_tier",
        "sector",
        "shares",
        "cost",
        "value",
        "value_base",
        "weight_percent",
        "pnl",
        "pnl_percent",
        "day_change_percent",
        "xirr_percent",
        "short_term_gain",
        "long_term_gain",
        "next_long_term_date",
        "next_long_term_days",
        "vs_200_day_percent",
        "range_position_percent",
        "volatility_percent",
        "max_drawdown_percent",
        "beta",
        "score",
        "observations",
    ];
}

/// One slice of `portfolio analyze --csv --groups`: the market, currency,
/// cap-tier and sector breakdowns in one table.
#[derive(Debug, Serialize)]
pub(crate) struct GroupCsv {
    grouping: &'static str,
    name: String,
    holdings: usize,
    value_base: f64,
    weight_percent: f64,
    pnl_base: f64,
    local_value: Option<f64>,
    local_currency: Option<String>,
    base_currency: String,
}

impl CsvRow for GroupCsv {
    const COLUMNS: &'static [&'static str] = &[
        "grouping",
        "name",
        "holdings",
        "value_base",
        "weight_percent",
        "pnl_base",
        "local_value",
        "local_currency",
        "base_currency",
    ];
}

impl GroupCsv {
    #[cfg(test)]
    pub(crate) fn grouping_name(&self) -> &'static str {
        self.grouping
    }
}

/// Every breakdown of an analysis as CSV rows, by grouping then largest first.
pub(crate) fn group_csv_rows(a: &Analysis) -> Vec<GroupCsv> {
    [
        ("market", &a.by_market),
        ("currency", &a.by_currency),
        ("cap_tier", &a.by_cap_tier),
        ("sector", &a.by_sector),
    ]
    .into_iter()
    .flat_map(|(grouping, groups)| {
        groups.iter().map(move |g| GroupCsv {
            grouping,
            name: g.name.clone(),
            holdings: g.holdings,
            value_base: g.value_base,
            weight_percent: g.weight_pct,
            pnl_base: g.pnl_base,
            local_value: g.local_value,
            local_currency: g.local_currency.clone(),
            base_currency: a.base_currency.clone(),
        })
    })
    .collect()
}

/// The holdings of an analysis as CSV rows, largest first.
pub(crate) fn csv_rows(a: &Analysis) -> Vec<PositionCsv> {
    let mut rows: Vec<PositionCsv> = a
        .positions
        .iter()
        .map(|p| PositionCsv {
            symbol: p.symbol.clone(),
            name: p.name.clone(),
            market: p.market.clone(),
            currency: p.currency.clone(),
            cap_tier: p.cap_tier,
            sector: p.sector.clone(),
            shares: p.shares,
            cost: p.cost,
            value: p.value,
            value_base: p.value_base,
            weight_percent: p.weight_pct,
            pnl: p.pnl,
            pnl_percent: p.pnl_pct,
            day_change_percent: p.day_change_pct,
            xirr_percent: p.xirr_pct,
            short_term_gain: p.short_term_gain,
            long_term_gain: p.long_term_gain,
            next_long_term_date: p.next_long_term.as_ref().map(|n| n.date),
            next_long_term_days: p.next_long_term.as_ref().map(|n| n.days),
            vs_200_day_percent: p.vs_200_day_pct,
            range_position_percent: p.range_position_pct,
            volatility_percent: p.volatility_pct,
            max_drawdown_percent: p.max_drawdown_pct,
            beta: p.beta,
            score: p.score,
            observations: a
                .observations
                .iter()
                .filter(|o| o.symbol.as_deref() == Some(p.symbol.as_str()))
                .count(),
        })
        .collect();
    rows.sort_by(|x, y| {
        y.value_base
            .unwrap_or(-1.0)
            .total_cmp(&x.value_base.unwrap_or(-1.0))
    });
    rows
}

/// Whose holding-period rules apply to a symbol, from its currency.
fn tax_country(currency: Option<&str>) -> Option<Country> {
    match currency? {
        "INR" => Some(Country::India),
        "USD" => Some(Country::Us),
        "GBP" | "GBp" => Some(Country::Uk),
        _ => None,
    }
}

/// The country whose rules value the whole portfolio's tax: the market's.
fn home_country(market: Market) -> Option<Country> {
    match market {
        Market::In => Some(Country::India),
        Market::Us => Some(Country::Us),
        Market::Global(market::Exchange::Uk) => Some(Country::Uk),
        _ => None,
    }
}

/// Daily closes with their session date (in the market's own time zone,
/// from the chart's UTC offset), gaps dropped.
fn closes_of(chart: &crate::api::ChartData) -> Vec<(chrono::NaiveDate, f64)> {
    let offset = chart.meta.as_ref().and_then(|m| m.gmtoffset).unwrap_or(0);
    let stamps = chart.timestamp.as_deref().unwrap_or_default();
    let closes = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_deref())
        .unwrap_or_default();
    stamps
        .iter()
        .zip(closes)
        .filter_map(|(t, c)| {
            let day = chrono::DateTime::from_timestamp(t + offset, 0)?.date_naive();
            Some((day, (*c)?))
        })
        .collect()
}

/// Sectors of `symbols`: stored snapshots first, then (with `client`) one
/// profile lookup each for the rest, at most [`SECTOR_LIMIT`].
async fn sectors(
    client: Option<&YahooClient>,
    symbols: &[String],
) -> std::collections::HashMap<String, String> {
    let mut found: std::collections::HashMap<String, String> =
        crate::db::with_db(|d| d.snapshot_sectors(symbols))
            .unwrap_or_default()
            .into_iter()
            .collect();
    let Some(client) = client else { return found };
    let missing: Vec<String> = symbols
        .iter()
        .filter(|s| !found.contains_key(*s) && !offmarket::is_fund(s))
        .take(SECTOR_LIMIT)
        .cloned()
        .collect();
    let looked: Vec<(String, Option<String>)> =
        futures_util::stream::iter(missing.into_iter().map(|sym| async move {
            let mut q = crate::api::Quote {
                symbol: Some(sym.clone()),
                ..Default::default()
            };
            crate::research::enrich_quote(client, &mut q, true).await;
            (sym, q.sector)
        }))
        .buffered(4)
        .collect()
        .await;
    for (sym, sector) in looked {
        if let Some(s) = sector.filter(|s| !s.is_empty()) {
            found.insert(sym, s);
        }
    }
    found
}

/// The inputs of an analysis, kept for callers that need more than the
/// dashboard (`portfolio checkup` reads the closes for correlations).
pub(crate) struct Gathered {
    pub(crate) portfolio: Portfolio,
    pub(crate) val: Valuation,
    pub(crate) inputs: Vec<PositionInput>,
    pub(crate) ctx: pa::Context,
}

/// Gather everything and analyse the active portfolio. `quick` skips the
/// per-holding price history and sector lookups (no risk figures).
pub(crate) async fn load_analysis(
    src: &impl views::DataSource,
    client: Option<&YahooClient>,
    market: Market,
    base: Option<&str>,
    quick: bool,
) -> Result<AnalyzeReport> {
    let g = gather_inputs(src, client, market, base, quick).await?;
    Ok(analysis_report(&g, market))
}

/// Prices, closes, sectors and scores of the active portfolio's holdings,
/// ready for [`pa::analyze`].
pub(crate) async fn gather_inputs(
    src: &impl views::DataSource,
    client: Option<&YahooClient>,
    market: Market,
    base: Option<&str>,
    quick: bool,
) -> Result<Gathered> {
    let portfolio = src.portfolio().map_err(|e| e.context(views::LocalData))?;
    let today = chrono::Local::now().date_naive();
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(src, &symbols, market, base).await?;
    let profile = crate::config::effective_profile();

    // A year of daily closes per holding (off-market funds have none).
    let charted: Vec<String> = symbols
        .iter()
        .filter(|s| !offmarket::is_fund(s) && !offmarket::is_sgb(s))
        .take(if quick { 0 } else { CHART_LIMIT })
        .cloned()
        .collect();
    let closes: std::collections::HashMap<String, Vec<(chrono::NaiveDate, f64)>> =
        futures_util::stream::iter(charted.into_iter().map(|sym| async move {
            let c = src
                .chart(&sym, "1y", "1d")
                .await
                .map(|c| closes_of(&c))
                .unwrap_or_default();
            (sym, c)
        }))
        .buffered(6)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect();
    let (bench_symbol, bench_name) = match market {
        Market::In => ("^NSEI", "NIFTY 50"),
        Market::Us => ("^GSPC", "S&P 500"),
        Market::Global(x) => x.info().benchmark(),
        _ => ("^GSPC", "S&P 500"),
    };
    let benchmark_closes = if quick {
        Vec::new()
    } else {
        src.chart(bench_symbol, "1y", "1d")
            .await
            .map(|c| closes_of(&c))
            .unwrap_or_default()
    };
    let sector_of = if quick {
        crate::db::with_db(|d| d.snapshot_sectors(&symbols))
            .unwrap_or_default()
            .into_iter()
            .collect()
    } else {
        sectors(client, &symbols).await
    };

    let inputs: Vec<PositionInput> = portfolio
        .holdings
        .iter()
        .map(|h| {
            let sym = &h.symbol;
            let q = val.quote(sym);
            let price = val.price(sym);
            let value = price.map(|p| h.shares * p);
            let currency = val.currency(sym);
            let hist = closes.get(sym).cloned().unwrap_or_default();
            let plain: Vec<f64> = hist.iter().map(|(_, c)| *c).collect();
            let score = q.map(|q| {
                crate::longterm::score_for_longterm(
                    q,
                    (!plain.is_empty()).then_some(plain.as_slice()),
                )
            });
            let market_label = if offmarket::is_fund(sym) || offmarket::is_sgb(sym) {
                Market::In.label().to_string()
            } else {
                market::market_of_symbol(sym, market).label().to_string()
            };
            PositionInput {
                symbol: sym.clone(),
                name: val.name(sym),
                market: market_label,
                tax_country: tax_country(currency.as_deref()),
                currency,
                cap_tier: Some(val.cap_tier(sym)),
                sector: sector_of
                    .get(sym)
                    .cloned()
                    .or_else(|| offmarket::is_fund(sym).then(|| "Mutual fund".to_string())),
                shares: h.shares,
                cost: h.cost_basis(),
                value,
                fx_to_base: val.to_base(sym, 1.0),
                price,
                day_change_pct: q.and_then(|q| q.regular_market_change_percent),
                fifty_day_average: q.and_then(|q| q.fifty_day_average),
                two_hundred_day_average: q.and_then(|q| q.two_hundred_day_average),
                fifty_two_week_low: q.and_then(|q| q.fifty_two_week_low),
                fifty_two_week_high: q.and_then(|q| q.fifty_two_week_high),
                xirr_pct: value
                    .and_then(|v| pf::xirr(&portfolio.cash_flows(sym), v, today))
                    .map(|r| r * 100.0),
                lots: h
                    .lots
                    .iter()
                    .map(|l| LotInput {
                        qty: l.qty,
                        price: l.price,
                        date: chrono::NaiveDate::parse_from_str(&l.date, "%Y-%m-%d").ok(),
                        estimated: l.date_estimated,
                    })
                    .collect(),
                closes: hist,
                score: score
                    .as_ref()
                    .filter(|s| s.total().is_some())
                    .map(|s| (s.total_score, s.data_coverage)),
                risk_tier: score.as_ref().map(|s| s.risk_tier.to_string()),
            }
        })
        .collect();
    let ctx = pa::Context {
        today,
        base_currency: val.base().to_string(),
        max_stock_weight: Some(profile.max_stock_weight),
        max_smallcap_weight: Some(profile.max_smallcap_weight),
        benchmark_closes,
        benchmark_name: bench_name.to_string(),
    };
    Ok(Gathered {
        portfolio,
        val,
        inputs,
        ctx,
    })
}

/// The dashboard for gathered inputs, with the tax a sale of everything
/// today would bring.
pub(crate) fn analysis_report(g: &Gathered, market: Market) -> AnalyzeReport {
    let (portfolio, val, today) = (&g.portfolio, &g.val, g.ctx.today);
    let analysis = pa::analyze(&g.inputs, &g.ctx);
    let tax_if_sold_today = home_country(market).map(|country| {
        let report = super::holdings::tax_report(country, portfolio, val, today);
        let e = tax::estimate(country, report.unrealized);
        TaxIfSold {
            country: match country {
                Country::India => "India",
                Country::Us => "United States",
                Country::Uk => "United Kingdom",
            },
            // `tax_report` works in the country's own currency, whatever
            // `--base` says.
            currency: country.tax_currency().to_string(),
            short_term_tax: e.st_tax,
            long_term_tax: e.lt_tax,
            cess: e.cess,
            total: e.st_tax + e.lt_tax + e.cess,
            exemption_used: e.exemption_used,
        }
    });
    AnalyzeReport {
        analysis,
        tax_if_sold_today,
        disclaimer: crate::verdict::DISCLAIMER,
    }
}

// ── Terminal dashboard ──

/// A bar of `width` cells for `pct` of 100.
fn bar(pct: f64, width: usize) -> String {
    let filled = ((pct.clamp(0.0, 100.0) / 100.0) * width as f64).round() as usize;
    format!(
        "{}{}",
        "█".repeat(filled),
        "░".repeat(width - filled.min(width))
    )
}

/// A 52-week range strip with a marker where the price sits.
fn range_strip(pos: Option<f64>) -> String {
    let Some(p) = pos else {
        return " ".repeat(9);
    };
    let at = ((p / 100.0) * 8.0).round() as usize;
    (0..9).map(|i| if i == at { '●' } else { '─' }).collect()
}

fn signed(v: f64, decimals: usize) -> String {
    format!("{:+.*}", decimals, v)
}

fn colour_signed(text: String, v: f64) -> String {
    if v > 0.0 {
        text.green().to_string()
    } else if v < 0.0 {
        text.red().to_string()
    } else {
        text
    }
}

fn opt_pct(v: Option<f64>) -> String {
    v.map_or("–".to_string(), |v| format!("{:.1}%", v))
}

fn opt_signed_pct(v: Option<f64>) -> String {
    match v {
        Some(v) => colour_signed(format!("{:+.1}%", v), v),
        None => "–".to_string(),
    }
}

/// The tax term of a holding in a few characters: `LT`, `ST 17d`,
/// `mix 52d` (lots on both sides; days until the next turns long-term).
pub(crate) fn term_cell(p: &pa::Position) -> String {
    let days = p.next_long_term.as_ref().map(|n| n.days);
    match (p.short_term_lots > 0, p.long_term_lots > 0, days) {
        (true, true, Some(d)) => format!("mix {}d", d),
        (true, false, Some(d)) => format!("ST {}d", d),
        (false, true, _) => "LT".to_string(),
        _ => "–".to_string(),
    }
}

fn print_groups(title: &str, groups: &[pa::Group], base: &str, local: bool) {
    if groups.is_empty() {
        return;
    }
    print_section(title);
    for g in groups {
        // The group's own-currency total, pence shown as pounds.
        let local_cell = match (&g.local_currency, g.local_value) {
            (Some(c), Some(v)) if local && c != base => {
                let (v, c) = market::to_major(v, c);
                format!("{:>16}", format_price(v, Some(&c)))
            }
            _ => " ".repeat(16),
        };
        println!(
            "  {} {} {:>5.1}%  {:>16}{}  {}  {}",
            pad_end(&truncate_name(&g.name, 22), 22),
            bar(g.weight_pct, 20).cyan(),
            g.weight_pct,
            format_price(g.value_base, Some(base)),
            local_cell,
            colour_signed(
                format!("{:>14}", format_price(g.pnl_base, Some(base))),
                g.pnl_base
            ),
            format!("{} holding(s)", g.holdings).dimmed()
        );
    }
}

fn truncate_name(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max - 1).collect();
        format!("{}…", t)
    }
}

/// Print the dashboard.
pub(crate) fn print_analysis(report: &AnalyzeReport) {
    let a = &report.analysis;
    let base = a.base_currency.as_str();
    print_header(&format!(
        "{} — Analysis",
        portfolio_title(t(Msg::YourPortfolio))
    ));

    // Summary.
    println!(
        "  {}  {}    {}  {} ({})    {}  {}",
        "Value".dimmed(),
        format_price(a.value, Some(base)).bold(),
        "P&L".dimmed(),
        colour_signed(format_price(a.pnl, Some(base)), a.pnl),
        colour_signed(signed(a.pnl_pct, 2) + "%", a.pnl_pct),
        "Today".dimmed(),
        match a.day_change_pct {
            Some(p) => colour_signed(
                format!("{} ({:+.2}%)", format_price(a.day_change, Some(base)), p),
                p,
            ),
            None => "–".to_string(),
        }
    );
    println!(
        "  {}  {}    {}  {}    {}  {:.1}",
        "Cost".dimmed(),
        format_price(a.cost, Some(base)),
        "Holdings".dimmed(),
        a.positions.len(),
        "Effective holdings".dimmed(),
        a.concentration.effective_holdings
    );

    // Holdings.
    print_section("Holdings");
    println!(
        "  {} {:>7} {:>14} {:>8} {:>7} {:>8} {:>7} {}  {:>6} {:>5} {:>3}",
        pad_end("Symbol", 14).bold(),
        "Weight".bold(),
        format!("Value ({})", base).bold(),
        "P&L%".bold(),
        "Today".bold(),
        "Term".bold(),
        "vs 200d".bold(),
        pad_end("52w range", 9).bold(),
        "Vol".bold(),
        "Score".bold(),
        "Obs".bold()
    );
    println!("  {}", "─".repeat(104).dimmed());
    let mut rows: Vec<&pa::Position> = a.positions.iter().collect();
    rows.sort_by(|x, y| {
        y.value_base
            .unwrap_or(-1.0)
            .total_cmp(&x.value_base.unwrap_or(-1.0))
    });
    for p in rows {
        let obs = a
            .observations
            .iter()
            .filter(|o| o.symbol.as_deref() == Some(&p.symbol))
            .count();
        let notices = a
            .observations
            .iter()
            .any(|o| o.symbol.as_deref() == Some(&p.symbol) && o.level == Level::Notice);
        let obs_cell = match obs {
            0 => "".to_string(),
            n if notices => format!("●{}", n).yellow().to_string(),
            n => format!("○{}", n).dimmed().to_string(),
        };
        println!(
            "  {} {:>7} {:>14} {:>8} {:>7} {:>8} {:>7} {}  {:>6} {:>5} {:>3}",
            pad_end(&truncate_name(&p.symbol, 14), 14).cyan(),
            opt_pct(p.weight_pct),
            p.value_base
                .map_or("–".to_string(), |v| format_price(v, Some(base))),
            opt_signed_pct(p.pnl_pct),
            opt_signed_pct(p.day_change_pct),
            term_cell(p),
            opt_signed_pct(p.vs_200_day_pct),
            range_strip(p.range_position_pct).dimmed(),
            p.volatility_pct
                .map_or("–".to_string(), |v| format!("{:.0}%", v)),
            p.score.map_or("–".to_string(), |s| format!("{:.0}", s)),
            obs_cell
        );
    }
    println!(
        "  {}",
        "Term: LT long-term, ST short-term (days until the next lot turns long-term), mix both. Vol: yearly volatility."
            .dimmed()
    );

    print_groups("By market", &a.by_market, base, true);
    print_groups("By currency", &a.by_currency, base, true);
    print_groups("By cap tier", &a.by_cap_tier, base, false);
    print_groups("By sector", &a.by_sector, base, false);

    // Tax terms.
    let tm = &a.terms;
    print_section("Short-term and long-term");
    print_kv(
        "Short-term unrealised gain",
        &colour_signed(
            format_price(tm.short_term_gain, Some(base)),
            tm.short_term_gain,
        ),
    );
    print_kv(
        "Long-term unrealised gain",
        &colour_signed(
            format_price(tm.long_term_gain, Some(base)),
            tm.long_term_gain,
        ),
    );
    print_kv(
        "Lots",
        &format!(
            "{} short-term, {} long-term{}",
            tm.short_term_lots,
            tm.long_term_lots,
            if tm.undated_lots > 0 {
                format!(", {} without a date", tm.undated_lots)
            } else {
                String::new()
            }
        ),
    );
    let mut soon: Vec<&pa::Position> = a
        .positions
        .iter()
        .filter(|p| p.next_long_term.is_some())
        .collect();
    soon.sort_by_key(|p| p.next_long_term.as_ref().map(|n| n.date));
    for p in soon.iter().take(5) {
        let n = p.next_long_term.as_ref().expect("filtered");
        print_kv(
            &format!("  {}", p.symbol),
            &format!(
                "{} share(s) long-term on {} (in {} days)",
                n.qty,
                n.date.format("%d %b %Y"),
                n.days
            ),
        );
    }
    if let Some(tx) = &report.tax_if_sold_today {
        print_kv(
            "Tax if all were sold today",
            &format!(
                "{} ({} rules; short-term {}, long-term {}{})",
                format_price(tx.total, Some(&tx.currency)),
                tx.country,
                format_price(tx.short_term_tax, Some(&tx.currency)),
                format_price(tx.long_term_tax, Some(&tx.currency)),
                if tx.cess > 0.0 {
                    format!(", cess {}", format_price(tx.cess, Some(&tx.currency)))
                } else {
                    String::new()
                }
            ),
        );
        println!(
            "  {}",
            "Standard rates, before this year's other gains and losses; an estimate, not a tax figure."
                .dimmed()
        );
    }

    // Risk and concentration.
    let r = &a.risk;
    let c = &a.concentration;
    print_section("Risk and concentration");
    print_kv(
        "Yearly volatility",
        &r.volatility_pct
            .map_or("–".to_string(), |v| format!("{:.1}%", v)),
    );
    print_kv(
        "Largest fall, last year",
        &r.max_drawdown_pct
            .map_or("–".to_string(), |v| format!("{:.1}%", v)),
    );
    print_kv(
        &format!("Beta against {}", r.benchmark),
        &r.beta.map_or("–".to_string(), |b| format!("{:.2}", b)),
    );
    print_kv(
        "Price history covers",
        &format!(
            "{:.0}% of the value, {} sessions",
            r.coverage_pct, r.sessions
        ),
    );
    if let Some(s) = r.weighted_score {
        print_kv("Weighted long-term score", &format!("{:.0} of 100", s));
    }
    print_kv(
        "Largest holding",
        &format!(
            "{} {:.1}%",
            c.largest.as_deref().unwrap_or("–"),
            c.largest_pct
        ),
    );
    print_kv("Top five holdings", &format!("{:.1}%", c.top5_pct));
    print_kv(
        "Effective holdings",
        &format!(
            "{:.1} (Herfindahl {:.3})",
            c.effective_holdings, c.herfindahl
        ),
    );

    // Observations.
    print_section(&format!("Observations ({})", a.observations.len()));
    if a.observations.is_empty() {
        println!("  {}", "Nothing to note.".dimmed());
    }
    for o in &a.observations {
        let mark = match o.level {
            Level::Notice => "●".yellow().to_string(),
            Level::Info => "○".dimmed().to_string(),
        };
        let who = o
            .symbol
            .as_deref()
            .map_or(String::new(), |s| format!("{} ", s.cyan().bold()));
        println!("  {} {}{}", mark, who, o.text);
    }
    println!("\n  {}", report.disclaimer.dimmed().italic());
    println!();
}

pub async fn cmd_portfolio_analyze(
    market: Market,
    base: Option<&str>,
    quick: bool,
    groups: bool,
    fmt: Format,
) -> Result<()> {
    let json = !fmt.is_table();
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        bail!("{}", t(Msg::PortfolioEmpty));
    }
    let client = YahooClient::new().await?;
    let src = views::LiveSource::new();
    if !json {
        eprintln!(
            "  {} Analysing {} holdings{}...",
            "⟳".yellow(),
            portfolio.holdings.len(),
            if quick {
                ""
            } else {
                " (prices, a year of closes, sectors)"
            }
        );
    }
    let report = load_analysis(&src, Some(&client), market, base, quick).await?;
    match fmt {
        Format::Json => return output::print_json(&report),
        Format::Csv if groups => return output::print_csv(&group_csv_rows(&report.analysis)),
        Format::Csv => return output::print_csv(&csv_rows(&report.analysis)),
        Format::Table => {}
    }
    print_analysis(&report);
    Ok(())
}
