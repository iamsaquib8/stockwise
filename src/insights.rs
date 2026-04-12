use crate::api::Quote;
use crate::market;
use colored::Colorize;

/// Generate investment insights based on available data
pub fn generate_insights(q: &Quote) -> Vec<String> {
    let mut insights = Vec::new();

    // Valuation insights
    if let Some(pe) = q.trailing_pe {
        if pe < 0.0 {
            insights.push("Company is currently unprofitable (negative P/E)".red().to_string());
        } else if pe < 10.0 {
            insights.push(format!(
                "P/E of {:.1} is very low — could be undervalued or facing challenges",
                pe
            ));
        } else if pe < 20.0 {
            insights.push(format!(
                "P/E of {:.1} suggests reasonable valuation",
                pe
            ));
        } else if pe < 35.0 {
            insights.push(format!(
                "P/E of {:.1} indicates growth expectations priced in",
                pe
            ));
        } else {
            insights.push(
                format!("P/E of {:.1} is very high — stock is priced for aggressive growth", pe)
                    .yellow()
                    .to_string(),
            );
        }
    }

    // Forward vs trailing PE
    if let (Some(trailing), Some(forward)) = (q.trailing_pe, q.forward_pe) {
        if trailing > 0.0 && forward > 0.0 {
            let compression = ((trailing - forward) / trailing) * 100.0;
            if compression > 15.0 {
                insights.push(
                    format!(
                        "Forward P/E ({:.1}) is {:.0}% lower than trailing ({:.1}) — analysts expect strong earnings growth",
                        forward, compression, trailing
                    )
                    .green()
                    .to_string(),
                );
            } else if compression < -15.0 {
                insights.push(
                    format!(
                        "Forward P/E ({:.1}) is higher than trailing ({:.1}) — earnings expected to decline",
                        forward, trailing
                    )
                    .red()
                    .to_string(),
                );
            }
        }
    }

    // Price vs moving averages
    if let (Some(price), Some(ma50), Some(ma200)) =
        (q.regular_market_price, q.fifty_day_average, q.two_hundred_day_average)
    {
        if price > ma50 && ma50 > ma200 {
            insights.push(
                "Price above both 50-day and 200-day MA — strong uptrend (Golden Cross zone)"
                    .green()
                    .to_string(),
            );
        } else if price < ma50 && ma50 < ma200 {
            insights.push(
                "Price below both 50-day and 200-day MA — downtrend (Death Cross zone)"
                    .red()
                    .to_string(),
            );
        } else if price > ma200 && price < ma50 {
            insights.push("Price between MAs — possible short-term pullback in longer uptrend".to_string());
        } else if price < ma200 && price > ma50 {
            insights.push("Short-term recovery but still in longer-term downtrend".yellow().to_string());
        }

        let dist_from_200 = ((price - ma200) / ma200) * 100.0;
        if dist_from_200 > 30.0 {
            insights.push(
                format!(
                    "Price is {:.1}% above 200-day MA — extended, potential pullback risk",
                    dist_from_200
                )
                .yellow()
                .to_string(),
            );
        } else if dist_from_200 < -30.0 {
            insights.push(
                format!(
                    "Price is {:.1}% below 200-day MA — deeply oversold territory",
                    dist_from_200.abs()
                )
                .yellow()
                .to_string(),
            );
        }
    }

    // 52-week range position
    if let (Some(price), Some(high), Some(low)) =
        (q.regular_market_price, q.fifty_two_week_high, q.fifty_two_week_low)
    {
        let range = high - low;
        if range > 0.0 {
            let position = ((price - low) / range) * 100.0;
            if position > 90.0 {
                insights.push(
                    format!("Trading near 52-week high ({:.0}% of range) — momentum is strong but watch for resistance", position)
                        .to_string(),
                );
            } else if position < 10.0 {
                insights.push(
                    format!("Trading near 52-week low ({:.0}% of range) — could be a value opportunity or a falling knife", position)
                        .yellow()
                        .to_string(),
                );
            }
        }
    }

    // Dividend analysis
    if let Some(div_yield) = q.trailing_annual_dividend_yield {
        if div_yield > 0.0 {
            let pct = div_yield * 100.0;
            if pct > 5.0 {
                insights.push(
                    format!("High dividend yield of {:.2}% — check if it's sustainable", pct)
                        .yellow()
                        .to_string(),
                );
            } else if pct > 2.0 {
                insights.push(format!("Solid dividend yield of {:.2}%", pct).green().to_string());
            } else {
                insights.push(format!("Modest dividend yield of {:.2}%", pct).to_string());
            }
        }
    }

    // Earnings growth
    if let Some(growth) = q.earnings_quarterly_growth {
        let pct = growth * 100.0;
        if pct > 25.0 {
            insights.push(
                format!("Strong quarterly earnings growth of {:.1}%", pct)
                    .green()
                    .to_string(),
            );
        } else if pct < -10.0 {
            insights.push(
                format!("Quarterly earnings declined {:.1}%", pct)
                    .red()
                    .to_string(),
            );
        }
    }

    // Revenue growth
    if let Some(growth) = q.revenue_growth {
        let pct = growth * 100.0;
        if pct > 20.0 {
            insights.push(
                format!("Revenue growing at {:.1}% — strong top-line expansion", pct)
                    .green()
                    .to_string(),
            );
        } else if pct < 0.0 {
            insights.push(
                format!("Revenue declining at {:.1}% — shrinking top line", pct)
                    .red()
                    .to_string(),
            );
        }
    }

    // Profitability
    if let Some(margin) = q.profit_margins {
        let pct = margin * 100.0;
        if pct > 20.0 {
            insights.push(format!("Excellent profit margin of {:.1}%", pct).green().to_string());
        } else if pct < 0.0 {
            insights.push(format!("Negative profit margin ({:.1}%) — company is losing money", pct).red().to_string());
        }
    }

    // Debt analysis
    if let Some(de) = q.debt_to_equity {
        if de > 200.0 {
            insights.push(
                format!("Very high debt-to-equity ratio of {:.0} — significant leverage risk", de)
                    .red()
                    .to_string(),
            );
        } else if de < 30.0 {
            insights.push(
                format!("Low debt-to-equity of {:.0} — conservative balance sheet", de)
                    .green()
                    .to_string(),
            );
        }
    }

    // Beta / volatility
    if let Some(beta) = q.beta {
        if beta > 1.5 {
            insights.push(
                format!("High beta of {:.2} — stock is significantly more volatile than the market", beta)
                    .yellow()
                    .to_string(),
            );
        } else if beta < 0.5 {
            insights.push(
                format!("Low beta of {:.2} — defensive stock, less volatile than market", beta)
                    .to_string(),
            );
        }
    }

    // Analyst consensus
    if let (Some(rec), Some(target)) = (q.recommendation_mean, q.target_mean_price) {
        if let Some(price) = q.regular_market_price {
            let csym = market::currency_symbol(q.currency.as_deref());
            let upside = ((target - price) / price) * 100.0;
            let label = match q.recommendation_key.as_deref() {
                Some(k) => k.to_string(),
                None => format!("{:.1}", rec),
            };
            if upside > 20.0 {
                insights.push(
                    format!(
                        "Analyst consensus: {} — target {}{:.2} implies {:.1}% upside",
                        label, csym, target, upside
                    )
                    .green()
                    .to_string(),
                );
            } else if upside < -10.0 {
                insights.push(
                    format!(
                        "Analyst consensus: {} — target {}{:.2} implies {:.1}% downside",
                        label, csym, target, upside
                    )
                    .red()
                    .to_string(),
                );
            } else {
                insights.push(format!(
                    "Analyst consensus: {} — target {}{:.2} ({:+.1}%)",
                    label, csym, target, upside
                ));
            }
        }
    }

    if insights.is_empty() {
        insights.push("Limited data available for generating insights".dimmed().to_string());
    }

    insights
}

/// Generate an overall verdict
#[allow(dead_code)]
pub fn overall_verdict(q: &Quote) -> String {
    let mut score: i32 = 0;
    let mut factors = 0;

    // P/E valuation
    if let Some(pe) = q.trailing_pe {
        factors += 1;
        if pe > 0.0 && pe < 25.0 {
            score += 1;
        } else if pe > 40.0 || pe < 0.0 {
            score -= 1;
        }
    }

    // Trend
    if let (Some(price), Some(ma50), Some(ma200)) =
        (q.regular_market_price, q.fifty_day_average, q.two_hundred_day_average)
    {
        factors += 1;
        if price > ma50 && ma50 > ma200 {
            score += 2;
        } else if price < ma50 && ma50 < ma200 {
            score -= 2;
        }
    }

    // Earnings growth
    if let Some(g) = q.earnings_quarterly_growth {
        factors += 1;
        if g > 0.1 {
            score += 1;
        } else if g < -0.1 {
            score -= 1;
        }
    }

    // Profitability
    if let Some(m) = q.profit_margins {
        factors += 1;
        if m > 0.15 {
            score += 1;
        } else if m < 0.0 {
            score -= 1;
        }
    }

    // Analyst rating
    if let Some(rec) = q.recommendation_mean {
        factors += 1;
        if rec <= 2.0 {
            score += 1;
        } else if rec >= 4.0 {
            score -= 1;
        }
    }

    // Debt
    if let Some(de) = q.debt_to_equity {
        factors += 1;
        if de < 50.0 {
            score += 1;
        } else if de > 150.0 {
            score -= 1;
        }
    }

    if factors == 0 {
        return "Insufficient data to generate verdict".dimmed().to_string();
    }

    let normalized = score as f64 / factors as f64;
    if normalized >= 0.5 {
        format!(
            "  {} Overall outlook is BULLISH ({}/{})",
            "●".green(),
            score,
            factors
        )
        .green()
        .bold()
        .to_string()
    } else if normalized <= -0.5 {
        format!(
            "  {} Overall outlook is BEARISH ({}/{})",
            "●".red(),
            score,
            factors
        )
        .red()
        .bold()
        .to_string()
    } else {
        format!(
            "  {} Overall outlook is NEUTRAL ({}/{})",
            "●".yellow(),
            score,
            factors
        )
        .yellow()
        .bold()
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Quote;

    fn base_quote() -> Quote {
        Quote {
            symbol: Some("TEST.NS".into()),
            regular_market_price: Some(100.0),
            fifty_day_average: Some(90.0),
            two_hundred_day_average: Some(85.0),
            trailing_pe: Some(18.0),
            profit_margins: Some(0.20),
            earnings_quarterly_growth: Some(0.15),
            recommendation_mean: Some(1.8),
            debt_to_equity: Some(30.0),
            ..Default::default()
        }
    }

    #[test]
    fn test_generate_insights_non_empty() {
        let q = base_quote();
        let insights = generate_insights(&q);
        assert!(!insights.is_empty());
    }

    #[test]
    fn test_generate_insights_pe_low() {
        let mut q = Quote::default();
        q.trailing_pe = Some(8.0); // very low PE
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("P/E") || joined.contains("undervalued"));
    }

    #[test]
    fn test_generate_insights_pe_reasonable() {
        let mut q = Quote::default();
        q.trailing_pe = Some(15.0);
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("reasonable") || joined.contains("P/E"));
    }

    #[test]
    fn test_generate_insights_pe_high() {
        let mut q = Quote::default();
        q.trailing_pe = Some(50.0); // very high PE
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("P/E") || joined.contains("high") || joined.contains("aggressive"));
    }

    #[test]
    fn test_generate_insights_uptrend() {
        let mut q = Quote::default();
        q.regular_market_price = Some(120.0);
        q.fifty_day_average = Some(100.0);
        q.two_hundred_day_average = Some(90.0);
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.to_lowercase().contains("uptrend") || joined.contains("above"));
    }

    #[test]
    fn test_generate_insights_downtrend() {
        let mut q = Quote::default();
        q.regular_market_price = Some(80.0);
        q.fifty_day_average = Some(90.0);
        q.two_hundred_day_average = Some(95.0);
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.to_lowercase().contains("downtrend") || joined.contains("below"));
    }

    #[test]
    fn test_generate_insights_high_dividend() {
        let mut q = Quote::default();
        q.trailing_annual_dividend_yield = Some(0.07); // 7%
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("dividend") || joined.contains("yield"));
    }

    #[test]
    fn test_generate_insights_no_data() {
        let q = Quote::default();
        let insights = generate_insights(&q);
        // Should return at least the "limited data" fallback
        assert!(!insights.is_empty());
        assert!(insights[0].to_lowercase().contains("limited") || insights[0].contains("data"));
    }

    #[test]
    fn test_generate_insights_high_beta() {
        let mut q = Quote::default();
        q.beta = Some(2.0);
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("beta") || joined.contains("volatile"));
    }

    #[test]
    fn test_generate_insights_strong_earnings_growth() {
        let mut q = Quote::default();
        q.earnings_quarterly_growth = Some(0.30); // 30%
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("earnings") || joined.contains("growth"));
    }

    #[test]
    fn test_overall_verdict_bullish() {
        let q = base_quote(); // all positive signals
        let verdict = overall_verdict(&q);
        assert!(verdict.to_lowercase().contains("bullish") || verdict.contains("BULLISH"));
    }

    #[test]
    fn test_overall_verdict_bearish() {
        let mut q = Quote::default();
        q.trailing_pe = Some(-5.0); // negative PE (unprofitable)
        q.regular_market_price = Some(80.0);
        q.fifty_day_average = Some(90.0);
        q.two_hundred_day_average = Some(95.0); // price below both MAs
        q.earnings_quarterly_growth = Some(-0.20); // declining earnings
        q.profit_margins = Some(-0.05); // negative margin
        let verdict = overall_verdict(&q);
        assert!(verdict.to_lowercase().contains("bearish") || verdict.contains("BEARISH"));
    }

    #[test]
    fn test_overall_verdict_insufficient_data() {
        let q = Quote::default();
        let verdict = overall_verdict(&q);
        assert!(verdict.to_lowercase().contains("insufficient") || verdict.contains("data"));
    }

    #[test]
    fn test_overall_verdict_contains_score() {
        let q = base_quote();
        let verdict = overall_verdict(&q);
        // Verdict includes score like "3/5"
        assert!(verdict.contains('/'));
    }
}
