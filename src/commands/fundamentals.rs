//! Company fundamentals: analysis, comparisons, peers, dividends, insider activity, and the all-in-one `deep` report.

use super::*;

pub async fn cmd_analyze(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("No data found for this symbol")?;

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
    print_section("Insights");
    for insight in insights::generate_insights(q) {
        println!("  {} {}", "→".cyan(), insight);
    }

    // Verdict
    println!();
    println!("{}", "─".repeat(60).dimmed());
    println!("{}", insights::overall_verdict(q));
    println!("{}", "─".repeat(60).dimmed());

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
    pub(crate) analyst_rating: Option<String>,
}

impl CsvRow for CompareRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "currency",
        "price",
        "change_percent",
        "market_cap",
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
        "analyst_rating",
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
            analyst_rating: q.recommendation_key.clone(),
        }
    }
}

pub async fn cmd_compare(symbols: &[String], market: Market, fmt: Format) -> Result<()> {
    let resolved = resolve_symbols(symbols, market);
    let client = YahooClient::new().await?;
    let sym_refs: Vec<&str> = resolved.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    if quotes.is_empty() {
        bail!(
            "No quotes for {}{}",
            resolved.join(", "),
            market::not_found_hint(market)
        );
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

pub async fn cmd_peers(symbol: &str, market: Market) -> Result<()> {
    market.require_stocks("peers")?;
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[&resolved]).await?;
    let q = quotes.first().context("Symbol not found")?;
    let sector = q.sector.as_deref().unwrap_or("Unknown");

    // Get all popular stocks and filter by same sector
    let all_symbols = market.universe();
    let all_refs: Vec<&str> = all_symbols.iter().map(String::as_str).collect();
    let all_quotes = client.get_quote(&all_refs).await?;
    let mut peers: Vec<&crate::api::Quote> = all_quotes
        .iter()
        .filter(|p| p.sector.as_deref() == Some(sector) && p.symbol.as_deref() != Some(&resolved))
        .collect();
    peers.sort_by(|a, b| {
        b.market_cap
            .unwrap_or(0.0)
            .partial_cmp(&a.market_cap.unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    print_header(&format!("Peer Comparison: {} ({})", resolved, sector));

    // Include the target stock first
    let mut all_peers = vec![q];
    all_peers.extend(peers.iter().take(8).copied());

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
        print_kv("Analyst Consensus", &sentiment_label(rec));
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
            "Target Price",
            &format!(
                "{}{:.2} ({:+.1}%)",
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

pub async fn cmd_deep(symbol: &str, period: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;
    let cur_ref = &resolved;

    // ── 1. Quote + Fundamentals ──
    let quotes = client.get_quote(&[cur_ref.as_str()]).await?;
    let q = quotes.first().context("Symbol not found")?;
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
        &q.market_cap
            .map_or("N/A".into(), |v| format_large_number(v, cur)),
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
        print_kv(
            "Income on {}1L",
            &format!("{}{:.0}/year", csym, 100000.0 * dy),
        );
    }

    // Analyst
    print_section("Analyst Ratings");
    if let Some(rec) = q.recommendation_mean {
        print_kv("Consensus", &sentiment_label(rec));
        print_kv(
            "Score",
            &format!("{:.1}/5  {}", rec, rating_bar(5.0 - rec, 4.0)),
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
        print_kv("Price Target", &format!("{}{:.2} ({})", csym, target, u));
    }
    if let Some(n) = q.number_of_analyst_opinions {
        print_kv("# Analysts", &n.to_string());
    }

    // ── 2. Technical Indicators ──
    let chart = client.get_chart(&resolved, period, "1d").await?;
    let closes: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|qi| qi.close.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    let highs: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|qi| qi.high.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    let lows: Vec<f64> = chart
        .indicators
        .quote
        .first()
        .and_then(|qi| qi.low.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();
    let volumes: Vec<u64> = chart
        .indicators
        .quote
        .first()
        .and_then(|qi| qi.volume.as_ref())
        .map(|c| c.iter().filter_map(|v| *v).collect())
        .unwrap_or_default();

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
        let n = highs.len();
        if n > 1 {
            let (pivot, r1, r2, _r3, s1, s2, _s3) =
                technical::pivot_points(highs[n - 1], lows[n - 1], closes[n - 1]);
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
        let opens: Vec<f64> = chart
            .indicators
            .quote
            .first()
            .and_then(|qi| qi.open.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();
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

    println!(
        "\n  {}",
        "Not financial advice. Do your own research."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}
