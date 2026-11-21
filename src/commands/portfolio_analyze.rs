//! `portfolio analyze`: gathers the inputs of [`crate::portfolio_analysis`]
//! (prices, a year of closes, sectors, scores, tax rules, the profile's
//! limits) and prints the dashboard. The TUI's portfolio screen uses the
//! same loader.

use super::*;
use crate::offmarket;
use crate::portfolio as pf;
use crate::portfolio_analysis::{self as pa, Analysis, Level, LotInput, PositionInput, TermRule};
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
    /// Splits or bonus issues not yet applied to the lots (the analysis
    /// counts the adjusted shares), and costs converted at today's rate.
    pub warnings: Vec<String>,
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
    /// Holdings in another currency than the tax currency, taxed under the
    /// residence's rules for foreign assets: not in the totals above.
    pub foreign: Vec<ForeignTaxRow>,
    /// Holdings taxed under other rules (debt funds, SGBs, gold ETFs,
    /// crypto, funds of unknown category): not in the totals above.
    pub other_rules: Vec<OtherRuleRow>,
    /// Holdings without a live price: not in the totals above.
    pub unpriced: Vec<String>,
    /// ELSS units still in their three-year lock-in: in the figure above,
    /// but they cannot be redeemed before `unlocks_on`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub locked: Vec<LockedRow>,
}

/// ELSS units bought less than three years ago (each purchase, SIP
/// instalments too, is locked for three years from its own date).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LockedRow {
    pub symbol: String,
    pub units: f64,
    /// The day the earliest of these units can be redeemed.
    pub unlocks_on: String,
    /// The day the last of them can.
    pub all_unlocked_on: String,
}

/// An ELSS (tax-saver) fund, by its scheme name.
pub(crate) fn is_elss(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("elss") || n.contains("tax saver") || n.contains("taxsaver")
}

/// The ELSS lots of `portfolio` still in their three-year lock-in on `today`.
pub(crate) fn elss_locked(
    portfolio: &Portfolio,
    name_of: impl Fn(&str) -> Option<String>,
    today: chrono::NaiveDate,
) -> Vec<LockedRow> {
    let mut out = Vec::new();
    for h in &portfolio.holdings {
        if !name_of(&h.symbol).is_some_and(|n| is_elss(&n)) {
            continue;
        }
        let unlocks: Vec<(chrono::NaiveDate, f64)> = h
            .lots
            .iter()
            .filter_map(|l| {
                let d = pf::parse_date(&l.date).ok()?;
                let free = d.checked_add_months(chrono::Months::new(36))?;
                (free > today).then_some((free, l.qty))
            })
            .collect();
        let (Some(first), Some(last)) = (
            unlocks.iter().map(|u| u.0).min(),
            unlocks.iter().map(|u| u.0).max(),
        ) else {
            continue;
        };
        out.push(LockedRow {
            symbol: h.symbol.clone(),
            units: unlocks.iter().map(|u| u.1).sum(),
            unlocks_on: first.to_string(),
            all_unlocked_on: last.to_string(),
        });
    }
    out
}

/// A foreign holding in "tax if sold": its gain in the tax currency (cost
/// at the purchase dates' exchange rates), when the analysis is in that
/// currency, and a rough tax at the foreign-asset rates.
#[derive(Debug, Serialize)]
pub struct ForeignTaxRow {
    pub symbol: String,
    pub currency: Option<String>,
    /// Months to turn long-term under the residence's rules.
    pub long_term_months: Option<u32>,
    pub short_term_gain: Option<f64>,
    pub long_term_gain: Option<f64>,
    pub estimated_tax: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct OtherRuleRow {
    pub symbol: String,
    pub reason: String,
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

/// The country a currency's own market belongs to (used when no country
/// of residence is known).
fn currency_country(currency: Option<&str>) -> Option<Country> {
    match currency? {
        "INR" => Some(Country::India),
        "USD" => Some(Country::Us),
        "GBP" | "GBp" => Some(Country::Uk),
        _ => None,
    }
}

/// The investor's country of residence, whose rules tax every holding: the
/// configured default market (`market = "in"` in the config), else the
/// command's market.
pub(crate) fn residence(market: Market) -> Option<Country> {
    if let Some(c) = crate::config::get()
        .market
        .as_deref()
        .and_then(Country::parse)
    {
        return Some(c);
    }
    match market {
        Market::In => Some(Country::India),
        Market::Us => Some(Country::Us),
        Market::Global(market::Exchange::Uk) => Some(Country::Uk),
        _ => None,
    }
}

/// Whose rules and which holding period apply to `symbol` for a resident
/// of `residence`. An Indian resident: listed shares and equity funds
/// 12 months; foreign shares 24 months (Sec. 112, unlisted/foreign);
/// debt funds bought from 1 April 2023 never long-term (Sec. 50AA), older
/// units 24 months; crypto never long-term. A US resident pays US tax on
/// everything, 12 months. Without a residence, the currency's market rules.
pub(crate) fn term_rule(
    residence: Option<Country>,
    symbol: &str,
    currency: Option<&str>,
    mark: Option<&crate::offmarket::Mark>,
) -> (Option<Country>, TermRule) {
    let Some(home) = residence else {
        return (currency_country(currency), TermRule::CountryDefault);
    };
    let local = home.currency_scale(currency.unwrap_or("")).is_some();
    match home {
        Country::India if !local => (Some(home), TermRule::Months(24)),
        Country::India => {
            use crate::offmarket::IndiaClass;
            let rule = match crate::offmarket::india_class(symbol, mark) {
                IndiaClass::Vda => TermRule::NoLongTerm,
                IndiaClass::Other(_)
                    if offmarket::is_fund(symbol)
                        && mark.and_then(|m| m.category.as_deref()).is_some() =>
                {
                    TermRule::DebtFund
                }
                _ => TermRule::CountryDefault,
            };
            (Some(home), rule)
        }
        _ => (Some(home), TermRule::CountryDefault),
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
    /// Splits not yet applied to the file's lots (`portfolio` has them applied).
    pub(crate) pending: crate::corp_actions::PendingSplits,
    /// The investor's country of residence (whose tax rules apply).
    pub(crate) residence: Option<Country>,
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
    let loaded = src.portfolio().map_err(|e| e.context(views::LocalData))?;
    // A split or bonus not yet applied to the lots would show as a fake
    // loss: analyse a copy with it applied, and say so.
    let pending = match client {
        Some(c) if !cfg!(test) => crate::corp_actions::pending_splits(c, &loaded).await,
        _ => crate::corp_actions::PendingSplits::offline(&loaded),
    };
    let portfolio = pending.adjusted(&loaded);
    let today = chrono::Local::now().date_naive();
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let mut val = value_holdings(src, &symbols, market, base).await?;
    if client.is_some() {
        val.load_history(&portfolio).await;
    }
    let profile = crate::config::effective_profile();
    let home = residence(market);

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
    // Daily rates for a year of closes in other currencies than the base,
    // so risk is measured in the base currency.
    let base_cur = val.base().to_string();
    let year_ago = today - chrono::Duration::days(400);
    let mut fx_needs: Vec<(String, chrono::NaiveDate)> = symbols
        .iter()
        .filter(|s| closes.contains_key(*s))
        .filter_map(|s| val.currency(s))
        .map(|c| (c, year_ago))
        .collect();
    let regions = benchmark_regions(&portfolio, &val, market);
    fx_needs.extend(regions.iter().map(|r| (r.currency.to_string(), year_ago)));
    let daily_fx = if quick || client.is_none() {
        crate::fx_history::FxHistory::new(&base_cur)
    } else {
        crate::fx_history::FxHistory::load(&base_cur, &fx_needs).await
    };
    let (benchmark_closes, bench_name) = if quick {
        (
            Vec::new(),
            regions
                .first()
                .map_or(String::new(), |r| r.name.to_string()),
        )
    } else {
        let mut parts = Vec::new();
        for r in &regions {
            let c = src
                .chart(r.symbol, "1y", "1d")
                .await
                .map(|c| closes_of(&c))
                .unwrap_or_default();
            let c = in_base(&c, r.currency, &daily_fx);
            if !c.is_empty() {
                parts.push((r, c));
            }
        }
        match parts.as_slice() {
            [] => (
                Vec::new(),
                regions
                    .first()
                    .map_or(String::new(), |r| r.name.to_string()),
            ),
            [(r, c)] => (c.clone(), r.name.to_string()),
            _ => {
                let total: f64 = parts.iter().map(|(r, _)| r.weight).sum();
                let name = parts
                    .iter()
                    .map(|(r, _)| format!("{:.0}% {}", r.weight / total * 100.0, r.name))
                    .collect::<Vec<_>>()
                    .join(" + ");
                let blend: Vec<(Vec<(chrono::NaiveDate, f64)>, f64)> =
                    parts.into_iter().map(|(r, c)| (c, r.weight)).collect();
                (pa::blend_benchmarks(&blend), name)
            }
        }
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
            let closes_base = match currency.as_deref() {
                Some(c) if !c.eq_ignore_ascii_case(&base_cur) => in_base(&hist, c, &daily_fx),
                _ => Vec::new(),
            };
            let (tax_country, term_rule) = term_rule(home, sym, currency.as_deref(), val.mark(sym));
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
                tax_country,
                term_rule,
                cost_base: val.holding_cost_base(h).map(|(c, _)| c),
                closes_base,
                currency,
                cap_tier: Some(val.cap_tier(sym)),
                sector: sector_of.get(sym).cloned().or_else(|| {
                    offmarket::is_fund(sym).then(|| {
                        crate::xray::fund_sector_label(
                            sym,
                            val.name(sym).as_deref(),
                            val.mark(sym).and_then(|m| m.category.as_deref()),
                        )
                    })
                }),
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
                pe: q.and_then(|q| q.trailing_pe),
                pb: q.and_then(|q| q.price_to_book),
                dividend_yield: q.and_then(|q| q.dividend_yield_fraction()),
                // In the base currency, each flow at its own day's rate. Not
                // shown when a purchase date is a guess.
                xirr_pct: value
                    .and_then(|v| val.to_base(sym, v))
                    .filter(|_| !h.has_estimated_dates())
                    .and_then(|v| pf::xirr(&val.flows_base(&portfolio, sym)?, v, today))
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
        benchmark_name: bench_name,
    };
    Ok(Gathered {
        portfolio,
        val,
        inputs,
        ctx,
        pending,
        residence: home,
    })
}

/// One index of a (possibly blended) benchmark.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Region {
    pub(crate) symbol: &'static str,
    pub(crate) name: &'static str,
    /// The currency the index is quoted in.
    pub(crate) currency: &'static str,
    /// Base-currency value of the holdings it stands for.
    pub(crate) weight: f64,
}

/// The index for holdings in `currency`, else `None`.
fn index_for(currency: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match currency {
        "INR" => Some(("^NSEI", "NIFTY 50", "INR")),
        "USD" => Some(("^GSPC", "S&P 500", "USD")),
        "GBP" | "GBp" => Some(("^FTSE", "FTSE 100", "GBP")),
        _ => None,
    }
}

/// The benchmark indices for the portfolio, largest first: one per market
/// its holdings trade in, weighted by their value in the base currency
/// (markets under 5% of the value fold into the others). A single market,
/// or none priced, gives the market's own index.
pub(crate) fn benchmark_regions(
    portfolio: &Portfolio,
    val: &Valuation,
    market: Market,
) -> Vec<Region> {
    let mut regions: Vec<Region> = Vec::new();
    let mut total = 0.0;
    for h in &portfolio.holdings {
        let Some(v) = val
            .price(&h.symbol)
            .and_then(|p| val.to_base(&h.symbol, h.shares * p))
        else {
            continue;
        };
        let Some((symbol, name, currency)) = val.currency(&h.symbol).as_deref().and_then(index_for)
        else {
            continue;
        };
        total += v;
        match regions.iter_mut().find(|r| r.symbol == symbol) {
            Some(r) => r.weight += v,
            None => regions.push(Region {
                symbol,
                name,
                currency,
                weight: v,
            }),
        }
    }
    regions.retain(|r| total > 0.0 && r.weight / total >= 0.05);
    regions.sort_by(|a, b| b.weight.total_cmp(&a.weight));
    if regions.is_empty() {
        let (symbol, name) = match market {
            Market::In => ("^NSEI", "NIFTY 50"),
            Market::Us => ("^GSPC", "S&P 500"),
            Market::Global(x) => x.info().benchmark(),
            _ => ("^GSPC", "S&P 500"),
        };
        regions.push(Region {
            symbol,
            name,
            currency: market_currency(market),
            weight: 1.0,
        });
    }
    regions
}

/// `closes` in `currency` converted to `fx`'s base at each day's rate;
/// days without a rate are dropped, and the whole series when over a tenth
/// of them have none.
pub(crate) fn in_base(
    closes: &[(chrono::NaiveDate, f64)],
    currency: &str,
    fx: &crate::fx_history::FxHistory,
) -> Vec<(chrono::NaiveDate, f64)> {
    let out: Vec<(chrono::NaiveDate, f64)> = closes
        .iter()
        .filter_map(|(d, c)| fx.convert_on(*c, currency, *d).map(|v| (*d, v)))
        .collect();
    if (out.len() as f64) < closes.len() as f64 * 0.9 {
        return Vec::new();
    }
    out
}

/// The dashboard for gathered inputs, with the tax a sale of everything
/// today would bring.
pub(crate) fn analysis_report(g: &Gathered, _market: Market) -> AnalyzeReport {
    let (portfolio, val, today) = (&g.portfolio, &g.val, g.ctx.today);
    let analysis = pa::analyze(&g.inputs, &g.ctx);
    let tax_if_sold_today = g.residence.map(|country| {
        let report = super::holdings::tax_report(country, portfolio, val, today);
        let e = tax::estimate(country, report.unrealized);
        let foreign = foreign_rows(country, portfolio, &g.inputs, val, today);
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
            foreign,
            other_rules: report
                .other_rules
                .into_iter()
                .filter(|o| portfolio.holding(&o.symbol).is_some())
                .map(|o| OtherRuleRow {
                    symbol: o.symbol,
                    reason: o.reason,
                })
                .collect(),
            unpriced: report.unpriced,
            locked: if country == Country::India {
                elss_locked(portfolio, |s| val.name(s), today)
            } else {
                Vec::new()
            },
        }
    });
    let mut warnings = g.pending.warnings();
    let totals = val.totals(portfolio);
    if !totals.cost_at_today_fx.is_empty() {
        warnings.push(format!(
            "No exchange rate for the purchase dates of {}: their cost is converted at today's rate.",
            totals.cost_at_today_fx.join(", ")
        ));
    }
    AnalyzeReport {
        analysis,
        tax_if_sold_today,
        warnings,
        disclaimer: crate::verdict::DISCLAIMER,
    }
}

/// The holdings "tax if sold" leaves out because they are in another
/// currency: their gains split by the residence's holding period (an
/// Indian resident's foreign shares turn long-term after 24 months), in
/// the tax currency when the analysis is in it, with a rough tax: India
/// 12.5% long-term and the profile's slab rate short-term; elsewhere the
/// country's own rates.
fn foreign_rows(
    country: Country,
    portfolio: &Portfolio,
    inputs: &[PositionInput],
    val: &Valuation,
    today: chrono::NaiveDate,
) -> Vec<ForeignTaxRow> {
    let in_tax_currency = val.base().eq_ignore_ascii_case(country.tax_currency());
    let slab = crate::config::effective_profile().tax_slab;
    let (st_rate, lt_rate) = match country {
        Country::India => (slab, 12.5),
        c => (c.st_rate(), c.lt_rate()),
    };
    inputs
        .iter()
        .filter(|i| {
            country
                .currency_scale(i.currency.as_deref().unwrap_or(""))
                .is_none()
        })
        .map(|i| {
            let months = i
                .term_rule
                .months_for(country.long_term_months(), today)
                .or(country.long_term_months());
            let gains = foreign_gains(portfolio.holding(&i.symbol), i.price, months, val, today)
                .filter(|_| in_tax_currency);
            ForeignTaxRow {
                symbol: i.symbol.clone(),
                currency: i.currency.clone(),
                long_term_months: months,
                short_term_gain: gains.map(|(st, _)| round(st, 2)),
                long_term_gain: gains.map(|(_, lt)| round(lt, 2)),
                estimated_tax: gains.map(|(st, lt)| {
                    round((st.max(0.0) * st_rate + lt.max(0.0) * lt_rate) / 100.0, 2)
                }),
            }
        })
        .collect()
}

/// Short- and long-term gain of a holding's lots in the base currency:
/// value at today's rate less each lot's cost at its purchase day's rate
/// (or what was paid in the base). `None` without a price, a rate or a
/// purchase date.
fn foreign_gains(
    h: Option<&pf::Holding>,
    price: Option<f64>,
    months: Option<u32>,
    val: &Valuation,
    today: chrono::NaiveDate,
) -> Option<(f64, f64)> {
    let (h, price) = (h?, price?);
    let (mut st, mut lt) = (0.0, 0.0);
    for l in &h.lots {
        let bought = pf::parse_date(&l.date).ok()?;
        let value = val.to_base(&h.symbol, l.qty * price)?;
        let (cost, _) = val.lot_cost_base(&h.symbol, l)?;
        if months.is_some_and(|m| tax::is_long_term(bought, today, m)) {
            lt += value - cost;
        } else {
            st += value - cost;
        }
    }
    Some((st, lt))
}

// ── Terminal dashboard ──

/// The holdings "tax if sold" does not count, each kind in its own list.
fn print_tax_left_out(tx: &TaxIfSold) {
    if !tx.foreign.is_empty() {
        println!(
            "  {}",
            format!(
                "Foreign holdings ({} rules for foreign assets), not in the figure above:",
                tx.country
            )
            .dimmed()
        );
        for f in &tx.foreign {
            let term = f
                .long_term_months
                .map_or("no long-term term".to_string(), |m| {
                    format!("long-term after {m} months")
                });
            match (f.short_term_gain, f.long_term_gain, f.estimated_tax) {
                (Some(st), Some(lt), Some(est)) => println!(
                    "    {} short-term {}, long-term {}, tax about {} ({})",
                    pad_end(&f.symbol, 14).cyan(),
                    format_price(st, Some(&tx.currency)),
                    format_price(lt, Some(&tx.currency)),
                    format_price(est, Some(&tx.currency)),
                    term
                ),
                _ => println!(
                    "    {} {} (run with --base {} for the gain)",
                    pad_end(&f.symbol, 14).cyan(),
                    term,
                    tx.currency
                ),
            }
        }
    }
    if !tx.other_rules.is_empty() {
        println!(
            "  {}",
            "Taxed under other rules, not in the figure above:".dimmed()
        );
        for o in &tx.other_rules {
            println!(
                "    {} {}",
                pad_end(&o.symbol, 14).cyan(),
                o.reason.dimmed()
            );
        }
    }
    if !tx.unpriced.is_empty() {
        println!(
            "  {} {}",
            "No live price, not in the figure above:".dimmed(),
            tx.unpriced.join(", ")
        );
    }
    if !tx.locked.is_empty() {
        println!(
            "  {}",
            "ELSS units in the three-year lock-in (in the figure above, but not redeemable yet):"
                .dimmed()
        );
        for l in &tx.locked {
            let when = if l.unlocks_on == l.all_unlocked_on {
                format!("from {}", l.unlocks_on)
            } else {
                format!("from {} (all by {})", l.unlocks_on, l.all_unlocked_on)
            };
            println!(
                "    {} {:.3} units, redeemable {}",
                pad_end(&l.symbol, 14).cyan(),
                l.units,
                when
            );
        }
    }
}

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
    for w in &report.warnings {
        println!("  {} {}", "!".yellow(), w);
    }

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
    let v = &a.valuation;
    if v.pe.is_some() || v.pb.is_some() {
        let cell = |x: Option<f64>, cover: f64| {
            x.map_or("–".to_string(), |x| {
                format!("{x:.1} ({cover:.0}% of value)")
            })
        };
        println!(
            "  {}  {}    {}  {}    {}  {}",
            "P/E".dimmed(),
            cell(v.pe, v.pe_coverage_pct),
            "P/B".dimmed(),
            cell(v.pb, v.pb_coverage_pct),
            "Dividend yield".dimmed(),
            v.dividend_yield_pct
                .map_or("–".to_string(), |y| format!("{y:.2}%"))
        );
        if !v.without_pe.is_empty() {
            println!(
                "  {}",
                format!(
                    "No trailing P/E (a trailing loss, usually) for {}: left out of the P/E.",
                    v.without_pe.join(", ")
                )
                .dimmed()
            );
        }
    }

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
        print_tax_left_out(tx);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elss_units_stay_locked_for_three_years_from_each_purchase() {
        let mut p = Portfolio::default();
        p.add_lot(
            "MF:120503",
            pf::Lot::new(10.0, 50.0, "2022-06-01", "manual"),
        );
        p.add_lot("MF:120503", pf::Lot::new(5.0, 60.0, "2024-01-10", "manual"));
        p.add_lot("MF:120503", pf::Lot::new(4.0, 70.0, "2025-03-05", "manual"));
        p.add_lot("MF:999999", pf::Lot::new(4.0, 70.0, "2025-03-05", "manual"));
        let names = |s: &str| {
            Some(if s == "MF:120503" {
                "Axis ELSS Tax Saver Fund - Direct Growth".to_string()
            } else {
                "Parag Parikh Flexi Cap Fund".to_string()
            })
        };
        let today = chrono::NaiveDate::from_ymd_opt(2026, 11, 21).unwrap();
        let locked = elss_locked(&p, names, today);
        assert_eq!(locked.len(), 1);
        assert_eq!(locked[0].units, 9.0);
        assert_eq!(locked[0].unlocks_on, "2027-01-10");
        assert_eq!(locked[0].all_unlocked_on, "2028-03-05");
        assert!(is_elss("Mirae Asset Tax Saver Fund"));
        assert!(!is_elss("Nippon India Small Cap"));
    }
    use crate::api::Quote;

    fn quote(symbol: &str, price: f64, currency: &str) -> Quote {
        Quote {
            symbol: Some(symbol.into()),
            regular_market_price: Some(price),
            currency: Some(currency.into()),
            ..Default::default()
        }
    }

    #[test]
    fn residence_decides_the_holding_period() {
        // An Indian resident: US shares 24 months, Indian shares the default.
        assert_eq!(
            term_rule(Some(Country::India), "AAPL", Some("USD"), None),
            (Some(Country::India), TermRule::Months(24))
        );
        assert_eq!(
            term_rule(Some(Country::India), "TCS.NS", Some("INR"), None),
            (Some(Country::India), TermRule::CountryDefault)
        );
        assert_eq!(
            term_rule(Some(Country::India), "BTC-INR", Some("INR"), None).1,
            TermRule::NoLongTerm
        );
        // A US resident's Indian shares follow US rules.
        assert_eq!(
            term_rule(Some(Country::Us), "TCS.NS", Some("INR"), None),
            (Some(Country::Us), TermRule::CountryDefault)
        );
        // No residence: the currency's market, as before.
        assert_eq!(
            term_rule(None, "AAPL", Some("USD"), None),
            (Some(Country::Us), TermRule::CountryDefault)
        );
    }

    #[test]
    fn a_debt_fund_with_a_category_gets_the_debt_fund_rule() {
        let mark = crate::offmarket::Mark {
            symbol: "MF:119551".into(),
            name: Some("Some Liquid Fund - Direct Plan".into()),
            source: crate::offmarket::PriceSource::AmfiNav,
            as_of: None,
            category: Some("Debt Scheme - Liquid Fund".into()),
        };
        assert_eq!(
            term_rule(Some(Country::India), "MF:119551", Some("INR"), Some(&mark)).1,
            TermRule::DebtFund
        );
    }

    #[test]
    fn mixed_portfolios_get_a_blended_benchmark() {
        let mut p = Portfolio::default();
        p.add_lot("TCS.NS", pf::Lot::new(10.0, 3000.0, "2024-01-10", "manual"));
        p.add_lot("AAPL", pf::Lot::new(10.0, 150.0, "2024-01-10", "manual"));
        let symbols: Vec<String> = p.holdings.iter().map(|h| h.symbol.clone()).collect();
        let mut val = Valuation::new(
            vec![quote("TCS.NS", 3000.0, "INR"), quote("AAPL", 150.0, "USD")],
            &symbols,
            Market::In,
            Some("INR"),
        );
        val.fx.set_rate("USD", 80.0);
        let r = benchmark_regions(&p, &val, Market::In);
        assert_eq!(r.len(), 2);
        // ₹30,000 of TCS and ₹1,20,000 of AAPL.
        assert_eq!((r[0].symbol, r[1].symbol), ("^GSPC", "^NSEI"));
        assert!((r[0].weight - 120_000.0).abs() < 1e-6);
        // One market only: its own index.
        let only_in = benchmark_regions(
            &p,
            &Valuation::new(
                vec![quote("TCS.NS", 3000.0, "INR")],
                &symbols,
                Market::In,
                Some("INR"),
            ),
            Market::In,
        );
        assert_eq!(only_in.len(), 1);
        assert_eq!(only_in[0].symbol, "^NSEI");
    }

    #[test]
    fn closes_convert_at_each_days_rate() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let mut fx = crate::fx_history::FxHistory::new("INR");
        fx.insert(
            "USD",
            vec![(d("2025-01-02"), 80.0), (d("2025-01-03"), 82.0)],
        );
        let closes = vec![(d("2025-01-02"), 10.0), (d("2025-01-03"), 10.0)];
        assert_eq!(
            in_base(&closes, "USD", &fx),
            vec![(d("2025-01-02"), 800.0), (d("2025-01-03"), 820.0)]
        );
        // Too few rates: nothing rather than a gappy series.
        assert!(in_base(&closes, "EUR", &fx).is_empty());
    }

    #[test]
    fn foreign_gains_use_the_purchase_day_rate() {
        let d = |s: &str| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let mut p = Portfolio::default();
        p.add_lot("AAPL", pf::Lot::new(10.0, 100.0, "2025-05-15", "manual"));
        let symbols = vec!["AAPL".to_string()];
        let mut val = Valuation::new(
            vec![quote("AAPL", 100.0, "USD")],
            &symbols,
            Market::In,
            Some("INR"),
        );
        val.fx.set_rate("USD", 84.0);
        val.hist.insert("USD", vec![(d("2025-05-15"), 75.0)]);
        // 18 months later, 24-month rule: all short-term, ₹9,000 from the rupee.
        let (st, lt) = foreign_gains(
            p.holding("AAPL"),
            Some(100.0),
            Some(24),
            &val,
            d("2026-11-15"),
        )
        .unwrap();
        assert!((st - 9000.0).abs() < 1e-6);
        assert_eq!(lt, 0.0);
    }
}
