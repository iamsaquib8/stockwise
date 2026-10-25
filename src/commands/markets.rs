//! Market-wide views: movers, sectors, news, sentiment, screeners, rankings, and IPOs.

use super::*;

/// One stock in `movers --json/--csv`. `side` is `gainer` or `loser`;
/// `rank` starts at 1 within each side.
#[derive(Debug, Serialize)]
pub(crate) struct MoverRow {
    pub(crate) side: &'static str,
    pub(crate) rank: usize,
    pub(crate) symbol: String,
    pub(crate) price: f64,
    pub(crate) change_percent: f64,
    pub(crate) volume: Option<u64>,
    pub(crate) currency: Option<String>,
}

impl CsvRow for MoverRow {
    const COLUMNS: &'static [&'static str] = &[
        "side",
        "rank",
        "symbol",
        "price",
        "change_percent",
        "volume",
        "currency",
    ];
}

/// Top and bottom `n` by today's change. Quotes without a price or change
/// are left out rather than ranked as 0%.
pub(crate) fn mover_rows(quotes: &[crate::api::Quote], n: usize) -> (Vec<MoverRow>, Vec<MoverRow>) {
    let mut ranked: Vec<(&crate::api::Quote, f64, f64)> = quotes
        .iter()
        .filter_map(|q| Some((q, q.regular_market_price?, q.regular_market_change_percent?)))
        .collect();
    ranked.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    let row = |side, rank, (q, price, pct): &(&crate::api::Quote, f64, f64)| MoverRow {
        side,
        rank,
        symbol: q.symbol.clone().unwrap_or_default(),
        price: *price,
        change_percent: *pct,
        volume: q.regular_market_volume,
        currency: q.currency.clone(),
    };
    let gainers = ranked
        .iter()
        .take(n)
        .enumerate()
        .map(|(i, r)| row("gainer", i + 1, r))
        .collect();
    let losers = ranked
        .iter()
        .rev()
        .take(n)
        .enumerate()
        .map(|(i, r)| row("loser", i + 1, r))
        .collect();
    (gainers, losers)
}

/// `--universe` names NSE indices: fail before any request elsewhere.
fn check_universe(market: Market, universe: Option<market::Universe>) -> Result<()> {
    if universe.is_some() && market != Market::In {
        bail!(
            "--universe lists NSE index members; use it with the Indian market (-m in), not {}",
            market.label()
        );
    }
    Ok(())
}

/// Symbols a scan covers and its title: the members of an NSE index with
/// `--universe` (from NSE; the built-in list when NSE can't answer), else
/// the market's built-in list.
async fn scan_symbols(
    client: &YahooClient,
    market: Market,
    universe: Option<market::Universe>,
) -> Result<(Vec<String>, String)> {
    let Some(u) = universe else {
        return Ok((market.universe(), market.label().to_string()));
    };
    let index = u.nse_index();
    match client.index_constituents(index).await {
        Ok(symbols) => Ok((symbols, index.to_string())),
        Err(e) => match u.fallback() {
            Some(list) => {
                eprintln!(
                    "  \u{24d8} {}",
                    tf(
                        Msg::UniverseFallback,
                        &[&index, &format!("{:#}", e), &list.len()]
                    )
                );
                Ok((list, index.to_string()))
            }
            None => Err(e.context(format!(
                "NSE's {} member list is unavailable and StockWise has no built-in copy of it",
                index
            ))),
        },
    }
}

pub async fn cmd_movers(
    market: Market,
    universe: Option<market::Universe>,
    fmt: Format,
) -> Result<()> {
    check_universe(market, universe)?;
    let client = YahooClient::new().await?;
    let (symbols, scope) = scan_symbols(&client, market, universe).await?;
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;
    let (gainers, losers) = mover_rows(&quotes, 10);
    match fmt {
        Format::Json => {
            return output::print_json(&serde_json::json!({
                "gainers": gainers,
                "losers": losers,
            }));
        }
        Format::Csv => {
            let all: Vec<MoverRow> = gainers.into_iter().chain(losers).collect();
            return output::print_csv(&all);
        }
        Format::Table => {}
    }

    for (title, rows) in [("Top Gainers", &gainers), ("Top Losers", &losers)] {
        print_header(&format!("{} — {}", title, scope));
        println!(
            "  {:<14} {:>10} {:>10} {:>14}",
            "Symbol".bold(),
            "Price".bold(),
            "Change %".bold(),
            "Volume".bold()
        );
        println!("  {}", "─".repeat(52).dimmed());
        for r in rows.iter() {
            let pct = format!("{:+.2}%", r.change_percent);
            println!(
                "  {:<14} {:>10} {:>10} {:>14}",
                r.symbol.cyan(),
                format_asset_price(r.price, r.currency.as_deref(), &r.symbol),
                if r.side == "gainer" {
                    pct.green()
                } else {
                    pct.red()
                },
                r.volume.map_or("N/A".to_string(), format_volume)
            );
        }
    }
    println!();
    Ok(())
}

// ── Sectors ──

/// One sector in `sectors --json/--csv`, best performer first.
#[derive(Debug, Serialize)]
pub(crate) struct SectorRow {
    pub(crate) sector: String,
    pub(crate) symbol: String,
    pub(crate) price: Option<f64>,
    pub(crate) change_percent: Option<f64>,
    pub(crate) currency: Option<String>,
}

impl CsvRow for SectorRow {
    const COLUMNS: &'static [&'static str] =
        &["sector", "symbol", "price", "change_percent", "currency"];
}

pub async fn cmd_sectors(market: Market, fmt: Format) -> Result<()> {
    market.require_sectors("sectors")?;
    let client = YahooClient::new().await?;
    let sectors = market.sectors();
    let title = format!("{} Sector Performance", market.label());
    let symbols: Vec<&str> = sectors.iter().map(|(s, _)| *s).collect();
    let mut quotes = client.get_quote(&symbols).await?;
    // Best first; sectors without a change today go last.
    quotes.sort_by(|a, b| {
        let key = |q: &crate::api::Quote| q.regular_market_change_percent.unwrap_or(f64::MIN);
        key(b)
            .partial_cmp(&key(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    if !fmt.is_table() {
        let rows: Vec<SectorRow> = quotes
            .iter()
            .map(|q| {
                let sym = q.symbol.clone().unwrap_or_default();
                let name = sectors
                    .iter()
                    .find(|(s, _)| *s == sym)
                    .map_or(sym.clone(), |(_, n)| n.to_string());
                SectorRow {
                    sector: name,
                    symbol: sym,
                    price: q.regular_market_price,
                    change_percent: q.regular_market_change_percent,
                    currency: q.currency.clone(),
                }
            })
            .collect();
        return match fmt {
            Format::Csv => output::print_csv(&rows),
            _ => output::print_json(&rows),
        };
    }

    print_header(&title);
    println!(
        "  {:<18} {:>10} {:>10} {:>20}",
        "Sector".bold(),
        "Price".bold(),
        "Change %".bold(),
        "".bold()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let name = sectors
            .iter()
            .find(|(s, _)| *s == sym)
            .map(|(_, n)| *n)
            .unwrap_or(sym);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let bar_len = (pct.abs() * 3.0).min(20.0) as usize;
        let bar = if pct >= 0.0 {
            "█".repeat(bar_len).green().to_string()
        } else {
            "█".repeat(bar_len).red().to_string()
        };
        let pct_str = if pct >= 0.0 {
            format!("{:+.2}%", pct).green().to_string()
        } else {
            format!("{:+.2}%", pct).red().to_string()
        };
        println!(
            "  {:<18} {:>10} {:>10} {}",
            name.bold(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            pct_str,
            bar
        );
    }
    println!();
    Ok(())
}

// ── News ──

// ── Multi-source news helpers ──────────────────────────────────

#[derive(Debug, Clone)]
pub(super) struct NewsItem {
    title: String,
    publisher: String,
    timestamp: Option<i64>,   // unix seconds
    source_tag: &'static str, // e.g. "Yahoo", "Google News"
}

/// Strip CDATA wrappers and XML entities from an RSS field
pub(super) fn rss_clean(s: &str) -> String {
    let s = s.trim();
    let s = s
        .strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(s);
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .trim()
        .to_string()
}

/// Extract text between two markers (first occurrence)
pub(super) fn between<'a>(s: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = s.find(open)? + open.len();
    let end = s[start..].find(close)?;
    Some(&s[start..start + end])
}

/// Parse RSS XML into NewsItems
pub(super) fn parse_rss(xml: &str, source_tag: &'static str) -> Vec<NewsItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let end = chunk.find("</item>").unwrap_or(chunk.len());
            let item = &chunk[..end];

            // Title: try CDATA first, then plain
            let title_raw = between(item, "<title><![CDATA[", "]]></title>")
                .or_else(|| between(item, "<title>", "</title>"))?;
            let title = rss_clean(title_raw);
            if title.is_empty() {
                return None;
            }

            // Publisher from <source ...>Name</source> or hardcode source_tag
            let publisher = between(item, "<source", "</source>")
                .and_then(|s| between(s, ">", "\n").or_else(|| Some(s.trim())))
                .map(rss_clean)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| source_tag.to_string());

            // Timestamp from <pubDate>
            let timestamp = between(item, "<pubDate>", "</pubDate>")
                .and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok())
                .map(|d| d.timestamp());

            Some(NewsItem {
                title,
                publisher,
                timestamp,
                source_tag,
            })
        })
        .collect()
}

/// Fetch Yahoo Finance RSS for a symbol
pub(super) async fn fetch_yahoo_rss(client: &YahooClient, symbol: &str) -> Vec<NewsItem> {
    let url = crate::api::build_url(
        "https://feeds.finance.yahoo.com/rss/2.0/headline",
        None,
        &[("s", symbol), ("region", "US"), ("lang", "en-US")],
    );
    match client.raw_get_text(&url).await {
        Ok(xml) => parse_rss(&xml, "Yahoo RSS"),
        Err(_) => vec![],
    }
}

/// Fetch Google News RSS for a query
pub(super) async fn fetch_google_news_rss(client: &YahooClient, query: &str) -> Vec<NewsItem> {
    let url = crate::api::build_url(
        "https://news.google.com/rss/search",
        None,
        &[("q", query), ("hl", "en"), ("gl", "US"), ("ceid", "US:en")],
    );
    match client.raw_get_text(&url).await {
        Ok(xml) => parse_rss(&xml, "Google News"),
        Err(_) => vec![],
    }
}

/// Fetch Yahoo Finance search API news (JSON)
pub(super) async fn fetch_yahoo_json_news(
    client: &YahooClient,
    query: &str,
    count: usize,
) -> Vec<NewsItem> {
    let count = count.to_string();
    let url = crate::api::build_url(
        "https://query2.finance.yahoo.com/v1/finance/search",
        None,
        &[("q", query), ("quotesCount", "0"), ("newsCount", &count)],
    );
    let resp = match client.raw_get(&url).await {
        Ok(r) => r,
        Err(_) => return vec![],
    };
    resp.get("news")
        .and_then(|n| n.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|item| {
                    let title = item.get("title").and_then(|t| t.as_str())?.to_string();
                    let publisher = item
                        .get("publisher")
                        .and_then(|p| p.as_str())
                        .unwrap_or("Yahoo Finance")
                        .to_string();
                    let timestamp = item.get("providerPublishTime").and_then(|t| t.as_i64());
                    Some(NewsItem {
                        title,
                        publisher,
                        timestamp,
                        source_tag: "Yahoo Finance",
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Deduplicate news items: remove near-duplicate titles, sort newest first
pub(super) fn collate_news(mut items: Vec<NewsItem>) -> Vec<NewsItem> {
    // Sort newest first (None timestamps go to the end)
    items.sort_by_key(|i| std::cmp::Reverse(i.timestamp.unwrap_or(0)));

    // Dedup: fingerprint as owned Strings so there are no borrow issues
    let mut seen: Vec<Vec<String>> = Vec::new();
    let mut result = Vec::new();

    for item in items {
        let words: Vec<String> = item
            .title
            .split_whitespace()
            .filter(|w| w.len() > 3)
            .map(|w| w.to_lowercase())
            .collect();
        let is_dup = seen.iter().any(|existing| {
            if existing.is_empty() || words.is_empty() {
                return false;
            }
            let matches = words.iter().filter(|w| existing.contains(w)).count();
            matches as f64 / words.len().min(existing.len()) as f64 > 0.6
        });
        if !is_dup {
            seen.push(words);
            result.push(item);
        }
    }
    result
}

pub(super) fn print_news_items(items: &[NewsItem], limit: usize) {
    if items.is_empty() {
        println!("  {}", "No recent news found.".dimmed());
        return;
    }
    let source_colors: &[(&str, &str)] = &[
        ("Yahoo Finance", "cyan"),
        ("Yahoo RSS", "blue"),
        ("Google News", "green"),
    ];
    for item in items.iter().take(limit) {
        let ts = item
            .timestamp
            .and_then(|t| chrono::DateTime::from_timestamp(t, 0))
            .map(|d| d.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();
        let tag = source_colors
            .iter()
            .find(|(s, _)| *s == item.source_tag)
            .map(|(_, c)| match *c {
                "cyan" => format!("[{}]", item.source_tag).cyan().to_string(),
                "blue" => format!("[{}]", item.source_tag).blue().to_string(),
                "green" => format!("[{}]", item.source_tag).green().to_string(),
                _ => format!("[{}]", item.source_tag),
            })
            .unwrap_or_else(|| format!("[{}]", item.source_tag));
        println!();
        println!("  {} {}", "●".cyan(), item.title.bold());
        println!(
            "    {} {} · {} · {}",
            "└".dimmed(),
            item.publisher.dimmed(),
            ts.dimmed(),
            tag
        );
    }
}

// ───────────────────────────────────────────────────────────────

pub async fn cmd_news(symbol: &str, market: Market) -> Result<()> {
    let resolved = market::resolve_symbol(symbol, market);
    let client = YahooClient::new().await?;

    // Fetch from 3 sources concurrently
    let company_query = resolved.split('.').next().unwrap_or(&resolved);
    let google_query = format!("{} stock", company_query);

    let (yahoo_json, yahoo_rss, google) = tokio::join!(
        fetch_yahoo_json_news(&client, &resolved, 10),
        fetch_yahoo_rss(&client, &resolved),
        fetch_google_news_rss(&client, &google_query),
    );

    let all: Vec<NewsItem> = [yahoo_json, yahoo_rss, google].concat();
    let collated = collate_news(all);

    print_header(&format!("News: {} — {} sources", resolved, 3));
    print_news_items(&collated, 15);
    println!();
    Ok(())
}

// ── Risk Analysis ──

pub async fn cmd_sentiment() -> Result<()> {
    let client = YahooClient::new().await?;
    // Fetch VIX, S&P500, and some breadth proxies
    let quotes = client
        .get_quote(&["^VIX", "^GSPC", "^IXIC", "^DJI", "SPY", "QQQ"])
        .await?;

    let vix = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some("^VIX"))
        .and_then(|q| q.regular_market_price);
    let sp_chg = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some("^GSPC"))
        .and_then(|q| q.regular_market_change_percent);

    let mut score = 50.0_f64;
    let mut factors = Vec::new();

    // VIX: <15 extreme greed, 15-20 greed, 20-25 neutral, 25-35 fear, >35 extreme fear
    if let Some(v) = vix {
        let vix_score = match v {
            x if x < 15.0 => 90.0,
            x if x < 20.0 => 70.0,
            x if x < 25.0 => 50.0,
            x if x < 35.0 => 30.0,
            _ => 10.0,
        };
        score = (score + vix_score) / 2.0;
        factors.push(format!(
            "VIX: {:.1} → {}",
            v,
            if vix_score > 60.0 {
                "Greed"
            } else if vix_score < 40.0 {
                "Fear"
            } else {
                "Neutral"
            }
        ));
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
        let closes: Vec<f64> = c
            .indicators
            .quote
            .first()
            .and_then(|q| q.close.as_ref())
            .map(|c| c.iter().filter_map(|v| *v).collect())
            .unwrap_or_default();
        if closes.len() > 5 {
            let monthly_ret = (closes.last().unwrap() / closes[0] - 1.0) * 100.0;
            let trend_score = (50.0 + monthly_ret * 5.0).clamp(0.0, 100.0);
            score = (score + trend_score) / 2.0;
            factors.push(format!("1M trend: {:+.2}%", monthly_ret));
        }
    }

    // News sentiment from multiple sources
    let (yahoo_news, google_news) = tokio::join!(
        fetch_yahoo_json_news(&client, "stock market today", 10),
        fetch_google_news_rss(&client, "US India stock market today"),
    );
    let all_news: Vec<NewsItem> = [yahoo_news, google_news].concat();
    let collated_news = collate_news(all_news);

    // Simple keyword-based sentiment scoring
    let bullish_words = [
        "rally", "surge", "gain", "rise", "high", "bull", "growth", "record", "up", "buy",
        "strong", "positive",
    ];
    let bearish_words = [
        "fall",
        "drop",
        "crash",
        "decline",
        "fear",
        "bear",
        "loss",
        "low",
        "sell",
        "weak",
        "negative",
        "recession",
    ];
    let mut news_bull = 0i32;
    let mut news_bear = 0i32;
    for item in &collated_news {
        let t = item.title.to_lowercase();
        for w in &bullish_words {
            if t.contains(w) {
                news_bull += 1;
            }
        }
        for w in &bearish_words {
            if t.contains(w) {
                news_bear += 1;
            }
        }
    }
    if news_bull + news_bear > 0 {
        let news_score =
            ((news_bull as f64 / (news_bull + news_bear) as f64) * 100.0).clamp(0.0, 100.0);
        score = (score * 0.7 + news_score * 0.3).clamp(0.0, 100.0);
        let label = if news_score > 55.0 {
            "bullish"
        } else if news_score < 45.0 {
            "bearish"
        } else {
            "neutral"
        };
        factors.push(format!(
            "News sentiment: {} ({} positive / {} negative headlines)",
            label, news_bull, news_bear
        ));
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
        let ch = if i == pos {
            "▼".bold().to_string()
        } else if i < 10 {
            "█".red().to_string()
        } else if i < 20 {
            "█".red().dimmed().to_string()
        } else if i < 30 {
            "█".yellow().to_string()
        } else if i < 40 {
            "█".green().dimmed().to_string()
        } else {
            "█".green().to_string()
        };
        gauge.push_str(&ch);
    }
    println!("\n  {}  {}  {}", "Fear".red(), gauge, "Greed".green());
    let score_display = match color {
        "green" => format!("{:.0}", score).green().bold().to_string(),
        "red" => format!("{:.0}", score).red().bold().to_string(),
        _ => format!("{:.0}", score).yellow().bold().to_string(),
    };
    println!("  {:>28} {} {}", "", score_display, label.bold());

    print_section("Contributing Factors");
    for f in &factors {
        println!("  {} {}", "→".cyan(), f);
    }

    if !collated_news.is_empty() {
        print_section("Top Market Headlines");
        print_news_items(&collated_news, 5);
    }
    println!();
    Ok(())
}

// ── Tax Harvest ──

/// One match in `screen --json/--csv`.
#[derive(Debug, Serialize)]
pub(crate) struct ScreenRow {
    pub(crate) symbol: String,
    pub(crate) name: Option<String>,
    pub(crate) price: Option<f64>,
    pub(crate) change_percent: Option<f64>,
    pub(crate) pe: Option<f64>,
    pub(crate) market_cap: Option<f64>,
    pub(crate) dividend_yield: Option<f64>,
    pub(crate) currency: Option<String>,
}

impl CsvRow for ScreenRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "price",
        "change_percent",
        "pe",
        "market_cap",
        "dividend_yield",
        "currency",
    ];
}

impl From<&crate::api::Quote> for ScreenRow {
    fn from(q: &crate::api::Quote) -> Self {
        ScreenRow {
            symbol: q.symbol.clone().unwrap_or_default(),
            name: q.short_name.clone().or_else(|| q.long_name.clone()),
            price: q.regular_market_price,
            change_percent: q.regular_market_change_percent,
            pe: q.trailing_pe,
            market_cap: q.market_cap,
            dividend_yield: q.trailing_annual_dividend_yield,
            currency: q.currency.clone(),
        }
    }
}

/// The quotes a preset screen keeps, largest first (momentum: biggest
/// gainers first), and the screen's title.
pub(crate) fn screen_matches<'a>(
    category: &str,
    quotes: &'a [crate::api::Quote],
) -> Result<(String, Vec<&'a crate::api::Quote>)> {
    let (title, mut filtered): (String, Vec<&crate::api::Quote>) = match category {
        "undervalued" => ("Undervalued Stocks (P/E < 15)".into(),
            quotes.iter().filter(|q| q.trailing_pe.is_some_and(|pe| pe > 0.0 && pe < 15.0)).collect()),
        "growth" => ("Growth Stocks (EPS Forward > EPS TTM)".into(),
            quotes.iter().filter(|q| matches!((q.eps_trailing_twelve_months, q.eps_forward), (Some(ttm), Some(fwd)) if fwd > ttm && ttm > 0.0)).collect()),
        "dividend" => ("Dividend Stocks (Yield > 2%)".into(),
            quotes.iter().filter(|q| q.trailing_annual_dividend_yield.is_some_and(|d| d > 0.02)).collect()),
        "momentum" => {
            let mut s: Vec<&crate::api::Quote> = quotes.iter().collect();
            s.retain(|q| q.regular_market_change_percent.is_some());
            s.sort_by(|a, b| b.regular_market_change_percent.partial_cmp(&a.regular_market_change_percent).unwrap_or(std::cmp::Ordering::Equal));
            s.truncate(15);
            ("Momentum Leaders (top daily gainers)".into(), s)
        }
        "bluechip" => ("Blue Chip (Market Cap > $100B / ₹5L Cr)".into(),
            // $100B for USD listings, ₹5L Cr (5e12) for INR listings.
            quotes.iter().filter(|q| {
                let floor = if market::is_inr(q.currency.as_deref()) { 5e12 } else { 1e11 };
                q.market_cap.is_some_and(|mc| mc > floor)
            }).collect()),
        _ => bail!("Unknown screen '{}'. Use one of: {}", category, market::SCREENER_CATEGORIES.join(", ")),
    };

    if category != "momentum" {
        filtered.sort_by(|a, b| {
            b.market_cap
                .unwrap_or(0.0)
                .partial_cmp(&a.market_cap.unwrap_or(0.0))
                .unwrap()
        });
    }

    Ok((title, filtered))
}

pub async fn cmd_screen(
    category: &str,
    market: Market,
    universe: Option<market::Universe>,
    fmt: Format,
) -> Result<()> {
    market.require_stocks("screen")?;
    check_universe(market, universe)?;
    let client = YahooClient::new().await?;
    let (symbols, scope) = scan_symbols(&client, market, universe).await?;
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    let (title, filtered) = screen_matches(category, &quotes)?;

    if !fmt.is_table() {
        let rows: Vec<ScreenRow> = filtered.iter().map(|q| ScreenRow::from(*q)).collect();
        return match fmt {
            Format::Csv => output::print_csv(&rows),
            _ => output::print_json(&rows),
        };
    }

    let title = if universe.is_some() {
        format!("{} — {}", title, scope)
    } else {
        title
    };
    print_header(&title);
    if filtered.is_empty() {
        println!("  {}", "No stocks match this screen.".dimmed());
        return Ok(());
    }
    println!(
        "  {:<14} {:>10} {:>10} {:>10} {:>14}",
        "Symbol".bold(),
        "Price".bold(),
        "Change %".bold(),
        "P/E".bold(),
        "Market Cap".bold()
    );
    println!("  {}", "─".repeat(62).dimmed());

    for q in &filtered {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 {
            format!("+{:.2}%", pct).green().to_string()
        } else {
            format!("{:.2}%", pct).red().to_string()
        };
        println!(
            "  {:<14} {:>10} {:>10} {:>10} {:>14}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            pct_str,
            q.trailing_pe
                .map_or("N/A".to_string(), |v| format!("{:.1}", v)),
            q.market_cap
                .map_or("N/A".to_string(), |v| format_large_number(v, cur))
        );
    }
    println!("\n  {} {} stocks matched", "→".cyan(), filtered.len());

    // AI screen analysis
    let mut ai = crate::ai::AiClient::connect().await;
    if ai.auto_available().await && !filtered.is_empty() {
        let mut data = format!("Screen: {}\n", category);
        for q in filtered.iter().take(8) {
            let sym = q.symbol.as_deref().unwrap_or("?");
            data.push_str(&format!(
                "{}: price={:.2}, P/E={}, chg={:+.2}%\n",
                sym,
                q.regular_market_price.unwrap_or(0.0),
                q.trailing_pe.map_or("N/A".into(), |v| format!("{:.1}", v)),
                q.regular_market_change_percent.unwrap_or(0.0)
            ));
        }
        print_section(&format!("AI Analysis ({})", ai.provider().name()));
        if let Ok(analysis) = ai.analyze_screen(&data).await {
            print_ai_answer(&analysis);
        }
    }

    println!();
    Ok(())
}

// ── Dashboard ──

pub async fn cmd_picks(market: Market) -> Result<()> {
    market.require_stocks("picks")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header(&format!("Top Picks — {} Market", market.label()));
    println!(
        "  {} Scoring {} stocks across 8 factors...\n",
        "⟳".yellow(),
        quotes.len()
    );

    let mut scored: Vec<(&crate::api::Quote, f64, Vec<&str>)> = Vec::new();
    for q in &quotes {
        let mut score = 50.0_f64;
        let mut reasons: Vec<&str> = Vec::new();
        // P/E
        if let Some(pe) = q.trailing_pe {
            if pe > 0.0 && pe < 20.0 {
                score += 10.0;
                reasons.push("Low P/E");
            } else if pe > 40.0 {
                score -= 10.0;
            }
        }
        // Forward > Trailing (earnings growing)
        if let (Some(t), Some(f)) = (q.trailing_pe, q.forward_pe)
            && f < t
            && t > 0.0
            && f > 0.0
        {
            score += 8.0;
            reasons.push("Earnings growth");
        }
        // Moving averages
        if let (Some(p), Some(ma50), Some(ma200)) = (
            q.regular_market_price,
            q.fifty_day_average,
            q.two_hundred_day_average,
        ) {
            if p > ma50 && ma50 > ma200 {
                score += 12.0;
                reasons.push("Strong uptrend");
            } else if p < ma50 && ma50 < ma200 {
                score -= 10.0;
            }
        }
        // Analyst rating
        if let Some(rec) = q.recommendation_mean {
            if rec <= 2.0 {
                score += 10.0;
                reasons.push("Analyst buy");
            } else if rec >= 4.0 {
                score -= 8.0;
            }
        }
        // Revenue growth
        if let Some(g) = q.revenue_growth
            && g > 0.15
        {
            score += 8.0;
            reasons.push("Revenue growth >15%");
        }
        // Profit margin
        if let Some(m) = q.profit_margins
            && m > 0.2
        {
            score += 5.0;
            reasons.push("High margins");
        }
        // Dividend
        if let Some(d) = q.trailing_annual_dividend_yield
            && d > 0.02
        {
            score += 3.0;
            reasons.push("Dividend >2%");
        }
        // Volume
        if let (Some(v), Some(avg)) = (q.regular_market_volume, q.average_daily_volume_3_month)
            && avg > 0
            && v as f64 / avg as f64 > 1.3
        {
            score += 5.0;
            reasons.push("High volume");
        }
        score = score.clamp(0.0, 100.0);
        scored.push((q, score, reasons));
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    println!(
        "  {:<14} {:>10} {:>8} {:>7} {}",
        "Symbol".bold(),
        "Price".bold(),
        "Chg%".bold(),
        "Score".bold(),
        "Reasons".bold()
    );
    println!("  {}", "─".repeat(72).dimmed());

    for (q, score, reasons) in scored.iter().take(15) {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let cur = q.currency.as_deref();
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let pct_str = if pct >= 0.0 {
            format!("+{:.2}%", pct).green().to_string()
        } else {
            format!("{:.2}%", pct).red().to_string()
        };
        let score_str = if *score >= 75.0 {
            format!("{:.0}", score).green().bold().to_string()
        } else if *score >= 60.0 {
            format!("{:.0}", score).yellow().to_string()
        } else {
            format!("{:.0}", score).dimmed().to_string()
        };
        let reasons_str = reasons.join(", ");
        println!(
            "  {:<14} {:>10} {:>8} {:>7} {}",
            sym.cyan(),
            format_price(q.regular_market_price.unwrap_or(0.0), cur),
            pct_str,
            score_str,
            reasons_str.dimmed()
        );
    }
    println!(
        "\n  {}",
        "Scores are algorithmic. Always do your own research."
            .dimmed()
            .italic()
    );
    println!();
    Ok(())
}

// ── Sentiment / Fear & Greed ──

pub async fn cmd_earnings(_market: Market) -> Result<()> {
    let client = YahooClient::new().await?;

    // Check portfolio + watchlist stocks
    let portfolio = Portfolio::load()?;
    let wl = Watchlist::load()?;
    let mut all_symbols: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    for s in &wl.symbols {
        if !all_symbols.contains(s) {
            all_symbols.push(s.clone());
        }
    }

    if all_symbols.is_empty() {
        println!(
            "\n  {}",
            "Add stocks to portfolio or watchlist first.".dimmed()
        );
        return Ok(());
    }

    let sym_refs: Vec<&str> = all_symbols.iter().map(|s| s.as_str()).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header("Earnings Overview");
    println!(
        "  {:<14} {:>10} {:>10} {:>10} {:>10}",
        "Symbol".bold(),
        "Price".bold(),
        "EPS TTM".bold(),
        "EPS Fwd".bold(),
        "P/E Fwd".bold()
    );
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
            (Some(t), Some(f)) if f > t && t > 0.0 => {
                growing += 1;
                "▲".green().to_string()
            }
            (Some(t), Some(f)) if f < t && t > 0.0 => {
                declining += 1;
                "▼".red().to_string()
            }
            _ => "—".dimmed().to_string(),
        };

        println!(
            "  {:<14} {:>10} {:>10} {:>10} {:>10} {}",
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
    println!(
        "  {} {} stocks with declining earnings",
        "▼".red(),
        declining
    );
    println!();
    Ok(())
}

// ── Long-Term Wealth Bot ──

pub async fn cmd_rotation(market: Market) -> Result<()> {
    market.require_sectors("rotation")?;
    let client = YahooClient::new().await?;
    let sectors = market.sectors();
    let title = format!("{} Sector Rotation", market.label());
    let syms: Vec<&str> = sectors.iter().map(|(s, _)| *s).collect();
    let quotes = client.get_quote(&syms).await?;

    print_header(&title);
    println!(
        "  {:<16} {:>10} {:>10} {:>10} {:>10}",
        "Sector".bold(),
        "Price".bold(),
        "Change%".bold(),
        "vs 50MA".bold(),
        "Signal".bold()
    );
    println!("  {}", "─".repeat(60).dimmed());

    for (i, q) in quotes.iter().enumerate() {
        let name = sectors.get(i).map(|(_, n)| *n).unwrap_or("???");
        let cur = q.currency.as_deref();
        let price = q.regular_market_price.unwrap_or(0.0);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let ma50 = q.fifty_day_average.unwrap_or(price);
        let vs_ma = if ma50.abs() > f64::EPSILON {
            ((price - ma50) / ma50) * 100.0
        } else {
            0.0
        };
        let signal = if vs_ma > 3.0 && pct > 0.0 {
            "OVERWEIGHT".green().bold().to_string()
        } else if vs_ma < -3.0 {
            "UNDERWEIGHT".red().to_string()
        } else {
            "NEUTRAL".yellow().to_string()
        };
        let pct_str = if pct >= 0.0 {
            format!("{:+.2}%", pct).green().to_string()
        } else {
            format!("{:+.2}%", pct).red().to_string()
        };
        let ma_str = if vs_ma >= 0.0 {
            format!("{:+.1}%", vs_ma).green().to_string()
        } else {
            format!("{:+.1}%", vs_ma).red().to_string()
        };
        println!(
            "  {:<16} {:>10} {:>10} {:>10} {:>10}",
            name.bold(),
            format_price(price, cur),
            pct_str,
            ma_str,
            signal
        );
    }
    println!(
        "\n  {} Overweight sectors above 50-day MA with positive momentum.",
        "→".cyan()
    );
    println!(
        "  {} Underweight sectors below 50-day MA trending down.",
        "→".cyan()
    );
    println!();
    Ok(())
}

// ── Quality-Growth Matrix ──

pub async fn cmd_matrix(market: Market) -> Result<()> {
    market.require_stocks("matrix")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header("Quality vs Growth Matrix");

    // Quadrants: High Quality + High Growth = STARS, etc.
    let mut stars = Vec::new();
    let mut quality_plays = Vec::new();
    let mut growth_plays = Vec::new();
    let mut laggards = Vec::new();

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let roe = q.return_on_equity.unwrap_or(0.0);
        let margin = q.profit_margins.unwrap_or(0.0);
        let rev_growth = q.revenue_growth.unwrap_or(0.0);
        let quality = roe * 50.0 + margin * 50.0; // combined quality score
        let growth = rev_growth * 100.0;

        let entry = (
            sym.to_string(),
            quality,
            growth,
            q.trailing_pe.unwrap_or(0.0),
        );
        if quality > 10.0 && growth > 10.0 {
            stars.push(entry);
        } else if quality > 10.0 {
            quality_plays.push(entry);
        } else if growth > 10.0 {
            growth_plays.push(entry);
        } else {
            laggards.push(entry);
        }
    }

    let print_quad = |name: &str, _color: &str, items: &[(String, f64, f64, f64)]| {
        print_section(name);
        if items.is_empty() {
            println!("  {}", "None".dimmed());
            return;
        }
        for (sym, q, g, pe) in items.iter().take(8) {
            let sym_str = sym.cyan();
            println!(
                "  {:<14} Quality: {:>5.1}  Growth: {:>5.1}%  P/E: {:>5.1}",
                sym_str, q, g, pe
            );
        }
    };

    print_quad("Stars (High Quality + High Growth)", "green", &stars);
    print_quad(
        "Quality Plays (High Quality, Low Growth)",
        "cyan",
        &quality_plays,
    );
    print_quad(
        "Growth Plays (High Growth, Lower Quality)",
        "yellow",
        &growth_plays,
    );
    print_quad("Laggards", "red", &laggards);
    println!();
    Ok(())
}

// ── Earnings Surprise ──

pub async fn cmd_surprise(market: Market) -> Result<()> {
    market.require_stocks("surprise")?;
    let client = YahooClient::new().await?;
    let symbols = market.universe();
    let sym_refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let quotes = client.get_quote(&sym_refs).await?;

    print_header("Earnings Momentum");
    println!(
        "  {:<14} {:>10} {:>10} {:>10} {:>8} {:>10}",
        "Symbol".bold(),
        "EPS TTM".bold(),
        "EPS Fwd".bold(),
        "Growth%".bold(),
        "P/E".bold(),
        "Signal".bold()
    );
    println!("  {}", "─".repeat(66).dimmed());

    let mut accelerating = Vec::new();
    let mut decelerating = Vec::new();

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let eps_ttm = q.eps_trailing_twelve_months.unwrap_or(0.0);
        let eps_fwd = q.eps_forward.unwrap_or(0.0);
        if eps_ttm <= 0.0 {
            continue;
        }
        let growth = ((eps_fwd / eps_ttm) - 1.0) * 100.0;
        let pe = q.trailing_pe.unwrap_or(0.0);
        let signal = if growth > 15.0 {
            accelerating.push(sym.to_string());
            "ACCELERATING".green().bold().to_string()
        } else if growth > 0.0 {
            "Growing".green().to_string()
        } else if growth > -10.0 {
            "Slowing".yellow().to_string()
        } else {
            decelerating.push(sym.to_string());
            "DECELERATING".red().bold().to_string()
        };
        let g_str = if growth >= 0.0 {
            format!("+{:.1}%", growth).green().to_string()
        } else {
            format!("{:.1}%", growth).red().to_string()
        };
        println!(
            "  {:<14} {:>10.2} {:>10.2} {:>10} {:>8.1} {:>10}",
            sym.cyan(),
            eps_ttm,
            eps_fwd,
            g_str,
            pe,
            signal
        );
    }

    print_section("Summary");
    println!(
        "  {} {} stocks with accelerating earnings",
        "▲".green(),
        accelerating.len()
    );
    println!(
        "  {} {} stocks with decelerating earnings",
        "▼".red(),
        decelerating.len()
    );
    println!();
    Ok(())
}

// ── Dividend Calendar ──

pub async fn cmd_heatmap(market: Market, universe: Option<market::Universe>) -> Result<()> {
    check_universe(market, universe)?;
    let client = YahooClient::new().await?;
    let (symbols, scope) = scan_symbols(&client, market, universe).await?;
    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
    let mut quotes = client.get_quote(&refs).await?;
    quotes.sort_by(|a, b| {
        b.regular_market_change_percent
            .unwrap_or(0.0)
            .partial_cmp(&a.regular_market_change_percent.unwrap_or(0.0))
            .unwrap()
    });

    print_header(&format!("{} Market Heatmap", scope));
    println!();

    for q in &quotes {
        let sym = q.symbol.as_deref().unwrap_or("???");
        let short = sym.split('.').next().unwrap_or(sym);
        let pct = q.regular_market_change_percent.unwrap_or(0.0);
        let block_count = (pct.abs() * 3.0).clamp(1.0, 15.0) as usize;
        let blocks = "█".repeat(block_count);
        let (colored_blocks, pct_str) = if pct >= 0.0 {
            (
                blocks.green().to_string(),
                format!("{:+.2}%", pct).green().to_string(),
            )
        } else {
            (
                blocks.red().to_string(),
                format!("{:+.2}%", pct).red().to_string(),
            )
        };
        println!("  {:<12} {} {}", short, colored_blocks, pct_str);
    }
    println!();
    Ok(())
}

// ── Patterns (candlestick pattern detection) ──

pub async fn cmd_sectorcmp(sector1: &str, sector2: &str, market: Market) -> Result<()> {
    market.require_sectors("sectorcmp")?;
    let client = YahooClient::new().await?;
    let sectors = market.sectors();
    let find = |name: &str| -> Option<&str> {
        sectors
            .iter()
            .find(|(_, n)| n.to_lowercase().contains(&name.to_lowercase()))
            .map(|(s, _)| *s)
    };
    let sym1 = find(sector1).context(format!("Sector '{}' not found", sector1))?;
    let sym2 = find(sector2).context(format!("Sector '{}' not found", sector2))?;
    let quotes = client.get_quote(&[sym1, sym2]).await?;
    if quotes.len() < 2 {
        bail!("Could not fetch both sectors");
    }

    print_header(&format!("{} vs {}", sector1, sector2));
    println!(
        "  {:<20} {:>14} {:>14}",
        "Metric".bold(),
        sector1.bold().cyan(),
        sector2.bold().cyan()
    );
    println!("  {}", "─".repeat(50).dimmed());

    let rows: Vec<(&str, Box<dyn Fn(&crate::api::Quote) -> String>)> = vec![
        (
            "Price",
            Box::new(|q: &crate::api::Quote| {
                format_price(q.regular_market_price.unwrap_or(0.0), q.currency.as_deref())
            }),
        ),
        (
            "Change %",
            Box::new(|q| {
                let v = q.regular_market_change_percent.unwrap_or(0.0);
                if v >= 0.0 {
                    format!("{:+.2}%", v).green().to_string()
                } else {
                    format!("{:+.2}%", v).red().to_string()
                }
            }),
        ),
        (
            "P/E",
            Box::new(|q| q.trailing_pe.map_or("—".into(), |v| format!("{:.1}", v))),
        ),
        (
            "50-Day MA",
            Box::new(|q| format_optional_price(q.fifty_day_average, q.currency.as_deref())),
        ),
        (
            "52W High",
            Box::new(|q| format_optional_price(q.fifty_two_week_high, q.currency.as_deref())),
        ),
        (
            "52W Low",
            Box::new(|q| format_optional_price(q.fifty_two_week_low, q.currency.as_deref())),
        ),
    ];

    for (label, func) in &rows {
        println!(
            "  {:<20} {:>14} {:>14}",
            (*label).dimmed(),
            func(&quotes[0]),
            func(&quotes[1])
        );
    }

    // Verdict
    let pct1 = quotes[0].regular_market_change_percent.unwrap_or(0.0);
    let pct2 = quotes[1].regular_market_change_percent.unwrap_or(0.0);
    let winner = if pct1 > pct2 { sector1 } else { sector2 };
    println!(
        "\n  {} {} is outperforming today.",
        "→".cyan(),
        winner.green().bold()
    );
    println!();
    Ok(())
}

// ── What-If Calculator ──

pub async fn cmd_ipo() -> Result<()> {
    let client = YahooClient::new().await?;

    let (yahoo_ipo, google_ipo, google_listing) = tokio::join!(
        fetch_yahoo_json_news(&client, "IPO initial public offering", 12),
        fetch_google_news_rss(&client, "IPO stock market listing 2025"),
        fetch_google_news_rss(&client, "new IPO listing NSE BSE 2025"),
    );

    let all: Vec<NewsItem> = [yahoo_ipo, google_ipo, google_listing].concat();
    let collated = collate_news(all);

    print_header("IPO Calendar & News — 3 sources");
    print_news_items(&collated, 15);
    println!();
    Ok(())
}

// ── Options Overview (Put/Call data from quote) ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn universes_are_for_the_indian_market_only() {
        assert!(check_universe(Market::In, Some(market::Universe::Nifty50)).is_ok());
        assert!(check_universe(Market::Us, None).is_ok());
        let err = check_universe(Market::Us, Some(market::Universe::BankNifty))
            .unwrap_err()
            .to_string();
        assert!(err.contains("-m in"), "{err}");
    }
}
