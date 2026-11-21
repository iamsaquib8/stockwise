//! More read tools over existing commands, most of them for Indian
//! investors: G-sec yields, the list of government securities and the bond
//! calculator, gold and silver in rupees with Sovereign Gold Bonds, the
//! rupee against another currency, trailing returns, correlation, a side by
//! side comparison and the portfolio's year in review. Their definitions
//! are in [`super::TOOLS`].

use super::args::Args;
use crate::market::{self, Market};
use crate::views::DataSource;
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Run one of this module's tools.
pub(super) async fn run<D: DataSource>(
    src: &D,
    name: &str,
    a: &Args,
    m: Market,
    budget: usize,
) -> Result<Value> {
    match name {
        "bonds_yields" => {
            let set = crate::gsec::yields(crate::commands::BONDS_TIMEOUT).await?;
            Ok(json!({ "yields": set.rows }))
        }
        "bonds_list" => {
            let (list, _) = crate::gsec::securities(crate::commands::BONDS_TIMEOUT).await?;
            let rows: Vec<_> = list
                .into_iter()
                .filter(|s| {
                    a.str("query")
                        .is_none_or(|q| crate::commands::matches_query(s, q))
                })
                .collect();
            let total = rows.len();
            Ok(json!({ "securities": rows, "count": total }))
        }
        "bonds_calc" => bonds_calc(a).await,
        "commodity" => commodity(src, a.str("view").unwrap_or("spot")).await,
        "fx_history" => fx_history(src, a, budget).await,
        "returns" => returns(src, a, m).await,
        "correlate" => correlate(src, a, m).await,
        "compare" => compare(src, a, m).await,
        "portfolio_wrap" => {
            let period = match (a.str("year"), a.str("fy")) {
                (Some(y), _) => {
                    Some(crate::wrap::Period::parse_year(y).map_err(anyhow::Error::msg)?)
                }
                (None, Some(f)) => {
                    Some(crate::wrap::Period::parse_fy(f).map_err(anyhow::Error::msg)?)
                }
                (None, None) => None,
            };
            let base = a.str("base").map(str::to_ascii_uppercase);
            let report =
                crate::commands::wrap_report(src, m, base.as_deref(), period, false).await?;
            let report = if a.bool("redact") {
                crate::wrap::redact(&report)
            } else {
                report
            };
            Ok(serde_json::to_value(report)?)
        }
        other => bail!("Tool '{}' has no handler", other),
    }
}

fn date(a: &Args, key: &str) -> Result<Option<chrono::NaiveDate>> {
    a.str(key)
        .map(|s| {
            chrono::NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d")
                .with_context(|| format!("{key} must be a date like 2033-06-15"))
        })
        .transpose()
}

async fn bonds_calc(a: &Args) -> Result<Value> {
    let input = crate::commands::CalcInput {
        security: a.str("security"),
        coupon: a.f64("coupon"),
        maturity: date(a, "maturity")?,
        price: a.f64("price"),
        ytm: a.f64("ytm"),
        settle: date(a, "settle")?,
        tbill: a.bool("tbill"),
        dirty: a.bool("dirty"),
    };
    let settle = input
        .settle
        .unwrap_or_else(|| crate::clock::session_date(Market::In, chrono::Utc::now()));
    let found = match input.security {
        Some(name) if input.coupon.is_none() || input.maturity.is_none() => {
            let (list, _) = crate::gsec::securities(crate::commands::BONDS_TIMEOUT)
                .await
                .with_context(|| {
                    format!(
                        "Cannot look up '{name}' in RBI's list of G-secs; pass coupon and maturity instead"
                    )
                })?;
            Some(crate::gsec::lookup(&list, name)?.clone())
        }
        _ => None,
    };
    Ok(serde_json::to_value(crate::commands::calculate(
        &input,
        found.as_ref(),
        settle,
    )?)?)
}

async fn commodity<D: DataSource>(src: &D, view: &str) -> Result<Value> {
    use crate::commodity as cm;
    Ok(match view {
        "futures" | "etfs" => {
            let list = if view == "futures" { cm::FUTURES } else { cm::ETFS };
            let symbols: Vec<&str> = list.iter().map(|(s, ..)| *s).collect();
            let quotes = src.quotes(&symbols).await?;
            json!({ "view": view, "rows": cm::instrument_rows(list, &quotes) })
        }
        "sgb" => {
            let quotes = src.quotes(&[cm::GOLD_FUTURE, cm::USDINR]).await?;
            let gold = cm::gold_inr_per_gram(&quotes);
            let (live, warning) = match src.sgb_list().await {
                Ok(list) => (Some(list.rows), None),
                Err(e) => (
                    None,
                    Some(format!(
                        "NSE's SGB prices were not available ({}); issue terms only",
                        super::client_error(&e)
                    )),
                ),
            };
            let today = crate::clock::session_date(Market::In, chrono::Utc::now());
            json!({
                "view": "sgb",
                "gold_inr_per_gram": gold,
                "rows": cm::sgb_rows(live.as_deref(), gold, today),
                "warning": warning,
            })
        }
        _ => {
            let quotes = src
                .quotes(&[cm::GOLD_FUTURE, cm::SILVER_FUTURE, cm::USDINR])
                .await?;
            let rows = cm::spot_rows(&quotes);
            if rows.iter().all(|r| r.price.is_none()) {
                bail!("No gold or silver quote from Yahoo Finance (GC=F, SI=F); try again shortly");
            }
            json!({ "view": "spot", "rows": rows })
        }
    })
}

fn currency_code(s: &str, what: &str) -> Result<String> {
    let c = s.trim().to_ascii_uppercase();
    if c.len() != 3 || !c.chars().all(|ch| ch.is_ascii_alphabetic()) {
        bail!("{what} must be a 3-letter currency code such as USD or INR");
    }
    Ok(c)
}

async fn fx_history<D: DataSource>(src: &D, a: &Args, budget: usize) -> Result<Value> {
    let cur = currency_code(a.str("currency").unwrap_or("USD"), "currency")?;
    let base = currency_code(a.str("base").unwrap_or("INR"), "base")?;
    if cur == base {
        bail!("currency and base are both {cur}");
    }
    let period = a.str("period").unwrap_or("1y");
    let pair = format!("{cur}{base}=X");
    let chart = src.chart(&pair, period, "1d").await?;
    let closes = chart.dated_closes();
    let Some(&(_, first)) = closes.first() else {
        bail!("No rates for {cur}/{base}");
    };
    let (last_date, last) = closes[closes.len() - 1];
    let high = closes.iter().map(|(_, c)| *c).fold(f64::MIN, f64::max);
    let low = closes.iter().map(|(_, c)| *c).fold(f64::MAX, f64::min);
    let rows: Vec<Value> = closes
        .iter()
        .map(|(d, c)| json!({ "date": d, "rate": crate::output::round(*c, 4) }))
        .collect();
    let recent = super::handlers::newest_within(&rows, budget.saturating_sub(600));
    Ok(json!({
        "pair": format!("{cur}/{base}"),
        "symbol": pair,
        "period": period,
        "first": first,
        "last": last,
        "last_date": last_date,
        "change_percent": (first != 0.0).then(|| crate::output::round((last / first - 1.0) * 100.0, 2)),
        "high": high,
        "low": low,
        "days": closes.len(),
        "rates": recent,
    }))
}

async fn returns<D: DataSource>(src: &D, a: &Args, m: Market) -> Result<Value> {
    let sym = market::resolve_symbol(a.need_str("symbol")?.trim(), m);
    let set = crate::views::quotes(src, std::slice::from_ref(&sym)).await?;
    let q = set
        .quotes
        .first()
        .with_context(|| format!("No quote for {}{}", sym, market::not_found_hint(m)))?;
    let mut rows = Vec::new();
    for (range, label) in [
        ("5d", "1 Week"),
        ("1mo", "1 Month"),
        ("3mo", "3 Months"),
        ("6mo", "6 Months"),
        ("1y", "1 Year"),
        ("2y", "2 Years"),
        ("5y", "5 Years"),
    ] {
        if let Ok(chart) = src.chart(&sym, range, "1d").await {
            rows.extend(crate::commands::return_row(
                range,
                label,
                &chart.dated_closes(),
            ));
        }
    }
    Ok(json!({
        "symbol": sym,
        "currency": q.currency,
        "price": q.regular_market_price,
        "returns": rows,
        "about": "Price returns; dividends not included. growth_of_100000 is what 100,000 (₹1 lakh for rupee stocks) became.",
    }))
}

async fn correlate<D: DataSource>(src: &D, a: &Args, m: Market) -> Result<Value> {
    let mut syms: Vec<String> = a
        .list("symbols")
        .iter()
        .map(|s| market::resolve_symbol(s.trim(), m))
        .collect();
    syms.dedup();
    if syms.len() < 2 {
        bail!("Give at least 2 different symbols to correlate");
    }
    let mut series = Vec::new();
    let mut missing = Vec::new();
    for s in &syms {
        match src.chart(s, "1y", "1d").await {
            Ok(c) => series.push((s.clone(), c.dated_closes())),
            Err(_) => missing.push(s.clone()),
        }
    }
    if series.len() < 2 {
        bail!(
            "Not enough price history to correlate (none for {}){}",
            missing.join(", "),
            market::not_found_hint(m)
        );
    }
    let matrix = crate::commands::correlation_matrix(&series);
    let names: Vec<&str> = series.iter().map(|(s, _)| s.as_str()).collect();
    Ok(json!({
        "period": "1y",
        "interval": "1d",
        "symbols": names,
        "matrix": matrix,
        "missing": missing,
    }))
}

async fn compare<D: DataSource>(src: &D, a: &Args, m: Market) -> Result<Value> {
    let syms: Vec<String> = a
        .list("symbols")
        .iter()
        .map(|s| market::resolve_symbol(s.trim(), m))
        .collect();
    let client = src.yahoo().await?;
    let refs: Vec<&str> = syms.iter().map(String::as_str).collect();
    let mut quotes = client.get_quote(&refs).await?;
    if quotes.is_empty() {
        bail!("No quotes for {}{}", syms.join(", "), market::not_found_hint(m));
    }
    for q in &mut quotes {
        let _ = crate::research::enrich_quote(client, q, false).await;
    }
    let rows: Vec<crate::commands::CompareRow> =
        quotes.iter().map(crate::commands::CompareRow::from).collect();
    let found: Vec<String> = quotes.iter().filter_map(|q| q.symbol.clone()).collect();
    let missing: Vec<&String> = syms.iter().filter(|s| !found.contains(s)).collect();
    Ok(json!({ "stocks": rows, "missing": missing }))
}
