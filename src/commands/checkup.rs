//! `portfolio checkup` and `portfolio wrap`: gather the inputs of
//! [`crate::checkup`] and [`crate::wrap`] from the `portfolio analyze`
//! loader, scorecards, the harvest and Regular-plan reports, goals and
//! targets, then print them.
//!
//! Network sources are read with a bounded number of requests at a time;
//! one that fails is named in the report and its checks are skipped, never
//! failing the whole report. `--offline` reads only the cache: stored
//! market snapshots for prices and stored daily bars for closes.

use super::*;
use crate::checkup::{self as ck, Checkup, CheckupInput, Fundamentals, HoldingFacts, Lens};
use crate::offmarket;
use crate::portfolio_analysis::{self as pa, Analysis};
use crate::tax::Country;
use crate::wrap::{self, Period, WrapReport};
use futures_util::StreamExt;
use std::time::Duration;

/// Most stocks whose statements are read, largest first.
const FUNDAMENTALS_LIMIT: usize = 30;
/// Statement and fund-profile requests at a time.
const CONCURRENCY: usize = 4;
/// Longest wait for one stock's statements or one fund's holdings.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);
/// Gross yearly return assumed for the Regular-plan cost (as `mf bleed`).
const BLEED_RETURN_PCT: f64 = 10.0;

// ── Offline data ──

/// Prices from the stored market snapshots and closes from the stored daily
/// bars: no request leaves the machine.
pub(crate) struct OfflineSource;

fn range_days(range: &str) -> i64 {
    match range {
        "1mo" => 31,
        "3mo" => 92,
        "6mo" => 183,
        "1y" => 366,
        "2y" => 731,
        "5y" => 1827,
        "10y" => 3653,
        "max" => 36_525,
        _ => 366,
    }
}

/// A chart of the bars stored for `symbol` over `range` back from `now`
/// (Unix seconds); an error when none are stored.
fn stored_chart(
    db: &crate::db::CacheDb,
    symbol: &str,
    range: &str,
    interval: &str,
    now: i64,
) -> Result<crate::api::ChartData> {
    let bars = db.get_bars(symbol, interval, now - range_days(range) * 86_400, now)?;
    if bars.is_empty() {
        bail!("no stored {interval} bars for {symbol}");
    }
    let chart = serde_json::json!({
        "meta": { "symbol": symbol },
        "timestamp": bars.iter().map(|b| b.ts).collect::<Vec<_>>(),
        "indicators": { "quote": [{ "close": bars.iter().map(|b| Some(b.close)).collect::<Vec<_>>() }] },
    });
    Ok(serde_json::from_value(chart)?)
}

impl views::DataSource for OfflineSource {
    async fn quotes(&self, symbols: &[&str]) -> Result<Vec<crate::api::Quote>> {
        let rows =
            crate::db::with_db(|d| d.query_snapshots(&Default::default())).unwrap_or_default();
        Ok(symbols
            .iter()
            .filter_map(|s| rows.iter().find(|r| r.symbol.eq_ignore_ascii_case(s)))
            .filter(|r| r.price.is_some())
            .map(|r| crate::api::Quote {
                symbol: Some(r.symbol.clone()),
                long_name: (!r.name.is_empty()).then(|| r.name.clone()),
                regular_market_price: r.price,
                regular_market_change_percent: r.change_1d,
                market_cap: r.mcap,
                trailing_pe: r.pe,
                currency: crate::portfolio::symbol_currency(&r.symbol).map(str::to_string),
                sector: r.sector.clone(),
                industry: r.industry.clone(),
                quote_type: Some("EQUITY".into()),
                ..Default::default()
            })
            .collect())
    }

    async fn chart(
        &self,
        symbol: &str,
        range: &str,
        interval: &str,
    ) -> Result<crate::api::ChartData> {
        let now = chrono::Utc::now().timestamp();
        crate::db::with_db(|d| stored_chart(d, symbol, range, interval, now))
            .ok_or_else(|| anyhow::anyhow!("no stored {interval} bars for {symbol} (offline)"))
    }

    async fn search(&self, _query: &str) -> Result<Vec<crate::api::SearchResult>> {
        Ok(Vec::new())
    }
}

// ── Checkup ──

/// What the checkup gathered beyond its findings, for `wrap`.
pub(crate) struct CheckupRun {
    pub(crate) checkup: Checkup,
    pub(crate) analysis: Analysis,
    pub(crate) tax: Option<ck::TaxFacts>,
}

fn home_country(market: Market) -> Option<Country> {
    match market {
        Market::In => Some(Country::India),
        Market::Us => Some(Country::Us),
        _ => None,
    }
}

fn is_fund_like(symbol: &str) -> bool {
    offmarket::is_fund(symbol) || crate::xray::is_known_etf(symbol)
}

/// A stock's checkup fundamentals from its research snapshot.
pub(crate) fn fundamentals_of(r: &crate::research::Research) -> Fundamentals {
    let p = crate::scorecard::calculate_piotroski(r);
    let a = crate::scorecard::calculate_altman(r);
    let (pat_yoy_pct, opm_change_pp) = ck::quarter_yoy(&r.quarters);
    let (revenue_yoy_pct, revenue_decelerating) = ck::revenue_trend(&r.quarters);
    let n = r.holdings.len();
    let promoter_change_pp = (n >= 2)
        .then(|| Some(r.holdings[n - 1].promoter? - r.holdings[n - 2].promoter?))
        .flatten();
    Fundamentals {
        piotroski: (p.evaluated > 0).then_some((p.score, p.evaluated)),
        altman_z: a.z_score,
        altman_distress: a.zone.map(|z| z == crate::scorecard::AltmanZone::Distress),
        cfo_to_pat: ck::cfo_to_pat(&r.annual),
        promoter_pledged: r.holdings.last().and_then(|h| h.pledged),
        promoter_change_pp,
        pat_yoy_pct,
        opm_change_pp,
        pe: r.fin.trailing_pe.or(r.quote.trailing_pe),
        peg: r.fin.peg,
        revenue_yoy_pct,
        revenue_decelerating,
    }
}

/// Statements and shareholding of `symbols`, a few at a time. Failures are
/// returned by name.
async fn fetch_fundamentals(
    client: &YahooClient,
    symbols: Vec<String>,
    market: Market,
) -> (Vec<(String, Fundamentals)>, Vec<String>, Vec<String>) {
    let needs = crate::research::Needs {
        statements: true,
        ownership: true,
        ..crate::research::Needs::NONE
    };
    let results: Vec<(String, Result<(Fundamentals, Vec<String>)>)> =
        futures_util::stream::iter(symbols.into_iter().map(|sym| async move {
            let m = market::market_of_symbol(&sym, market);
            let r = tokio::time::timeout(
                FETCH_TIMEOUT,
                crate::research::fetch(client, &sym, m, 30, needs),
            )
            .await
            .map_err(|_| anyhow::anyhow!("timed out"))
            .and_then(|r| r)
            .map(|r| (fundamentals_of(&r), r.warnings));
            (sym, r)
        }))
        .buffer_unordered(CONCURRENCY)
        .collect()
        .await;
    let mut ok = Vec::new();
    let mut failed = Vec::new();
    // Sources that failed for a stock whose other data came through: its
    // facts are kept, and the gap is named.
    let mut partial = Vec::new();
    for (sym, r) in results {
        match r {
            Ok((f, warnings)) => {
                partial.extend(warnings.into_iter().map(|w| format!("{sym}: {w}")));
                ok.push((sym, f));
            }
            Err(_) => failed.push(sym),
        }
    }
    failed.sort();
    partial.sort();
    (ok, failed, partial)
}

/// Fund holdings for the look-through: saved disclosures first, then (online)
/// Yahoo's top holdings, then for an index ETF or index fund the shipped
/// weights of its index ([`crate::index_weights`]). `funds` is (symbol,
/// value, scheme name).
async fn fund_holdings(
    client: Option<&YahooClient>,
    funds: Vec<(String, f64, Option<String>)>,
    market: Market,
) -> Vec<ck::FundHoldings> {
    futures_util::stream::iter(funds.into_iter().map(|(sym, value, name)| async move {
        let profile = match crate::funds::load_fund_disclosure(&sym) {
            Some(p) => Some(p),
            None => match client {
                Some(c) => tokio::time::timeout(
                    FETCH_TIMEOUT,
                    crate::funds::fetch_fund_profile(c, &sym, market),
                )
                .await
                .ok()
                .and_then(Result::ok),
                None => None,
            },
        };
        let holdings: Vec<(String, f64)> = profile
            .iter()
            .flat_map(|p| &p.holdings)
            .filter_map(|h| Some((h.key()?, h.weight)))
            .collect();
        if !holdings.is_empty() {
            return Some(ck::FundHoldings {
                symbol: sym,
                value,
                holdings,
                index_rest: false,
            });
        }
        let table = crate::index_weights::for_fund(&sym, name.as_deref())?;
        Some(ck::FundHoldings {
            symbol: sym,
            value,
            holdings: table.holdings(),
            index_rest: true,
        })
    }))
    .buffer_unordered(CONCURRENCY)
    .filter_map(|x| async move { x })
    .collect()
    .await
}

/// Saved goals still short of their target, in `currency` with its rate to
/// the base. `Ok(None)` without goals; `Err` when goals exist but no rate is
/// known (never ranked as if already in the base currency).
fn goal_facts(
    rows: &[super::planning::GoalRow],
    currency: &str,
    fx_to_base: Option<f64>,
) -> std::result::Result<Option<Vec<ck::GoalFacts>>, ()> {
    if rows.is_empty() {
        return Ok(None);
    }
    let fx_to_base = fx_to_base.ok_or(())?;
    Ok(Some(
        rows.iter()
            .filter(|r| r.status != "funded")
            .map(|r| ck::GoalFacts {
                name: r.name.clone(),
                overdue: r.status == "overdue",
                funded_pct: r.funded_pct,
                monthly_sip: r.monthly_sip,
                gap: (r.target_future - r.linked_value).max(0.0),
                currency: currency.to_string(),
                fx_to_base,
            })
            .collect(),
    ))
}

/// The harvest report. Offline it skips the 31 Jan 2018 price lookup, so
/// grandfathered lots count at their cost.
async fn harvest(
    src: &impl views::DataSource,
    portfolio: &Portfolio,
    market: Market,
    today: chrono::NaiveDate,
    offline: bool,
) -> Result<super::holdings::HarvestSummary> {
    if !offline {
        return Ok(
            super::holdings::harvest_summary(src, portfolio, market, today)
                .await?
                .0,
        );
    }
    let symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = value_holdings(src, &symbols, market, None).await?;
    Ok(super::holdings::compute_harvest(
        portfolio, &val, market, today,
    ))
}

/// Count the shares of `pending` splits and bonuses in `inputs` (lots and
/// totals), for holdings whose inputs still carry the unadjusted share count
/// of `portfolio`. Cost is unchanged; value follows the price.
fn adjust_for_pending_splits(
    inputs: &mut [pa::PositionInput],
    portfolio: &Portfolio,
    pending: &crate::corp_actions::PendingSplits,
) {
    for (sym, _) in &pending.by_symbol {
        let Some(h) = portfolio.holding(sym) else {
            continue;
        };
        let Some(i) = inputs.iter_mut().find(|i| &i.symbol == sym) else {
            continue;
        };
        let adjusted = pending.adjusted_shares(h);
        // Already counted (or not this holding's share count): leave it.
        if (i.shares - h.shares).abs() > 1e-9 || h.shares <= 0.0 {
            continue;
        }
        let ratio = adjusted / h.shares;
        if !ratio.is_finite() || ratio <= 0.0 || (ratio - 1.0).abs() < 1e-12 {
            continue;
        }
        i.shares = adjusted;
        i.value = i.price.map(|p| p * adjusted);
        for l in &mut i.lots {
            l.qty *= ratio;
            l.price /= ratio;
        }
    }
}

/// Gather and evaluate. `fundamentals` reads statements (network) for the
/// forensic and valuation lenses; `offline` reads the cache only.
pub(crate) async fn run_checkup(
    src: &impl views::DataSource,
    market: Market,
    base: Option<&str>,
    offline: bool,
    fundamentals: bool,
) -> Result<CheckupRun> {
    let client = if offline {
        None
    } else {
        src.yahoo().await.ok()
    };
    let mut g = super::portfolio_analyze::gather_inputs(src, client, market, base, false).await?;
    if g.portfolio.holdings.is_empty() {
        bail!("{}", t(Msg::PortfolioEmpty));
    }
    let mut skipped: Vec<String> = Vec::new();
    // A split or bonus not yet applied to the lots: count the shares after
    // it, so the holding is not shown at half its value.
    let pending = match client {
        Some(c) => crate::corp_actions::pending_splits(c, &g.portfolio).await,
        None => crate::corp_actions::PendingSplits::offline(&g.portfolio),
    };
    adjust_for_pending_splits(&mut g.inputs, &g.portfolio, &pending);
    skipped.extend(pending.warnings());
    let analysis = pa::analyze(&g.inputs, &g.ctx);
    let today = g.ctx.today;
    let total = analysis.value;
    if !analysis.left_out.is_empty() {
        skipped.push(format!(
            "No price or exchange rate for {}: left out of the totals",
            analysis.left_out.join(", ")
        ));
    }

    let input_of = |sym: &str| g.inputs.iter().find(|i| i.symbol == sym);
    let mut holdings: Vec<HoldingFacts> = analysis
        .positions
        .iter()
        .filter_map(|p| {
            let value = p.value_base?;
            let fx = input_of(&p.symbol)
                .and_then(|i| i.fx_to_base)
                .unwrap_or(1.0);
            Some(HoldingFacts {
                symbol: p.symbol.clone(),
                value,
                weight_pct: p.weight_pct.unwrap_or(0.0),
                cap_tier: input_of(&p.symbol).and_then(|i| i.cap_tier),
                sector: p.sector.clone(),
                is_fund: is_fund_like(&p.symbol),
                fundamentals: None,
                long_term_soon: p.next_long_term.as_ref().map(|n| {
                    (
                        n.days,
                        n.gain.unwrap_or(0.0) * fx,
                        n.rate_difference.map(|d| d * fx),
                    )
                }),
            })
        })
        .collect();

    // Forensic and valuation: statements of the largest stocks.
    if fundamentals {
        let mut stocks: Vec<&HoldingFacts> = holdings
            .iter()
            .filter(|h| {
                !h.is_fund && !offmarket::is_sgb(&h.symbol) && !market::is_namespaced(&h.symbol)
            })
            .collect();
        stocks.sort_by(|a, b| b.value.total_cmp(&a.value));
        let wanted: Vec<String> = stocks
            .iter()
            .take(FUNDAMENTALS_LIMIT)
            .map(|h| h.symbol.clone())
            .collect();
        if stocks.len() > FUNDAMENTALS_LIMIT {
            skipped.push(format!(
                "Scorecards cover the {FUNDAMENTALS_LIMIT} largest stocks of {}",
                stocks.len()
            ));
        }
        match client {
            _ if wanted.is_empty() => {}
            None => skipped.push(
                "Scorecards, cash conversion, shareholding and results: not read (offline)".into(),
            ),
            Some(c) => {
                let (found, failed, partial) = fetch_fundamentals(c, wanted, market).await;
                if !failed.is_empty() {
                    skipped.push(format!("No statements for {}", failed.join(", ")));
                }
                skipped.extend(partial);
                for (sym, f) in found {
                    if let Some(h) = holdings.iter_mut().find(|h| h.symbol == sym) {
                        h.fundamentals = Some(f);
                    }
                }
            }
        }
    }

    // Look-through and fund overlap.
    let fund_values: Vec<(String, f64, Option<String>)> = holdings
        .iter()
        .filter(|h| h.is_fund)
        .map(|h| {
            let name = analysis
                .positions
                .iter()
                .find(|p| p.symbol == h.symbol)
                .and_then(|p| p.name.clone());
            (h.symbol.clone(), h.value, name)
        })
        .collect();
    let funds = fund_holdings(client, fund_values.clone(), market).await;
    let look_through = if funds.is_empty() {
        Vec::new()
    } else {
        let direct: Vec<(String, f64)> = holdings
            .iter()
            .filter(|h| !funds.iter().any(|f| f.symbol == h.symbol))
            .map(|h| (h.symbol.clone(), h.value))
            .collect();
        ck::look_through(&direct, &funds, total)
    };
    if fund_values.len() > funds.len() {
        skipped.push(format!(
            "Holdings of {} of {} funds not known: they count as one holding each",
            fund_values.len() - funds.len(),
            fund_values.len()
        ));
    }

    // Holdings whose year of closes could not be read.
    let no_history: Vec<&str> = g
        .inputs
        .iter()
        .filter(|i| {
            i.closes.is_empty() && !offmarket::is_fund(&i.symbol) && !offmarket::is_sgb(&i.symbol)
        })
        .map(|i| i.symbol.as_str())
        .collect();
    if !no_history.is_empty() {
        skipped.push(format!(
            "No price history for {}: left out of beta, drawdown and correlations",
            no_history.join(", ")
        ));
    }

    // Correlation from the year of closes.
    let series: Vec<ck::Series> = g
        .inputs
        .iter()
        .filter_map(|i| {
            let h = holdings.iter().find(|h| h.symbol == i.symbol)?;
            Some(ck::Series {
                symbol: &i.symbol,
                value: h.value,
                weight_pct: h.weight_pct,
                closes: &i.closes,
            })
        })
        .collect();
    let correlated = series
        .iter()
        .any(|s| s.closes.len() > ck::MIN_CORRELATION_OBS)
        .then(|| ck::correlated_pairs(&series));
    // The drawdown is measured on the holdings with a year of closes.
    let drawdown_coverage_pct = (total > 0.0).then(|| {
        let covered: f64 = g
            .inputs
            .iter()
            .filter(|i| i.closes.len() >= pa::MIN_CLOSES)
            .filter_map(|i| holdings.iter().find(|h| h.symbol == i.symbol))
            .map(|h| h.value)
            .sum();
        (covered / total * 100.0).clamp(0.0, 100.0)
    });

    // Amounts reported in the tax, fund or goal currency are ranked in the
    // base currency: fetch those rates too.
    let mut extra: Vec<String> = vec!["INR".into(), market_currency(market).into()];
    if let Some(c) = home_country(market) {
        extra.push(c.tax_currency().into());
    }
    g.val.add_rates(src, &extra).await;
    let g = g;
    let fx = |c: &str| g.val.fx.convert(1.0, c);

    // Tax headroom.
    let tax = match home_country(market) {
        None => None,
        Some(country) => match harvest(src, &g.portfolio, market, today, offline).await {
            Ok(h) if fx(h.currency).is_none() => {
                skipped.push(format!(
                    "Tax headroom: no {} to {} exchange rate",
                    h.currency, g.ctx.base_currency
                ));
                None
            }
            Ok(h) => {
                let (_, year_end) = country.tax_year(today);
                let cess = if country == Country::India {
                    1.0 + crate::tax::INDIA_CESS_RATE
                } else {
                    1.0
                };
                Some(ck::TaxFacts {
                    currency: h.currency.to_string(),
                    exemption_remaining: h.ltcg_exemption_remaining,
                    lt_gain_available: h
                        .gain_harvest_candidates
                        .iter()
                        .map(|r| r.lt_pnl.max(0.0))
                        .sum(),
                    lt_rate: country.lt_rate() / 100.0 * cess,
                    st_loss: h.total_st_loss_harvestable,
                    lt_loss: h.total_lt_loss_harvestable,
                    loss_tax_saved: h.estimated_loss_tax_saved,
                    days_to_year_end: (year_end - today).num_days(),
                    year_end: year_end.format("%d %b %Y").to_string(),
                    fx_to_base: fx(h.currency).unwrap_or(1.0),
                })
            }
            Err(e) => {
                skipped.push(format!("Tax headroom: {e:#}"));
                None
            }
        },
    };

    // Regular-plan cost.
    let has_funds = g
        .portfolio
        .holdings
        .iter()
        .any(|h| offmarket::is_fund(&h.symbol));
    let bleed = if !has_funds {
        None
    } else if fx("INR").is_none() {
        skipped.push(format!(
            "Regular vs Direct plan costs: no INR to {} exchange rate",
            g.ctx.base_currency
        ));
        None
    } else if offline {
        skipped.push("Regular vs Direct plan costs: not read (offline)".into());
        None
    } else {
        match super::taxpreview::build_bleed_report(&g.portfolio, BLEED_RETURN_PCT, market).await {
            Ok(r) => Some(
                r.funds
                    .iter()
                    .map(|f| ck::BleedFacts {
                        symbol: f.bleed.symbol.clone(),
                        value: f.bleed.value,
                        annual_cost: f.bleed.annual_bleed,
                        fx_to_base: fx("INR").unwrap_or(1.0),
                    })
                    .collect(),
            ),
            Err(e) => {
                skipped.push(format!("Regular vs Direct plan costs: {e:#}"));
                None
            }
        }
    };

    // Goals and targets.
    let goals = match super::planning::goal_rows(Some(src), market).await {
        Ok(rows) => {
            let cur = market_currency(market);
            match goal_facts(&rows, cur, fx(cur)) {
                Ok(facts) => facts,
                Err(()) => {
                    skipped.push(format!(
                        "Goals: no {cur} to {} exchange rate",
                        g.ctx.base_currency
                    ));
                    None
                }
            }
        }
        Err(e) => {
            skipped.push(format!("Goals: {e:#}"));
            None
        }
    };
    // The same normalized targets and 5/25 bands as `portfolio target drift`.
    let drift = (!g.portfolio.targets.is_empty()).then(|| {
        let band = g
            .portfolio
            .drift_tolerance
            .unwrap_or(crate::config::effective_profile().default_drift_pct);
        let band = if band > 0.0 {
            band
        } else {
            crate::rebalance::ABSOLUTE_BAND_PCT
        };
        let base = analysis.base_currency.clone();
        let inputs: Vec<crate::rebalance::HoldingInput> = analysis
            .positions
            .iter()
            .filter_map(|p| {
                let value = p.value_base?;
                let is_fund = crate::offmarket::is_fund(&p.symbol);
                let foreign = p
                    .currency
                    .as_deref()
                    .is_some_and(|c| !c.eq_ignore_ascii_case(&base));
                Some(crate::rebalance::HoldingInput {
                    symbol: p.symbol.clone(),
                    shares: p.shares,
                    value,
                    price: if p.shares > 0.0 {
                        value / p.shares
                    } else {
                        0.0
                    },
                    sector: p.sector.clone(),
                    is_fund,
                    is_etf: crate::xray::is_known_etf(&p.symbol),
                    asset_class: crate::rebalance::classify(
                        &p.symbol,
                        is_fund,
                        crate::xray::default_etf_category(&p.symbol),
                        foreign,
                    ),
                    ..Default::default()
                })
            })
            .collect();
        crate::rebalance::drift_report(&inputs, &g.portfolio.targets, band, &base)
            .rows
            .into_iter()
            .filter(|r| r.kind != "untargeted")
            .map(|r| ck::DriftFacts {
                key: r.key,
                current_pct: r.current_pct,
                target_pct: r.target_pct,
                band_pct: r.band_pct,
            })
            .collect()
    });

    let input = CheckupInput {
        base_currency: analysis.base_currency.clone(),
        total_value: total,
        holdings,
        look_through,
        fund_overlaps: ck::fund_overlaps(&funds),
        funds_compared: funds.len(),
        beta: analysis.risk.beta,
        max_drawdown_pct: analysis.risk.max_drawdown_pct,
        drawdown_coverage_pct,
        benchmark: analysis.risk.benchmark.clone(),
        correlated,
        tax: tax.clone(),
        bleed,
        goals,
        drift,
        max_stock_weight: g.ctx.max_stock_weight,
        max_smallcap_weight: g.ctx.max_smallcap_weight,
    };
    Ok(CheckupRun {
        checkup: ck::evaluate(&input, skipped),
        analysis,
        tax,
    })
}

/// The checkup for MCP and `--json` callers.
pub(crate) async fn checkup_report(
    src: &impl views::DataSource,
    market: Market,
    base: Option<&str>,
    offline: bool,
) -> Result<Checkup> {
    Ok(run_checkup(src, market, base, offline, true).await?.checkup)
}

fn print_checkup(c: &Checkup) {
    let cur = c.base_currency.as_str();
    print_header(&format!(
        "{} — Checkup",
        portfolio_title(t(Msg::YourPortfolio))
    ));
    println!(
        "  {}  {}    {}  {:.1} (HHI {:.3})    {}  {}/{} clean",
        "Value".dimmed(),
        format_price(c.total_value, Some(cur)).bold(),
        "Effective bets".dimmed(),
        c.effective_bets,
        c.herfindahl,
        "Checks".dimmed(),
        c.clean,
        c.evaluated
    );
    // Lenses in order of their largest finding, clean ones last.
    let mut lenses: Vec<(Lens, f64)> = Lens::ALL
        .iter()
        .map(|l| {
            let top = c
                .findings
                .iter()
                .filter(|f| f.lens == *l)
                .map(|f| f.impact)
                .fold(-1.0, f64::max);
            (*l, top)
        })
        .collect();
    lenses.sort_by(|a, b| b.1.total_cmp(&a.1));
    for (lens, _) in lenses {
        let found: Vec<&ck::Finding> = c.findings.iter().filter(|f| f.lens == lens).collect();
        let ran = c.checks.iter().filter(|x| x.lens == lens).count();
        print_section(&format!("{} ({})", lens.title(), found.len()));
        if found.is_empty() {
            let text = if ran == 0 {
                "Not checked: no data for this lens.".to_string()
            } else {
                format!("Nothing to note ({ran} check(s)).")
            };
            println!("  {}", text.dimmed());
            continue;
        }
        for f in found {
            let mark = match f.severity {
                ck::Severity::High => "●".red().to_string(),
                ck::Severity::Medium => "●".yellow().to_string(),
                ck::Severity::Low => "○".dimmed().to_string(),
            };
            let who = f
                .symbol
                .as_deref()
                .map_or(String::new(), |s| format!("{} ", s.cyan().bold()));
            println!("  {} {}{}", mark, who, f.observed);
            let stake = match (f.at_stake, f.weight_pct) {
                (Some(a), Some(w)) => format!("{} · {:.1}%", format_price(a, Some(cur)), w),
                (Some(a), None) => format_price(a, Some(cur)),
                (None, Some(w)) => format!("{w:.1}%"),
                (None, None) => "–".into(),
            };
            println!(
                "      {} {}    {} {}",
                "at stake".dimmed(),
                stake,
                "inspect".dimmed(),
                f.inspect.cyan()
            );
        }
    }
    if !c.skipped.is_empty() {
        print_section("Not checked");
        for s in &c.skipped {
            println!("  {} {}", "!".yellow(), s);
        }
    }
    println!("\n  {}\n", c.disclaimer.dimmed().italic());
}

/// One finding in `portfolio checkup --csv`; amounts in the base currency.
#[derive(Debug, Serialize)]
pub(crate) struct FindingCsv {
    rank: usize,
    lens: ck::Lens,
    rule: ck::Rule,
    severity: ck::Severity,
    symbol: Option<String>,
    observed: String,
    exposure: f64,
    at_stake: Option<f64>,
    weight_percent: Option<f64>,
    impact: f64,
    inspect: String,
    currency: String,
}

impl CsvRow for FindingCsv {
    const COLUMNS: &'static [&'static str] = &[
        "rank",
        "lens",
        "rule",
        "severity",
        "symbol",
        "observed",
        "exposure",
        "at_stake",
        "weight_percent",
        "impact",
        "inspect",
        "currency",
    ];
}

/// The findings of a checkup as CSV rows, in rank order.
pub(crate) fn finding_csv_rows(c: &Checkup) -> Vec<FindingCsv> {
    c.findings
        .iter()
        .enumerate()
        .map(|(i, f)| FindingCsv {
            rank: i + 1,
            lens: f.lens,
            rule: f.rule,
            severity: f.severity,
            symbol: f.symbol.clone(),
            observed: f.observed.clone(),
            exposure: round(f.exposure, 2),
            at_stake: round_opt(f.at_stake, 2),
            weight_percent: round_opt(f.weight_pct, 2),
            impact: round(f.impact, 2),
            inspect: f.inspect.clone(),
            currency: c.base_currency.clone(),
        })
        .collect()
}

pub async fn cmd_portfolio_checkup(
    market: Market,
    base: Option<&str>,
    offline: bool,
    fmt: Format,
) -> Result<()> {
    if fmt.is_table() {
        eprintln!(
            "  {} Checking the portfolio{}...",
            "⟳".yellow(),
            if offline {
                " from local data"
            } else {
                " (prices, a year of closes, statements, fund holdings)"
            }
        );
    }
    let c = if offline {
        checkup_report(&OfflineSource, market, base, true).await?
    } else {
        checkup_report(&views::LiveSource::new(), market, base, false).await?
    };
    match fmt {
        Format::Json => output::print_json(&c),
        Format::Csv => output::print_csv(&finding_csv_rows(&c)),
        Format::Table => {
            print_checkup(&c);
            Ok(())
        }
    }
}

// ── Wrap ──

/// The chart range that reaches back to `opening`.
fn range_back_to(opening: chrono::NaiveDate, today: chrono::NaiveDate) -> &'static str {
    let days = (today - opening).num_days();
    // Each range must cover the opening with a few days to spare.
    ["1y", "2y", "5y", "10y"]
        .into_iter()
        .find(|r| range_days(r) - 5 >= days)
        .unwrap_or("max")
}

fn closes_of(chart: &crate::api::ChartData) -> Vec<(chrono::NaiveDate, f64)> {
    let stamps = chart.timestamp.as_deref().unwrap_or_default();
    let closes = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_deref())
        .unwrap_or_default();
    let offset = chart.meta.as_ref().and_then(|m| m.gmtoffset).unwrap_or(0);
    stamps
        .iter()
        .zip(closes)
        .filter_map(|(t, c)| {
            Some((
                chrono::DateTime::from_timestamp(t + offset, 0)?.date_naive(),
                (*c)?,
            ))
        })
        .collect()
}

/// Daily closes (or NAVs for `MF:` funds) of `symbol`, oldest first.
async fn price_history(
    src: &impl views::DataSource,
    symbol: &str,
    range: &str,
    offline: bool,
) -> Vec<(chrono::NaiveDate, f64)> {
    if let Some(id) = offmarket::fund_id(symbol) {
        let stored = || {
            crate::db::with_db(|d| d.get_navs(id))
                .unwrap_or_default()
                .into_iter()
                .filter_map(|(d, n)| {
                    Some((chrono::NaiveDate::parse_from_str(&d, "%Y-%m-%d").ok()?, n))
                })
                .collect::<Vec<_>>()
        };
        if offline {
            return stored();
        }
        let live = match (id.parse::<u32>(), crate::funds::http_client()) {
            (Ok(code), Ok(c)) => crate::funds::load_history(&c, code)
                .await
                .ok()
                .map(|h| h.navs),
            _ => None,
        };
        let mut navs = live.unwrap_or_else(stored);
        navs.sort_by_key(|n| n.0);
        return navs;
    }
    src.chart(symbol, range, "1d")
        .await
        .map(|c| closes_of(&c))
        .unwrap_or_default()
}

/// Build the year-in-review card.
pub(crate) async fn wrap_report(
    src: &impl views::DataSource,
    market: Market,
    base: Option<&str>,
    period: Option<Period>,
    offline: bool,
) -> Result<WrapReport> {
    let portfolio = src.portfolio().map_err(|e| e.context(views::LocalData))?;
    if portfolio.holdings.is_empty() && portfolio.realized.is_empty() {
        bail!("{}", t(Msg::PortfolioEmpty));
    }
    // A split Yahoo reports but the lots have not taken yet: value a copy
    // with it applied (closes are in today's share count).
    let pending = if offline {
        crate::corp_actions::PendingSplits::offline(&portfolio)
    } else {
        crate::corp_actions::pending_splits_live(&portfolio).await
    };
    pending.print();
    let portfolio = pending.adjusted(&portfolio);
    // The checkup describes what is held now; a portfolio sold out during
    // the year still gets its review, without that part.
    let run = if portfolio.holdings.is_empty() {
        None
    } else {
        Some(run_checkup(src, market, base, offline, false).await?)
    };
    let today = chrono::Local::now().date_naive();
    let a = run.as_ref().map(|r| &r.analysis);

    let held: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let val = if held.is_empty() {
        Valuation::new(Vec::new(), &portfolio.symbols_with_history(), market, base)
    } else {
        value_holdings(src, &held, market, base).await?
    };
    let period = period.unwrap_or_else(|| Period::default_for(val.base(), today));
    if period.start > today {
        bail!("{} has not started yet", period.label);
    }
    let as_of = period.as_of(today);
    let range = range_back_to(period.opening(), today);
    let mut symbols = portfolio.symbols_with_history();
    // Sold-out holdings need their own exchange rates.
    let mut val = val;
    let currencies = val.currencies(&symbols);
    val.add_rates(src, &currencies).await;
    if !offline {
        val.load_history(&portfolio).await;
    }
    symbols.sort();

    let histories: Vec<(String, Vec<(chrono::NaiveDate, f64)>)> =
        futures_util::stream::iter(symbols.iter().cloned().map(|sym| async move {
            let c = price_history(src, &sym, range, offline).await;
            (sym, c)
        }))
        .buffered(6)
        .collect()
        .await;

    let mut years = Vec::new();
    for (sym, closes) in &histories {
        let start = wrap::close_on(closes, period.opening());
        let end = if as_of == today {
            val.price(sym).or_else(|| wrap::close_on(closes, as_of))
        } else {
            wrap::close_on(closes, as_of)
        };
        // Each amount at its own day's exchange rate (today's when no
        // history was found).
        let fx = |d: chrono::NaiveDate| val.rate_on(sym, d).map(|r| r.0);
        let y = wrap::symbol_year_in(&portfolio, sym, &period, as_of, start, end, &fx);
        years.push(y);
    }
    let t = wrap::totals(&years, &period, as_of, 3);

    let (bench_symbol, bench_name) = match market {
        Market::In => ("^NSEI", "NIFTY 50"),
        Market::Us => ("^GSPC", "S&P 500"),
        Market::Global(x) => x.info().benchmark(),
        _ => ("^GSPC", "S&P 500"),
    };
    // A total-return index when Yahoo has one; else the price index,
    // compared with the return without dividends.
    let tri = super::planning::total_return_source(bench_symbol);
    let mut bench_total_return = false;
    let mut bench = Vec::new();
    match tri {
        // An ETF's dividend-adjusted closes (NIFTYBEES for NIFTY 50) need
        // Yahoo's raw chart; offline there are none.
        Some(tr @ super::planning::TotalReturn::AdjustedEtf(_)) if !offline => {
            if let Ok(client) = src.yahoo().await
                && let Some((closes, _)) =
                    super::planning::total_return_closes(client, tr, period.opening()).await
            {
                bench = closes;
            }
        }
        Some(super::planning::TotalReturn::Index(sym)) => {
            bench = src
                .chart(sym, range, "1d")
                .await
                .map(|c| closes_of(&c))
                .unwrap_or_default();
        }
        _ => {}
    }
    if tri.is_some() {
        bench_total_return = wrap::benchmark_return(&bench, &period, as_of).is_some();
    }
    if !bench_total_return {
        bench = src
            .chart(bench_symbol, range, "1d")
            .await
            .map(|c| closes_of(&c))
            .unwrap_or_default();
    }
    let benchmark_return_pct = wrap::benchmark_return(&bench, &period, as_of);
    let compared = if bench_total_return {
        t.return_pct
    } else {
        t.price_return_pct
    };
    let at_work = t.start_value + t.bought;
    let share = |v: f64| (at_work > 0.0).then(|| v / at_work * 100.0);

    let tax = home_country(market).map(|country| {
        let (_, end) = country.tax_year(today);
        let limit = country.exemption();
        let remaining = run
            .as_ref()
            .and_then(|r| r.tax.as_ref())
            .map(|t| t.exemption_remaining);
        wrap::TaxFooter {
            year: country.tax_year_label(today),
            days_left: (end - today).num_days(),
            currency: country.tax_currency().to_string(),
            exemption_limit: (limit > 0.0).then_some(limit),
            exemption_remaining: remaining.filter(|_| limit > 0.0),
            exemption_used_pct: remaining
                .filter(|_| limit > 0.0)
                .map(|r| ((limit - r) / limit * 100.0).clamp(0.0, 100.0)),
            next_advance_tax: (country == Country::India)
                .then(|| wrap::next_advance_tax(today))
                .flatten(),
        }
    });
    let slices = |groups: &[pa::Group], n: usize| -> Vec<wrap::Slice> {
        groups
            .iter()
            .take(n)
            .map(|g| wrap::Slice {
                name: g.name.clone(),
                weight_pct: g.weight_pct,
            })
            .collect()
    };
    Ok(WrapReport {
        as_of,
        base_currency: val.base().to_string(),
        redacted: false,
        return_pct: t.return_pct,
        benchmark: match tri.filter(|_| bench_total_return) {
            Some(tr) => tr.label(bench_name),
            None => bench_name.to_string(),
        },
        benchmark_return_pct,
        alpha_bps: compared
            .zip(benchmark_return_pct)
            .map(|(r, b)| (r - b) * 100.0),
        price_return_pct: t.price_return_pct,
        benchmark_total_return: bench_total_return,
        start_value: Some(t.start_value),
        end_value: Some(t.end_value),
        capital_deployed: Some(t.bought),
        withdrawn: Some(t.sold),
        dividends: Some(t.dividends),
        realized_gain: Some(t.realized_gain),
        deployed_pct: share(t.bought),
        dividends_pct: share(t.dividends),
        realized_gain_pct: share(t.realized_gain),
        winners: t.winners,
        laggards: t.laggards,
        cap_tiers: a.map_or(Vec::new(), |a| slices(&a.by_cap_tier, 5)),
        top_sectors: a.map_or(Vec::new(), |a| slices(&a.by_sector, 3)),
        holdings: a.map_or(0, |a| a.positions.len()),
        effective_bets: run.as_ref().map_or(0.0, |r| r.checkup.effective_bets),
        checkup_clean: run.as_ref().map(|r| (r.checkup.clean, r.checkup.evaluated)),
        tax,
        unpriced: t.unpriced,
        unconverted: t.unconverted,
        disclaimer: wrap::DISCLAIMER,
        period,
    })
}

pub async fn cmd_portfolio_wrap(
    market: Market,
    base: Option<&str>,
    year: Option<&str>,
    fy: Option<&str>,
    redact: bool,
    offline: bool,
    json: bool,
) -> Result<()> {
    let period = match (year, fy) {
        (Some(y), _) => Some(Period::parse_year(y).map_err(anyhow::Error::msg)?),
        (None, Some(f)) => Some(Period::parse_fy(f).map_err(anyhow::Error::msg)?),
        (None, None) => None,
    };
    if !json {
        eprintln!("  {} Reviewing the year...", "⟳".yellow());
    }
    let report = if offline {
        wrap_report(&OfflineSource, market, base, period, true).await?
    } else {
        wrap_report(&views::LiveSource::new(), market, base, period, false).await?
    };
    let report = if redact {
        wrap::redact(&report)
    } else {
        report
    };
    if json {
        return output::print_json(&report);
    }
    println!();
    for (i, line) in wrap::render(&report).iter().enumerate() {
        if i == 0 {
            println!("  {}", line.bold());
        } else if line == report.disclaimer {
            println!("  {}", line.dimmed().italic());
        } else {
            println!("  {line}");
        }
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csv_has_one_row_per_finding_in_rank_order() {
        let mut input = ck::CheckupInput {
            base_currency: "INR".into(),
            total_value: 1000.0,
            beta: Some(1.5),
            max_drawdown_pct: Some(-40.0),
            benchmark: "NIFTY 50".into(),
            ..Default::default()
        };
        input.holdings = vec![ck::HoldingFacts {
            symbol: "A.NS".into(),
            value: 1000.0,
            weight_pct: 100.0,
            ..Default::default()
        }];
        input.max_stock_weight = Some(150.0);
        let c = ck::evaluate(&input, vec![]);
        let rows = finding_csv_rows(&c);
        assert_eq!(rows.len(), c.findings.len());
        assert_eq!(rows[0].rank, 1);
        let v = serde_json::to_value(&rows[0]).unwrap();
        assert_eq!(v.as_object().unwrap().len(), FindingCsv::COLUMNS.len());
        for col in FindingCsv::COLUMNS {
            assert!(v.get(*col).is_some(), "{col}");
        }
    }

    #[test]
    fn goals_without_an_exchange_rate_are_not_ranked() {
        let row = super::super::planning::GoalRow {
            name: "home".into(),
            target: 1_000_000.0,
            target_future: 1_000_000.0,
            by: "2030-01".into(),
            months_left: 38,
            holdings: vec![],
            linked_value: 400_000.0,
            valued_at: "none",
            funded_pct: 40.0,
            monthly_sip: 10_000.0,
            status: "needs_sip",
        };
        assert_eq!(goal_facts(&[], "INR", None), Ok(None));
        assert_eq!(goal_facts(std::slice::from_ref(&row), "INR", None), Err(()));
        let facts = goal_facts(&[row], "INR", Some(0.012)).unwrap().unwrap();
        assert_eq!(facts[0].gap, 600_000.0);
        assert_eq!(facts[0].fx_to_base, 0.012);
    }

    #[test]
    fn offline_history_reaches_an_old_period() {
        // Stored daily bars for 2009-2010; a 2010 wrap reviewed in 2026.
        let mut db = crate::db::CacheDb::open_in_memory().unwrap();
        let bar = |day: &str, close: f64| {
            let ts = chrono::NaiveDate::parse_from_str(day, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(0, 0, 0)
                .unwrap()
                .and_utc()
                .timestamp();
            crate::db::Bar {
                ts,
                open: close,
                high: close,
                low: close,
                close,
                volume: 0.0,
            }
        };
        db.put_bars(
            "OLD.NS",
            "1d",
            &[
                bar("2009-12-31", 100.0),
                bar("2010-06-30", 110.0),
                bar("2010-12-31", 125.0),
            ],
        )
        .unwrap();
        let period = Period::calendar(2010).unwrap();
        let today = chrono::NaiveDate::parse_from_str("2026-11-18", "%Y-%m-%d").unwrap();
        let now = today.and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp();
        let range = range_back_to(period.opening(), today);
        let closes = closes_of(&stored_chart(&db, "OLD.NS", range, "1d", now).unwrap());
        assert_eq!(wrap::close_on(&closes, period.opening()), Some(100.0));
        assert_eq!(wrap::close_on(&closes, period.as_of(today)), Some(125.0));
        assert!(stored_chart(&db, "NONE.NS", range, "1d", now).is_err());
    }

    #[test]
    fn chart_range_reaches_the_opening() {
        let d = |s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        assert_eq!(range_back_to(d("2026-03-31"), d("2026-11-18")), "1y");
        assert_eq!(range_back_to(d("2025-03-31"), d("2026-11-18")), "2y");
        assert_eq!(range_back_to(d("2022-12-31"), d("2026-11-18")), "5y");
        assert_eq!(range_back_to(d("2016-12-31"), d("2026-11-18")), "10y");
        assert_eq!(range_back_to(d("2009-12-31"), d("2026-11-18")), "max");
        // Every range reaches back to its opening.
        let today = d("2026-11-18");
        for days in [1, 300, 361, 400, 726, 1800, 1830, 3600, 3700, 6000] {
            let opening = today - chrono::Days::new(days);
            let r = range_back_to(opening, today);
            assert!(range_days(r) >= days as i64, "{days} days: {r}");
        }
        assert_eq!(range_days("2y"), 731);
    }

    #[test]
    fn fundamentals_from_research() {
        use crate::research::{Holding, Period as P, Research};
        let d = |s| chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        let mut r = Research::default();
        r.holdings = vec![
            Holding {
                date: d("2026-06-30"),
                promoter: Some(55.0),
                pledged: Some(false),
                ..Default::default()
            },
            Holding {
                date: d("2026-09-30"),
                promoter: Some(52.5),
                pledged: Some(true),
                ..Default::default()
            },
        ];
        r.annual = vec![P {
            end: d("2026-03-31"),
            ocf: Some(50.0),
            net_income: Some(100.0),
            ..Default::default()
        }];
        r.fin.trailing_pe = Some(70.0);
        let f = fundamentals_of(&r);
        assert_eq!(f.promoter_pledged, Some(true));
        assert!((f.promoter_change_pp.unwrap() + 2.5).abs() < 1e-9);
        assert_eq!(f.cfo_to_pat, Some(0.5));
        assert_eq!(f.pe, Some(70.0));
        assert_eq!(f.pat_yoy_pct, None);
    }
}
