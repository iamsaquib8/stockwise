//! `commodity`: gold and silver spot in ₹ and USD, COMEX futures, Indian
//! gold/silver ETFs and Sovereign Gold Bonds. The rows come from
//! `crate::commodity`; this file fetches quotes and prints them.

use super::*;
use crate::commodity::{self as cm, InstrumentRow, SgbRow, SpotRow};

/// Which `commodity` view to show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommodityView {
    Spot,
    Futures,
    Etfs,
    Sgb,
}

pub async fn cmd_commodity(view: CommodityView, fmt: Format) -> Result<()> {
    match view {
        CommodityView::Spot => spot(fmt).await,
        CommodityView::Futures => instruments(fmt, cm::FUTURES, true).await,
        CommodityView::Etfs => instruments(fmt, cm::ETFS, false).await,
        CommodityView::Sgb => sgb(fmt).await,
    }
}

fn print_rows<T: Serialize + CsvRow>(fmt: Format, rows: &[T]) -> Result<()> {
    match fmt {
        Format::Csv => output::print_csv(rows),
        _ => output::print_json(rows),
    }
}

fn pct(v: Option<f64>) -> String {
    match v {
        Some(p) if p >= 0.0 => format!("▲ {:.2}%", p).green().to_string(),
        Some(p) => format!("▼ {:.2}%", p).red().to_string(),
        None => "N/A".dimmed().to_string(),
    }
}

async fn spot(fmt: Format) -> Result<()> {
    let client = YahooClient::new().await?;
    let quotes = client
        .get_quote(&[cm::GOLD_FUTURE, cm::SILVER_FUTURE, cm::USDINR])
        .await?;
    let rows = cm::spot_rows(&quotes);
    if rows.iter().all(|r| r.price.is_none()) {
        bail!("No gold or silver quote from Yahoo Finance (GC=F, SI=F) — try again shortly");
    }
    if !fmt.is_table() {
        return print_rows(fmt, &rows);
    }
    print_header(t(Msg::CommoditySpotTitle));
    let mut last_metal = "";
    for r in &rows {
        let label = if r.metal == last_metal {
            String::new()
        } else {
            last_metal = &r.metal;
            metal_label(&r.metal).to_string()
        };
        println!(
            "  {} {} {}  {}",
            pad_end(&label, 8).bold(),
            pad_start(&spot_price(r), 14).bold(),
            pad_end(&unit_label(&r.unit), 6).dimmed(),
            pct(r.change_percent)
        );
    }
    if let Some(fx) = quotes
        .iter()
        .find(|q| q.symbol.as_deref() == Some(cm::USDINR))
        .and_then(|q| q.regular_market_price)
    {
        println!(
            "\n  {}",
            tf(Msg::CommoditySpotSource, &[&format!("{fx:.2}")]).dimmed()
        );
    }
    println!("  {}", t(Msg::CommodityDutyNote).dimmed());
    println!("  {}\n", t(Msg::CommodityMcxNote).dimmed());
    Ok(())
}

fn metal_label(metal: &str) -> &'static str {
    match metal {
        "gold" => t(Msg::Gold),
        _ => t(Msg::Silver),
    }
}

fn unit_label(unit: &str) -> String {
    match unit {
        "USD/oz" => "/oz".into(),
        "INR/g" => "/g".into(),
        "INR/10g" => "/10 g".into(),
        "INR/kg" => "/kg".into(),
        other => other.to_string(),
    }
}

fn spot_price(r: &SpotRow) -> String {
    r.price
        .map_or_else(|| "N/A".to_string(), |p| format_price(p, Some(&r.currency)))
}

async fn instruments(fmt: Format, list: &[(&str, &str, &str)], futures: bool) -> Result<()> {
    let client = YahooClient::new().await?;
    let symbols: Vec<&str> = list.iter().map(|(s, ..)| *s).collect();
    let quotes = client.get_quote(&symbols).await?;
    let rows = cm::instrument_rows(list, &quotes);
    if !fmt.is_table() {
        return print_rows(fmt, &rows);
    }
    print_header(t(if futures {
        Msg::CommodityFuturesTitle
    } else {
        Msg::CommodityEtfsTitle
    }));
    for r in &rows {
        print_instrument(r, futures);
    }
    println!(
        "\n  {}\n",
        t(if futures {
            Msg::CommodityFuturesNote
        } else {
            Msg::CommodityEtfsNote
        })
        .dimmed()
    );
    Ok(())
}

fn print_instrument(r: &InstrumentRow, futures: bool) {
    let price = r.price.map_or_else(
        || "N/A".dimmed().to_string(),
        |p| format_price(p, r.currency.as_deref()),
    );
    // Yahoo's short name says which contract month is front ("Gold Dec 26").
    let contract = if futures {
        r.contract.clone().unwrap_or_default()
    } else {
        String::new()
    };
    println!(
        "  {} {} {}  {}  {}  {}",
        pad_end(r.symbol.trim_end_matches(".NS"), 12).bold(),
        pad_end(&r.name, 30),
        pad_start(&price, 11).bold(),
        pad_end(&pct(r.change_percent), 9),
        pad_start(&r.volume.map(format_volume).unwrap_or_default(), 7).dimmed(),
        contract.dimmed()
    );
}

async fn sgb(fmt: Format) -> Result<()> {
    let client = YahooClient::new().await?;
    let quotes = client.get_quote(&[cm::GOLD_FUTURE, cm::USDINR]).await?;
    let gold = cm::gold_inr_per_gram(&quotes);
    let (live, problem) = match cm::nse_sgb_list().await {
        Ok(list) => (Some(list.rows), None),
        Err(e) => (None, Some(e)),
    };
    let today = crate::clock::session_date(Market::In, chrono::Utc::now());
    let rows = cm::sgb_rows(live.as_deref(), gold, today);
    if let Some(e) = &problem {
        eprintln!(
            "  {} {}",
            "!".yellow(),
            tf(Msg::SgbNoQuotes, &[&format!("{e:#}")])
        );
    }
    if !fmt.is_table() {
        return print_rows(fmt, &rows);
    }
    print_header(t(Msg::SgbTitle));
    let live_prices = live.is_some();
    if live_prices {
        println!(
            "  {} {} {} {} {} {}",
            pad_end(t(Msg::Symbol), 12).dimmed(),
            pad_end(t(Msg::SgbMatures), 9).dimmed(),
            pad_start(t(Msg::Price), 10).dimmed(),
            pad_start(t(Msg::SgbPremium), 9).dimmed(),
            pad_start(t(Msg::SgbYield), 7).dimmed(),
            pad_start(t(Msg::Volume), 8).dimmed()
        );
    } else {
        println!(
            "  {} {} {} {} {}",
            pad_end(t(Msg::Symbol), 12).dimmed(),
            pad_end(t(Msg::SgbSeries), 20).dimmed(),
            pad_end(t(Msg::SgbMatures), 9).dimmed(),
            pad_start(t(Msg::SgbIssuePrice), 10).dimmed(),
            pad_start(t(Msg::SgbInterest), 10).dimmed()
        );
    }
    for r in &rows {
        print_sgb(r, live_prices);
    }
    println!();
    if let Some(g) = gold.filter(|_| live_prices) {
        println!(
            "  {}",
            tf(Msg::SgbGoldBasis, &[&format_price(g, Some("INR"))]).dimmed()
        );
    }
    println!("  {}", t(Msg::SgbNote).dimmed());
    if !live_prices {
        println!("  {}", t(Msg::SgbSymbolsUnconfirmed).dimmed());
    }
    println!();
    Ok(())
}

fn print_sgb(r: &SgbRow, live_prices: bool) {
    let money = |v: Option<f64>| v.map_or_else(|| "N/A".to_string(), |p| format!("₹{p:.2}"));
    let matures = r
        .maturity
        .map_or("—".into(), |m| m.format("%b %Y").to_string());
    if !live_prices {
        println!(
            "  {} {} {} {} {}",
            pad_end(&r.symbol, 12).bold(),
            pad_end(r.series.as_deref().unwrap_or("—"), 20),
            pad_end(&matures, 9),
            pad_start(&money(r.issue_price), 10),
            pad_start(&money(r.interest_per_year), 10)
        );
        return;
    }
    let premium = match r.premium_percent {
        Some(p) if p >= 0.0 => format!("+{p:.2}%").yellow().to_string(),
        Some(p) => format!("{p:.2}%").green().to_string(),
        None => "N/A".dimmed().to_string(),
    };
    println!(
        "  {} {} {} {} {} {}",
        pad_end(&r.symbol, 12).bold(),
        pad_end(&matures, 9),
        pad_start(&money(r.price), 10).bold(),
        pad_start(&premium, 9),
        pad_start(
            &r.current_yield_percent
                .map_or("N/A".into(), |y| format!("{y:.2}%")),
            7
        ),
        pad_start(&r.volume.map(format_volume).unwrap_or_default(), 8).dimmed()
    );
}
