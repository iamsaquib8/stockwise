use crate::i18n::{Msg, t};
use crate::market::{self, AssetClass};
use colored::Colorize;

pub fn format_price(val: f64, currency: Option<&str>) -> String {
    with_currency(val, currency, 2)
}

/// `val` with `dp` decimals and its currency: `₹1350.75`, `HK$142.50`, or
/// `118.20p` for London prices quoted in pence (`GBp`).
fn with_currency(val: f64, currency: Option<&str>, dp: usize) -> String {
    if currency.and_then(market::minor_unit).is_some() {
        return format!("{:.*}p", dp, val);
    }
    format!("{}{:.*}", market::currency_symbol(currency), dp, val)
}

/// Decimal places for a price: stocks and indices use 2, FX pairs 4, and
/// crypto enough to show low-priced coins (2 above 1, then 4, then 8).
pub fn price_decimals(class: AssetClass, price: f64) -> usize {
    match class {
        AssetClass::Fx => 4,
        AssetClass::Crypto => match price.abs() {
            p if p >= 1.0 => 2,
            p if p >= 0.01 => 4,
            _ => 8,
        },
        AssetClass::Stock | AssetClass::Index => 2,
    }
}

/// Like `format_price`, with the precision `symbol`'s asset class needs
/// (`USDINR=X` → ₹84.1234, `DOGE-USD` → $0.1234).
pub fn format_asset_price(val: f64, currency: Option<&str>, symbol: &str) -> String {
    let dp = price_decimals(AssetClass::of(symbol), val);
    with_currency(val, currency, dp)
}

pub fn format_change(val: f64, pct: f64) -> String {
    format_change_dp(val, pct, 2)
}

/// `format_change` with `dp` decimals for the absolute change (FX and
/// low-priced coins move in fractions of a unit).
pub fn format_change_dp(val: f64, pct: f64, dp: usize) -> String {
    let arrow = if val >= 0.0 { "▲" } else { "▼" };
    let text = format!("{} {:.*} ({:.2}%)", arrow, dp, val, pct);
    if val >= 0.0 {
        text.green().bold().to_string()
    } else {
        text.red().bold().to_string()
    }
}

/// Columns a string takes in a terminal. Devanagari vowel signs, virama and
/// nukta combine with the previous letter and take no column of their own,
/// so `"मूल्य".chars().count()` (5) overstates its width (3).
pub fn display_width(text: &str) -> usize {
    text.chars().filter(|c| !is_zero_width(*c)).count()
}

fn is_zero_width(c: char) -> bool {
    matches!(
        c,
        '\u{0900}'..='\u{0902}'
            | '\u{093A}'
            | '\u{093C}'
            | '\u{0941}'..='\u{0948}'
            | '\u{094D}'
            | '\u{0951}'..='\u{0957}'
            | '\u{0962}'..='\u{0963}'
            | '\u{200B}'..='\u{200D}'
            | '\u{FE00}'..='\u{FE0F}'
    )
}

/// Left-align `text` in `width` terminal columns (format's `{:<w}` counts
/// chars, which misaligns Hindi labels).
pub fn pad_end(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(display_width(text));
    format!("{}{}", text, " ".repeat(fill))
}

/// Right-align `text` in `width` terminal columns.
pub fn pad_start(text: &str, width: usize) -> String {
    let fill = width.saturating_sub(display_width(text));
    format!("{}{}", " ".repeat(fill), text)
}

/// Market caps and other large amounts. Yahoo reports those of London
/// listings in pounds even though their prices are in pence, so `GBp` is
/// shown with `£` here.
pub fn format_large_number(val: f64, currency: Option<&str>) -> String {
    let currency = currency.map(|c| market::minor_unit(c).map_or(c, |(major, _)| major));
    let sym = market::currency_symbol(currency);
    if market::is_inr(currency) {
        format_large_number_inr(val, sym)
    } else {
        format_large_number_usd(val, sym)
    }
}

fn format_large_number_usd(val: f64, sym: &str) -> String {
    if val >= 1_000_000_000_000.0 {
        format!("{}{:.2}T", sym, val / 1_000_000_000_000.0)
    } else if val >= 1_000_000_000.0 {
        format!("{}{:.2}B", sym, val / 1_000_000_000.0)
    } else if val >= 1_000_000.0 {
        format!("{}{:.2}M", sym, val / 1_000_000.0)
    } else if val >= 1_000.0 {
        format!("{}{:.1}K", sym, val / 1_000.0)
    } else {
        format!("{}{:.2}", sym, val)
    }
}

fn format_large_number_inr(val: f64, sym: &str) -> String {
    // Lakhs and crores in every language; Hindi spells the units out.
    if val >= 1_000_000_000_000.0 {
        format!(
            "{}{:.2}{}",
            sym,
            val / 1_000_000_000_000.0,
            t(Msg::UnitLakhCrore)
        )
    } else if val >= 10_000_000_000.0 {
        format!(
            "{}{:.2}{}",
            sym,
            val / 10_000_000_000.0,
            t(Msg::UnitThousandCrore)
        )
    } else if val >= 10_000_000.0 {
        format!("{}{:.2}{}", sym, val / 10_000_000.0, t(Msg::UnitCrore))
    } else if val >= 100_000.0 {
        format!("{}{:.2}{}", sym, val / 100_000.0, t(Msg::UnitLakh))
    } else if val >= 1_000.0 {
        format!("{}{:.1}K", sym, val / 1_000.0)
    } else {
        format!("{}{:.2}", sym, val)
    }
}

pub fn format_volume(val: u64) -> String {
    if val >= 1_000_000_000 {
        format!("{:.2}B", val as f64 / 1_000_000_000.0)
    } else if val >= 1_000_000 {
        format!("{:.2}M", val as f64 / 1_000_000.0)
    } else if val >= 1_000 {
        format!("{:.1}K", val as f64 / 1_000.0)
    } else {
        format!("{}", val)
    }
}

pub fn format_optional_f64(val: Option<f64>, suffix: &str) -> String {
    match val {
        Some(v) => format!("{:.2}{}", v, suffix),
        None => "N/A".dimmed().to_string(),
    }
}

pub fn format_optional_price(val: Option<f64>, currency: Option<&str>) -> String {
    match val {
        Some(v) => format_price(v, currency),
        None => "N/A".dimmed().to_string(),
    }
}

pub fn format_optional_pct(val: Option<f64>) -> String {
    match val {
        Some(v) => {
            let text = format!("{:.2}%", v * 100.0);
            if v >= 0.0 {
                text.green().to_string()
            } else {
                text.red().to_string()
            }
        }
        None => "N/A".dimmed().to_string(),
    }
}

pub fn sparkline(data: &[f64]) -> String {
    let chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    if data.is_empty() {
        return String::new();
    }
    let min = data.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = data.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let range = max - min;
    if range == 0.0 {
        return chars[4].to_string().repeat(data.len());
    }

    data.iter()
        .map(|&v| {
            let idx = (((v - min) / range) * 7.0).round() as usize;
            chars[idx.min(7)]
        })
        .collect()
}

pub fn print_header(text: &str) {
    let line = "─".repeat(60);
    println!("\n{}", line.dimmed());
    println!("  {}", text.bold().cyan());
    println!("{}", line.dimmed());
}

pub fn print_section(text: &str) {
    println!("\n  {}", text.bold().yellow());
    println!("  {}", "─".repeat(40).dimmed());
}

pub fn print_kv(key: &str, value: &str) {
    println!("  {} {}", pad_end(key, 28).dimmed(), value);
}

/// Print a non-intrusive next-step hint in interactive table mode unless
/// `STOCKWISE_NO_HINTS` is set.
pub fn print_hint(text: &str) {
    if std::env::var_os("STOCKWISE_NO_HINTS").is_none() {
        println!("  {} {}", "💡".dimmed(), text.dimmed());
    }
}

pub fn rating_bar(score: f64, max: f64) -> String {
    let normalized = ((score / max) * 10.0).round() as usize;
    let filled = "█".repeat(normalized.min(10));
    let empty = "░".repeat(10 - normalized.min(10));
    format!("{}{}", filled.green(), empty.dimmed())
}

pub fn sentiment_label(recommendation_mean: f64) -> String {
    match recommendation_mean {
        x if x <= 1.5 => "Strong Buy".green().bold().to_string(),
        x if x <= 2.5 => "Buy".green().to_string(),
        x if x <= 3.5 => "Hold".yellow().to_string(),
        x if x <= 4.5 => "Sell".red().to_string(),
        _ => "Strong Sell".red().bold().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_price_usd() {
        assert_eq!(format_price(100.50, Some("USD")), "$100.50");
        assert_eq!(format_price(0.0, None), "$0.00");
    }

    #[test]
    fn test_format_price_inr() {
        let result = format_price(1350.75, Some("INR"));
        assert!(result.contains("1350.75"));
        assert!(result.contains("\u{20B9}"));
    }

    #[test]
    fn test_format_large_number_usd() {
        let result = format_large_number(1_500_000_000_000.0, Some("USD"));
        assert!(result.contains("1.50T"));
        let result = format_large_number(2_500_000_000.0, Some("USD"));
        assert!(result.contains("2.50B"));
        let result = format_large_number(3_500_000.0, Some("USD"));
        assert!(result.contains("3.50M"));
    }

    #[test]
    fn test_format_large_number_inr() {
        let result = format_large_number(18_280_000_000_000.0, Some("INR"));
        assert!(result.contains("L Cr"));
        let result = format_large_number(50_000_000_000.0, Some("INR"));
        assert!(result.contains("K Cr"));
        let result = format_large_number(150_000_000.0, Some("INR"));
        assert!(result.contains("Cr"));
    }

    #[test]
    fn test_format_volume() {
        assert_eq!(format_volume(1_500_000_000), "1.50B");
        assert_eq!(format_volume(25_000_000), "25.00M");
        assert_eq!(format_volume(1_500), "1.5K");
        assert_eq!(format_volume(500), "500");
    }

    #[test]
    fn test_format_optional_f64() {
        assert_eq!(format_optional_f64(Some(25.5), "x"), "25.50x");
        assert_eq!(format_optional_f64(Some(1.25), ""), "1.25");
    }

    #[test]
    fn test_format_optional_price() {
        let result = format_optional_price(Some(100.0), Some("USD"));
        assert!(result.contains("$100.00"));
    }

    #[test]
    fn test_sparkline_empty() {
        assert_eq!(sparkline(&[]), "");
    }

    #[test]
    fn test_sparkline_constant() {
        let result = sparkline(&[50.0, 50.0, 50.0]);
        assert_eq!(result.chars().count(), 3);
    }

    #[test]
    fn test_sparkline_increasing() {
        let result = sparkline(&[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(result.chars().count(), 5);
    }

    #[test]
    fn test_sparkline_single_value() {
        let result = sparkline(&[42.0]);
        assert_eq!(result.chars().count(), 1);
    }

    #[test]
    fn test_format_large_number_usd_k() {
        let result = format_large_number(5_000.0, Some("USD"));
        assert!(result.contains("K"));
    }

    #[test]
    fn test_format_large_number_usd_small() {
        let result = format_large_number(500.0, Some("USD"));
        assert!(result.contains("$"));
        assert!(result.contains("500.00"));
    }

    #[test]
    fn test_format_large_number_inr_lakh() {
        let result = format_large_number(500_000.0, Some("INR"));
        assert!(result.contains("L")); // lakhs
    }

    #[test]
    fn test_format_large_number_inr_k() {
        let result = format_large_number(5_000.0, Some("INR"));
        assert!(result.contains("K"));
    }

    #[test]
    fn test_format_optional_f64_none() {
        let result = format_optional_f64(None, "x");
        assert!(result.contains("N/A"));
    }

    #[test]
    fn test_rating_bar_zero() {
        let result = rating_bar(0.0, 100.0);
        // Should be all empty blocks
        assert!(result.contains("░"));
    }

    #[test]
    fn test_rating_bar_full() {
        let result = rating_bar(100.0, 100.0);
        // Should be all filled blocks
        assert!(result.contains("█"));
    }

    #[test]
    fn test_sentiment_label_strong_buy() {
        let label = sentiment_label(1.0);
        assert!(label.contains("Strong Buy") || label.to_lowercase().contains("strong buy"));
    }

    #[test]
    fn test_sentiment_label_buy() {
        let label = sentiment_label(2.0);
        assert!(label.contains("Buy") || label.to_lowercase().contains("buy"));
    }

    #[test]
    fn test_sentiment_label_hold() {
        let label = sentiment_label(3.0);
        assert!(label.contains("Hold") || label.to_lowercase().contains("hold"));
    }

    #[test]
    fn test_sentiment_label_sell() {
        let label = sentiment_label(4.0);
        assert!(label.contains("Sell") || label.to_lowercase().contains("sell"));
    }

    #[test]
    fn test_sentiment_label_strong_sell() {
        let label = sentiment_label(5.0);
        assert!(label.contains("Strong Sell") || label.to_lowercase().contains("strong sell"));
    }

    #[test]
    fn test_format_change_positive() {
        let result = format_change(10.0, 1.5);
        assert!(result.contains("▲"));
        assert!(result.contains("10.00"));
        assert!(result.contains("1.50%"));
    }

    #[test]
    fn test_format_change_negative() {
        let result = format_change(-5.0, -0.5);
        assert!(result.contains("▼"));
        assert!(result.contains("5.00"));
    }

    #[test]
    fn test_format_optional_pct_some_positive() {
        let result = format_optional_pct(Some(0.15));
        assert!(result.contains("15.00%"));
    }

    #[test]
    fn test_format_optional_pct_none() {
        let result = format_optional_pct(None);
        assert!(result.contains("N/A"));
    }

    #[test]
    fn price_decimals_by_asset_class() {
        assert_eq!(price_decimals(AssetClass::Stock, 0.5), 2);
        assert_eq!(price_decimals(AssetClass::Index, 25_000.0), 2);
        assert_eq!(price_decimals(AssetClass::Fx, 84.1), 4);
        assert_eq!(price_decimals(AssetClass::Crypto, 104_000.0), 2);
        assert_eq!(price_decimals(AssetClass::Crypto, 0.1734), 4);
        assert_eq!(price_decimals(AssetClass::Crypto, 0.000_012_34), 8);
    }

    #[test]
    fn format_asset_price_uses_class_precision() {
        assert_eq!(
            format_asset_price(84.123_456, Some("INR"), "USDINR=X"),
            "\u{20B9}84.1235"
        );
        assert_eq!(
            format_asset_price(1.085_21, Some("USD"), "EURUSD=X"),
            "$1.0852"
        );
        assert_eq!(
            format_asset_price(0.173_456, Some("USD"), "DOGE-USD"),
            "$0.1735"
        );
        assert_eq!(
            format_asset_price(0.000_012_345, Some("USD"), "SHIB-USD"),
            "$0.00001234"
        );
        assert_eq!(
            format_asset_price(104_250.5, Some("USD"), "BTC-USD"),
            "$104250.50"
        );
        assert_eq!(
            format_asset_price(3500.0, Some("INR"), "TCS.NS"),
            "\u{20B9}3500.00"
        );
        assert_eq!(
            format_asset_price(150.25, Some("JPY"), "USDJPY=X"),
            "\u{00A5}150.2500"
        );
    }

    #[test]
    fn london_pence_and_other_currencies() {
        // London prices are in pence (GBp): 118.20p, not £118.20.
        assert_eq!(format_price(118.2, Some("GBp")), "118.20p");
        assert_eq!(format_asset_price(118.2, Some("GBp"), "VOD.L"), "118.20p");
        assert_eq!(format_price(1.182, Some("GBP")), "\u{00A3}1.18");
        // Market caps of London listings come in pounds.
        assert_eq!(
            format_large_number(27_383_695_360.0, Some("GBp")),
            "\u{00A3}27.38B"
        );
        assert_eq!(format_price(142.5, Some("HKD")), "HK$142.50");
        assert_eq!(format_price(74.44, Some("SGD")), "S$74.44");
        assert_eq!(format_price(281.77, Some("CAD")), "C$281.77");
        assert_eq!(format_price(55.05, Some("AUD")), "A$55.05");
        assert_eq!(format_price(3105.0, Some("JPY")), "\u{00A5}3105.00");
        assert_eq!(format_price(214.4, Some("EUR")), "\u{20AC}214.40");
    }

    #[test]
    fn format_change_dp_shows_small_moves() {
        let fx = format_change_dp(0.0123, 0.015, 4);
        assert!(fx.contains("0.0123"), "{fx}");
        assert!(format_change(0.0123, 0.015).contains("0.01"));
    }

    #[test]
    fn display_width_ignores_combining_marks() {
        assert_eq!(display_width("Price"), 5);
        assert_eq!(display_width("मूल्य"), 3);
        assert_eq!(display_width("कीमत"), 4);
        assert_eq!(pad_end("मूल्य", 6), "मूल्य   ");
        assert_eq!(pad_start("P&L", 5), "  P&L");
        // Never truncates.
        assert_eq!(pad_end("too long", 3), "too long");
    }
}
