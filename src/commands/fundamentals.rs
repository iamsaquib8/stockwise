//! Company fundamentals: analysis, comparisons, peers, dividends, insider activity, and the all-in-one `deep` report.

use super::*;
use crate::results;

/// The Altman line for `deep` and `analyze`. The industrial Z-score does not
/// describe banks and lenders, so they get a note instead of a zone label.
fn altman_summary(a: &scorecard::AltmanScorecard) -> Option<String> {
    if a.is_financial {
        return Some("n/a for banks and lenders".to_string());
    }
    let Some(z) = a.z_score else {
        return Some(format!("n/a (missing {})", a.missing_factors.join(", ")));
    };
    Some(match a.zone_label.as_deref() {
        Some(label) => format!("{z:.2} — {label}"),
        None => format!("{z:.2}"),
    })
}

fn print_unified_research_footer(r_opt: Option<&crate::research::Research>, q: &crate::api::Quote) {
    if let Some(r) = r_opt {
        let sc = scorecard::generate_scorecard_report(r);
        print_section("Accounting Scorecards (Piotroski & Altman)");
        print_kv(
            "Piotroski F-Score",
            &format!(
                "{}/{} evaluated ({})",
                sc.piotroski.score, sc.piotroski.evaluated, sc.piotroski.rating_label
            ),
        );
        if let Some(text) = altman_summary(&sc.altman) {
            print_kv("Altman Z-Score", &text);
        }

        let cards = insights::generate_rich_insights(r);
        print_section("Key Insights");
        if cards.is_empty() {
            for insight in insights::generate_insights(q) {
                println!("  {} {}", "→".cyan(), insight);
            }
        } else {
            for c in cards.iter().take(6) {
                let metric = c
                    .metric
                    .as_deref()
                    .map(|m| format!(" [{m}]"))
                    .unwrap_or_default();
                println!(
                    "  {} {}: {}{}",
                    c.impact.icon(),
                    c.title.bold(),
                    c.description,
                    metric.cyan()
                );
            }
        }

        let v = crate::verdict::generate_verdict(r, &cards);
        println!();
        println!("{}", "─".repeat(60).dimmed());
        crate::verdict::print_snapshot(&v, &r.symbol);
        println!("{}", "─".repeat(60).dimmed());
    } else {
        print_section("Insights");
        for insight in insights::generate_insights(q) {
            println!("  {} {}", "→".cyan(), insight);
        }
        println!();
        println!("{}", "─".repeat(60).dimmed());
        println!("{}", insights::overall_verdict(q));
        println!("{}", "─".repeat(60).dimmed());
    }
}

pub async fn cmd_analyze(symbol: &str, market: Market, json: bool) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let mut quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes
        .first_mut()
        .context("No data found for this symbol")?;
    crate::research::enrich_quote(&client, q, false).await;
    let q = &*q;
    let r_opt =
        crate::research::fetch(&client, symbol, market, 60, crate::research::Needs::VERDICT)
            .await
            .ok();

    if json {
        let cards = r_opt
            .as_ref()
            .map(insights::generate_rich_insights)
            .unwrap_or_default();
        let verdict = r_opt
            .as_ref()
            .map(|r| crate::verdict::generate_verdict(r, &cards));
        let scorecard = r_opt.as_ref().map(scorecard::generate_scorecard_report);
        let out = serde_json::json!({
            "symbol": resolved,
            "quote": market::quote_json(q),
            "verdict": verdict,
            "scorecard": scorecard,
            "insights": cards,
        });
        return output::print_json(&out);
    }

    let name = q
        .long_name
        .as_deref()
        .or(q.short_name.as_deref())
        .unwrap_or("Unknown");
    let price = q.regular_market_price.unwrap_or(0.0);
    let change = q.regular_market_change.unwrap_or(0.0);
    let change_pct = q.regular_market_change_percent.unwrap_or(0.0);
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);

    print_header(&format!("Deep Analysis: {} — {}", resolved, name));
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
        &match (q.market_cap, market::cap_badge(q)) {
            (Some(v), Some(b)) => format!("{}  {}", format_large_number(v, cur), b.dimmed()),
            (Some(v), None) => format_large_number(v, cur),
            (None, Some(b)) => format!("N/A  {}", b.dimmed()),
            (None, None) => "N/A".to_string(),
        },
    );
    if let Some(exchange) = &q.exchange {
        print_kv("Exchange", exchange);
    }

    // Valuation
    print_section("Valuation Metrics");
    print_kv("P/E (Trailing)", &format_optional_f64(q.trailing_pe, "x"));
    print_kv("P/E (Forward)", &format_optional_f64(q.forward_pe, "x"));
    print_kv("Price/Book", &format_optional_f64(q.price_to_book, "x"));
    print_kv(
        "EV/Revenue",
        &format_optional_f64(q.enterprise_to_revenue, "x"),
    );
    print_kv(
        "EV/EBITDA",
        &format_optional_f64(q.enterprise_to_ebitda, "x"),
    );

    // Profitability
    print_section("Profitability & Growth");
    print_kv("Profit Margin", &format_optional_pct(q.profit_margins));
    print_kv("Return on Equity", &format_optional_pct(q.return_on_equity));
    print_kv("Revenue Growth", &format_optional_pct(q.revenue_growth));
    print_kv(
        "Earnings Growth (Q)",
        &format_optional_pct(q.earnings_quarterly_growth),
    );
    print_kv(
        "EPS (TTM)",
        &format_optional_f64(q.eps_trailing_twelve_months, ""),
    );
    print_kv("EPS (Forward)", &format_optional_f64(q.eps_forward, ""));
    print_kv(
        "Revenue/Share",
        &format_optional_f64(q.revenue_per_share, ""),
    );

    // Financial health
    print_section("Financial Health");
    print_kv("Debt/Equity", &format_optional_f64(q.debt_to_equity, ""));
    print_kv("Current Ratio", &format_optional_f64(q.current_ratio, "x"));
    print_kv("Book Value", &format_optional_f64(q.book_value, ""));

    // Analyst Ratings (skipped when Yahoo has no coverage)
    if has_analyst_data(q) {
        print_section("Analyst Ratings");
    }
    if let Some(rec) = q.recommendation_mean {
        print_kv(
            "Analyst Rating",
            &format!("{}  {}", mean_rating_text(rec), rating_bar(5.0 - rec, 4.0)),
        );
    }
    if let Some(target) = q.target_mean_price {
        let upside = q.regular_market_price.map(|p| ((target - p) / p) * 100.0);
        print_kv(
            "Analyst Mean Price Estimate",
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

    print_unified_research_footer(r_opt.as_ref(), q);

    // AI-powered analysis (with --ai, if the provider is available)
    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await {
        print_section(&format!("AI Analysis ({})", ai.provider().name()));
        let stock_data = crate::ai::StockData::from_quote(q);
        match ai.analyze_stock(&stock_data).await {
            Ok(analysis) => print_ai_answer(&analysis),
            Err(_) => {
                println!("  {}", "AI analysis unavailable.".dimmed());
            }
        }
    }

    let bare = resolved.strip_suffix(".NS").unwrap_or(&resolved);
    println!();
    print_hint(&format!(
        "Drill down: stockwise verdict {bare} · stockwise score {bare} · stockwise insights {bare} · stockwise events {bare}"
    ));
    println!(
        "\n  {}",
        "Disclaimer: This is not financial advice. Do your own research."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

/// One stock in `compare --json/--csv`: a row per symbol, a column per
/// metric (the table shows the transpose). Ratios are fractions as Yahoo
/// reports them (`dividend_yield` 0.012 = 1.2%).
#[derive(Debug, Serialize)]
pub(crate) struct CompareRow {
    pub(crate) symbol: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) price: Option<f64>,
    pub(crate) change_percent: Option<f64>,
    pub(crate) market_cap: Option<f64>,
    pub(crate) cap_tier: &'static str,
    pub(crate) pe_ttm: Option<f64>,
    pub(crate) pe_forward: Option<f64>,
    pub(crate) eps_ttm: Option<f64>,
    pub(crate) price_to_book: Option<f64>,
    pub(crate) dividend_yield: Option<f64>,
    pub(crate) profit_margin: Option<f64>,
    pub(crate) revenue_growth: Option<f64>,
    pub(crate) debt_to_equity: Option<f64>,
    pub(crate) beta: Option<f64>,
    pub(crate) year_high: Option<f64>,
    pub(crate) year_low: Option<f64>,
    pub(crate) sma_50: Option<f64>,
    /// Analysts' mean rating, 1 (most positive) to 5.
    pub(crate) analyst_mean_rating: Option<f64>,
}

impl CsvRow for CompareRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "currency",
        "price",
        "change_percent",
        "market_cap",
        "cap_tier",
        "pe_ttm",
        "pe_forward",
        "eps_ttm",
        "price_to_book",
        "dividend_yield",
        "profit_margin",
        "revenue_growth",
        "debt_to_equity",
        "beta",
        "year_high",
        "year_low",
        "sma_50",
        "analyst_mean_rating",
    ];
}

impl From<&crate::api::Quote> for CompareRow {
    fn from(q: &crate::api::Quote) -> Self {
        Self {
            symbol: q.symbol.clone(),
            name: q.short_name.clone().or_else(|| q.long_name.clone()),
            currency: q.currency.clone(),
            price: q.regular_market_price,
            change_percent: q.regular_market_change_percent,
            market_cap: q.market_cap,
            cap_tier: market::cap_tier_of(q).as_str(),
            pe_ttm: q.trailing_pe,
            pe_forward: q.forward_pe,
            eps_ttm: q.eps_trailing_twelve_months,
            price_to_book: q.price_to_book,
            dividend_yield: q.trailing_annual_dividend_yield,
            profit_margin: q.profit_margins,
            revenue_growth: q.revenue_growth,
            debt_to_equity: q.debt_to_equity,
            beta: q.beta,
            year_high: q.fifty_two_week_high,
            year_low: q.fifty_two_week_low,
            sma_50: q.fifty_day_average,
            analyst_mean_rating: q.recommendation_mean,
        }
    }
}

/// Whether the quote carries any analyst figure worth a section.
fn has_analyst_data(q: &crate::api::Quote) -> bool {
    q.recommendation_mean.is_some()
        || q.target_mean_price.is_some()
        || q.number_of_analyst_opinions.is_some()
}

/// Notional sum and its label for the `deep` dividend-income row.
///
/// One lakh is the familiar unit for an Indian investor; elsewhere ten
/// thousand keeps the number readable without implying rupees.
fn dividend_income_basis(currency: Option<&str>) -> (&'static str, f64) {
    if currency == Some("INR") {
        ("₹1L", 100_000.0)
    } else {
        ("$10k", 10_000.0)
    }
}

pub async fn cmd_compare(symbols: &[String], market: Market, fmt: Format) -> Result<()> {
    let resolved = resolve_symbols(symbols, market);
    let client = YahooClient::new().await?;
    let sym_refs: Vec<&str> = resolved.iter().map(|s| s.as_str()).collect();
    let mut quotes = client.get_quote(&sym_refs).await?;

    if quotes.is_empty() {
        bail!(
            "No quotes for {}{}",
            resolved.join(", "),
            market::not_found_hint(market)
        );
    }
    for q in &mut quotes {
        crate::research::enrich_quote(&client, q, false).await;
    }
    if !fmt.is_table() {
        let rows: Vec<CompareRow> = quotes.iter().map(CompareRow::from).collect();
        return match fmt {
            Format::Csv => output::print_csv(&rows),
            _ => output::print_json(&rows),
        };
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
                q.market_cap.map_or("N/A".into(), |v| {
                    format_large_number(v, q.currency.as_deref())
                })
            }),
        ),
        (
            "Cap tier",
            Box::new(|q| {
                market::cap_badge(q)
                    .map_or("N/A".into(), |b| b.trim_matches(['[', ']']).to_string())
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
            Box::new(|q| {
                q.price_to_book
                    .map_or("N/A".into(), |v| format!("{:.2}", v))
            }),
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
            Box::new(|q| {
                q.debt_to_equity
                    .map_or("N/A".into(), |v| format!("{:.0}", v))
            }),
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
            "Analyst Mean Rating (1 = most positive)",
            Box::new(|q| {
                q.recommendation_mean
                    .map_or("N/A".into(), |r| format!("{r:.1}/5"))
            }),
        ),
    ];

    for (label, func) in &rows {
        let cells: Vec<String> = quotes.iter().map(func).collect();
        // Leave out rows no stock has a value for.
        if cells.iter().all(|c| c == "N/A") {
            continue;
        }
        print!("  {:<22}", label.dimmed());
        for c in &cells {
            print!(" {:>14}", c);
        }
        println!();
    }

    // AI comparison verdict
    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await {
        let mut data = String::new();
        for q in &quotes {
            let sym = q.symbol.as_deref().unwrap_or("?");
            data.push_str(&format!(
                "{}: price={:.2}, P/E={}, fwdP/E={}, chg={:+.2}%\n",
                sym,
                q.regular_market_price.unwrap_or(0.0),
                q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.forward_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.regular_market_change_percent.unwrap_or(0.0)
            ));
        }
        print_section(&format!("AI Comparison ({})", ai.provider().name()));
        if let Ok(comparison) = ai.compare_stocks(&data).await {
            print_ai_answer(&comparison);
        }
    }

    println!();
    Ok(())
}

/// Peers shown, and the most candidates looked up to find them.
const PEER_COUNT: usize = 8;
const PEER_LOOKUPS: usize = 40;

pub async fn cmd_peers(symbol: &str, market: Market) -> Result<()> {
    market.require_stocks("peers")?;
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let mut quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first_mut().context("Symbol not found")?;
    // The v7 quote has no sector; it comes from quoteSummary's assetProfile.
    crate::research::enrich_quote(&client, q, true).await;
    let q = &*q;
    let Some(sector) = q.sector.as_deref() else {
        bail!(
            "Yahoo has no sector for {}, so peers cannot be matched",
            resolved
        );
    };

    // Walk the popular stocks, largest first, and keep those in the same
    // sector. Each lookup is one cached quoteSummary call, so stop early.
    let all_symbols = market.universe();
    let all_refs: Vec<&str> = all_symbols.iter().map(String::as_str).collect();
    let mut all_quotes = client.get_quote(&all_refs).await?;
    all_quotes.sort_by(|a, b| {
        b.market_cap
            .unwrap_or(0.0)
            .partial_cmp(&a.market_cap.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let mut peers: Vec<crate::api::Quote> = Vec::new();
    for mut p in all_quotes
        .into_iter()
        .filter(|p| p.symbol.as_deref() != Some(&resolved))
        .take(PEER_LOOKUPS)
    {
        crate::research::enrich_quote(&client, &mut p, true).await;
        if p.sector.as_deref() == Some(sector) {
            peers.push(p);
            if peers.len() >= PEER_COUNT {
                break;
            }
        }
    }

    print_header(&format!("Peer Comparison: {} ({})", resolved, sector));

    // Include the target stock first
    let mut all_peers = vec![q];
    all_peers.extend(peers.iter());

    println!(
        "  {:<14} {:>10} {:>8} {:>8} {:>8} {:>10}",
        "Symbol".bold(),
        "Price".bold(),
        "P/E".bold(),
        "P/B".bold(),
        "Chg%".bold(),
        "Mkt Cap".bold()
    );
    println!("  {}", "─".repeat(62).dimmed());

    for p in &all_peers {
        let sym = p.symbol.as_deref().unwrap_or("???");
        let cur = p.currency.as_deref();
        let is_target = sym == resolved;
        let name = if is_target {
            format!("{} ←", sym).cyan().bold().to_string()
        } else {
            sym.cyan().to_string()
        };
        let pct = p.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 {
            format!("+{:.2}%", pct).green().to_string()
        } else {
            format!("{:.2}%", pct).red().to_string()
        };
        println!(
            "  {:<14} {:>10} {:>8} {:>8} {:>8} {:>10}",
            name,
            format_price(p.regular_market_price.unwrap_or(0.0), cur),
            p.trailing_pe.map_or("—".into(), |v| format!("{:.1}", v)),
            p.price_to_book.map_or("—".into(), |v| format!("{:.1}", v)),
            pct_str,
            p.market_cap
                .map_or("—".into(), |v| format_large_number(v, cur))
        );
    }
    if peers.is_empty() {
        println!("\n  {}", "No peers found in the same sector.".dimmed());
    }
    println!();
    Ok(())
}

// ── Dividends ──

/// Income and DRIP projections behind `dividends`, from the price and the
/// trailing yield (a fraction). `drip` starts from 1,000 shares and
/// reinvests every year's dividend at today's price.
pub(crate) fn dividend_json(
    symbol: &str,
    currency: Option<&str>,
    price: f64,
    div_yield: f64,
) -> serde_json::Value {
    let div_rate = div_yield * price;
    let income: Vec<serde_json::Value> = [100000.0, 500000.0, 1000000.0]
        .into_iter()
        .map(|invested| {
            let shares = (invested / price).floor();
            let annual = shares * div_rate;
            serde_json::json!({
                "invested": invested,
                "shares": shares,
                "annual_income": round(annual, 2),
                "monthly_income": round(annual / 12.0, 2),
            })
        })
        .collect();
    let mut drip = Vec::new();
    let initial = 1000.0;
    let mut shares = initial;
    for year in 1..=10 {
        shares += shares * div_rate / price;
        if year == 1 || year == 5 || year == 10 {
            drip.push(serde_json::json!({
                "year": year,
                "shares": round(shares, 4),
                "growth_percent": round((shares / initial - 1.0) * 100.0, 2),
            }));
        }
    }
    let pays = div_yield > 0.0;
    serde_json::json!({
        "symbol": symbol,
        "currency": currency,
        "price": price,
        "pays_dividend": pays,
        "dividend_yield_percent": round(div_yield * 100.0, 2),
        "annual_dividend_per_share": round(div_rate, 4),
        "income": if pays { income } else { Vec::new() },
        "drip": if pays { drip } else { Vec::new() },
    })
}

pub async fn cmd_dividends(symbol: &str, market: Market, fmt: Format) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let price = q
        .regular_market_price
        .filter(|p| *p > 0.0)
        .with_context(|| format!("No live price for {}", resolved))?;
    let div_yield = q.trailing_annual_dividend_yield.unwrap_or(0.0);
    let report = dividend_json(&resolved, cur, price, div_yield);
    if fmt == Format::Json {
        return output::print_json(&report);
    }

    print_header(&format!("Dividend Analysis: {}", resolved));
    print_kv("Price", &format_price(price, cur));

    if div_yield > 0.0 {
        print_kv(
            "Annual Dividend",
            &format!("{}{:.2}/share", csym, div_yield * price),
        );
        print_kv(
            "Dividend Yield",
            &format!("{:.2}%", div_yield * 100.0).green().to_string(),
        );

        // Dividend income projection
        print_section("Income Projection");
        for row in report["income"].as_array().into_iter().flatten() {
            let f = |k: &str| row[k].as_f64().unwrap_or_default();
            print_kv(
                &format!("{}{:.0} invested", csym, f("invested")),
                &format!(
                    "{}{:.0}/year ({}{:.0}/month)",
                    csym,
                    f("annual_income"),
                    csym,
                    f("monthly_income")
                ),
            );
        }

        // DRIP projection (dividend reinvestment)
        print_section("DRIP Growth (10 years)");
        for row in report["drip"].as_array().into_iter().flatten() {
            print_kv(
                &format!("Year {}", row["year"]),
                &format!(
                    "{:.2} shares (+{:.1}% from DRIP)",
                    row["shares"].as_f64().unwrap_or_default(),
                    row["growth_percent"].as_f64().unwrap_or_default()
                ),
            );
        }
    } else {
        println!("  {}", "This stock does not pay dividends.".dimmed());
    }
    println!();
    Ok(())
}

// ── Insider Activity (via Yahoo search news) ──

pub async fn cmd_insider(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let name = q
        .short_name
        .as_deref()
        .or(q.long_name.as_deref())
        .unwrap_or("Unknown");

    // Yahoo doesn't have a free insider endpoint, but we can show key holder metrics
    print_header(&format!("Insider & Institutional: {}", resolved));
    print_kv("Company", name);
    print_kv(
        "Price",
        &format_price(q.regular_market_price.unwrap_or(0.0), q.currency.as_deref()),
    );

    // Show what we have from the quote
    if let Some(rec) = q.recommendation_mean {
        print_kv("Analyst Rating", &mean_rating_text(rec));
    }
    if let Some(n) = q.number_of_analyst_opinions {
        print_kv("# Analysts", &n.to_string());
    }
    if let Some(target) = q.target_mean_price {
        let upside = q
            .regular_market_price
            .map(|p| ((target - p) / p) * 100.0)
            .unwrap_or(0.0);
        print_kv(
            "Analyst Mean Price Estimate",
            &format!(
                "{}{:.2} ({:+.1}% from price)",
                market::currency_symbol(q.currency.as_deref()),
                target,
                upside
            ),
        );
    }

    // Fetch insider-related news from multiple sources concurrently
    let base = resolved.split('.').next().unwrap_or(&resolved).to_string();
    let yahoo_insider_q = format!("{} insider", resolved);
    let insider_query = format!("{} insider buying selling", base);
    let analyst_query = format!("{} analyst rating upgrade downgrade", base);

    let (yahoo_insider, google_insider, google_analyst) = tokio::join!(
        fetch_yahoo_json_news(&client, &yahoo_insider_q, 8),
        fetch_google_news_rss(&client, &insider_query),
        fetch_google_news_rss(&client, &analyst_query),
    );

    let all_insider: Vec<NewsItem> = [yahoo_insider, google_insider, google_analyst].concat();
    let collated = collate_news(all_insider);

    if !collated.is_empty() {
        print_section("Recent Insider & Analyst News");
        print_news_items(&collated, 8);
    }
    println!();
    Ok(())
}

// ── IPO Calendar ──

/// Open, high, low, close and volume series of equal length. Yahoo can leave
/// a null in one field of a bar (usually a bar still forming), so a bar
/// missing any price is dropped from every series rather than from one.
fn aligned_ohlcv(
    chart: &crate::api::ChartData,
) -> (Vec<f64>, Vec<f64>, Vec<f64>, Vec<f64>, Vec<u64>) {
    let mut out = (vec![], vec![], vec![], vec![], vec![]);
    let Some(q) = chart.indicators.quote.first() else {
        return out;
    };
    let at = |v: &Option<Vec<Option<f64>>>, i: usize| {
        v.as_ref().and_then(|v| v.get(i).copied().flatten())
    };
    let n = q.close.as_ref().map_or(0, |c| c.len());
    for i in 0..n {
        let (Some(o), Some(h), Some(l), Some(c)) = (
            at(&q.open, i),
            at(&q.high, i),
            at(&q.low, i),
            at(&q.close, i),
        ) else {
            continue;
        };
        out.0.push(o);
        out.1.push(h);
        out.2.push(l);
        out.3.push(c);
        out.4.push(
            q.volume
                .as_ref()
                .and_then(|v| v.get(i).copied().flatten())
                .unwrap_or(0),
        );
    }
    out
}

pub async fn cmd_deep(symbol: &str, period: &str, market: Market, json: bool) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let cur_ref = &resolved;

    // ── 1. Quote + Fundamentals ──
    let mut quotes = client.get_quote(&[cur_ref.as_str()]).await?;
    let q = quotes.first_mut().context("Symbol not found")?;
    crate::research::enrich_quote(&client, q, false).await;
    let q = &*q;
    let r_opt = crate::research::fetch(&client, symbol, market, 60, crate::research::Needs::ALL)
        .await
        .ok();

    if json {
        let cards = r_opt
            .as_ref()
            .map(insights::generate_rich_insights)
            .unwrap_or_default();
        let verdict = r_opt
            .as_ref()
            .map(|r| crate::verdict::generate_verdict(r, &cards));
        let scorecard = r_opt.as_ref().map(scorecard::generate_scorecard_report);
        let out = serde_json::json!({
            "symbol": resolved,
            "period": period,
            "quote": market::quote_json(q),
            "verdict": verdict,
            "scorecard": scorecard,
            "insights": cards,
        });
        return output::print_json(&out);
    }

    let cur = q.currency.as_deref();
    let csym = market::currency_symbol(cur);
    let name = q
        .long_name
        .as_deref()
        .or(q.short_name.as_deref())
        .unwrap_or("Unknown");
    let price = q.regular_market_price.unwrap_or(0.0);
    let change = q.regular_market_change.unwrap_or(0.0);
    let change_pct = q.regular_market_change_percent.unwrap_or(0.0);

    print_header(&format!("DEEP DIVE: {} — {}", resolved, name));
    println!(
        "  {}  {}\n",
        format_price(price, cur).bold(),
        format_change(change, change_pct)
    );

    // Overview
    print_section("Company");
    if let Some(s) = &q.sector {
        print_kv("Sector", s);
    }
    if let Some(i) = &q.industry {
        print_kv("Industry", i);
    }
    if let Some(e) = &q.exchange {
        print_kv("Exchange", e);
    }
    print_kv(
        "Market Cap",
        &match (q.market_cap, market::cap_badge(q)) {
            (Some(v), Some(b)) => format!("{}  {}", format_large_number(v, cur), b.dimmed()),
            (Some(v), None) => format_large_number(v, cur),
            (None, Some(b)) => format!("N/A  {}", b.dimmed()),
            (None, None) => "N/A".into(),
        },
    );

    // Price
    print_section("Price Action");
    print_kv(
        "Open",
        &format_price(q.regular_market_open.unwrap_or(0.0), cur),
    );
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
        "Avg Vol (3M)",
        &format_volume(q.average_daily_volume_3_month.unwrap_or(0)),
    );

    // Valuation
    print_section("Valuation");
    print_kv("P/E (TTM)", &format_optional_f64(q.trailing_pe, "x"));
    print_kv("P/E (Forward)", &format_optional_f64(q.forward_pe, "x"));
    print_kv("P/B", &format_optional_f64(q.price_to_book, "x"));
    print_kv(
        "EV/Revenue",
        &format_optional_f64(q.enterprise_to_revenue, "x"),
    );
    print_kv(
        "EV/EBITDA",
        &format_optional_f64(q.enterprise_to_ebitda, "x"),
    );
    if let Some(pe) = q.trailing_pe
        && let Some(g) = q.earnings_quarterly_growth
        && g > 0.01
    {
        print_kv("PEG Ratio", &format!("{:.2}", pe / (g * 100.0)));
    }

    // Profitability
    print_section("Profitability & Growth");
    print_kv(
        "EPS (TTM)",
        &format_optional_f64(q.eps_trailing_twelve_months, ""),
    );
    print_kv("EPS (Forward)", &format_optional_f64(q.eps_forward, ""));
    print_kv("Profit Margin", &format_optional_pct(q.profit_margins));
    print_kv("Return on Equity", &format_optional_pct(q.return_on_equity));
    print_kv("Revenue Growth", &format_optional_pct(q.revenue_growth));
    print_kv(
        "Earnings Growth (Q)",
        &format_optional_pct(q.earnings_quarterly_growth),
    );

    // Financial Health
    print_section("Financial Health");
    print_kv("Debt/Equity", &format_optional_f64(q.debt_to_equity, ""));
    print_kv("Current Ratio", &format_optional_f64(q.current_ratio, "x"));
    print_kv("Book Value", &format_optional_f64(q.book_value, ""));
    print_kv("Beta", &format_optional_f64(q.beta, ""));

    // Dividends
    if let Some(dy) = q.trailing_annual_dividend_yield
        && dy > 0.0
    {
        print_section("Dividend");
        print_kv("Yield", &format!("{:.2}%", dy * 100.0));
        let annual = dy * price;
        print_kv("Annual/Share", &format!("{}{:.2}", csym, annual));
        let (income_label, notional) = dividend_income_basis(cur);
        print_kv(
            &format!("Income on {income_label}"),
            &format!("{}{:.0}/year", csym, notional * dy),
        );
    }

    // Analyst (skipped when Yahoo has no coverage)
    if has_analyst_data(q) {
        print_section("Analyst Ratings");
    }
    if let Some(rec) = q.recommendation_mean {
        print_kv(
            "Analyst Rating",
            &format!("{}  {}", mean_rating_text(rec), rating_bar(5.0 - rec, 4.0)),
        );
    }
    if let Some(target) = q.target_mean_price {
        let upside = q
            .regular_market_price
            .map(|p| ((target - p) / p) * 100.0)
            .unwrap_or(0.0);
        let u = if upside >= 0.0 {
            format!("+{:.1}%", upside).green().to_string()
        } else {
            format!("{:.1}%", upside).red().to_string()
        };
        print_kv(
            "Analyst Mean Price Estimate",
            &format!("{}{:.2} ({})", csym, target, u),
        );
    }
    if let Some(n) = q.number_of_analyst_opinions {
        print_kv("# Analysts", &n.to_string());
    }

    // ── 2. Technical Indicators ──
    let chart = client.get_chart(&resolved, period, "1d").await?;
    let (opens, highs, lows, closes, volumes) = aligned_ohlcv(&chart);

    if closes.len() >= 20 {
        // Chart
        print_section(&format!("Price Chart ({})", period));
        let color = if *closes.last().unwrap() >= closes[0] {
            "green"
        } else {
            "red"
        };
        for line in charts::line_chart(&closes, 55, 8, color, "") {
            println!("{}", line);
        }

        // Key technicals
        print_section("Technical Indicators");
        if let Some(rsi) = technical::rsi(&closes, 14) {
            let r = if rsi >= 70.0 {
                format!("{:.1}", rsi).red().to_string()
            } else if rsi <= 30.0 {
                format!("{:.1}", rsi).green().to_string()
            } else {
                format!("{:.1}", rsi).to_string()
            };
            print_kv("RSI (14)", &r);
            println!("{}", charts::rsi_gauge(rsi));
        }
        for period in [20, 50, 200] {
            if let Some(ma) = technical::sma(&closes, period) {
                let sig = if price > ma {
                    "▲ Above".green().to_string()
                } else {
                    "▼ Below".red().to_string()
                };
                print_kv(
                    &format!("SMA {}", period),
                    &format!("{}{:.2}  {}", csym, ma, sig),
                );
            }
        }
        if let Some((_, _, hist)) = technical::macd(&closes) {
            let h = if hist > 0.0 {
                format!("{:.2} Bullish", hist).green().to_string()
            } else {
                format!("{:.2} Bearish", hist).red().to_string()
            };
            print_kv("MACD Histogram", &h);
        }
        if let Some((upper, middle, lower)) = technical::bollinger_bands(&closes, 20) {
            print_kv(
                "Bollinger",
                &format!(
                    "{}{:.2} / {}{:.2} / {}{:.2}",
                    csym, lower, csym, middle, csym, upper
                ),
            );
        }
        if let Some(atr) = technical::atr(&highs, &lows, &closes, 14) {
            print_kv(
                "ATR (14)",
                &format!("{}{:.2} ({:.2}%)", csym, atr, (atr / price) * 100.0),
            );
        }
        if let Some(vwap) = technical::vwap(&highs, &lows, &closes, &volumes) {
            let sig = if price > vwap {
                "Above".green().to_string()
            } else {
                "Below".red().to_string()
            };
            print_kv("VWAP", &format!("{}{:.2} ({})", csym, vwap, sig));
        }

        // Support/Resistance
        // Bars with a missing close (today's, while the market is open) are
        // dropped from `closes` only, so index each series from its own end.
        // Levels come from the last completed session, not today's live bar.
        if let Some(p) = chart.prior_session(
            chrono::Utc::now(),
            market::market_of_symbol(&resolved, market),
        ) {
            let (pivot, r1, r2, _r3, s1, s2, _s3) = technical::pivot_points(p.high, p.low, p.close);
            print_section(&format!(
                "Support & Resistance (from the {} session)",
                p.date.format("%d %b")
            ));
            print_kv("R2", &format!("{}{:.2}", csym, r2).red().to_string());
            print_kv("R1", &format!("{}{:.2}", csym, r1).red().to_string());
            print_kv("Pivot", &format!("{}{:.2}", csym, pivot).bold().to_string());
            print_kv("S1", &format!("{}{:.2}", csym, s1).green().to_string());
            print_kv("S2", &format!("{}{:.2}", csym, s2).green().to_string());
        }

        // Fibonacci
        if let Some((sh, sl)) = technical::find_swing_points(&highs, &lows, 60.min(highs.len())) {
            let fibs = technical::fibonacci_levels(sh, sl);
            print_section("Fibonacci (3M swing)");
            for (name, val) in [
                ("23.6%", fibs[0]),
                ("38.2%", fibs[1]),
                ("50.0%", fibs[2]),
                ("61.8%", fibs[3]),
            ] {
                let marker = if (price - val).abs() / price < 0.01 {
                    " ← HERE".yellow().bold().to_string()
                } else {
                    String::new()
                };
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
            print_kv(
                "Max Drawdown",
                &format!("-{:.1}%", mdd * 100.0).red().to_string(),
            );
        }
        if let Some(var95) = technical::value_at_risk(&closes, 0.95) {
            print_kv("Daily VaR (95%)", &format!("{:.2}%", var95 * 100.0));
        }

        // Volume
        let obv = technical::obv(&closes, &volumes);
        if obv.len() > 20 {
            let obv_trend = if obv.last() > obv.get(obv.len().saturating_sub(20)) {
                "Accumulation".green().to_string()
            } else {
                "Distribution".red().to_string()
            };
            print_kv("OBV Trend", &obv_trend);
        }

        // Gaps
        let gaps = technical::detect_gaps(&opens, &highs, &lows, &closes);
        let unfilled: Vec<_> = gaps.iter().filter(|g| !g.filled).collect();
        if !unfilled.is_empty() {
            print_section(&format!("Open Gaps ({})", unfilled.len()));
            for g in unfilled.iter().rev().take(3) {
                let dir = match g.gap_type {
                    technical::GapType::Up => "UP".green().to_string(),
                    technical::GapType::Down => "DN".red().to_string(),
                };
                println!(
                    "  {} {}{:.2} — {}{:.2}",
                    dir, csym, g.gap_low, csym, g.gap_high
                );
            }
        }
    }

    // ── 3. Unified Scorecards, Rich Insights & 6-Pillar Verdict ──
    print_unified_research_footer(r_opt.as_ref(), q);

    // ── 4. AI Analysis ──
    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await {
        print_section(&format!("AI Analysis ({})", ai.provider().name()));
        let stock_data = crate::ai::StockData::from_quote(q);
        match ai.analyze_stock(&stock_data).await {
            Ok(analysis) => print_ai_answer(&analysis),
            Err(_) => {
                println!("  {}", "AI unavailable.".dimmed());
            }
        }
    }

    let bare = resolved.strip_suffix(".NS").unwrap_or(&resolved);
    println!();
    print_hint(&format!(
        "Drill down: stockwise verdict {bare} · stockwise score {bare} · stockwise insights {bare} · stockwise events {bare}"
    ));
    println!(
        "\n  {}",
        "Not financial advice. Do your own research."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

/// Transparent accounting scorecards: Piotroski 9-Point F-Score and Altman Z-Score
pub async fn cmd_fundamental_score(
    symbol: &str,
    market: Market,
    json: bool,
    explain: bool,
) -> Result<()> {
    let explain = explain || crate::config::effective_profile().auto_explain;
    let client = YahooClient::new().await?;
    let r = crate::research::fetch(
        &client,
        symbol,
        market,
        60,
        crate::research::Needs::STATEMENTS,
    )
    .await?;
    let report = scorecard::generate_scorecard_report(&r);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        scorecard::print_scorecard_report(&report);
        if explain {
            print_section("How to Read Accounting Scorecards (--explain)");
            println!(
                "  • {}: Tests 9 binary criteria across Profitability (4),",
                "Piotroski F-Score (0–9)".bold()
            );
            println!(
                "    Leverage/Liquidity (3), and Operating Efficiency (2). 7–9 = Strong, 0–3 = Weak."
            );
            println!(
                "  • {}: Combines working capital, retained earnings, EBIT,",
                "Altman Z-Score".bold()
            );
            println!(
                "    market cap, and sales relative to assets/liabilities to measure bankruptcy risk."
            );
            println!("    Z > 2.99 = Safe Zone, 1.81–2.99 = Grey Zone, < 1.81 = Distress Zone.\n");
        }
    }
    Ok(())
}

// ── Results ──

/// Signed money, e.g. `-₹1.20K Cr`.
fn money(v: Option<f64>, currency: Option<&str>) -> String {
    match v {
        None => "-".into(),
        Some(v) if v < 0.0 => format!("-{}", format_large_number(-v, currency)),
        Some(v) => format_large_number(v, currency),
    }
}

/// A number with `places` decimals, or `-`.
fn num(v: Option<f64>, places: usize) -> String {
    v.map_or("-".into(), |v| format!("{v:.places$}"))
}

/// A number with its sign and `suffix`, or `-`.
fn signed(v: Option<f64>, places: usize, suffix: &str) -> String {
    v.map_or("-".into(), |v| format!("{v:+.places$}{suffix}"))
}

/// Print a table with a label column and right-aligned cells.
fn print_grid(columns: &[String], rows: &[(String, Vec<String>)]) {
    const W: usize = 11;
    print!("  {:<22}", "");
    for c in columns {
        print!("{:>W$}", c.bold());
    }
    println!();
    for (label, cells) in rows {
        if cells.iter().all(|c| c == "-") {
            continue;
        }
        print!("  {:<22}", label);
        for c in cells {
            print!("{c:>W$}");
        }
        println!();
    }
}

/// The quarterly table with growth and consensus sections.
fn print_quarterly(c: &results::Collected, n: usize) {
    let views = results::quarter_views(&c.quarterly, n, c.is_india);
    let ttm = results::ttm(&c.quarterly).map(results::ttm_rounded);
    let ccy = c.currency.as_deref();
    let mut columns: Vec<String> = views.iter().map(|v| v.quarter.clone()).collect();
    if ttm.is_some() {
        columns.push("TTM".into());
    }
    let mut rows: Vec<(String, Vec<String>)> = Vec::new();
    // `cell` makes one cell per quarter, `t` the TTM cell.
    let mut row =
        |label: &str, cell: &dyn Fn(&results::QuarterView) -> String, t: Option<String>| {
            let mut cells: Vec<String> = views.iter().map(cell).collect();
            if ttm.is_some() {
                cells.push(t.unwrap_or_else(|| "-".into()));
            }
            rows.push((label.to_string(), cells));
        };
    let tt = ttm.as_ref();
    row(
        "Revenue",
        &|v| money(v.period.revenue, ccy),
        tt.map(|t| money(t.revenue, ccy)),
    );
    row(
        "Net interest income",
        &|v| money(v.period.nii, ccy),
        tt.map(|t| money(t.nii, ccy)),
    );
    row(
        "Operating profit",
        &|v| money(v.period.operating_profit, ccy),
        tt.map(|t| money(t.operating_profit, ccy)),
    );
    row(
        "OPM %",
        &|v| num(v.opm_pct, 1),
        tt.map(|t| num(t.opm_pct, 1)),
    );
    row(
        "Other income",
        &|v| money(v.period.other_income, ccy),
        tt.map(|t| money(t.other_income, ccy)),
    );
    row(
        "Interest",
        &|v| money(v.period.interest, ccy),
        tt.map(|t| money(t.interest, ccy)),
    );
    row(
        "Depreciation",
        &|v| money(v.period.depreciation, ccy),
        tt.map(|t| money(t.depreciation, ccy)),
    );
    row(
        "Profit before tax",
        &|v| money(v.period.pbt, ccy),
        tt.map(|t| money(t.pbt, ccy)),
    );
    row(
        "Tax %",
        &|v| num(v.tax_pct, 1),
        tt.map(|t| num(t.tax_pct, 1)),
    );
    row(
        "Net profit",
        &|v| money(v.period.pat, ccy),
        tt.map(|t| money(t.pat, ccy)),
    );
    row(
        "Diluted EPS",
        &|v| num(v.period.eps, 2),
        tt.map(|t| num(t.eps, 2)),
    );
    row("Gross NPA %", &|v| num(v.period.gnpa_pct, 2), None);
    row("Net NPA %", &|v| num(v.period.nnpa_pct, 2), None);
    print_grid(&columns, &rows);

    let mut growth: Vec<(String, Vec<String>)> = Vec::new();
    let mut g = |label: &str, cell: &dyn Fn(&results::QuarterView) -> String| {
        let mut cells: Vec<String> = views.iter().map(cell).collect();
        if ttm.is_some() {
            cells.push("-".into());
        }
        growth.push((label.to_string(), cells));
    };
    g("Revenue YoY %", &|v| signed(v.yoy.revenue, 1, ""));
    g("Revenue QoQ %", &|v| signed(v.qoq.revenue, 1, ""));
    g("Operating profit YoY %", &|v| {
        signed(v.yoy.operating_profit, 1, "")
    });
    g("Net profit YoY %", &|v| signed(v.yoy.pat, 1, ""));
    g("Net profit QoQ %", &|v| signed(v.qoq.pat, 1, ""));
    g("EPS YoY %", &|v| signed(v.yoy.eps, 1, ""));
    g("OPM change YoY (bps)", &|v| signed(v.opm_bps_yoy, 0, ""));
    g("OPM change QoQ (bps)", &|v| signed(v.opm_bps_qoq, 0, ""));
    print_section("Growth");
    print_grid(&columns, &growth);

    if views.iter().any(|v| v.period.surprise.is_some()) {
        let mut sur: Vec<(String, Vec<String>)> = Vec::new();
        let mut s = |label: &str, cell: &dyn Fn(&results::QuarterView) -> String| {
            let mut cells: Vec<String> = views.iter().map(cell).collect();
            if ttm.is_some() {
                cells.push("-".into());
            }
            sur.push((label.to_string(), cells));
        };
        s("EPS estimate", &|v| {
            num(v.period.surprise.as_ref().map(|x| x.estimate), 2)
        });
        s("EPS surprise %", &|v| {
            signed(
                v.period.surprise.as_ref().map(|x| x.surprise * 100.0),
                1,
                "",
            )
        });
        s("Versus estimate", &|v| {
            v.period
                .surprise
                .as_ref()
                .map_or("-".into(), |x| x.outcome.to_uppercase())
        });
        let ex = |f: fn(&results::ResultReaction) -> Option<f64>| {
            move |v: &results::QuarterView| {
                signed(
                    v.period.reaction.as_ref().and_then(f).map(|x| x * 100.0),
                    1,
                    "",
                )
            }
        };
        s("1-day vs index %", &ex(|r| r.one_day_excess));
        s("5-day vs index %", &ex(|r| r.five_day_excess));
        print_section("Against consensus (EPS)");
        print_grid(&columns, &sur);
    }
}

/// The fiscal-year table and the CAGR lines.
fn print_annual(c: &results::Collected, n: usize) {
    let views = results::annual_views(&c.annual, n, c.is_india);
    let ccy = c.currency.as_deref();
    let columns: Vec<String> = views.iter().map(|v| v.fiscal_year.clone()).collect();
    let cells = |f: &dyn Fn(&results::AnnualView) -> String| views.iter().map(f).collect();
    let rows: Vec<(String, Vec<String>)> = vec![
        ("Revenue".into(), cells(&|v| money(v.period.revenue, ccy))),
        (
            "Operating profit".into(),
            cells(&|v| money(v.period.operating_profit, ccy)),
        ),
        ("OPM %".into(), cells(&|v| num(v.opm_pct, 1))),
        ("Net profit".into(), cells(&|v| money(v.period.pat, ccy))),
        ("Diluted EPS".into(), cells(&|v| num(v.period.eps, 2))),
        (
            "Revenue YoY %".into(),
            cells(&|v| signed(v.revenue_yoy_pct, 1, "")),
        ),
        (
            "Net profit YoY %".into(),
            cells(&|v| signed(v.pat_yoy_pct, 1, "")),
        ),
        (
            "Operating cash flow".into(),
            cells(&|v| money(v.period.ocf, ccy)),
        ),
        ("Capex".into(), cells(&|v| money(v.period.capex, ccy))),
        (
            "Free cash flow".into(),
            cells(&|v| money(v.period.fcf, ccy)),
        ),
        ("CFO / PAT %".into(), cells(&|v| num(v.cfo_to_pat_pct, 0))),
        ("ROE %".into(), cells(&|v| num(v.roe_pct, 1))),
        ("ROCE %".into(), cells(&|v| num(v.roce_pct, 1))),
    ];
    print_grid(&columns, &rows);
    let cagr = results::cagrs(&c.annual);
    if !cagr.is_empty() {
        print_section("Compound annual growth");
        for g in cagr {
            println!(
                "  {:>2}Y   revenue {:>8}   net profit {:>8}   EPS {:>8}",
                g.years,
                signed(g.revenue_pct, 1, "%"),
                signed(g.pat_pct, 1, "%"),
                signed(g.eps_pct, 1, "%"),
            );
        }
    }
}

/// Source and currency footer; source notes go to stderr.
fn print_results_footer(c: &results::Collected) {
    println!();
    println!(
        "  {}",
        format!(
            "Source: {} · Currency: {} · Amounts as reported (consolidated where filed) · Not investment advice",
            c.sources.join(", "),
            c.currency.as_deref().unwrap_or("unknown"),
        )
        .dimmed()
    );
    for w in &c.warnings {
        eprintln!("  {} {}", "ⓘ".dimmed(), w);
    }
}

/// The season scoreboard: totals, then one line per reporter.
fn print_scoreboard(b: &results::Scoreboard) {
    print_header(&format!(
        "Results season: {} ({})",
        b.title,
        b.quarter.as_deref().unwrap_or("no quarter yet")
    ));
    if b.reported == 0 {
        println!("  No results found for {} symbols.", b.total);
        return;
    }
    let agg = &b.aggregate;
    println!(
        "  {}/{} reported {}: revenue {} YoY, operating profit {} YoY, net profit {} YoY",
        b.reported,
        b.total,
        b.quarter.as_deref().unwrap_or(""),
        signed(agg.revenue_yoy_pct, 1, "%"),
        signed(agg.operating_profit_yoy_pct, 1, "%"),
        signed(agg.pat_yoy_pct, 1, "%"),
    );
    println!(
        "  EPS vs consensus: {} beat / {} in-line / {} miss",
        b.beats, b.in_line, b.misses
    );
    println!();
    println!(
        "  {:<14}{:>10}{:>10}{:>10}{:>11}{:>10}{:>10}",
        "Symbol", "Quarter", "Rev YoY%", "OP YoY%", "PAT YoY%", "OPM bps", "vs est"
    );
    for r in &b.reporters {
        println!(
            "  {:<14}{:>10}{:>10}{:>10}{:>11}{:>10}{:>10}",
            r.symbol,
            r.quarter,
            signed(r.revenue_yoy_pct, 1, ""),
            signed(r.operating_profit_yoy_pct, 1, ""),
            signed(r.pat_yoy_pct, 1, ""),
            signed(r.opm_bps_yoy, 0, ""),
            r.outcome.as_deref().unwrap_or("-").to_uppercase(),
        );
    }
    if !b.pending.is_empty() {
        println!(
            "\n  {} {}",
            "Not yet reported:".dimmed(),
            b.pending.join(", ")
        );
    }
    if !b.failed.is_empty() {
        eprintln!("  ⓘ No data for: {}", b.failed.join(", "));
    }
}

/// `results SYMBOL`, `results --holdings` and `results --universe INDEX`.
#[allow(clippy::too_many_arguments)]
pub async fn cmd_results(
    symbol: Option<&str>,
    quarters: usize,
    annual: bool,
    holdings: bool,
    universe: Option<market::Universe>,
    sort: results::Sort,
    market: Market,
    fmt: Format,
) -> Result<()> {
    if !(1..=40).contains(&quarters) {
        bail!("--quarters takes 1 to 40");
    }
    let store = results::default_store();
    if symbol.is_none() {
        if annual {
            bail!("--annual shows one symbol's fiscal years; name a symbol");
        }
        if !holdings && universe.is_none() {
            bail!("Name a symbol, or use --holdings or --universe nifty50");
        }
        super::markets::check_universe(market, universe)?;
        let client = YahooClient::new().await?;
        let (symbols, title) = if holdings {
            let mut s: Vec<String> = Portfolio::load()?
                .holdings
                .into_iter()
                .map(|h| h.symbol)
                .collect();
            s.extend(Watchlist::load()?.symbols);
            let mut seen = std::collections::HashSet::new();
            s.retain(|x| seen.insert(x.clone()));
            if s.is_empty() {
                bail!("No holdings or watchlist symbols yet. Add some, or use --universe");
            }
            (s, "portfolio and watchlist".to_string())
        } else {
            let scan = super::markets::scan_symbols(&client, market, universe).await?;
            (scan.symbols, scan.scope)
        };
        let board = results::season(&client, &symbols, &title, market, store, sort).await;
        return match fmt {
            Format::Json => output::print_json(&board),
            Format::Csv => output::print_csv(&results::reporters_csv(&board)),
            Format::Table => {
                print_scoreboard(&board);
                Ok(())
            }
        };
    }
    let symbol = symbol.unwrap_or_default();
    let client = YahooClient::new().await?;
    let c = results::collect(&client, symbol, market, store, !annual, true).await?;
    if annual && c.annual.is_empty() {
        bail!("No annual results found for {}", c.symbol);
    }
    match (fmt, annual) {
        (Format::Json, false) => output::print_json(&results::quarterly_json(&c, quarters)),
        (Format::Json, true) => output::print_json(&results::annual_json(&c, quarters)),
        (Format::Csv, false) => output::print_csv(&results::quarterly_csv(&c, quarters)),
        (Format::Csv, true) => output::print_csv(&results::annual_csv(&c, quarters)),
        (Format::Table, _) => {
            print_header(&format!(
                "{}: {} ({})",
                if annual {
                    "Annual results"
                } else {
                    "Quarterly results"
                },
                c.symbol,
                c.name
            ));
            if annual {
                print_annual(&c, quarters);
            } else {
                print_quarterly(&c, quarters);
            }
            print_results_footer(&c);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{ChartData, Indicators, QuoteIndicator};

    fn chart(close: Vec<Option<f64>>) -> ChartData {
        let n = close.len();
        let some = |x: f64| Some(vec![Some(x); n]);
        ChartData {
            meta: None,
            timestamp: None,
            indicators: Indicators {
                quote: vec![QuoteIndicator {
                    open: some(10.0),
                    high: some(12.0),
                    low: some(9.0),
                    close: Some(close),
                    volume: Some(vec![Some(100); n]),
                }],
            },
            events: None,
        }
    }

    #[test]
    fn a_bar_with_a_null_close_leaves_every_series_the_same_length() {
        // The live bar of an NSE chart: high and low set, close still null.
        let (o, h, l, c, v) = aligned_ohlcv(&chart(vec![Some(11.0), Some(11.5), None]));
        assert_eq!(
            (o.len(), h.len(), l.len(), c.len(), v.len()),
            (2, 2, 2, 2, 2)
        );
        assert_eq!(c, vec![11.0, 11.5]);
    }

    fn altman(is_financial: bool) -> scorecard::AltmanScorecard {
        let mut r = crate::research::Research::default();
        r.sector = Some(
            if is_financial {
                "Financial Services"
            } else {
                "Technology"
            }
            .into(),
        );
        let mut a = scorecard::generate_scorecard_report(&r).altman;
        a.z_score = Some(0.32);
        a.zone_label = Some("Distress Zone (High Default Risk)".into());
        a
    }

    #[test]
    fn altman_line_for_a_bank_has_no_zone() {
        let a = altman(true);
        assert!(a.is_financial);
        let line = altman_summary(&a).unwrap();
        assert_eq!(line, "n/a for banks and lenders");
        assert!(!line.contains("Distress") && !line.contains("0.32"));
    }

    #[test]
    fn altman_line_for_an_industrial_keeps_the_zone_without_double_brackets() {
        let line = altman_summary(&altman(false)).unwrap();
        assert_eq!(line, "0.32 — Distress Zone (High Default Risk)");
    }

    #[test]
    fn analyst_section_needs_at_least_one_figure() {
        let mut q = crate::api::Quote::default();
        assert!(!has_analyst_data(&q));
        q.number_of_analyst_opinions = Some(12);
        assert!(has_analyst_data(&q));
    }

    #[test]
    fn dividend_income_basis_is_lakh_in_inr_and_ten_thousand_elsewhere() {
        assert_eq!(dividend_income_basis(Some("INR")), ("₹1L", 100_000.0));
        assert_eq!(dividend_income_basis(Some("USD")), ("$10k", 10_000.0));
        assert_eq!(dividend_income_basis(None), ("$10k", 10_000.0));
    }
}
