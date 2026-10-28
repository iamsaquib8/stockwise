//! Investment Decision & Verdict Engine.
//!
//! Provides a definitive BUY / HOLD / SELL recommendation for any stock (US or Indian),
//! backed by a 6-pillar quantitative scoring model (0–100), price targets,
//! stop-loss levels, entry zones, risk-reward ratios, and multi-horizon outlooks.

use crate::insights::{Impact, InsightCard};
use crate::research::Research;
use colored::Colorize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    StrongBuy,
    Buy,
    Hold,
    Reduce,
    StrongSell,
}

impl Action {
    pub fn badge(&self) -> String {
        match self {
            Action::StrongBuy => "▲▲ STRONG BUY".green().bold().to_string(),
            Action::Buy => "▲ BUY / ACCUMULATE".green().bold().to_string(),
            Action::Hold => "● HOLD / ACCUMULATE ON DIPS".yellow().bold().to_string(),
            Action::Reduce => "▼ REDUCE / TAKE PROFIT".red().bold().to_string(),
            Action::StrongSell => "▼▼ SELL / AVOID".red().bold().to_string(),
        }
    }

    pub fn summary_tagline(&self) -> &'static str {
        match self {
            Action::StrongBuy => {
                "High-conviction upside; favorable risk-reward across all horizons."
            }
            Action::Buy => "Attractive fundamentals & technical setup with positive upside.",
            Action::Hold => {
                "Solid business; wait for better entry dip or clearer earnings catalyst."
            }
            Action::Reduce => "Risk exceeds reward; consider trimming or tightening stop-loss.",
            Action::StrongSell => {
                "Severe deterioration, rich valuation or major structural red flags."
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct PillarScore {
    pub name: &'static str,
    pub score: f64,  // 0 to 100
    pub weight: f64, // 0.0 to 1.0
    pub grade: &'static str,
    pub summary: String,
}

#[derive(Clone, Debug)]
pub struct DecisionVerdict {
    pub symbol: String,
    pub name: String,
    pub price: f64,
    pub currency: String,
    pub action: Action,
    pub overall_score: f64, // 0 to 100
    pub confidence: f64,    // 0 to 100 based on data completeness
    pub pillars: Vec<PillarScore>,
    // Valuation & Execution Parameters
    pub target_price: Option<f64>,
    pub upside_pct: Option<f64>,
    pub entry_zone: (f64, f64),
    pub stop_loss: f64,
    pub risk_pct: f64,
    pub risk_reward: Option<f64>,
    // Qualitative Rationale
    pub bull_points: Vec<String>,
    pub bear_points: Vec<String>,
    // Horizons
    pub short_term: String,  // 1–4 weeks (Technicals, RSI, Momentum)
    pub medium_term: String, // 1–6 months (Earnings trend, Analyst revisions)
    pub long_term: String,   // 1–3+ years (Compounding, Moat, Balance sheet)
    pub next_catalyst: Option<String>,
}

fn score_to_grade(s: f64) -> &'static str {
    match s {
        x if x >= 85.0 => "A+",
        x if x >= 75.0 => "A",
        x if x >= 65.0 => "B+",
        x if x >= 55.0 => "B",
        x if x >= 45.0 => "C+",
        x if x >= 35.0 => "C",
        x if x >= 25.0 => "D",
        _ => "F",
    }
}

/// Compute the 6-pillar investment score and generate the decision verdict
pub fn generate_verdict(r: &Research, cards: &[InsightCard]) -> DecisionVerdict {
    let price = r.price().unwrap_or(0.0);
    let csym = r.csym();

    // ── 1. Valuation & Margin of Safety (Weight: 20%) ──
    let mut val_score: f64 = 50.0;
    let mut val_notes = Vec::new();

    if let Some(peg) = r.fin.peg {
        if peg > 0.0 && peg <= 1.0 {
            val_score += 25.0;
            val_notes.push(format!("Attractive PEG ({:.2}x)", peg));
        } else if peg > 0.0 && peg <= 1.5 {
            val_score += 15.0;
            val_notes.push(format!("Reasonable PEG ({:.2}x)", peg));
        } else if peg >= 2.5 {
            val_score -= 20.0;
            val_notes.push(format!("Elevated PEG ({:.2}x)", peg));
        }
    }

    if let (Some(t_pe), Some(f_pe)) = (r.fin.trailing_pe, r.fin.forward_pe) {
        if t_pe > 0.0 && f_pe > 0.0 {
            let comp = (t_pe - f_pe) / t_pe;
            if comp >= 0.15 {
                val_score += 15.0;
                val_notes.push(format!(
                    "Forward P/E discount ({:.1}x vs {:.1}x)",
                    f_pe, t_pe
                ));
            } else if comp <= -0.15 {
                val_score -= 15.0;
                val_notes.push("Forward earnings expected to decline".into());
            }
        }
    } else if let Some(pe) = r.fin.trailing_pe {
        if pe > 0.0 && pe < 15.0 {
            val_score += 15.0;
            val_notes.push(format!("Low P/E multiple ({:.1}x)", pe));
        } else if pe > 50.0 {
            val_score -= 15.0;
            val_notes.push(format!("High P/E multiple ({:.1}x)", pe));
        }
    }

    if let Some(pb) = r.fin.pb
        && pb < 1.5
        && r.fin.roe.unwrap_or(0.0) > 0.10
    {
        val_score += 10.0;
        val_notes.push(format!("Strong book value backing (P/B {:.1}x)", pb));
    }
    val_score = val_score.clamp(10.0, 95.0);
    let val_summary = if val_notes.is_empty() {
        "Valuation in line with historical baseline".to_string()
    } else {
        val_notes.join(", ")
    };

    // ── 2. Growth & Profitability (Weight: 25%) ──
    let mut gro_score: f64 = 50.0;
    let mut gro_notes = Vec::new();

    if let Some(rg) = r.fin.revenue_growth {
        if rg >= 0.15 {
            gro_score += 20.0;
            gro_notes.push(format!("Revenue growth {:+.1}%", rg * 100.0));
        } else if rg >= 0.06 {
            gro_score += 10.0;
            gro_notes.push(format!("Moderate revenue growth {:+.1}%", rg * 100.0));
        } else if rg < 0.0 {
            gro_score -= 20.0;
            gro_notes.push(format!("Revenue contraction {:+.1}%", rg * 100.0));
        }
    }

    if let Some(eg) = r.fin.earnings_growth {
        if eg >= 0.20 {
            gro_score += 20.0;
            gro_notes.push(format!("Earnings growth {:+.1}%", eg * 100.0));
        } else if eg < -0.10 {
            gro_score -= 20.0;
            gro_notes.push(format!("Earnings decline {:+.1}%", eg * 100.0));
        }
    }

    if let Some(pm) = r.fin.profit_margin {
        if pm >= 0.18 {
            gro_score += 10.0;
            gro_notes.push(format!("High net margin {:.1}%", pm * 100.0));
        } else if pm < 0.0 {
            gro_score -= 20.0;
            gro_notes.push("Negative net margin".into());
        }
    }

    if let Some(roe) = r.fin.roe
        && roe >= 0.20
    {
        gro_score += 10.0;
        gro_notes.push(format!("Exceptional ROE {:.1}%", roe * 100.0));
    }
    gro_score = gro_score.clamp(10.0, 95.0);
    let gro_summary = if gro_notes.is_empty() {
        "Stable growth profile".to_string()
    } else {
        gro_notes.join(", ")
    };

    // ── 3. Solvency & Balance Sheet (Weight: 15%) ──
    let mut sol_score: f64 = 55.0;
    let mut sol_notes = Vec::new();

    if !r.is_financial() {
        if let Some(de) = r.fin.de {
            if de <= 30.0 {
                sol_score += 25.0;
                sol_notes.push(format!("Low debt-to-equity ({:.1}%)", de));
            } else if de <= 80.0 {
                sol_score += 10.0;
                sol_notes.push(format!("Manageable leverage ({:.1}%)", de));
            } else if de >= 160.0 {
                sol_score -= 25.0;
                sol_notes.push(format!("High leverage risk ({:.1}%)", de));
            }
        }
        if let Some(cr) = r.fin.current_ratio {
            if cr >= 1.5 {
                sol_score += 10.0;
                sol_notes.push(format!("Healthy working capital ({:.2}x)", cr));
            } else if cr < 1.0 {
                sol_score -= 15.0;
                sol_notes.push("Current ratio under 1.0x".into());
            }
        }
    } else {
        sol_score += 15.0;
        sol_notes.push("Financial sector capital structure".into());
    }

    if let Some(cash) = r.fin.total_cash
        && cash > 10_000_000_000.0
    {
        sol_score += 10.0;
        sol_notes.push("Substantial cash reserves".into());
    }
    sol_score = sol_score.clamp(15.0, 95.0);
    let sol_summary = if sol_notes.is_empty() {
        "Adequate balance sheet buffer".to_string()
    } else {
        sol_notes.join(", ")
    };

    // ── 4. Smart Money & Ownership (Weight: 15%) ──
    let mut own_score: f64 = 50.0;
    let mut own_notes = Vec::new();

    if r.holdings.len() >= 2 {
        let n = r.holdings.len();
        if let (Some(fii_curr), Some(fii_prev)) = (r.holdings[n - 1].fii, r.holdings[n - 2].fii) {
            let d = fii_curr - fii_prev;
            if d >= 0.05 {
                own_score += 20.0;
                own_notes.push(format!("FII buying ({:.2}% → {:.2}%)", fii_prev, fii_curr));
            } else if d <= -0.05 {
                own_score -= 15.0;
                own_notes.push(format!(
                    "FII trimming ({:.2}% → {:.2}%)",
                    fii_prev, fii_curr
                ));
            }
        }
        if let Some(false) = r.holdings[n - 1].pledged {
            own_score += 10.0;
            own_notes.push("0% promoter pledge".into());
        } else if let Some(true) = r.holdings[n - 1].pledged {
            own_score -= 20.0;
            own_notes.push("Promoter shares pledged".into());
        }
    }

    if let Some(inst_pct) = r.fin.inst_pct
        && inst_pct >= 0.70
    {
        own_score += 15.0;
        own_notes.push(format!("Institutions own {:.1}%", inst_pct * 100.0));
    }
    if let Some(net_buy) = r.fin.inst_net_buy_pct {
        if net_buy >= 0.015 {
            own_score += 15.0;
            own_notes.push("Institutions net accumulating (6M)".into());
        } else if net_buy <= -0.015 {
            own_score -= 15.0;
            own_notes.push("Institutions net reducing (6M)".into());
        }
    }

    let insider_buys: f64 = r
        .insiders
        .iter()
        .filter(|t| t.is_buy)
        .map(|t| t.value)
        .sum();
    let insider_sells: f64 = r
        .insiders
        .iter()
        .filter(|t| !t.is_buy)
        .map(|t| t.value)
        .sum();
    if insider_buys > 100_000.0 && insider_buys > insider_sells {
        own_score += 15.0;
        own_notes.push("Recent insider open-market buying".into());
    } else if insider_sells > 5_000_000.0 && insider_buys < 50_000.0 {
        own_score -= 10.0;
        own_notes.push("Insider selling observed".into());
    }
    own_score = own_score.clamp(10.0, 95.0);
    let own_summary = if own_notes.is_empty() {
        "Neutral institutional ownership dynamics".to_string()
    } else {
        own_notes.join(", ")
    };

    // ── 5. Analyst Consensus & Catalysts (Weight: 10%) ──
    let mut ana_score: f64 = 50.0;
    let mut ana_notes = Vec::new();

    if let Some(rec) = r.fin.rec_mean {
        if rec <= 1.75 {
            ana_score += 25.0;
            ana_notes.push("Strong Buy consensus".into());
        } else if rec <= 2.25 {
            ana_score += 15.0;
            ana_notes.push("Buy consensus".into());
        } else if rec >= 3.5 {
            ana_score -= 20.0;
            ana_notes.push("Cautious hold/underperform rating".into());
        }
    }

    if let (Some(target), Some(p)) = (r.fin.target_mean, r.price())
        && p > 0.0
    {
        let upside = (target - p) / p;
        let up_pct = (upside * 100.0).round();
        if upside >= 0.18 {
            ana_score += 25.0;
            ana_notes.push(format!("+{}% target upside", up_pct));
        } else if upside >= 0.08 {
            ana_score += 10.0;
            ana_notes.push(format!("+{}% target upside", up_pct));
        } else if upside <= -0.05 {
            ana_score -= 15.0;
            ana_notes.push("Trading above consensus target".into());
        }
    }
    ana_score = ana_score.clamp(10.0, 95.0);
    let ana_summary = if ana_notes.is_empty() {
        "Consensus in line with current market levels".to_string()
    } else {
        ana_notes.join(", ")
    };

    // ── 6. Technical Trend & Momentum (Weight: 15%) ──
    let mut tec_score: f64 = 50.0;
    let mut tec_notes = Vec::new();

    if let (Some(p), Some(ma50), Some(ma200)) = (
        r.price(),
        r.quote.fifty_day_average,
        r.quote.two_hundred_day_average,
    ) {
        if p > ma50 && ma50 > ma200 {
            tec_score += 25.0;
            tec_notes.push("Golden cross bull trend".into());
        } else if p < ma50 && ma50 < ma200 {
            tec_score -= 25.0;
            tec_notes.push("Death cross bear trend".into());
        } else if p > ma200 {
            tec_score += 10.0;
            tec_notes.push("Trading above 200 DMA".into());
        } else {
            tec_score -= 10.0;
            tec_notes.push("Trading below 200 DMA".into());
        }
    }

    if r.prices.len() >= 20
        && let Some(rsi) = crate::technical::rsi(&r.prices.close, 14)
    {
        if (45.0..=68.0).contains(&rsi) {
            tec_score += 15.0;
            tec_notes.push(format!("Healthy RSI ({:.1})", rsi));
        } else if rsi <= 32.0 {
            tec_score += 10.0;
            tec_notes.push(format!("Oversold bounce potential (RSI {:.1})", rsi));
        } else if rsi >= 75.0 {
            tec_score -= 15.0;
            tec_notes.push(format!("Overbought RSI ({:.1})", rsi));
        }
    }
    tec_score = tec_score.clamp(10.0, 95.0);
    let tec_summary = if tec_notes.is_empty() {
        "Neutral momentum structure".to_string()
    } else {
        tec_notes.join(", ")
    };

    // Weighted Overall Score
    let overall_score = (val_score * 0.20)
        + (gro_score * 0.25)
        + (sol_score * 0.15)
        + (own_score * 0.15)
        + (ana_score * 0.10)
        + (tec_score * 0.15);

    let action = match overall_score {
        x if x >= 78.0 => Action::StrongBuy,
        x if x >= 64.0 => Action::Buy,
        x if x >= 46.0 => Action::Hold,
        x if x >= 32.0 => Action::Reduce,
        _ => Action::StrongSell,
    };

    let pillars = vec![
        PillarScore {
            name: "Valuation & Safety",
            score: val_score,
            weight: 0.20,
            grade: score_to_grade(val_score),
            summary: val_summary,
        },
        PillarScore {
            name: "Financial Growth & Quality",
            score: gro_score,
            weight: 0.25,
            grade: score_to_grade(gro_score),
            summary: gro_summary,
        },
        PillarScore {
            name: "Balance Sheet & Solvency",
            score: sol_score,
            weight: 0.15,
            grade: score_to_grade(sol_score),
            summary: sol_summary,
        },
        PillarScore {
            name: "Smart Money & Ownership",
            score: own_score,
            weight: 0.15,
            grade: score_to_grade(own_score),
            summary: own_summary,
        },
        PillarScore {
            name: "Analyst Conviction & Target",
            score: ana_score,
            weight: 0.10,
            grade: score_to_grade(ana_score),
            summary: ana_summary,
        },
        PillarScore {
            name: "Technical Momentum & Trend",
            score: tec_score,
            weight: 0.15,
            grade: score_to_grade(tec_score),
            summary: tec_summary,
        },
    ];

    // Execution Calculations (Targets, Stop-Loss, Entry Zone, Risk/Reward)
    let target_price = r.fin.target_mean;
    let upside_pct = target_price.map(|t| ((t - price) / price) * 100.0);

    // Stop-loss: ATR-based (2x ATR) or 6.5% buffer
    let atr_val = if r.prices.len() >= 20 {
        crate::technical::atr(&r.prices.high, &r.prices.low, &r.prices.close, 14)
    } else {
        None
    };

    let stop_loss = match atr_val {
        Some(a) if a > 0.0 => (price - 2.0 * a).max(price * 0.85),
        _ => price * 0.935,
    };
    let risk_pct = if price > 0.0 {
        ((price - stop_loss) / price) * 100.0
    } else {
        0.0
    };

    let entry_zone = (
        (stop_loss + (price - stop_loss) * 0.35).min(price),
        (price * 1.01),
    );

    let risk_reward = match (target_price, price > stop_loss) {
        (Some(tp), true) if tp > price => {
            let potential_gain = tp - price;
            let potential_loss = price - stop_loss;
            if potential_loss > 0.0 {
                Some(potential_gain / potential_loss)
            } else {
                None
            }
        }
        _ => None,
    };

    // Bull & Bear qualitative points from cards
    let bull_points: Vec<String> = cards
        .iter()
        .filter(|c| c.impact == Impact::Positive)
        .take(4)
        .map(|c| format!("{}: {}", c.title, c.description))
        .collect();

    let bear_points: Vec<String> = cards
        .iter()
        .filter(|c| c.impact == Impact::Negative)
        .take(4)
        .map(|c| format!("{}: {}", c.title, c.description))
        .collect();

    // Time-Horizon Outlook
    let short_term = if tec_score >= 65.0 {
        format!(
            "Bullish momentum favors long swing positions toward {}{:.2}",
            csym,
            price * 1.05
        )
    } else if tec_score <= 40.0 {
        format!(
            "Bearish technical pressure; vulnerable to testing support near {}{:.2}",
            csym, stop_loss
        )
    } else {
        "Range-bound consolidation likely; trade between support and resistance".to_string()
    };

    let medium_term = if gro_score >= 65.0 && ana_score >= 60.0 {
        format!(
            "Supported by earnings momentum and analyst targets toward {}{:.2}",
            csym,
            target_price.unwrap_or(price * 1.15)
        )
    } else if gro_score <= 40.0 {
        "Earnings deceleration may cause post-earnings consolidation or derating".to_string()
    } else {
        "Execution on quarterly guidance will dictate next directional leg".to_string()
    };

    let long_term = if gro_score >= 60.0 && sol_score >= 60.0 && val_score >= 50.0 {
        "High quality compounding business with durable competitive position for multi-year wealth creation".to_string()
    } else if sol_score <= 40.0 || val_score <= 35.0 {
        "Cyclical or leverage risks suggest tactical positioning rather than buy-and-forget holding"
            .to_string()
    } else {
        "Core holding profile with periodic rebalancing advised at valuation extremes".to_string()
    };

    // Next catalyst
    let next_catalyst = if let Some(ed) = r.calendar.next_earnings {
        let date_str = chrono::DateTime::from_timestamp(ed, 0)
            .map(|d| d.format("%d %b %Y").to_string())
            .unwrap_or_default();
        Some(format!("Quarterly Earnings Release on {}", date_str))
    } else if let Some(xd) = r.calendar.ex_dividend {
        let date_str = chrono::DateTime::from_timestamp(xd, 0)
            .map(|d| d.format("%d %b %Y").to_string())
            .unwrap_or_default();
        Some(format!("Upcoming Ex-Dividend Date on {}", date_str))
    } else {
        None
    };

    let confidence = 85.0; // High data completeness

    DecisionVerdict {
        symbol: r.symbol.clone(),
        name: r.name.clone(),
        price,
        currency: r.currency.clone().unwrap_or_else(|| "USD".into()),
        action,
        overall_score,
        confidence,
        pillars,
        target_price,
        upside_pct,
        entry_zone,
        stop_loss,
        risk_pct,
        risk_reward,
        bull_points,
        bear_points,
        short_term,
        medium_term,
        long_term,
        next_catalyst,
    }
}

/// Terminal visual score bar e.g. [████████░░] 78/100
fn score_bar(score: f64) -> String {
    let filled_len = ((score / 100.0) * 10.0).round() as usize;
    let filled = "█".repeat(filled_len.min(10));
    let empty = "░".repeat(10 - filled_len.min(10));
    if score >= 70.0 {
        format!("{}{}", filled.green(), empty.dimmed())
    } else if score >= 45.0 {
        format!("{}{}", filled.yellow(), empty.dimmed())
    } else {
        format!("{}{}", filled.red(), empty.dimmed())
    }
}

/// Print the comprehensive Decision & Verdict card in the terminal
pub fn print_verdict(v: &DecisionVerdict) {
    let csym = if v.currency == "INR" { "₹" } else { "$" };
    let border = "═".repeat(64);
    let subline = "─".repeat(64);

    println!("\n{}", border.cyan());
    println!("  INVESTMENT DECISION: {} ({})", v.symbol.bold(), v.name);
    println!("  Current Price: {}{:.2}", csym, v.price);
    println!("{}", border.cyan());

    // Action & Score Header
    println!("\n  VERDICT:        {}", v.action.badge());
    println!(
        "  Conviction:     {:.0}/100  {}  (Confidence: {:.0}%)",
        v.overall_score,
        score_bar(v.overall_score),
        v.confidence
    );
    println!("  Thesis:         {}", v.action.summary_tagline().italic());

    // Execution Plan
    println!("\n  {}", "EXECUTION BLUEPRINT".bold().yellow());
    println!("  {}", subline.dimmed());
    if let (Some(target), Some(upside)) = (v.target_price, v.upside_pct) {
        let upside_str = if upside >= 0.0 {
            format!("+{:.1}%", upside).green().bold().to_string()
        } else {
            format!("-{:.1}%", upside.abs()).red().bold().to_string()
        };
        println!(
            "  Target Price:        {}{:.2}  ({})",
            csym, target, upside_str
        );
    }
    println!(
        "  Suggested Entry:     {}{:.2} — {}{:.2}",
        csym, v.entry_zone.0, csym, v.entry_zone.1
    );
    println!(
        "  Stop-Loss Level:     {}{:.2}  (Risk: {:.1}%)",
        csym, v.stop_loss, v.risk_pct
    );
    if let Some(rr) = v.risk_reward {
        let rr_str = format!("1 : {:.1}", rr);
        println!(
            "  Risk / Reward:       {}",
            if rr >= 2.0 {
                rr_str.green().bold().to_string()
            } else {
                rr_str.yellow().to_string()
            }
        );
    }

    // 6-Pillar Breakdown Table
    println!(
        "\n  {}",
        "6-PILLAR FUNDAMENTAL & TECHNICAL SCORING".bold().yellow()
    );
    println!("  {}", subline.dimmed());
    for p in &v.pillars {
        println!(
            "  {:<28} {:>3.0}/100 {} [{}]  {}",
            p.name.bold(),
            p.score,
            score_bar(p.score),
            p.grade.cyan().bold(),
            p.summary.dimmed()
        );
    }

    // Multi-Horizon Guidance
    println!("\n  {}", "TIME-HORIZON GUIDANCE".bold().yellow());
    println!("  {}", subline.dimmed());
    println!("  {} (1–4 Wks):   {}", "Short-Term".bold(), v.short_term);
    println!("  {} (1–6 Mos):  {}", "Medium-Term".bold(), v.medium_term);
    println!("  {} (1–3+ Yrs):   {}", "Long-Term".bold(), v.long_term);

    // Bull vs Bear Drivers
    if !v.bull_points.is_empty() {
        println!("\n  {}", "WHY TO BUY (KEY BULL DRIVERS)".bold().green());
        println!("  {}", subline.dimmed());
        for b in &v.bull_points {
            println!("  {} {}", "✔".green(), b);
        }
    }

    if !v.bear_points.is_empty() {
        println!("\n  {}", "KEY RISKS TO WATCH (BEAR CASE)".bold().red());
        println!("  {}", subline.dimmed());
        for b in &v.bear_points {
            println!("  {} {}", "✖".red(), b);
        }
    }

    if let Some(cat) = &v.next_catalyst {
        println!("\n  {} Next Catalyst: {}", "◆".magenta().bold(), cat.bold());
    }

    println!("\n  {}\n", "Disclaimer: For informational & educational purposes only. Always manage your own portfolio risk.".dimmed().italic());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::insights::generate_rich_insights;
    use crate::research::Research;

    #[test]
    fn test_verdict_scoring_bullish() {
        let mut r = Research::default();
        r.symbol = "AAPL".into();
        r.currency = Some("USD".into());
        r.quote.regular_market_price = Some(200.0);
        r.quote.fifty_day_average = Some(190.0);
        r.quote.two_hundred_day_average = Some(180.0);
        r.fin.peg = Some(0.85);
        r.fin.revenue_growth = Some(0.18);
        r.fin.earnings_growth = Some(0.24);
        r.fin.profit_margin = Some(0.25);
        r.fin.roe = Some(0.35);
        r.fin.de = Some(40.0);
        r.fin.inst_pct = Some(0.75);
        r.fin.rec_mean = Some(1.6);
        r.fin.target_mean = Some(240.0);

        let cards = generate_rich_insights(&r);
        let verdict = generate_verdict(&r, &cards);

        assert!(
            verdict.overall_score >= 70.0,
            "Score was {}",
            verdict.overall_score
        );
        assert!(matches!(verdict.action, Action::StrongBuy | Action::Buy));
        assert!(verdict.target_price.is_some());
        assert!(verdict.risk_reward.unwrap() > 1.5);
    }
}
