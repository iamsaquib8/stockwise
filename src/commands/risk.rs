//! Risk and return statistics: risk metrics, correlation, returns, forecasts, and option-style expected moves.

use super::*;

/// Risk label for an annualized volatility (as a fraction).
fn risk_level(vol: f64) -> &'static str {
    if vol > 0.5 {
        "Very High"
    } else if vol > 0.3 {
        "High"
    } else if vol > 0.2 {
        "Moderate"
    } else {
        "Low"
    }
}

/// Daily bars in a year for `symbol`: 252 for stocks, 365 for crypto, 260 for
/// FX (the same table the backtest engine uses).
fn bars_per_year(symbol: &str) -> f64 {
    crate::backtest::engine::bars_per_year([symbol])
}

/// `risk --json`: the metrics the table shows, from daily closes.
/// Percentages are rounded to 2 decimals, ratios to 3.
pub(crate) fn risk_json(
    symbol: &str,
    currency: Option<&str>,
    period: &str,
    closes: &[f64],
) -> serde_json::Value {
    let current = closes.last().copied().unwrap_or(f64::NAN);
    let first = closes.first().copied().unwrap_or(f64::NAN);
    let ppy = bars_per_year(symbol);
    let vol = technical::annualized_volatility_with(closes, ppy);
    let pct = |v: Option<f64>| v.map(|x| round(x * 100.0, 2));
    serde_json::json!({
        "symbol": symbol,
        "currency": currency,
        "period": period,
        "interval": "1d",
        "bars": closes.len(),
        "price": current,
        "return_percent": round((current / first - 1.0) * 100.0, 2),
        "volatility_percent": pct(vol),
        "risk_level": vol.map(|v| risk_level(v).to_lowercase().replace(' ', "_")),
        "sharpe": round_opt(technical::sharpe_ratio_with(closes, 0.05, ppy), 3),
        "sortino": round_opt(technical::sortino_ratio_with(closes, 0.05, ppy), 3),
        "calmar": round_opt(technical::calmar_ratio_with(closes, ppy), 3),
        "max_drawdown": technical::max_drawdown(closes).map(|(mdd, peak, trough)| {
            serde_json::json!({
                "percent": round(mdd * 100.0, 2),
                "peak_price": closes[peak],
                "trough_price": closes[trough],
                "trading_days": trough as i64 - peak as i64,
            })
        }),
        "var_95_percent": pct(technical::value_at_risk(closes, 0.95)),
        "var_99_percent": pct(technical::value_at_risk(closes, 0.99)),
    })
}

pub async fn cmd_risk(symbol: &str, period: &str, market: Market, fmt: Format) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    // Always daily bars: every metric below annualizes with the symbol's bars
    // per year (252 for stocks, 365 for crypto).
    let ppy = bars_per_year(&resolved);
    let chart = client.get_chart(&resolved, period, "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    if closes.len() < 30 {
        bail!("Not enough price data for risk analysis — try a longer period, e.g. -p 2y");
    }

    if fmt == Format::Json {
        return output::print_json(&risk_json(&resolved, cur, period, &closes));
    }

    let current = *closes.last().unwrap();
    let first = closes[0];
    let total_return = (current / first - 1.0) * 100.0;

    print_header(&format!("Risk Analysis: {}", resolved));
    println!(
        "  Current: {}  |  Return ({}): {}\n",
        format_price(current, cur).bold(),
        period,
        if total_return >= 0.0 {
            format!("+{:.2}%", total_return).green().to_string()
        } else {
            format!("{:.2}%", total_return).red().to_string()
        }
    );

    print_section("Volatility Metrics");
    if let Some(vol) = technical::annualized_volatility_with(&closes, ppy) {
        print_kv("Annual Volatility", &format!("{:.2}%", vol * 100.0));
        let level = risk_level(vol);
        let level = if vol > 0.3 {
            level.red().to_string()
        } else if vol > 0.2 {
            level.yellow().to_string()
        } else {
            level.green().to_string()
        };
        print_kv("Risk Level", &level);
    }

    print_section("Risk-Adjusted Returns");
    if let Some(sharpe) = technical::sharpe_ratio_with(&closes, 0.05, ppy) {
        let color = if sharpe > 1.0 {
            format!("{:.3}", sharpe).green().to_string()
        } else if sharpe > 0.0 {
            format!("{:.3}", sharpe).yellow().to_string()
        } else {
            format!("{:.3}", sharpe).red().to_string()
        };
        print_kv("Sharpe Ratio", &color);
        let interp = match sharpe {
            s if s > 2.0 => "Excellent risk-adjusted returns",
            s if s > 1.0 => "Good risk-adjusted returns",
            s if s > 0.0 => "Acceptable but below average",
            _ => "Poor — not compensating for risk",
        };
        print_kv("Interpretation", interp);
    }
    if let Some(sortino) = technical::sortino_ratio_with(&closes, 0.05, ppy) {
        print_kv("Sortino Ratio", &format!("{:.3}", sortino));
    }
    if let Some(calmar) = technical::calmar_ratio_with(&closes, ppy) {
        print_kv("Calmar Ratio", &format!("{:.3}", calmar));
    }

    print_section("Drawdown Analysis");
    if let Some((mdd, peak_idx, trough_idx)) = technical::max_drawdown(&closes) {
        print_kv(
            "Max Drawdown",
            &format!("-{:.2}%", mdd * 100.0).red().to_string(),
        );
        print_kv(
            "Peak → Trough",
            &format!(
                "{}{:.2} → {}{:.2}",
                csym, closes[peak_idx], csym, closes[trough_idx]
            ),
        );
        print_kv(
            "Duration",
            &format!("{} trading days", trough_idx as i64 - peak_idx as i64),
        );
    }

    print_section("Value at Risk (VaR)");
    if let Some(var95) = technical::value_at_risk(&closes, 0.95) {
        print_kv(
            "Daily VaR (95%)",
            &format!("{:.2}%", var95 * 100.0).red().to_string(),
        );
    }
    if let Some(var99) = technical::value_at_risk(&closes, 0.99) {
        print_kv(
            "Daily VaR (99%)",
            &format!("{:.2}%", var99 * 100.0).red().to_string(),
        );
    }

    print_section(&format!("Price Chart ({})", period));
    let color = if *closes.last().unwrap() >= closes[0] {
        "green"
    } else {
        "red"
    };
    for line in charts::line_chart(&closes, 50, 8, color, "") {
        println!("{}", line);
    }

    println!(
        "\n  {}",
        "Risk metrics based on historical data. Past performance ≠ future results."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

// ── Correlation ──

/// Pairwise correlation matrix (diagonal 1.0), rounded to 4 decimals, from
/// the returns on dates both series traded; `None` where two series share too
/// few dates to compare (see [`technical::dated_correlation`]).
pub(crate) fn correlation_matrix(
    series: &[(String, Vec<(chrono::NaiveDate, f64)>)],
) -> Vec<Vec<Option<f64>>> {
    series
        .iter()
        .enumerate()
        .map(|(i, (_, a))| {
            series
                .iter()
                .enumerate()
                .map(|(j, (_, b))| {
                    if i == j {
                        Some(1.0)
                    } else {
                        round_opt(technical::dated_correlation(a, b), 4)
                    }
                })
                .collect()
        })
        .collect()
}

/// `correlate --csv`: a header of symbols, then one row per symbol.
pub(crate) fn correlation_csv(symbols: &[&str], matrix: &[Vec<Option<f64>>]) -> String {
    let mut header = vec!["symbol"];
    header.extend_from_slice(symbols);
    let mut out = output::csv_line(&header);
    for (sym, row) in symbols.iter().zip(matrix) {
        let mut cells = vec![sym.to_string()];
        // Same number formatting as the JSON output (1.0, not 1).
        cells.extend(row.iter().map(|c| output::csv_cell(&serde_json::json!(c))));
        out.push_str(&output::csv_line(&cells));
    }
    out
}

pub async fn cmd_correlate(symbols: &[String], market: Market, fmt: Format) -> Result<()> {
    if symbols.len() < 2 {
        bail!("Need at least 2 symbols to correlate");
    }
    let resolved = resolve_symbols(symbols, market);
    let client = YahooClient::new().await?;

    let mut all_closes: Vec<(String, Vec<(chrono::NaiveDate, f64)>)> = Vec::new();
    for sym in &resolved {
        let chart = client.get_chart(sym, "1y", "1d").await?;
        all_closes.push((sym.clone(), chart.dated_closes()));
    }
    let matrix = correlation_matrix(&all_closes);
    let names: Vec<&str> = all_closes.iter().map(|(s, _)| s.as_str()).collect();
    match fmt {
        Format::Json => {
            return output::print_json(&serde_json::json!({
                "period": "1y",
                "interval": "1d",
                "symbols": names,
                "matrix": matrix,
            }));
        }
        Format::Csv => {
            print!("{}", correlation_csv(&names, &matrix));
            return Ok(());
        }
        Format::Table => {}
    }

    print_header("Correlation Matrix (1Y daily returns)");
    print!("  {:<12}", "");
    for (sym, _) in &all_closes {
        print!(" {:>10}", sym.cyan());
    }
    println!();
    println!("  {}", "─".repeat(12 + all_closes.len() * 11).dimmed());

    for (i, (sym_a, _)) in all_closes.iter().enumerate() {
        print!("  {:<12}", sym_a.cyan());
        for (j, cell) in matrix[i].iter().enumerate() {
            if i == j {
                print!(" {:>10}", "1.000".bold());
            } else {
                // Too little overlapping data: say so rather than showing 0.
                let Some(corr) = *cell else {
                    print!(" {:>10}", "N/A".dimmed());
                    continue;
                };
                let colored = if corr > 0.7 {
                    format!("{:.3}", corr).green().to_string()
                } else if corr > 0.3 {
                    format!("{:.3}", corr).yellow().to_string()
                } else if corr > -0.3 {
                    format!("{:.3}", corr).to_string()
                } else {
                    format!("{:.3}", corr).red().to_string()
                };
                print!(" {:>10}", colored);
            }
        }
        println!();
    }
    println!(
        "\n  {} >0.7 strong  {} 0.3-0.7 moderate  {} <-0.3 inverse",
        "●".green(),
        "●".yellow(),
        "●".red()
    );
    println!();
    Ok(())
}

// ── Export ──

/// One period of `returns --json/--csv`. `growth_of_100000` is what 1,00,000
/// (in the stock's currency) at the start would be worth at the end.
#[derive(Debug, Serialize)]
pub(crate) struct ReturnRow {
    pub(crate) period: &'static str,
    pub(crate) label: &'static str,
    pub(crate) start_price: f64,
    pub(crate) end_price: f64,
    pub(crate) return_percent: f64,
    /// `None` (null) for a span under a year.
    pub(crate) annualized_percent: Option<f64>,
    pub(crate) growth_of_100000: f64,
}

impl CsvRow for ReturnRow {
    const COLUMNS: &'static [&'static str] = &[
        "period",
        "label",
        "start_price",
        "end_price",
        "return_percent",
        "annualized_percent",
        "growth_of_100000",
    ];
}

/// Spans shorter than this many calendar days get no annualized figure: a
/// month's return compounded over a year says little.
const MIN_ANNUALIZED_DAYS: i64 = 360;

/// Return over a series of dated daily closes; `None` with fewer than two.
/// The annualized figure uses the calendar time between the first and last
/// bar (`growth ^ (365.25 / days) - 1`), so it is the same for a stock and for
/// crypto, and a one-year span annualizes to its own return. It is `None` for
/// spans under a year.
pub(crate) fn return_row(
    period: &'static str,
    label: &'static str,
    closes: &[(chrono::NaiveDate, f64)],
) -> Option<ReturnRow> {
    if closes.len() < 2 {
        return None;
    }
    let (first_date, first) = closes[0];
    let (last_date, last) = *closes.last()?;
    let ret = ((last / first) - 1.0) * 100.0;
    let days = (last_date - first_date).num_days();
    let annual = (days >= MIN_ANNUALIZED_DAYS && first > 0.0 && last > 0.0)
        .then(|| ((last / first).powf(365.25 / days as f64) - 1.0) * 100.0);
    Some(ReturnRow {
        period,
        label,
        start_price: first,
        end_price: last,
        return_percent: round(ret, 2),
        annualized_percent: annual.map(|a| round(a, 2)),
        growth_of_100000: round(100000.0 * (last / first), 2),
    })
}

pub async fn cmd_returns(symbol: &str, market: Market, fmt: Format) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[resolved.as_str()]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);

    let periods = [
        ("5d", "1 Week"),
        ("1mo", "1 Month"),
        ("3mo", "3 Months"),
        ("6mo", "6 Months"),
        ("1y", "1 Year"),
        ("2y", "2 Years"),
        ("5y", "5 Years"),
    ];
    let mut rows = Vec::new();
    for (range, label) in periods {
        let chart = match client.get_chart(&resolved, range, "1d").await {
            Ok(c) => c,
            Err(_) => continue,
        };
        rows.extend(return_row(range, label, &chart.dated_closes()));
    }
    match fmt {
        Format::Json => {
            return output::print_json(&serde_json::json!({
                "symbol": resolved,
                "currency": cur,
                "price": q.regular_market_price,
                "returns": rows,
            }));
        }
        Format::Csv => return output::print_csv(&rows),
        Format::Table => {}
    }

    print_header(&match q.regular_market_price {
        Some(price) => format!("Returns: {} @ {}{:.2}", resolved, csym, price),
        None => format!("Returns: {}", resolved),
    });
    println!(
        "  {:<14} {:>10} {:>12} {:>14}",
        "Period".bold(),
        "Return".bold(),
        "Annualized".bold(),
        "₹1L became".bold()
    );
    println!("  {}", "─".repeat(54).dimmed());

    for row in &rows {
        let (ret, value, label) = (row.return_percent, row.growth_of_100000, row.label);
        let ret_str = if ret >= 0.0 {
            format!("+{:.2}%", ret).green().to_string()
        } else {
            format!("{:.2}%", ret).red().to_string()
        };
        let ann_str = match row.annualized_percent {
            Some(a) if a >= 0.0 => format!("+{:.1}%", a).green().to_string(),
            Some(a) => format!("{:.1}%", a).red().to_string(),
            None => "—".dimmed().to_string(),
        };
        println!(
            "  {:<14} {:>10} {:>12} {:>14}",
            label,
            ret_str,
            ann_str,
            format!("{}{:.0}", csym, value)
        );
    }
    println!();
    Ok(())
}

// ── Heatmap ──

pub async fn cmd_forecast(symbol: &str, days: usize, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, period, "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    if closes.len() < 30 {
        bail!("Not enough price data — try a longer period with -p");
    }

    let n = closes.len();
    // Linear regression on last 90 days
    let window = 90.min(n);
    let recent = &closes[n - window..];
    let (slope, intercept) = linear_regression(recent);
    let vol = technical::annualized_volatility(recent).unwrap_or(0.3);
    let daily_vol = vol / (252.0_f64).sqrt();

    // Project forward
    let mut forecast = closes.clone();
    let mut upper = Vec::new();
    let mut lower = Vec::new();
    for d in 0..days {
        let projected = slope * (window + d) as f64 + intercept;
        let band = daily_vol * projected * ((d + 1) as f64).sqrt();
        forecast.push(projected);
        upper.push(projected + 1.96 * band);
        lower.push(projected - 1.96 * band);
    }

    let current = *closes.last().unwrap();
    let projected_end = *forecast.last().unwrap();
    let change_pct = technical::percent_change(current, projected_end);

    print_header(&format!("Forecast: {} — {} day projection", resolved, days));

    // Show chart with historical + forecast
    let chart_data = &forecast[forecast.len().saturating_sub(90 + days)..];
    let color = if projected_end >= current {
        "green"
    } else {
        "red"
    };
    for line in charts::line_chart(chart_data, 55, 10, color, "Price Trend + Forecast") {
        println!("{}", line);
    }
    println!(
        "  {}  {}",
        "Historical ←".dimmed(),
        format!("→ {} day forecast", days).cyan()
    );

    print_section("Projection");
    print_kv("Current Price", &format!("{}{:.2}", csym, current));
    print_kv(
        &format!("{}-Day Forecast", days),
        &format!("{}{:.2}", csym, projected_end),
    );
    let chg = if change_pct >= 0.0 {
        format!("+{:.2}%", change_pct).green().to_string()
    } else {
        format!("{:.2}%", change_pct).red().to_string()
    };
    print_kv("Expected Change", &chg);
    print_kv(
        "Daily Trend",
        &format!("{}{:.4}/day", if slope >= 0.0 { "+" } else { "" }, slope),
    );

    print_section("Confidence Bands (95%)");
    if let (Some(u), Some(l)) = (upper.last(), lower.last()) {
        print_kv(
            "Upper Band",
            &format!("{}{:.2}", csym, u).green().to_string(),
        );
        print_kv("Center", &format!("{}{:.2}", csym, projected_end));
        print_kv("Lower Band", &format!("{}{:.2}", csym, l).red().to_string());
        print_kv(
            "Band Width",
            &format!("{:.2}%", ((u - l) / projected_end) * 100.0),
        );
    }
    print_kv("Annualized Vol", &format!("{:.1}%", vol * 100.0));

    println!(
        "\n  {}",
        "Forecast based on linear trend extrapolation. NOT a prediction."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

pub(super) fn linear_regression(data: &[f64]) -> (f64, f64) {
    let n = data.len() as f64;
    let sum_x: f64 = (0..data.len()).map(|i| i as f64).sum();
    let sum_y: f64 = data.iter().sum();
    let sum_xy: f64 = data.iter().enumerate().map(|(i, &y)| i as f64 * y).sum();
    let sum_x2: f64 = (0..data.len()).map(|i| (i as f64).powi(2)).sum();
    let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x.powi(2));
    let intercept = (sum_y - slope * sum_x) / n;
    (slope, intercept)
}

// ── Multi-timeframe ──

pub async fn cmd_options(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().with_context(|| {
        format!(
            "No quote for {}{}",
            resolved,
            market::not_found_hint(market)
        )
    })?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let price = q
        .regular_market_price
        .with_context(|| format!("No live price for {}", resolved))?;

    print_header(&format!("Options Overview: {}", resolved));
    print_kv("Spot Price", &format_price(price, cur));

    // Use beta and volatility as implied vol proxy
    if let Some(beta) = q.beta {
        print_kv("Beta", &format!("{:.2}", beta));
    }

    // Fetch 1mo data for realized vol
    let chart = client.get_chart(&resolved, period, "1d").await?;
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    if let Some(vol) = technical::annualized_volatility(&closes) {
        print_kv("Realized Vol (1M)", &format!("{:.1}%", vol * 100.0));
        // Estimate option prices using simplified Black-Scholes-ish approach
        let daily_move = vol / (252.0_f64).sqrt();
        print_section("Expected Moves");
        for days in [1, 5, 10, 30] {
            let move_pct = daily_move * (days as f64).sqrt() * price;
            print_kv(
                &format!("{}-day move", days),
                &format!(
                    "+/- {}{:.2} ({:.2}%)",
                    csym,
                    move_pct,
                    (move_pct / price) * 100.0
                ),
            );
        }

        print_section("Suggested Strike Prices");
        let otm_pct = [2.0, 5.0, 10.0];
        println!(
            "  {:<10} {:>14} {:>14}",
            "Distance".bold(),
            "Call Strike".bold(),
            "Put Strike".bold()
        );
        println!("  {}", "─".repeat(40).dimmed());
        for pct in otm_pct {
            let call = price * (1.0 + pct / 100.0);
            let put = price * (1.0 - pct / 100.0);
            println!(
                "  {:<10} {:>14} {:>14}",
                format!("{}% OTM", pct),
                format!("{}{:.2}", csym, call).green(),
                format!("{}{:.2}", csym, put).red()
            );
        }
    }
    println!(
        "\n  {}",
        format!(
            "Full chain with greeks: stockwise options chain {}{}",
            symbol,
            if market == Market::Us { " -m us" } else { "" }
        )
        .dimmed()
        .italic()
    );
    println!();
    Ok(())
}

// ── Fibonacci Levels (standalone) ──
