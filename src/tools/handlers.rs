//! Each tool's handler: checked arguments in, the JSON the matching command
//! prints with `--json` out (trimmed where a model doesn't need it all, such
//! as equity curves).

use super::{Args, ToolContext};
use crate::backtest::costs::CostPreset;
use crate::backtest::engine::Fill;
use crate::backtest::rules;
use crate::commands;
use crate::derivatives;
use crate::funds;
use crate::market::{self, Market};
use crate::output::{Format, round};
use crate::tax::Country;
use crate::views::{self, DataSource};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

pub(super) async fn run<D: DataSource>(
    ctx: &ToolContext<D>,
    name: &str,
    a: &Args,
) -> Result<Value> {
    let src = &ctx.source;
    let m = a.market(ctx.market);
    // Room for the envelope around the data.
    let budget = ctx.max_result_bytes.saturating_sub(super::NOTE.len() + 400);
    match name {
        "quote" => quote(src, a, m).await,
        "history" => history(src, a, m, budget).await,
        "technical" => technical(src, a, m).await,
        "risk" => risk(src, a, m).await,
        "search" => search(src, a).await,
        "markets" => Ok(json!({
            "indices": views::markets(src, m, a.bool("crypto"), a.bool("fx"), a.bool("global")).await?
        })),
        "mf_search" => mf_search(a).await,
        "mf_nav" => mf_nav(a).await,
        "mf_sip" => mf_sip(a).await,
        "options_chain" => {
            let chain = chain(a, m).await?;
            Ok(chain_within(&chain, a.usize_or("strikes", 6), budget))
        }
        "options_pcr" => Ok(commands::pcr_json(&chain(a, m).await?)),
        "options_maxpain" => {
            let mut v = commands::maxpain_view(&chain(a, m).await?)?;
            centre_curve(&mut v, 8);
            Ok(v)
        }
        "options_payoff" => options_payoff(a, m).await,
        "screen" => screen(src, a, m).await,
        "backtest" => backtest(src, a, m).await,
        "portfolio_summary" => {
            let base = a.str("base").map(str::to_ascii_uppercase);
            Ok(serde_json::to_value(
                views::portfolio(src, m, base.as_deref()).await?,
            )?)
        }
        "holdings_lots" => holdings_lots(src, a, m),
        "tax_estimate" => tax_estimate(src, a, m).await,
        "calendar" => calendar(src, a, m).await,
        "goal_sip" => goal_sip(a, m),
        "goal_retire" => goal_retire(a, m),
        "goal_education" => goal_education(a, m),
        "alerts_list" => Ok(json!({ "alerts": views::alerts(src).await? })),
        "watchlist" => Ok(json!({ "watchlist": views::watchlist(src).await? })),
        "order_preview" => order_preview(src, a).await,
        other => bail!("Tool '{}' has no handler", other),
    }
}

fn symbol(a: &Args, m: Market) -> Result<String> {
    Ok(market::resolve_symbol(a.need_str("symbol")?, m))
}

fn period<'a>(a: &'a Args, default: &'a str) -> Result<&'a str> {
    let p = a.str("period").unwrap_or(default);
    views::check_period(p)?;
    Ok(p)
}

/// Keep the first `n` items of the array at `key`.
fn truncate_at(v: &mut Value, key: &str, n: usize) {
    if let Some(Value::Array(items)) = v.get_mut(key) {
        items.truncate(n);
    }
}

fn size(v: &impl serde::Serialize) -> usize {
    serde_json::to_string(v).map_or(0, |s| s.len())
}

/// The newest of `items` whose JSON fits in `budget` bytes, oldest first.
pub(super) fn newest_within<T: serde::Serialize>(items: &[T], budget: usize) -> &[T] {
    let mut used = 2;
    let mut start = items.len();
    while start > 0 {
        let next = size(&items[start - 1]) + 1;
        if used + next > budget {
            break;
        }
        used += next;
        start -= 1;
    }
    &items[start..]
}

/// The option chain with `strikes` each side of the money, or fewer until
/// it fits in `budget` bytes: the strikes nearest the money matter most.
fn chain_within(chain: &derivatives::OptionChain, strikes: usize, budget: usize) -> Value {
    let mut n = strikes.max(1);
    loop {
        let mut v = commands::chain_json(chain, n, None, 0.0);
        truncate_at(&mut v, "expiries", 6);
        v["strikes_each_side"] = json!(n);
        if n == 1 || size(&v) <= budget {
            return v;
        }
        n = (n * 2 / 3).max(1);
    }
}

async fn quote(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let resolved: Vec<String> = a
        .list("symbols")
        .iter()
        .map(|s| market::resolve_symbol(s, m))
        .collect();
    let set = views::quotes(src, &resolved).await?;
    if set.quotes.is_empty() {
        bail!(
            "No quote for {}{}",
            set.missing.join(", "),
            market::not_found_hint(m)
        );
    }
    let rows: Vec<commands::QuoteRow> = set.quotes.iter().map(commands::QuoteRow::from).collect();
    Ok(json!({ "quotes": rows, "missing": set.missing }))
}

async fn history(src: &impl DataSource, a: &Args, m: Market, budget: usize) -> Result<Value> {
    let period = period(a, "6mo")?;
    let h = views::history(src, &symbol(a, m)?, period, m).await?;
    let closes: Vec<f64> = h.bars.iter().filter_map(|b| b.close).collect();
    let (first, last) = (closes.first().copied(), closes.last().copied());
    let high = h.bars.iter().filter_map(|b| b.high).reduce(f64::max);
    let low = h.bars.iter().filter_map(|b| b.low).reduce(f64::min);
    let limit = a.usize_or("limit", 60);
    let total = h.bars.len();
    // The newest bars, as many as fit next to the summary fields.
    let bars = newest_within(
        &h.bars[total.saturating_sub(limit)..],
        budget.saturating_sub(600),
    );
    Ok(json!({
        "symbol": h.symbol,
        "currency": h.currency,
        "period": h.period,
        "interval": h.interval,
        "bar_count": total,
        "first_close": first,
        "last_close": last,
        "change_percent": first.zip(last).filter(|(f, _)| *f != 0.0).map(|(f, l)| round((l / f - 1.0) * 100.0, 2)),
        "period_high": high,
        "period_low": low,
        "bars_shown": bars.len(),
        "bars": bars,
    }))
}

/// Closes, highs, lows and volumes of a chart, skipping missing values —
/// what `technical` and `risk` compute on.
fn series(chart: &crate::api::ChartData) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<u64>) {
    let q = chart.indicators.quote.first();
    let f = |v: Option<&Vec<Option<f64>>>| -> Vec<f64> {
        v.map(|c| c.iter().filter_map(|x| *x).collect())
            .unwrap_or_default()
    };
    (
        f(q.and_then(|q| q.close.as_ref())),
        f(q.and_then(|q| q.high.as_ref())),
        f(q.and_then(|q| q.low.as_ref())),
        q.and_then(|q| q.volume.as_ref())
            .map(|c| c.iter().filter_map(|x| *x).collect())
            .unwrap_or_default(),
    )
}

async fn technical(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let period = period(a, "6mo")?;
    let resolved = symbol(a, m)?;
    let interval = commands::period_to_interval(period);
    let chart = src
        .chart(&resolved, period, interval)
        .await
        .with_context(|| {
            format!(
                "No price history for {resolved}{}",
                market::not_found_hint(m)
            )
        })?;
    let (closes, highs, lows, volumes) = series(&chart);
    if closes.len() < 30 {
        bail!(
            "Not enough price data for technical analysis of {resolved} over {period} \
             ({} bars) — try a longer period such as 6mo or 1y",
            closes.len()
        );
    }
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    Ok(commands::technical_json(
        &resolved, cur, period, interval, &closes, &highs, &lows, &volumes,
    ))
}

async fn risk(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let period = period(a, "1y")?;
    let resolved = symbol(a, m)?;
    // Daily bars, as `risk` annualizes with 252 trading days.
    let chart = src.chart(&resolved, period, "1d").await.with_context(|| {
        format!(
            "No price history for {resolved}{}",
            market::not_found_hint(m)
        )
    })?;
    let (closes, ..) = series(&chart);
    if closes.len() < 30 {
        bail!(
            "Not enough price data for risk analysis of {resolved} over {period} ({} bars) — \
             try a longer period such as 1y or 2y",
            closes.len()
        );
    }
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    Ok(commands::risk_json(&resolved, cur, period, &closes))
}

async fn search(src: &impl DataSource, a: &Args) -> Result<Value> {
    let mut results = src.search(a.need_str("query")?).await?;
    results.truncate(a.usize_or("limit", 10));
    Ok(json!({ "results": results }))
}

/// A scheme by AMFI code (`122639`, `MF:122639`) or ISIN.
fn find_scheme<'a>(schemes: &'a [funds::Scheme], key: &str) -> Result<&'a funds::Scheme> {
    let key = key.trim();
    let key = key
        .strip_prefix("MF:")
        .or_else(|| key.strip_prefix("mf:"))
        .unwrap_or(key);
    let found = match key.parse::<u32>() {
        Ok(code) => schemes.iter().find(|s| s.code == code),
        Err(_) => schemes.iter().find(|s| {
            [&s.isin_growth, &s.isin_reinvest]
                .into_iter()
                .flatten()
                .any(|i| i.eq_ignore_ascii_case(key))
        }),
    };
    found.with_context(|| {
        format!("No scheme with code or ISIN {key} in AMFI's NAV list; find it with mf_search")
    })
}

async fn mf_search(a: &Args) -> Result<Value> {
    let schemes = funds::load_amfi().await?;
    let mut hits = funds::search(&schemes, a.need_str("query")?);
    match a.str("plan") {
        Some("direct") => hits.retain(|s| s.is_direct()),
        Some("regular") => hits.retain(|s| !s.is_direct()),
        _ => {}
    }
    let total = hits.len();
    hits.truncate(a.usize_or("limit", 10));
    Ok(json!({ "total_matches": total, "schemes": hits }))
}

async fn mf_nav(a: &Args) -> Result<Value> {
    let (schemes, ter) = tokio::join!(funds::load_amfi(), funds::load_ter());
    let schemes = schemes?;
    let scheme = find_scheme(&schemes, a.need_str("scheme")?)?;
    let ter = ter.ok().and_then(|t| t.for_scheme(scheme));
    Ok(serde_json::to_value(commands::NavRow {
        scheme,
        ter: &ter,
    })?)
}

async fn mf_sip(a: &Args) -> Result<Value> {
    let key = a.need_str("scheme")?;
    let code = match key.trim_start_matches("MF:").parse::<u32>() {
        Ok(code) => code,
        Err(_) => find_scheme(&funds::load_amfi().await?, key)?.code,
    };
    let amount = a.need_f64("monthly")?;
    let run = commands::run_sip(code, amount, a.str("period").unwrap_or("5y")).await?;
    let mut v = run.json(code, amount);
    // One row per instalment is more than a model needs.
    if let Some(o) = v.as_object_mut() {
        o.remove("history");
    }
    Ok(v)
}

async fn chain(a: &Args, m: Market) -> Result<derivatives::OptionChain> {
    let expiry = match a.str("expiry") {
        None => None,
        Some(e) => Some(
            derivatives::parse_expiry(e)
                .with_context(|| format!("Expiry '{e}' is not a date like 2026-11-27"))?,
        ),
    };
    derivatives::fetch_chain(a.need_str("symbol")?, m, expiry).await
}

/// Keep `half` points either side of the max-pain strike.
fn centre_curve(v: &mut Value, half: usize) {
    let strike = v["max_pain"].as_f64();
    if let (Some(k), Some(Value::Array(curve))) = (strike, v.get_mut("curve")) {
        let i = curve
            .iter()
            .position(|p| p["strike"].as_f64() == Some(k))
            .unwrap_or(0);
        let lo = i.saturating_sub(half);
        let hi = (i + half + 1).min(curve.len());
        *curve = curve[lo..hi].to_vec();
    }
}

async fn options_payoff(a: &Args, m: Market) -> Result<Value> {
    use crate::payoff::Preset;
    let strategy = match a.need_str("strategy")? {
        "bull-call" => Preset::BullCall,
        "bear-put" => Preset::BearPut,
        "straddle" => Preset::Straddle,
        "strangle" => Preset::Strangle,
        "iron-condor" => Preset::IronCondor,
        "butterfly" => Preset::Butterfly,
        "covered-call" => Preset::CoveredCall,
        other => bail!("Unknown strategy '{}'", other),
    };
    let expiry = match a.str("expiry") {
        None => None,
        Some(e) => Some(
            derivatives::parse_expiry(e)
                .with_context(|| format!("Expiry '{e}' is not a date like 2026-11-27"))?,
        ),
    };
    let args = commands::PayoffArgs {
        legs: Vec::new(),
        strategy: Some(strategy),
        width: a.f64("width"),
        expiry,
        lots: a.i64("lots").unwrap_or(1) as u32,
        lot_size: None,
        rate_pct: None,
        div_yield_pct: 0.0,
        iv_pct: None,
        today: false,
        json: true,
    };
    let analysis = commands::payoff_analysis(a.need_str("symbol")?, &args, m).await?;
    let mut v = analysis.json(&args);
    // 120 grid points draw a chart; 20 describe the shape.
    if let Some(Value::Array(curve)) = v.get_mut("curve") {
        let step = curve.len().div_ceil(20).max(1);
        let last = curve.last().cloned();
        let mut thin: Vec<Value> = curve.iter().step_by(step).cloned().collect();
        if let Some(l) = last
            && thin.last() != Some(&l)
        {
            thin.push(l);
        }
        *curve = thin;
    }
    Ok(v)
}

async fn screen(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    m.require_stocks("screen")?;
    let universe = m.universe();
    let refs: Vec<&str> = universe.iter().map(String::as_str).collect();
    let quotes = src.quotes(&refs).await?;
    let (title, matches) = commands::screen_matches(a.need_str("category")?, &quotes)?;
    let total = matches.len();
    let rows: Vec<commands::ScreenRow> = matches
        .into_iter()
        .take(a.usize_or("limit", 20))
        .map(commands::ScreenRow::from)
        .collect();
    Ok(json!({
        "screen": title,
        "universe": m.label(),
        "scanned": quotes.len(),
        "matches": total,
        "stocks": rows,
    }))
}

async fn backtest(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let strategy = a.str("strategy").unwrap_or("rsi");
    // Only bundled strategies: a model must not point this at files.
    if !rules::is_builtin_name(strategy) {
        bail!(
            "Unknown strategy '{}'. Built-in strategies: {}",
            strategy,
            rules::builtin_names().join(", ")
        );
    }
    let period = a.str("period").unwrap_or("2y");
    let opts = commands::BacktestOpts {
        strategy: strategy.to_string(),
        params: Vec::new(),
        costs: CostPreset::Auto,
        brokerage: None,
        slippage_bps: None,
        fill: Fill::NextOpen,
        capital: a.f64("capital").unwrap_or(100_000.0),
        fmt: Format::Json,
    };
    let run = commands::run_single(src, a.need_str("symbol")?, period, &opts, m).await?;
    let mut v = run.json(period, &opts)?;
    if let Some(o) = v.as_object_mut() {
        o.remove("equity");
        if let Some(Value::Array(trades)) = o.get_mut("trades") {
            let n = trades.len();
            trades.drain(..n.saturating_sub(10));
        }
        o.insert(
            "trades_shown".into(),
            json!("the 10 most recent; trade_count has the total"),
        );
    }
    Ok(v)
}

/// The tax rules for lot terms and estimates: the user's choice, else the
/// market's country (India for everything but the US and UK).
fn country(a: &Args, m: Market) -> Result<Country> {
    let code = match (a.str("country"), m) {
        (Some(c), _) => c,
        (None, Market::Us) => "us",
        (None, Market::Global(market::Exchange::Uk)) => "uk",
        (None, _) => "in",
    };
    Country::parse(code).with_context(|| format!("Unsupported country '{code}'"))
}

fn holdings_lots(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let p = src.portfolio().map_err(|e| e.context(views::LocalData))?;
    let country = country(a, m)?;
    let today = chrono::Local::now().date_naive();
    let only = match a.str("symbol") {
        None => None,
        Some(s) => {
            let held = commands::held_symbol(&p, s, m);
            if p.holding(&held).is_none() {
                bail!(
                    "{} is not in the portfolio (holdings: {})",
                    held,
                    p.holdings
                        .iter()
                        .map(|h| h.symbol.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            Some(held)
        }
    };
    let mut lots = Vec::new();
    for h in p
        .holdings
        .iter()
        .filter(|h| only.as_ref().is_none_or(|s| *s == h.symbol))
    {
        for (i, lot) in h.lots.iter().enumerate() {
            let bought = chrono::NaiveDate::parse_from_str(&lot.date, "%Y-%m-%d").ok();
            lots.push(json!({
                "symbol": h.symbol,
                "lot": i + 1,
                "date": lot.date,
                "date_estimated": lot.date_estimated,
                "qty": lot.qty,
                "price": lot.price,
                "cost": round(lot.cost(), 2),
                "days_held": bought.map(|d| (today - d).num_days()),
                "term": bought.map(|d| if country.is_long_term(d, today) { "long" } else { "short" }),
                "source": lot.source,
            }));
        }
    }
    let total = lots.len();
    lots.truncate(a.usize_or("limit", 100));
    let sales: Vec<&crate::portfolio::Sale> = p
        .realized
        .iter()
        .filter(|s| only.as_ref().is_none_or(|o| *o == s.symbol))
        .rev()
        .take(20)
        .collect();
    Ok(json!({
        "tax_rules": country.name(),
        "long_term_after_months": country.long_term_months(),
        "total_lots": total,
        "lots": lots,
        "recent_sales": sales,
    }))
}

async fn tax_estimate(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let country = country(a, m)?;
    let p = src.portfolio().map_err(|e| e.context(views::LocalData))?;
    let symbols: Vec<&str> = p.holdings.iter().map(|h| h.symbol.as_str()).collect();
    let quotes = if symbols.is_empty() {
        Vec::new()
    } else {
        src.quotes(&symbols).await?
    };
    let today = chrono::Local::now().date_naive();
    let report = commands::tax_report(country, &p, &quotes, today);
    Ok(commands::tax_json(country, &report, today))
}

async fn calendar(src: &impl DataSource, a: &Args, m: Market) -> Result<Value> {
    let mut symbols: Vec<String> = a
        .list("symbols")
        .iter()
        .map(|s| market::resolve_symbol(s, m))
        .collect();
    if symbols.is_empty() {
        let p = src.portfolio().map_err(|e| e.context(views::LocalData))?;
        let wl = src.watchlist().map_err(|e| e.context(views::LocalData))?;
        symbols.extend(p.holdings.into_iter().map(|h| h.symbol));
        symbols.extend(wl.symbols);
    }
    let mut seen = std::collections::HashSet::new();
    symbols.retain(|s| seen.insert(s.clone()));
    symbols.truncate(30);
    if symbols.is_empty() {
        return Ok(json!({
            "events": [],
            "note": "No symbols: pass some, or add holdings or a watchlist",
        }));
    }
    let days = a.i64("days").unwrap_or(30) as u32;
    let client = src.yahoo().await?;
    let (rows, _, notes) = commands::calendar_rows(client, &symbols, days).await;
    Ok(json!({
        "symbols": symbols,
        "days": days,
        "events": rows,
        "source_notes": notes,
    }))
}

fn pct(a: &Args, name: &str, default: f64) -> f64 {
    a.f64(name).unwrap_or(default)
}

fn goal_sip(a: &Args, m: Market) -> Result<Value> {
    let plan = crate::goals::plan_sip(
        a.need_f64("target")?,
        a.i64("years").unwrap_or(1) as u32,
        pct(a, "return_pct", 12.0),
        pct(a, "step_up_pct", 0.0),
        pct(a, "savings", 0.0),
    );
    commands::goal_json("sip", commands::market_currency(m), &plan)
}

fn goal_retire(a: &Args, m: Market) -> Result<Value> {
    let plan = crate::goals::plan_retirement(
        a.need_f64("monthly_expense")?,
        a.i64("years_to_retire").unwrap_or(1) as u32,
        a.i64("years_in_retirement").unwrap_or(1) as u32,
        pct(a, "inflation_pct", 6.0),
        pct(a, "return_pre_pct", 12.0),
        pct(a, "return_post_pct", 7.0),
        pct(a, "step_up_pct", 0.0),
        pct(a, "savings", 0.0),
    );
    commands::goal_json("retire", commands::market_currency(m), &plan)
}

fn goal_education(a: &Args, m: Market) -> Result<Value> {
    let plan = crate::goals::plan_education(
        a.need_f64("cost")?,
        a.i64("years").unwrap_or(1) as u32,
        pct(a, "inflation_pct", 8.0),
        pct(a, "return_pct", 12.0),
        pct(a, "step_up_pct", 0.0),
        pct(a, "savings", 0.0),
    );
    commands::goal_json("education", commands::market_currency(m), &plan)
}

/// A dry-run check of an order the user is considering. Sends nothing and
/// never reads broker credentials: it prices the order from the last quote
/// and returns the `stockwise trade` command, whose own preview, value cap
/// and typed confirmation then apply.
async fn order_preview(src: &impl DataSource, a: &Args) -> Result<Value> {
    use crate::brokers::{self, OrderKind, Side};
    let side = match a.need_str("side")? {
        "buy" => Side::Buy,
        _ => Side::Sell,
    };
    let qty = a.i64("quantity").unwrap_or(0) as u32;
    // The symbol ends up in a command the user may paste into a shell.
    let sym = brokers::nse_symbol(a.need_str("symbol")?)?;
    if sym.len() > 20
        || !sym
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '&' || c == '-')
    {
        bail!(
            "'{}' is not an NSE symbol (letters, digits, & and - only)",
            sym
        );
    }
    // Orders go through Indian brokers on NSE, whatever the default market.
    let resolved = market::resolve_symbol(&sym, Market::In);
    let (limit, trigger) = (a.f64("limit"), a.f64("trigger"));
    let kind = match (limit, trigger) {
        (None, None) => OrderKind::Market,
        (Some(price), None) => OrderKind::Limit { price },
        (Some(price), Some(trigger)) => OrderKind::StopLimit { trigger, price },
        (None, Some(trigger)) => OrderKind::StopMarket { trigger },
    };
    let quotes = views::quotes(src, std::slice::from_ref(&resolved)).await?;
    if quotes.quotes.is_empty() {
        bail!("No NSE quote for {resolved}; check the symbol with the search tool");
    }
    let last = quotes.quotes.first().and_then(|q| q.regular_market_price);
    brokers::validate_order(side, &kind, last)?;
    let est = brokers::estimate_value(&kind, qty, last);
    let broker = a.need_str("broker")?;
    let mut command = format!(
        "stockwise trade {} {} {} --broker {}",
        if side == Side::Buy { "buy" } else { "sell" },
        sym,
        qty,
        broker
    );
    if let Some(p) = limit {
        command.push_str(&format!(" --limit {p}"));
    }
    if let Some(t) = trigger {
        command.push_str(&format!(" --trigger {t}"));
    }
    Ok(json!({
        "status": "preview only — nothing was sent to a broker",
        "side": if side == Side::Buy { "BUY" } else { "SELL" },
        "symbol": sym,
        "exchange": "NSE",
        "quantity": qty,
        "order_type": match kind {
            OrderKind::Market => "MARKET",
            OrderKind::Limit { .. } => "LIMIT",
            OrderKind::StopLimit { .. } => "SL",
            OrderKind::StopMarket { .. } => "SL-M",
        },
        "limit": limit,
        "trigger": trigger,
        "last_price": last,
        "estimated_value_inr": est.map(|v| round(v, 2)),
        "dry_run_command": format!("{command} --dry-run"),
        "command": command,
        "next_step": "Show the user the command. Only they can place the order: `stockwise trade` \
                      shows its own preview, checks max_order_value from their broker config and \
                      asks them to type yes. Try --dry-run first.",
    }))
}
