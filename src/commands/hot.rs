//! `stockwise hot`: your hottest holdings and watchlist names, then what is
//! hot in the market. Data, not recommendations.

use super::*;
use crate::hot::{Deal, Flows, Group, Heat, Mover, OiMove};
use std::time::Duration;

/// Per-source timeout for the market sections.
const SOURCE_TIMEOUT: Duration = Duration::from_secs(20);

/// Large caps for cross-market movers.
const US_LEADERS: &[&str] = &[
    "AAPL", "MSFT", "NVDA", "GOOGL", "AMZN", "META", "AVGO", "TSLA", "BRK-B", "JPM", "LLY", "V",
    "UNH", "XOM", "MA", "COST", "HD", "PG", "NFLX", "AMD", "ORCL", "CRM", "ADBE", "WMT", "BAC",
];
const IN_LEADERS: &[&str] = &[
    "RELIANCE.NS",
    "TCS.NS",
    "HDFCBANK.NS",
    "ICICIBANK.NS",
    "INFY.NS",
    "BHARTIARTL.NS",
    "SBIN.NS",
    "ITC.NS",
    "LT.NS",
    "HINDUNILVR.NS",
    "KOTAKBANK.NS",
    "AXISBANK.NS",
    "BAJFINANCE.NS",
    "MARUTI.NS",
    "SUNPHARMA.NS",
    "HCLTECH.NS",
    "M&M.NS",
    "TITAN.NS",
    "NTPC.NS",
    "ULTRACEMCO.NS",
    "ONGC.NS",
    "TATASTEEL.NS",
    "POWERGRID.NS",
    "ADANIENT.NS",
    "WIPRO.NS",
];

/// One of your names, with its heat.
#[derive(Debug, Clone, Serialize)]
pub struct HotRow {
    pub symbol: String,
    pub held: bool,
    pub weight: f64,
    pub change_pct: Option<f64>,
    pub sigma: Option<f64>,
    pub volume_x: Option<f64>,
    pub heat: Heat,
    pub why: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub events: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
}

/// Everything `hot` shows.
#[derive(Debug, Clone, Serialize, Default)]
pub struct HotReport {
    pub as_of: i64,
    pub market: String,
    pub scope: String,
    pub yours: Vec<HotRow>,
    pub top_movers: Vec<Mover>,
    pub unusual_volume: Vec<Mover>,
    pub gaps: Vec<Mover>,
    pub new_highs: Vec<Mover>,
    pub new_lows: Vec<Mover>,
    pub deals: Vec<Deal>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub flows: Option<Flows>,
    pub fo_ban: Vec<String>,
    pub oi_buildup: Vec<OiMove>,
    pub sectors: Vec<Group>,
    pub themes: Vec<Group>,
    pub most_mentioned: Vec<(String, usize)>,
    pub cross_market: Vec<Mover>,
    pub warnings: Vec<String>,
}

async fn timed<T>(
    what: &str,
    f: impl std::future::Future<Output = Result<T>>,
    warnings: &mut Vec<String>,
) -> Option<T> {
    match tokio::time::timeout(SOURCE_TIMEOUT, f).await {
        Ok(Ok(v)) => Some(v),
        Ok(Err(e)) => {
            warnings.push(format!("{what}: {e:#}"));
            None
        }
        Err(_) => {
            warnings.push(format!("{what}: timed out"));
            None
        }
    }
}

/// Headlines for the "most mentioned" count: Google News business and
/// market searches in the market's locale.
async fn market_titles(client: &YahooClient, india: bool) -> Result<Vec<String>> {
    let locale = if india {
        "hl=en-IN&gl=IN&ceid=IN:en"
    } else {
        "hl=en-US&gl=US&ceid=US:en"
    };
    let urls = [
        format!("https://news.google.com/rss/headlines/section/topic/BUSINESS?{locale}"),
        format!(
            "https://news.google.com/rss/search?q={}&{locale}",
            crate::research::url_encode(if india {
                "stocks shares NSE when:1d"
            } else {
                "stocks shares NYSE Nasdaq when:1d"
            })
        ),
    ];
    let mut titles = Vec::new();
    let mut last_err = None;
    for u in urls {
        match client.raw_get_text(&u).await {
            Ok(xml) => titles.extend(
                crate::research::parse_rss(&xml, "Google News")
                    .into_iter()
                    .map(|h| h.title),
            ),
            Err(e) => last_err = Some(e),
        }
    }
    if titles.is_empty()
        && let Some(e) = last_err
    {
        return Err(e);
    }
    Ok(titles)
}

/// Build the report. `limit` caps each market list.
pub(crate) async fn build_hot(
    client: &YahooClient,
    market: Market,
    cap: Option<market::CapFilter>,
    universe: Option<market::Universe>,
    limit: usize,
    with_yours: bool,
) -> Result<HotReport> {
    let india = market == Market::In;
    let now = chrono::Utc::now();
    let mut warnings = Vec::new();
    let mut r = HotReport {
        as_of: now.timestamp(),
        market: market.id().to_string(),
        ..Default::default()
    };

    // Your investments.
    if with_yours {
        match crate::pulse::gather(client, market, crate::pulse::Options::default()).await {
            Ok(p) => {
                r.yours = crate::hot::rank(&p.rows)
                    .into_iter()
                    .map(|(row, heat)| HotRow {
                        symbol: row.symbol.clone(),
                        held: row.held,
                        weight: row.weight,
                        change_pct: row.change_pct,
                        sigma: row.sigma,
                        volume_x: row.volume_x,
                        heat,
                        why: row.why.line(),
                        events: row
                            .events
                            .iter()
                            .map(|e| {
                                format!(
                                    "{} {}",
                                    e.kind.label().to_lowercase(),
                                    e.date.format("%a %d %b")
                                )
                            })
                            .collect(),
                        action: row.action.clone(),
                    })
                    .collect();
                warnings.extend(p.warnings);
            }
            Err(e) => warnings.push(format!("Your investments: {e:#}")),
        }
    }

    // The universe.
    let elapsed = crate::hot::session_elapsed(market, now);
    let small = matches!(
        cap,
        Some(market::CapFilter::Small) | Some(market::CapFilter::Micro)
    );
    let min_value = match (india, small) {
        (true, false) => 5e7,
        (true, true) => 1e7,
        (false, false) => 5e6,
        (false, true) => 1e6,
    };
    match super::markets::scan_symbols(client, market, universe).await {
        Ok(scan) => {
            r.scope = super::markets::cap_scope(&scan.scope, cap);
            match super::markets::scan_quotes(client, market, &scan, cap, false).await {
                Ok(mut quotes) => {
                    // Cached snapshots (used for `--cap`) carry price and change
                    // but not the volume averages, opens and 52-week range the
                    // scanners read: ask Yahoo for those live.
                    let thin = quotes
                        .iter()
                        .filter(|q| q.average_daily_volume_3_month.is_none())
                        .count();
                    if thin * 2 > quotes.len() {
                        let syms: Vec<String> =
                            quotes.iter().filter_map(|q| q.symbol.clone()).collect();
                        let refs: Vec<&str> = syms.iter().map(String::as_str).collect();
                        if let Some(live) =
                            timed("Live quotes", client.get_quote(&refs), &mut warnings).await
                            && !live.is_empty()
                        {
                            quotes = live;
                        }
                    }
                    r.top_movers = crate::hot::top_movers(&quotes, elapsed, min_value, limit);
                    r.unusual_volume =
                        crate::hot::unusual_volume(&quotes, elapsed, min_value, limit);
                    r.gaps =
                        crate::hot::gaps(&quotes, if small { 4.0 } else { 3.0 }, min_value, limit);
                    r.new_highs = crate::hot::range_breaks(&quotes, true, min_value, limit);
                    r.new_lows = crate::hot::range_breaks(&quotes, false, min_value, limit);
                    if let Some(titles) =
                        timed("News mentions", market_titles(client, india), &mut warnings).await
                    {
                        r.most_mentioned =
                            crate::hot::most_mentioned(&titles, &quotes, limit.min(8));
                    }
                    if india {
                        // OI build-up needs the day changes of F&O stocks.
                        if let Some(v) = timed(
                            "NSE OI build-up",
                            client.nse_get_json(
                                "https://www.nseindia.com/api/live-analysis-oi-spurts-underlyings",
                                Duration::from_secs(300),
                            ),
                            &mut warnings,
                        )
                        .await
                        {
                            r.oi_buildup = crate::hot::parse_oi_spurts(&v, &quotes, limit);
                        }
                    }
                }
                Err(e) => warnings.push(format!("Universe quotes: {e:#}")),
            }
        }
        Err(e) => warnings.push(format!("Universe: {e:#}")),
    }

    // Sectors and themes in one quote request.
    let mut syms: Vec<&str> = crate::hot::sector_symbols(india)
        .iter()
        .map(|(s, _)| *s)
        .collect();
    for t in crate::hot::THEMES {
        syms.extend(if india { t.india } else { t.us });
    }
    syms.sort();
    syms.dedup();
    if let Some(qs) = timed("Sectors and themes", client.get_quote(&syms), &mut warnings).await {
        r.sectors = crate::hot::sector_moves(&qs, india);
        r.themes = crate::hot::theme_moves(&qs, india);
    }

    // Cross-market movers among large caps of India and the US (#358).
    let mut leaders: Vec<&str> = US_LEADERS.to_vec();
    leaders.extend(IN_LEADERS);
    if let Some(qs) = timed("Cross-market", client.get_quote(&leaders), &mut warnings).await {
        let us_elapsed = crate::hot::session_elapsed(Market::Us, now);
        let in_elapsed = crate::hot::session_elapsed(Market::In, now);
        let mut movers: Vec<Mover> = qs
            .iter()
            .filter_map(|q| {
                let sym = q.symbol.as_deref()?;
                let el = if sym.ends_with(".NS") {
                    in_elapsed
                } else {
                    us_elapsed
                };
                let rv = crate::hot::rvol(q, el);
                Some(Mover {
                    symbol: sym.to_string(),
                    name: q.short_name.clone(),
                    price: q.regular_market_price,
                    change_pct: q.regular_market_change_percent.filter(|c| c.is_finite()),
                    rvol: rv,
                    gap_pct: crate::hot::gap_pct(q),
                    note: if sym.ends_with(".NS") { "India" } else { "US" }.into(),
                })
            })
            .filter(|m| m.change_pct.is_some())
            .collect();
        movers.sort_by(|a, b| {
            b.change_pct
                .unwrap_or(0.0)
                .abs()
                .total_cmp(&a.change_pct.unwrap_or(0.0).abs())
        });
        movers.truncate(limit.min(8));
        r.cross_market = movers;
    }

    // NSE market data.
    if india {
        if let Some(v) = timed(
            "NSE bulk/block deals",
            client.nse_get_json(
                "https://www.nseindia.com/api/snapshot-capital-market-largedeal",
                Duration::from_secs(900),
            ),
            &mut warnings,
        )
        .await
        {
            r.deals = crate::hot::parse_large_deals(&v);
            r.deals.truncate(limit);
        }
        if let Some(v) = timed(
            "NSE FII/DII flows",
            client.nse_get_json(
                "https://www.nseindia.com/api/fiidiiTradeReact",
                Duration::from_secs(1800),
            ),
            &mut warnings,
        )
        .await
        {
            r.flows = crate::hot::parse_fii_dii(&v);
        }
        if let Some(csv) = timed(
            "NSE F&O ban list",
            client.nse_get_text(
                "https://nsearchives.nseindia.com/content/fo/fo_secban.csv",
                Duration::from_secs(3600),
            ),
            &mut warnings,
        )
        .await
        {
            r.fo_ban = crate::hot::parse_fo_ban(&csv);
        }
    }
    r.warnings = warnings;
    Ok(r)
}

fn pct(v: Option<f64>) -> String {
    match v {
        Some(p) if p > 0.0 => format!("{p:+.1}%").green().to_string(),
        Some(p) if p < 0.0 => format!("{p:+.1}%").red().to_string(),
        Some(p) => format!("{p:+.1}%"),
        None => "—".dimmed().to_string(),
    }
}

fn print_movers(title: &str, list: &[Mover]) {
    if list.is_empty() {
        return;
    }
    print_section(title);
    for m in list {
        println!(
            "  {:<16} {:>8}  {}{}",
            m.symbol.cyan(),
            pct(m.change_pct),
            m.note.dimmed(),
            m.name
                .as_deref()
                .map(|n| format!("  {}", crate::pulse::clip(n, 28).dimmed()))
                .unwrap_or_default()
        );
    }
}

fn print_groups(title: &str, list: &[Group]) {
    if list.is_empty() {
        return;
    }
    print_section(title);
    for g in list {
        let extra = match (&g.best, &g.worst) {
            (Some(b), Some(w)) if g.members > 1 => format!(
                "  best {} {:+.1}%, worst {} {:+.1}% ({}/{} priced)",
                b.0, b.1, w.0, w.1, g.priced, g.members
            ),
            _ => String::new(),
        };
        println!(
            "  {:<20} {:>8}{}",
            g.name,
            pct(g.change_pct),
            extra.dimmed()
        );
    }
}

/// The report as plain text (Telegram, MCP text content).
pub(crate) fn plain_hot(r: &HotReport, n: usize) -> String {
    let mut out = Vec::new();
    let p = |v: Option<f64>| v.map(|x| format!("{x:+.1}%")).unwrap_or_else(|| "—".into());
    if !r.yours.is_empty() {
        out.push("Your investments (hottest first):".to_string());
        for h in r.yours.iter().take(n) {
            out.push(format!(
                "• {} {} — {}{}",
                h.symbol,
                p(h.change_pct),
                h.why,
                h.action
                    .as_ref()
                    .map(|a| format!(" [{a}]"))
                    .unwrap_or_default()
            ));
        }
    }
    let list = |out: &mut Vec<String>, title: &str, v: &[Mover]| {
        if !v.is_empty() {
            out.push(String::new());
            out.push(format!("{title}:"));
            for m in v.iter().take(n) {
                out.push(format!("• {} {} {}", m.symbol, p(m.change_pct), m.note));
            }
        }
    };
    list(&mut out, "Unusual volume", &r.unusual_volume);
    list(&mut out, "Gaps", &r.gaps);
    list(&mut out, "52-week highs", &r.new_highs);
    list(&mut out, "52-week lows", &r.new_lows);
    if let Some(f) = &r.flows {
        out.push(String::new());
        out.push(format!(
            "FII/DII {}: FII {} cr, DII {} cr",
            f.date,
            f.fii_net
                .map(|v| format!("{v:+.0}"))
                .unwrap_or_else(|| "—".into()),
            f.dii_net
                .map(|v| format!("{v:+.0}"))
                .unwrap_or_else(|| "—".into())
        ));
    }
    if !r.themes.is_empty() {
        out.push(String::new());
        out.push(format!(
            "Themes: {}",
            r.themes
                .iter()
                .map(|g| format!("{} {}", g.name, p(g.change_pct)))
                .collect::<Vec<_>>()
                .join(" · ")
        ));
    }
    if !r.most_mentioned.is_empty() {
        out.push(format!(
            "Most mentioned: {}",
            r.most_mentioned
                .iter()
                .map(|(s, c)| format!("{s} ({c})"))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out.join("\n")
}

pub async fn cmd_hot(
    market: Market,
    cap: Option<market::CapFilter>,
    universe: Option<market::Universe>,
    limit: usize,
    json: bool,
) -> Result<()> {
    if !matches!(market, Market::In | Market::Us) {
        bail!("`hot` covers India (-m in) and the US (-m us)");
    }
    let client = YahooClient::new().await?;
    let r = build_hot(&client, market, cap, universe, limit, true).await?;
    if json {
        return output::print_json(&r);
    }
    let now = chrono::Utc::now();
    print_header(&format!(
        "What's hot — {} · {}",
        crate::clock::format_local(market, now),
        if r.scope.is_empty() {
            market.label().to_string()
        } else {
            r.scope.clone()
        }
    ));

    print_section("Your investments — hottest first");
    if r.yours.is_empty() {
        println!(
            "  {}",
            "Nothing stands out in your holdings or watchlist today.".dimmed()
        );
    }
    for h in r.yours.iter().take(limit) {
        let mut detail = Vec::new();
        if let Some(z) = h.sigma.filter(|z| z.abs() >= 1.0) {
            detail.push(format!("{:.1}σ", z.abs()));
        }
        if let Some(v) = h.volume_x.filter(|v| *v >= 1.5) {
            detail.push(format!("{v:.1}× vol"));
        }
        detail.extend(h.events.iter().cloned());
        if h.held {
            detail.push(format!("{:.1}% of portfolio", h.weight * 100.0));
        } else {
            detail.push("watchlist".into());
        }
        println!(
            "  {:<14} {:>8}  heat {:>4.1}  {}{}",
            h.symbol.cyan(),
            pct(h.change_pct),
            h.heat.score,
            h.why,
            h.action
                .as_ref()
                .map(|a| format!("  [{a}]").bold().to_string())
                .unwrap_or_default()
        );
        println!("  {:<14} {}", "", detail.join(" · ").dimmed());
    }

    print_movers("Biggest moves", &r.top_movers);
    print_movers(
        "Unusual volume (RVOL, paced for the time of day)",
        &r.unusual_volume,
    );
    print_movers("Gaps at the open", &r.gaps);
    print_movers("52-week highs", &r.new_highs);
    print_movers("52-week lows", &r.new_lows);
    if !r.most_mentioned.is_empty() {
        print_section("Most mentioned in today's news");
        println!(
            "  {}",
            r.most_mentioned
                .iter()
                .map(|(s, c)| format!("{} ({c})", s.cyan()))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    print_groups("Sectors", &r.sectors);
    print_groups("Themes", &r.themes);
    if let Some(f) = &r.flows {
        print_section(&format!("FII / DII flows — {} (₹ crore)", f.date));
        let v = |x: Option<f64>| match x {
            Some(n) if n >= 0.0 => format!("{n:+.0}").green().to_string(),
            Some(n) => format!("{n:+.0}").red().to_string(),
            None => "—".into(),
        };
        println!("  FII/FPI net {}   DII net {}", v(f.fii_net), v(f.dii_net));
    }
    if !r.deals.is_empty() {
        print_section("NSE bulk and block deals");
        for d in &r.deals {
            println!(
                "  {:<12} {:<5} {:<4} {:>12} @ {:<9.2} {}",
                d.symbol.cyan(),
                d.kind,
                if d.buy {
                    "buy".green().to_string()
                } else {
                    "sell".red().to_string()
                },
                format!("{:.0}", d.qty),
                d.price,
                crate::pulse::clip(&d.client, 40).dimmed()
            );
        }
    }
    if !r.fo_ban.is_empty() {
        print_section("F&O ban list (no new positions today)");
        println!("  {}", r.fo_ban.join(", "));
    }
    if !r.oi_buildup.is_empty() {
        print_section("Open-interest build-up");
        for o in &r.oi_buildup {
            println!(
                "  {:<12} OI {:>+7.1}%  price {:>8}  {}",
                o.symbol.cyan(),
                o.oi_change_pct,
                pct(o.price_change_pct),
                o.reading.dimmed()
            );
        }
    }
    print_movers(
        "Cross-market movers (India and US large caps)",
        &r.cross_market,
    );
    for w in &r.warnings {
        println!("  {} {}", "!".yellow(), w.dimmed());
    }
    println!(
        "\n  {} Heat = |excess move| in σ + volume surge + fresh news/filing importance + a date within 3 days, scaled by portfolio weight. Lists describe today's data; none is a recommendation.\n",
        "→".dimmed()
    );
    Ok(())
}
