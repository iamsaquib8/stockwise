use crate::alerts::{AlertCondition, AlertStore};
use crate::api::YahooClient;
use crate::backtest;
use crate::charts;
use crate::display::*;
use crate::intraday;
use crate::insights;
use crate::longterm;
use crate::market::{self, Market};
use crate::portfolio::Portfolio;
use crate::technical;
use crate::watchlist::Watchlist;
use crate::wealth::WealthHistory;
use anyhow::{Context, Result};
use colored::Colorize;

fn resolve_symbols(symbols: &[String], market: Market) -> Vec<String> {
    symbols
        .iter()
        .map(|s| market::resolve_symbol(s, market))
        .collect()
}

pub async fn cmd_quote(symbols: &[String], market: Market) -> Result<()> {
    let resolved = resolve_symbols(symbols, market);
    let client = YahooClient::new().await?;
    let sym_refs: Vec<&str> = resolved.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    if quotes.is_empty() {
        println!("{}", "No results found. Check the symbol(s).".red());
        return Ok(());
    }

    for q in &quotes {
        let symbol = q.symbol.as_deref().unwrap_or("???");
        let name = q.long_name.as_deref().or(q.short_name.as_deref()).unwrap_or("Unknown");
        let price = q.regular_market_price.unwrap_or(0.0);
        let change = q.regular_market_change.unwrap_or(0.0);
        let change_pct = q.regular_market_change_percent.unwrap_or(0.0);
        let cur = q.currency.as_deref();

        print_header(&format!("{} — {}", symbol, name));

        println!(
            "  {}  {}",
            format_price(price, cur).bold(),
            format_change(change, change_pct)
        );

        println!();
        print_kv("Open", &format_price(q.regular_market_open.unwrap_or(0.0), cur));
        print_kv(
            "Day Range",
            &format!(
                "{} — {}",
                format_price(q.regular_market_day_low.unwrap_or(0.0), cur),
                format_price(q.regular_market_day_high.unwrap_or(0.0), cur)
            ),
        );
        print_kv(
            "52-Week Range",
            &format!(
                "{} — {}",
                format_price(q.fifty_two_week_low.unwrap_or(0.0), cur),
                format_price(q.fifty_two_week_high.unwrap_or(0.0), cur)
            ),
        );
        print_kv(
            "Volume",
            &format_volume(q.regular_market_volume.unwrap_or(0)),
        );
        print_kv(
            "Avg Volume (3M)",
            &format_volume(q.average_daily_volume_3_month.unwrap_or(0)),
        );
        print_kv(
            "Market Cap",
            &q.market_cap
                .map_or("N/A".to_string(), |v| format_large_number(v, cur)),
        );
        print_kv("P/E (TTM)", &format_optional_f64(q.trailing_pe, ""));
        print_kv("EPS (TTM)", &format_optional_f64(q.eps_trailing_twelve_months, ""));
        print_kv("50-Day MA", &format_optional_price(q.fifty_day_average, cur));
        print_kv("200-Day MA", &format_optional_price(q.two_hundred_day_average, cur));

        if let Some(div) = q.trailing_annual_dividend_yield {
            if div > 0.0 {
                print_kv("Dividend Yield", &format!("{:.2}%", div * 100.0));
            }
        }
    }
    println!();
    Ok(())
}

pub async fn cmd_analyze(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("No data found for this symbol")?;

    let name = q.long_name.as_deref().or(q.short_name.as_deref()).unwrap_or("Unknown");
    let price = q.regular_market_price.unwrap_or(0.0);
    let change = q.regular_market_change.unwrap_or(0.0);
    let change_pct = q.regular_market_change_percent.unwrap_or(0.0);
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);

    print_header(&format!(
        "Deep Analysis: {} — {}",
        resolved, name
    ));
    println!(
        "  {}  {}\n",
        format_price(price, cur).bold(),
        format_change(change, change_pct)
    );

    // Overview
    print_section("Company Overview");
    if let Some(sector) = &q.sector {
        print_kv("Sector", sector);
    }
    if let Some(industry) = &q.industry {
        print_kv("Industry", industry);
    }
    print_kv(
        "Market Cap",
        &q.market_cap
            .map_or("N/A".to_string(), |v| format_large_number(v, cur)),
    );
    if let Some(exchange) = &q.exchange {
        print_kv("Exchange", exchange);
    }

    // Valuation
    print_section("Valuation Metrics");
    print_kv("P/E (Trailing)", &format_optional_f64(q.trailing_pe, "x"));
    print_kv("P/E (Forward)", &format_optional_f64(q.forward_pe, "x"));
    print_kv("Price/Book", &format_optional_f64(q.price_to_book, "x"));
    print_kv("EV/Revenue", &format_optional_f64(q.enterprise_to_revenue, "x"));
    print_kv("EV/EBITDA", &format_optional_f64(q.enterprise_to_ebitda, "x"));

    // Profitability
    print_section("Profitability & Growth");
    print_kv("Profit Margin", &format_optional_pct(q.profit_margins));
    print_kv("Return on Equity", &format_optional_pct(q.return_on_equity));
    print_kv("Revenue Growth", &format_optional_pct(q.revenue_growth));
    print_kv("Earnings Growth (Q)", &format_optional_pct(q.earnings_quarterly_growth));
    print_kv("EPS (TTM)", &format_optional_f64(q.eps_trailing_twelve_months, ""));
    print_kv("EPS (Forward)", &format_optional_f64(q.eps_forward, ""));
    print_kv("Revenue/Share", &format_optional_f64(q.revenue_per_share, ""));

    // Financial health
    print_section("Financial Health");
    print_kv("Debt/Equity", &format_optional_f64(q.debt_to_equity, ""));
    print_kv("Current Ratio", &format_optional_f64(q.current_ratio, "x"));
    print_kv("Book Value", &format_optional_f64(q.book_value, ""));

    // Analyst Ratings
    print_section("Analyst Ratings");
    if let Some(rec) = q.recommendation_mean {
        print_kv("Consensus", &sentiment_label(rec));
        print_kv(
            "Rating Score",
            &format!("{:.1}/5  {}", rec, rating_bar(5.0 - rec, 4.0)),
        );
    }
    if let Some(target) = q.target_mean_price {
        let upside = q.regular_market_price.map(|p| ((target - p) / p) * 100.0);
        print_kv(
            "Price Target",
            &format!(
                "{}{:.2} ({})",
                csym,
                target,
                upside.map_or("N/A".to_string(), |u| if u >= 0.0 {
                    format!("+{:.1}%", u).green().to_string()
                } else {
                    format!("{:.1}%", u).red().to_string()
                })
            ),
        );
    }
    if let Some(n) = q.number_of_analyst_opinions {
        print_kv("# Analysts", &n.to_string());
    }

    // Risk
    print_section("Risk Profile");
    print_kv("Beta", &format_optional_f64(q.beta, ""));
    if let Some(beta) = q.beta {
        let risk = if beta > 1.5 {
            "High".red().to_string()
        } else if beta > 1.0 {
            "Moderate".yellow().to_string()
        } else {
            "Low".green().to_string()
        };
        print_kv("Volatility", &risk);
    }

    // Insights
    print_section("AI Insights");
    for insight in insights::generate_insights(q) {
        println!("  {} {}", "→".cyan(), insight);
    }

    // Verdict
    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!("{}", insights::overall_verdict(q));
    println!("{}", "─".repeat(60).dimmed());

    println!(
        "\n  {}",
        "Disclaimer: This is not financial advice. Do your own research."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

pub async fn cmd_technical(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    // Fetch 1 year of daily data for technical analysis
    let chart = client.get_chart(&resolved, "1y", "1d").await?;
    let cur = chart
        .meta
        .as_ref()
        .and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    let highs: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.high.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    let lows: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.low.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    let volumes: Vec<u64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.volume.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    if closes.len() < 30 {
        println!("{}", "Not enough data for technical analysis".red());
        return Ok(());
    }

    let current = *closes.last().unwrap();
    print_header(&format!("Technical Analysis: {}", resolved));
    println!(
        "  Current Price: {}\n",
        format_price(current, cur).bold()
    );

    // Moving Averages
    print_section("Moving Averages");
    let ma_periods = [10, 20, 50, 100, 200];
    for period in ma_periods {
        if let Some(ma) = technical::sma(&closes, period) {
            let signal = if current > ma { "▲ Above" } else { "▼ Below" };
            let colored_signal = if current > ma {
                signal.green().to_string()
            } else {
                signal.red().to_string()
            };
            print_kv(
                &format!("SMA {}", period),
                &format!("{}{:.2}  {}", csym, ma, colored_signal),
            );
        }
    }

    // EMA
    println!();
    if let Some(ema12) = technical::ema(&closes, 12) {
        print_kv("EMA 12", &format!("{}{:.2}", csym, ema12));
    }
    if let Some(ema26) = technical::ema(&closes, 26) {
        print_kv("EMA 26", &format!("{}{:.2}", csym, ema26));
    }

    // RSI
    print_section("Momentum Indicators");
    if let Some(rsi) = technical::rsi(&closes, 14) {
        let rsi_color = if rsi >= 70.0 {
            format!("{:.2}", rsi).red().to_string()
        } else if rsi <= 30.0 {
            format!("{:.2}", rsi).green().to_string()
        } else {
            format!("{:.2}", rsi).to_string()
        };
        print_kv("RSI (14)", &rsi_color);
        println!("{}", charts::rsi_gauge(rsi));
        print_kv("Signal", technical::rsi_signal(rsi));
    }

    // MACD
    if let Some((macd_line, signal, histogram)) = technical::macd(&closes) {
        println!();
        print_kv("MACD Line", &format!("{:.4}", macd_line));
        print_kv("Signal Line", &format!("{:.4}", signal));
        let hist_str = if histogram >= 0.0 {
            format!("{:.4}", histogram).green().to_string()
        } else {
            format!("{:.4}", histogram).red().to_string()
        };
        print_kv("Histogram", &hist_str);
        print_kv("Signal", technical::macd_signal(histogram));
    }

    // Bollinger Bands
    print_section("Volatility");
    if let Some((upper, middle, lower)) = technical::bollinger_bands(&closes, 20) {
        print_kv("BB Upper", &format!("{}{:.2}", csym, upper));
        print_kv("BB Middle", &format!("{}{:.2}", csym, middle));
        print_kv("BB Lower", &format!("{}{:.2}", csym, lower));

        let bb_position = if current > upper {
            "Above upper band — overbought".red().to_string()
        } else if current < lower {
            "Below lower band — oversold".green().to_string()
        } else {
            "Within bands — normal range".to_string()
        };
        print_kv("BB Position", &bb_position);
    }

    // ATR
    if let Some(atr) = technical::atr(&highs, &lows, &closes, 14) {
        print_kv(
            "ATR (14)",
            &format!("{}{:.2} ({:.2}%)", csym, atr, (atr / current) * 100.0),
        );
    }

    // VWAP
    if let Some(vwap) = technical::vwap(&highs, &lows, &closes, &volumes) {
        print_kv("VWAP", &format!("{}{:.2}", csym, vwap));
    }

    // Price chart (last 60 days) — braille line chart
    print_section("Price Chart (60 days)");
    let recent: Vec<f64> = if closes.len() > 60 {
        closes[closes.len() - 60..].to_vec()
    } else {
        closes.clone()
    };
    let color = if recent.last() >= recent.first() { "green" } else { "red" };
    for line in charts::line_chart(&recent, 50, 8, color, "") {
        println!("{}", line);
    }

    // SMA overlay chart if enough data
    if let Some(_) = technical::sma(&closes, 20) {
        let sma20_vals: Vec<f64> = (0..recent.len())
            .map(|i| {
                let end = closes.len() - recent.len() + i + 1;
                technical::sma(&closes[..end], 20).unwrap_or(recent[i])
            })
            .collect();
        println!();
        for line in charts::dual_line_chart(&recent, &sma20_vals, 50, 6, "Price vs SMA-20", "Price", "SMA-20") {
            println!("{}", line);
        }
    }

    let min = recent.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = recent.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    println!(
        "  {} {}{:.2}  {} {}{:.2}",
        "Low:".dimmed(),
        csym,
        min,
        "High:".dimmed(),
        csym,
        max
    );

    // Overall technical summary
    print_section("Technical Summary");
    let mut bullish = 0i32;
    let mut bearish = 0i32;

    if let Some(ma50) = technical::sma(&closes, 50) {
        if current > ma50 {
            bullish += 1;
        } else {
            bearish += 1;
        }
    }
    if let Some(ma200) = technical::sma(&closes, 200) {
        if current > ma200 {
            bullish += 1;
        } else {
            bearish += 1;
        }
    }
    if let Some(rsi) = technical::rsi(&closes, 14) {
        if rsi < 30.0 {
            bullish += 1;
        } else if rsi > 70.0 {
            bearish += 1;
        }
    }
    if let Some((_, _, hist)) = technical::macd(&closes) {
        if hist > 0.0 {
            bullish += 1;
        } else {
            bearish += 1;
        }
    }

    let total = bullish + bearish;
    if total > 0 {
        let verdict = if bullish > bearish {
            format!("BULLISH ({}/{})", bullish, total)
                .green()
                .bold()
                .to_string()
        } else if bearish > bullish {
            format!("BEARISH ({}/{})", bearish, total)
                .red()
                .bold()
                .to_string()
        } else {
            format!("NEUTRAL ({}/{})", bullish, total)
                .yellow()
                .bold()
                .to_string()
        };
        println!("  Technical Bias: {}", verdict);
    }

    println!(
        "\n  {}",
        "Past performance does not indicate future results."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

pub async fn cmd_compare(symbols: &[String], market: Market) -> Result<()> {
    let resolved = resolve_symbols(symbols, market);
    let client = YahooClient::new().await?;
    let sym_refs: Vec<&str> = resolved.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    if quotes.is_empty() {
        println!("{}", "No results found.".red());
        return Ok(());
    }

    print_header("Stock Comparison");

    // Header row
    print!("  {:<22}", "Metric".bold());
    for q in &quotes {
        print!(
            " {:>14}",
            q.symbol.as_deref().unwrap_or("???").bold().cyan()
        );
    }
    println!();
    println!("  {}", "─".repeat(22 + quotes.len() * 15).dimmed());

    // Rows
    let rows: Vec<(&str, Box<dyn Fn(&crate::api::Quote) -> String>)> = vec![
        (
            "Price",
            Box::new(|q: &crate::api::Quote| {
                q.regular_market_price
                    .map_or("N/A".into(), |v| format_price(v, q.currency.as_deref()))
            }),
        ),
        (
            "Change %",
            Box::new(|q| {
                q.regular_market_change_percent.map_or("N/A".into(), |v| {
                    let s = format!("{:+.2}%", v);
                    if v >= 0.0 {
                        s.green().to_string()
                    } else {
                        s.red().to_string()
                    }
                })
            }),
        ),
        (
            "Market Cap",
            Box::new(|q| {
                q.market_cap
                    .map_or("N/A".into(), |v| format_large_number(v, q.currency.as_deref()))
            }),
        ),
        (
            "P/E (TTM)",
            Box::new(|q| q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v))),
        ),
        (
            "P/E (Fwd)",
            Box::new(|q| q.forward_pe.map_or("N/A".into(), |v| format!("{:.1}", v))),
        ),
        (
            "EPS (TTM)",
            Box::new(|q| {
                let csym = market::currency_symbol(q.currency.as_deref());
                q.eps_trailing_twelve_months
                    .map_or("N/A".into(), |v| format!("{}{:.2}", csym, v))
            }),
        ),
        (
            "P/B",
            Box::new(|q| q.price_to_book.map_or("N/A".into(), |v| format!("{:.2}", v))),
        ),
        (
            "Div Yield",
            Box::new(|q| {
                q.trailing_annual_dividend_yield
                    .map_or("N/A".into(), |v| format!("{:.2}%", v * 100.0))
            }),
        ),
        (
            "Profit Margin",
            Box::new(|q| {
                q.profit_margins
                    .map_or("N/A".into(), |v| format!("{:.1}%", v * 100.0))
            }),
        ),
        (
            "Revenue Growth",
            Box::new(|q| {
                q.revenue_growth
                    .map_or("N/A".into(), |v| format!("{:.1}%", v * 100.0))
            }),
        ),
        (
            "Debt/Equity",
            Box::new(|q| q.debt_to_equity.map_or("N/A".into(), |v| format!("{:.0}", v))),
        ),
        (
            "Beta",
            Box::new(|q| q.beta.map_or("N/A".into(), |v| format!("{:.2}", v))),
        ),
        (
            "52W High",
            Box::new(|q| {
                q.fifty_two_week_high
                    .map_or("N/A".into(), |v| format_price(v, q.currency.as_deref()))
            }),
        ),
        (
            "52W Low",
            Box::new(|q| {
                q.fifty_two_week_low
                    .map_or("N/A".into(), |v| format_price(v, q.currency.as_deref()))
            }),
        ),
        (
            "50-Day MA",
            Box::new(|q| {
                q.fifty_day_average
                    .map_or("N/A".into(), |v| format_price(v, q.currency.as_deref()))
            }),
        ),
        (
            "Analyst Rating",
            Box::new(|q| q.recommendation_key.clone().unwrap_or("N/A".into())),
        ),
    ];

    for (label, func) in &rows {
        print!("  {:<22}", label.dimmed());
        for q in &quotes {
            print!(" {:>14}", func(q));
        }
        println!();
    }

    println!();
    Ok(())
}

pub async fn cmd_history(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    let interval = match period {
        "1d" | "5d" => "5m",
        "1mo" => "1d",
        "3mo" | "6mo" => "1d",
        "1y" | "2y" => "1wk",
        "5y" | "10y" | "max" => "1mo",
        _ => "1d",
    };

    let chart = client.get_chart(&resolved, period, interval).await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let timestamps = chart.timestamp.unwrap_or_default();
    let closes: Vec<Option<f64>> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.clone())
        .unwrap_or_default();
    let volumes: Vec<Option<u64>> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.volume.clone())
        .unwrap_or_default();

    if timestamps.is_empty() {
        println!("{}", "No historical data available.".red());
        return Ok(());
    }

    print_header(&format!("Price History: {} ({})", resolved, period));

    // Price chart
    let valid_closes: Vec<f64> = closes.iter().filter_map(|v| *v).collect();
    let valid_volumes: Vec<u64> = volumes.iter().filter_map(|v| *v).collect();
    if !valid_closes.is_empty() {
        let color = if valid_closes.last() >= valid_closes.first() { "green" } else { "red" };
        println!();
        for line in charts::line_chart(&valid_closes, 55, 10, color, "Price") {
            println!("{}", line);
        }
        // Volume bars
        if !valid_volumes.is_empty() {
            println!("\n  {}", "Volume".bold());
            for line in charts::volume_bars(&valid_volumes, 55, 3) {
                println!("{}", line);
            }
        }
        println!();
        let first = valid_closes.first().unwrap();
        let last = valid_closes.last().unwrap();
        let total_change = ((last - first) / first) * 100.0;
        let change_str = if total_change >= 0.0 {
            format!("+{:.2}%", total_change).green().to_string()
        } else {
            format!("{:.2}%", total_change).red().to_string()
        };
        println!(
            "  {} {}{:.2}  {} {}{:.2}  {} {}",
            "Start:".dimmed(),
            csym,
            first,
            "End:".dimmed(),
            csym,
            last,
            "Change:".dimmed(),
            change_str
        );
        let min = valid_closes.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = valid_closes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        println!(
            "  {} {}{:.2}  {} {}{:.2}",
            "Low:".dimmed(),
            csym,
            min,
            "High:".dimmed(),
            csym,
            max
        );
    }

    // Show last N data points as a table
    let show_count = 20.min(timestamps.len());
    let start = timestamps.len() - show_count;

    println!();
    println!(
        "  {:<14} {:>12} {:>14}",
        "Date".bold(),
        "Close".bold(),
        "Volume".bold()
    );
    println!("  {}", "─".repeat(42).dimmed());

    for i in start..timestamps.len() {
        let date = chrono::DateTime::from_timestamp(timestamps[i], 0)
            .map(|d| d.format("%Y-%m-%d").to_string())
            .unwrap_or_else(|| "???".to_string());
        let close = closes
            .get(i)
            .and_then(|v| *v)
            .map(|v| format_price(v, cur))
            .unwrap_or_else(|| "N/A".to_string());
        let vol = volumes
            .get(i)
            .and_then(|v| *v)
            .map(|v| format_volume(v))
            .unwrap_or_else(|| "N/A".to_string());
        println!("  {:<14} {:>12} {:>14}", date.dimmed(), close, vol);
    }

    println!();
    Ok(())
}

pub async fn cmd_portfolio(
    action: &str,
    symbol: Option<&str>,
    shares: Option<f64>,
    cost: Option<f64>,
    market: Market,
) -> Result<()> {
    let mut portfolio = Portfolio::load()?;

    match action {
        "add" => {
            let sym = symbol.context("Symbol is required")?;
            let resolved = market::resolve_symbol(sym, market);
            let shares = shares.context("Number of shares is required")?;
            let cost = cost.context("Cost per share is required")?;
            portfolio.add(&resolved, shares, cost);
            portfolio.save()?;
            println!(
                "  {} Added {} shares of {} at {:.2}",
                "✓".green(),
                shares,
                resolved,
                cost
            );
        }
        "remove" => {
            let sym = symbol.context("Symbol is required")?;
            let resolved = market::resolve_symbol(sym, market);
            if portfolio.remove(&resolved) {
                portfolio.save()?;
                println!("  {} Removed {} from portfolio", "✓".green(), resolved);
            } else {
                println!("  {} {} not found in portfolio", "✗".red(), resolved);
            }
        }
        "show" | "list" => {
            if portfolio.holdings.is_empty() {
                println!(
                    "\n  {}",
                    "Portfolio is empty. Use 'stockwise portfolio add <SYMBOL> <SHARES> <COST>' to add holdings."
                        .dimmed()
                );
                return Ok(());
            }

            // Fetch current prices for all holdings
            let symbols: Vec<String> =
                portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
            let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
            let client = YahooClient::new().await?;
            let quotes = client.get_quote(&sym_refs).await?;

            print_header("Your Portfolio");
            println!(
                "  {:<12} {:>8} {:>10} {:>10} {:>12} {:>10} {:>10}",
                "Symbol".bold(),
                "Shares".bold(),
                "Avg Cost".bold(),
                "Price".bold(),
                "Value".bold(),
                "P&L".bold(),
                "P&L %".bold(),
            );
            println!("  {}", "─".repeat(76).dimmed());

            let mut total_cost = 0.0;
            let mut total_value = 0.0;

            for holding in &portfolio.holdings {
                let quote = quotes
                    .iter()
                    .find(|q| q.symbol.as_deref() == Some(&holding.symbol));
                let current_price = quote.and_then(|q| q.regular_market_price);
                let cur = quote.and_then(|q| q.currency.as_deref());
                let csym = market::currency_symbol(cur);

                let price = current_price.unwrap_or(0.0);
                let value = holding.shares * price;
                let cost_basis = holding.shares * holding.avg_cost;
                let pnl = value - cost_basis;
                let pnl_pct = if cost_basis > 0.0 {
                    (pnl / cost_basis) * 100.0
                } else {
                    0.0
                };

                total_cost += cost_basis;
                total_value += value;

                let pnl_str = if pnl >= 0.0 {
                    format!("+{}{:.2}", csym, pnl).green().to_string()
                } else {
                    format!("-{}{:.2}", csym, pnl.abs()).red().to_string()
                };
                let pnl_pct_str = if pnl_pct >= 0.0 {
                    format!("+{:.2}%", pnl_pct).green().to_string()
                } else {
                    format!("{:.2}%", pnl_pct).red().to_string()
                };

                println!(
                    "  {:<12} {:>8.2} {:>10} {:>10} {:>12} {:>10} {:>10}",
                    holding.symbol.cyan(),
                    holding.shares,
                    format_price(holding.avg_cost, cur),
                    format_price(price, cur),
                    format_price(value, cur),
                    pnl_str,
                    pnl_pct_str,
                );
            }

            let total_pnl = total_value - total_cost;
            let total_pnl_pct = if total_cost > 0.0 {
                (total_pnl / total_cost) * 100.0
            } else {
                0.0
            };

            // Use first holding's currency for totals (mixed-currency portfolios just show numbers)
            let total_cur = quotes.first().and_then(|q| q.currency.as_deref());
            let tcsym = market::currency_symbol(total_cur);

            println!("  {}", "─".repeat(76).dimmed());
            let total_pnl_str = if total_pnl >= 0.0 {
                format!("+{}{:.2}", tcsym, total_pnl)
                    .green()
                    .bold()
                    .to_string()
            } else {
                format!("-{}{:.2}", tcsym, total_pnl.abs())
                    .red()
                    .bold()
                    .to_string()
            };
            let total_pnl_pct_str = if total_pnl_pct >= 0.0 {
                format!("+{:.2}%", total_pnl_pct)
                    .green()
                    .bold()
                    .to_string()
            } else {
                format!("{:.2}%", total_pnl_pct).red().bold().to_string()
            };
            println!(
                "  {:<12} {:>8} {:>10} {:>10} {:>12} {:>10} {:>10}",
                "TOTAL".bold(),
                "",
                format_price(total_cost, total_cur),
                "",
                format_price(total_value, total_cur).bold(),
                total_pnl_str,
                total_pnl_pct_str,
            );

            // Allocation chart
            if portfolio.holdings.len() > 1 {
                print_section("Allocation");
                let alloc_items: Vec<(&str, f64)> = portfolio
                    .holdings
                    .iter()
                    .map(|h| {
                        let price = quotes
                            .iter()
                            .find(|q| q.symbol.as_deref() == Some(&h.symbol))
                            .and_then(|q| q.regular_market_price)
                            .unwrap_or(0.0);
                        (h.symbol.as_str(), h.shares * price)
                    })
                    .collect();
                for line in charts::allocation_chart(&alloc_items) {
                    println!("{}", line);
                }
            }
            println!();
        }
        _ => {
            println!("  Unknown portfolio action: {}", action);
            println!("  Available: add, remove, show");
        }
    }

    Ok(())
}

pub async fn cmd_markets(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;

    let sections: Vec<(&str, &[(&str, &str)])> = match market {
        Market::Us => vec![
            ("US Markets", market::US_INDICES),
            ("Indian Markets", market::INDIA_INDICES),
        ],
        Market::In => vec![
            ("Indian Markets", market::INDIA_INDICES),
            ("US Markets", market::US_INDICES),
        ],
    };

    for (title, indices) in &sections {
        let symbols: Vec<&str> = indices.iter().map(|(sym, _)| *sym).collect();
        let quotes = client.get_quote(&symbols).await?;

        print_header(title);

        for (i, q) in quotes.iter().enumerate() {
            let label = indices.get(i).map(|(_, name)| *name).unwrap_or("???");
            let price = q.regular_market_price.unwrap_or(0.0);
            let change = q.regular_market_change.unwrap_or(0.0);
            let change_pct = q.regular_market_change_percent.unwrap_or(0.0);
            let cur = q.currency.as_deref();

            println!(
                "  {:<16} {}  {}",
                label.bold(),
                format_price(price, cur).bold(),
                format_change(change, change_pct)
            );
        }
    }

    println!();
    Ok(())
}

// ══════════════════════════════════════════════════════════
// NEW FEATURES
// ══════════════════════════════════════════════════════════

// ── 1. Watchlist ──

pub async fn cmd_watchlist(action: &str, symbols: &[String], market: Market) -> Result<()> {
    let mut wl = Watchlist::load()?;

    match action {
        "add" => {
            for sym in symbols {
                let resolved = market::resolve_symbol(sym, market);
                if wl.add(&resolved) {
                    println!("  {} Added {} to watchlist", "✓".green(), resolved.cyan());
                } else {
                    println!("  {} {} already in watchlist", "·".dimmed(), resolved);
                }
            }
            wl.save()?;
        }
        "remove" | "rm" => {
            for sym in symbols {
                let resolved = market::resolve_symbol(sym, market);
                if wl.remove(&resolved) {
                    println!("  {} Removed {}", "✓".green(), resolved);
                } else {
                    println!("  {} {} not found", "✗".red(), resolved);
                }
            }
            wl.save()?;
        }
        "show" | "list" | "" => {
            if wl.symbols.is_empty() {
                println!(
                    "\n  {}",
                    "Watchlist is empty. Use 'stockwise watch add <SYMBOL>' to add."
                        .dimmed()
                );
                return Ok(());
            }

            let client = YahooClient::new().await?;
            let sym_refs: Vec<&str> = wl.symbols.iter().map(|s| s.as_str()).collect();
            let quotes = client.get_quote(&sym_refs).await?;

            print_header("Watchlist");
            println!(
                "  {:<12} {:>10} {:>12} {:>10} {:>14}",
                "Symbol".bold(), "Price".bold(), "Change".bold(),
                "Change %".bold(), "Volume".bold(),
            );
            println!("  {}", "─".repeat(62).dimmed());

            for q in &quotes {
                let sym = q.symbol.as_deref().unwrap_or("???");
                let cur = q.currency.as_deref();
                let price = q.regular_market_price.unwrap_or(0.0);
                let change = q.regular_market_change.unwrap_or(0.0);
                let pct = q.regular_market_change_percent.unwrap_or(0.0);
                let vol = q.regular_market_volume.unwrap_or(0);
                let change_str = if change >= 0.0 {
                    format!("+{:.2}", change).green().to_string()
                } else {
                    format!("{:.2}", change).red().to_string()
                };
                let pct_str = if pct >= 0.0 {
                    format!("+{:.2}%", pct).green().to_string()
                } else {
                    format!("{:.2}%", pct).red().to_string()
                };
                println!(
                    "  {:<12} {:>10} {:>12} {:>10} {:>14}",
                    sym.cyan(), format_price(price, cur), change_str, pct_str, format_volume(vol),
                );
            }
            println!();
        }
        _ => {
            println!("  Unknown action: {}. Available: add, remove, show", action);
        }
    }
    Ok(())
}

// ── 2. Movers ──

pub async fn cmd_movers(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let symbols = match market {
        Market::Us => market::US_POPULAR,
        Market::In => market::INDIA_POPULAR,
    };
    let sym_refs: Vec<&str> = symbols.to_vec();
    let mut quotes = client.get_quote(&sym_refs).await?;
    quotes.sort_by(|a, b| {
        b.regular_market_change_percent
            .unwrap_or(0.0)
            .partial_cmp(&a.regular_market_change_percent.unwrap_or(0.0))
            .unwrap()
    });

    let market_name = match market { Market::Us => "US", Market::In => "India" };

    print_header(&format!("Top Gainers — {}", market_name));
    println!("  {:<14} {:>10} {:>10} {:>14}", "Symbol".bold(), "Price".bold(), "Change %".bold(), "Volume".bold());
    println!("  {}", "─".repeat(52).dimmed());
    for q in quotes.iter().take(10) {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        println!("  {:<14} {:>10} {:>10} {:>14}", sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            format!("{:+.2}%", pct).green(), format_volume(q.regular_market_volume.unwrap_or(0)));
    }

    print_header(&format!("Top Losers — {}", market_name));
    println!("  {:<14} {:>10} {:>10} {:>14}", "Symbol".bold(), "Price".bold(), "Change %".bold(), "Volume".bold());
    println!("  {}", "─".repeat(52).dimmed());
    for q in quotes.iter().rev().take(10) {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        println!("  {:<14} {:>10} {:>10} {:>14}", sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            format!("{:+.2}%", pct).red(), format_volume(q.regular_market_volume.unwrap_or(0)));
    }
    println!();
    Ok(())
}

// ── 3. Sectors ──

pub async fn cmd_sectors(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let (sectors, title) = match market {
        Market::Us => (market::US_SECTOR_ETFS, "US Sector Performance"),
        Market::In => (market::INDIA_SECTOR_INDICES, "India Sector Performance"),
    };
    let symbols: Vec<&str> = sectors.iter().map(|(s, _)| *s).collect();
    let mut quotes = client.get_quote(&symbols).await?;
    quotes.sort_by(|a, b| {
        b.regular_market_change_percent.unwrap_or(0.0)
            .partial_cmp(&a.regular_market_change_percent.unwrap_or(0.0)).unwrap()
    });

    print_header(title);
    println!("  {:<18} {:>10} {:>10} {:>20}", "Sector".bold(), "Price".bold(), "Change %".bold(), "".bold());
    println!("  {}", "─".repeat(60).dimmed());

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let name = sectors.iter().find(|(s, _)| *s == sym).map(|(_, n)| *n).unwrap_or(sym);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let bar_len = (pct.abs() * 3.0).min(20.0) as usize;
        let bar = if pct >= 0.0 { "█".repeat(bar_len).green().to_string() } else { "█".repeat(bar_len).red().to_string() };
        let pct_str = if pct >= 0.0 { format!("{:+.2}%", pct).green().to_string() } else { format!("{:+.2}%", pct).red().to_string() };
        println!("  {:<18} {:>10} {:>10} {}", name.bold(), format_price(q.regular_market_price.unwrap_or(0.0), cur), pct_str, bar);
    }
    println!();
    Ok(())
}

// ── 4. News ──

pub async fn cmd_news(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    let search_url = format!(
        "https://query2.finance.yahoo.com/v1/finance/search?q={}&quotesCount=0&newsCount=10",
        resolved
    );
    let resp: serde_json::Value = client.raw_get(&search_url).await.context("Failed to fetch news")?;

    print_header(&format!("News: {}", resolved));

    if let Some(news) = resp.get("news").and_then(|n| n.as_array()) {
        if news.is_empty() {
            println!("  {}", "No recent news found.".dimmed());
        }
        for item in news.iter().take(10) {
            let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("No title");
            let publisher = item.get("publisher").and_then(|p| p.as_str()).unwrap_or("Unknown");
            let timestamp = item.get("providerPublishTime").and_then(|t| t.as_i64())
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
                .unwrap_or_default();
            println!();
            println!("  {} {}", "●".cyan(), title.bold());
            println!("    {} {} · {}", "└".dimmed(), publisher.dimmed(), timestamp.dimmed());
        }
    } else {
        println!("  {}", "No news data available.".dimmed());
    }
    println!();
    Ok(())
}

// ── 5. Risk Analysis ──

pub async fn cmd_risk(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "1y", "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let closes: Vec<f64> = chart.indicators.quote.first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    if closes.len() < 30 {
        println!("{}", "Not enough data for risk analysis".red());
        return Ok(());
    }

    let current = *closes.last().unwrap();
    let first = closes[0];
    let total_return = (current / first - 1.0) * 100.0;

    print_header(&format!("Risk Analysis: {}", resolved));
    println!("  Current: {}  |  1Y Return: {}\n",
        format_price(current, cur).bold(),
        if total_return >= 0.0 { format!("+{:.2}%", total_return).green().to_string() }
        else { format!("{:.2}%", total_return).red().to_string() }
    );

    print_section("Volatility Metrics");
    if let Some(vol) = technical::annualized_volatility(&closes) {
        print_kv("Annual Volatility", &format!("{:.2}%", vol * 100.0));
        let risk_level = if vol > 0.5 { "Very High".red().to_string() }
            else if vol > 0.3 { "High".red().to_string() }
            else if vol > 0.2 { "Moderate".yellow().to_string() }
            else { "Low".green().to_string() };
        print_kv("Risk Level", &risk_level);
    }

    print_section("Risk-Adjusted Returns");
    if let Some(sharpe) = technical::sharpe_ratio(&closes, 0.05) {
        let color = if sharpe > 1.0 { format!("{:.3}", sharpe).green().to_string() }
            else if sharpe > 0.0 { format!("{:.3}", sharpe).yellow().to_string() }
            else { format!("{:.3}", sharpe).red().to_string() };
        print_kv("Sharpe Ratio", &color);
        let interp = match sharpe {
            s if s > 2.0 => "Excellent risk-adjusted returns",
            s if s > 1.0 => "Good risk-adjusted returns",
            s if s > 0.0 => "Acceptable but below average",
            _ => "Poor — not compensating for risk",
        };
        print_kv("Interpretation", interp);
    }
    if let Some(sortino) = technical::sortino_ratio(&closes, 0.05) {
        print_kv("Sortino Ratio", &format!("{:.3}", sortino));
    }
    if let Some(calmar) = technical::calmar_ratio(&closes) {
        print_kv("Calmar Ratio", &format!("{:.3}", calmar));
    }

    print_section("Drawdown Analysis");
    if let Some((mdd, peak_idx, trough_idx)) = technical::max_drawdown(&closes) {
        print_kv("Max Drawdown", &format!("-{:.2}%", mdd * 100.0).red().to_string());
        print_kv("Peak → Trough", &format!("{}{:.2} → {}{:.2}", csym, closes[peak_idx], csym, closes[trough_idx]));
        print_kv("Duration", &format!("{} trading days", trough_idx as i64 - peak_idx as i64));
    }

    print_section("Value at Risk (VaR)");
    if let Some(var95) = technical::value_at_risk(&closes, 0.95) {
        print_kv("Daily VaR (95%)", &format!("{:.2}%", var95 * 100.0).red().to_string());
    }
    if let Some(var99) = technical::value_at_risk(&closes, 0.99) {
        print_kv("Daily VaR (99%)", &format!("{:.2}%", var99 * 100.0).red().to_string());
    }

    print_section("1Y Price Chart");
    let color = if *closes.last().unwrap() >= closes[0] { "green" } else { "red" };
    for line in charts::line_chart(&closes, 50, 8, color, "") {
        println!("{}", line);
    }

    println!("\n  {}", "Risk metrics based on 1Y daily data. Past performance ≠ future results.".dimmed().italic());
    println!();
    Ok(())
}

// ── 6. Correlation ──

pub async fn cmd_correlate(symbols: &[String], market: Market) -> Result<()> {
    if symbols.len() < 2 {
        println!("{}", "Need at least 2 symbols to correlate.".red());
        return Ok(());
    }
    let resolved = resolve_symbols(symbols, market);
    let client = YahooClient::new().await?;

    let mut all_closes: Vec<(String, Vec<f64>)> = Vec::new();
    for sym in &resolved {
        let chart = client.get_chart(sym, "1y", "1d").await?;
        let closes: Vec<f64> = chart.indicators.quote.first()
            .and_then(|q| q.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();
        all_closes.push((sym.clone(), closes));
    }

    print_header("Correlation Matrix (1Y daily returns)");
    print!("  {:<12}", "");
    for (sym, _) in &all_closes { print!(" {:>10}", sym.cyan()); }
    println!();
    println!("  {}", "─".repeat(12 + all_closes.len() * 11).dimmed());

    for (i, (sym_a, closes_a)) in all_closes.iter().enumerate() {
        print!("  {:<12}", sym_a.cyan());
        for (j, (_, closes_b)) in all_closes.iter().enumerate() {
            if i == j {
                print!(" {:>10}", "1.000".bold());
            } else {
                let corr = technical::correlation(closes_a, closes_b).unwrap_or(0.0);
                let colored = if corr > 0.7 { format!("{:.3}", corr).green().to_string() }
                    else if corr > 0.3 { format!("{:.3}", corr).yellow().to_string() }
                    else if corr > -0.3 { format!("{:.3}", corr).to_string() }
                    else { format!("{:.3}", corr).red().to_string() };
                print!(" {:>10}", colored);
            }
        }
        println!();
    }
    println!("\n  {} >0.7 strong  {} 0.3-0.7 moderate  {} <-0.3 inverse", "●".green(), "●".yellow(), "●".red());
    println!();
    Ok(())
}

// ── 7. Export ──

pub async fn cmd_export(what: &str, _market: Market) -> Result<()> {
    match what {
        "portfolio" => {
            let portfolio = Portfolio::load()?;
            if portfolio.holdings.is_empty() { println!("  {}", "Portfolio is empty.".dimmed()); return Ok(()); }
            let client = YahooClient::new().await?;
            let symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
            let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
            let quotes = client.get_quote(&sym_refs).await?;
            let path = "portfolio_export.csv";
            let mut csv = String::from("Symbol,Shares,Avg Cost,Current Price,Value,P&L,P&L %\n");
            for h in &portfolio.holdings {
                let price = quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol))
                    .and_then(|q| q.regular_market_price).unwrap_or(0.0);
                let value = h.shares * price;
                let cost = h.shares * h.avg_cost;
                let pnl = value - cost;
                let pnl_pct = if cost > 0.0 { (pnl / cost) * 100.0 } else { 0.0 };
                csv.push_str(&format!("{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}%\n", h.symbol, h.shares, h.avg_cost, price, value, pnl, pnl_pct));
            }
            std::fs::write(path, &csv)?;
            println!("  {} Exported portfolio to {}", "✓".green(), path.cyan());
        }
        "watchlist" => {
            let wl = Watchlist::load()?;
            if wl.symbols.is_empty() { println!("  {}", "Watchlist is empty.".dimmed()); return Ok(()); }
            let client = YahooClient::new().await?;
            let sym_refs: Vec<&str> = wl.symbols.iter().map(|s| s.as_str()).collect();
            let quotes = client.get_quote(&sym_refs).await?;
            let path = "watchlist_export.csv";
            let mut csv = String::from("Symbol,Price,Change,Change %,Volume,Market Cap\n");
            for q in &quotes {
                csv.push_str(&format!("{},{:.2},{:.2},{:.2}%,{},{:.0}\n",
                    q.symbol.as_deref().unwrap_or(""), q.regular_market_price.unwrap_or(0.0),
                    q.regular_market_change.unwrap_or(0.0), q.regular_market_change_percent.unwrap_or(0.0),
                    q.regular_market_volume.unwrap_or(0), q.market_cap.unwrap_or(0.0)));
            }
            std::fs::write(path, &csv)?;
            println!("  {} Exported watchlist to {}", "✓".green(), path.cyan());
        }
        _ => { println!("  Unknown: {}. Available: portfolio, watchlist", what); }
    }
    Ok(())
}

// ── 8. Alerts ──

pub async fn cmd_alert(action: &str, symbol: Option<&str>, condition: Option<&str>, target: Option<f64>, market: Market) -> Result<()> {
    let mut store = AlertStore::load()?;
    match action {
        "add" => {
            let sym = symbol.context("Symbol is required")?;
            let resolved = market::resolve_symbol(sym, market);
            let cond_str = condition.context("Condition required (above/below)")?;
            let cond = match cond_str { "above" | ">" => AlertCondition::Above, "below" | "<" => AlertCondition::Below,
                _ => { println!("  Condition must be 'above' or 'below'"); return Ok(()); } };
            let tgt = target.context("Target price is required")?;
            store.add(&resolved, cond.clone(), tgt);
            store.save()?;
            println!("  {} Alert: {} {} {:.2}", "✓".green(), resolved.cyan(), cond, tgt);
        }
        "remove" | "rm" => {
            let idx_str = symbol.context("Alert index required (see 'alert list')")?;
            let idx: usize = idx_str.parse().context("Invalid index")?;
            if store.remove_by_index(idx) { store.save()?; println!("  {} Removed alert #{}", "✓".green(), idx); }
            else { println!("  {} Invalid index", "✗".red()); }
        }
        "check" => {
            if store.alerts.is_empty() { println!("  {}", "No alerts configured.".dimmed()); return Ok(()); }
            let a_syms: Vec<String> = store.alerts.iter().map(|a| a.symbol.clone()).collect();
            let mut unique: Vec<&str> = a_syms.iter().map(|s| s.as_str()).collect();
            unique.sort(); unique.dedup();
            let client = YahooClient::new().await?;
            let quotes = client.get_quote(&unique).await?;
            print_header("Alert Check");
            let mut triggered = 0;
            for alert in &store.alerts {
                let price = quotes.iter().find(|q| q.symbol.as_deref() == Some(alert.symbol.as_str()))
                    .and_then(|q| q.regular_market_price).unwrap_or(0.0);
                let cur = quotes.iter().find(|q| q.symbol.as_deref() == Some(alert.symbol.as_str()))
                    .and_then(|q| q.currency.as_deref());
                let is_hit = match alert.condition { AlertCondition::Above => price >= alert.target, AlertCondition::Below => price <= alert.target };
                let status = if is_hit { triggered += 1; "TRIGGERED".red().bold().to_string() } else { "waiting".dimmed().to_string() };
                println!("  {} {} {:.2} | Now: {} | {}", alert.symbol.cyan(), alert.condition, alert.target, format_price(price, cur), status);
            }
            if triggered == 0 { println!("\n  {}", "No alerts triggered.".dimmed()); }
            else { println!("\n  {} {} alert(s) triggered!", "⚠".yellow(), triggered); }
            println!();
        }
        "list" | "show" | "" => {
            if store.alerts.is_empty() { println!("\n  {}", "No alerts. Use 'stockwise alert add <SYMBOL> above/below <PRICE>'.".dimmed()); return Ok(()); }
            print_header("Price Alerts");
            for (i, a) in store.alerts.iter().enumerate() {
                println!("  #{} {} {} {:.2}  (set {})", i, a.symbol.cyan(), a.condition, a.target, a.created_at.dimmed());
            }
            println!();
        }
        _ => { println!("  Unknown action: {}. Available: add, remove, check, list", action); }
    }
    Ok(())
}

// ── 9. Screener ──

pub async fn cmd_screen(category: &str, market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let symbols = match market { Market::Us => market::US_POPULAR, Market::In => market::INDIA_POPULAR };
    let sym_refs: Vec<&str> = symbols.to_vec();
    let quotes = client.get_quote(&sym_refs).await?;

    let (title, mut filtered): (String, Vec<&crate::api::Quote>) = match category {
        "undervalued" => ("Undervalued Stocks (P/E < 15)".into(),
            quotes.iter().filter(|q| q.trailing_pe.is_some_and(|pe| pe > 0.0 && pe < 15.0)).collect()),
        "growth" => ("Growth Stocks (EPS Forward > EPS TTM)".into(),
            quotes.iter().filter(|q| matches!((q.eps_trailing_twelve_months, q.eps_forward), (Some(ttm), Some(fwd)) if fwd > ttm && ttm > 0.0)).collect()),
        "dividend" => ("Dividend Stocks (Yield > 2%)".into(),
            quotes.iter().filter(|q| q.trailing_annual_dividend_yield.is_some_and(|d| d > 0.02)).collect()),
        "momentum" => {
            let mut s: Vec<&crate::api::Quote> = quotes.iter().collect();
            s.sort_by(|a, b| b.regular_market_change_percent.unwrap_or(0.0).partial_cmp(&a.regular_market_change_percent.unwrap_or(0.0)).unwrap());
            s.truncate(15);
            ("Momentum Leaders (top daily gainers)".into(), s)
        }
        "bluechip" => ("Blue Chip (Market Cap > $100B / ₹5L Cr)".into(),
            quotes.iter().filter(|q| q.market_cap.is_some_and(|mc| mc > 100_000_000_000.0)).collect()),
        _ => { println!("  Unknown screen: {}. Available: {}", category, market::SCREENER_CATEGORIES.join(", ")); return Ok(()); }
    };

    if category != "momentum" {
        filtered.sort_by(|a, b| b.market_cap.unwrap_or(0.0).partial_cmp(&a.market_cap.unwrap_or(0.0)).unwrap());
    }

    print_header(&title);
    if filtered.is_empty() { println!("  {}", "No stocks match this screen.".dimmed()); return Ok(()); }
    println!("  {:<14} {:>10} {:>10} {:>10} {:>14}", "Symbol".bold(), "Price".bold(), "Change %".bold(), "P/E".bold(), "Market Cap".bold());
    println!("  {}", "─".repeat(62).dimmed());

    for q in &filtered {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 { format!("+{:.2}%", pct).green().to_string() } else { format!("{:.2}%", pct).red().to_string() };
        println!("  {:<14} {:>10} {:>10} {:>10} {:>14}", sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur), pct_str,
            q.trailing_pe.map_or("N/A".to_string(), |v| format!("{:.1}", v)),
            q.market_cap.map_or("N/A".to_string(), |v| format_large_number(v, cur)));
    }
    println!("\n  {} {} stocks matched", "→".cyan(), filtered.len());
    println!();
    Ok(())
}

// ── 10. Dashboard ──

pub async fn cmd_dashboard(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let (indices, market_title) = match market { Market::Us => (market::US_INDICES, "US"), Market::In => (market::INDIA_INDICES, "India") };
    let idx_syms: Vec<&str> = indices.iter().map(|(s, _)| *s).collect();
    let idx_quotes = client.get_quote(&idx_syms).await?;

    print_header(&format!("Dashboard — {}", market_title));
    print_section("Market Indices");
    for (i, q) in idx_quotes.iter().enumerate() {
        let label = indices.get(i).map(|(_, n)| *n).unwrap_or("???");
        let cur = q.currency.as_deref();
        println!("  {:<16} {}  {}", label, format_price(q.regular_market_price.unwrap_or(0.0), cur).bold(),
            format_change(q.regular_market_change.unwrap_or(0.0), q.regular_market_change_percent.unwrap_or(0.0)));
    }

    let portfolio = Portfolio::load()?;
    if !portfolio.holdings.is_empty() {
        let p_syms: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
        let p_refs: Vec<&str> = p_syms.iter().map(|s| s.as_str()).collect();
        let p_quotes = client.get_quote(&p_refs).await?;
        let mut total_cost = 0.0_f64; let mut total_value = 0.0_f64;
        for h in &portfolio.holdings {
            let price = p_quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol)).and_then(|q| q.regular_market_price).unwrap_or(0.0);
            total_cost += h.shares * h.avg_cost; total_value += h.shares * price;
        }
        let pnl = total_value - total_cost;
        let pnl_pct = if total_cost > 0.0 { (pnl / total_cost) * 100.0 } else { 0.0 };
        let cur = p_quotes.first().and_then(|q| q.currency.as_deref());
        let csym = market::currency_symbol(cur);
        print_section(&format!("Portfolio ({} holdings)", portfolio.holdings.len()));
        print_kv("Total Value", &format_price(total_value, cur));
        let pnl_str = if pnl >= 0.0 { format!("+{}{:.2} (+{:.2}%)", csym, pnl, pnl_pct).green().bold().to_string() }
            else { format!("-{}{:.2} ({:.2}%)", csym, pnl.abs(), pnl_pct).red().bold().to_string() };
        print_kv("P&L", &pnl_str);
    }

    let wl = Watchlist::load()?;
    if !wl.symbols.is_empty() {
        let w_refs: Vec<&str> = wl.symbols.iter().map(|s| s.as_str()).collect();
        let w_quotes = client.get_quote(&w_refs).await?;
        print_section(&format!("Watchlist ({} symbols)", wl.symbols.len()));
        for q in &w_quotes {
            let cur = q.currency.as_deref();
            let pct = q.regular_market_change_percent.unwrap_or(0.0);
            let pct_str = if pct >= 0.0 { format!("+{:.2}%", pct).green().to_string() } else { format!("{:.2}%", pct).red().to_string() };
            println!("  {:<12} {}  {}", q.symbol.as_deref().unwrap_or("???"), format_price(q.regular_market_price.unwrap_or(0.0), cur), pct_str);
        }
    }

    let store = AlertStore::load()?;
    if !store.alerts.is_empty() {
        let a_syms: Vec<String> = store.alerts.iter().map(|a| a.symbol.clone()).collect();
        let mut unique: Vec<&str> = a_syms.iter().map(|s| s.as_str()).collect(); unique.sort(); unique.dedup();
        let a_quotes = client.get_quote(&unique).await?;
        let mut triggered = Vec::new();
        for alert in &store.alerts {
            let price = a_quotes.iter().find(|q| q.symbol.as_deref() == Some(alert.symbol.as_str())).and_then(|q| q.regular_market_price).unwrap_or(0.0);
            let hit = match alert.condition { AlertCondition::Above => price >= alert.target, AlertCondition::Below => price <= alert.target };
            if hit { triggered.push(format!("{} {} {:.2}", alert.symbol, alert.condition, alert.target)); }
        }
        if !triggered.is_empty() {
            print_section("Triggered Alerts");
            for t in &triggered { println!("  {} {}", "⚠".yellow(), t.red()); }
        }
    }
    println!();
    Ok(())
}

// ── 11. Search ──

pub async fn cmd_search(query: &str) -> Result<()> {
    let client = YahooClient::new().await?;
    let results = client.search(query).await?;
    if results.is_empty() { println!("  {}", "No results found.".dimmed()); return Ok(()); }
    print_header(&format!("Search: {}", query));
    println!("  {:<14} {:<30} {:>10} {:>10}", "Symbol".bold(), "Name".bold(), "Type".bold(), "Exchange".bold());
    println!("  {}", "─".repeat(68).dimmed());
    for r in &results {
        println!("  {:<14} {:<30} {:>10} {:>10}", r.symbol.as_deref().unwrap_or("???").cyan(),
            r.long_name.as_deref().or(r.short_name.as_deref()).unwrap_or("—"),
            r.quote_type.as_deref().unwrap_or("—").dimmed(), r.exchange.as_deref().unwrap_or("—").dimmed());
    }
    println!();
    Ok(())
}

// ── Intraday Bot ──

pub async fn cmd_intraday(amount: f64, target_pct: f64) -> Result<()> {
    let target_profit = amount * (target_pct / 100.0);
    let csym = "₹";

    print_header("Intraday Trading Bot — Indian Market");
    println!(
        "  Capital: {}  |  Target: {}% ({}{:.2})\n",
        format!("{}{:.2}", csym, amount).bold().cyan(),
        target_pct,
        csym,
        target_profit
    );
    println!("  {} Scanning 30 NIFTY stocks...\n", "⟳".yellow());

    let client = YahooClient::new().await?;
    let signals = intraday::scan_intraday(&client).await?;
    let plans = intraday::generate_trade_plans(&signals, amount, target_pct);

    // Show top signals
    print_section("Market Scan (top signals)");
    println!(
        "  {:<14} {:>8} {:>8} {:>6} {:>6} {:>6} {:>7}",
        "Symbol".bold(), "Price".bold(), "Chg%".bold(),
        "RSI".bold(), "Vol.R".bold(), "BB%".bold(), "Score".bold(),
    );
    println!("  {}", "─".repeat(60).dimmed());

    for s in signals.iter().take(15) {
        let chg = if s.change_pct >= 0.0 {
            format!("{:+.2}%", s.change_pct).green().to_string()
        } else {
            format!("{:+.2}%", s.change_pct).red().to_string()
        };
        let rsi_str = s.rsi.map_or("—".to_string(), |r| {
            let s = format!("{:.0}", r);
            if r < 35.0 { s.green().to_string() }
            else if r > 70.0 { s.red().to_string() }
            else { s }
        });
        let bb_str = s.bb_position.map_or("—".to_string(), |p| format!("{:.0}%", p * 100.0));
        let score_str = if s.score >= 70.0 {
            format!("{:.0}", s.score).green().bold().to_string()
        } else if s.score >= 55.0 {
            format!("{:.0}", s.score).yellow().to_string()
        } else {
            format!("{:.0}", s.score).dimmed().to_string()
        };

        println!(
            "  {:<14} {:>8} {:>8} {:>6} {:>6} {:>6} {:>7}",
            s.symbol.cyan(),
            format!("{}{:.2}", csym, s.price),
            chg,
            rsi_str,
            format!("{:.1}x", s.volume_ratio),
            bb_str,
            score_str,
        );
    }

    // Show trade plans
    if plans.is_empty() {
        println!("\n  {}", "No high-confidence trades found right now. Market conditions may be unfavorable.".yellow());
        println!("  {}", "Try again closer to market open (9:15 AM IST) for better signals.".dimmed());
    } else {
        print_section(&format!(
            "Trade Suggestions (to make {}{:.2})",
            csym, target_profit
        ));

        let shown = plans.len().min(5);
        for (i, plan) in plans.iter().take(shown).enumerate() {
            let s = &plan.signal;
            let profit_pct = (plan.expected_profit / plan.capital_required) * 100.0;

            println!();
            println!(
                "  {}  {} — {} (Score: {})",
                format!("#{}", i + 1).bold().cyan(),
                s.symbol.bold().cyan(),
                s.name,
                format!("{:.0}", s.score).green(),
            );
            println!("  {}", "─".repeat(50).dimmed());

            // Signal reasoning
            let mut reasons = Vec::new();
            if let Some(r) = s.rsi {
                if r < 40.0 { reasons.push(format!("RSI oversold ({:.0})", r)); }
                else if r < 50.0 { reasons.push(format!("RSI neutral-low ({:.0})", r)); }
            }
            if s.above_vwap { reasons.push("Above VWAP".to_string()); }
            if s.volume_ratio > 1.2 { reasons.push(format!("High volume ({:.1}x avg)", s.volume_ratio)); }
            if let Some(p) = s.bb_position {
                if p < 0.3 { reasons.push("Near Bollinger lower band".to_string()); }
            }
            if let Some(h) = s.macd_histogram {
                if h > 0.0 { reasons.push("MACD bullish".to_string()); }
            }
            if !reasons.is_empty() {
                println!("  {} {}", "Why:".dimmed(), reasons.join(" · "));
            }

            println!();
            println!(
                "  {} {}    {} {}    {} {}    {} {}",
                "Direction:".dimmed(), s.direction.to_string().green().bold(),
                "Entry:".dimmed(), format!("{}{:.2}", csym, plan.entry).bold(),
                "Target:".dimmed(), format!("{}{:.2}", csym, plan.target).green().bold(),
                "Stop Loss:".dimmed(), format!("{}{:.2}", csym, plan.stop_loss).red(),
            );
            println!(
                "  {} {}    {} {}    {} {}    {} {}",
                "Qty:".dimmed(), format!("{} shares", plan.qty).bold(),
                "Capital:".dimmed(), format!("{}{:.2}", csym, plan.capital_required),
                "Exp. Profit:".dimmed(), format!("{}{:.2} ({:.2}%)", csym, plan.expected_profit, profit_pct).green(),
                "R:R".dimmed(), format!("1:{:.1}", plan.risk_reward).yellow(),
            );

            if plan.expected_profit >= target_profit {
                println!(
                    "  {} {}",
                    "✓".green().bold(),
                    format!("This trade alone can hit your {}{:.2} target!", csym, target_profit)
                        .green()
                );
            } else {
                let remaining = target_profit - plan.expected_profit;
                println!(
                    "  {} Covers {}{:.2} of target — {}{:.2} remaining",
                    "→".dimmed(),
                    csym, plan.expected_profit,
                    csym, remaining,
                );
            }
        }

        // Summary
        let total_possible: f64 = plans.iter().take(3).map(|p| p.expected_profit).sum();
        println!();
        println!("  {}", "─".repeat(60).dimmed());
        println!(
            "  {} Top 3 trades combined: {}{:.2} potential (target: {}{:.2})",
            if total_possible >= target_profit { "✓".green().bold() } else { "→".yellow().bold() },
            csym,
            total_possible,
            csym,
            target_profit
        );
    }

    println!();
    println!("  {}", "─".repeat(60).dimmed());
    println!("  {}", "IMPORTANT DISCLAIMER".bold().red());
    println!("  {}", "─".repeat(60).dimmed());
    println!("  {}", "This is NOT financial advice. Intraday trading involves".red());
    println!("  {}", "significant risk of loss. These are algorithmic suggestions".red());
    println!("  {}", "based on technical indicators, not guarantees. Always do".red());
    println!("  {}", "your own research and never risk money you can't afford".red());
    println!("  {}", "to lose. Past patterns do not predict future results.".red());
    println!();
    Ok(())
}

// ══════════════════════════════════════════════════════════
// WEALTH GENERATION FEATURES
// ══════════════════════════════════════════════════════════

// ── 1. Backtest ──

pub async fn cmd_backtest(symbol: &str, strategy: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, period, "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());

    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let volumes: Vec<u64> = chart.indicators.quote.first().and_then(|q| q.volume.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();

    if closes.len() < 50 {
        println!("{}", "Not enough data for backtesting. Use a longer period.".red());
        return Ok(());
    }

    let result = backtest::run_backtest(strategy, &closes, &highs, &lows, &volumes);
    let result = match result {
        Some(r) => r,
        None => {
            println!("  Unknown strategy: {}. Available: rsi, macd, sma, bb, vwap, mr", strategy);
            return Ok(());
        }
    };

    print_header(&format!("Backtest: {} — {} strategy ({})", resolved, strategy.to_uppercase(), period));
    println!();

    // Equity curve chart
    for line in charts::line_chart(&result.equity_curve, 55, 8, if result.total_return >= 0.0 { "green" } else { "red" }, "Equity Curve (starting at 100)") {
        println!("{}", line);
    }

    print_section("Performance");
    let ret_str = if result.total_return >= 0.0 { format!("+{:.2}%", result.total_return).green().to_string() } else { format!("{:.2}%", result.total_return).red().to_string() };
    let bh_str = if result.buy_hold_return >= 0.0 { format!("+{:.2}%", result.buy_hold_return).green().to_string() } else { format!("{:.2}%", result.buy_hold_return).red().to_string() };
    print_kv("Strategy Return", &ret_str);
    print_kv("Buy & Hold Return", &bh_str);
    let alpha = result.total_return - result.buy_hold_return;
    let alpha_str = if alpha >= 0.0 { format!("+{:.2}%", alpha).green().bold().to_string() } else { format!("{:.2}%", alpha).red().to_string() };
    print_kv("Alpha (vs B&H)", &alpha_str);

    print_section("Trade Statistics");
    print_kv("Total Trades", &result.trades.len().to_string());
    print_kv("Win Rate", &format!("{:.1}%", result.win_rate));
    print_kv("Avg Win", &format!("+{:.2}%", result.avg_win).green().to_string());
    print_kv("Avg Loss", &format!("{:.2}%", result.avg_loss).red().to_string());
    print_kv("Max Drawdown", &format!("-{:.2}%", result.max_drawdown).red().to_string());
    if let Some(s) = result.sharpe { print_kv("Sharpe Ratio", &format!("{:.3}", s)); }

    // Last 10 trades
    if !result.trades.is_empty() {
        print_section("Recent Trades");
        let csym = market::currency_symbol(cur);
        println!("  {:<6} {:>10} {:>10} {:>10}", "Trade".bold(), "Entry".bold(), "Exit".bold(), "P&L".bold());
        println!("  {}", "─".repeat(40).dimmed());
        for (i, t) in result.trades.iter().rev().take(10).enumerate() {
            let pnl = if t.pnl_pct >= 0.0 { format!("+{:.2}%", t.pnl_pct).green().to_string() } else { format!("{:.2}%", t.pnl_pct).red().to_string() };
            println!("  {:<6} {:>10} {:>10} {:>10}", format!("#{}", result.trades.len() - i), format!("{}{:.2}", csym, t.entry_price), format!("{}{:.2}", csym, t.exit_price), pnl);
        }
    }
    println!("\n  {}", "Backtests use historical data. Past results ≠ future performance.".dimmed().italic());
    println!();
    Ok(())
}

// ── 2. SIP/DCA Simulator ──

pub async fn cmd_sip(symbol: &str, amount: f64, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, period, "1mo").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let timestamps = chart.timestamp.unwrap_or_default();
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let n = timestamps.len().min(closes.len());

    if n < 2 { println!("{}", "Not enough data.".red()); return Ok(()); }

    let mut total_invested = 0.0;
    let mut total_units = 0.0;
    let mut value_history = Vec::new();
    let mut invested_history = Vec::new();

    for i in 0..n {
        let price = closes[i];
        let units = amount / price;
        total_units += units;
        total_invested += amount;
        let current_value = total_units * closes[i];
        value_history.push(current_value);
        invested_history.push(total_invested);
    }

    let final_value = total_units * closes[n - 1];
    let total_return = ((final_value / total_invested) - 1.0) * 100.0;
    let xirr_approx = if n > 1 { total_return / (n as f64 / 12.0) } else { 0.0 }; // annualized approx

    print_header(&format!("SIP Simulator: {} — {}{}/month for {}", resolved, csym, amount, period));

    // Chart: invested vs value
    for line in charts::dual_line_chart(&value_history, &invested_history, 55, 8, "Portfolio Value vs Amount Invested", "Value", "Invested") {
        println!("{}", line);
    }

    print_section("Results");
    print_kv("Months Invested", &n.to_string());
    print_kv("Monthly Amount", &format!("{}{:.2}", csym, amount));
    print_kv("Total Invested", &format!("{}{:.2}", csym, total_invested));
    print_kv("Current Value", &format!("{}{:.2}", csym, final_value).bold().to_string());
    let profit = final_value - total_invested;
    let profit_str = if profit >= 0.0 { format!("+{}{:.2}", csym, profit).green().bold().to_string() } else { format!("-{}{:.2}", csym, profit.abs()).red().bold().to_string() };
    print_kv("Profit/Loss", &profit_str);
    let ret_str = if total_return >= 0.0 { format!("+{:.2}%", total_return).green().to_string() } else { format!("{:.2}%", total_return).red().to_string() };
    print_kv("Total Return", &ret_str);
    print_kv("Annualized (approx)", &format!("{:.2}%", xirr_approx));
    print_kv("Total Units", &format!("{:.4}", total_units));
    print_kv("Avg Cost/Unit", &format!("{}{:.2}", csym, total_invested / total_units));
    print_kv("Current Price", &format!("{}{:.2}", csym, closes[n - 1]));
    println!();
    Ok(())
}

// ── 3. Forecast ──

pub async fn cmd_forecast(symbol: &str, days: usize, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "1y", "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    if closes.len() < 30 { println!("{}", "Not enough data.".red()); return Ok(()); }

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
    let change_pct = ((projected_end / current) - 1.0) * 100.0;

    print_header(&format!("Forecast: {} — {} day projection", resolved, days));

    // Show chart with historical + forecast
    let chart_data = &forecast[forecast.len().saturating_sub(90 + days)..];
    let color = if projected_end >= current { "green" } else { "red" };
    for line in charts::line_chart(chart_data, 55, 10, color, "Price Trend + Forecast") {
        println!("{}", line);
    }
    println!("  {}  {}",
        "Historical ←".dimmed(),
        format!("→ {} day forecast", days).cyan()
    );

    print_section("Projection");
    print_kv("Current Price", &format!("{}{:.2}", csym, current));
    print_kv(&format!("{}-Day Forecast", days), &format!("{}{:.2}", csym, projected_end));
    let chg = if change_pct >= 0.0 { format!("+{:.2}%", change_pct).green().to_string() } else { format!("{:.2}%", change_pct).red().to_string() };
    print_kv("Expected Change", &chg);
    print_kv("Daily Trend", &format!("{}{:.4}/day", if slope >= 0.0 { "+" } else { "" }, slope));

    print_section("Confidence Bands (95%)");
    if let (Some(u), Some(l)) = (upper.last(), lower.last()) {
        print_kv("Upper Band", &format!("{}{:.2}", csym, u).green().to_string());
        print_kv("Center", &format!("{}{:.2}", csym, projected_end));
        print_kv("Lower Band", &format!("{}{:.2}", csym, l).red().to_string());
        print_kv("Band Width", &format!("{:.2}%", ((u - l) / projected_end) * 100.0));
    }
    print_kv("Annualized Vol", &format!("{:.1}%", vol * 100.0));

    println!("\n  {}", "Forecast based on linear trend extrapolation. NOT a prediction.".dimmed().italic());
    println!();
    Ok(())
}

fn linear_regression(data: &[f64]) -> (f64, f64) {
    let n = data.len() as f64;
    let sum_x: f64 = (0..data.len()).map(|i| i as f64).sum();
    let sum_y: f64 = data.iter().sum();
    let sum_xy: f64 = data.iter().enumerate().map(|(i, &y)| i as f64 * y).sum();
    let sum_x2: f64 = (0..data.len()).map(|i| (i as f64).powi(2)).sum();
    let slope = (n * sum_xy - sum_x * sum_y) / (n * sum_x2 - sum_x.powi(2));
    let intercept = (sum_y - slope * sum_x) / n;
    (slope, intercept)
}

// ── 4. Multi-timeframe ──

pub async fn cmd_multitimeframe(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    print_header(&format!("Multi-Timeframe Analysis: {}", resolved));

    let timeframes = [("1mo", "1d", "1 Month"), ("3mo", "1d", "3 Months"), ("6mo", "1d", "6 Months"), ("1y", "1d", "1 Year"), ("2y", "1wk", "2 Years"), ("5y", "1mo", "5 Years")];

    println!("  {:<12} {:>8} {:>10} {:>6} {:>8} {:>8} {:>8}",
        "Timeframe".bold(), "Return".bold(), "Volatility".bold(), "RSI".bold(), "SMA".bold(), "MACD".bold(), "Signal".bold());
    println!("  {}", "─".repeat(68).dimmed());

    for (range, interval, label) in &timeframes {
        let chart = match client.get_chart(&resolved, range, interval).await { Ok(c) => c, Err(_) => continue };
        let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
        if closes.len() < 5 { continue; }

        let ret = (closes.last().unwrap() / closes[0] - 1.0) * 100.0;
        let vol = technical::annualized_volatility(&closes).map(|v| format!("{:.1}%", v * 100.0)).unwrap_or("—".into());
        let rsi = technical::rsi(&closes, 14).map(|r| format!("{:.0}", r)).unwrap_or("—".into());
        let sma_sig = technical::sma(&closes, 20).map(|s| if *closes.last().unwrap() > s { "▲".green().to_string() } else { "▼".red().to_string() }).unwrap_or("—".into());
        let macd_sig = technical::macd(&closes).map(|(_, _, h)| if h > 0.0 { "▲".green().to_string() } else { "▼".red().to_string() }).unwrap_or("—".into());

        let mut bullish = 0; let mut bearish = 0;
        if let Some(r) = technical::rsi(&closes, 14) { if r < 40.0 { bullish += 1; } else if r > 60.0 { bearish += 1; } }
        if let Some(s) = technical::sma(&closes, 20) { if *closes.last().unwrap() > s { bullish += 1; } else { bearish += 1; } }
        if let Some((_, _, h)) = technical::macd(&closes) { if h > 0.0 { bullish += 1; } else { bearish += 1; } }
        let signal = if bullish > bearish { "BUY".green().bold().to_string() } else if bearish > bullish { "SELL".red().bold().to_string() } else { "HOLD".yellow().to_string() };

        let ret_str = if ret >= 0.0 { format!("+{:.2}%", ret).green().to_string() } else { format!("{:.2}%", ret).red().to_string() };
        println!("  {:<12} {:>8} {:>10} {:>6} {:>8} {:>8} {:>8}", label, ret_str, vol, rsi, sma_sig, macd_sig, signal);
    }
    println!();
    Ok(())
}

// ── 5. Top Picks ──

pub async fn cmd_picks(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let symbols = match market { Market::Us => market::US_POPULAR, Market::In => market::INDIA_POPULAR };
    let sym_refs: Vec<&str> = symbols.to_vec();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header(&format!("Top Picks — {} Market", match market { Market::Us => "US", Market::In => "India" }));
    println!("  {} Scoring {} stocks across 8 factors...\n", "⟳".yellow(), quotes.len());

    let mut scored: Vec<(&crate::api::Quote, f64, Vec<&str>)> = Vec::new();
    for q in &quotes {
        let mut score = 50.0_f64;
        let mut reasons: Vec<&str> = Vec::new();
        // P/E
        if let Some(pe) = q.trailing_pe { if pe > 0.0 && pe < 20.0 { score += 10.0; reasons.push("Low P/E"); } else if pe > 40.0 { score -= 10.0; } }
        // Forward > Trailing (earnings growing)
        if let (Some(t), Some(f)) = (q.trailing_pe, q.forward_pe) { if f < t && t > 0.0 && f > 0.0 { score += 8.0; reasons.push("Earnings growth"); } }
        // Moving averages
        if let (Some(p), Some(ma50), Some(ma200)) = (q.regular_market_price, q.fifty_day_average, q.two_hundred_day_average) {
            if p > ma50 && ma50 > ma200 { score += 12.0; reasons.push("Strong uptrend"); }
            else if p < ma50 && ma50 < ma200 { score -= 10.0; }
        }
        // Analyst rating
        if let Some(rec) = q.recommendation_mean { if rec <= 2.0 { score += 10.0; reasons.push("Analyst buy"); } else if rec >= 4.0 { score -= 8.0; } }
        // Revenue growth
        if let Some(g) = q.revenue_growth { if g > 0.15 { score += 8.0; reasons.push("Revenue growth >15%"); } }
        // Profit margin
        if let Some(m) = q.profit_margins { if m > 0.2 { score += 5.0; reasons.push("High margins"); } }
        // Dividend
        if let Some(d) = q.trailing_annual_dividend_yield { if d > 0.02 { score += 3.0; reasons.push("Dividend >2%"); } }
        // Volume
        if let (Some(v), Some(avg)) = (q.regular_market_volume, q.average_daily_volume_3_month) {
            if avg > 0 && v as f64 / avg as f64 > 1.3 { score += 5.0; reasons.push("High volume"); }
        }
        score = score.clamp(0.0, 100.0);
        scored.push((q, score, reasons));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

    println!("  {:<14} {:>10} {:>8} {:>7} {}", "Symbol".bold(), "Price".bold(), "Chg%".bold(), "Score".bold(), "Reasons".bold());
    println!("  {}", "─".repeat(72).dimmed());

    for (q, score, reasons) in scored.iter().take(15) {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 { format!("+{:.2}%", pct).green().to_string() } else { format!("{:.2}%", pct).red().to_string() };
        let score_str = if *score >= 75.0 { format!("{:.0}", score).green().bold().to_string() } else if *score >= 60.0 { format!("{:.0}", score).yellow().to_string() } else { format!("{:.0}", score).dimmed().to_string() };
        let reasons_str = reasons.join(", ");
        println!("  {:<14} {:>10} {:>8} {:>7} {}", sym.cyan(), format_price(q.regular_market_price.unwrap_or(0.0), cur), pct_str, score_str, reasons_str.dimmed());
    }
    println!("\n  {}", "Scores are algorithmic. Always do your own research.".dimmed().italic());
    println!();
    Ok(())
}

// ── 6. Sentiment / Fear & Greed ──

pub async fn cmd_sentiment() -> Result<()> {
    let client = YahooClient::new().await?;
    // Fetch VIX, S&P500, and some breadth proxies
    let quotes = client.get_quote(&["^VIX", "^GSPC", "^IXIC", "^DJI", "SPY", "QQQ"]).await?;

    let vix = quotes.iter().find(|q| q.symbol.as_deref() == Some("^VIX")).and_then(|q| q.regular_market_price);
    let sp_chg = quotes.iter().find(|q| q.symbol.as_deref() == Some("^GSPC")).and_then(|q| q.regular_market_change_percent);

    let mut score = 50.0_f64;
    let mut factors = Vec::new();

    // VIX: <15 extreme greed, 15-20 greed, 20-25 neutral, 25-35 fear, >35 extreme fear
    if let Some(v) = vix {
        let vix_score = match v { x if x < 15.0 => 90.0, x if x < 20.0 => 70.0, x if x < 25.0 => 50.0, x if x < 35.0 => 30.0, _ => 10.0 };
        score = (score + vix_score) / 2.0;
        factors.push(format!("VIX: {:.1} → {}", v, if vix_score > 60.0 { "Greed" } else if vix_score < 40.0 { "Fear" } else { "Neutral" }));
    }

    // Market momentum
    if let Some(chg) = sp_chg {
        let mom_score = (50.0 + chg * 10.0).clamp(0.0, 100.0);
        score = (score + mom_score) / 2.0;
        factors.push(format!("S&P 500: {:+.2}% today", chg));
    }

    // Get broader market momentum (20-day)
    let chart = client.get_chart("^GSPC", "1mo", "1d").await;
    if let Ok(c) = chart {
        let closes: Vec<f64> = c.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
        if closes.len() > 5 {
            let monthly_ret = (closes.last().unwrap() / closes[0] - 1.0) * 100.0;
            let trend_score = (50.0 + monthly_ret * 5.0).clamp(0.0, 100.0);
            score = (score + trend_score) / 2.0;
            factors.push(format!("1M trend: {:+.2}%", monthly_ret));
        }
    }

    let (label, color) = match score {
        s if s >= 80.0 => ("EXTREME GREED", "green"),
        s if s >= 60.0 => ("GREED", "green"),
        s if s >= 40.0 => ("NEUTRAL", "yellow"),
        s if s >= 20.0 => ("FEAR", "red"),
        _ => ("EXTREME FEAR", "red"),
    };

    print_header("Market Sentiment — Fear & Greed");

    // Gauge
    let gauge_width = 50;
    let pos = (score / 100.0 * gauge_width as f64) as usize;
    let mut gauge = String::new();
    for i in 0..gauge_width {
        let ch = if i == pos { "▼".bold().to_string() } else if i < 10 { "█".red().to_string() } else if i < 20 { "█".red().dimmed().to_string() } else if i < 30 { "█".yellow().to_string() } else if i < 40 { "█".green().dimmed().to_string() } else { "█".green().to_string() };
        gauge.push_str(&ch);
    }
    println!("\n  {}  {}  {}", "Fear".red(), gauge, "Greed".green());
    let score_display = match color { "green" => format!("{:.0}", score).green().bold().to_string(), "red" => format!("{:.0}", score).red().bold().to_string(), _ => format!("{:.0}", score).yellow().bold().to_string() };
    println!("  {:>28} {} {}", "", score_display, label.bold());

    print_section("Contributing Factors");
    for f in &factors { println!("  {} {}", "→".cyan(), f); }
    println!();
    Ok(())
}

// ── 7. Tax Harvest ──

pub async fn cmd_harvest(market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() { println!("\n  {}", "Portfolio is empty.".dimmed()); return Ok(()); }

    let symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
    let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&sym_refs).await?;

    print_header("Tax-Loss Harvesting Opportunities");
    let mut found = false;

    println!("  {:<14} {:>10} {:>10} {:>12} {:>10}", "Symbol".bold(), "Avg Cost".bold(), "Price".bold(), "Loss".bold(), "Loss %".bold());
    println!("  {}", "─".repeat(60).dimmed());

    for h in &portfolio.holdings {
        let quote = quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol));
        let price = quote.and_then(|q| q.regular_market_price).unwrap_or(0.0);
        let cur = quote.and_then(|q| q.currency.as_deref());
        let csym = market::currency_symbol(cur);
        let pnl = (price - h.avg_cost) * h.shares;
        let pnl_pct = ((price / h.avg_cost) - 1.0) * 100.0;

        if pnl < 0.0 {
            found = true;
            println!("  {:<14} {:>10} {:>10} {:>12} {:>10}",
                h.symbol.cyan(),
                format!("{}{:.2}", csym, h.avg_cost),
                format!("{}{:.2}", csym, price),
                format!("-{}{:.2}", csym, pnl.abs()).red(),
                format!("{:.2}%", pnl_pct).red(),
            );
        }
    }

    if !found {
        println!("\n  {} No losing positions — nothing to harvest!", "✓".green());
    } else {
        println!("\n  {} Sell these positions to realize losses and offset capital gains.", "→".yellow());
        println!("  {}", "Consult a tax advisor. Wash sale rules may apply.".dimmed().italic());
    }
    println!();
    Ok(())
}

// ── 8. Wealth Tracker ──

pub async fn cmd_wealth(market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    let mut history = WealthHistory::load()?;

    if portfolio.holdings.is_empty() {
        println!("\n  {}", "Portfolio is empty. Add holdings first.".dimmed());
        return Ok(());
    }

    // Fetch current values and snapshot
    let symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
    let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&sym_refs).await?;

    let mut total_value = 0.0;
    let mut total_cost = 0.0;
    for h in &portfolio.holdings {
        let price = quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol)).and_then(|q| q.regular_market_price).unwrap_or(0.0);
        total_value += h.shares * price;
        total_cost += h.shares * h.avg_cost;
    }

    // Save today's snapshot
    history.add_snapshot(total_value, total_cost, portfolio.holdings.len());
    history.save()?;

    let cur = quotes.first().and_then(|q| q.currency.as_deref());
    let csym = market::currency_symbol(cur);

    print_header("Wealth Tracker");

    if history.snapshots.len() >= 2 {
        let values: Vec<f64> = history.snapshots.iter().map(|s| s.total_value).collect();
        let color = if *values.last().unwrap() >= values[0] { "green" } else { "red" };
        for line in charts::line_chart(&values, 55, 8, color, "Portfolio Value Over Time") {
            println!("{}", line);
        }
    }

    print_section("Current Snapshot");
    print_kv("Total Value", &format!("{}{:.2}", csym, total_value).bold().to_string());
    print_kv("Total Invested", &format!("{}{:.2}", csym, total_cost));
    let pnl = total_value - total_cost;
    let pnl_pct = if total_cost > 0.0 { (pnl / total_cost) * 100.0 } else { 0.0 };
    let pnl_str = if pnl >= 0.0 { format!("+{}{:.2} (+{:.2}%)", csym, pnl, pnl_pct).green().bold().to_string() } else { format!("-{}{:.2} ({:.2}%)", csym, pnl.abs(), pnl_pct).red().bold().to_string() };
    print_kv("Total P&L", &pnl_str);
    print_kv("Snapshots", &history.snapshots.len().to_string());

    if history.snapshots.len() >= 2 {
        let first = &history.snapshots[0];
        let growth = ((total_value / first.total_value) - 1.0) * 100.0;
        print_kv("Since First Track", &format!("{:+.2}% (from {})", growth, first.date));
    }

    println!("\n  {}", "Run 'stockwise wealth' daily to build your growth chart.".dimmed().italic());
    println!();
    Ok(())
}

// ── 9. Rebalance ──

pub async fn cmd_rebalance(targets: &[(String, f64)], market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() { println!("\n  {}", "Portfolio is empty.".dimmed()); return Ok(()); }

    let symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
    let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&sym_refs).await?;

    let mut total_value = 0.0;
    let mut holdings_value: Vec<(String, f64, f64)> = Vec::new(); // symbol, value, price
    for h in &portfolio.holdings {
        let price = quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol)).and_then(|q| q.regular_market_price).unwrap_or(0.0);
        let value = h.shares * price;
        total_value += value;
        holdings_value.push((h.symbol.clone(), value, price));
    }

    let cur = quotes.first().and_then(|q| q.currency.as_deref());
    let csym = market::currency_symbol(cur);

    print_header("Portfolio Rebalance");
    print_kv("Total Value", &format!("{}{:.2}", csym, total_value));

    // If no targets given, show equal-weight rebalance
    let target_pct = if targets.is_empty() {
        let equal = 100.0 / holdings_value.len() as f64;
        holdings_value.iter().map(|(s, _, _)| (s.clone(), equal)).collect::<Vec<_>>()
    } else {
        targets.to_vec()
    };

    println!();
    println!("  {:<14} {:>10} {:>8} {:>8} {:>12}", "Symbol".bold(), "Current".bold(), "Now%".bold(), "Target%".bold(), "Action".bold());
    println!("  {}", "─".repeat(56).dimmed());

    for (sym, value, price) in &holdings_value {
        let current_pct = (value / total_value) * 100.0;
        let tgt = target_pct.iter().find(|(s, _)| s == sym).map(|(_, p)| *p).unwrap_or(100.0 / holdings_value.len() as f64);
        let target_value = total_value * (tgt / 100.0);
        let diff = target_value - value;
        let diff_shares = if *price > 0.0 { diff / price } else { 0.0 };

        let action = if diff.abs() < 1.0 {
            "OK".green().to_string()
        } else if diff > 0.0 {
            format!("BUY {:.0} shares", diff_shares.ceil()).green().to_string()
        } else {
            format!("SELL {:.0} shares", diff_shares.abs().floor()).red().to_string()
        };

        println!("  {:<14} {:>10} {:>8} {:>8} {:>12}", sym.cyan(), format!("{}{:.2}", csym, value), format!("{:.1}%", current_pct), format!("{:.1}%", tgt), action);
    }
    println!();
    Ok(())
}

// ── 10. Earnings Calendar ──

pub async fn cmd_earnings(market: Market) -> Result<()> {
    let client = YahooClient::new().await?;

    // Check portfolio + watchlist stocks
    let portfolio = Portfolio::load()?;
    let wl = Watchlist::load()?;
    let mut all_symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
    for s in &wl.symbols { if !all_symbols.contains(s) { all_symbols.push(s.clone()); } }

    if all_symbols.is_empty() {
        println!("\n  {}", "Add stocks to portfolio or watchlist first.".dimmed());
        return Ok(());
    }

    let sym_refs: Vec<&str> = all_symbols.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header("Earnings Overview");
    println!("  {:<14} {:>10} {:>10} {:>10} {:>10}", "Symbol".bold(), "Price".bold(), "EPS TTM".bold(), "EPS Fwd".bold(), "P/E Fwd".bold());
    println!("  {}", "─".repeat(58).dimmed());

    let mut growing = 0;
    let mut declining = 0;
    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let eps_ttm = q.eps_trailing_twelve_months;
        let eps_fwd = q.eps_forward;
        let fwd_pe = q.forward_pe;

        let growth_indicator = match (eps_ttm, eps_fwd) {
            (Some(t), Some(f)) if f > t && t > 0.0 => { growing += 1; "▲".green().to_string() }
            (Some(t), Some(f)) if f < t && t > 0.0 => { declining += 1; "▼".red().to_string() }
            _ => "—".dimmed().to_string(),
        };

        println!("  {:<14} {:>10} {:>10} {:>10} {:>10} {}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            eps_ttm.map_or("—".to_string(), |v| format!("{:.2}", v)),
            eps_fwd.map_or("—".to_string(), |v| format!("{:.2}", v)),
            fwd_pe.map_or("—".to_string(), |v| format!("{:.1}", v)),
            growth_indicator,
        );
    }

    print_section("Summary");
    println!("  {} {} stocks with growing earnings", "▲".green(), growing);
    println!("  {} {} stocks with declining earnings", "▼".red(), declining);
    println!();
    Ok(())
}

// ── Long-Term Wealth Bot ──

pub async fn cmd_longterm(amount: Option<f64>, market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let symbols = match market { Market::Us => market::US_POPULAR, Market::In => market::INDIA_POPULAR };
    let sym_refs: Vec<&str> = symbols.to_vec();
    let quotes = client.get_quote(&sym_refs).await?;
    let csym = match market { Market::Us => "$", Market::In => "₹" };

    let monthly = amount.unwrap_or(10000.0);

    print_header(&format!("Long-Term Wealth Bot — {} Market", match market { Market::Us => "US", Market::In => "India" }));
    println!("  {} Analyzing {} stocks for long-term wealth building...\n", "⟳".yellow(), quotes.len());

    let mut scores: Vec<longterm::LongTermScore> = quotes.iter().map(|q| longterm::score_for_longterm(q)).collect();
    scores.sort_by(|a, b| b.total_score.partial_cmp(&a.total_score).unwrap());

    // Top picks table
    print_section("Top Long-Term Picks");
    println!("  {:<14} {:>6} {:>6} {:>6} {:>6} {:>6} {:>7} {:>8}",
        "Symbol".bold(), "Val".bold(), "Grow".bold(), "Qual".bold(), "Mom".bold(), "Div".bold(), "Total".bold(), "5Y Est".bold());
    println!("  {}", "─".repeat(72).dimmed());

    for s in scores.iter().take(10) {
        let score_color = if s.total_score >= 70.0 { format!("{:.0}", s.total_score).green().bold().to_string() }
            else if s.total_score >= 55.0 { format!("{:.0}", s.total_score).yellow().to_string() }
            else { format!("{:.0}", s.total_score).dimmed().to_string() };
        let ret5y = if s.projected_5y_return >= 50.0 { format!("+{:.0}%", s.projected_5y_return).green().to_string() }
            else { format!("+{:.0}%", s.projected_5y_return).to_string() };
        println!("  {:<14} {:>6.0} {:>6.0} {:>6.0} {:>6.0} {:>6.0} {:>7} {:>8}",
            s.symbol.cyan(), s.valuation_score, s.growth_score, s.quality_score, s.momentum_score, s.dividend_score, score_color, ret5y);
    }

    // Detailed top 3
    for (i, s) in scores.iter().take(3).enumerate() {
        println!();
        print_section(&format!("#{} {} — {}", i + 1, s.symbol, s.name));
        print_kv("Price", &format!("{}{:.2}", csym, s.price));
        print_kv("Score", &format!("{:.0}/100", s.total_score).bold().to_string());

        if !s.reasons.is_empty() {
            println!();
            println!("  {} {}", "Strengths:".green(), "");
            for r in &s.reasons { println!("    {} {}", "✓".green(), r); }
        }
        if !s.risk_flags.is_empty() {
            println!("  {} {}", "Risks:".red(), "");
            for r in &s.risk_flags { println!("    {} {}", "✗".red(), r); }
        }

        println!();
        print_kv("Est. 5Y Return", &format!("+{:.1}%", s.projected_5y_return));
        let investment_5y = monthly * 60.0;
        let est_value = investment_5y * (1.0 + s.projected_5y_return / 100.0);
        print_kv(&format!("If SIP {}{}/mo × 5Y", csym, monthly), &format!("{}{:.0} → {}{:.0}", csym, investment_5y, csym, est_value).green().to_string());
    }

    // Suggested portfolio allocation
    print_section("Suggested SIP Allocation");
    let top5: Vec<&longterm::LongTermScore> = scores.iter().take(5).collect();
    let total_score: f64 = top5.iter().map(|s| s.total_score).sum();
    println!("  Monthly budget: {}{:.0}\n", csym, monthly);
    for s in &top5 {
        let pct = s.total_score / total_score;
        let alloc = monthly * pct;
        let bar_len = (pct * 30.0) as usize;
        println!("  {:<14} {}{:>8.0} ({:>4.1}%) {}", s.symbol.cyan(), csym, alloc, pct * 100.0, "█".repeat(bar_len).green());
    }

    println!("\n  {}", "─".repeat(60).dimmed());
    println!("  {}", "Long-term investing involves risk. Diversify across asset classes.".dimmed().italic());
    println!("  {}", "This is algorithmic analysis, NOT financial advice.".dimmed().italic());
    println!();
    Ok(())
}

// ── Tax Calculator ──

pub async fn cmd_tax(country: &str, market: Market) -> Result<()> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() { println!("\n  {}", "Portfolio is empty.".dimmed()); return Ok(()); }

    let symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
    let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&sym_refs).await?;

    let mut total_stcg = 0.0_f64; // short-term capital gains
    let mut total_ltcg = 0.0_f64; // long-term capital gains
    let mut total_stcl = 0.0_f64;
    let mut total_ltcl = 0.0_f64;

    let (country_name, stcg_rate, ltcg_rate, ltcg_exempt, holding_months, currency_sym) = match country.to_lowercase().as_str() {
        "in" | "india" => ("India", 20.0, 12.5, 125000.0, 12, "₹"),
        "us" | "usa" => ("United States", 37.0, 20.0, 0.0, 12, "$"),
        "uk" => ("United Kingdom", 20.0, 20.0, 3000.0, 12, "£"),
        _ => { println!("  Supported countries: in (India), us (USA), uk (UK)"); return Ok(()); }
    };

    print_header(&format!("Tax Report — {} (FY 2025-26)", country_name));

    println!("  {:<14} {:>10} {:>10} {:>12} {:>8} {:>8}", "Symbol".bold(), "Cost".bold(), "Value".bold(), "Gain/Loss".bold(), "Type".bold(), "Tax".bold());
    println!("  {}", "─".repeat(68).dimmed());

    for h in &portfolio.holdings {
        let price = quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol)).and_then(|q| q.regular_market_price).unwrap_or(0.0);
        let cost = h.shares * h.avg_cost;
        let value = h.shares * price;
        let gain = value - cost;

        // Determine holding period from added_at
        let is_longterm = chrono::NaiveDate::parse_from_str(&h.added_at, "%Y-%m-%d")
            .ok()
            .map(|d| {
                let now = chrono::Local::now().date_naive();
                use chrono::Datelike;
                let months = (now.year() - d.year()) * 12 + (now.month() as i32 - d.month() as i32);
                months >= holding_months
            })
            .unwrap_or(false);

        let hold_type = if is_longterm { "LTCG" } else { "STCG" };
        let tax_rate = if is_longterm { ltcg_rate } else { stcg_rate };

        if gain >= 0.0 {
            if is_longterm { total_ltcg += gain; } else { total_stcg += gain; }
        } else {
            if is_longterm { total_ltcl += gain.abs(); } else { total_stcl += gain.abs(); }
        }

        let gain_str = if gain >= 0.0 { format!("+{}{:.2}", currency_sym, gain).green().to_string() } else { format!("-{}{:.2}", currency_sym, gain.abs()).red().to_string() };
        let tax = if gain > 0.0 { gain * tax_rate / 100.0 } else { 0.0 };
        let tax_str = if tax > 0.0 { format!("{}{:.0}", currency_sym, tax) } else { "—".dimmed().to_string() };

        println!("  {:<14} {:>10} {:>10} {:>12} {:>8} {:>8}",
            h.symbol.cyan(),
            format!("{}{:.0}", currency_sym, cost),
            format!("{}{:.0}", currency_sym, value),
            gain_str, hold_type, tax_str);
    }

    println!("  {}", "─".repeat(68).dimmed());

    print_section("Tax Summary");
    print_kv("Short-Term Gains", &format!("{}{:.2}", currency_sym, total_stcg));
    print_kv("Short-Term Losses", &format!("{}{:.2}", currency_sym, total_stcl));
    print_kv("Long-Term Gains", &format!("{}{:.2}", currency_sym, total_ltcg));
    print_kv("Long-Term Losses", &format!("{}{:.2}", currency_sym, total_ltcl));

    // Net gains after offsetting losses
    let net_stcg = (total_stcg - total_stcl).max(0.0);
    let net_ltcg = (total_ltcg - total_ltcl - ltcg_exempt).max(0.0);

    let stcg_tax = net_stcg * stcg_rate / 100.0;
    let ltcg_tax = net_ltcg * ltcg_rate / 100.0;
    let total_tax = stcg_tax + ltcg_tax;

    print_section("Estimated Tax Liability");
    print_kv(&format!("STCG Tax ({}%)", stcg_rate), &format!("{}{:.2}", currency_sym, stcg_tax));
    if ltcg_exempt > 0.0 {
        print_kv("LTCG Exemption", &format!("{}{:.0}", currency_sym, ltcg_exempt));
    }
    print_kv(&format!("LTCG Tax ({}%)", ltcg_rate), &format!("{}{:.2}", currency_sym, ltcg_tax));
    println!("  {}", "─".repeat(40).dimmed());
    let total_str = if total_tax > 0.0 { format!("{}{:.2}", currency_sym, total_tax).red().bold().to_string() } else { format!("{}{:.2}", currency_sym, total_tax).green().to_string() };
    print_kv("TOTAL TAX", &total_str);

    if total_stcl + total_ltcl > 0.0 {
        println!();
        print_kv("Tax Saved (losses)", &format!("{}{:.2}", currency_sym, (total_stcl * stcg_rate / 100.0 + total_ltcl * ltcg_rate / 100.0)).green().to_string());
    }

    println!("\n  {}", "Tax rates are approximate. Consult a chartered accountant / CPA.".dimmed().italic());
    println!();
    Ok(())
}
