use super::*;
use crate::events;
use crate::insights;
use crate::research;
use crate::verdict;

pub async fn cmd_events(symbol: &str, days: i64, use_ai: bool, market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = research::fetch(&client, symbol, market, days).await?;
    let mut evts = events::build_events(&r);

    if use_ai {
        println!(
            "  {} Synthesizing event summaries with local AI...",
            "●".cyan()
        );
        events::summarize_with_ai(&mut evts, &r.short_name, 5).await;
    }

    print_header(&format!(
        "Key Events: {} ({}) — Last {} Days",
        r.symbol, r.name, days
    ));
    events::print_digest(&evts, &r);

    let upcoming = events::upcoming(&r);
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
        "  {} For actionable Buy/Sell guidance, run '{}'",
        "💡".yellow(),
        format!("stockwise verdict {}", symbol).cyan()
    );
    println!();
    Ok(())
}

pub async fn cmd_insights(symbol: &str, market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = research::fetch(&client, symbol, market, 60).await?;
    let cards = insights::generate_rich_insights(&r);

    print_header(&format!("Insights: {} ({})", r.symbol, r.name));
    insights::print_insight_cards(&cards, None);

    println!(
        "  {} For the definitive Buy/Sell decision and scoring, run '{}'",
        "💡".yellow(),
        format!("stockwise verdict {}", symbol).cyan()
    );
    println!();
    Ok(())
}

pub async fn cmd_verdict(symbol: &str, market: Market) -> Result<()> {
    let client = YahooClient::new().await?;
    let r = research::fetch(&client, symbol, market, 60).await?;
    let cards = insights::generate_rich_insights(&r);
    let decision = verdict::generate_verdict(&r, &cards);

    verdict::print_verdict(&decision);
    Ok(())
}
