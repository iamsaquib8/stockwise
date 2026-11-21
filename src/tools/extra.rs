//! Read tools over the newer commands: the portfolio dashboard, market
//! conditions, picks, movers, dividends, the emergency fund, the fit check
//! against the user's own rules, and the journal of assistant changes.

use super::ToolContext;
use super::args::Args;
use crate::api::YahooClient;
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
        "market_conditions" => {
            let client = YahooClient::new().await?;
            let report = crate::commands::conditions_report(&client, m, universe(a, m)?).await?;
            Ok(serde_json::to_value(report)?)
        }
        "picks" => {
            m.require_stocks("picks")?;
            let client = YahooClient::new().await?;
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
            let run = crate::picks::run(
                &client,
                m,
                universe(a, m)?,
                cap,
                floors,
                horizon,
                a.usize_or("top", 10),
            )
            .await?;
            Ok(json!({
                "scanned": run.scanned,
                "below_floor": run.below_floor,
                "thin_data": run.thin_data,
                "flagged_check_data": run.flagged.iter().map(|p| json!({"symbol": p.symbol, "flags": p.flags})).collect::<Vec<_>>(),
                "picks": run.picks,
                "methodology": run.methodology,
                "horizon": horizon,
                "method": "Percentile ranks among the stocks that clear the size and liquidity floors: value 25%, quality 25%, growth 15%, trend 20%, risk 15%. Losses and negative book value rank last; implausible figures are left out to check; at most 3 per sector; a pillar without data pulls the score toward 50.",
            }))
        }
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
