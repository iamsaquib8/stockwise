//! `stockwise today`: what changed in the portfolio today and why, the
//! market backdrop, and a news feed for holdings and the watchlist.

use super::*;
use crate::pulse::{Pulse, Row};

fn pct_colored(v: Option<f64>) -> String {
    match v {
        Some(p) if p > 0.0 => format!("{p:+.2}%").green().to_string(),
        Some(p) if p < 0.0 => format!("{p:+.2}%").red().to_string(),
        Some(p) => format!("{p:+.2}%"),
        None => "—".dimmed().to_string(),
    }
}

fn money_colored(v: Option<f64>, base: &str) -> String {
    let sym = market::currency_symbol(Some(base));
    match v {
        Some(p) if p >= 0.0 => format!("+{sym}{:.0}", p).green().to_string(),
        Some(p) => format!("-{sym}{:.0}", p.abs()).red().to_string(),
        None => "—".dimmed().to_string(),
    }
}

/// The move details after the why: σ and volume when notable.
fn stats(r: &Row) -> String {
    let mut parts = Vec::new();
    if let Some(z) = r.sigma.filter(|z| z.abs() >= 1.5) {
        parts.push(format!("{:.1}σ", z.abs()));
    }
    if let Some(v) = r.volume_x.filter(|v| *v >= 1.8) {
        parts.push(format!("{v:.1}× vol"));
    }
    if let Some(g) = r.gap_pct.filter(|g| g.abs() >= 2.0) {
        parts.push(format!("gap {g:+.1}%"));
    }
    if let Some(b) = r.range_break {
        parts.push(format!("52w {b}"));
    }
    for e in r.events.iter().take(1) {
        parts.push(format!("{} {}", e.kind.label().to_lowercase(), e.date.format("%d %b")));
    }
    parts.join(", ")
}

/// The backdrop line, e.g. `NIFTY 50 24,310 +0.4% · …`.
pub(crate) fn backdrop_line(p: &Pulse) -> String {
    p.backdrop
        .iter()
        .filter(|i| i.price.is_some())
        .map(|i| {
            format!(
                "{} {}",
                i.name,
                i.change_pct
                    .map(|c| format!("{c:+.1}%"))
                    .unwrap_or_else(|| "—".into())
            )
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// `today` as plain text (Telegram, MCP text content).
pub(crate) fn plain_today(p: &Pulse, feed_limit: usize) -> String {
    let mut out = Vec::new();
    let sym = market::currency_symbol(Some(&p.base_currency));
    if let (Some(v), Some(d)) = (p.value, p.day_pnl) {
        out.push(format!(
            "Portfolio {sym}{v:.0} · day {}{sym}{:.0}{}",
            if d >= 0.0 { "+" } else { "-" },
            d.abs(),
            p.day_pnl_pct.map(|x| format!(" ({x:+.2}%)")).unwrap_or_default()
        ));
    }
    let b = backdrop_line(p);
    if !b.is_empty() {
        out.push(format!("Market: {b}"));
    }
    let rows = p.by_impact();
    if !rows.is_empty() {
        out.push(String::new());
        out.push("Holdings (by day impact):".into());
        // Biggest absolute impact first.
        let mut rows = rows;
        rows.sort_by(|a, b| {
            b.day_pnl
                .unwrap_or(0.0)
                .abs()
                .total_cmp(&a.day_pnl.unwrap_or(0.0).abs())
        });
        for r in rows.iter().take(12) {
            out.push(format!(
                "• {} {} — {}{}",
                r.symbol,
                r.change_pct.map(|c| format!("{c:+.1}%")).unwrap_or_else(|| "—".into()),
                r.why.line(),
                r.action.as_ref().map(|a| format!(" [{a}]")).unwrap_or_default()
            ));
        }
    }
    let watched: Vec<&Row> = p.watched().collect();
    if !watched.is_empty() {
        out.push(String::new());
        out.push("Watchlist:".into());
        for r in watched.iter().take(8) {
            out.push(format!(
                "• {} {} — {}",
                r.symbol,
                r.change_pct.map(|c| format!("{c:+.1}%")).unwrap_or_else(|| "—".into()),
                r.why.line()
            ));
        }
    }
    let feed = feed(p);
    if !feed.is_empty() {
        out.push(String::new());
        out.push("News:".into());
        for n in feed.iter().take(feed_limit) {
            out.push(format!(
                "• {} — {} ({})",
                crate::pulse::clip(&n.title, 100),
                n.symbol.as_deref().unwrap_or(""),
                n.source
            ));
        }
    }
    out.join("\n")
}

/// The merged news feed: every row's items, ranked by weight × importance.
pub(crate) fn feed(p: &Pulse) -> Vec<crate::news::NewsItem> {
    let weights: Vec<(String, f64)> = p
        .rows
        .iter()
        .map(|r| (r.symbol.clone(), if r.held { r.weight } else { crate::news::WATCHLIST_WEIGHT }))
        .collect();
    crate::news::rank_feed(
        p.rows.iter().flat_map(|r| r.news.iter().cloned()).collect(),
        &weights,
    )
}

pub async fn cmd_today(market: Market, days: u32, no_news: bool, limit: usize, json: bool) -> Result<()> {
    let client = YahooClient::new().await?;
    let pulse = crate::pulse::gather(
        &client,
        market,
        crate::pulse::Options {
            days: days.max(1),
            watchlist: true,
            news: !no_news,
            events: true,
            event_days: 3,
        },
    )
    .await?;
    if json {
        return output::print_json(&serde_json::json!({
            "pulse": pulse,
            "feed": feed(&pulse).into_iter().take(limit).collect::<Vec<_>>(),
        }));
    }
    let now = chrono::Utc::now();
    let phase = crate::clock::market_phase(market, now);
    print_header(&format!(
        "Today — {} ({:?})",
        crate::clock::format_local(market, now),
        phase
    ));
    let base = pulse.base_currency.clone();
    if pulse.rows.iter().all(|r| !r.held) {
        println!(
            "  {} No holdings yet — add some with {}.",
            "→".dimmed(),
            "stockwise portfolio add".cyan()
        );
    } else {
        println!(
            "  Portfolio {}  day {} ({})",
            pulse
                .value
                .map(|v| format!("{}{:.0}", market::currency_symbol(Some(&base)), v))
                .unwrap_or_else(|| "—".into())
                .bold(),
            money_colored(pulse.day_pnl, &base),
            pct_colored(pulse.day_pnl_pct)
        );
    }
    let b = backdrop_line(&pulse);
    if !b.is_empty() {
        println!("  Market: {}", b.dimmed());
    }

    let mut rows: Vec<&Row> = pulse.held().collect();
    rows.sort_by(|a, b| {
        b.day_pnl
            .unwrap_or(0.0)
            .abs()
            .total_cmp(&a.day_pnl.unwrap_or(0.0).abs())
    });
    if !rows.is_empty() {
        print_section("Holdings by day impact");
        for r in &rows {
            let st = stats(r);
            println!(
                "  {:<14} {:>8} {:>10}  {}{}{}",
                r.symbol.cyan(),
                pct_colored(r.change_pct),
                money_colored(r.day_pnl, &base),
                r.why.line(),
                if st.is_empty() { String::new() } else { format!("  {}", format!("({st})").dimmed()) },
                r.action.as_ref().map(|a| format!("  [{a}]").bold().to_string()).unwrap_or_default()
            );
        }
    }
    let watched: Vec<&Row> = pulse.watched().collect();
    if !watched.is_empty() {
        print_section("Watchlist");
        for r in &watched {
            let st = stats(r);
            println!(
                "  {:<14} {:>8}  {}{}",
                r.symbol.cyan(),
                pct_colored(r.change_pct),
                r.why.line(),
                if st.is_empty() { String::new() } else { format!("  {}", format!("({st})").dimmed()) }
            );
        }
    }
    if !no_news {
        let items = feed(&pulse);
        print_section(&format!(
            "News feed — last {} day{}, by weight × importance",
            days,
            if days == 1 { "" } else { "s" }
        ));
        if items.is_empty() {
            println!("  {}", "No fresh headlines or filings.".dimmed());
        }
        for n in items.iter().take(limit) {
            let tag = if n.official {
                format!("[{}]", n.source).yellow().to_string()
            } else {
                format!("[{}]", n.publisher).dimmed().to_string()
            };
            println!(
                "  {} {} {}  {} {}",
                "●".cyan(),
                n.symbol.as_deref().unwrap_or("").cyan(),
                crate::pulse::clip(&n.title, 110).bold(),
                tag,
                n.local_time().unwrap_or_default().dimmed()
            );
            if let Some(l) = &n.link {
                println!("      {}", l.dimmed());
            }
        }
    }
    for w in &pulse.warnings {
        println!("  {} {}", "!".yellow(), w.dimmed());
    }
    println!(
        "\n  {} Moves are explained by the most important fresh headline or filing; \"unexplained\" means an unusual move (2.5σ or more against the benchmark) with none found.\n",
        "→".dimmed()
    );
    Ok(())
}
