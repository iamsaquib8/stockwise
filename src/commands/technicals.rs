//! Price-chart analysis: indicators, history, multi-timeframe views, patterns, levels, volume, gaps, and stops.

use super::*;

/// Bullish and bearish votes behind the "Technical Bias" verdict: price vs
/// SMA 50 and SMA 200, RSI extremes, MACD histogram sign.
fn technical_bias(closes: &[f64], current: f64) -> (i32, i32) {
    let mut bullish = 0;
    let mut bearish = 0;
    for ma in [technical::sma(closes, 50), technical::sma(closes, 200)]
        .into_iter()
        .flatten()
    {
        if current > ma {
            bullish += 1;
        } else {
            bearish += 1;
        }
    }
    if let Some(rsi) = technical::rsi(closes, 14) {
        if rsi < 30.0 {
            bullish += 1;
        } else if rsi > 70.0 {
            bearish += 1;
        }
    }
    if let Some((_, _, hist)) = technical::macd(closes) {
        if hist > 0.0 {
            bullish += 1;
        } else {
            bearish += 1;
        }
    }
    (bullish, bearish)
}

/// `technical --json`: the indicators the table shows. Indicator values are
/// rounded to 4 decimals, percentages to 2; null when there are too few bars.
pub(crate) fn technical_json(
    symbol: &str,
    currency: Option<&str>,
    period: &str,
    interval: &str,
    closes: &[f64],
    highs: &[f64],
    lows: &[f64],
    volumes: &[u64],
) -> serde_json::Value {
    let current = closes.last().copied().unwrap_or(f64::NAN);
    let r4 = |v: Option<f64>| round_opt(v, 4);
    let atr = technical::atr(highs, lows, closes, 14);
    let rsi = technical::rsi(closes, 14);
    let (bullish, bearish) = technical_bias(closes, current);
    let verdict = match bullish.cmp(&bearish) {
        _ if bullish + bearish == 0 => None,
        std::cmp::Ordering::Greater => Some("bullish"),
        std::cmp::Ordering::Less => Some("bearish"),
        std::cmp::Ordering::Equal => Some("neutral"),
    };
    serde_json::json!({
        "symbol": symbol,
        "currency": currency,
        "period": period,
        "interval": interval,
        "bars": closes.len(),
        "price": current,
        "sma_10": r4(technical::sma(closes, 10)),
        "sma_20": r4(technical::sma(closes, 20)),
        "sma_50": r4(technical::sma(closes, 50)),
        "sma_100": r4(technical::sma(closes, 100)),
        "sma_200": r4(technical::sma(closes, 200)),
        "ema_12": r4(technical::ema(closes, 12)),
        "ema_26": r4(technical::ema(closes, 26)),
        "rsi_14": round_opt(rsi, 2),
        "rsi_signal": rsi.map(technical::rsi_signal),
        "macd": technical::macd(closes).map(|(line, signal, hist)| serde_json::json!({
            "line": round(line, 4),
            "signal": round(signal, 4),
            "histogram": round(hist, 4),
            "trend": technical::macd_signal(hist),
        })),
        "bollinger": technical::bollinger_bands(closes, 20).map(|(upper, middle, lower)| {
            let position = if current > upper {
                "above_upper"
            } else if current < lower {
                "below_lower"
            } else {
                "within"
            };
            serde_json::json!({
                "upper": round(upper, 4),
                "middle": round(middle, 4),
                "lower": round(lower, 4),
                "position": position,
            })
        }),
        "atr_14": r4(atr),
        "atr_percent": atr.map(|a| round(a / current * 100.0, 2)),
        "vwap": r4(technical::vwap(highs, lows, closes, volumes)),
        "bias": {
            "verdict": verdict,
            "bullish": bullish,
            "bearish": bearish,
        },
    })
}

pub async fn cmd_technical(symbol: &str, period: &str, market: Market, fmt: Format) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    let interval = period_to_interval(period);
    let chart = client.get_chart(&resolved, period, interval).await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
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
        bail!("Not enough price data for technical analysis — try a longer period, e.g. -p 6mo");
    }

    let current = *closes.last().unwrap();
    if fmt == Format::Json {
        return output::print_json(&technical_json(
            &resolved, cur, period, interval, &closes, &highs, &lows, &volumes,
        ));
    }
    print_header(&format!("Technical Analysis: {}", resolved));
    println!("  Current Price: {}\n", format_price(current, cur).bold());

    // Moving Averages
    print_section("Moving Averages");
    let ma_periods = [10, 20, 50, 100, 200];
    for period in ma_periods {
        if let Some(ma) = technical::sma(&closes, period) {
            let signal = if current > ma {
                "▲ Above"
            } else {
                "▼ Below"
            };
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
    let color = if recent.last() >= recent.first() {
        "green"
    } else {
        "red"
    };
    for line in charts::line_chart(&recent, 50, 8, color, "") {
        println!("{}", line);
    }

    // SMA overlay chart if enough data
    if technical::sma(&closes, 20).is_some() {
        let sma20_vals: Vec<f64> = (0..recent.len())
            .map(|i| {
                let end = closes.len() - recent.len() + i + 1;
                technical::sma(&closes[..end], 20).unwrap_or(recent[i])
            })
            .collect();
        println!();
        for line in charts::dual_line_chart(
            &recent,
            &sma20_vals,
            50,
            6,
            "Price vs SMA-20",
            "Price",
            "SMA-20",
        ) {
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
    let (bullish, bearish) = technical_bias(&closes, current);

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
    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await {
        let rsi_val: String =
            technical::rsi(&closes, 14).map_or("N/A".into(), |r| format!("{:.0}", r));
        let macd_val: String = technical::macd(&closes).map_or("N/A".into(), |(_, _, h)| {
            if h > 0.0 {
                "bullish".into()
            } else {
                "bearish".into()
            }
        });
        let bb_pos: String =
            technical::bollinger_bands(&closes, 20).map_or("N/A".into(), |(u, _, l)| {
                let p = (current - l) / (u - l) * 100.0;
                format!("{:.0}% of range", p)
            });
        let sma_pos: String = technical::sma(&closes, 200).map_or("N/A".into(), |s| {
            if current > s {
                "above 200-MA".into()
            } else {
                "below 200-MA".into()
            }
        });
        let data = format!(
            "Stock: {}, Price: {:.2}, RSI: {}, MACD: {}, BB: {}, Trend: {}",
            resolved, current, rsi_val, macd_val, bb_pos, sma_pos
        );
        print_section(&format!("AI Interpretation ({})", ai.provider().name()));
        match ai.interpret_technicals(&data).await {
            Ok(analysis) => print_ai_answer(&analysis),
            Err(_) => {
                println!("  {}", "AI unavailable.".dimmed());
            }
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

/// `history --csv` columns; the bars come from [`views::history`].
impl CsvRow for views::Bar {
    const COLUMNS: &'static [&'static str] = &["time", "open", "high", "low", "close", "volume"];
}

pub async fn cmd_history(symbol: &str, period: &str, market: Market, fmt: Format) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let hist = views::history(&client, &resolved, period, market).await?;

    match fmt {
        Format::Csv => return output::print_csv(&hist.bars),
        Format::Json => return output::print_json(&hist),
        Format::Table => {}
    }

    let cur = hist.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let timestamps: Vec<i64> = hist.bars.iter().map(|b| b.ts).collect();
    let closes: Vec<Option<f64>> = hist.bars.iter().map(|b| b.close).collect();
    let volumes: Vec<Option<u64>> = hist.bars.iter().map(|b| b.volume).collect();

    print_header(&format!("Price History: {} ({})", resolved, period));

    // Price chart
    let valid_closes: Vec<f64> = closes.iter().filter_map(|v| *v).collect();
    let valid_volumes: Vec<u64> = volumes.iter().filter_map(|v| *v).collect();
    if !valid_closes.is_empty() {
        let color = if valid_closes.last() >= valid_closes.first() {
            "green"
        } else {
            "red"
        };
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
        let max = valid_closes
            .iter()
            .cloned()
            .fold(f64::NEG_INFINITY, f64::max);
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
            .map(format_volume)
            .unwrap_or_else(|| "N/A".to_string());
        println!("  {:<14} {:>12} {:>14}", date.dimmed(), close, vol);
    }

    println!();
    Ok(())
}

pub async fn cmd_chart(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    // Fetch quote for current price
    let quotes = client.get_quote(&[resolved.as_str()]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let price = q.regular_market_price.unwrap_or(0.0);
    let name = q
        .short_name
        .as_deref()
        .or(q.long_name.as_deref())
        .unwrap_or("Unknown");

    print_header(&format!(
        "{} — {}  {}",
        resolved,
        name,
        format_price(price, cur).bold()
    ));

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

        let closes: Vec<f64> = chart
            .indicators
            .quote
            .first()
            .and_then(|qi| qi.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();

        if closes.len() < 3 {
            continue;
        }

        let first = closes[0];
        let last = *closes.last().unwrap();
        let change = ((last / first) - 1.0) * 100.0;
        let min = closes.iter().cloned().fold(f64::INFINITY, f64::min);
        let max = closes.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let vol =
            technical::annualized_volatility_with(&closes, technical::periods_per_year(interval));

        let change_str = if change >= 0.0 {
            format!("{:+.2}%", change).green().bold().to_string()
        } else {
            format!("{:+.2}%", change).red().bold().to_string()
        };

        let color = if last >= first { "green" } else { "red" };
        println!();
        for line in charts::line_chart(
            &closes,
            55,
            6,
            color,
            &format!("{} ({})", label, change_str),
        ) {
            println!("{}", line);
        }
        println!(
            "  {} {}{:.2}  {} {}{:.2}  {} {}",
            "Low:".dimmed(),
            csym,
            min,
            "High:".dimmed(),
            csym,
            max,
            "Vol:".dimmed(),
            vol.map_or("—".into(), |v| format!("{:.0}%", v * 100.0)),
        );
    }

    println!();
    Ok(())
}

// ── Sector Rotation ──

pub async fn cmd_multitimeframe(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    print_header(&format!("Multi-Timeframe Analysis: {}", resolved));

    let timeframes = [
        ("1mo", "1d", "1 Month"),
        ("3mo", "1d", "3 Months"),
        ("6mo", "1d", "6 Months"),
        ("1y", "1d", "1 Year"),
        ("2y", "1wk", "2 Years"),
        ("5y", "1mo", "5 Years"),
    ];

    println!(
        "  {:<12} {:>8} {:>10} {:>6} {:>8} {:>8} {:>8}",
        "Timeframe".bold(),
        "Return".bold(),
        "Volatility".bold(),
        "RSI".bold(),
        "SMA".bold(),
        "MACD".bold(),
        "Signal".bold()
    );
    println!("  {}", "─".repeat(68).dimmed());

    for (range, interval, label) in &timeframes {
        let chart = match client.get_chart(&resolved, range, interval).await {
            Ok(c) => c,
            Err(_) => continue,
        };
        let closes: Vec<f64> = chart
            .indicators
            .quote
            .first()
            .and_then(|q| q.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();
        if closes.len() < 5 {
            continue;
        }

        let ret = (closes.last().unwrap() / closes[0] - 1.0) * 100.0;
        let vol =
            technical::annualized_volatility_with(&closes, technical::periods_per_year(interval))
                .map(|v| format!("{:.1}%", v * 100.0))
                .unwrap_or("—".into());
        let rsi = technical::rsi(&closes, 14)
            .map(|r| format!("{:.0}", r))
            .unwrap_or("—".into());
        let sma_sig = technical::sma(&closes, 20)
            .map(|s| {
                if *closes.last().unwrap() > s {
                    "▲".green().to_string()
                } else {
                    "▼".red().to_string()
                }
            })
            .unwrap_or("—".into());
        let macd_sig = technical::macd(&closes)
            .map(|(_, _, h)| {
                if h > 0.0 {
                    "▲".green().to_string()
                } else {
                    "▼".red().to_string()
                }
            })
            .unwrap_or("—".into());

        let mut bullish = 0;
        let mut bearish = 0;
        if let Some(r) = technical::rsi(&closes, 14) {
            if r < 40.0 {
                bullish += 1;
            } else if r > 60.0 {
                bearish += 1;
            }
        }
        if let Some(s) = technical::sma(&closes, 20) {
            if *closes.last().unwrap() > s {
                bullish += 1;
            } else {
                bearish += 1;
            }
        }
        if let Some((_, _, h)) = technical::macd(&closes) {
            if h > 0.0 {
                bullish += 1;
            } else {
                bearish += 1;
            }
        }
        let signal = if bullish > bearish {
            "BUY".green().bold().to_string()
        } else if bearish > bullish {
            "SELL".red().bold().to_string()
        } else {
            "HOLD".yellow().to_string()
        };

        let ret_str = if ret >= 0.0 {
            format!("+{:.2}%", ret).green().to_string()
        } else {
            format!("{:.2}%", ret).red().to_string()
        };
        println!(
            "  {:<12} {:>8} {:>10} {:>6} {:>8} {:>8} {:>8}",
            label, ret_str, vol, rsi, sma_sig, macd_sig, signal
        );
    }
    println!();
    Ok(())
}

// ── Top Picks ──

pub async fn cmd_patterns(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client
        .get_chart(&resolved, period, period_to_interval(period))
        .await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let _csym = market::currency_symbol(cur);
    let opens: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.open.as_ref())
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
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    let n = opens
        .len()
        .min(highs.len())
        .min(lows.len())
        .min(closes.len());
    if n < 3 {
        bail!("Not enough price data — try a longer period with -p");
    }

    print_header(&format!("Candlestick Patterns: {}", resolved));
    let mut found = Vec::new();

    for i in 1..n {
        let o = opens[i];
        let h = highs[i];
        let l = lows[i];
        let c = closes[i];
        let prev_o = opens[i - 1];
        let prev_c = closes[i - 1];
        let body = (c - o).abs();
        let range = h - l;
        if range < f64::EPSILON {
            continue;
        }

        // Doji
        if body / range < 0.1 {
            found.push((i, "Doji", "Indecision — potential reversal", "yellow"));
        }
        // Hammer (bullish)
        if c > o && (o - l) > body * 2.0 && (h - c) < body * 0.5 {
            found.push((i, "Hammer", "Bullish reversal signal", "green"));
        }
        // Shooting Star (bearish)
        if o > c && (h - o) > body * 2.0 && (c - l) < body * 0.5 {
            found.push((i, "Shooting Star", "Bearish reversal signal", "red"));
        }
        // Bullish Engulfing
        if i > 0 && prev_c < prev_o && c > o && o < prev_c && c > prev_o {
            found.push((i, "Bullish Engulfing", "Strong bullish reversal", "green"));
        }
        // Bearish Engulfing
        if i > 0 && prev_c > prev_o && c < o && o > prev_c && c < prev_o {
            found.push((i, "Bearish Engulfing", "Strong bearish reversal", "red"));
        }
        // Morning Star (3-candle)
        if i >= 2 {
            let pp_o = opens[i - 2];
            let pp_c = closes[i - 2];
            if pp_c < pp_o
                && (prev_c - prev_o).abs() / (highs[i - 1] - lows[i - 1]).max(0.01) < 0.3
                && c > o
                && c > (pp_o + pp_c) / 2.0
            {
                found.push((
                    i,
                    "Morning Star",
                    "Strong bullish reversal (3-candle)",
                    "green",
                ));
            }
        }
    }

    if found.is_empty() {
        println!(
            "  {}",
            "No significant patterns detected in this period.".dimmed()
        );
    } else {
        let timestamps = chart.timestamp.unwrap_or_default();
        println!(
            "  {:<14} {:<22} {}",
            "Date".bold(),
            "Pattern".bold(),
            "Signal".bold()
        );
        println!("  {}", "─".repeat(55).dimmed());
        for (idx, name, desc, color) in found.iter().rev().take(10) {
            let date = timestamps
                .get(*idx)
                .and_then(|&t| chrono::DateTime::from_timestamp(t, 0))
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let colored = match *color {
                "green" => name.green().bold().to_string(),
                "red" => name.red().bold().to_string(),
                _ => name.yellow().to_string(),
            };
            println!("  {:<14} {:<22} {}", date.dimmed(), colored, desc.dimmed());
        }
    }
    println!();
    Ok(())
}

// ── Compare Sectors ──

pub async fn cmd_support(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client
        .get_chart(&resolved, period, period_to_interval(period))
        .await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

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
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    if closes.len() < 5 {
        bail!("Not enough price data — try a longer period with -p");
    }

    let current = *closes.last().unwrap();
    let last_high = *highs.last().unwrap();
    let last_low = *lows.last().unwrap();
    let last_close = *closes.last().unwrap();

    let (pivot, r1, r2, r3, s1, s2, s3) = technical::pivot_points(last_high, last_low, last_close);

    print_header(&format!("Support & Resistance: {}", resolved));
    println!("  Current Price: {}\n", format_price(current, cur).bold());

    print_section("Classic Pivot Points");
    print_kv(
        "R3 (Resistance)",
        &format!("{}{:.2}", csym, r3).red().to_string(),
    );
    print_kv("R2", &format!("{}{:.2}", csym, r2).red().to_string());
    print_kv("R1", &format!("{}{:.2}", csym, r1).red().to_string());
    print_kv("Pivot", &format!("{}{:.2}", csym, pivot).bold().to_string());
    print_kv("S1", &format!("{}{:.2}", csym, s1).green().to_string());
    print_kv("S2", &format!("{}{:.2}", csym, s2).green().to_string());
    print_kv(
        "S3 (Support)",
        &format!("{}{:.2}", csym, s3).green().to_string(),
    );

    // Fibonacci levels
    if let Some((swing_high, swing_low)) =
        technical::find_swing_points(&highs, &lows, 60.min(highs.len()))
    {
        let fibs = technical::fibonacci_levels(swing_high, swing_low);
        print_section("Fibonacci Retracement");
        print_kv("Swing High", &format!("{}{:.2}", csym, swing_high));
        print_kv("23.6%", &format!("{}{:.2}", csym, fibs[0]));
        print_kv("38.2%", &format!("{}{:.2}", csym, fibs[1]));
        print_kv(
            "50.0%",
            &format!("{}{:.2}", csym, fibs[2]).bold().to_string(),
        );
        print_kv("61.8%", &format!("{}{:.2}", csym, fibs[3]));
        print_kv("78.6%", &format!("{}{:.2}", csym, fibs[4]));
        print_kv("Swing Low", &format!("{}{:.2}", csym, swing_low));
    }

    // Price position
    print_section("Current Position");
    if current > r1 {
        println!(
            "  {} Above R1 — bullish, next target R2 ({}{:.2})",
            "▲".green(),
            csym,
            r2
        );
    } else if current > pivot {
        println!(
            "  {} Above pivot — mildly bullish, resistance at R1 ({}{:.2})",
            "▲".green(),
            csym,
            r1
        );
    } else if current > s1 {
        println!(
            "  {} Below pivot — mildly bearish, support at S1 ({}{:.2})",
            "▼".yellow(),
            csym,
            s1
        );
    } else {
        println!(
            "  {} Below S1 — bearish, next support S2 ({}{:.2})",
            "▼".red(),
            csym,
            s2
        );
    }
    println!();
    Ok(())
}

// ── Volume Profile ──

pub async fn cmd_fibs(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client
        .get_chart(&resolved, period, period_to_interval(period))
        .await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);
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
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    if closes.len() < 20 {
        bail!("Not enough price data — try a longer period with -p");
    }

    let current = *closes.last().unwrap();

    // Find swing points from different lookbacks
    print_header(&format!("Fibonacci Retracement: {}", resolved));
    println!("  Current: {}\n", format_price(current, cur).bold());

    for (lookback, label) in [(30, "1 Month"), (60, "3 Month"), (120, "6 Month")] {
        let lb = lookback.min(highs.len());
        if let Some((sh, sl)) = technical::find_swing_points(&highs, &lows, lb) {
            let fibs = technical::fibonacci_levels(sh, sl);
            print_section(&format!(
                "{} Swing ({}{:.2} → {}{:.2})",
                label, csym, sh, csym, sl
            ));
            let levels = [
                ("0.0% (High)", sh),
                ("23.6%", fibs[0]),
                ("38.2%", fibs[1]),
                ("50.0%", fibs[2]),
                ("61.8%", fibs[3]),
                ("78.6%", fibs[4]),
                ("100% (Low)", sl),
            ];
            for (name, val) in &levels {
                let marker = if (current - val).abs() / current < 0.01 {
                    " ← YOU ARE HERE".yellow().bold().to_string()
                } else {
                    String::new()
                };
                print_kv(name, &format!("{}{:.2}{}", csym, val, marker));
            }
        }
    }
    println!();
    Ok(())
}

pub async fn cmd_volume(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client
        .get_chart(&resolved, period, period_to_interval(period))
        .await?;
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
    let n = closes.len().min(volumes.len());
    if n < 10 {
        bail!("Not enough price data — try a longer period with -p");
    }

    print_header(&format!("Volume Analysis: {}", resolved));

    // OBV
    let obv = technical::obv(&closes, &volumes);
    if obv.len() > 20 {
        let recent_obv: Vec<f64> = obv[obv.len() - 60.min(obv.len())..].to_vec();
        for line in charts::line_chart(&recent_obv, 50, 6, "cyan", "On-Balance Volume (OBV)") {
            println!("{}", line);
        }
        let obv_trend = if obv.last() > obv.get(obv.len().saturating_sub(20)) {
            "Rising — accumulation".green().to_string()
        } else {
            "Falling — distribution".red().to_string()
        };
        print_kv("OBV Trend", &obv_trend);
    }

    // A/D Line
    let ad = technical::ad_line(&highs, &lows, &closes, &volumes);
    if ad.len() > 20 {
        let recent_ad: Vec<f64> = ad[ad.len() - 60.min(ad.len())..].to_vec();
        println!();
        for line in charts::line_chart(&recent_ad, 50, 6, "yellow", "Accumulation/Distribution") {
            println!("{}", line);
        }
    }

    // Volume stats
    print_section("Volume Statistics");
    let avg_vol_20: u64 = volumes[n.saturating_sub(20)..].iter().sum::<u64>() / 20.min(n) as u64;
    let today_vol = *volumes.last().unwrap();
    let ratio = today_vol as f64 / avg_vol_20.max(1) as f64;
    print_kv("Today's Volume", &format_volume(today_vol));
    print_kv("20-Day Avg Volume", &format_volume(avg_vol_20));
    let ratio_str = if ratio > 1.5 {
        format!("{:.2}x (HIGH)", ratio).green().bold().to_string()
    } else if ratio > 1.0 {
        format!("{:.2}x (above avg)", ratio).to_string()
    } else {
        format!("{:.2}x (below avg)", ratio).red().to_string()
    };
    print_kv("Volume Ratio", &ratio_str);

    // Volume bars
    let recent_vols: Vec<u64> = volumes[n.saturating_sub(40)..].to_vec();
    println!();
    println!("  {}", "Volume (40 days)".bold());
    for line in charts::volume_bars(&recent_vols, 40, 4) {
        println!("{}", line);
    }
    println!();
    Ok(())
}

// ── Gaps ──

pub async fn cmd_gaps(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client
        .get_chart(&resolved, period, period_to_interval(period))
        .await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);
    let opens: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.open.as_ref())
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
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

    let gaps = technical::detect_gaps(&opens, &highs, &lows, &closes);
    let timestamps = chart.timestamp.unwrap_or_default();

    print_header(&format!("Gap Analysis: {}", resolved));

    if gaps.is_empty() {
        println!("  {}", "No price gaps detected in this period.".dimmed());
        println!();
        return Ok(());
    }

    let unfilled: Vec<&technical::Gap> = gaps.iter().filter(|g| !g.filled).collect();
    let filled: Vec<&technical::Gap> = gaps.iter().filter(|g| g.filled).collect();

    println!(
        "  Found {} gaps ({} unfilled, {} filled)\n",
        gaps.len(),
        unfilled.len(),
        filled.len()
    );

    if !unfilled.is_empty() {
        print_section("Open (Unfilled) Gaps");
        for g in &unfilled {
            let date = timestamps
                .get(g.index)
                .and_then(|&t| chrono::DateTime::from_timestamp(t, 0))
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let dir = match g.gap_type {
                technical::GapType::Up => "GAP UP".green().bold().to_string(),
                technical::GapType::Down => "GAP DOWN".red().bold().to_string(),
            };
            println!(
                "  {} {} — {}{:.2} to {}{:.2} ({})",
                date.dimmed(),
                dir,
                csym,
                g.gap_low,
                csym,
                g.gap_high,
                format!("{}{:.2} range", csym, g.gap_high - g.gap_low).dimmed()
            );
        }
    }

    if !filled.is_empty() {
        print_section(&format!("Filled Gaps ({})", filled.len()));
        for g in filled.iter().rev().take(5) {
            let date = timestamps
                .get(g.index)
                .and_then(|&t| chrono::DateTime::from_timestamp(t, 0))
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let dir = match g.gap_type {
                technical::GapType::Up => "UP",
                technical::GapType::Down => "DN",
            };
            println!(
                "  {} {} {}{:.2}–{}{:.2} {}",
                date.dimmed(),
                dir,
                csym,
                g.gap_low,
                csym,
                g.gap_high,
                "FILLED".dimmed()
            );
        }
    }
    println!();
    Ok(())
}

// ── Stoploss Calculator ──

pub async fn cmd_stoploss(
    symbol: &str,
    entry: Option<f64>,
    period: &str,
    market: Market,
) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client
        .get_chart(&resolved, period, period_to_interval(period))
        .await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

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
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    if closes.len() < 14 {
        bail!("Not enough price data — try a longer period with -p");
    }

    let current = *closes.last().unwrap();
    let entry_price = entry.unwrap_or(current);
    if entry_price <= 0.0 {
        bail!("Entry price must be greater than zero");
    }
    let atr = technical::atr(&highs, &lows, &closes, 14).unwrap_or(0.0);

    print_header(&format!("Stop-Loss Calculator: {}", resolved));
    print_kv("Entry Price", &format!("{}{:.2}", csym, entry_price));
    print_kv("Current Price", &format!("{}{:.2}", csym, current));
    print_kv(
        "ATR (14)",
        &format!("{}{:.2} ({:.2}%)", csym, atr, (atr / current) * 100.0),
    );

    let (cons, moderate, aggressive) = technical::atr_stop_loss(entry_price, atr);
    print_section("ATR-Based Stop Loss");
    print_kv(
        "Conservative (3x ATR)",
        &format!(
            "{}{:.2} ({:.2}% risk)",
            csym,
            cons,
            ((entry_price - cons) / entry_price) * 100.0
        )
        .green()
        .to_string(),
    );
    print_kv(
        "Moderate (2x ATR)",
        &format!(
            "{}{:.2} ({:.2}% risk)",
            csym,
            moderate,
            ((entry_price - moderate) / entry_price) * 100.0
        )
        .yellow()
        .to_string(),
    );
    print_kv(
        "Aggressive (1.5x ATR)",
        &format!(
            "{}{:.2} ({:.2}% risk)",
            csym,
            aggressive,
            ((entry_price - aggressive) / entry_price) * 100.0
        )
        .red()
        .to_string(),
    );

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
        print_kv(
            &format!("R:R 1:{:.1}", rr),
            &format!(
                "{}{:.2} (+{:.2}%)",
                csym,
                target,
                ((target - entry_price) / entry_price) * 100.0
            )
            .green()
            .to_string(),
        );
    }
    println!();
    Ok(())
}

// ── Peers Comparison ──
