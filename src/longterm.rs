use crate::api::Quote;

/// Score a stock for long-term wealth building (0-100)
#[derive(Debug)]
pub struct LongTermScore {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: Option<String>,
    pub total_score: f64,
    pub valuation_score: f64,
    pub growth_score: f64,
    pub quality_score: f64,
    pub momentum_score: f64,
    pub dividend_score: f64,
    pub reasons: Vec<String>,
    pub risk_flags: Vec<String>,
    // Projection
    pub projected_5y_return: f64, // estimated 5-year return %
    pub sip_monthly: f64,         // suggested monthly SIP amount for ₹10L goal in 10y
}

pub fn score_for_longterm(q: &Quote) -> LongTermScore {
    let mut valuation: f64 = 50.0;
    let mut growth: f64 = 50.0;
    let mut quality: f64 = 50.0;
    let mut momentum: f64 = 50.0;
    let mut dividend: f64 = 50.0;
    let mut reasons = Vec::new();
    let mut risk_flags = Vec::new();

    // ── Valuation ──
    if let Some(pe) = q.trailing_pe {
        if pe > 0.0 && pe < 15.0 {
            valuation += 25.0;
            reasons.push("Attractively valued (P/E < 15)".into());
        } else if pe < 25.0 {
            valuation += 10.0;
        } else if pe > 50.0 {
            valuation -= 15.0;
            risk_flags.push("Very expensive valuation".into());
        }
    }
    if let Some(pb) = q.price_to_book {
        if pb > 0.0 && pb < 3.0 {
            valuation += 10.0;
        } else if pb > 10.0 {
            valuation -= 5.0;
        }
    }
    if let (Some(t), Some(f)) = (q.trailing_pe, q.forward_pe) {
        if f < t && t > 0.0 && f > 0.0 {
            valuation += 10.0;
            reasons.push("Forward P/E compression — earnings growth expected".into());
        }
    }

    // ── Growth ──
    if let Some(g) = q.revenue_growth {
        if g > 0.20 {
            growth += 25.0;
            reasons.push(format!("Strong revenue growth ({:.0}%)", g * 100.0));
        } else if g > 0.10 {
            growth += 15.0;
        } else if g < 0.0 {
            growth -= 15.0;
            risk_flags.push("Revenue declining".into());
        }
    }
    if let Some(g) = q.earnings_quarterly_growth {
        if g > 0.20 {
            growth += 20.0;
            reasons.push(format!("Earnings growing {:.0}% QoQ", g * 100.0));
        } else if g < -0.10 {
            growth -= 10.0;
        }
    }
    if let (Some(ttm), Some(fwd)) = (q.eps_trailing_twelve_months, q.eps_forward) {
        if fwd > ttm && ttm > 0.0 {
            growth += 10.0;
        }
    }

    // ── Quality ──
    if let Some(m) = q.profit_margins {
        if m > 0.20 {
            quality += 20.0;
            reasons.push(format!("High profit margins ({:.0}%)", m * 100.0));
        } else if m > 0.10 {
            quality += 10.0;
        } else if m < 0.0 {
            quality -= 20.0;
            risk_flags.push("Unprofitable company".into());
        }
    }
    if let Some(roe) = q.return_on_equity {
        if roe > 0.20 {
            quality += 15.0;
            reasons.push(format!("Excellent ROE ({:.0}%)", roe * 100.0));
        } else if roe > 0.10 {
            quality += 5.0;
        }
    }
    if let Some(de) = q.debt_to_equity {
        if de < 50.0 {
            quality += 10.0;
            reasons.push("Low debt".into());
        } else if de > 200.0 {
            quality -= 15.0;
            risk_flags.push("High debt burden".into());
        }
    }
    if let Some(cr) = q.current_ratio {
        if cr > 1.5 {
            quality += 5.0;
        } else if cr < 1.0 {
            risk_flags.push("Liquidity concern (current ratio < 1)".into());
        }
    }

    // ── Momentum ──
    if let (Some(price), Some(ma50), Some(ma200)) =
        (q.regular_market_price, q.fifty_day_average, q.two_hundred_day_average)
    {
        if price > ma50 && ma50 > ma200 {
            momentum += 20.0;
            reasons.push("Strong uptrend (Golden Cross)".into());
        } else if price < ma50 && ma50 < ma200 {
            momentum -= 15.0;
            risk_flags.push("Downtrend (Death Cross)".into());
        }
    }
    if let Some(rec) = q.recommendation_mean {
        if rec <= 2.0 {
            momentum += 10.0;
            reasons.push("Strong analyst buy consensus".into());
        } else if rec >= 4.0 {
            momentum -= 10.0;
        }
    }

    // ── Dividend ──
    if let Some(dy) = q.trailing_annual_dividend_yield {
        if dy > 0.03 {
            dividend += 25.0;
            reasons.push(format!("Good dividend yield ({:.1}%)", dy * 100.0));
        } else if dy > 0.01 {
            dividend += 10.0;
        }
    }

    valuation = valuation.clamp(0.0, 100.0);
    growth = growth.clamp(0.0, 100.0);
    quality = quality.clamp(0.0, 100.0);
    momentum = momentum.clamp(0.0, 100.0);
    dividend = dividend.clamp(0.0, 100.0);

    // Weighted total: growth and quality matter most for long-term
    let total = (valuation * 0.20 + growth * 0.25 + quality * 0.25 + momentum * 0.15 + dividend * 0.15).clamp(0.0, 100.0);

    // Rough 5-year projection based on score
    let annual_return_est = match total {
        s if s >= 80.0 => 18.0,
        s if s >= 70.0 => 14.0,
        s if s >= 60.0 => 10.0,
        s if s >= 50.0 => 7.0,
        s if s >= 40.0 => 4.0,
        _ => 0.0,
    };
    let projected_5y = ((1.0 + annual_return_est / 100.0_f64).powi(5) - 1.0) * 100.0;

    // SIP to reach ₹10L in 10 years at estimated rate
    let monthly_rate = annual_return_est / 100.0 / 12.0;
    let months = 120.0;
    let sip = if monthly_rate > 0.0 {
        1_000_000.0 * monthly_rate / ((1.0 + monthly_rate).powf(months) - 1.0)
    } else {
        1_000_000.0 / months
    };

    LongTermScore {
        symbol: q.symbol.clone().unwrap_or_default(),
        name: q.short_name.clone().or(q.long_name.clone()).unwrap_or_default(),
        price: q.regular_market_price.unwrap_or(0.0),
        currency: q.currency.clone(),
        total_score: total,
        valuation_score: valuation,
        growth_score: growth,
        quality_score: quality,
        momentum_score: momentum,
        dividend_score: dividend,
        reasons,
        risk_flags,
        projected_5y_return: projected_5y,
        sip_monthly: sip,
    }
}
