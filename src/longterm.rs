use crate::api::Quote;
use crate::technical;
use crate::verdict::Band;
use colored::Colorize;

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
    Lower,
    Moderate,
    Higher,
}

impl std::fmt::Display for RiskTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RiskTier::Lower => write!(f, "Lower"),
            RiskTier::Moderate => write!(f, "Moderate"),
            RiskTier::Higher => write!(f, "Higher"),
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
    /// Which pillars had at least one input (same order as the scores above).
    pub pillar_data: [bool; 6],
    /// Share of the composite weight whose pillar had data, 0 to 100.
    pub data_coverage: f64,
    pub moat: MoatRating,
    pub risk_tier: RiskTier,
    /// Facts that added points to a pillar, worded as data ("P/E 9.8 (below 12)").
    pub raised_by: Vec<String>,
    /// Facts that took points away, worded as data.
    pub lowered_by: Vec<String>,
    // Advanced metrics
    pub peg_ratio: Option<f64>,
    pub ev_ebitda: Option<f64>,
    pub earnings_yield: Option<f64>, // inverse of P/E — Buffett metric
    // Projections
    pub est_annual_return: f64,
    pub projected_5y_return: f64,
    pub projected_10y_return: f64,
    pub sip_monthly_10l_10y: f64, // SIP to reach SIP_TARGET (₹10L / $1M) in 10 years
    pub drip_multiplier_10y: f64, // dividend reinvestment multiplier over 10 years
    // Monte Carlo
    pub monte_carlo_median: f64, // median 5Y return from simulation
    pub monte_carlo_p10: f64,    // 10th percentile (bad case)
    pub monte_carlo_p90: f64,    // 90th percentile (good case)
}

/// The analysts' mean rating as a number and its scale, never as a call.
fn rating_text(mean: f64) -> String {
    format!("mean rating {mean:.1}/5 (1 = most positive)")
}

/// Pillar weights in the composite, in the order of `pillar_data`.
pub const WEIGHTS: [f64; 6] = [0.18, 0.22, 0.25, 0.10, 0.10, 0.15];
pub const PILLAR_NAMES: [&str; 6] = [
    "Valuation",
    "Growth",
    "Quality",
    "Momentum",
    "Dividend",
    "Safety",
];

/// The composite is only shown when pillars holding at least this share of the
/// weight had data (the same rule as `verdict`).
pub const MIN_COVERAGE: f64 = 50.0;

impl LongTermScore {
    pub fn pillar_scores(&self) -> [f64; 6] {
        [
            self.valuation_score,
            self.growth_score,
            self.quality_score,
            self.momentum_score,
            self.dividend_score,
            self.safety_score,
        ]
    }

    /// The pillar's score, or `None` when none of its inputs were available.
    pub fn pillar(&self, i: usize) -> Option<f64> {
        self.pillar_data[i].then(|| self.pillar_scores()[i])
    }

    /// The 0 to 100 composite, or `None` when too little data went into it.
    pub fn total(&self) -> Option<f64> {
        (self.data_coverage >= MIN_COVERAGE).then_some(self.total_score)
    }
}

/// Best composite first; names without a composite go last.
pub fn rank(scores: &mut [LongTermScore]) {
    scores.sort_by(|a, b| {
        let key = |s: &LongTermScore| s.total().unwrap_or(f64::NEG_INFINITY);
        key(b)
            .partial_cmp(&key(a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

/// Target of the "SIP for ₹10L / $1M in 10 years" line.
pub const SIP_TARGET: f64 = 1_000_000.0;

/// What a monthly SIP of `monthly` is worth after `years` at `annual_return_pct`
/// a year. Each instalment grows only for the time it stays invested.
pub fn sip_corpus(monthly: f64, years: u32, annual_return_pct: f64) -> f64 {
    monthly * crate::goals::sip_growth_factor(years, annual_return_pct, 0.0)
}

pub fn score_for_longterm(q: &Quote, hist_closes: Option<&[f64]>) -> LongTermScore {
    let mut val: f64 = 50.0;
    let mut growth: f64 = 50.0;
    let mut quality: f64 = 50.0;
    let mut momentum: f64 = 50.0;
    let mut dividend: f64 = 50.0;
    let mut safety: f64 = 50.0;
    let mut raised_by: Vec<String> = Vec::new();
    let mut lowered_by: Vec<String> = Vec::new();
    let mut moat_points: i32 = 0;
    // Pillars with at least one input: valuation, growth, quality, momentum, dividend, safety.
    let mut seen = [false; 6];

    let price = q.regular_market_price.unwrap_or(0.0);

    // ══════════════════════════════════════════
    // PILLAR 1: VALUATION (deep)
    // ══════════════════════════════════════════

    // P/E analysis
    seen[0] |=
        q.trailing_pe.is_some() || q.price_to_book.is_some() || q.enterprise_to_ebitda.is_some();
    if let Some(pe) = q.trailing_pe {
        if pe > 0.0 && pe < 12.0 {
            val += 25.0;
            raised_by.push(format!("P/E {:.1} (below 12)", pe));
        } else if pe < 18.0 {
            val += 15.0;
            raised_by.push(format!("P/E {:.1} (12 to 18)", pe));
        } else if pe < 30.0 {
            val += 5.0;
        } else if pe > 50.0 {
            val -= 15.0;
            lowered_by.push(format!("P/E {:.0} (above 50)", pe));
        }
    }

    // PEG ratio (P/E relative to growth)
    let peg = match (q.trailing_pe, q.earnings_quarterly_growth) {
        (Some(pe), Some(g)) if pe > 0.0 && g > 0.01 => {
            let peg = pe / (g * 100.0);
            if peg < 1.0 {
                val += 15.0;
                raised_by.push(format!("PEG {:.2} (below 1)", peg));
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
            lowered_by.push(format!("P/B {:.1} (above 10)", pb));
        }
    }

    // Forward P/E compression = earnings growth expected
    if let (Some(t), Some(f)) = (q.trailing_pe, q.forward_pe) {
        if f < t * 0.8 && t > 0.0 && f > 0.0 {
            val += 10.0;
            growth += 10.0;
            raised_by.push(format!(
                "Forward P/E {:.0} is more than 20% below trailing P/E {:.0} (earnings estimated to rise)",
                f, t
            ));
        } else if f > t * 1.2 && t > 0.0 {
            lowered_by.push(
                "Forward P/E is more than 20% above trailing P/E (earnings estimated to fall)"
                    .into(),
            );
        }
    }

    // EV/EBITDA
    let ev_ebitda = q.enterprise_to_ebitda;
    if let Some(ev) = ev_ebitda {
        if ev > 0.0 && ev < 10.0 {
            val += 8.0;
            raised_by.push(format!("EV/EBITDA {:.1}x (below 10)", ev));
        } else if ev > 25.0 {
            val -= 5.0;
        }
    }

    // Earnings yield (Buffett: compare to bond yields)
    let earnings_yield = q.trailing_pe.filter(|&pe| pe > 0.0).map(|pe| 100.0 / pe);
    if let Some(ey) = earnings_yield
        && ey > 8.0
    {
        val += 5.0; // earnings yield above 8%
    }

    // ══════════════════════════════════════════
    // PILLAR 2: GROWTH
    // ══════════════════════════════════════════

    seen[1] |= q.revenue_growth.is_some()
        || q.earnings_quarterly_growth.is_some()
        || (q.eps_trailing_twelve_months.is_some() && q.eps_forward.is_some());
    if let Some(g) = q.revenue_growth {
        let pct = g * 100.0;
        if pct > 25.0 {
            growth += 25.0;
            raised_by.push(format!("Revenue growth {:.0}% (above 25%)", pct));
            moat_points += 1;
        } else if pct > 15.0 {
            growth += 18.0;
            raised_by.push(format!("Revenue growth {:.0}% (above 15%)", pct));
        } else if pct > 8.0 {
            growth += 10.0;
        } else if pct > 0.0 {
            growth += 3.0;
        } else {
            growth -= 15.0;
            lowered_by.push(format!("Revenue growth {:.0}% (negative)", pct));
        }
    }

    if let Some(g) = q.earnings_quarterly_growth {
        let pct = g * 100.0;
        if pct > 30.0 {
            growth += 20.0;
            raised_by.push(format!("Quarterly earnings growth {:.0}% (above 30%)", pct));
        } else if pct > 15.0 {
            growth += 12.0;
        } else if pct < -15.0 {
            growth -= 12.0;
            lowered_by.push(format!(
                "Quarterly earnings growth {:.0}% (below -15%)",
                pct
            ));
        }
    }

    // EPS trajectory
    if let (Some(ttm), Some(fwd)) = (q.eps_trailing_twelve_months, q.eps_forward) {
        if fwd > ttm * 1.15 && ttm > 0.0 {
            growth += 10.0;
            raised_by.push(format!(
                "Forward EPS {:.0} is more than 15% above trailing EPS {:.0}",
                fwd, ttm
            ));
        } else if fwd < ttm * 0.85 && ttm > 0.0 {
            growth -= 8.0;
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 3: QUALITY (moat indicators)
    // ══════════════════════════════════════════

    seen[2] |=
        q.profit_margins.is_some() || q.return_on_equity.is_some() || q.debt_to_equity.is_some();
    if let Some(m) = q.profit_margins {
        let pct = m * 100.0;
        if pct > 25.0 {
            quality += 22.0;
            moat_points += 2;
            raised_by.push(format!("Profit margin {:.0}% (above 25%)", pct));
        } else if pct > 15.0 {
            quality += 12.0;
            moat_points += 1;
        } else if pct > 5.0 {
            quality += 3.0;
        } else if pct < 0.0 {
            quality -= 20.0;
            lowered_by.push("Profit margin is negative".into());
        }
    }

    if let Some(roe) = q.return_on_equity {
        let pct = roe * 100.0;
        if pct > 25.0 {
            quality += 18.0;
            moat_points += 2;
            raised_by.push(format!("ROE {:.0}% (above 25%)", pct));
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
            raised_by.push("Debt/equity below 30".into());
        } else if de < 80.0 {
            quality += 5.0;
            safety += 5.0;
        } else if de > 150.0 {
            quality -= 10.0;
            safety -= 15.0;
            lowered_by.push(format!("Debt/equity {:.0} (above 150)", de));
        } else if de > 250.0 {
            quality -= 20.0;
            safety -= 25.0;
            lowered_by.push(format!("Debt/equity {:.0} (above 250)", de));
        }
    }

    if let Some(cr) = q.current_ratio {
        if cr > 2.0 {
            safety += 10.0;
        } else if cr > 1.5 {
            safety += 5.0;
        } else if cr < 1.0 {
            safety -= 10.0;
            lowered_by.push(format!("Current ratio {:.1} (below 1)", cr));
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 4: MOMENTUM
    // ══════════════════════════════════════════

    seen[3] |= (q.regular_market_price.is_some()
        && q.fifty_day_average.is_some()
        && q.two_hundred_day_average.is_some())
        || q.recommendation_mean.is_some()
        || q.target_mean_price.is_some();
    if let (Some(p), Some(ma50), Some(ma200)) = (
        q.regular_market_price,
        q.fifty_day_average,
        q.two_hundred_day_average,
    ) {
        if p > ma50 && ma50 > ma200 {
            momentum += 20.0;
            raised_by.push(
                "Price is above the 50-day average, which is above the 200-day average".into(),
            );
        } else if p > ma200 && p < ma50 {
            momentum += 5.0; // between the two averages
        } else if p < ma50 && ma50 < ma200 {
            momentum -= 15.0;
            lowered_by.push(
                "Price is below the 50-day average, which is below the 200-day average".into(),
            );
        }

        // Distance from the 200-day average
        let dist = ((p - ma200) / ma200) * 100.0;
        if dist < -20.0 {
            momentum += 10.0;
            raised_by.push(format!(
                "Price is {:.0}% below the 200-day average",
                dist.abs()
            ));
        } else if dist > 40.0 {
            momentum -= 10.0;
            lowered_by.push(format!("Price is {:.0}% above the 200-day average", dist));
        }
    }

    if let Some(rec) = q.recommendation_mean {
        if rec <= 1.8 {
            momentum += 12.0;
            raised_by.push(format!("Analyst {}", rating_text(rec)));
        } else if rec <= 2.5 {
            momentum += 6.0;
        } else if rec >= 4.0 {
            momentum -= 10.0;
            lowered_by.push(format!("Analyst {}", rating_text(rec)));
        }
    }

    // Analyst upside
    if let (Some(target), Some(p)) = (q.target_mean_price, q.regular_market_price) {
        let upside = ((target - p) / p) * 100.0;
        if upside > 25.0 {
            momentum += 8.0;
            raised_by.push(format!(
                "Analyst mean price estimate is {:.0}% above the price",
                upside
            ));
        }
    }

    // ══════════════════════════════════════════
    // PILLAR 5: DIVIDEND
    // ══════════════════════════════════════════

    seen[4] = q.trailing_annual_dividend_yield.is_some();
    let div_yield = q.trailing_annual_dividend_yield.unwrap_or(0.0);
    if div_yield > 0.04 {
        dividend += 25.0;
        raised_by.push(format!(
            "Dividend yield {:.1}% (above 4%)",
            div_yield * 100.0
        ));
    } else if div_yield > 0.025 {
        dividend += 15.0;
        raised_by.push(format!(
            "Dividend yield {:.1}% (above 2.5%)",
            div_yield * 100.0
        ));
    } else if div_yield > 0.01 {
        dividend += 8.0;
    }
    // No dividend isn't necessarily bad for growth stocks
    if div_yield == 0.0 && growth > 70.0 {
        dividend += 5.0; // no dividend is not penalised for fast growers
    }

    // ══════════════════════════════════════════
    // PILLAR 6: SAFETY
    // ══════════════════════════════════════════

    // Beta-based volatility
    if let Some(beta) = q.beta {
        if beta < 0.8 {
            safety += 15.0;
            raised_by.push(format!("Beta {:.2} (below 0.8)", beta));
        } else if beta < 1.2 {
            safety += 5.0;
        } else if beta > 1.8 {
            safety -= 15.0;
            lowered_by.push(format!("Beta {:.2} (above 1.8)", beta));
        }
    }

    // Market cap (large cap = safer). The cut-offs are US dollars: the cap is
    // converted from the quote currency, so the same size scores the same in
    // rupees, euros or dollars. A currency we cannot convert gets no points.
    let currency = q.currency.as_deref().or_else(|| {
        q.symbol
            .as_deref()
            .and_then(crate::portfolio::symbol_currency)
    });
    let market_cap_usd = q
        .market_cap
        .and_then(|mc| crate::market::approx_usd(mc, currency));
    seen[5] |= q.beta.is_some()
        || market_cap_usd.is_some()
        || q.debt_to_equity.is_some()
        || q.current_ratio.is_some()
        || hist_closes.is_some_and(|c| technical::annualized_volatility(c).is_some());
    if let Some(mc) = market_cap_usd {
        if mc > 500_000_000_000.0 {
            safety += 10.0; // above $500B
        } else if mc > 100_000_000_000.0 {
            safety += 5.0; // above $100B
        } else if mc < 10_000_000_000.0 {
            safety -= 5.0; // below $10B
            lowered_by.push("Market cap below about $10B".into());
        }
    }

    // Historical volatility from price data
    if let Some(closes) = hist_closes
        && let Some(vol) = technical::annualized_volatility(closes)
    {
        if vol < 0.2 {
            safety += 8.0;
        } else if vol > 0.5 {
            safety -= 10.0;
            lowered_by.push(format!(
                "Annualised volatility {:.0}% (above 50%)",
                vol * 100.0
            ));
        }
    }

    // ══════════════════════════════════════════
    // MOAT ASSESSMENT
    // ══════════════════════════════════════════

    // Additional moat signals from market cap dominance
    if market_cap_usd.is_some_and(|mc| mc > 1_000_000_000_000.0) {
        moat_points += 1; // market cap above $1T
    }

    let moat = match moat_points {
        p if p >= 4 => MoatRating::Wide,
        p if p >= 2 => MoatRating::Narrow,
        _ => MoatRating::None,
    };

    if moat == MoatRating::Wide {
        raised_by.push("Moat signals: high margins, ROE and growth".into());
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
    let total = [val, growth, quality, momentum, dividend, safety]
        .iter()
        .zip(WEIGHTS)
        .map(|(score, w)| score * w)
        .sum::<f64>()
        .clamp(0.0, 100.0);

    let data_coverage = seen
        .iter()
        .zip(WEIGHTS)
        .filter(|(has, _)| **has)
        .map(|(_, w)| w)
        .sum::<f64>()
        * 100.0;

    // Risk tier
    let risk_tier = if safety >= 65.0 && quality >= 60.0 {
        RiskTier::Lower
    } else if safety >= 45.0 {
        RiskTier::Moderate
    } else {
        RiskTier::Higher
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

    // Monthly SIP that reaches 10 lakh (₹) / 1 million ($) in 10 years. Same
    // maths as `goal sip`: payments at the start of each month.
    let sip = SIP_TARGET / crate::goals::sip_growth_factor(10, est_annual, 0.0);

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
        name: q
            .short_name
            .clone()
            .or(q.long_name.clone())
            .unwrap_or_default(),
        price,
        currency: q.currency.clone(),
        valuation_score: val,
        growth_score: growth,
        quality_score: quality,
        momentum_score: momentum,
        dividend_score: dividend,
        safety_score: safety,
        total_score: total,
        pillar_data: seen,
        data_coverage,
        moat,
        risk_tier,
        raised_by,
        lowered_by,
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

// ── Human output ──

fn paint_band(text: &str, band: Band) -> String {
    match band {
        Band::Strong => text.green().to_string(),
        Band::Moderate => text.yellow().to_string(),
        Band::Weak => text.red().to_string(),
        Band::NoData => text.dimmed().to_string(),
    }
}

fn pillar_cell(score: Option<f64>) -> String {
    match score {
        Some(v) => format!("{v:>5.0}"),
        None => format!("{:>5}", "—"),
    }
}

fn percentile(v: f64) -> String {
    format!("{v:+.0}%")
}

/// The table and the detail sections of `longterm`, as text. Pillar scores are
/// descriptions of the data: a pillar without inputs shows a dash, the
/// composite is left out when under half its weight had data, and nothing in
/// it is a rating, a call or a target.
pub fn render_report(scores: &[LongTermScore], monthly: f64, csym: &str) -> String {
    use std::fmt::Write as _;
    let mut o = String::new();
    let line = |o: &mut String, text: String| {
        let _ = writeln!(o, "{text}");
    };
    let kv = |o: &mut String, key: &str, value: &str| {
        let _ = writeln!(
            o,
            "  {} {}",
            crate::display::pad_end(key, 30).dimmed(),
            value
        );
    };

    line(
        &mut o,
        format!("\n  {}", "Ranking (6-pillar score, 0-100)".bold().yellow()),
    );
    line(&mut o, format!("  {}", "─".repeat(40).dimmed()));
    let mut head = format!("  {:<14}", "Symbol".bold());
    for name in PILLAR_NAMES {
        let _ = write!(head, " {:>5}", &name[..3]);
    }
    let _ = write!(head, " {:>6} {:>6} {:>7}", "Score", "Moat", "Risk");
    line(&mut o, head);
    line(&mut o, format!("  {}", "─".repeat(78).dimmed()));
    for s in scores.iter().take(12) {
        let total = match s.total() {
            Some(t) => paint_band(&format!("{t:.0}"), Band::of(Some(t))),
            None => "n/a".dimmed().to_string(),
        };
        let moat = match s.moat {
            MoatRating::Wide => "Wide",
            MoatRating::Narrow => "Narrow",
            MoatRating::None => "—",
        };
        let mut row = format!("  {:<14}", s.symbol.cyan());
        for i in 0..6 {
            let _ = write!(row, " {}", pillar_cell(s.pillar(i)));
        }
        let risk = if s.total().is_some() {
            s.risk_tier.to_string()
        } else {
            "n/a".to_string()
        };
        let _ = write!(row, " {:>6} {:>6} {:>7}", total, moat, risk);
        line(&mut o, row);
    }
    if scores.iter().take(12).any(|s| s.total().is_none()) {
        line(
            &mut o,
            format!(
                "  {}",
                "n/a: under half of the score's inputs were available. — : no data for that pillar."
                    .dimmed()
            ),
        );
    }

    for (i, s) in scores
        .iter()
        .filter(|s| s.total().is_some())
        .take(3)
        .enumerate()
    {
        let total = s.total_score;
        line(
            &mut o,
            format!(
                "\n  {}",
                format!("#{} {} — {} [{}]", i + 1, s.symbol, s.name, s.moat)
                    .bold()
                    .yellow()
            ),
        );
        line(&mut o, format!("  {}", "─".repeat(40).dimmed()));
        kv(&mut o, "Price", &format!("{}{:.2}", csym, s.price));
        kv(
            &mut o,
            "Score",
            &format!(
                "{:.0}/100 ({}) · data coverage {:.0}%",
                total,
                Band::of(Some(total)).label(),
                s.data_coverage
            ),
        );
        kv(&mut o, "Risk level", &s.risk_tier.to_string());
        let bands = (0..6)
            .map(|i| format!("{} {}", PILLAR_NAMES[i], Band::of(s.pillar(i)).label()))
            .collect::<Vec<_>>()
            .join(" · ");
        kv(&mut o, "Pillar bands", &bands);
        if let Some(peg) = s.peg_ratio {
            kv(&mut o, "PEG ratio", &format!("{peg:.2}"));
        }
        if let Some(ey) = s.earnings_yield {
            kv(&mut o, "Earnings yield", &format!("{ey:.1}%"));
        }
        if !s.raised_by.is_empty() {
            line(&mut o, format!("\n    {}", "Added points".dimmed()));
            for r in &s.raised_by {
                line(&mut o, format!("    {} {}", "+".green(), r));
            }
        }
        if !s.lowered_by.is_empty() {
            line(&mut o, format!("\n    {}", "Took points away".dimmed()));
            for r in &s.lowered_by {
                line(&mut o, format!("    {} {}", "−".red(), r));
            }
        }

        line(
            &mut o,
            format!(
                "\n  {}",
                "Illustration at an assumed return".bold().yellow()
            ),
        );
        line(&mut o, format!("  {}", "─".repeat(40).dimmed()));
        kv(
            &mut o,
            "Assumed annual return",
            &format!(
                "{:.1}% (set from the score; not a forecast)",
                s.est_annual_return
            ),
        );
        kv(
            &mut o,
            "5 years at that rate",
            &format!("{:+.0}%", s.projected_5y_return),
        );
        kv(
            &mut o,
            "10 years at that rate",
            &format!("{:+.0}%", s.projected_10y_return),
        );
        if s.drip_multiplier_10y > 1.01 {
            kv(
                &mut o,
                "Dividends reinvested (10Y)",
                &format!("{:.2}x", s.drip_multiplier_10y),
            );
        }
        kv(
            &mut o,
            &format!(
                "SIP to reach {} in 10Y",
                crate::display::format_large_number(SIP_TARGET, s.currency.as_deref())
            ),
            &format!("{}{:.0}/month at that rate", csym, s.sip_monthly_10l_10y),
        );

        line(
            &mut o,
            format!("\n  {}", "Simulated 5Y range (1000 paths)".bold().yellow()),
        );
        line(&mut o, format!("  {}", "─".repeat(40).dimmed()));
        kv(&mut o, "90th percentile", &percentile(s.monte_carlo_p90));
        kv(&mut o, "Median", &percentile(s.monte_carlo_median));
        kv(&mut o, "10th percentile", &percentile(s.monte_carlo_p10));

        // SIP calculator: every instalment grows for its own time in the market.
        let inv_5y = monthly * 60.0;
        let inv_10y = monthly * 120.0;
        line(&mut o, format!("\n  {}", "SIP arithmetic".bold().yellow()));
        line(&mut o, format!("  {}", "─".repeat(40).dimmed()));
        kv(
            &mut o,
            &format!("{csym}{monthly}/mo × 5Y"),
            &format!(
                "{csym}{inv_5y:.0} paid in → {csym}{:.0}",
                sip_corpus(monthly, 5, s.est_annual_return)
            ),
        );
        kv(
            &mut o,
            &format!("{csym}{monthly}/mo × 10Y"),
            &format!(
                "{csym}{inv_10y:.0} paid in → {csym}{:.0}",
                sip_corpus(monthly, 10, s.est_annual_return)
            ),
        );
        line(
            &mut o,
            format!(
                "    {}",
                format!(
                    "at the assumed {:.1}%/yr, paid at the start of each month; dividends are not added on top",
                    s.est_annual_return
                )
                .dimmed()
            ),
        );
    }

    line(&mut o, format!("\n  {}", "─".repeat(60).dimmed()));
    line(
        &mut o,
        format!("  {}", crate::verdict::DISCLAIMER.dimmed().italic()),
    );
    line(
        &mut o,
        format!(
            "  {}",
            "The assumed return and the simulation are illustrations built from the score and the past year's volatility. Past performance does not predict future results."
                .dimmed()
                .italic()
        ),
    );
    o
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
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u1 = (seed >> 33) as f64 / (1u64 << 31) as f64;
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u2 = (seed >> 33) as f64 / (1u64 << 31) as f64;
            let u1 = u1.max(1e-10);
            let z = (-2.0 * u1.ln()).sqrt() * (2.0 * std::f64::consts::PI * u2).cos();
            let ret = daily_return + daily_vol * z;
            value *= 1.0 + ret;
        }
        results.push(value - 100.0); // return %
    }

    results.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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
    fn sip_corpus_grows_each_instalment_for_its_own_time() {
        // ₹15,000/month at 12% a year (1% a month), issue #67.
        let five = sip_corpus(15_000.0, 5, 12.0);
        let ten = sip_corpus(15_000.0, 10, 12.0);
        assert!(
            (five / 100_000.0 - 12.37).abs() / 12.37 < 0.005,
            "5y {five}"
        );
        assert!((ten / 100_000.0 - 34.85).abs() / 34.85 < 0.005, "10y {ten}");
        // Never more than paid in times the growth of the full period.
        assert!(five < 15_000.0 * 60.0 * 1.12_f64.powi(5));
    }

    #[test]
    fn sip_corpus_without_growth_is_what_was_paid_in() {
        assert!((sip_corpus(1_000.0, 5, 0.0) - 60_000.0).abs() < 1e-6);
    }

    #[test]
    fn sip_to_target_matches_goal_plan() {
        let q = mock_quote(15.0, 14.0, 0.2, 0.2, 0.02);
        let s = score_for_longterm(&q, None);
        let plan = crate::goals::plan_sip(SIP_TARGET, 10, s.est_annual_return, 0.0, 0.0);
        assert!((s.sip_monthly_10l_10y - plan.monthly_sip).abs() < 1e-6);
        // And that SIP really reaches the target.
        let got = sip_corpus(s.sip_monthly_10l_10y, 10, s.est_annual_return);
        assert!((got - SIP_TARGET).abs() < 1.0, "{got}");
    }

    #[test]
    fn test_high_quality_stock_scores_high() {
        let q = mock_quote(12.0, 10.0, 0.25, 0.30, 0.035);
        let score = score_for_longterm(&q, None);
        assert!(
            score.total_score > 55.0,
            "High quality stock scored only {:.0}",
            score.total_score
        );
    }

    #[test]
    fn test_expensive_unprofitable_scores_low() {
        let q = mock_quote(60.0, 70.0, -0.05, -0.10, 0.0);
        let score = score_for_longterm(&q, None);
        assert!(
            score.total_score < 45.0,
            "Bad stock scored {:.0}",
            score.total_score
        );
    }

    #[test]
    fn test_moat_detection() {
        let q = mock_quote(15.0, 12.0, 0.30, 0.28, 0.02);
        let score = score_for_longterm(&q, None);
        assert!(
            score.moat == MoatRating::Wide || score.moat == MoatRating::Narrow,
            "High margin + ROE should detect moat, got {:?}",
            score.moat
        );
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
        let closes: Vec<f64> = (0..252)
            .map(|i| 100.0 + (i as f64 * 0.05).sin() * 10.0)
            .collect();
        let score = score_for_longterm(&q, Some(&closes));
        assert!(
            score.monte_carlo_p90 > score.monte_carlo_p10,
            "P90 should be > P10"
        );
        assert!(score.monte_carlo_median > score.monte_carlo_p10);
    }

    #[test]
    fn test_drip_multiplier() {
        let q = mock_quote(15.0, 12.0, 0.20, 0.15, 0.04); // 4% dividend
        let score = score_for_longterm(&q, None);
        assert!(score.drip_multiplier_10y > 1.0, "DRIP should boost returns");
        assert!(
            score.drip_multiplier_10y < 2.0,
            "10Y DRIP at 4% should be ~1.48"
        );
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

    #[test]
    fn market_cap_cutoffs_are_dollars_in_any_currency() {
        let capped = |cap: f64, cur: &str, sym: &str| {
            let mut q = mock_quote(15.0, 14.0, 0.2, 0.2, 0.02);
            q.symbol = Some(sym.into());
            q.currency = Some(cur.into());
            q.market_cap = Some(cap);
            score_for_longterm(&q, None)
        };
        // $600B is mega cap (+10 safety); so is the same size in rupees.
        let usd = capped(6.0e11, "USD", "BIG");
        let inr = capped(6.0e11 / 0.0114, "INR", "BIG.NS");
        assert_eq!(usd.safety_score, inr.safety_score);
        // A $5B company is small (< $10B); so is ₹5,000 crore (about $570M)...
        let small_us = capped(5.0e9, "USD", "MID");
        let small_in = capped(5.0e10, "INR", "SMALL.NS");
        assert!(
            small_us
                .lowered_by
                .iter()
                .any(|f| f.contains("Market cap below"))
        );
        assert!(
            small_in
                .lowered_by
                .iter()
                .any(|f| f.contains("Market cap below"))
        );
        // ...but ₹5 lakh crore (about $57B) is neither flagged nor a mega cap.
        let large_in = capped(5.0e12, "INR", "LARGE.NS");
        assert!(
            !large_in
                .lowered_by
                .iter()
                .any(|f| f.contains("Market cap below"))
        );
        // A rupee cap over ₹1 lakh crore no longer earns the $1T moat point.
        let moaty_in = capped(1.5e12, "INR", "A.NS");
        let moaty_us = capped(1.5e12, "USD", "A");
        assert!(moaty_us.moat as u8 >= moaty_in.moat as u8);
    }

    // ── No advice in anything the command shows ──

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\u{1b}' {
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    /// `assert_neutral` plus the words a call or a level would use.
    fn assert_no_advice(text: &str) {
        let plain = strip_ansi(text);
        crate::verdict::testing::assert_neutral(&plain);
        let lower = plain.to_lowercase();
        for word in [
            "avoid",
            "subscribe",
            "apply",
            "target",
            "entry",
            "stop",
            "consider",
            "recommend",
            "outperform",
            "underperform",
            "overweight",
            "underweight",
            "upside",
            "opportunity",
            "bargain",
            "dip",
        ] {
            let mut from = 0;
            while let Some(i) = lower[from..].find(word) {
                let at = from + i;
                let end = at + word.len();
                let whole = !lower[..at]
                    .chars()
                    .next_back()
                    .is_some_and(char::is_alphanumeric)
                    && !lower[end..]
                        .chars()
                        .next()
                        .is_some_and(char::is_alphanumeric);
                assert!(
                    !whole,
                    "advice wording {word:?}: {}",
                    &plain[at.saturating_sub(40)..(end + 40).min(plain.len())]
                );
                from = end;
            }
        }
    }

    /// Every field the scorer reads, set to a bullish extreme.
    fn full_quote(symbol: &str) -> Quote {
        let mut q = mock_quote(9.0, 6.0, 0.30, 0.30, 0.05);
        q.symbol = Some(symbol.into());
        q.currency = Some("USD".into());
        q.price_to_book = Some(1.5);
        q.enterprise_to_ebitda = Some(8.0);
        q.earnings_quarterly_growth = Some(0.4);
        q.revenue_growth = Some(0.3);
        q.eps_trailing_twelve_months = Some(5.0);
        q.eps_forward = Some(7.0);
        q.debt_to_equity = Some(20.0);
        q.current_ratio = Some(2.5);
        q.fifty_day_average = Some(110.0);
        q.two_hundred_day_average = Some(90.0);
        q.recommendation_mean = Some(1.4);
        q.target_mean_price = Some(150.0);
        q.beta = Some(0.7);
        q.market_cap = Some(2.0e12);
        q
    }

    /// The opposite extreme: expensive, losing money, heavy debt, downtrend.
    fn weak_quote(symbol: &str) -> Quote {
        let mut q = mock_quote(80.0, 120.0, -0.1, -0.2, 0.0);
        q.symbol = Some(symbol.into());
        q.currency = Some("USD".into());
        q.price_to_book = Some(15.0);
        q.enterprise_to_ebitda = Some(40.0);
        q.earnings_quarterly_growth = Some(-0.4);
        q.revenue_growth = Some(-0.2);
        q.eps_trailing_twelve_months = Some(5.0);
        q.eps_forward = Some(3.0);
        q.debt_to_equity = Some(300.0);
        q.current_ratio = Some(0.6);
        q.fifty_day_average = Some(90.0);
        q.two_hundred_day_average = Some(130.0);
        q.recommendation_mean = Some(4.6);
        q.target_mean_price = Some(70.0);
        q.beta = Some(2.2);
        q.market_cap = Some(2.0e9);
        q
    }

    fn sparse_quote(symbol: &str) -> Quote {
        let mut q = Quote::default();
        q.symbol = Some(symbol.into());
        q.regular_market_price = Some(100.0);
        q.trailing_pe = Some(22.0);
        q
    }

    fn empty_quote(symbol: &str) -> Quote {
        let mut q = Quote::default();
        q.symbol = Some(symbol.into());
        q
    }

    fn scored(quotes: &[Quote]) -> Vec<LongTermScore> {
        let closes: Vec<f64> = (0..252)
            .map(|i| 100.0 + (i as f64 * 0.07).sin() * 12.0)
            .collect();
        let mut v: Vec<_> = quotes
            .iter()
            .map(|q| score_for_longterm(q, Some(&closes)))
            .collect();
        rank(&mut v);
        v
    }

    #[test]
    fn report_is_neutral_for_full_weak_sparse_and_empty_data() {
        for (name, quotes) in [
            ("full", vec![full_quote("AAA")]),
            ("weak", vec![weak_quote("BBB")]),
            ("sparse", vec![sparse_quote("CCC")]),
            ("empty", vec![empty_quote("DDD")]),
            (
                "mixed",
                vec![
                    full_quote("AAA"),
                    weak_quote("BBB"),
                    sparse_quote("CCC"),
                    empty_quote("DDD"),
                ],
            ),
        ] {
            let scores = scored(&quotes);
            let text = render_report(&scores, 10_000.0, "$");
            assert!(text.contains("Ranking"), "{name}: {text}");
            assert_no_advice(&text);
            for s in &scores {
                for line in s.raised_by.iter().chain(&s.lowered_by) {
                    assert_no_advice(line);
                }
                assert_no_advice(&s.moat.to_string());
                assert_no_advice(&s.risk_tier.to_string());
            }
        }
    }

    #[test]
    fn extreme_quotes_trigger_every_wording_path() {
        let strong = score_for_longterm(&full_quote("AAA"), None);
        let weak = score_for_longterm(&weak_quote("BBB"), None);
        // The analyst lines are the number and its scale, with no call.
        assert!(
            strong
                .raised_by
                .iter()
                .any(|r| r == "Analyst mean rating 1.4/5 (1 = most positive)")
        );
        assert!(
            weak.lowered_by
                .iter()
                .any(|r| r == "Analyst mean rating 4.6/5 (1 = most positive)")
        );
        assert!(
            strong
                .raised_by
                .iter()
                .any(|r| r.starts_with("Analyst mean price estimate is"))
        );
        // Plenty of lines on both sides, all of them data.
        assert!(strong.raised_by.len() >= 10, "{:?}", strong.raised_by);
        assert!(weak.lowered_by.len() >= 8, "{:?}", weak.lowered_by);
        for line in strong.raised_by.iter().chain(&weak.lowered_by) {
            assert_no_advice(line);
        }
    }

    #[test]
    fn empty_quote_has_no_composite_and_says_so() {
        let scores = scored(&[empty_quote("DDD")]);
        let s = &scores[0];
        assert!(s.total().is_none());
        assert_eq!(s.pillar_data.iter().filter(|p| **p).count(), 1); // volatility from the chart
        let text = strip_ansi(&render_report(&scores, 10_000.0, "$"));
        assert!(text.contains("n/a"), "{text}");
        // No detail section, projection or SIP line for a name without a score.
        assert!(!text.contains("Illustration"), "{text}");
        assert!(!text.contains("SIP"), "{text}");
    }

    #[test]
    fn sparse_quote_dashes_the_pillars_it_has_no_data_for() {
        let s = score_for_longterm(&sparse_quote("CCC"), None);
        assert_eq!(s.pillar_data, [true, false, false, false, false, false]);
        assert!((s.data_coverage - 18.0).abs() < 1e-9);
        assert!(s.total().is_none());
        assert!(s.pillar(0).is_some() && s.pillar(1).is_none());
    }

    #[test]
    fn full_quote_has_full_coverage_and_is_ranked_first() {
        let scores = scored(&[
            empty_quote("DDD"),
            weak_quote("BBB"),
            full_quote("AAA"),
            sparse_quote("CCC"),
        ]);
        assert_eq!(scores[0].symbol, "AAA");
        assert!((scores[0].data_coverage - 100.0).abs() < 1e-9);
        // Names without a composite come after every name with one.
        let first_none = scores.iter().position(|s| s.total().is_none()).unwrap();
        assert!(scores[first_none..].iter().all(|s| s.total().is_none()));
    }

    #[test]
    fn detail_sections_describe_data_and_label_the_projection() {
        let scores = scored(&[full_quote("AAA")]);
        let text = strip_ansi(&render_report(&scores, 15_000.0, "$"));
        for part in [
            "Pillar bands",
            "Added points",
            "Assumed annual return",
            "not a forecast",
            "90th percentile",
            "10th percentile",
            "not a recommendation",
        ] {
            assert!(text.contains(part), "missing {part:?} in {text}");
        }
        for gone in [
            "Best case",
            "Worst case",
            "Conservative",
            "Aggressive",
            "Golden Cross",
        ] {
            assert!(!text.contains(gone), "{gone:?} still shown");
        }
    }

    #[test]
    fn rating_text_is_a_number_and_a_scale() {
        assert_eq!(rating_text(1.84), "mean rating 1.8/5 (1 = most positive)");
    }
}
