//! Strategy tools: SIP and what-if calculators, intraday ideas, long-term picks, and AI reports.
//! Backtests live in `backtests.rs`.

use super::*;

// ── SIP/DCA Simulator ──

pub async fn cmd_sip(
    symbol: &str,
    amount: f64,
    period: &str,
    market: Market,
    fmt: Format,
) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let chart = client.get_chart(&resolved, period, "1mo").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);

    // Monthly bars; null bars (halts) are dropped.
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).filter(|p| *p > 0.0).collect())
        .unwrap_or_default();
    let n = closes.len();

    if n < 2 {
        bail!("Not enough price data — try a longer period with -p");
    }

    let mut total_invested = 0.0;
    let mut total_units = 0.0;
    let mut value_history = Vec::new();
    let mut invested_history = Vec::new();
    let mut cashflows = Vec::new();

    for (i, &price) in closes.iter().enumerate() {
        total_units += amount / price;
        total_invested += amount;
        value_history.push(total_units * price);
        invested_history.push(total_invested);
        cashflows.push((i as f64 / 12.0, -amount));
    }

    let final_value = total_units * closes[n - 1];
    let total_return = ((final_value / total_invested) - 1.0) * 100.0;
    // XIRR: each instalment earns only from its own date, not the whole period.
    cashflows.push(((n - 1) as f64 / 12.0, final_value));
    let xirr = technical::xirr(&cashflows).map(|r| r * 100.0);

    if fmt == Format::Json {
        return output::print_json(&serde_json::json!({
            "symbol": resolved,
            "currency": cur,
            "period": period,
            "interval": "1mo",
            "monthly_amount": amount,
            "months": n,
            "total_invested": round(total_invested, 2),
            "final_value": round(final_value, 2),
            "profit": round(final_value - total_invested, 2),
            "total_return_percent": round(total_return, 2),
            "xirr_percent": round_opt(xirr, 2),
            "total_units": round(total_units, 4),
            "avg_cost": round(total_invested / total_units, 2),
            "current_price": closes[n - 1],
        }));
    }

    print_header(&format!(
        "SIP Simulator: {} — {}{}/month for {}",
        resolved, csym, amount, period
    ));

    // Chart: invested vs value
    for line in charts::dual_line_chart(
        &value_history,
        &invested_history,
        55,
        8,
        "Portfolio Value vs Amount Invested",
        "Value",
        "Invested",
    ) {
        println!("{}", line);
    }

    print_section("Results");
    print_kv("Months Invested", &n.to_string());
    print_kv("Monthly Amount", &format!("{}{:.2}", csym, amount));
    print_kv("Total Invested", &format!("{}{:.2}", csym, total_invested));
    print_kv(
        "Current Value",
        &format!("{}{:.2}", csym, final_value).bold().to_string(),
    );
    let profit = final_value - total_invested;
    let profit_str = if profit >= 0.0 {
        format!("+{}{:.2}", csym, profit).green().bold().to_string()
    } else {
        format!("-{}{:.2}", csym, profit.abs())
            .red()
            .bold()
            .to_string()
    };
    print_kv("Profit/Loss", &profit_str);
    let ret_str = if total_return >= 0.0 {
        format!("+{:.2}%", total_return).green().to_string()
    } else {
        format!("{:.2}%", total_return).red().to_string()
    };
    print_kv("Total Return", &ret_str);
    print_kv(
        "Annualized (XIRR)",
        &xirr.map_or("N/A".to_string(), |r| format!("{:.2}%", r)),
    );
    print_kv("Total Units", &format!("{:.4}", total_units));
    print_kv(
        "Avg Cost/Unit",
        &format!("{}{:.2}", csym, total_invested / total_units),
    );
    print_kv("Current Price", &format!("{}{:.2}", csym, closes[n - 1]));
    println!();
    Ok(())
}

// ── Forecast ──

pub async fn cmd_whatif(
    symbol: &str,
    amount: f64,
    date_str: &str,
    market: Market,
    fmt: Format,
) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    // Determine period from date
    let target_date = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .context("Date format: YYYY-MM-DD")?;
    let today = chrono::Local::now().date_naive();
    let days = (today - target_date).num_days();
    let range = if days <= 30 {
        "1mo"
    } else if days <= 90 {
        "3mo"
    } else if days <= 180 {
        "6mo"
    } else if days <= 365 {
        "1y"
    } else if days <= 730 {
        "2y"
    } else {
        "5y"
    };

    let chart = client.get_chart(&resolved, range, "1d").await?;
    let cur = chart.meta.as_ref().and_then(|m| m.currency.as_deref());
    let csym = market::currency_symbol(cur);
    let timestamps = chart.timestamp.unwrap_or_default();
    let closes_raw = chart
        .indicators
        .quote
        .first()
        .and_then(|q| q.close.clone())
        .unwrap_or_default();
    // Pair each timestamp with its close, dropping bars with no price, so the
    // timestamp↔price index alignment can never drift (a null interior close
    // would otherwise shift every later index and pick the wrong buy price).
    let series: Vec<(i64, f64)> = timestamps
        .iter()
        .zip(closes_raw.iter())
        .filter_map(|(&ts, &c)| c.map(|p| (ts, p)))
        .collect();
    if series.is_empty() {
        bail!("No price data available for that range");
    }

    // Find closest price to target date
    let target_ts = target_date
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp();
    let mut best_idx = 0;
    let mut best_diff = i64::MAX;
    for (i, (ts, _)) in series.iter().enumerate() {
        let diff = (ts - target_ts).abs();
        if diff < best_diff {
            best_diff = diff;
            best_idx = i;
        }
    }

    let buy_price = series[best_idx].1;
    let Some(current) = series.last().map(|(_, p)| *p) else {
        bail!("No price data available for that range");
    };
    if buy_price <= 0.0 {
        bail!("Could not find a price for that date (use YYYY-MM-DD on or after the listing date)");
    }

    let shares = (amount / buy_price).floor();
    let invested = shares * buy_price;
    let value = shares * current;
    let pnl = value - invested;
    let pnl_pct = ((current / buy_price) - 1.0) * 100.0;

    let years = days as f64 / 365.0;
    let cagr = if years > 0.1 {
        ((current / buy_price).powf(1.0 / years) - 1.0) * 100.0
    } else {
        pnl_pct
    };

    if fmt == Format::Json {
        let buy_date = chrono::DateTime::from_timestamp(series[best_idx].0, 0)
            .map(|d| d.format("%Y-%m-%d").to_string());
        return output::print_json(&serde_json::json!({
            "symbol": resolved,
            "currency": cur,
            "amount": amount,
            "date": date_str,
            "buy_date": buy_date,
            "buy_price": buy_price,
            "shares": shares,
            "invested": round(invested, 2),
            "current_price": current,
            "value": round(value, 2),
            "pnl": round(pnl, 2),
            "pnl_percent": round(pnl_pct, 2),
            "cagr_percent": round(cagr, 2),
            "days": days,
            "years": round(years, 2),
        }));
    }

    print_header(&format!(
        "What If: {}{:.0} in {} on {}",
        csym, amount, resolved, date_str
    ));
    print_kv("Buy Date", date_str);
    print_kv("Buy Price", &format!("{}{:.2}", csym, buy_price));
    print_kv("Shares Bought", &format!("{:.0}", shares));
    print_kv("Amount Invested", &format!("{}{:.2}", csym, invested));
    println!();
    print_kv("Current Price", &format!("{}{:.2}", csym, current));
    print_kv(
        "Current Value",
        &format!("{}{:.2}", csym, value).bold().to_string(),
    );
    let pnl_str = if pnl >= 0.0 {
        format!("+{}{:.2} (+{:.2}%)", csym, pnl, pnl_pct)
            .green()
            .bold()
            .to_string()
    } else {
        format!("-{}{:.2} ({:.2}%)", csym, pnl.abs(), pnl_pct)
            .red()
            .bold()
            .to_string()
    };
    print_kv("Profit/Loss", &pnl_str);
    print_kv("CAGR", &format!("{:.1}%", cagr));
    print_kv(
        "Holding Period",
        &format!("{} days ({:.1} years)", days, years),
    );
    println!();
    Ok(())
}

pub async fn cmd_intraday(
    amount: f64,
    target_pct: f64,
    strategy: Option<&str>,
    market: Market,
) -> Result<()> {
    // The bot plans around exchange sessions (entries, square-off times).
    market.require_bot_support("intraday")?;
    // Load the optional rule first so a bad file fails before any scanning.
    let rule = match strategy {
        Some(s) => Some(backtest::rules::load(s)?.compile()?),
        None => None,
    };
    let target_profit = amount * (target_pct / 100.0);
    let csym = market.currency_symbol();
    let market_name = market.label();

    print_header(&format!("Intraday Trading Bot — {} Market", market_name));
    println!(
        "  Capital: {}  |  Target: {}% ({}{:.2})  |  Max Risk: 1%/trade\n",
        format!("{}{:.2}", csym, amount).bold().cyan(),
        target_pct,
        csym,
        target_profit
    );

    let client = YahooClient::new().await?;

    // Sector heat
    if market == Market::In {
        println!("  {} Scanning sectors...", "⟳".yellow());
        if let Ok(heats) = intraday::scan_sector_heat(&client).await {
            print_section("Sector Heat");
            for h in &heats {
                let bar_len = (h.change_pct.abs() * 5.0).min(20.0) as usize;
                let bar = if h.change_pct >= 0.0 {
                    "█".repeat(bar_len.max(1)).green().to_string()
                } else {
                    "█".repeat(bar_len.max(1)).red().to_string()
                };
                let pct = if h.change_pct >= 0.0 {
                    format!("{:+.2}%", h.change_pct).green().to_string()
                } else {
                    format!("{:+.2}%", h.change_pct).red().to_string()
                };
                let hot = if h.hot {
                    " HOT".yellow().bold().to_string()
                } else {
                    String::new()
                };
                println!("  {:<12} {} {}{}", h.name, bar, pct, hot);
            }
        }
    }

    // Scan stocks
    let extra = rule
        .as_ref()
        .map_or(String::new(), |r| format!(" + rule `{}`", r.name));
    println!(
        "\n  {} Scanning stocks with 9 strategies{}...\n",
        "⟳".yellow(),
        extra
    );
    let signals = intraday::scan_intraday_with(&client, market, rule.as_ref()).await?;
    let plans = intraday::generate_trade_plans(&signals, amount, target_pct, 1.0);

    // Market regime summary
    let regimes: Vec<_> = signals.iter().map(|s| &s.regime).collect();
    let uptrend = regimes
        .iter()
        .filter(|r| {
            matches!(
                r,
                intraday::MarketRegime::Uptrend | intraday::MarketRegime::StrongUptrend
            )
        })
        .count();
    let downtrend = regimes
        .iter()
        .filter(|r| {
            matches!(
                r,
                intraday::MarketRegime::Downtrend | intraday::MarketRegime::StrongDowntrend
            )
        })
        .count();
    let market_bias = if uptrend > downtrend * 2 {
        "BULLISH".green().bold()
    } else if downtrend > uptrend * 2 {
        "BEARISH".red().bold()
    } else {
        "MIXED".yellow().bold()
    };
    println!(
        "  Market Bias: {} ({} bullish, {} bearish, {} ranging)\n",
        market_bias,
        uptrend,
        downtrend,
        signals.len() - uptrend - downtrend
    );

    // Top signals table
    print_section("Signal Scanner");
    println!(
        "  {:<14} {:>8} {:>8} {:>5} {:>6} {:>8} {:>7} {:>5}",
        "Symbol".bold(),
        "Price".bold(),
        "Chg%".bold(),
        "RSI".bold(),
        "Vol".bold(),
        "Regime".bold(),
        "Score".bold(),
        "Conf".bold()
    );
    println!("  {}", "─".repeat(72).dimmed());

    for s in signals.iter().take(15) {
        let chg = if s.change_pct >= 0.0 {
            format!("{:+.2}%", s.change_pct).green().to_string()
        } else {
            format!("{:+.2}%", s.change_pct).red().to_string()
        };
        let rsi_str = s.rsi.map_or("—".into(), |r| {
            let t = format!("{:.0}", r);
            if r < 35.0 {
                t.green().to_string()
            } else if r > 70.0 {
                t.red().to_string()
            } else {
                t
            }
        });
        let regime_str = match s.regime {
            intraday::MarketRegime::StrongUptrend => "▲▲".green().to_string(),
            intraday::MarketRegime::Uptrend => "▲".green().to_string(),
            intraday::MarketRegime::Ranging => "─".yellow().to_string(),
            intraday::MarketRegime::Downtrend => "▼".red().to_string(),
            intraday::MarketRegime::StrongDowntrend => "▼▼".red().to_string(),
        };
        let score_str = if s.score >= 70.0 {
            format!("{:.0}", s.score).green().bold().to_string()
        } else if s.score >= 55.0 {
            format!("{:.0}", s.score).yellow().to_string()
        } else {
            format!("{:.0}", s.score).dimmed().to_string()
        };
        let conf_str = match s.confidence {
            intraday::Confidence::High => "H".green().bold().to_string(),
            intraday::Confidence::Medium => "M".yellow().to_string(),
            intraday::Confidence::Low => "L".dimmed().to_string(),
        };
        println!(
            "  {:<14} {:>8} {:>8} {:>5} {:>6} {:>8} {:>7} {:>5}",
            s.symbol.cyan(),
            format!("{}{:.2}", csym, s.price),
            chg,
            rsi_str,
            format!("{:.1}x", s.volume_ratio),
            regime_str,
            score_str,
            conf_str
        );
    }

    // Trade plans
    if plans.is_empty() {
        println!(
            "\n  {}",
            "No high-confidence trades found. Market conditions may be unfavorable.".yellow()
        );
    } else {
        print_section(&format!(
            "Trade Plans (target: {}{:.2})",
            csym, target_profit
        ));

        for (i, plan) in plans.iter().take(5).enumerate() {
            let s = &plan.signal;
            println!();
            println!(
                "  {}  {} — {} [{} | {} confidence]",
                format!("#{}", i + 1).bold().cyan(),
                s.symbol.bold().cyan(),
                s.name,
                s.regime,
                match s.confidence {
                    intraday::Confidence::High => "HIGH".green().bold().to_string(),
                    intraday::Confidence::Medium => "MED".yellow().to_string(),
                    _ => "LOW".dimmed().to_string(),
                }
            );
            println!("  {}", "─".repeat(55).dimmed());

            // Show which strategies triggered
            println!(
                "  {} {}",
                "Strategies:".dimmed(),
                s.strategies
                    .iter()
                    .map(|st| format!("{} ({:.0})", st.name, st.strength))
                    .collect::<Vec<_>>()
                    .join(" + ")
            );
            for st in &s.strategies {
                println!("    {} {}", "→".cyan(), st.reason.dimmed());
            }

            println!();
            println!(
                "  {} {}    {} {}    {} {}",
                "Entry:".dimmed(),
                format!("{}{:.2}", csym, plan.entry).bold(),
                "Target 1:".dimmed(),
                format!("{}{:.2}", csym, plan.target1).green(),
                "Target 2:".dimmed(),
                format!("{}{:.2}", csym, plan.target2).green().bold()
            );
            println!(
                "  {} {}    {} {}    {} {}",
                "Stop Loss:".dimmed(),
                format!("{}{:.2}", csym, plan.stop_loss).red(),
                "Trailing:".dimmed(),
                format!("{}{:.2}", csym, plan.trailing_stop).red().dimmed(),
                "R:R:".dimmed(),
                format!("1:{:.1}", plan.risk_reward).yellow()
            );
            let capital = format!(
                "{}{:.0} ({:.0}%)",
                csym, plan.capital_required, plan.position_pct
            );
            println!(
                "  {} {}    {} {}    {} {}    {} {}",
                "Qty:".dimmed(),
                plan.qty.to_string().bold(),
                "Capital:".dimmed(),
                capital,
                "Max Risk:".dimmed(),
                format!("{}{:.0}", csym, plan.max_risk).red(),
                "Exp P&L:".dimmed(),
                format!("+{}{:.0}", csym, plan.expected_profit)
                    .green()
                    .bold()
            );
            println!(
                "  {} Kelly fraction: {:.1}%",
                "→".dimmed(),
                plan.kelly_fraction * 100.0
            );
        }

        let total_profit: f64 = plans.iter().take(3).map(|p| p.expected_profit).sum();
        let total_risk: f64 = plans.iter().take(3).map(|p| p.max_risk).sum();
        println!("\n  {}", "─".repeat(60).dimmed());
        println!(
            "  {} Top 3 combined: {} potential | {} max risk",
            if total_profit >= target_profit {
                "✓".green().bold()
            } else {
                "→".yellow().bold()
            },
            format!("+{}{:.0}", csym, total_profit).green().bold(),
            format!("{}{:.0}", csym, total_risk).red()
        );
    }

    println!("\n  {}", "─".repeat(60).dimmed());
    println!(
        "  {}",
        "This is NOT financial advice. Trade at your own risk."
            .red()
            .italic()
    );
    println!();
    Ok(())
}

// ── AI-Powered Reports ──

pub async fn cmd_longterm(amount: Option<f64>, market: Market) -> Result<()> {
    market.require_bot_support("longterm")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;
    let csym = market.currency_symbol();
    let monthly = amount.unwrap_or(10000.0);

    print_header(&format!(
        "Long-Term Wealth Bot v2 — {} Market",
        market.label()
    ));
    println!(
        "  {} Scoring {} stocks across 6 pillars + Monte Carlo...\n",
        "⟳".yellow(),
        quotes.len()
    );

    // Fetch 1Y chart data for volatility analysis (for top candidates)
    let mut scores: Vec<longterm::LongTermScore> = Vec::new();
    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("");
        let hist = client.get_chart(sym, "1y", "1d").await.ok().and_then(|c| {
            c.indicators
                .quote
                .first()
                .and_then(|qi| qi.close.as_ref())
                .map(|c| c.iter().filter_map(|v| *v).collect::<Vec<f64>>())
        });
        scores.push(longterm::score_for_longterm(q, hist.as_deref()));
    }
    scores.sort_by(|a, b| {
        b.total_score
            .partial_cmp(&a.total_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Summary table with all 6 pillars
    print_section("Ranking (6-Pillar Score)");
    println!(
        "  {:<14} {:>5} {:>5} {:>5} {:>5} {:>5} {:>5} {:>6} {:>6} {:>8}",
        "Symbol".bold(),
        "Val".bold(),
        "Grw".bold(),
        "Qua".bold(),
        "Mom".bold(),
        "Div".bold(),
        "Saf".bold(),
        "Total".bold(),
        "Moat".bold(),
        "Risk".bold()
    );
    println!("  {}", "─".repeat(78).dimmed());

    for s in scores.iter().take(12) {
        let sc = if s.total_score >= 70.0 {
            format!("{:.0}", s.total_score).green().bold().to_string()
        } else if s.total_score >= 55.0 {
            format!("{:.0}", s.total_score).yellow().to_string()
        } else {
            format!("{:.0}", s.total_score).dimmed().to_string()
        };
        let moat_str = match s.moat {
            longterm::MoatRating::Wide => "Wide".green().to_string(),
            longterm::MoatRating::Narrow => "Nar".yellow().to_string(),
            longterm::MoatRating::None => "—".dimmed().to_string(),
        };
        let risk_str = match s.risk_tier {
            longterm::RiskTier::Conservative => "Cons".green().to_string(),
            longterm::RiskTier::Moderate => "Mod".yellow().to_string(),
            longterm::RiskTier::Aggressive => "Aggr".red().to_string(),
        };
        println!(
            "  {:<14} {:>5.0} {:>5.0} {:>5.0} {:>5.0} {:>5.0} {:>5.0} {:>6} {:>6} {:>8}",
            s.symbol.cyan(),
            s.valuation_score,
            s.growth_score,
            s.quality_score,
            s.momentum_score,
            s.dividend_score,
            s.safety_score,
            sc,
            moat_str,
            risk_str
        );
    }

    // Detailed top 3
    for (i, s) in scores.iter().take(3).enumerate() {
        println!();
        print_section(&format!(
            "#{} {} — {} [{}]",
            i + 1,
            s.symbol,
            s.name,
            s.moat
        ));
        print_kv("Price", &format!("{}{:.2}", csym, s.price));
        print_kv(
            "Score",
            &format!("{:.0}/100 ({})", s.total_score, s.risk_tier)
                .bold()
                .to_string(),
        );
        if let Some(peg) = s.peg_ratio {
            print_kv("PEG Ratio", &format!("{:.2}", peg));
        }
        if let Some(ey) = s.earnings_yield {
            print_kv("Earnings Yield", &format!("{:.1}%", ey));
        }

        if !s.reasons.is_empty() {
            println!();
            for r in &s.reasons {
                println!("    {} {}", "✓".green(), r);
            }
        }
        if !s.risk_flags.is_empty() {
            for r in &s.risk_flags {
                println!("    {} {}", "✗".red(), r);
            }
        }

        print_section("Projections");
        print_kv(
            "Est. Annual Return",
            &format!("{:.1}%", s.est_annual_return),
        );
        print_kv(
            "5-Year Projection",
            &format!("+{:.0}%", s.projected_5y_return)
                .green()
                .to_string(),
        );
        print_kv(
            "10-Year Projection",
            &format!("+{:.0}%", s.projected_10y_return)
                .green()
                .to_string(),
        );
        if s.drip_multiplier_10y > 1.01 {
            print_kv(
                "DRIP Boost (10Y)",
                &format!("{:.2}x from dividend reinvestment", s.drip_multiplier_10y),
            );
        }
        print_kv(
            &format!("SIP for {}10L/10Y", csym),
            &format!("{}{:.0}/month", csym, s.sip_monthly_10l_10y),
        );

        // Monte Carlo
        print_section("Monte Carlo (1000 sims, 5Y)");
        let mc_bar = |val: f64| -> String {
            if val >= 0.0 {
                format!("+{:.0}%", val).green().to_string()
            } else {
                format!("{:.0}%", val).red().to_string()
            }
        };
        print_kv("Best case (P90)", &mc_bar(s.monte_carlo_p90));
        print_kv("Median", &mc_bar(s.monte_carlo_median));
        print_kv("Worst case (P10)", &mc_bar(s.monte_carlo_p10));

        // SIP projection
        let inv_5y = monthly * 60.0;
        let est_5y = inv_5y * (1.0 + s.projected_5y_return / 100.0);
        let inv_10y = monthly * 120.0;
        let est_10y = inv_10y * (1.0 + s.projected_10y_return / 100.0) * s.drip_multiplier_10y;
        print_section("SIP Calculator");
        print_kv(
            &format!("{}{}/mo × 5Y", csym, monthly),
            &format!("{}{:.0} → {}{:.0}", csym, inv_5y, csym, est_5y)
                .green()
                .to_string(),
        );
        print_kv(
            &format!("{}{}/mo × 10Y", csym, monthly),
            &format!(
                "{}{:.0} → {}{:.0} (incl DRIP)",
                csym, inv_10y, csym, est_10y
            )
            .green()
            .bold()
            .to_string(),
        );
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
        let moat_tag = match s.moat {
            longterm::MoatRating::Wide => " [Wide Moat]".green().to_string(),
            longterm::MoatRating::Narrow => " [Narrow]".yellow().to_string(),
            _ => String::new(),
        };
        println!(
            "  {:<14} {}{:>8.0} ({:>4.1}%) {}{}",
            s.symbol.cyan(),
            csym,
            alloc,
            pct * 100.0,
            "█".repeat(bar_len).green(),
            moat_tag
        );
    }

    println!("\n  {}", "─".repeat(60).dimmed());
    println!(
        "  {}",
        "Long-term investing involves risk. Diversify across asset classes."
            .dimmed()
            .italic()
    );
    println!(
        "  {}",
        "Monte Carlo uses historical volatility. Past ≠ future."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

// ── Tax Calculator ──

pub async fn cmd_report(what: &str, market: Market) -> Result<()> {
    // Asking for a report is the opt-in: a configured cloud provider is used.
    let mut ai = crate::ai::AiClient::connect_explicit().await;
    if !ai.is_available().await {
        bail!("{}", ai.unavailable_reason());
    }

    let client = YahooClient::new().await?;

    match what {
        "intraday" => {
            market.require_bot_support("report intraday")?;
            println!(
                "  {} Generating intraday market notes with AI...\n",
                "⟳".yellow()
            );
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
            print_header("AI Intraday Market Notes");
            println!();
            print_ai_answer(&report);
            println!();
        }
        "longterm" => {
            println!(
                "  {} Generating long-term research notes...\n",
                "⟳".yellow()
            );
            market.require_bot_support("report longterm")?;
            let symbols = market.universe();
            let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
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
            print_header("AI Long-Term Research Notes");
            println!();
            print_ai_answer(&report);
            println!();
        }
        "portfolio" => {
            let portfolio = Portfolio::load()?;
            if portfolio.holdings.is_empty() {
                println!("  {}", "Portfolio is empty.".dimmed());
                return Ok(());
            }
            println!("  {} Analyzing your portfolio...\n", "⟳".yellow());
            let symbols: Vec<String> = portfolio
                .holdings
                .iter()
                .map(|h| h.symbol.clone())
                .collect();
            // Funds at their NAV and SGBs from NSE, like `portfolio`; a
            // missing price is N/A, never 0.
            let val = super::portfolio::value_holdings(&client, &symbols, market, None).await?;
            let mut data = String::new();
            for h in &portfolio.holdings {
                let price = val.price(&h.symbol);
                let pnl = match price {
                    Some(p) if h.avg_cost > 0.0 => {
                        format!("{:+.1}%", (p / h.avg_cost - 1.0) * 100.0)
                    }
                    _ => "N/A".into(),
                };
                let name = val
                    .mark(&h.symbol)
                    .and_then(|m| m.name.clone())
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default();
                data.push_str(&format!(
                    "{}{}: {} {}, avg={:.2}, now={}, P&L={}, P/E={}\n",
                    h.symbol,
                    name,
                    h.shares,
                    crate::offmarket::unit_word(&h.symbol),
                    h.avg_cost,
                    price.map_or("N/A".into(), |p| format!("{p:.2}")),
                    pnl,
                    val.quote(&h.symbol)
                        .and_then(|q| q.trailing_pe)
                        .map_or("N/A".into(), |v| format!("{:.1}", v)),
                ));
            }
            let report = ai.generate_portfolio_report(&data).await?;
            print_header("AI Portfolio Notes");
            println!();
            print_ai_answer(&report);
            println!();
        }
        "stock" => {
            println!("  Usage: stockwise report stock <SYMBOL>");
            println!("  Example: stockwise report stock RELIANCE");
        }
        _ => {
            // Treat as a stock symbol
            let resolved = market::resolve_symbol(what, market);
            println!(
                "  {} Analyzing {} with AI...\n",
                "⟳".yellow(),
                resolved.cyan()
            );
            let quotes = client.get_quote(&[resolved.as_str()]).await?;
            let q = quotes.first().context("Symbol not found")?;
            let stock_data = crate::ai::StockData::from_quote(q);
            let report = ai.analyze_stock(&stock_data).await?;
            print_header(&format!("AI Analysis: {}", resolved));
            println!();
            print_ai_answer(&report);
            println!();
        }
    }

    println!(
        "  {}",
        format!("Generated by {} ({}).", ai.provider().name(), ai.model())
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}
