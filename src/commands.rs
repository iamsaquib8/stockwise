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
use crate::simulator::{SimHistory, SimSession, SimTrade};
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

    // AI-powered analysis (if Ollama available)
    let ai = crate::ai::AiClient::new();
    if ai.is_available().await {
        print_section("Ollama AI Analysis");
        let stock_data = crate::ai::StockData::from_quote(q);
        match ai.analyze_stock(&stock_data).await {
            Ok(analysis) => { for line in analysis.lines() { println!("  {}", line); } }
            Err(_) => { println!("  {}", "AI analysis unavailable.".dimmed()); }
        }
    }

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

    // AI technical interpretation
    let ai = crate::ai::AiClient::new();
    if ai.is_available().await {
        let rsi_val: String = technical::rsi(&closes, 14).map_or("N/A".into(), |r| format!("{:.0}", r));
        let macd_val: String = technical::macd(&closes).map_or("N/A".into(), |(_,_,h)| if h > 0.0 { "bullish".into() } else { "bearish".into() });
        let bb_pos: String = technical::bollinger_bands(&closes, 20).map_or("N/A".into(), |(u,_,l)| { let p = (current - l) / (u - l) * 100.0; format!("{:.0}% of range", p) });
        let sma_pos: String = technical::sma(&closes, 200).map_or("N/A".into(), |s| if current > s { "above 200-MA".into() } else { "below 200-MA".into() });
        let data = format!("Stock: {}, Price: {:.2}, RSI: {}, MACD: {}, BB: {}, Trend: {}", resolved, current, rsi_val, macd_val, bb_pos, sma_pos);
        print_section("Ollama AI Interpretation");
        match ai.interpret_technicals(&data).await {
            Ok(analysis) => { for line in analysis.lines() { println!("  {}", line); } }
            Err(_) => { println!("  {}", "AI unavailable.".dimmed()); }
        }
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

    // AI comparison verdict
    let ai = crate::ai::AiClient::new();
    if ai.is_available().await {
        let mut data = String::new();
        for q in &quotes {
            let sym = q.symbol.as_deref().unwrap_or("?");
            data.push_str(&format!("{}: price={:.2}, P/E={}, fwdP/E={}, chg={:+.2}%\n", sym,
                q.regular_market_price.unwrap_or(0.0),
                q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.forward_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.regular_market_change_percent.unwrap_or(0.0)));
        }
        print_section("Ollama AI Verdict");
        if let Ok(verdict) = ai.compare_stocks(&data).await {
            for line in verdict.lines() { println!("  {}", line); }
        }
    }

    println!();
    Ok(())
}

pub async fn cmd_history(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    let (api_range, interval) = match period {
        "1d" => ("1d", "5m"),
        "5d" | "1w" => ("5d", "15m"),
        "1mo" => ("1mo", "1d"),
        "3mo" | "6mo" => (period, "1d"),
        "1y" | "2y" => (period, "1wk"),
        "5y" | "10y" | "max" => (period, "1mo"),
        _ => (period, "1d"),
    };

    let chart = client.get_chart(&resolved, api_range, interval).await?;
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

    // AI screen analysis
    let ai = crate::ai::AiClient::new();
    if ai.is_available().await && !filtered.is_empty() {
        let mut data = format!("Screen: {}\n", category);
        for q in filtered.iter().take(8) {
            let sym = q.symbol.as_deref().unwrap_or("?");
            data.push_str(&format!("{}: price={:.2}, P/E={}, chg={:+.2}%\n", sym,
                q.regular_market_price.unwrap_or(0.0),
                q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.regular_market_change_percent.unwrap_or(0.0)));
        }
        print_section("Ollama AI Analysis");
        if let Ok(analysis) = ai.analyze_screen(&data).await {
            for line in analysis.lines() { println!("  {}", line); }
        }
    }

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

pub async fn cmd_intraday(amount: f64, target_pct: f64, market: Market) -> Result<()> {
    let target_profit = amount * (target_pct / 100.0);
    let csym = match market { Market::In => "₹", Market::Us => "$" };
    let market_name = match market { Market::In => "Indian", Market::Us => "US" };

    print_header(&format!("Intraday Trading Bot — {} Market", market_name));
    println!("  Capital: {}  |  Target: {}% ({}{:.2})  |  Max Risk: 1%/trade\n",
        format!("{}{:.2}", csym, amount).bold().cyan(), target_pct, csym, target_profit);

    let client = YahooClient::new().await?;

    // Sector heat
    if market == Market::In {
        println!("  {} Scanning sectors...", "⟳".yellow());
        if let Ok(heats) = intraday::scan_sector_heat(&client).await {
            print_section("Sector Heat");
            for h in &heats {
                let bar_len = (h.change_pct.abs() * 5.0).min(20.0) as usize;
                let bar = if h.change_pct >= 0.0 { "█".repeat(bar_len.max(1)).green().to_string() } else { "█".repeat(bar_len.max(1)).red().to_string() };
                let pct = if h.change_pct >= 0.0 { format!("{:+.2}%", h.change_pct).green().to_string() } else { format!("{:+.2}%", h.change_pct).red().to_string() };
                let hot = if h.hot { " HOT".yellow().bold().to_string() } else { String::new() };
                println!("  {:<12} {} {}{}", h.name, bar, pct, hot);
            }
        }
    }

    // Scan stocks
    println!("\n  {} Scanning stocks with 9 strategies...\n", "⟳".yellow());
    let signals = intraday::scan_intraday(&client, market).await?;
    let plans = intraday::generate_trade_plans(&signals, amount, target_pct, 1.0);

    // Market regime summary
    let regimes: Vec<_> = signals.iter().map(|s| &s.regime).collect();
    let uptrend = regimes.iter().filter(|r| matches!(r, intraday::MarketRegime::Uptrend | intraday::MarketRegime::StrongUptrend)).count();
    let downtrend = regimes.iter().filter(|r| matches!(r, intraday::MarketRegime::Downtrend | intraday::MarketRegime::StrongDowntrend)).count();
    let market_bias = if uptrend > downtrend * 2 { "BULLISH".green().bold() } else if downtrend > uptrend * 2 { "BEARISH".red().bold() } else { "MIXED".yellow().bold() };
    println!("  Market Bias: {} ({} bullish, {} bearish, {} ranging)\n", market_bias, uptrend, downtrend, signals.len() - uptrend - downtrend);

    // Top signals table
    print_section("Signal Scanner");
    println!("  {:<14} {:>8} {:>8} {:>5} {:>6} {:>8} {:>7} {:>5}",
        "Symbol".bold(), "Price".bold(), "Chg%".bold(), "RSI".bold(), "Vol".bold(), "Regime".bold(), "Score".bold(), "Conf".bold());
    println!("  {}", "─".repeat(72).dimmed());

    for s in signals.iter().take(15) {
        let chg = if s.change_pct >= 0.0 { format!("{:+.2}%", s.change_pct).green().to_string() } else { format!("{:+.2}%", s.change_pct).red().to_string() };
        let rsi_str = s.rsi.map_or("—".into(), |r| { let t = format!("{:.0}", r); if r < 35.0 { t.green().to_string() } else if r > 70.0 { t.red().to_string() } else { t } });
        let regime_str = match s.regime { intraday::MarketRegime::StrongUptrend => "▲▲".green().to_string(), intraday::MarketRegime::Uptrend => "▲".green().to_string(), intraday::MarketRegime::Ranging => "─".yellow().to_string(), intraday::MarketRegime::Downtrend => "▼".red().to_string(), intraday::MarketRegime::StrongDowntrend => "▼▼".red().to_string() };
        let score_str = if s.score >= 70.0 { format!("{:.0}", s.score).green().bold().to_string() } else if s.score >= 55.0 { format!("{:.0}", s.score).yellow().to_string() } else { format!("{:.0}", s.score).dimmed().to_string() };
        let conf_str = match s.confidence { intraday::Confidence::High => "H".green().bold().to_string(), intraday::Confidence::Medium => "M".yellow().to_string(), intraday::Confidence::Low => "L".dimmed().to_string() };
        println!("  {:<14} {:>8} {:>8} {:>5} {:>6} {:>8} {:>7} {:>5}",
            s.symbol.cyan(), format!("{}{:.2}", csym, s.price), chg, rsi_str, format!("{:.1}x", s.volume_ratio), regime_str, score_str, conf_str);
    }

    // Trade plans
    if plans.is_empty() {
        println!("\n  {}", "No high-confidence trades found. Market conditions may be unfavorable.".yellow());
    } else {
        print_section(&format!("Trade Plans (target: {}{:.2})", csym, target_profit));

        for (i, plan) in plans.iter().take(5).enumerate() {
            let s = &plan.signal;
            println!();
            println!("  {}  {} — {} [{} | {} confidence]",
                format!("#{}", i + 1).bold().cyan(), s.symbol.bold().cyan(), s.name,
                s.regime, match s.confidence { intraday::Confidence::High => "HIGH".green().bold().to_string(), intraday::Confidence::Medium => "MED".yellow().to_string(), _ => "LOW".dimmed().to_string() });
            println!("  {}", "─".repeat(55).dimmed());

            // Show which strategies triggered
            println!("  {} {}", "Strategies:".dimmed(),
                s.strategies.iter().map(|st| format!("{} ({:.0})", st.name, st.strength)).collect::<Vec<_>>().join(" + "));
            for st in &s.strategies {
                println!("    {} {}", "→".cyan(), st.reason.dimmed());
            }

            println!();
            println!("  {} {}    {} {}    {} {}",
                "Entry:".dimmed(), format!("{}{:.2}", csym, plan.entry).bold(),
                "Target 1:".dimmed(), format!("{}{:.2}", csym, plan.target1).green(),
                "Target 2:".dimmed(), format!("{}{:.2}", csym, plan.target2).green().bold());
            println!("  {} {}    {} {}    {} {}",
                "Stop Loss:".dimmed(), format!("{}{:.2}", csym, plan.stop_loss).red(),
                "Trailing:".dimmed(), format!("{}{:.2}", csym, plan.trailing_stop).red().dimmed(),
                "R:R:".dimmed(), format!("1:{:.1}", plan.risk_reward).yellow());
            println!("  {} {}    {} {}    {} {}    {} {}",
                "Qty:".dimmed(), format!("{}", plan.qty).bold(),
                "Capital:".dimmed(), format!("{}{:.0} ({:.0}%)", csym, plan.capital_required, plan.position_pct),
                "Max Risk:".dimmed(), format!("{}{:.0}", csym, plan.max_risk).red(),
                "Exp P&L:".dimmed(), format!("+{}{:.0}", csym, plan.expected_profit).green().bold());
            println!("  {} Kelly fraction: {:.1}%", "→".dimmed(), plan.kelly_fraction * 100.0);
        }

        let total_profit: f64 = plans.iter().take(3).map(|p| p.expected_profit).sum();
        let total_risk: f64 = plans.iter().take(3).map(|p| p.max_risk).sum();
        println!("\n  {}", "─".repeat(60).dimmed());
        println!("  {} Top 3 combined: {} potential | {} max risk",
            if total_profit >= target_profit { "✓".green().bold() } else { "→".yellow().bold() },
            format!("+{}{:.0}", csym, total_profit).green().bold(),
            format!("{}{:.0}", csym, total_risk).red());
    }

    println!("\n  {}", "─".repeat(60).dimmed());
    println!("  {}", "This is NOT financial advice. Trade at your own risk.".red().italic());
    println!();
    Ok(())
}

// ── AI-Powered Reports ──

pub async fn cmd_report(what: &str, market: Market) -> Result<()> {
    let ai = crate::ai::AiClient::new();
    if !ai.is_available().await {
        println!("  {} Ollama is not running. Start it with: {}", "✗".red(), "ollama serve".cyan());
        println!("  {} Reports need a local AI model. Set model: {}", "→".dimmed(), "STOCKWISE_MODEL=gemma3:4b".cyan());
        return Ok(());
    }

    let client = YahooClient::new().await?;

    match what {
        "intraday" => {
            println!("  {} Generating intraday report with local AI...\n", "⟳".yellow());
            let signals = intraday::scan_intraday(&client, market).await?;
            // Build summary for AI
            let mut data = String::new();
            for s in signals.iter().take(10) {
                data.push_str(&format!("{}: price={:.2}, chg={:+.2}%, RSI={}, vol={:.1}x, regime={}, score={:.0}, strategies={}\n",
                    s.symbol, s.price, s.change_pct,
                    s.rsi.map_or("N/A".into(), |r| format!("{:.0}", r)),
                    s.volume_ratio, s.regime, s.score,
                    s.strategies.iter().map(|st| st.name).collect::<Vec<_>>().join("+")
                ));
            }
            let report = ai.generate_intraday_report(&data).await?;
            print_header("AI Intraday Trading Brief");
            println!();
            for line in report.lines() { println!("  {}", line); }
            println!();
        }
        "longterm" => {
            println!("  {} Generating long-term investment memo...\n", "⟳".yellow());
            let symbols = match market { Market::In => market::INDIA_POPULAR, Market::Us => market::US_POPULAR };
            let sym_refs: Vec<&str> = symbols.to_vec();
            let quotes = client.get_quote(&sym_refs).await?;
            let mut data = String::new();
            for q in quotes.iter().take(15) {
                let sym = q.symbol.as_deref().unwrap_or("?");
                data.push_str(&format!("{}: price={:.2}, P/E={}, fwdP/E={}, div={}, chg={:+.2}%, 50MA={:.2}, 200MA={:.2}\n",
                    sym, q.regular_market_price.unwrap_or(0.0),
                    q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                    q.forward_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                    q.trailing_annual_dividend_yield.map_or("0".into(), |v| format!("{:.2}%", v * 100.0)),
                    q.regular_market_change_percent.unwrap_or(0.0),
                    q.fifty_day_average.unwrap_or(0.0), q.two_hundred_day_average.unwrap_or(0.0),
                ));
            }
            let report = ai.generate_longterm_report(&data).await?;
            print_header("AI Long-Term Investment Memo");
            println!();
            for line in report.lines() { println!("  {}", line); }
            println!();
        }
        "portfolio" => {
            let portfolio = Portfolio::load()?;
            if portfolio.holdings.is_empty() { println!("  {}", "Portfolio is empty.".dimmed()); return Ok(()); }
            println!("  {} Analyzing your portfolio...\n", "⟳".yellow());
            let symbols: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
            let sym_refs: Vec<&str> = symbols.iter().map(|s| s.as_str()).collect();
            let quotes = client.get_quote(&sym_refs).await?;
            let mut data = String::new();
            for h in &portfolio.holdings {
                let q = quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol));
                let price = q.and_then(|q| q.regular_market_price).unwrap_or(0.0);
                let pnl_pct = ((price / h.avg_cost) - 1.0) * 100.0;
                data.push_str(&format!("{}: {} shares, avg={:.2}, now={:.2}, P&L={:+.1}%, P/E={}\n",
                    h.symbol, h.shares, h.avg_cost, price, pnl_pct,
                    q.and_then(|q| q.trailing_pe).map_or("N/A".into(), |v| format!("{:.1}", v)),
                ));
            }
            let report = ai.generate_portfolio_report(&data).await?;
            print_header("AI Portfolio Review");
            println!();
            for line in report.lines() { println!("  {}", line); }
            println!();
        }
        "stock" => {
            println!("  Usage: stockwise report stock <SYMBOL>");
            println!("  Example: stockwise report stock RELIANCE");
        }
        _ => {
            // Treat as a stock symbol
            let resolved = market::resolve_symbol(what, market);
            println!("  {} Analyzing {} with local AI...\n", "⟳".yellow(), resolved.cyan());
            let quotes = client.get_quote(&[resolved.as_str()]).await?;
            let q = quotes.first().context("Symbol not found")?;
            let stock_data = crate::ai::StockData::from_quote(q);
            let report = ai.analyze_stock(&stock_data).await?;
            print_header(&format!("AI Analysis: {}", resolved));
            println!();
            for line in report.lines() { println!("  {}", line); }
            println!();
        }
    }

    println!("  {}", "Generated by local AI (Ollama). Not financial advice.".dimmed().italic());
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
    // AI backtest interpretation
    let ai = crate::ai::AiClient::new();
    if ai.is_available().await {
        let data = format!("Strategy: {}, Period: {}, Return: {:.2}%, Buy&Hold: {:.2}%, Win rate: {:.0}%, Trades: {}, Max DD: {:.1}%, Sharpe: {}",
            strategy, period, result.total_return, result.buy_hold_return, result.win_rate, result.trades.len(), result.max_drawdown,
            result.sharpe.map_or("N/A".into(), |s| format!("{:.2}", s)));
        print_section("Ollama AI Interpretation");
        if let Ok(interp) = ai.interpret_backtest(&data).await {
            for line in interp.lines() { println!("  {}", line); }
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

    print_header(&format!("Long-Term Wealth Bot v2 — {} Market", match market { Market::Us => "US", Market::In => "India" }));
    println!("  {} Scoring {} stocks across 6 pillars + Monte Carlo...\n", "⟳".yellow(), quotes.len());

    // Fetch 1Y chart data for volatility analysis (for top candidates)
    let mut scores: Vec<longterm::LongTermScore> = Vec::new();
    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("");
        let hist = client.get_chart(sym, "1y", "1d").await.ok().and_then(|c| {
            c.indicators.quote.first().and_then(|qi| qi.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect::<Vec<f64>>())
        });
        scores.push(longterm::score_for_longterm(q, hist.as_deref()));
    }
    scores.sort_by(|a, b| b.total_score.partial_cmp(&a.total_score).unwrap());

    // Summary table with all 6 pillars
    print_section("Ranking (6-Pillar Score)");
    println!("  {:<14} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6} {:>8}",
        "Symbol".bold(), "Val".bold(), "Grw".bold(), "Qua".bold(), "Mom".bold(), "Div".bold(), "Saf".bold(), "Total".bold(), "Moat".bold(), "Risk".bold());
    println!("  {}", "─".repeat(78).dimmed());

    for s in scores.iter().take(12) {
        let sc = if s.total_score >= 70.0 { format!("{:.0}", s.total_score).green().bold().to_string() }
            else if s.total_score >= 55.0 { format!("{:.0}", s.total_score).yellow().to_string() }
            else { format!("{:.0}", s.total_score).dimmed().to_string() };
        let moat_str = match s.moat { longterm::MoatRating::Wide => "Wide".green().to_string(), longterm::MoatRating::Narrow => "Nar".yellow().to_string(), longterm::MoatRating::None => "—".dimmed().to_string() };
        let risk_str = match s.risk_tier { longterm::RiskTier::Conservative => "Cons".green().to_string(), longterm::RiskTier::Moderate => "Mod".yellow().to_string(), longterm::RiskTier::Aggressive => "Aggr".red().to_string() };
        println!("  {:<14} {:>5.0} {:>5.0} {:>5.0} {:>5.0} {:>5.0} {:>5.0} {:>6} {:>6} {:>8}",
            s.symbol.cyan(), s.valuation_score, s.growth_score, s.quality_score, s.momentum_score, s.dividend_score, s.safety_score, sc, moat_str, risk_str);
    }

    // Detailed top 3
    for (i, s) in scores.iter().take(3).enumerate() {
        println!();
        print_section(&format!("#{} {} — {} [{}]", i + 1, s.symbol, s.name, s.moat));
        print_kv("Price", &format!("{}{:.2}", csym, s.price));
        print_kv("Score", &format!("{:.0}/100 ({})", s.total_score, s.risk_tier).bold().to_string());
        if let Some(peg) = s.peg_ratio { print_kv("PEG Ratio", &format!("{:.2}", peg)); }
        if let Some(ey) = s.earnings_yield { print_kv("Earnings Yield", &format!("{:.1}%", ey)); }

        if !s.reasons.is_empty() {
            println!();
            for r in &s.reasons { println!("    {} {}", "✓".green(), r); }
        }
        if !s.risk_flags.is_empty() {
            for r in &s.risk_flags { println!("    {} {}", "✗".red(), r); }
        }

        print_section("Projections");
        print_kv("Est. Annual Return", &format!("{:.1}%", s.est_annual_return));
        print_kv("5-Year Projection", &format!("+{:.0}%", s.projected_5y_return).green().to_string());
        print_kv("10-Year Projection", &format!("+{:.0}%", s.projected_10y_return).green().to_string());
        if s.drip_multiplier_10y > 1.01 {
            print_kv("DRIP Boost (10Y)", &format!("{:.2}x from dividend reinvestment", s.drip_multiplier_10y));
        }
        print_kv(&format!("SIP for {}10L/10Y", csym), &format!("{}{:.0}/month", csym, s.sip_monthly_10l_10y));

        // Monte Carlo
        print_section("Monte Carlo (1000 sims, 5Y)");
        let mc_bar = |val: f64| -> String { if val >= 0.0 { format!("+{:.0}%", val).green().to_string() } else { format!("{:.0}%", val).red().to_string() } };
        print_kv("Best case (P90)", &mc_bar(s.monte_carlo_p90));
        print_kv("Median", &mc_bar(s.monte_carlo_median));
        print_kv("Worst case (P10)", &mc_bar(s.monte_carlo_p10));

        // SIP projection
        let inv_5y = monthly * 60.0;
        let est_5y = inv_5y * (1.0 + s.projected_5y_return / 100.0);
        let inv_10y = monthly * 120.0;
        let est_10y = inv_10y * (1.0 + s.projected_10y_return / 100.0) * s.drip_multiplier_10y;
        print_section("SIP Calculator");
        print_kv(&format!("{}{}/mo × 5Y", csym, monthly), &format!("{}{:.0} → {}{:.0}", csym, inv_5y, csym, est_5y).green().to_string());
        print_kv(&format!("{}{}/mo × 10Y", csym, monthly), &format!("{}{:.0} → {}{:.0} (incl DRIP)", csym, inv_10y, csym, est_10y).green().bold().to_string());
    }

    // Portfolio allocation
    print_section("Suggested SIP Allocation");
    let top5: Vec<&longterm::LongTermScore> = scores.iter().take(5).collect();
    let total_sc: f64 = top5.iter().map(|s| s.total_score).sum();
    println!("  Monthly budget: {}{:.0}\n", csym, monthly);
    for s in &top5 {
        let pct = s.total_score / total_sc;
        let alloc = monthly * pct;
        let bar_len = (pct * 30.0) as usize;
        let moat_tag = match s.moat { longterm::MoatRating::Wide => " [Wide Moat]".green().to_string(), longterm::MoatRating::Narrow => " [Narrow]".yellow().to_string(), _ => String::new() };
        println!("  {:<14} {}{:>8.0} ({:>4.1}%) {}{}", s.symbol.cyan(), csym, alloc, pct * 100.0, "█".repeat(bar_len).green(), moat_tag);
    }

    println!("\n  {}", "─".repeat(60).dimmed());
    println!("  {}", "Long-term investing involves risk. Diversify across asset classes.".dimmed().italic());
    println!("  {}", "Monte Carlo uses historical volatility. Past ≠ future.".dimmed().italic());
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

// ── Import from brokers ──

pub async fn cmd_import(source: &str, file: &str, market: Market) -> Result<()> {
    let path = std::path::Path::new(file);
    if !path.exists() {
        println!("  {} File not found: {}", "✗".red(), file);
        println!();
        println!("  {} How to export:", "→".cyan());
        match source {
            "kite" | "zerodha" => {
                println!("    1. Login to {} → Portfolio → Holdings", "kite.zerodha.com".cyan());
                println!("    2. Click the {} icon (top-right of holdings table)", "download/export".bold());
                println!("    3. Save the CSV file");
                println!("    4. Run: stockwise import kite <path-to-file.csv>");
            }
            "indmoney" => {
                println!("    1. Open {} app → Portfolio → Stocks", "IndMoney".cyan());
                println!("    2. Tap {} → Download Statement / Export", "⋮ (menu)".bold());
                println!("    3. Save the CSV/Excel file");
                println!("    4. Run: stockwise import indmoney <path-to-file.csv>");
            }
            _ => {
                println!("    Provide a CSV with columns: Symbol, Quantity, Average Price");
                println!("    Run: stockwise import csv <path-to-file.csv>");
            }
        }
        return Ok(());
    }

    let content = std::fs::read_to_string(path).context("Failed to read file")?;
    let lines: Vec<&str> = content.lines().collect();
    if lines.len() < 2 {
        println!("  {} File is empty or has no data rows.", "✗".red());
        return Ok(());
    }

    let header = lines[0].to_lowercase();
    let mut portfolio = Portfolio::load()?;
    let mut imported = 0;

    match source {
        "kite" | "zerodha" => {
            // Kite CSV: Instrument, Qty., Avg. cost, LTP, Cur. val, P&L, Net chg., Day chg.
            let cols = parse_csv_header(&header);
            let sym_col = find_col(&cols, &["instrument", "tradingsymbol", "symbol", "stock"]);
            let qty_col = find_col(&cols, &["qty", "qty.", "quantity", "shares"]);
            let avg_col = find_col(&cols, &["avg. cost", "avg cost", "average cost", "avg_price", "average price", "buy avg", "buy avg."]);

            if sym_col.is_none() || qty_col.is_none() || avg_col.is_none() {
                println!("  {} Could not detect Kite CSV columns.", "✗".red());
                println!("  Expected columns: Instrument, Qty., Avg. cost");
                println!("  Found: {}", header);
                return Ok(());
            }
            let (si, qi, ai) = (sym_col.unwrap(), qty_col.unwrap(), avg_col.unwrap());

            for line in &lines[1..] {
                let fields = parse_csv_row(line);
                if fields.len() <= si.max(qi).max(ai) { continue; }
                let symbol_raw = fields[si].trim().trim_matches('"');
                if symbol_raw.is_empty() { continue; }
                let qty: f64 = fields[qi].trim().trim_matches('"').replace(',', "").parse().unwrap_or(0.0);
                let avg: f64 = fields[ai].trim().trim_matches('"').replace(',', "").parse().unwrap_or(0.0);
                if qty <= 0.0 || avg <= 0.0 { continue; }

                let resolved = market::resolve_symbol(symbol_raw, market);
                portfolio.add(&resolved, qty, avg);
                imported += 1;
                println!("  {} {} — {} shares @ ₹{:.2}", "✓".green(), resolved.cyan(), qty, avg);
            }
        }
        "indmoney" => {
            // IndMoney: Stock Name, Symbol/ISIN, Quantity, Avg Buy Price, Current Price, ...
            let cols = parse_csv_header(&header);
            let sym_col = find_col(&cols, &["symbol", "isin", "stock symbol", "scrip", "stock name", "name"]);
            let qty_col = find_col(&cols, &["quantity", "qty", "shares", "units"]);
            let avg_col = find_col(&cols, &["avg buy price", "avg price", "average price", "buy price", "avg. buy price", "average cost"]);

            if sym_col.is_none() || qty_col.is_none() || avg_col.is_none() {
                println!("  {} Could not detect IndMoney CSV columns.", "✗".red());
                println!("  Expected columns: Symbol/Stock Name, Quantity, Avg Buy Price");
                println!("  Found: {}", header);
                return Ok(());
            }
            let (si, qi, ai) = (sym_col.unwrap(), qty_col.unwrap(), avg_col.unwrap());

            for line in &lines[1..] {
                let fields = parse_csv_row(line);
                if fields.len() <= si.max(qi).max(ai) { continue; }
                let symbol_raw = fields[si].trim().trim_matches('"');
                if symbol_raw.is_empty() { continue; }
                let qty: f64 = fields[qi].trim().trim_matches('"').replace(',', "").parse().unwrap_or(0.0);
                let avg: f64 = fields[ai].trim().trim_matches('"').replace(',', "").parse().unwrap_or(0.0);
                if qty <= 0.0 || avg <= 0.0 { continue; }

                let resolved = market::resolve_symbol(symbol_raw, market);
                portfolio.add(&resolved, qty, avg);
                imported += 1;
                println!("  {} {} — {} shares @ ₹{:.2}", "✓".green(), resolved.cyan(), qty, avg);
            }
        }
        "csv" | _ => {
            // Generic CSV: try to find Symbol, Quantity, Price columns
            let cols = parse_csv_header(&header);
            let sym_col = find_col(&cols, &["symbol", "stock", "instrument", "name", "ticker", "scrip", "tradingsymbol"]);
            let qty_col = find_col(&cols, &["quantity", "qty", "qty.", "shares", "units"]);
            let avg_col = find_col(&cols, &["avg price", "avg. price", "average price", "avg cost", "avg. cost", "buy price", "price", "cost"]);

            if sym_col.is_none() || qty_col.is_none() || avg_col.is_none() {
                println!("  {} Could not auto-detect CSV columns.", "✗".red());
                println!("  Your CSV must have columns for: Symbol, Quantity, Average Price");
                println!("  Found: {}", header);
                return Ok(());
            }
            let (si, qi, ai) = (sym_col.unwrap(), qty_col.unwrap(), avg_col.unwrap());

            for line in &lines[1..] {
                let fields = parse_csv_row(line);
                if fields.len() <= si.max(qi).max(ai) { continue; }
                let symbol_raw = fields[si].trim().trim_matches('"');
                if symbol_raw.is_empty() { continue; }
                let qty: f64 = fields[qi].trim().trim_matches('"').replace(',', "").parse().unwrap_or(0.0);
                let avg: f64 = fields[ai].trim().trim_matches('"').replace(',', "").parse().unwrap_or(0.0);
                if qty <= 0.0 || avg <= 0.0 { continue; }

                let resolved = market::resolve_symbol(symbol_raw, market);
                portfolio.add(&resolved, qty, avg);
                imported += 1;
                println!("  {} {} — {} shares @ {:.2}", "✓".green(), resolved.cyan(), qty, avg);
            }
        }
    }

    if imported > 0 {
        portfolio.save()?;
        println!("\n  {} Imported {} holdings into portfolio.", "✓".green().bold(), imported);
        println!("  Run {} to see your portfolio.", "stockwise portfolio show".cyan());
    } else {
        println!("\n  {} No holdings found in file. Check the format.", "✗".red());
    }
    println!();
    Ok(())
}

fn parse_csv_header(header: &str) -> Vec<String> {
    parse_csv_row(header).iter().map(|s| s.trim().trim_matches('"').to_lowercase()).collect()
}

fn parse_csv_row(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        match ch {
            '"' => in_quotes = !in_quotes,
            ',' if !in_quotes => {
                fields.push(current.clone());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    fields.push(current);
    fields
}

fn find_col(cols: &[String], names: &[&str]) -> Option<usize> {
    for name in names {
        if let Some(i) = cols.iter().position(|c| c.contains(name)) {
            return Some(i);
        }
    }
    None
}

// ── Angel One Trading ──

pub async fn cmd_trade(action: &str, symbol: Option<&str>, qty: Option<u32>, price: Option<f64>, _market: Market) -> Result<()> {
    match action {
        "setup" => {
            println!();
            print_header("Angel One SmartAPI Setup");
            println!("  To connect your Angel One account, you need:");
            println!();
            println!("  1. Go to {} and create an app", "smartapi.angelone.in".cyan());
            println!("  2. Note your {} (from the app dashboard)", "API Key".bold());
            println!("  3. Your {} (Angel One login ID)", "Client ID".bold());
            println!("  4. Your {} (Angel One login password)", "Password".bold());
            println!("  5. Your {} (from Authenticator app / Angel One TOTP setup)", "TOTP Secret".bold());
            println!();
            println!("  Then run:");
            println!("    {} stockwise trade config <API_KEY> <CLIENT_ID> <PASSWORD> <TOTP_SECRET>", "$".dimmed());
            println!();
            println!("  {}", "Your credentials are stored locally and never sent anywhere except Angel One's API.".dimmed());
            println!();
            Ok(())
        }
        "config" => {
            // symbol=api_key, qty arg repurposed... Actually let's parse from positional args differently
            // This is called as: stockwise trade config <API_KEY> <CLIENT_ID> <PASSWORD> <TOTP_SECRET>
            // But our CLI structure passes them as symbol, and we need more args.
            // For simplicity, read from the config directly
            println!("  {}", "Use this format:".bold());
            println!("    stockwise trade config");
            println!("  Then edit the config file at:");
            let path = dirs::data_local_dir().map(|d| d.join("stockwise/angel_config.json"));
            if let Some(p) = &path { println!("    {}", p.display().to_string().cyan()); }
            println!();
            // Create template config
            let config = crate::angel::AngelConfig::load()?;
            if !config.is_configured() {
                let template = crate::angel::AngelConfig {
                    api_key: "YOUR_API_KEY".into(),
                    client_id: "YOUR_CLIENT_ID".into(),
                    password: "YOUR_PASSWORD".into(),
                    totp_secret: "YOUR_TOTP_SECRET".into(),
                };
                template.save()?;
                println!("  {} Created config template. Edit the file above with your credentials.", "✓".green());
            } else {
                println!("  {} Already configured for client: {}", "✓".green(), config.client_id.cyan());
            }
            println!();
            Ok(())
        }
        "holdings" => {
            let client = crate::angel::AngelClient::new().await?;
            let holdings = client.get_holdings().await?;

            if holdings.is_empty() {
                println!("\n  {}", "No holdings found.".dimmed());
                return Ok(());
            }

            print_header("Angel One — Holdings (Live)");
            println!("  {:<14} {:>8} {:>10} {:>10} {:>12} {:>10}",
                "Symbol".bold(), "Qty".bold(), "Avg Cost".bold(), "LTP".bold(), "P&L".bold(), "P&L %".bold());
            println!("  {}", "─".repeat(68).dimmed());

            let mut total_pnl = 0.0_f64;
            let mut total_value = 0.0_f64;
            let mut total_cost = 0.0_f64;

            for h in &holdings {
                let sym = h.tradingsymbol.as_deref().unwrap_or("???");
                let qty = h.quantity.unwrap_or(0);
                let avg = h.averageprice.unwrap_or(0.0);
                let ltp = h.ltp.unwrap_or(0.0);
                let pnl = h.pnl.unwrap_or(0.0);
                let pnl_pct = h.pnlpercentage.unwrap_or(0.0);
                total_pnl += pnl;
                total_value += ltp * qty as f64;
                total_cost += avg * qty as f64;

                let pnl_str = if pnl >= 0.0 { format!("+₹{:.2}", pnl).green().to_string() } else { format!("-₹{:.2}", pnl.abs()).red().to_string() };
                let pct_str = if pnl_pct >= 0.0 { format!("+{:.2}%", pnl_pct).green().to_string() } else { format!("{:.2}%", pnl_pct).red().to_string() };

                println!("  {:<14} {:>8} {:>10} {:>10} {:>12} {:>10}",
                    sym.cyan(), qty, format!("₹{:.2}", avg), format!("₹{:.2}", ltp), pnl_str, pct_str);
            }

            println!("  {}", "─".repeat(68).dimmed());
            let total_str = if total_pnl >= 0.0 { format!("+₹{:.2}", total_pnl).green().bold().to_string() } else { format!("-₹{:.2}", total_pnl.abs()).red().bold().to_string() };
            println!("  {:<14} {:>8} {:>10} {:>10} {:>12}",
                "TOTAL".bold(), "", format!("₹{:.0}", total_cost), format!("₹{:.0}", total_value), total_str);

            // Also sync to stockwise portfolio
            let mut portfolio = Portfolio::load()?;
            for h in &holdings {
                let sym = h.tradingsymbol.as_deref().unwrap_or("");
                let qty = h.quantity.unwrap_or(0) as f64;
                let avg = h.averageprice.unwrap_or(0.0);
                if !sym.is_empty() && qty > 0.0 && avg > 0.0 {
                    let resolved = if sym.contains('.') { sym.to_string() } else { format!("{}.NS", sym) };
                    // Remove old and re-add to sync
                    portfolio.remove(&resolved);
                    portfolio.add(&resolved, qty, avg);
                }
            }
            portfolio.save()?;
            println!("\n  {} Holdings synced to StockWise portfolio.", "✓".green());
            println!();
            Ok(())
        }
        "positions" => {
            let client = crate::angel::AngelClient::new().await?;
            let positions = client.get_positions().await?;

            if positions.is_empty() {
                println!("\n  {}", "No open positions.".dimmed());
                return Ok(());
            }

            print_header("Angel One — Open Positions");
            println!("  {:<14} {:>8} {:>10} {:>10} {:>12}",
                "Symbol".bold(), "Qty".bold(), "Buy Avg".bold(), "LTP".bold(), "P&L".bold());
            println!("  {}", "─".repeat(58).dimmed());

            for p in &positions {
                let sym = p.tradingsymbol.as_deref().unwrap_or("???");
                let qty = p.quantity.as_deref().unwrap_or("0");
                let avg = p.buyavgprice.as_deref().unwrap_or("0");
                let ltp = p.ltp.as_deref().unwrap_or("0");
                let pnl = p.pnl.as_deref().unwrap_or("0");
                let pnl_f: f64 = pnl.parse().unwrap_or(0.0);
                let pnl_str = if pnl_f >= 0.0 { format!("+₹{}", pnl).green().to_string() } else { format!("-₹{}", pnl).red().to_string() };
                println!("  {:<14} {:>8} {:>10} {:>10} {:>12}", sym.cyan(), qty, format!("₹{}", avg), format!("₹{}", ltp), pnl_str);
            }
            println!();
            Ok(())
        }
        "buy" => {
            let sym = symbol.context("Symbol required. Usage: stockwise trade buy RELIANCE 10")?;
            let quantity = qty.context("Quantity required. Usage: stockwise trade buy RELIANCE 10")?;
            let sym_upper = sym.to_uppercase();

            let client = crate::angel::AngelClient::new().await?;

            // Search for the symbol token
            let token = client.search_scrip(&sym_upper, "NSE").await?
                .context(format!("Could not find symbol {} on NSE", sym_upper))?;

            println!("  {} Placing {} order: {} {} shares...", "⟳".yellow(), "BUY".green().bold(), sym_upper.cyan(), quantity);

            let result = client.place_order(&sym_upper, &token, "NSE", "BUY", quantity, "MARKET", 0.0, 0.0).await?;

            if let Some(id) = &result.orderid {
                println!("  {} Order placed! ID: {}", "✓".green().bold(), id.cyan());
                println!("  {} BUY {} × {} shares at MARKET", "→".green(), sym_upper.cyan(), quantity);
            }
            println!();
            Ok(())
        }
        "sell" => {
            let sym = symbol.context("Symbol required. Usage: stockwise trade sell RELIANCE 10")?;
            let quantity = qty.context("Quantity required. Usage: stockwise trade sell RELIANCE 10")?;
            let sym_upper = sym.to_uppercase();

            let client = crate::angel::AngelClient::new().await?;
            let token = client.search_scrip(&sym_upper, "NSE").await?
                .context(format!("Could not find symbol {} on NSE", sym_upper))?;

            println!("  {} Placing {} order: {} {} shares...", "⟳".yellow(), "SELL".red().bold(), sym_upper.cyan(), quantity);

            let result = client.place_order(&sym_upper, &token, "NSE", "SELL", quantity, "MARKET", 0.0, 0.0).await?;

            if let Some(id) = &result.orderid {
                println!("  {} Order placed! ID: {}", "✓".green().bold(), id.cyan());
                println!("  {} SELL {} × {} shares at MARKET", "→".red(), sym_upper.cyan(), quantity);
            }
            println!();
            Ok(())
        }
        "limit" => {
            let sym = symbol.context("Symbol required. Usage: stockwise trade limit RELIANCE 10 1300")?;
            let quantity = qty.context("Quantity required")?;
            let limit_price = price.context("Limit price required. Usage: stockwise trade limit RELIANCE 10 1300")?;
            let sym_upper = sym.to_uppercase();

            let client = crate::angel::AngelClient::new().await?;
            let token = client.search_scrip(&sym_upper, "NSE").await?
                .context(format!("Could not find symbol {}", sym_upper))?;

            println!("  {} Placing LIMIT BUY: {} × {} @ ₹{:.2}...", "⟳".yellow(), sym_upper.cyan(), quantity, limit_price);

            let result = client.place_order(&sym_upper, &token, "NSE", "BUY", quantity, "LIMIT", limit_price, 0.0).await?;

            if let Some(id) = &result.orderid {
                println!("  {} Limit order placed! ID: {}", "✓".green().bold(), id.cyan());
            }
            println!();
            Ok(())
        }
        "orders" => {
            let client = crate::angel::AngelClient::new().await?;
            let orders = client.get_order_book().await?;

            if orders.is_empty() {
                println!("\n  {}", "No orders today.".dimmed());
                return Ok(());
            }

            print_header("Angel One — Order Book");
            println!("  {:<12} {:<14} {:>6} {:>8} {:>10} {:>12}",
                "Order ID".bold(), "Symbol".bold(), "Type".bold(), "Qty".bold(), "Price".bold(), "Status".bold());
            println!("  {}", "─".repeat(66).dimmed());

            for o in &orders {
                let id = o.orderid.as_deref().unwrap_or("???");
                let sym = o.tradingsymbol.as_deref().unwrap_or("???");
                let txn = o.transactiontype.as_deref().unwrap_or("?");
                let qty = o.quantity.as_deref().unwrap_or("0");
                let price = o.price.as_deref().unwrap_or("0");
                let status = o.status.as_deref().unwrap_or("???");
                let txn_str = if txn == "BUY" { txn.green().to_string() } else { txn.red().to_string() };
                let status_str = match status {
                    "complete" => status.green().bold().to_string(),
                    "rejected" => status.red().to_string(),
                    "open" | "pending" => status.yellow().to_string(),
                    _ => status.to_string(),
                };
                println!("  {:<12} {:<14} {:>6} {:>8} {:>10} {:>12}",
                    id.dimmed(), sym.cyan(), txn_str, qty, format!("₹{}", price), status_str);
            }
            println!();
            Ok(())
        }
        _ => {
            println!();
            println!("  {} Angel One Trading Commands:", "→".cyan());
            println!();
            println!("    {} — Set up Angel One API credentials", "stockwise trade setup".bold());
            println!("    {} — Create config file to edit", "stockwise trade config".bold());
            println!("    {} — View your live holdings", "stockwise trade holdings".bold());
            println!("    {} — View open intraday positions", "stockwise trade positions".bold());
            println!("    {} — Place a market buy order", "stockwise trade buy RELIANCE 10".bold());
            println!("    {} — Place a market sell order", "stockwise trade sell RELIANCE 10".bold());
            println!("    {} — Place a limit buy order", "stockwise trade limit RELIANCE 10 1300".bold());
            println!("    {} — View today's order book", "stockwise trade orders".bold());
            println!();
            Ok(())
        }
    }
}

// ══════════════════════════════════════════════════════════
// ADVANCED STOCK FEATURES
// ══════════════════════════════════════════════════════════

// ── 1. Support & Resistance ──

pub async fn cmd_support(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "3mo", "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    if closes.len() < 5 { println!("{}", "Not enough data.".red()); return Ok(()); }

    let current = *closes.last().unwrap();
    let last_high = *highs.last().unwrap();
    let last_low = *lows.last().unwrap();
    let last_close = *closes.last().unwrap();

    let (pivot, r1, r2, r3, s1, s2, s3) = technical::pivot_points(last_high, last_low, last_close);

    print_header(&format!("Support & Resistance: {}", resolved));
    println!("  Current Price: {}\n", format_price(current, cur).bold());

    print_section("Classic Pivot Points");
    print_kv("R3 (Resistance)", &format!("{}{:.2}", csym, r3).red().to_string());
    print_kv("R2", &format!("{}{:.2}", csym, r2).red().to_string());
    print_kv("R1", &format!("{}{:.2}", csym, r1).red().to_string());
    print_kv("Pivot", &format!("{}{:.2}", csym, pivot).bold().to_string());
    print_kv("S1", &format!("{}{:.2}", csym, s1).green().to_string());
    print_kv("S2", &format!("{}{:.2}", csym, s2).green().to_string());
    print_kv("S3 (Support)", &format!("{}{:.2}", csym, s3).green().to_string());

    // Fibonacci levels
    if let Some((swing_high, swing_low)) = technical::find_swing_points(&highs, &lows, 60.min(highs.len())) {
        let fibs = technical::fibonacci_levels(swing_high, swing_low);
        print_section("Fibonacci Retracement");
        print_kv("Swing High", &format!("{}{:.2}", csym, swing_high));
        print_kv("23.6%", &format!("{}{:.2}", csym, fibs[0]));
        print_kv("38.2%", &format!("{}{:.2}", csym, fibs[1]));
        print_kv("50.0%", &format!("{}{:.2}", csym, fibs[2]).bold().to_string());
        print_kv("61.8%", &format!("{}{:.2}", csym, fibs[3]));
        print_kv("78.6%", &format!("{}{:.2}", csym, fibs[4]));
        print_kv("Swing Low", &format!("{}{:.2}", csym, swing_low));
    }

    // Price position
    print_section("Current Position");
    if current > r1 { println!("  {} Above R1 — bullish, next target R2 ({}{:.2})", "▲".green(), csym, r2); }
    else if current > pivot { println!("  {} Above pivot — mildly bullish, resistance at R1 ({}{:.2})", "▲".green(), csym, r1); }
    else if current > s1 { println!("  {} Below pivot — mildly bearish, support at S1 ({}{:.2})", "▼".yellow(), csym, s1); }
    else { println!("  {} Below S1 — bearish, next support S2 ({}{:.2})", "▼".red(), csym, s2); }
    println!();
    Ok(())
}

// ── 2. Volume Profile ──

pub async fn cmd_volume(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "3mo", "1d").await?;
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let volumes: Vec<u64> = chart.indicators.quote.first().and_then(|q| q.volume.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let n = closes.len().min(volumes.len());
    if n < 10 { println!("{}", "Not enough data.".red()); return Ok(()); }

    print_header(&format!("Volume Analysis: {}", resolved));

    // OBV
    let obv = technical::obv(&closes, &volumes);
    if obv.len() > 20 {
        let recent_obv: Vec<f64> = obv[obv.len() - 60.min(obv.len())..].to_vec();
        for line in charts::line_chart(&recent_obv, 50, 6, "cyan", "On-Balance Volume (OBV)") { println!("{}", line); }
        let obv_trend = if obv.last() > obv.get(obv.len().saturating_sub(20)) { "Rising — accumulation".green().to_string() } else { "Falling — distribution".red().to_string() };
        print_kv("OBV Trend", &obv_trend);
    }

    // A/D Line
    let ad = technical::ad_line(&highs, &lows, &closes, &volumes);
    if ad.len() > 20 {
        let recent_ad: Vec<f64> = ad[ad.len() - 60.min(ad.len())..].to_vec();
        println!();
        for line in charts::line_chart(&recent_ad, 50, 6, "yellow", "Accumulation/Distribution") { println!("{}", line); }
    }

    // Volume stats
    print_section("Volume Statistics");
    let avg_vol_20: u64 = volumes[n.saturating_sub(20)..].iter().sum::<u64>() / 20.min(n) as u64;
    let today_vol = *volumes.last().unwrap();
    let ratio = today_vol as f64 / avg_vol_20.max(1) as f64;
    print_kv("Today's Volume", &format_volume(today_vol));
    print_kv("20-Day Avg Volume", &format_volume(avg_vol_20));
    let ratio_str = if ratio > 1.5 { format!("{:.2}x (HIGH)", ratio).green().bold().to_string() } else if ratio > 1.0 { format!("{:.2}x (above avg)", ratio).to_string() } else { format!("{:.2}x (below avg)", ratio).red().to_string() };
    print_kv("Volume Ratio", &ratio_str);

    // Volume bars
    let recent_vols: Vec<u64> = volumes[n.saturating_sub(40)..].to_vec();
    println!();
    println!("  {}", "Volume (40 days)".bold());
    for line in charts::volume_bars(&recent_vols, 40, 4) { println!("{}", line); }
    println!();
    Ok(())
}

// ── 3. Gaps ──

pub async fn cmd_gaps(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "3mo", "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);
    let opens: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.open.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();

    let gaps = technical::detect_gaps(&opens, &highs, &lows, &closes);
    let timestamps = chart.timestamp.unwrap_or_default();

    print_header(&format!("Gap Analysis: {}", resolved));

    if gaps.is_empty() {
        println!("  {}", "No price gaps detected in the last 3 months.".dimmed());
        println!();
        return Ok(());
    }

    let unfilled: Vec<&technical::Gap> = gaps.iter().filter(|g| !g.filled).collect();
    let filled: Vec<&technical::Gap> = gaps.iter().filter(|g| g.filled).collect();

    println!("  Found {} gaps ({} unfilled, {} filled)\n", gaps.len(), unfilled.len(), filled.len());

    if !unfilled.is_empty() {
        print_section("Open (Unfilled) Gaps");
        for g in &unfilled {
            let date = timestamps.get(g.index).and_then(|&t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
            let dir = match g.gap_type { technical::GapType::Up => "GAP UP".green().bold().to_string(), technical::GapType::Down => "GAP DOWN".red().bold().to_string() };
            println!("  {} {} — {}{:.2} to {}{:.2} ({})", date.dimmed(), dir, csym, g.gap_low, csym, g.gap_high, format!("{}{:.2} range", csym, g.gap_high - g.gap_low).dimmed());
        }
    }

    if !filled.is_empty() {
        print_section(&format!("Filled Gaps ({})", filled.len()));
        for g in filled.iter().rev().take(5) {
            let date = timestamps.get(g.index).and_then(|&t| chrono::DateTime::from_timestamp(t, 0)).map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
            let dir = match g.gap_type { technical::GapType::Up => "UP", technical::GapType::Down => "DN" };
            println!("  {} {} {}{:.2}–{}{:.2} {}", date.dimmed(), dir, csym, g.gap_low, csym, g.gap_high, "FILLED".dimmed());
        }
    }
    println!();
    Ok(())
}

// ── 4. Stoploss Calculator ──

pub async fn cmd_stoploss(symbol: &str, entry: Option<f64>, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "1mo", "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    if closes.len() < 14 { println!("{}", "Not enough data.".red()); return Ok(()); }

    let current = *closes.last().unwrap();
    let entry_price = entry.unwrap_or(current);
    let atr = technical::atr(&highs, &lows, &closes, 14).unwrap_or(0.0);

    print_header(&format!("Stop-Loss Calculator: {}", resolved));
    print_kv("Entry Price", &format!("{}{:.2}", csym, entry_price));
    print_kv("Current Price", &format!("{}{:.2}", csym, current));
    print_kv("ATR (14)", &format!("{}{:.2} ({:.2}%)", csym, atr, (atr / current) * 100.0));

    let (cons, moderate, aggressive) = technical::atr_stop_loss(entry_price, atr);
    print_section("ATR-Based Stop Loss");
    print_kv("Conservative (3x ATR)", &format!("{}{:.2} ({:.2}% risk)", csym, cons, ((entry_price - cons) / entry_price) * 100.0).green().to_string());
    print_kv("Moderate (2x ATR)", &format!("{}{:.2} ({:.2}% risk)", csym, moderate, ((entry_price - moderate) / entry_price) * 100.0).yellow().to_string());
    print_kv("Aggressive (1.5x ATR)", &format!("{}{:.2} ({:.2}% risk)", csym, aggressive, ((entry_price - aggressive) / entry_price) * 100.0).red().to_string());

    // Percentage-based
    print_section("Percentage-Based");
    for pct in [2.0, 3.0, 5.0, 8.0] {
        let stop = entry_price * (1.0 - pct / 100.0);
        print_kv(&format!("{}% Stop", pct), &format!("{}{:.2}", csym, stop));
    }

    // Chandelier exit
    if let Some(chandelier) = technical::chandelier_exit(&highs, atr, 3.0) {
        print_section("Chandelier Exit (3x ATR)");
        print_kv("Trailing Stop", &format!("{}{:.2}", csym, chandelier));
    }

    // Target prices (reward ratios)
    print_section("Target Prices (from entry)");
    let risk = entry_price - moderate;
    for rr in [1.5, 2.0, 3.0] {
        let target = entry_price + risk * rr;
        print_kv(&format!("R:R 1:{:.1}", rr), &format!("{}{:.2} (+{:.2}%)", csym, target, ((target - entry_price) / entry_price) * 100.0).green().to_string());
    }
    println!();
    Ok(())
}

// ── 5. Peers Comparison ──

pub async fn cmd_peers(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let sector = q.sector.as_deref().unwrap_or("Unknown");

    // Get all popular stocks and filter by same sector
    let all_symbols = match market { Market::Us => market::US_POPULAR, Market::In => market::INDIA_POPULAR };
    let all_refs: Vec<&str> = all_symbols.to_vec();
    let all_quotes = client.get_quote(&all_refs).await?;
    let mut peers: Vec<&crate::api::Quote> = all_quotes.iter().filter(|p| p.sector.as_deref() == Some(sector) && p.symbol.as_deref() != Some(&resolved)).collect();
    peers.sort_by(|a, b| b.market_cap.unwrap_or(0.0).partial_cmp(&a.market_cap.unwrap_or(0.0)).unwrap());

    print_header(&format!("Peer Comparison: {} ({})", resolved, sector));

    // Include the target stock first
    let mut all_peers = vec![q];
    all_peers.extend(peers.iter().take(8).copied());

    println!("  {:<14} {:>10} {:>8} {:>8} {:>8} {:>10}", "Symbol".bold(), "Price".bold(), "P/E".bold(), "P/B".bold(), "Chg%".bold(), "Mkt Cap".bold());
    println!("  {}", "─".repeat(62).dimmed());

    for p in &all_peers {
        let sym = p.symbol.as_deref().unwrap_or("???");
        let cur = p.currency.as_deref();
        let is_target = sym == resolved;
        let name = if is_target { format!("{} ←", sym).cyan().bold().to_string() } else { sym.cyan().to_string() };
        let pct = p.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 { format!("+{:.2}%", pct).green().to_string() } else { format!("{:.2}%", pct).red().to_string() };
        println!("  {:<14} {:>10} {:>8} {:>8} {:>8} {:>10}", name,
            format_price(p.regular_market_price.unwrap_or(0.0), cur),
            p.trailing_pe.map_or("—".into(), |v| format!("{:.1}", v)),
            p.price_to_book.map_or("—".into(), |v| format!("{:.1}", v)),
            pct_str,
            p.market_cap.map_or("—".into(), |v| format_large_number(v, cur)));
    }
    if peers.is_empty() { println!("\n  {}", "No peers found in the same sector.".dimmed()); }
    println!();
    Ok(())
}

// ── 6. Dividends ──

pub async fn cmd_dividends(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);

    print_header(&format!("Dividend Analysis: {}", resolved));
    let price = q.regular_market_price.unwrap_or(0.0);
    print_kv("Price", &format_price(price, cur));

    let div_yield = q.trailing_annual_dividend_yield.unwrap_or(0.0);
    let div_rate = q.trailing_annual_dividend_yield.unwrap_or(0.0) * price;
    if div_yield > 0.0 {
        print_kv("Annual Dividend", &format!("{}{:.2}/share", csym, div_rate));
        print_kv("Dividend Yield", &format!("{:.2}%", div_yield * 100.0).green().to_string());

        // Dividend income projection
        print_section("Income Projection");
        for investment in [100000.0, 500000.0, 1000000.0] {
            let shares = (investment / price).floor();
            let annual_income = shares * div_rate;
            let monthly = annual_income / 12.0;
            print_kv(&format!("{}{:.0} invested", csym, investment), &format!("{}{:.0}/year ({}{:.0}/month)", csym, annual_income, csym, monthly));
        }

        // DRIP projection (dividend reinvestment)
        print_section("DRIP Growth (10 years)");
        let mut shares = 1000.0 / price * price; // normalized to 1000 units of currency
        let initial_shares = shares;
        for year in 1..=10 {
            let div_income = shares * div_rate;
            let new_shares = div_income / price;
            shares += new_shares;
            if year == 1 || year == 5 || year == 10 {
                let growth = ((shares / initial_shares) - 1.0) * 100.0;
                print_kv(&format!("Year {}", year), &format!("{:.2} shares (+{:.1}% from DRIP)", shares, growth));
            }
        }
    } else {
        println!("  {}", "This stock does not pay dividends.".dimmed());
    }
    println!();
    Ok(())
}

// ── 7. Insider Activity (via Yahoo search news) ──

pub async fn cmd_insider(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let name = q.short_name.as_deref().or(q.long_name.as_deref()).unwrap_or("Unknown");

    // Yahoo doesn't have a free insider endpoint, but we can show key holder metrics
    print_header(&format!("Insider & Institutional: {}", resolved));
    print_kv("Company", name);
    print_kv("Price", &format_price(q.regular_market_price.unwrap_or(0.0), q.currency.as_deref()));

    // Show what we have from the quote
    if let Some(rec) = q.recommendation_mean { print_kv("Analyst Consensus", &sentiment_label(rec)); }
    if let Some(n) = q.number_of_analyst_opinions { print_kv("# Analysts", &n.to_string()); }
    if let Some(target) = q.target_mean_price {
        let upside = q.regular_market_price.map(|p| ((target - p) / p) * 100.0).unwrap_or(0.0);
        print_kv("Target Price", &format!("{}{:.2} ({:+.1}%)", market::currency_symbol(q.currency.as_deref()), target, upside));
    }

    // Fetch insider-related news
    let search_url = format!("https://query2.finance.yahoo.com/v1/finance/search?q={} insider&quotesCount=0&newsCount=8", resolved);
    if let Ok(resp) = client.raw_get(&search_url).await {
        if let Some(news) = resp.get("news").and_then(|n| n.as_array()) {
            if !news.is_empty() {
                print_section("Recent Insider News");
                for item in news.iter().take(5) {
                    let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
                    let publisher = item.get("publisher").and_then(|p| p.as_str()).unwrap_or("");
                    if !title.is_empty() {
                        println!("  {} {}", "●".cyan(), title.bold());
                        println!("    {} {}", "└".dimmed(), publisher.dimmed());
                    }
                }
            }
        }
    }
    println!();
    Ok(())
}

// ── 8. IPO Calendar ──

pub async fn cmd_ipo() -> Result<()> {
    let client = YahooClient::new().await?;
    let search_url = "https://query2.finance.yahoo.com/v1/finance/search?q=IPO&quotesCount=0&newsCount=15";
    let resp = client.raw_get(search_url).await.context("Failed to fetch IPO news")?;

    print_header("IPO Calendar & News");

    if let Some(news) = resp.get("news").and_then(|n| n.as_array()) {
        if news.is_empty() { println!("  {}", "No IPO news found.".dimmed()); }
        for item in news.iter().take(12) {
            let title = item.get("title").and_then(|t| t.as_str()).unwrap_or("");
            let publisher = item.get("publisher").and_then(|p| p.as_str()).unwrap_or("");
            let ts = item.get("providerPublishTime").and_then(|t| t.as_i64())
                .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
                .map(|d| d.format("%Y-%m-%d").to_string()).unwrap_or_default();
            if !title.is_empty() {
                println!();
                println!("  {} {}", "●".cyan(), title.bold());
                println!("    {} {} · {}", "└".dimmed(), publisher.dimmed(), ts.dimmed());
            }
        }
    }
    println!();
    Ok(())
}

// ── 9. Options Overview (Put/Call data from quote) ──

pub async fn cmd_options(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let price = q.regular_market_price.unwrap_or(0.0);

    print_header(&format!("Options Overview: {}", resolved));
    print_kv("Spot Price", &format_price(price, cur));

    // Use beta and volatility as implied vol proxy
    if let Some(beta) = q.beta { print_kv("Beta", &format!("{:.2}", beta)); }

    // Fetch 1mo data for realized vol
    let chart = client.get_chart(&resolved, "1mo", "1d").await?;
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    if let Some(vol) = technical::annualized_volatility(&closes) {
        print_kv("Realized Vol (1M)", &format!("{:.1}%", vol * 100.0));
        // Estimate option prices using simplified Black-Scholes-ish approach
        let daily_move = vol / (252.0_f64).sqrt();
        print_section("Expected Moves");
        for days in [1, 5, 10, 30] {
            let move_pct = daily_move * (days as f64).sqrt() * price;
            print_kv(&format!("{}-day move", days), &format!("+/- {}{:.2} ({:.2}%)", csym, move_pct, (move_pct / price) * 100.0));
        }

        print_section("Suggested Strike Prices");
        let otm_pct = [2.0, 5.0, 10.0];
        println!("  {:<10} {:>14} {:>14}", "Distance".bold(), "Call Strike".bold(), "Put Strike".bold());
        println!("  {}", "─".repeat(40).dimmed());
        for pct in otm_pct {
            let call = price * (1.0 + pct / 100.0);
            let put = price * (1.0 - pct / 100.0);
            println!("  {:<10} {:>14} {:>14}", format!("{}% OTM", pct), format!("{}{:.2}", csym, call).green(), format!("{}{:.2}", csym, put).red());
        }
    }
    println!("\n  {}", "For full options chain, check your broker platform.".dimmed().italic());
    println!();
    Ok(())
}

// ── 10. Fibonacci Levels (standalone) ──

pub async fn cmd_fibs(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, "6mo", "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);
    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|q| q.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    if closes.len() < 20 { println!("{}", "Not enough data.".red()); return Ok(()); }

    let current = *closes.last().unwrap();

    // Find swing points from different lookbacks
    print_header(&format!("Fibonacci Retracement: {}", resolved));
    println!("  Current: {}\n", format_price(current, cur).bold());

    for (lookback, label) in [(30, "1 Month"), (60, "3 Month"), (120, "6 Month")] {
        let lb = lookback.min(highs.len());
        if let Some((sh, sl)) = technical::find_swing_points(&highs, &lows, lb) {
            let fibs = technical::fibonacci_levels(sh, sl);
            print_section(&format!("{} Swing ({}{:.2} → {}{:.2})", label, csym, sh, csym, sl));
            let levels = [("0.0% (High)", sh), ("23.6%", fibs[0]), ("38.2%", fibs[1]), ("50.0%", fibs[2]), ("61.8%", fibs[3]), ("78.6%", fibs[4]), ("100% (Low)", sl)];
            for (name, val) in &levels {
                let marker = if (current - val).abs() / current < 0.01 { " ← YOU ARE HERE".yellow().bold().to_string() } else { String::new() };
                print_kv(name, &format!("{}{:.2}{}", csym, val, marker));
            }
        }
    }
    println!();
    Ok(())
}
// ══════════════════════════════════════════════════════════
// PAPER TRADING SIMULATOR
// ══════════════════════════════════════════════════════════

pub async fn cmd_sim(action: &str, amount: Option<f64>, target: Option<f64>, market: Market) -> Result<()> {
    let csym = match market { Market::In => "₹", Market::Us => "$" };

    match action {
        "start" => {
            let capital = amount.unwrap_or(25000.0);
            let target_pct = target.unwrap_or(2.0);
            let today = chrono::Local::now().format("%Y-%m-%d").to_string();

            let mut history = SimHistory::load()?;
            if history.sessions.iter().any(|s| s.date == today && !s.settled) {
                println!("  {} Already have an open simulation for today.", "!".yellow());
                println!("  Run {} to settle it, or {} to check status.", "stockwise sim settle".cyan(), "stockwise sim status".cyan());
                return Ok(());
            }

            print_header("Paper Trading Simulation — Starting");
            println!("  Date: {}  |  Capital: {}{:.0}  |  Target: {}%\n", today.bold(), csym, capital, target_pct);
            println!("  {} Running intraday bot scan...\n", "⟳".yellow());

            let client = YahooClient::new().await?;
            let signals = intraday::scan_intraday(&client, market).await?;
            let plans = intraday::generate_trade_plans(&signals, capital, target_pct, 1.0);

            if plans.is_empty() {
                println!("  {} No trades qualified today. Try tomorrow.", "→".yellow());
                return Ok(());
            }

            let sim_trades: Vec<SimTrade> = plans.iter().take(5).map(|p| {
                SimTrade {
                    symbol: p.signal.symbol.clone(), direction: p.signal.direction.to_string(),
                    entry_price: p.entry, target1: p.target1, target2: p.target2, stop_loss: p.stop_loss,
                    qty: p.qty, capital: p.capital_required, score: p.signal.score,
                    confidence: p.signal.confidence.to_string(),
                    strategies: p.signal.strategies.iter().map(|s| s.name.to_string()).collect(),
                    exit_price: None, pnl: None, pnl_pct: None, hit_target: None, hit_stop: None,
                }
            }).collect();

            println!("  {} Simulation trades locked in:\n", "✓".green().bold());
            println!("  {:<14} {:>6} {:>10} {:>10} {:>10} {:>6}", "Symbol".bold(), "Qty".bold(), "Entry".bold(), "Target".bold(), "Stop".bold(), "Score".bold());
            println!("  {}", "─".repeat(60).dimmed());
            for t in &sim_trades {
                println!("  {:<14} {:>6} {:>10} {:>10} {:>10} {:>6}", t.symbol.cyan(), t.qty,
                    format!("{}{:.2}", csym, t.entry_price), format!("{}{:.2}", csym, t.target2).green(),
                    format!("{}{:.2}", csym, t.stop_loss).red(), format!("{:.0}", t.score));
            }

            history.sessions.push(SimSession {
                date: today, market: match market { Market::In => "IN", Market::Us => "US" }.into(),
                capital, target_pct, trades: sim_trades,
                total_pnl: None, total_pnl_pct: None, win_count: None, loss_count: None, settled: false,
            });
            history.save()?;

            println!("\n  {} Trades recorded. Run {} at EOD to see results.", "✓".green(), "stockwise sim settle".cyan());
            println!("  {} Run {} anytime to check live P&L.", "→".dimmed(), "stockwise sim status".cyan());
            println!();
        }
        "status" => {
            let history = SimHistory::load()?;
            let open: Vec<&SimSession> = history.sessions.iter().filter(|s| !s.settled).collect();
            if open.is_empty() { println!("\n  {} No open simulations. Run {}.", "→".dimmed(), "stockwise sim start".cyan()); return Ok(()); }

            let client = YahooClient::new().await?;
            for session in &open {
                print_header(&format!("Simulation Status — {}", session.date));
                println!("  Capital: {}{:.0}  |  Target: {}%\n", csym, session.capital, session.target_pct);
                let syms: Vec<&str> = session.trades.iter().map(|t| t.symbol.as_str()).collect();
                let quotes = client.get_quote(&syms).await?;

                println!("  {:<14} {:>6} {:>10} {:>10} {:>12} {:>10}", "Symbol".bold(), "Qty".bold(), "Entry".bold(), "Now".bold(), "P&L".bold(), "Status".bold());
                println!("  {}", "─".repeat(66).dimmed());

                let mut total_pnl = 0.0_f64;
                for t in &session.trades {
                    let current = quotes.iter().find(|q| q.symbol.as_deref() == Some(&t.symbol)).and_then(|q| q.regular_market_price).unwrap_or(t.entry_price);
                    let pnl = (current - t.entry_price) * t.qty as f64;
                    total_pnl += pnl;
                    let status = if current >= t.target2 { "TARGET HIT".green().bold().to_string() } else if current <= t.stop_loss { "STOPPED".red().bold().to_string() } else if current >= t.target1 { "T1 hit".green().to_string() } else { "Open".yellow().to_string() };
                    let pnl_str = if pnl >= 0.0 { format!("+{}{:.0}", csym, pnl).green().to_string() } else { format!("-{}{:.0}", csym, pnl.abs()).red().to_string() };
                    println!("  {:<14} {:>6} {:>10} {:>10} {:>12} {:>10}", t.symbol.cyan(), t.qty, format!("{}{:.2}", csym, t.entry_price), format!("{}{:.2}", csym, current), pnl_str, status);
                }
                println!("  {}", "─".repeat(66).dimmed());
                let total_str = if total_pnl >= 0.0 { format!("+{}{:.0}", csym, total_pnl).green().bold().to_string() } else { format!("-{}{:.0}", csym, total_pnl.abs()).red().bold().to_string() };
                let target_profit = session.capital * (session.target_pct / 100.0);
                let on_track = if total_pnl >= target_profit { "TARGET MET".green().bold().to_string() } else { format!("{}{:.0} to go", csym, target_profit - total_pnl).yellow().to_string() };
                println!("  Total: {}  |  {}", total_str, on_track);
            }
            println!();
        }
        "settle" => {
            let mut history = SimHistory::load()?;
            let client = YahooClient::new().await?;
            let mut settled_any = false;

            for session in history.sessions.iter_mut().filter(|s| !s.settled) {
                let syms: Vec<&str> = session.trades.iter().map(|t| t.symbol.as_str()).collect();
                let quotes = client.get_quote(&syms).await?;
                let mut total_pnl = 0.0_f64;
                let (mut wins, mut losses) = (0u32, 0u32);

                print_header(&format!("Settling — {}", session.date));
                println!("  {:<14} {:>10} {:>10} {:>12} {:>8}", "Symbol".bold(), "Entry".bold(), "Close".bold(), "P&L".bold(), "Result".bold());
                println!("  {}", "─".repeat(58).dimmed());

                for trade in session.trades.iter_mut() {
                    let current = quotes.iter().find(|q| q.symbol.as_deref() == Some(&trade.symbol)).and_then(|q| q.regular_market_price).unwrap_or(trade.entry_price);
                    let pnl = (current - trade.entry_price) * trade.qty as f64;
                    let pnl_pct = ((current / trade.entry_price) - 1.0) * 100.0;
                    trade.exit_price = Some(current); trade.pnl = Some(pnl); trade.pnl_pct = Some(pnl_pct);
                    trade.hit_target = Some(current >= trade.target1); trade.hit_stop = Some(current <= trade.stop_loss);
                    total_pnl += pnl;
                    if pnl > 0.0 { wins += 1; } else { losses += 1; }
                    let result = if current >= trade.target1 { "WIN".green().bold().to_string() } else if current <= trade.stop_loss { "STOPPED".red().to_string() } else if pnl > 0.0 { "Profit".green().to_string() } else { "Loss".red().to_string() };
                    let pnl_str = if pnl >= 0.0 { format!("+{}{:.0}", csym, pnl).green().to_string() } else { format!("-{}{:.0}", csym, pnl.abs()).red().to_string() };
                    println!("  {:<14} {:>10} {:>10} {:>12} {:>8}", trade.symbol.cyan(), format!("{}{:.2}", csym, trade.entry_price), format!("{}{:.2}", csym, current), pnl_str, result);
                }

                let total_pnl_pct = (total_pnl / session.capital) * 100.0;
                session.total_pnl = Some(total_pnl); session.total_pnl_pct = Some(total_pnl_pct);
                session.win_count = Some(wins); session.loss_count = Some(losses); session.settled = true;
                settled_any = true;

                println!("  {}", "─".repeat(58).dimmed());
                let verdict = if total_pnl >= session.capital * (session.target_pct / 100.0) {
                    format!("TARGET MET — {}{:.0} ({:+.2}%)", csym, total_pnl, total_pnl_pct).green().bold().to_string()
                } else if total_pnl > 0.0 {
                    format!("Partial win — {}{:.0} ({:+.2}%)", csym, total_pnl, total_pnl_pct).yellow().to_string()
                } else { format!("Loss — {}{:.0} ({:.2}%)", csym, total_pnl.abs(), total_pnl_pct).red().to_string() };
                println!("  {} {}", "→".bold(), verdict);
            }
            if !settled_any { println!("\n  {} No open simulations.", "→".dimmed()); }
            else { history.save()?; println!("\n  {} Run {} for stats.", "→".dimmed(), "stockwise sim history".cyan()); }
            println!();
        }
        "history" | "stats" => {
            let history = SimHistory::load()?;
            let stats = history.stats();
            if stats.total_days == 0 { println!("\n  {} No history. Run {}.", "→".dimmed(), "stockwise sim start".cyan()); return Ok(()); }

            print_header("Paper Trading Performance");
            let daily_pnls: Vec<f64> = history.sessions.iter().filter(|s| s.settled).filter_map(|s| s.total_pnl).collect();
            if daily_pnls.len() > 1 {
                let mut equity = vec![0.0_f64];
                for &pnl in &daily_pnls { equity.push(equity.last().unwrap() + pnl); }
                let color = if *equity.last().unwrap() >= 0.0 { "green" } else { "red" };
                for line in charts::line_chart(&equity, 50, 8, color, "Cumulative P&L") { println!("{}", line); }
                println!();
                println!("  {}", "Daily P&L".bold());
                for s in history.sessions.iter().filter(|s| s.settled) {
                    let pnl = s.total_pnl.unwrap_or(0.0);
                    let bar_len = (pnl.abs() / stats.best_day.unwrap_or(1.0).abs().max(stats.worst_day.unwrap_or(1.0).abs()) * 20.0).min(20.0) as usize;
                    let bar = if pnl >= 0.0 { "█".repeat(bar_len.max(1)).green().to_string() } else { "█".repeat(bar_len.max(1)).red().to_string() };
                    let ps = if pnl >= 0.0 { format!("+{}{:.0}", csym, pnl).green().to_string() } else { format!("-{}{:.0}", csym, pnl.abs()).red().to_string() };
                    println!("  {} {} {}", s.date.dimmed(), bar, ps);
                }
            }

            print_section("Performance");
            print_kv("Days Simulated", &stats.total_days.to_string());
            print_kv("Winning Days", &format!("{} ({:.0}%)", stats.winning_days, stats.day_win_rate));
            let ts = if stats.total_pnl >= 0.0 { format!("+{}{:.0}", csym, stats.total_pnl).green().bold().to_string() } else { format!("-{}{:.0}", csym, stats.total_pnl.abs()).red().bold().to_string() };
            print_kv("Total P&L", &ts);
            print_kv("Avg Daily P&L", &format!("{}{:.0}", csym, stats.avg_daily_pnl));
            if let Some(b) = stats.best_day { print_kv("Best Day", &format!("+{}{:.0}", csym, b).green().to_string()); }
            if let Some(w) = stats.worst_day { print_kv("Worst Day", &format!("{}{:.0}", csym, w).red().to_string()); }
            print_kv("Total Trades", &stats.total_trades.to_string());
            print_kv("Trade Win Rate", &format!("{:.0}%", stats.trade_win_rate));
            print_kv("Max Win Streak", &format!("{} days", stats.max_win_streak));
            print_kv("Max Loss Streak", &format!("{} days", stats.max_loss_streak));
            if let Some(s) = stats.sharpe { print_kv("Sharpe (ann.)", &format!("{:.2}", s)); }

            print_section("Bot Confidence");
            if stats.total_days >= 5 {
                let conf = if stats.day_win_rate >= 65.0 && stats.trade_win_rate >= 55.0 && stats.total_pnl > 0.0 {
                    "HIGH — Bot is consistently profitable. Consider going live.".green().bold().to_string()
                } else if stats.day_win_rate >= 50.0 && stats.total_pnl > 0.0 {
                    "MODERATE — Profitable but inconsistent. Keep testing.".yellow().to_string()
                } else { "LOW — Underperforming. Review strategy.".red().to_string() };
                println!("  {}", conf);
            } else { println!("  {} Need 5+ days for confidence rating ({}/5)", "→".dimmed(), stats.total_days); }
            println!();
        }
        "reset" => {
            SimHistory::default().save()?;
            println!("  {} Simulation history cleared.", "✓".green());
            println!();
        }
        _ => {
            println!();
            println!("  {} Paper Trading Simulator:", "→".cyan());
            println!();
            println!("    {} — Lock in today's bot trades", "stockwise sim start [AMOUNT] [TARGET%]".bold());
            println!("    {} — Check live P&L", "stockwise sim status".bold());
            println!("    {} — Settle at end-of-day", "stockwise sim settle".bold());
            println!("    {} — View cumulative stats + confidence", "stockwise sim history".bold());
            println!("    {} — Clear all data", "stockwise sim reset".bold());
            println!();
            println!("  {} Morning: {} → Track: {} → EOD: {}", "Workflow:".dimmed(), "sim start".cyan(), "sim status".cyan(), "sim settle".cyan());
            println!();
        }
    }
    Ok(())
}

// ── Deep Dive: Everything about a stock ──

pub async fn cmd_deep(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let cur_ref = &resolved;

    // ── 1. Quote + Fundamentals ──
    let quotes = client.get_quote(&[cur_ref.as_str()]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let name = q.long_name.as_deref().or(q.short_name.as_deref()).unwrap_or("Unknown");
    let price = q.regular_market_price.unwrap_or(0.0);
    let change = q.regular_market_change.unwrap_or(0.0);
    let change_pct = q.regular_market_change_percent.unwrap_or(0.0);

    print_header(&format!("DEEP DIVE: {} — {}", resolved, name));
    println!("  {}  {}\n", format_price(price, cur).bold(), format_change(change, change_pct));

    // Overview
    print_section("Company");
    if let Some(s) = &q.sector { print_kv("Sector", s); }
    if let Some(i) = &q.industry { print_kv("Industry", i); }
    if let Some(e) = &q.exchange { print_kv("Exchange", e); }
    print_kv("Market Cap", &q.market_cap.map_or("N/A".into(), |v| format_large_number(v, cur)));

    // Price
    print_section("Price Action");
    print_kv("Open", &format_price(q.regular_market_open.unwrap_or(0.0), cur));
    print_kv("Day Range", &format!("{} — {}", format_price(q.regular_market_day_low.unwrap_or(0.0), cur), format_price(q.regular_market_day_high.unwrap_or(0.0), cur)));
    print_kv("52-Week Range", &format!("{} — {}", format_price(q.fifty_two_week_low.unwrap_or(0.0), cur), format_price(q.fifty_two_week_high.unwrap_or(0.0), cur)));
    print_kv("Volume", &format_volume(q.regular_market_volume.unwrap_or(0)));
    print_kv("Avg Vol (3M)", &format_volume(q.average_daily_volume_3_month.unwrap_or(0)));

    // Valuation
    print_section("Valuation");
    print_kv("P/E (TTM)", &format_optional_f64(q.trailing_pe, "x"));
    print_kv("P/E (Forward)", &format_optional_f64(q.forward_pe, "x"));
    print_kv("P/B", &format_optional_f64(q.price_to_book, "x"));
    print_kv("EV/Revenue", &format_optional_f64(q.enterprise_to_revenue, "x"));
    print_kv("EV/EBITDA", &format_optional_f64(q.enterprise_to_ebitda, "x"));
    if let Some(pe) = q.trailing_pe {
        if let Some(g) = q.earnings_quarterly_growth {
            if g > 0.01 { print_kv("PEG Ratio", &format!("{:.2}", pe / (g * 100.0))); }
        }
    }

    // Profitability
    print_section("Profitability & Growth");
    print_kv("EPS (TTM)", &format_optional_f64(q.eps_trailing_twelve_months, ""));
    print_kv("EPS (Forward)", &format_optional_f64(q.eps_forward, ""));
    print_kv("Profit Margin", &format_optional_pct(q.profit_margins));
    print_kv("Return on Equity", &format_optional_pct(q.return_on_equity));
    print_kv("Revenue Growth", &format_optional_pct(q.revenue_growth));
    print_kv("Earnings Growth (Q)", &format_optional_pct(q.earnings_quarterly_growth));

    // Financial Health
    print_section("Financial Health");
    print_kv("Debt/Equity", &format_optional_f64(q.debt_to_equity, ""));
    print_kv("Current Ratio", &format_optional_f64(q.current_ratio, "x"));
    print_kv("Book Value", &format_optional_f64(q.book_value, ""));
    print_kv("Beta", &format_optional_f64(q.beta, ""));

    // Dividends
    if let Some(dy) = q.trailing_annual_dividend_yield {
        if dy > 0.0 {
            print_section("Dividend");
            print_kv("Yield", &format!("{:.2}%", dy * 100.0));
            let annual = dy * price;
            print_kv("Annual/Share", &format!("{}{:.2}", csym, annual));
            print_kv("Income on {}1L", &format!("{}{:.0}/year", csym, 100000.0 * dy));
        }
    }

    // Analyst
    print_section("Analyst Ratings");
    if let Some(rec) = q.recommendation_mean {
        print_kv("Consensus", &sentiment_label(rec));
        print_kv("Score", &format!("{:.1}/5  {}", rec, rating_bar(5.0 - rec, 4.0)));
    }
    if let Some(target) = q.target_mean_price {
        let upside = q.regular_market_price.map(|p| ((target - p) / p) * 100.0).unwrap_or(0.0);
        let u = if upside >= 0.0 { format!("+{:.1}%", upside).green().to_string() } else { format!("{:.1}%", upside).red().to_string() };
        print_kv("Price Target", &format!("{}{:.2} ({})", csym, target, u));
    }
    if let Some(n) = q.number_of_analyst_opinions { print_kv("# Analysts", &n.to_string()); }

    // ── 2. Technical Indicators ──
    let chart = client.get_chart(&resolved, "1y", "1d").await?;
    let closes: Vec<f64> = chart.indicators.quote.first().and_then(|qi| qi.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let highs: Vec<f64> = chart.indicators.quote.first().and_then(|qi| qi.high.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let lows: Vec<f64> = chart.indicators.quote.first().and_then(|qi| qi.low.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
    let volumes: Vec<u64> = chart.indicators.quote.first().and_then(|qi| qi.volume.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();

    if closes.len() >= 20 {
        // Chart
        print_section("1Y Price Chart");
        let color = if *closes.last().unwrap() >= closes[0] { "green" } else { "red" };
        for line in charts::line_chart(&closes, 55, 8, color, "") { println!("{}", line); }

        // Key technicals
        print_section("Technical Indicators");
        if let Some(rsi) = technical::rsi(&closes, 14) {
            let r = if rsi >= 70.0 { format!("{:.1}", rsi).red().to_string() } else if rsi <= 30.0 { format!("{:.1}", rsi).green().to_string() } else { format!("{:.1}", rsi).to_string() };
            print_kv("RSI (14)", &r);
            println!("{}", charts::rsi_gauge(rsi));
        }
        for period in [20, 50, 200] {
            if let Some(ma) = technical::sma(&closes, period) {
                let sig = if price > ma { "▲ Above".green().to_string() } else { "▼ Below".red().to_string() };
                print_kv(&format!("SMA {}", period), &format!("{}{:.2}  {}", csym, ma, sig));
            }
        }
        if let Some((_, _, hist)) = technical::macd(&closes) {
            let h = if hist > 0.0 { format!("{:.2} Bullish", hist).green().to_string() } else { format!("{:.2} Bearish", hist).red().to_string() };
            print_kv("MACD Histogram", &h);
        }
        if let Some((upper, middle, lower)) = technical::bollinger_bands(&closes, 20) {
            print_kv("Bollinger", &format!("{}{:.2} / {}{:.2} / {}{:.2}", csym, lower, csym, middle, csym, upper));
        }
        if let Some(atr) = technical::atr(&highs, &lows, &closes, 14) {
            print_kv("ATR (14)", &format!("{}{:.2} ({:.2}%)", csym, atr, (atr / price) * 100.0));
        }
        if let Some(vwap) = technical::vwap(&highs, &lows, &closes, &volumes) {
            let sig = if price > vwap { "Above".green().to_string() } else { "Below".red().to_string() };
            print_kv("VWAP", &format!("{}{:.2} ({})", csym, vwap, sig));
        }

        // Support/Resistance
        let n = highs.len();
        if n > 1 {
            let (pivot, r1, r2, _r3, s1, s2, _s3) = technical::pivot_points(highs[n-1], lows[n-1], closes[n-1]);
            print_section("Support & Resistance");
            print_kv("R2", &format!("{}{:.2}", csym, r2).red().to_string());
            print_kv("R1", &format!("{}{:.2}", csym, r1).red().to_string());
            print_kv("Pivot", &format!("{}{:.2}", csym, pivot).bold().to_string());
            print_kv("S1", &format!("{}{:.2}", csym, s1).green().to_string());
            print_kv("S2", &format!("{}{:.2}", csym, s2).green().to_string());
        }

        // Fibonacci
        if let Some((sh, sl)) = technical::find_swing_points(&highs, &lows, 60.min(n)) {
            let fibs = technical::fibonacci_levels(sh, sl);
            print_section("Fibonacci (3M swing)");
            for (name, val) in [("23.6%", fibs[0]), ("38.2%", fibs[1]), ("50.0%", fibs[2]), ("61.8%", fibs[3])] {
                let marker = if (price - val).abs() / price < 0.01 { " ← HERE".yellow().bold().to_string() } else { String::new() };
                print_kv(name, &format!("{}{:.2}{}", csym, val, marker));
            }
        }

        // Risk
        print_section("Risk Profile");
        if let Some(vol) = technical::annualized_volatility(&closes) {
            print_kv("Annualized Vol", &format!("{:.1}%", vol * 100.0));
        }
        if let Some(sharpe) = technical::sharpe_ratio(&closes, 0.05) {
            print_kv("Sharpe Ratio", &format!("{:.2}", sharpe));
        }
        if let Some((mdd, _, _)) = technical::max_drawdown(&closes) {
            print_kv("Max Drawdown", &format!("-{:.1}%", mdd * 100.0).red().to_string());
        }
        if let Some(var95) = technical::value_at_risk(&closes, 0.95) {
            print_kv("Daily VaR (95%)", &format!("{:.2}%", var95 * 100.0));
        }

        // Volume
        let obv = technical::obv(&closes, &volumes);
        if obv.len() > 20 {
            let obv_trend = if obv.last() > obv.get(obv.len().saturating_sub(20)) { "Accumulation".green().to_string() } else { "Distribution".red().to_string() };
            print_kv("OBV Trend", &obv_trend);
        }

        // Gaps
        let opens: Vec<f64> = chart.indicators.quote.first().and_then(|qi| qi.open.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect()).unwrap_or_default();
        let gaps = technical::detect_gaps(&opens, &highs, &lows, &closes);
        let unfilled: Vec<_> = gaps.iter().filter(|g| !g.filled).collect();
        if !unfilled.is_empty() {
            print_section(&format!("Open Gaps ({})", unfilled.len()));
            for g in unfilled.iter().rev().take(3) {
                let dir = match g.gap_type { technical::GapType::Up => "UP".green().to_string(), technical::GapType::Down => "DN".red().to_string() };
                println!("  {} {}{:.2} — {}{:.2}", dir, csym, g.gap_low, csym, g.gap_high);
            }
        }
    }

    // ── 3. Rule-based insights ──
    print_section("Insights");
    for insight in insights::generate_insights(q) {
        println!("  {} {}", "→".cyan(), insight);
    }
    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!("{}", insights::overall_verdict(q));
    println!("{}", "─".repeat(60).dimmed());

    // ── 4. AI Analysis ──
    let ai = crate::ai::AiClient::new();
    if ai.is_available().await {
        print_section("AI Analysis (Ollama)");
        let stock_data = crate::ai::StockData::from_quote(q);
        match ai.analyze_stock(&stock_data).await {
            Ok(analysis) => { for line in analysis.lines() { println!("  {}", line); } }
            Err(_) => { println!("  {}", "AI unavailable.".dimmed()); }
        }
    }

    println!("\n  {}", "Not financial advice. Do your own research.".dimmed().italic());
    println!();
    Ok(())
}

// ══════════════════════════════════════════════════════════
// DAEMON MODE — Continuous Market Worker
// ══════════════════════════════════════════════════════════

pub async fn cmd_daemon(mode: &str, amount: Option<f64>, market: Market) -> Result<()> {
    let csym = match market { Market::In => "₹", Market::Us => "$" };

    match mode {
        "intraday" => {
            let capital = amount.unwrap_or(25000.0);
            let target_pct = 2.0;
            let mut risk = crate::daemon::RiskState::new(capital);
            let mut positions: Vec<crate::daemon::LivePosition> = Vec::new();
            let mut entered = false;
            let mut tick_count = 0u32;
            let mut last_phase = crate::daemon::Phase::Closed;

            print_header("Intraday Daemon — Starting");
            println!("  Capital: {}{:.0}  |  Target: {}%  |  Max Risk: {}% daily", csym, capital, target_pct, risk.daily_loss_limit_pct);
            println!("  Max {} positions  |  Paper trading mode", risk.max_positions);
            println!("  {}", "─".repeat(60).dimmed());
            println!("  {} Press Ctrl+C to stop. Positions will be squared off.\n", "→".dimmed());

            loop {
                let phase = crate::daemon::current_phase_india();

                // Phase transition announcements
                if phase != last_phase {
                    let phase_str = match phase {
                        crate::daemon::Phase::PreMarket => format!("PRE-MARKET — Scanning...").yellow().bold().to_string(),
                        crate::daemon::Phase::Opening => format!("MARKET OPEN — Entering positions").green().bold().to_string(),
                        crate::daemon::Phase::Active => format!("ACTIVE TRADING — Monitoring").cyan().bold().to_string(),
                        crate::daemon::Phase::WindDown => format!("WIND-DOWN — Tightening stops, no new entries").yellow().to_string(),
                        crate::daemon::Phase::SquareOff => format!("SQUARE OFF — Closing all positions").red().bold().to_string(),
                        crate::daemon::Phase::PostMarket => format!("POST-MARKET — Settling").dimmed().to_string(),
                        crate::daemon::Phase::Closed => format!("MARKET CLOSED").dimmed().to_string(),
                    };
                    println!("\n  {} [{}] {}", "▶".bold(), chrono::Local::now().format("%H:%M:%S"), phase_str);
                    last_phase = phase;
                }

                match phase {
                    crate::daemon::Phase::Closed => {
                        println!("  {} Market is closed. Daemon will wait for market hours.", "→".dimmed());
                        println!("  {} NSE: Mon-Fri 9:00 AM – 3:30 PM IST", "→".dimmed());
                        // Wait 5 minutes before checking again
                        tokio::time::sleep(std::time::Duration::from_secs(300)).await;
                        continue;
                    }

                    crate::daemon::Phase::PreMarket => {
                        if !entered {
                            println!("  {} Scanning sectors and candidates...", "⟳".yellow());
                            let client = YahooClient::new().await?;
                            // Sector heat
                            if let Ok(heats) = intraday::scan_sector_heat(&client).await {
                                let hot: Vec<_> = heats.iter().filter(|h| h.hot).collect();
                                if !hot.is_empty() {
                                    println!("  {} Hot sectors: {}", "🔥".to_string(), hot.iter().map(|h| format!("{} ({:+.1}%)", h.name, h.change_pct)).collect::<Vec<_>>().join(", "));
                                }
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
                    }

                    crate::daemon::Phase::Opening => {
                        if !entered && !risk.killed {
                            println!("  {} Running 9-strategy scan...", "⟳".yellow());
                            let client = YahooClient::new().await?;
                            let signals = intraday::scan_intraday(&client, market).await?;
                            let plans = intraday::generate_trade_plans(&signals, capital, target_pct, 1.0);

                            if !plans.is_empty() {
                                positions = crate::daemon::plans_to_positions(&plans[..plans.len().min(risk.max_positions)]);
                                entered = true;

                                println!("  {} {} positions entered:\n", "✓".green().bold(), positions.len());
                                for p in &positions {
                                    println!("    {} {} {} × {} @ {}{:.2}  T1:{}{:.2}  T2:{}{:.2}  SL:{}{:.2}",
                                        "→".green(), p.direction, p.symbol.cyan(), p.qty, csym, p.entry_price,
                                        csym, p.target1, csym, p.target2, csym, p.stop_loss);
                                }
                            } else {
                                println!("  {} No qualified trades. Watching...", "→".yellow());
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(15)).await;
                    }

                    crate::daemon::Phase::Active | crate::daemon::Phase::WindDown => {
                        if !positions.is_empty() {
                            // Fetch live prices
                            let client = YahooClient::new().await?;
                            let syms: Vec<&str> = positions.iter().map(|p| p.symbol.as_str()).collect();
                            let quotes = client.get_quote(&syms).await?;

                            // Update each position
                            let mut total_pnl = 0.0_f64;
                            let mut any_change = false;

                            for pos in positions.iter_mut() {
                                let old_status = pos.status;
                                if let Some(q) = quotes.iter().find(|q| q.symbol.as_deref() == Some(&pos.symbol)) {
                                    if let Some(price) = q.regular_market_price {
                                        crate::daemon::update_position(pos, price, phase);
                                    }
                                }
                                total_pnl += pos.pnl;

                                if pos.status != old_status {
                                    any_change = true;
                                    let alert = match pos.status {
                                        crate::daemon::PositionStatus::T1Hit => format!("T1 HIT — booked 50% of {} at {}{:.2}", pos.symbol, csym, pos.current_price).green().bold().to_string(),
                                        crate::daemon::PositionStatus::T2Hit => format!("TARGET HIT — {} fully closed at {}{:.2}", pos.symbol, csym, pos.current_price).green().bold().to_string(),
                                        crate::daemon::PositionStatus::StopHit => {
                                            risk.record_exit(pos.pnl);
                                            format!("STOPPED — {} at {}{:.2} (P&L: {}{:.0})", pos.symbol, csym, pos.current_price, csym, pos.pnl).red().bold().to_string()
                                        }
                                        crate::daemon::PositionStatus::SquaredOff => {
                                            risk.record_exit(pos.pnl);
                                            format!("SQUARED OFF — {} at {}{:.2}", pos.symbol, csym, pos.current_price).yellow().to_string()
                                        }
                                        _ => String::new(),
                                    };
                                    if !alert.is_empty() {
                                        println!("  {} [{}] {}", "⚡".to_string(), chrono::Local::now().format("%H:%M:%S"), alert);
                                    }
                                }
                            }

                            // Check daily loss limit
                            let unrealized: f64 = positions.iter().filter(|p| p.status == crate::daemon::PositionStatus::Open || p.status == crate::daemon::PositionStatus::T1Hit).map(|p| p.pnl).sum();
                            if risk.check_daily_limit(unrealized) {
                                println!("  {} [{}] {} Daily loss limit breached! Closing all positions.", "🛑".to_string(), chrono::Local::now().format("%H:%M:%S"), "KILL SWITCH".red().bold());
                                for pos in positions.iter_mut() {
                                    if pos.status == crate::daemon::PositionStatus::Open || pos.status == crate::daemon::PositionStatus::T1Hit {
                                        pos.status = crate::daemon::PositionStatus::SquaredOff;
                                        risk.record_exit(pos.pnl);
                                    }
                                }
                            }

                            // Status update every 5 ticks (~5 minutes)
                            tick_count += 1;
                            if tick_count % 5 == 0 || any_change {
                                let open_count = positions.iter().filter(|p| matches!(p.status, crate::daemon::PositionStatus::Open | crate::daemon::PositionStatus::T1Hit)).count();
                                let total_str = if total_pnl >= 0.0 { format!("+{}{:.0}", csym, total_pnl).green().to_string() } else { format!("-{}{:.0}", csym, total_pnl.abs()).red().to_string() };
                                println!("  {} [{}] {} | {} open | P&L: {} | Phase: {}",
                                    "●".dimmed(), chrono::Local::now().format("%H:%M:%S"),
                                    format!("Tick #{}", tick_count).dimmed(), open_count, total_str, phase);
                            }

                            // All positions closed? Stop monitoring
                            if positions.iter().all(|p| matches!(p.status, crate::daemon::PositionStatus::T2Hit | crate::daemon::PositionStatus::StopHit | crate::daemon::PositionStatus::SquaredOff)) {
                                println!("\n  {} All positions closed. Waiting for post-market.", "✓".green().bold());
                                // Skip to post-market wait
                                tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                                continue;
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }

                    crate::daemon::Phase::SquareOff => {
                        // Force close everything
                        for pos in positions.iter_mut() {
                            if pos.status == crate::daemon::PositionStatus::Open || pos.status == crate::daemon::PositionStatus::T1Hit {
                                pos.status = crate::daemon::PositionStatus::SquaredOff;
                                risk.record_exit(pos.pnl);
                                println!("  {} Squared off {} at {}{:.2}", "→".yellow(), pos.symbol.cyan(), csym, pos.current_price);
                            }
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }

                    crate::daemon::Phase::PostMarket => {
                        // Settlement
                        if entered {
                            println!("\n  {}", "─".repeat(60).dimmed());
                            print_header("End of Day Settlement");

                            let total_pnl: f64 = positions.iter().map(|p| p.pnl).sum();
                            let wins = positions.iter().filter(|p| p.pnl > 0.0).count();
                            let losses = positions.iter().filter(|p| p.pnl <= 0.0).count();

                            println!("  {:<14} {:>10} {:>10} {:>12} {:>10}", "Symbol".bold(), "Entry".bold(), "Exit".bold(), "P&L".bold(), "Status".bold());
                            println!("  {}", "─".repeat(60).dimmed());
                            for p in &positions {
                                let pnl_str = if p.pnl >= 0.0 { format!("+{}{:.0}", csym, p.pnl).green().to_string() } else { format!("-{}{:.0}", csym, p.pnl.abs()).red().to_string() };
                                println!("  {:<14} {:>10} {:>10} {:>12} {:>10}", p.symbol.cyan(), format!("{}{:.2}", csym, p.entry_price), format!("{}{:.2}", csym, p.current_price), pnl_str, p.status);
                            }
                            println!("  {}", "─".repeat(60).dimmed());
                            let total_str = if total_pnl >= 0.0 { format!("+{}{:.0}", csym, total_pnl).green().bold().to_string() } else { format!("-{}{:.0}", csym, total_pnl.abs()).red().bold().to_string() };
                            println!("  Total: {}  |  {} wins, {} losses", total_str, wins, losses);

                            // Save to sim history
                            let mut history = crate::simulator::SimHistory::load()?;
                            let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                            let trades: Vec<crate::simulator::SimTrade> = positions.iter().map(|p| {
                                crate::simulator::SimTrade {
                                    symbol: p.symbol.clone(), direction: p.direction.clone(),
                                    entry_price: p.entry_price, target1: p.target1, target2: p.target2,
                                    stop_loss: p.stop_loss, qty: p.qty, capital: p.entry_price * p.qty as f64,
                                    score: p.score, confidence: "DAEMON".into(),
                                    strategies: p.strategies.clone(),
                                    exit_price: Some(p.current_price), pnl: Some(p.pnl),
                                    pnl_pct: Some(p.pnl_pct), hit_target: Some(p.status == crate::daemon::PositionStatus::T2Hit),
                                    hit_stop: Some(p.status == crate::daemon::PositionStatus::StopHit),
                                }
                            }).collect();
                            let total_pnl_pct = (total_pnl / capital) * 100.0;
                            history.sessions.push(crate::simulator::SimSession {
                                date: today, market: "IN".into(), capital, target_pct,
                                trades, total_pnl: Some(total_pnl), total_pnl_pct: Some(total_pnl_pct),
                                win_count: Some(wins as u32), loss_count: Some(losses as u32), settled: true,
                            });
                            history.save()?;
                            println!("  {} Saved to simulation history.", "✓".green());

                            // AI report if available
                            let ai = crate::ai::AiClient::new();
                            if ai.is_available().await {
                                println!("  {} Generating AI EOD report...", "⟳".yellow());
                                let mut data = format!("Intraday results: P&L={}{:.0}, {} wins {} losses\n", csym, total_pnl, wins, losses);
                                for p in &positions {
                                    data.push_str(&format!("{}: entry={:.2}, exit={:.2}, pnl={:.0}, status={}\n", p.symbol, p.entry_price, p.current_price, p.pnl, p.status));
                                }
                                if let Ok(report) = ai.generate_intraday_report(&data).await {
                                    print_section("AI EOD Analysis");
                                    for line in report.lines() { println!("  {}", line); }
                                }
                            }

                            println!("\n  {} Daemon complete for today. Exiting.", "✓".green().bold());
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                }
            }
        }

        "longterm" => {
            print_header("Long-Term Daemon — Daily Run");
            println!("  {} Running post-market analysis...\n", "⟳".yellow());

            let client = YahooClient::new().await?;

            // 1. Score all stocks
            println!("  [1/5] Scoring stocks...");
            let symbols = match market { Market::In => market::INDIA_POPULAR, Market::Us => market::US_POPULAR };
            let sym_refs: Vec<&str> = symbols.to_vec();
            let quotes = client.get_quote(&sym_refs).await?;
            let mut scores: Vec<crate::longterm::LongTermScore> = Vec::new();
            for q in &quotes {
                let sym = q.symbol.as_deref().unwrap_or("");
                let hist = client.get_chart(sym, "1y", "1d").await.ok().and_then(|c| {
                    c.indicators.quote.first().and_then(|qi| qi.close.as_ref()).map(|c| c.iter().filter_map(|v| *v).collect::<Vec<f64>>())
                });
                scores.push(crate::longterm::score_for_longterm(q, hist.as_deref()));
            }
            scores.sort_by(|a, b| b.total_score.partial_cmp(&a.total_score).unwrap());
            println!("    Top 3: {}", scores.iter().take(3).map(|s| format!("{} ({:.0})", s.symbol, s.total_score)).collect::<Vec<_>>().join(", "));

            // 2. Check portfolio
            println!("  [2/5] Checking portfolio...");
            let portfolio = Portfolio::load()?;
            if !portfolio.holdings.is_empty() {
                let p_syms: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
                let p_refs: Vec<&str> = p_syms.iter().map(|s| s.as_str()).collect();
                let p_quotes = client.get_quote(&p_refs).await?;
                let mut total_value = 0.0_f64;
                let mut total_cost = 0.0_f64;
                for h in &portfolio.holdings {
                    let price = p_quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol)).and_then(|q| q.regular_market_price).unwrap_or(0.0);
                    total_value += h.shares * price;
                    total_cost += h.shares * h.avg_cost;
                }
                let pnl = total_value - total_cost;
                let pnl_str = if pnl >= 0.0 { format!("+{}{:.0}", csym, pnl).green().to_string() } else { format!("-{}{:.0}", csym, pnl.abs()).red().to_string() };
                println!("    Portfolio: {}{:.0} ({}) | {} holdings", csym, total_value, pnl_str, portfolio.holdings.len());

                // Wealth snapshot
                let mut wealth = crate::wealth::WealthHistory::load()?;
                wealth.add_snapshot(total_value, total_cost, portfolio.holdings.len());
                wealth.save()?;
            } else {
                println!("    No portfolio holdings.");
            }

            // 3. Check alerts
            println!("  [3/5] Checking alerts...");
            let store = crate::alerts::AlertStore::load()?;
            if !store.alerts.is_empty() {
                let a_syms: Vec<String> = store.alerts.iter().map(|a| a.symbol.clone()).collect();
                let mut unique: Vec<&str> = a_syms.iter().map(|s| s.as_str()).collect();
                unique.sort(); unique.dedup();
                let a_quotes = client.get_quote(&unique).await?;
                let mut triggered = 0;
                for alert in &store.alerts {
                    let price = a_quotes.iter().find(|q| q.symbol.as_deref() == Some(alert.symbol.as_str())).and_then(|q| q.regular_market_price).unwrap_or(0.0);
                    let hit = match alert.condition { crate::alerts::AlertCondition::Above => price >= alert.target, crate::alerts::AlertCondition::Below => price <= alert.target };
                    if hit { triggered += 1; println!("    {} {} {} {:.2} — TRIGGERED (now {}{:.2})", "⚠".yellow(), alert.symbol, alert.condition, alert.target, csym, price); }
                }
                if triggered == 0 { println!("    No alerts triggered."); }
            }

            // 4. Tax harvest check
            println!("  [4/5] Tax harvest scan...");
            if !portfolio.holdings.is_empty() {
                let p_syms: Vec<String> = portfolio.holdings.iter().map(|h| h.symbol.clone()).collect();
                let p_refs: Vec<&str> = p_syms.iter().map(|s| s.as_str()).collect();
                let p_quotes = client.get_quote(&p_refs).await?;
                let losers: Vec<_> = portfolio.holdings.iter().filter(|h| {
                    let price = p_quotes.iter().find(|q| q.symbol.as_deref() == Some(&h.symbol)).and_then(|q| q.regular_market_price).unwrap_or(0.0);
                    price < h.avg_cost
                }).collect();
                if !losers.is_empty() {
                    println!("    {} positions with losses (harvest candidates): {}", losers.len(), losers.iter().map(|h| h.symbol.as_str()).collect::<Vec<_>>().join(", "));
                } else {
                    println!("    No tax-loss harvest opportunities.");
                }
            }

            // 5. AI report
            println!("  [5/5] Generating AI report...");
            let ai = crate::ai::AiClient::new();
            if ai.is_available().await {
                let mut data = String::new();
                for s in scores.iter().take(10) {
                    data.push_str(&format!("{}: score={:.0}, moat={}, 5Y={:+.0}%, risk={}\n", s.symbol, s.total_score, s.moat, s.projected_5y_return, s.risk_tier));
                }
                if let Ok(report) = ai.generate_longterm_report(&data).await {
                    print_section("AI Investment Memo");
                    for line in report.lines() { println!("  {}", line); }
                }
            } else {
                println!("    Ollama not running. Skipping AI report.");
            }

            println!("\n  {} Long-term daemon complete.", "✓".green().bold());
        }

        _ => {
            println!();
            println!("  {} Daemon Modes:", "→".cyan());
            println!();
            println!("    {} — Continuous intraday monitor (9 AM – 3:30 PM)", "stockwise daemon intraday [CAPITAL]".bold());
            println!("    {} — Daily long-term analysis (run after market close)", "stockwise daemon longterm".bold());
            println!();
            println!("  {} The intraday daemon runs in paper trading mode by default.", "→".dimmed());
            println!("  {} It scans, enters, monitors, and squares off automatically.", "→".dimmed());
            println!("  {} Results are saved to simulation history.", "→".dimmed());
            println!();
        }
    }
    Ok(())
}

// ── Multi-timeframe Chart ──

pub async fn cmd_chart(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    // Fetch quote for current price
    let quotes = client.get_quote(&[resolved.as_str()]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let price = q.regular_market_price.unwrap_or(0.0);
    let name = q.short_name.as_deref().or(q.long_name.as_deref()).unwrap_or("Unknown");

    print_header(&format!("{} — {}  {}", resolved, name, format_price(price, cur).bold()));

    let timeframes: [(&str, &str, &str); 4] = [
        ("5d", "15m", "1 Week"),
        ("1mo", "1d", "1 Month"),
        ("3mo", "1d", "3 Months"),
        ("1y", "1wk", "1 Year"),
    ];

    for (range, interval, label) in &timeframes {
        let chart = match client.get_chart(&resolved, range, interval).await {
            Ok(c) => c,
            Err(_) => continue,
        };

        let closes: Vec<f64> = chart.indicators.quote.first()
            .and_then(|qi| qi.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        if closes.len() < 3 { continue; }

        let first = closes[0];
        let last = *closes.last().unwrap();
        let change = ((last / first) - 1.0) * 100.0;
        let min = closes.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = closes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let vol = technical::annualized_volatility(&closes);

        let change_str = if change >= 0.0 {
            format!("{:+.2}%", change).green().bold().to_string()
        } else {
            format!("{:+.2}%", change).red().bold().to_string()
        };

        let color = if last >= first { "green" } else { "red" };
        println!();
        for line in charts::line_chart(&closes, 55, 6, color, &format!("{} ({})", label, change_str)) {
            println!("{}", line);
        }
        println!(
            "  {} {}{:.2}  {} {}{:.2}  {} {}",
            "Low:".dimmed(), csym, min,
            "High:".dimmed(), csym, max,
            "Vol:".dimmed(), vol.map_or("—".into(), |v| format!("{:.0}%", v * 100.0)),
        );
    }

    println!();
    Ok(())
}
