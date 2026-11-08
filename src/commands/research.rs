use super::*;
use crate::events;
use crate::insights;
use crate::research;
use crate::verdict;

#[derive(Debug, Serialize)]
pub(crate) struct EventCsvRow {
    pub(crate) date: String,
    pub(crate) kind: String,
    pub(crate) official: bool,
    pub(crate) sentiment: f64,
    pub(crate) importance: f64,
    pub(crate) stock_reaction_pct: Option<f64>,
    pub(crate) headline: String,
}

impl CsvRow for EventCsvRow {
    const COLUMNS: &'static [&'static str] = &[
        "date",
        "kind",
        "official",
        "sentiment",
        "importance",
        "stock_reaction_pct",
        "headline",
    ];
}

pub async fn cmd_events(
    symbol: &str,
    days: i64,
    use_ai: bool,
    market: Market,
    fmt: Format,
) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = research::fetch(&client, symbol, market, days, research::Needs::ALL).await?;
    let mut evts = events::build_events(&r);

    if use_ai && fmt.is_table() {
        println!(
            "  {} Synthesizing event summaries with local AI...",
            "●".cyan()
        );
        events::summarize_with_ai(&mut evts, &r.short_name, 5).await;
    }

    let upcoming = events::upcoming(&r);
    match fmt {
        Format::Json => {
            let digest = events::digest(&evts);
            let out = serde_json::json!({
                "symbol": r.symbol,
                "name": r.name,
                "days": days,
                "digest": digest,
                "upcoming": upcoming,
                "events": evts,
            });
            return output::print_json(&out);
        }
        Format::Csv => {
            let rows: Vec<EventCsvRow> = evts
                .iter()
                .map(|e| EventCsvRow {
                    date: chrono::DateTime::from_timestamp(e.ts, 0)
                        .map(|d| d.date_naive().to_string())
                        .unwrap_or_default(),
                    kind: e.kind.label().to_string(),
                    official: e.official,
                    sentiment: round(e.sentiment, 3),
                    importance: round(e.importance, 2),
                    stock_reaction_pct: e.reaction.as_ref().map(|rx| round(rx.stock * 100.0, 2)),
                    headline: e.headline.clone(),
                })
                .collect();
            return output::print_csv(&rows);
        }
        Format::Table => {}
    }

    print_header(&format!(
        "Key Events: {} ({}) — Last {} Days",
        r.symbol, r.name, days
    ));
    events::print_digest(&evts, &r);

    if !upcoming.is_empty() {
        print_section("Upcoming Catalysts & Dates");
        events::print_upcoming(&upcoming, r.now);
    }

    print_section(&format!("Timeline ({} Events)", evts.len()));
    if evts.is_empty() {
        println!(
            "  {}",
            "No significant events found in this time window.".dimmed()
        );
    } else {
        for e in &evts {
            events::print_event(e, &r);
        }
    }

    println!();
    println!(
        "  {} See the scored summary: '{}'",
        "💡".yellow(),
        format!("stockwise verdict {}", symbol).cyan()
    );
    println!();
    Ok(())
}

pub async fn cmd_insights(symbol: &str, market: Market, json: bool) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = research::fetch(&client, symbol, market, 60, research::Needs::VERDICT).await?;
    let cards = insights::generate_rich_insights(&r);

    if json {
        let out = serde_json::json!({
            "symbol": r.symbol,
            "name": r.name,
            "price": r.price(),
            "currency": r.currency,
            "shareholding": r.holdings,
            "insights": cards,
        });
        return output::print_json(&out);
    }

    print_header(&format!("Insights: {} ({})", r.symbol, r.name));
    insights::print_insight_cards(&cards, None);

    println!(
        "  {} For the 6-pillar score and its data coverage, run '{}'",
        "💡".yellow(),
        format!("stockwise verdict {}", symbol).cyan()
    );
    println!();
    Ok(())
}

pub async fn cmd_verdict(symbol: &str, market: Market, json: bool, explain: bool) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = research::fetch(&client, symbol, market, 60, research::Needs::VERDICT).await?;
    let cards = insights::generate_rich_insights(&r);
    let decision = verdict::generate_verdict(&r, &cards);

    if json {
        return output::print_json(&decision);
    }

    verdict::print_verdict_with(&decision, explain);
    Ok(())
}
