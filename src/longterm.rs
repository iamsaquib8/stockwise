use crate::api::Quote;
use crate::technical;

// ══════════════════════════════════════════════════════════
// LONG-TERM WEALTH ENGINE v2
// Multi-factor scoring + Monte Carlo + DRIP + Moat analysis
// ══════════════════════════════════════════════════════════

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MoatRating {
    Wide,
    Narrow,
    None,
}

impl std::fmt::Display for MoatRating {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MoatRating::Wide => write!(f, "Wide Moat"),
            MoatRating::Narrow => write!(f, "Narrow Moat"),
            MoatRating::None => write!(f, "No Moat"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RiskTier {
    Conservative,
    Moderate,
    Aggressive,
}

impl std::fmt::Display for RiskTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskTier::Conservative => write!(f, "Conservative"),
            RiskTier::Moderate => write!(f, "Moderate"),
            RiskTier::Aggressive => write!(f, "Aggressive"),
        }
    }
}

#[derive(Debug)]
pub struct LongTermScore {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: Option<String>,
    // Pillar scores (0-100 each)
    pub valuation_score: f64,
    pub growth_score: f64,
    pub quality_score: f64,
    pub momentum_score: f64,
    pub dividend_score: f64,
    pub safety_score: f64,
    // Composite
    pub total_score: f64,
    pub moat: MoatRating,
    pub risk_tier: RiskTier,
    pub reasons: Vec<String>,
    pub risk_flags: Vec<String>,
    // Advanced metrics
    pub peg_ratio: Option<f64>,
    pub ev_ebitda: Option<f64>,
    pub earnings_yield: Option<f64>, // inverse of P/E — Buffett metric
    // Projections
    pub est_annual_return: f64,
    pub projected_5y_return: f64,
    pub projected_10y_return: f64,
    pub sip_monthly_10l_10y: f64, // SIP to ₹10L in 10 years
    pub drip_multiplier_10y: f64, // dividend reinvestment multiplier over 10 years
    // Monte Carlo
    pub monte_carlo_median: f64,    // median 5Y return from simulation
    pub monte_carlo_p10: f64,       // 10th percentile (bad case)
    pub monte_carlo_p90: f64,       // 90th percentile (good case)
}

pub fn score_for_longterm(q: &Quote, hist_closes: Option<&[f64]>) -> LongTermScore {
    let mut val: f64 = 50.0;
    let mut growth: f64 = 50.0;
    let mut quality: f64 = 50.0;
    let mut momentum: f64 = 50.0;
    let mut dividend: f64 = 50.0;
    let mut safety: f64 = 50.0;
    let mut reasons: Vec<String> = Vec::new();
    let mut risk_flags: Vec<String> = Vec::new();
    let mut moat_points: i32 = 0;

    let price = q.regular_market_price.unwrap_or(0.0);

    // ══════════════════════════════════════════
    // PILLAR 1: VALUATION (deep)
    // ══════════════════════════════════════════

    // P/E analysis
    if let Some(pe) = q.trailing_pe {
        if pe > 0.0 && pe < 12.0 {
            val += 25.0;
            reasons.push(format!("Deep value (P/E {:.1})", pe));
        } else if pe < 18.0 {
            val += 15.0;
            reasons.push(format!("Fair value (P/E {:.1})", pe));
        } else if pe < 30.0 {
            val += 5.0;
        } else if pe > 50.0 {
            val -= 15.0;
            risk_flags.push(format!("Expensive (P/E {:.0})", pe));
        }
    }

    // PEG ratio (P/E relative to growth)
    let peg = match (q.trailing_pe, q.earnings_quarterly_growth) {
        (Some(pe), Some(g)) if pe > 0.0 && g > 0.01 => {
            let peg = pe / (g * 100.0);
            if peg < 1.0 {
                val += 15.0;
                reasons.push(format!("PEG < 1 ({:.2}) — growth at reasonable price", peg));
            } else if peg < 2.0 {
                val += 5.0;
            } else if peg > 3.0 {
                val -= 5.0;
            }
            Some(peg)
        }
        _ => None,
    };

    // P/B analysis
    if let Some(pb) = q.price_to_book {
        if pb > 0.0 && pb < 2.0 {
            val += 10.0;
        } else if pb > 10.0 {
            val -= 8.0;
            risk_flags.push(format!("High P/B ({:.1})", pb));
        }
    }

    // Forward P/E compression = earnings growth expected
    if let (Some(t), Some(f)) = (q.trailing_pe, q.forward_pe) {
        if f < t * 0.8 && t > 0.0 && f > 0.0 {
            val += 10.0;
            growth += 10.0;
            reasons.push(format!("P/E compression {:.0}→{:.0} (strong growth ahead)", t, f));
        } else if f > t * 1.2 && t > 0.0 {
            risk_flags.push("Forward P/E expanding — earnings expected to slow".into());
        }
    }

    // EV/EBITDA
    let ev_ebitda = q.enterprise_to_ebitda;
    if let Some(ev) = ev_ebitda {
        if ev > 0.0 && ev < 10.0 {
            val += 8.0;
            reasons.push(format!("EV/EBITDA attractive ({:.1}x)", ev));
        } else if ev > 25.0 {
            val -= 5.0;
        }
    }

    // Earnings yield (Buffett: compare to bond yields)
    let earnings_yield = q.trailing_pe.filter(|&pe| pe > 0.0).map(|pe| 100.0 / pe);
    if let Some(ey) = earnings_yield {
        if ey > 8.0 {
            val += 5.0; // earning more than bonds
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 2: GROWTH
    // ══════════════════════════════════════════

    if let Some(g) = q.revenue_growth {
        let pct = g * 100.0;
        if pct > 25.0 {
            growth += 25.0;
            reasons.push(format!("Exceptional revenue growth ({:.0}%)", pct));
            moat_points += 1; // fast growers often have moats
        } else if pct > 15.0 {
            growth += 18.0;
            reasons.push(format!("Strong revenue growth ({:.0}%)", pct));
        } else if pct > 8.0 {
            growth += 10.0;
        } else if pct > 0.0 {
            growth += 3.0;
        } else {
            growth -= 15.0;
            risk_flags.push(format!("Revenue declining ({:.0}%)", pct));
        }
    }

    if let Some(g) = q.earnings_quarterly_growth {
        let pct = g * 100.0;
        if pct > 30.0 {
            growth += 20.0;
            reasons.push(format!("Earnings surging {:.0}% QoQ", pct));
        } else if pct > 15.0 {
            growth += 12.0;
        } else if pct < -15.0 {
            growth -= 12.0;
            risk_flags.push(format!("Earnings declining {:.0}% QoQ", pct));
        }
    }

    // EPS trajectory
    if let (Some(ttm), Some(fwd)) = (q.eps_trailing_twelve_months, q.eps_forward) {
        if fwd > ttm * 1.15 && ttm > 0.0 {
            growth += 10.0;
            reasons.push(format!("EPS growth {:.0}→{:.0} (forward)", ttm, fwd));
        } else if fwd < ttm * 0.85 && ttm > 0.0 {
            growth -= 8.0;
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 3: QUALITY (moat indicators)
    // ══════════════════════════════════════════

    if let Some(m) = q.profit_margins {
        let pct = m * 100.0;
        if pct > 25.0 {
            quality += 22.0;
            moat_points += 2; // high margins = pricing power = moat
            reasons.push(format!("Excellent margins ({:.0}%) — pricing power", pct));
        } else if pct > 15.0 {
            quality += 12.0;
            moat_points += 1;
        } else if pct > 5.0 {
            quality += 3.0;
        } else if pct < 0.0 {
            quality -= 20.0;
            risk_flags.push("Unprofitable".into());
        }
    }

    if let Some(roe) = q.return_on_equity {
        let pct = roe * 100.0;
        if pct > 25.0 {
            quality += 18.0;
            moat_points += 2; // consistently high ROE = moat
            reasons.push(format!("Outstanding ROE ({:.0}%)", pct));
        } else if pct > 15.0 {
            quality += 10.0;
            moat_points += 1;
        } else if pct > 8.0 {
            quality += 3.0;
        } else if pct < 0.0 {
            quality -= 10.0;
        }
    }

    // Debt analysis (deep)
    if let Some(de) = q.debt_to_equity {
        if de < 30.0 {
            quality += 10.0;
            safety += 15.0;
            reasons.push("Very low debt — fortress balance sheet".into());
        } else if de < 80.0 {
            quality += 5.0;
            safety += 5.0;
        } else if de > 150.0 {
            quality -= 10.0;
            safety -= 15.0;
            risk_flags.push(format!("High leverage (D/E {:.0})", de));
        } else if de > 250.0 {
            quality -= 20.0;
            safety -= 25.0;
            risk_flags.push(format!("Dangerous debt levels (D/E {:.0})", de));
        }
    }

    if let Some(cr) = q.current_ratio {
        if cr > 2.0 {
            safety += 10.0;
        } else if cr > 1.5 {
            safety += 5.0;
        } else if cr < 1.0 {
            safety -= 10.0;
            risk_flags.push(format!("Liquidity risk (CR {:.1})", cr));
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 4: MOMENTUM
    // ══════════════════════════════════════════

    if let (Some(p), Some(ma50), Some(ma200)) =
        (q.regular_market_price, q.fifty_day_average, q.two_hundred_day_average)
    {
        if p > ma50 && ma50 > ma200 {
            momentum += 20.0;
            reasons.push("Strong uptrend (Golden Cross)".into());
        } else if p > ma200 && p < ma50 {
            momentum += 5.0; // pullback in uptrend
        } else if p < ma50 && ma50 < ma200 {
            momentum -= 15.0;
            risk_flags.push("Downtrend (Death Cross)".into());
        }

        // Distance from 200-day MA (mean reversion for long-term)
        let dist = ((p - ma200) / ma200) * 100.0;
        if dist < -20.0 {
            momentum += 10.0; // deeply below MA = potential entry
            reasons.push(format!("{:.0}% below 200-MA — contrarian opportunity", dist.abs()));
        } else if dist > 40.0 {
            momentum -= 10.0;
            risk_flags.push(format!("{:.0}% above 200-MA — extended", dist));
        }
    }

    if let Some(rec) = q.recommendation_mean {
        if rec <= 1.8 {
            momentum += 12.0;
            reasons.push("Strong analyst buy consensus".into());
        } else if rec <= 2.5 {
            momentum += 6.0;
        } else if rec >= 4.0 {
            momentum -= 10.0;
            risk_flags.push("Analyst sell consensus".into());
        }
    }

    // Analyst upside
    if let (Some(target), Some(p)) = (q.target_mean_price, q.regular_market_price) {
        let upside = ((target - p) / p) * 100.0;
        if upside > 25.0 {
            momentum += 8.0;
            reasons.push(format!("Analysts see {:.0}% upside", upside));
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 5: DIVIDEND
    // ══════════════════════════════════════════

    let div_yield = q.trailing_annual_dividend_yield.unwrap_or(0.0);
    if div_yield > 0.04 {
        dividend += 25.0;
        reasons.push(format!("High yield ({:.1}%)", div_yield * 100.0));
    } else if div_yield > 0.025 {
        dividend += 15.0;
        reasons.push(format!("Good yield ({:.1}%)", div_yield * 100.0));
    } else if div_yield > 0.01 {
        dividend += 8.0;
    }
    // No dividend isn't necessarily bad for growth stocks
    if div_yield == 0.0 && growth > 70.0 {
        dividend += 5.0; // growth stock gets a pass on dividends
    }

    // ══════════════════════════════════════════
    // PILLAR 6: SAFETY
    // ══════════════════════════════════════════

    // Beta-based volatility
    if let Some(beta) = q.beta {
        if beta < 0.8 {
            safety += 15.0;
            reasons.push(format!("Defensive (beta {:.2})", beta));
        } else if beta < 1.2 {
            safety += 5.0;
        } else if beta > 1.8 {
            safety -= 15.0;
            risk_flags.push(format!("High volatility (beta {:.2})", beta));
        }
    }

    // Market cap (large cap = safer)
    if let Some(mc) = q.market_cap {
        if mc > 500_000_000_000.0 {
            safety += 10.0; // mega cap
        } else if mc > 100_000_000_000.0 {
            safety += 5.0; // large cap
        } else if mc < 10_000_000_000.0 {
            safety -= 5.0; // small cap
            risk_flags.push("Small-cap — higher risk".into());
        }
    }

    // Historical volatility from price data
    if let Some(closes) = hist_closes {
        if let Some(vol) = technical::annualized_volatility(closes) {
            if vol < 0.2 {
                safety += 8.0;
            } else if vol > 0.5 {
                safety -= 10.0;
                risk_flags.push(format!("High historical volatility ({:.0}%)", vol * 100.0));
            }
        }
    }

    // ══════════════════════════════════════════
    // MOAT ASSESSMENT
    // ══════════════════════════════════════════

    // Additional moat signals from market cap dominance
    if let Some(mc) = q.market_cap {
        if mc > 1_000_000_000_000.0 {
            moat_points += 1; // mega-cap likely has moat
        }
    }

    let moat = match moat_points {
        p if p >= 4 => MoatRating::Wide,
        p if p >= 2 => MoatRating::Narrow,
        _ => MoatRating::None,
    };

    if moat == MoatRating::Wide {
        reasons.push("Wide competitive moat (high margins + ROE + growth)".into());
    }

    // ══════════════════════════════════════════
    // COMPOSITE SCORE
    // ══════════════════════════════════════════

    val = val.clamp(0.0, 100.0);
    growth = growth.clamp(0.0, 100.0);
    quality = quality.clamp(0.0, 100.0);
    momentum = momentum.clamp(0.0, 100.0);
    dividend = dividend.clamp(0.0, 100.0);
    safety = safety.clamp(0.0, 100.0);

    // Dynamic weighting: growth investors weight growth more, value investors weight valuation more
    // We use a balanced approach with slight tilt to quality+growth (long-term compounders)
    let total = (val * 0.18 + growth * 0.22 + quality * 0.25 + momentum * 0.10 + dividend * 0.10 + safety * 0.15).clamp(0.0, 100.0);

    // Risk tier
    let risk_tier = if safety >= 65.0 && quality >= 60.0 {
        RiskTier::Conservative
    } else if safety >= 45.0 {
        RiskTier::Moderate
    } else {
        RiskTier::Aggressive
    };

    // ══════════════════════════════════════════
    // PROJECTIONS
    // ══════════════════════════════════════════

    // Estimated annual return: data-driven from score + growth + earnings yield
    let base_return = match total {
        s if s >= 85.0 => 20.0,
        s if s >= 75.0 => 16.0,
        s if s >= 65.0 => 12.0,
        s if s >= 55.0 => 9.0,
        s if s >= 45.0 => 6.0,
        s if s >= 35.0 => 3.0,
        _ => 0.0,
    };

    // Adjust return estimate based on growth rate
    let growth_adj = q.revenue_growth.unwrap_or(0.0) * 15.0; // revenue growth contributes
    let est_annual = (base_return + growth_adj).clamp(0.0, 30.0);

    let projected_5y = ((1.0 + est_annual / 100.0_f64).powi(5) - 1.0) * 100.0;
    let projected_10y = ((1.0 + est_annual / 100.0_f64).powi(10) - 1.0) * 100.0;

    // SIP to ₹10L in 10 years
    let monthly_rate = est_annual / 100.0 / 12.0;
    let sip = if monthly_rate > 0.001 {
        1_000_000.0 * monthly_rate / ((1.0 + monthly_rate).powf(120.0) - 1.0)
    } else {
        1_000_000.0 / 120.0
    };

    // DRIP multiplier: how much dividends add over 10 years
    let drip = if div_yield > 0.0 {
        (1.0 + div_yield).powi(10) // compound dividend reinvestment
    } else {
        1.0
    };

    // ══════════════════════════════════════════
    // MONTE CARLO SIMULATION (simplified)
    // ══════════════════════════════════════════

    let (mc_median, mc_p10, mc_p90) = if let Some(closes) = hist_closes {
        monte_carlo_5y(closes, est_annual)
    } else {
        let vol = 0.25; // assume 25% if no data
        simple_monte_carlo(est_annual, vol)
    };

    LongTermScore {
        symbol: q.symbol.clone().unwrap_or_default(),
        name: q.short_name.clone().or(q.long_name.clone()).unwrap_or_default(),
        price,
        currency: q.currency.clone(),
        valuation_score: val,
        growth_score: growth,
        quality_score: quality,
        momentum_score: momentum,
        dividend_score: dividend,
        safety_score: safety,
        total_score: total,
        moat,
        risk_tier,
        reasons,
        risk_flags,
        peg_ratio: peg,
        ev_ebitda,
        earnings_yield,
        est_annual_return: est_annual,
        projected_5y_return: projected_5y,
        projected_10y_return: projected_10y,
        sip_monthly_10l_10y: sip,
        drip_multiplier_10y: drip,
        monte_carlo_median: mc_median,
        monte_carlo_p10: mc_p10,
        monte_carlo_p90: mc_p90,
    }
}

/// Simplified Monte Carlo: 1000 simulations of 5-year returns
fn monte_carlo_5y(closes: &[f64], est_annual: f64) -> (f64, f64, f64) {
    let vol = technical::annualized_volatility(closes).unwrap_or(0.25);
    simple_monte_carlo(est_annual, vol)
}

fn simple_monte_carlo(est_annual: f64, vol: f64) -> (f64, f64, f64) {
    let daily_return = est_annual / 100.0 / 252.0;
    let daily_vol = vol / (252.0_f64).sqrt();
    let days = 252 * 5; // 5 years
    let sims = 1000;

    let mut results: Vec<f64> = Vec::with_capacity(sims);

    // Use a simple deterministic pseudo-random for reproducibility
    let mut seed: u64 = 42;
    for _ in 0..sims {
        let mut value = 100.0_f64;
        for _ in 0..days {
            // Simple LCG PRNG → Box-Muller approximation
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let u1 = (seed >> 33) as f64 / (1u64 << 31) as f64;
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let u2 = (seed >> 33) as f64 / (1u64 << 31) as f64;
            let u1 = u1.max(1e-10);
            let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
            let ret = daily_return + daily_vol * z;
            value *= 1.0 + ret;
        }
        results.push(value - 100.0); // return %
    }

    results.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = results[sims / 2];
    let p10 = results[sims / 10];
    let p90 = results[sims * 9 / 10];

    (median, p10, p90)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Quote;

    fn mock_quote(pe: f64, fwd_pe: f64, margin: f64, roe: f64, div: f64) -> Quote {
        let mut q = Quote::default();
        q.symbol = Some("TEST.NS".into());
        q.short_name = Some("Test Corp".into());
        q.regular_market_price = Some(100.0);
        q.trailing_pe = Some(pe);
        q.forward_pe = Some(fwd_pe);
        q.profit_margins = Some(margin);
        q.return_on_equity = Some(roe);
        q.trailing_annual_dividend_yield = Some(div);
        q.market_cap = Some(500_000_000_000.0);
        q
    }

    #[test]
    fn test_high_quality_stock_scores_high() {
        let q = mock_quote(12.0, 10.0, 0.25, 0.30, 0.035);
        let score = score_for_longterm(&q, None);
        assert!(score.total_score > 55.0, "High quality stock scored only {:.0}", score.total_score);
    }

    #[test]
    fn test_expensive_unprofitable_scores_low() {
        let q = mock_quote(60.0, 70.0, -0.05, -0.10, 0.0);
        let score = score_for_longterm(&q, None);
        assert!(score.total_score < 45.0, "Bad stock scored {:.0}", score.total_score);
    }

    #[test]
    fn test_moat_detection() {
        let q = mock_quote(15.0, 12.0, 0.30, 0.28, 0.02);
        let score = score_for_longterm(&q, None);
        assert!(score.moat == MoatRating::Wide || score.moat == MoatRating::Narrow,
            "High margin + ROE should detect moat, got {:?}", score.moat);
    }

    #[test]
    fn test_score_bounds() {
        let q = mock_quote(20.0, 18.0, 0.15, 0.12, 0.015);
        let score = score_for_longterm(&q, None);
        assert!(score.total_score >= 0.0 && score.total_score <= 100.0);
        assert!(score.valuation_score >= 0.0 && score.valuation_score <= 100.0);
        assert!(score.growth_score >= 0.0 && score.growth_score <= 100.0);
        assert!(score.quality_score >= 0.0 && score.quality_score <= 100.0);
        assert!(score.safety_score >= 0.0 && score.safety_score <= 100.0);
    }

    #[test]
    fn test_sip_calculation_positive() {
        let q = mock_quote(15.0, 12.0, 0.20, 0.15, 0.02);
        let score = score_for_longterm(&q, None);
        assert!(score.sip_monthly_10l_10y > 0.0);
        assert!(score.sip_monthly_10l_10y < 20000.0); // should be reasonable
    }

    #[test]
    fn test_monte_carlo_produces_results() {
        let q = mock_quote(15.0, 12.0, 0.20, 0.15, 0.02);
        let closes: Vec<f64> = (0..252).map(|i| 100.0 + (i as f64 * 0.05).sin() * 10.0).collect();
        let score = score_for_longterm(&q, Some(&closes));
        assert!(score.monte_carlo_p90 > score.monte_carlo_p10, "P90 should be > P10");
        assert!(score.monte_carlo_median > score.monte_carlo_p10);
    }

    #[test]
    fn test_drip_multiplier() {
        let q = mock_quote(15.0, 12.0, 0.20, 0.15, 0.04); // 4% dividend
        let score = score_for_longterm(&q, None);
        assert!(score.drip_multiplier_10y > 1.0, "DRIP should boost returns");
        assert!(score.drip_multiplier_10y < 2.0, "10Y DRIP at 4% should be ~1.48");
    }

    #[test]
    fn test_projected_returns_positive_for_good_stock() {
        let q = mock_quote(12.0, 10.0, 0.25, 0.25, 0.03);
        let score = score_for_longterm(&q, None);
        assert!(score.projected_5y_return > 0.0);
        assert!(score.projected_10y_return > score.projected_5y_return);
    }

    #[test]
    fn test_simple_monte_carlo() {
        let (median, p10, p90) = simple_monte_carlo(12.0, 0.25);
        assert!(p90 > p10);
        assert!(median > p10);
        assert!(p90 > median);
    }
}
