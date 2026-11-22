//! Read tools over the newer commands: the portfolio dashboard, market
//! conditions, picks and their track record, batch verdicts, movers,
//! dividends, the emergency fund, the fit check against the user's own
//! rules, and the journal of assistant changes.
//!
//! Tools that need Yahoo beyond quotes and charts use the data source's own
//! client (`DataSource::yahoo`), so one MCP session shares one session
//! crumb, rate limiter and cache across calls.

use super::ToolContext;
use super::args::Args;
use crate::market::{self, Market};
use crate::views::{self, DataSource};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

fn universe(a: &Args, m: Market) -> Result<Option<market::Universe>> {
    use clap::ValueEnum;
    let Some(u) = a.str("universe") else {
        return Ok(None);
    };
    if m != Market::In {
        bail!("universe is for the Indian market (market: in)");
    }
    market::Universe::from_str(u, true)
        .map(Some)
        .map_err(|_| anyhow::anyhow!("Unknown universe '{}'", u))
}

pub(super) async fn run<D: DataSource>(
    ctx: &ToolContext<D>,
    name: &str,
    a: &Args,
    m: Market,
) -> Result<Value> {
    let src = &ctx.source;
    match name {
        "portfolio_analyze" => {
            let base = a.str("base").map(str::to_ascii_uppercase);
            let report =
                crate::commands::load_analysis(src, None, m, base.as_deref(), a.bool("quick"))
                    .await?;
            Ok(serde_json::to_value(report)?)
        }
        "portfolio_checkup" => {
            let base = a.str("base").map(str::to_ascii_uppercase);
            let report = if a.bool("offline") {
                crate::commands::checkup_report(
                    &crate::commands::OfflineSource,
                    m,
                    base.as_deref(),
                    true,
                )
                .await?
            } else {
                crate::commands::checkup_report(src, m, base.as_deref(), false).await?
            };
            Ok(serde_json::to_value(report)?)
        }
        "stock_action" => {
            let resolved = market::resolve_symbol(a.need_str("symbol")?, m);
            let horizon = horizon_arg(a)?;
            let r = crate::research::fetch(
                src.yahoo().await?,
                &resolved,
                m,
                60,
                crate::research::Needs {
                    filings: true,
                    ..crate::research::Needs::VERDICT
                },
            )
            .await?;
            let cards = crate::insights::generate_rich_insights(&r);
            let profile = super::current_profile(ctx);
            let mut v = crate::verdict::generate_verdict_for(&r, &cards, &profile, horizon);
            // The holding context comes from the caller's workspace: its
            // portfolio file and notes when it has one, else the data
            // source's portfolio that the other read tools see. When that
            // portfolio cannot be read, the action stays the public one,
            // without holding context.
            let (portfolio, notes) = match &ctx.workspace {
                Some(w) => (
                    crate::portfolio::Portfolio::load_from(&w.portfolio),
                    Some(w.dir.join(crate::notes::FILE)),
                ),
                None => (src.portfolio(), None),
            };
            if let Ok(p) = portfolio {
                crate::commands::apply_holding_in(&mut v, &r, m, &profile, &p, || match notes {
                    Some(path) => crate::store::load_from(&path),
                    None => crate::commands::load_notes(),
                });
            }
            Ok(serde_json::json!({
                "symbol": v.symbol,
                "name": v.name,
                "price": v.price,
                "currency": v.currency,
                "action": v.action,
                "action_coverage": v.action.coverage,
                "composite_score": v.overall_score,
                "score_coverage": v.data_coverage,
                "data_coverage": v.data_coverage,
                "pillars": v.pillars.iter().map(|p| serde_json::json!({
                    "name": p.name, "score": p.score, "band": p.band,
                })).collect::<Vec<_>>(),
                "meta": v.meta,
            }))
        }
        "portfolio_review" => {
            let base = a.str("base").map(str::to_ascii_uppercase);
            let client = src.yahoo().await.ok();
            let review = crate::commands::portfolio_review_report(
                src,
                client,
                m,
                base.as_deref(),
                horizon_arg(a)?,
            )
            .await?;
            Ok(serde_json::to_value(review)?)
        }
        "market_conditions" => {
            let client = src.yahoo().await?;
            let report = crate::commands::conditions_report(client, m, universe(a, m)?).await?;
            Ok(serde_json::to_value(report)?)
        }
        "picks" => {
            m.require_stocks("picks")?;
            let universe = universe(a, m)?;
            let cap = match a.str("cap") {
                Some(c) => {
                    use clap::ValueEnum;
                    Some(
                        market::CapFilter::from_str(c, true)
                            .map_err(|_| anyhow::anyhow!("Unknown cap tier '{}'", c))?,
                    )
                }
                None => None,
            };
            let floors =
                crate::picks::Floors::for_market(m, cap, a.f64("min_mcap"), a.f64("min_adv"));
            let horizon = match a.str("horizon") {
                Some("short") => crate::picks::Horizon::Short,
                _ => crate::picks::Horizon::Long,
            };
            let client = src.yahoo().await?;
            let run = crate::picks::run(
                client,
                m,
                universe,
                cap,
                floors,
                horizon,
                a.usize_or("top", 10),
            )
            .await?;
            // Logged like `stockwise picks`, so the track record (picks_review,
            // stockwise://picks/latest) counts lists an assistant asked for.
            let scope =
                crate::commands::cap_scope(universe.map_or(m.label(), |u| u.nse_index()), cap);
            let level = crate::commands::benchmark_level(client, m).await;
            crate::commands::log_picks(m, &scope, horizon, &run.picks, level);
            Ok(picks_value(&run))
        }
        "picks_review" => {
            let since = crate::picks_log::parse_days(a.str("since").unwrap_or("90d"))
                .map_err(|e| anyhow::anyhow!(e))?;
            let horizon = match a.str("horizon") {
                Some("short") => crate::picks::Horizon::Short,
                _ => crate::picks::Horizon::Long,
            };
            let review =
                crate::commands::picks_review_report(src.yahoo().await?, m, since, horizon).await?;
            let mut v = serde_json::to_value(review)?;
            v["market"] = json!(m.code());
            v["horizon"] = json!(horizon);
            v["about"] = json!(
                "Each pick measured from the day it was first listed; price return, dividends not included. Past lists do not predict future ones."
            );
            Ok(v)
        }
        "verdicts" => verdicts(src, a, m).await,
        "movers" => {
            m.require_stocks("movers")?;
            let set = views::quotes(src, &m.universe()).await?;
            let (gainers, losers) =
                crate::commands::mover_rows(&set.quotes, a.usize_or("count", 5));
            Ok(json!({ "gainers": gainers, "losers": losers, "scanned": set.quotes.len() }))
        }
        "dividends" => {
            let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m);
            let set = views::quotes(src, std::slice::from_ref(&sym)).await?;
            let q = set
                .quotes
                .first()
                .with_context(|| format!("No quote for {}", sym))?;
            let price = q
                .regular_market_price
                .with_context(|| format!("No price for {}", sym))?;
            let yld = q.trailing_annual_dividend_yield.unwrap_or(0.0);
            Ok(crate::commands::dividend_json(
                &sym,
                q.currency.as_deref(),
                price,
                yld,
            ))
        }
        "goal_emergency" => {
            let plan = crate::commands::emergency_plan(
                a.need_f64("monthly_expense")?,
                a.usize_or("months_cover", 6) as u32,
                a.f64("liquid").unwrap_or(0.0),
                a.usize_or("horizon_months", 12) as u32,
                a.f64("return_pct"),
                m,
            );
            Ok(serde_json::to_value(plan)?)
        }
        "fit_check" => fit_check(ctx, a, m).await,
        "changes_list" => {
            let journal = match &ctx.workspace {
                Some(w) => crate::audit::Journal::new(&w.dir),
                None => crate::audit::Journal::default_location()?,
            };
            let mut changes = journal.list()?;
            changes.truncate(a.usize_or("limit", 20));
            Ok(json!({ "changes": changes, "undo": "stockwise undo [ID]" }))
        }
        other => bail!("Tool '{}' has no handler", other),
    }
}

/// The `picks` tool's result: the list, what was left out and why, and the
/// method behind the scores.
/// The method line of a picks result, with the weights of the horizon that
/// was scored (from the scorer's own table).
fn picks_method(horizon: crate::picks::Horizon) -> String {
    let [v, q, g, t, r] = horizon.weights();
    let pct = |w: f64| format!("{:.0}%", w * 100.0);
    format!(
        "Percentile ranks among the stocks that clear the size and liquidity floors ({} horizon): value {}, quality {}, growth {}, trend {}, risk {}. Losses and negative book value rank last; implausible figures are left out to check; stocks under 60% data coverage are left out; with a positive top, at most 3 per known sector; a pillar without data pulls the score toward 50.",
        horizon.as_str(),
        pct(v),
        pct(q),
        pct(g),
        pct(t),
        pct(r)
    )
}

pub(crate) fn picks_value(run: &crate::picks::Run) -> Value {
    json!({
        "scanned": run.scanned,
        "below_floor": run.below_floor,
        "thin_data": run.thin_data,
        "flagged_check_data": run.flagged.iter().map(|p| json!({"symbol": p.symbol, "flags": p.flags})).collect::<Vec<_>>(),
        "picks": run.picks,
        "meta": run.meta,
        "methodology": run.methodology,
        "horizon": run.horizon,
        "method": picks_method(run.horizon),
    })
}

/// Below this data coverage (percent of inputs with data) a verdict is too
/// thin to lean on: the line the recommendations policy calls NO CALL.
pub(crate) const MIN_COVERAGE_PCT: f64 = 60.0;

/// Most stocks researched at once by `verdicts`.
const VERDICT_CONCURRENCY: usize = 4;

/// Compact 6-pillar scores for up to 25 stocks, researched a few at a time;
/// a stock that fails is listed in `failed` instead of failing the batch.
async fn verdicts<D: DataSource>(src: &D, a: &Args, m: Market) -> Result<Value> {
    use futures_util::StreamExt;
    let client = src.yahoo().await?;
    let mut syms: Vec<String> = a
        .list("symbols")
        .iter()
        .map(|s| market::resolve_symbol(s.trim(), m))
        .collect();
    // Each symbol once, in the order given (dedup alone keeps
    // non-adjacent repeats).
    let mut seen = std::collections::HashSet::new();
    syms.retain(|s| seen.insert(s.clone()));
    let results: Vec<(String, Result<crate::verdict::DecisionVerdict>)> =
        futures_util::stream::iter(syms.into_iter().map(|sym| async move {
            let r = async {
                let research =
                    crate::research::fetch(client, &sym, m, 60, crate::research::Needs::VERDICT)
                        .await?;
                let cards = crate::insights::generate_rich_insights(&research);
                Ok(crate::verdict::generate_verdict(&research, &cards))
            }
            .await;
            (sym, r)
        }))
        .buffered(VERDICT_CONCURRENCY)
        .collect()
        .await;
    let mut rows = Vec::new();
    let mut failed = Vec::new();
    for (sym, r) in results {
        match r {
            Ok(v) => rows.push(verdict_row(&v)),
            Err(e) => failed.push(json!({ "symbol": sym, "error": super::client_error(&e) })),
        }
    }
    if rows.is_empty() && !failed.is_empty() {
        bail!(
            "No verdict for any of the symbols{}",
            market::not_found_hint(m)
        );
    }
    Ok(json!({
        "verdicts": rows,
        "failed": failed,
        "disclaimer": crate::verdict::DISCLAIMER,
    }))
}

/// One stock of `verdicts`: the composite, coverage and each pillar's score.
fn verdict_row(v: &crate::verdict::DecisionVerdict) -> Value {
    json!({
        "symbol": v.symbol,
        "name": v.name,
        "price": v.price,
        "currency": v.currency,
        "overall_score": v.overall_score,
        "data_coverage": v.data_coverage,
        "low_coverage": v.data_coverage < MIN_COVERAGE_PCT,
        "pillars": v.pillars.iter().map(|p| json!({
            "name": p.name,
            "score": p.score,
            "band": p.band,
        })).collect::<Vec<_>>(),
        "warnings": v.warnings,
    })
}

/// One rule's result.
fn rule(name: &str, detail: String) -> Value {
    json!({ "rule": name, "detail": detail })
}

/// Whether a stock meets the user's own rules.
async fn fit_check<D: DataSource>(ctx: &ToolContext<D>, a: &Args, m: Market) -> Result<Value> {
    let src = &ctx.source;
    let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m);
    let amount = a.f64("amount").unwrap_or(0.0);
    let profile = super::current_profile(ctx);
    let notes: crate::notes::NoteStore = match &ctx.workspace {
        Some(w) => crate::store::load_from(&w.dir.join(crate::notes::FILE))?,
        None => crate::store::load(crate::notes::FILE).unwrap_or_default(),
    };
    let note = notes.get(&sym).cloned();
    let set = views::quotes(src, std::slice::from_ref(&sym)).await?;
    let q = set
        .quotes
        .first()
        .with_context(|| format!("No quote for {}", sym))?;
    let view = views::portfolio(src, m, None).await?;
    let total = view.totals.value;
    let held = view
        .holdings
        .iter()
        .find(|h| h.symbol.eq_ignore_ascii_case(&sym))
        .and_then(|h| h.value)
        .unwrap_or(0.0);
    let (mut passed, mut failed, mut unknown) = (Vec::new(), Vec::new(), Vec::new());
    let after = total + amount;
    // Weight of one stock: the profile's limit and the note's.
    if after > 0.0 {
        let w = (held + amount) / after * 100.0;
        let mut limits = vec![(
            "profile's largest weight of one stock",
            profile.max_stock_weight,
        )];
        if let Some(n) = note.as_ref().and_then(|n| n.max_weight_pct) {
            limits.push(("your note's largest weight", n));
        }
        for (label, limit) in limits {
            let detail = format!(
                "{:.1}% of the portfolio{}; {} is {:.0}%",
                w,
                if amount > 0.0 { " after adding it" } else { "" },
                label,
                limit
            );
            if w <= limit {
                passed.push(rule("weight", detail));
            } else {
                failed.push(rule("weight", detail));
            }
        }
    } else {
        unknown.push(rule(
            "weight",
            "No portfolio value to weigh it against".into(),
        ));
    }
    // Small and micro caps together.
    let tier = market::cap_tier_of(q);
    match tier {
        market::CapTier::Small | market::CapTier::Micro => {
            let small_now: f64 = view
                .holdings
                .iter()
                .filter(|h| matches!(h.cap_tier, "small" | "micro"))
                .filter_map(|h| h.value)
                .sum();
            let share = if after > 0.0 {
                (small_now + amount) / after * 100.0
            } else {
                100.0
            };
            let detail = format!(
                "{} cap; small and micro caps would be {:.1}% of the portfolio, the profile allows {:.0}%",
                tier.as_str(),
                share,
                profile.max_smallcap_weight
            );
            if share <= profile.max_smallcap_weight {
                passed.push(rule("small_caps", detail));
            } else {
                failed.push(rule("small_caps", detail));
            }
        }
        market::CapTier::Unknown => {
            unknown.push(rule("small_caps", "No market cap to tier it by".into()))
        }
        t => passed.push(rule(
            "small_caps",
            format!("{} cap, not small or micro", t.as_str()),
        )),
    }
    // Risk tier against the profile's risk level.
    let score = crate::longterm::score_for_longterm(q, None);
    let tier_name = score.risk_tier.to_string();
    let allowed: &[&str] = match profile.risk.as_str() {
        "low" => &["Lower"],
        "med" => &["Lower", "Moderate"],
        _ => &["Lower", "Moderate", "Higher"],
    };
    let detail = format!(
        "risk tier {}; a {} risk profile allows {}",
        tier_name,
        profile.risk.as_str(),
        allowed.join(", ")
    );
    if score.data_coverage < 0.5 {
        unknown.push(rule("risk", "Too little data for a risk tier".into()));
    } else if allowed.iter().any(|t| tier_name.starts_with(t)) {
        passed.push(rule("risk", detail));
    } else {
        failed.push(rule("risk", detail));
    }
    // The note's valuation and quality limits.
    if let Some(n) = &note {
        if let Some(max_pe) = n.max_pe {
            match q.trailing_pe.filter(|p| *p > 0.0) {
                Some(pe) if pe <= max_pe => passed.push(rule(
                    "pe",
                    format!("P/E {:.1}; your limit is {:.0}", pe, max_pe),
                )),
                Some(pe) => failed.push(rule(
                    "pe",
                    format!("P/E {:.1}; your limit is {:.0}", pe, max_pe),
                )),
                None => unknown.push(rule("pe", "No positive trailing P/E".into())),
            }
        }
        if let Some(min_roe) = n.min_roe_pct {
            match q.return_on_equity.map(|r| r * 100.0) {
                Some(roe) if roe >= min_roe => passed.push(rule(
                    "roe",
                    format!("ROE {:.1}%; your minimum is {:.0}%", roe, min_roe),
                )),
                Some(roe) => failed.push(rule(
                    "roe",
                    format!("ROE {:.1}%; your minimum is {:.0}%", roe, min_roe),
                )),
                None => unknown.push(rule("roe", "No return on equity in the quote".into())),
            }
        }
    }
    let answer = if !failed.is_empty() {
        "no"
    } else if passed.is_empty() {
        "unclear"
    } else {
        "yes"
    };
    Ok(json!({
        "symbol": sym,
        "fits_your_rules": answer,
        "passed": passed,
        "failed": failed,
        "unknown": unknown,
        "profile": profile.spec,
        "note": note,
        "about": "Measured against the user's own profile limits and notes; it is not a recommendation and says nothing about future prices.",
    }))
}

/// The `horizon` argument of the action tools (long by default).
fn horizon_arg(a: &Args) -> Result<crate::action::Horizon> {
    Ok(
        match a.str("horizon").map(str::to_ascii_lowercase).as_deref() {
            None | Some("long") => crate::action::Horizon::Long,
            Some("short") => crate::action::Horizon::Short,
            Some(other) => anyhow::bail!("Unknown horizon '{other}' (long or short)"),
        },
    )
}
