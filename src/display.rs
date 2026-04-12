use crate::market;
use colored::Colorize;

pub fn format_price(val: f64, currency: Option<&str>) -> String {
    let sym = market::currency_symbol(currency);
    format!("{}{:.2}", sym, val)
}

pub fn format_change(val: f64, pct: f64) -> String {
    let arrow = if val >= 0.0 { "▲" } else { "▼" };
    let text = format!("{} {:.2} ({:.2}%)", arrow, val, pct);
    if val >= 0.0 {
        text.green().bold().to_string()
    } else {
        text.red().bold().to_string()
    }
}

pub fn format_large_number(val: f64, currency: Option<&str>) -> String {
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
    if val >= 1_000_000_000_000.0 {
        format!("{}{:.2}L Cr", sym, val / 1_000_000_000_000.0)
    } else if val >= 10_000_000_000.0 {
        format!("{}{:.2}K Cr", sym, val / 10_000_000_000.0)
    } else if val >= 10_000_000.0 {
        format!("{}{:.2} Cr", sym, val / 10_000_000.0)
    } else if val >= 100_000.0 {
        format!("{}{:.2}L", sym, val / 100_000.0)
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
    println!("  {:<28} {}", key.dimmed(), value);
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
        assert_eq!(format_optional_f64(Some(3.14), ""), "3.14");
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
}
