use crate::api::Quote;
use crate::market;
use crate::research::Research;
use colored::Colorize;

// ───────────────────────────────────────────────────────────────
// Rich Insight Cards (INDmoney style + quantitative upgrade)
// ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Impact {
    Positive,
    Negative,
    Neutral,
}

impl Impact {
    pub fn badge(&self) -> String {
        match self {
            Impact::Positive => "POSITIVE IMPACT".green().bold().to_string(),
            Impact::Negative => "NEGATIVE IMPACT".red().bold().to_string(),
            Impact::Neutral => "NEUTRAL IMPACT".yellow().bold().to_string(),
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Impact::Positive => "▲",
            Impact::Negative => "▼",
            Impact::Neutral => "●",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InsightCategory {
    Ownership,
    Growth,
    Profitability,
    Valuation,
    Health,
    Analysts,
    Technicals,
    Dividends,
    Newsflow,
}

impl InsightCategory {
    pub fn label(&self) -> &'static str {
        match self {
            InsightCategory::Ownership => "OWNERSHIP & SMART MONEY",
            InsightCategory::Growth => "FINANCIAL GROWTH",
            InsightCategory::Profitability => "PROFITABILITY & MARGINS",
            InsightCategory::Valuation => "VALUATION & SAFETY",
            InsightCategory::Health => "BALANCE SHEET & SOLVENCY",
            InsightCategory::Analysts => "ANALYST CONSENSUS",
            InsightCategory::Technicals => "PRICE & TECHNICAL TREND",
            InsightCategory::Dividends => "DIVIDENDS & RETURNS",
            InsightCategory::Newsflow => "CATALYSTS & NEWSFLOW",
        }
    }
}

#[derive(Clone, Debug)]
pub struct InsightCard {
    pub impact: Impact,
    pub category: InsightCategory,
    pub title: String,
    pub description: String,
    pub metric: Option<String>,
    pub importance: u8, // 1 to 5
}

/// Generate structured, multi-dimensional investment insight cards from Research data
pub fn generate_rich_insights(r: &Research) -> Vec<InsightCard> {
    let mut cards = Vec::new();
    let csym = r.csym();

    // ──────────────── 1. Ownership & Smart Money ────────────────
    // A) India: NSE Shareholding pattern (FII, DII, Promoter, Pledges)
    if r.holdings.len() >= 2 {
        let n = r.holdings.len();
        let h_curr = &r.holdings[n - 1];
        let h_prev = &r.holdings[n - 2];
        let q_curr_label = r.quarter_label(h_curr.date);

        // FII Holding
        if let (Some(fii_curr), Some(fii_prev)) = (h_curr.fii, h_prev.fii) {
            let diff = fii_curr - fii_prev;
            if diff >= 0.05 {
                cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Ownership,
                    title: "FII Holding Up".to_string(),
                    description: format!(
                        "Foreign Institutions have increased holdings from {:.2}% to {:.2}% in {} quarter",
                        fii_prev, fii_curr, q_curr_label
                    ),
                    metric: Some(format!("+{:.2}% QoQ", diff)),
                    importance: 4,
                });
            } else if diff <= -0.05 {
                cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Ownership,
                    title: "FII Trimming Stake".to_string(),
                    description: format!(
                        "Foreign Institutions have decreased holdings from {:.2}% to {:.2}% in {} quarter",
                        fii_prev, fii_curr, q_curr_label
                    ),
                    metric: Some(format!("-{:.2}% QoQ", diff.abs())),
                    importance: 3,
                });
            }
        }

        // DII / Mutual Fund Holding
        if let (Some(dii_curr), Some(dii_prev)) = (h_curr.dii, h_prev.dii) {
            let diff = dii_curr - dii_prev;
            if diff >= 0.10 {
                cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Ownership,
                    title: "DII Accumulation".to_string(),
                    description: format!(
                        "Domestic Institutions increased holdings from {:.2}% to {:.2}% in {} quarter",
                        dii_prev, dii_curr, q_curr_label
                    ),
                    metric: Some(format!("+{:.2}% QoQ", diff)),
                    importance: 3,
                });
            } else if diff <= -0.15 {
                cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Ownership,
                    title: "DII Selling Pressure".to_string(),
                    description: format!(
                        "Domestic Institutions reduced holdings from {:.2}% to {:.2}% in {} quarter",
                        dii_prev, dii_curr, q_curr_label
                    ),
                    metric: Some(format!("-{:.2}% QoQ", diff.abs())),
                    importance: 3,
                });
            }
        }

        // Promoter Pledging
        if let Some(pledged) = h_curr.pledged {
            if !pledged {
                cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Ownership,
                    title: "Zero Promoter Pledge".to_string(),
                    description: "Zero promoter shares are pledged or encumbered, removing debt covenant liquidation risk.".to_string(),
                    metric: Some("0% Pledged".to_string()),
                    importance: 4,
                });
            } else {
                cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Ownership,
                    title: "Promoter Shares Pledged".to_string(),
                    description: "A portion of promoter holdings is encumbered or pledged, posing potential collateral risk.".to_string(),
                    metric: Some("Encumbered".to_string()),
                    importance: 5,
                });
            }
        }
    }

    // B) US / Global: Institutional % and 6-month net buying
    if let Some(inst_pct) = r.fin.inst_pct
        && inst_pct > 0.70
    {
        cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Ownership,
                title: "Institutional Heavyweight".to_string(),
                description: format!(
                    "Institutions own {:.1}% of total shares, indicating deep smart-money confidence and liquidity",
                    inst_pct * 100.0
                ),
                metric: Some(format!("{:.1}% inst.", inst_pct * 100.0)),
                importance: 3,
            });
    }
    if let Some(net_buy) = r.fin.inst_net_buy_pct {
        if net_buy > 0.015 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Ownership,
                title: "Institutions Net Buyers".to_string(),
                description: format!(
                    "Institutions increased net positions by +{:.1}% over the last 6-month reporting window",
                    net_buy * 100.0
                ),
                metric: Some(format!("+{:.1}% 6M", net_buy * 100.0)),
                importance: 4,
            });
        } else if net_buy < -0.015 {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Ownership,
                title: "Institutions Net Sellers".to_string(),
                description: format!(
                    "Institutions reduced net positions by -{:.1}% over the last 6-month reporting window",
                    net_buy.abs() * 100.0
                ),
                metric: Some(format!("-{:.1}% 6M", net_buy.abs() * 100.0)),
                importance: 4,
            });
        }
    }

    // C) Insider Transactions
    let recent_buys: f64 = r
        .insiders
        .iter()
        .filter(|t| t.is_buy)
        .map(|t| t.value)
        .sum();
    let recent_sells: f64 = r
        .insiders
        .iter()
        .filter(|t| !t.is_buy)
        .map(|t| t.value)
        .sum();
    if recent_buys > 100_000.0 && recent_buys > recent_sells * 2.0 {
        cards.push(InsightCard {
            impact: Impact::Positive,
            category: InsightCategory::Ownership,
            title: "Insider Open-Market Buys".to_string(),
            description: format!(
                "Company insiders and key management purchased shares worth {} on the open market",
                crate::display::format_large_number(recent_buys, r.currency.as_deref())
            ),
            metric: Some("Insider Buying".to_string()),
            importance: 4,
        });
    } else if recent_sells > 5_000_000.0 && recent_buys < 50_000.0 {
        cards.push(InsightCard {
            impact: Impact::Negative,
            category: InsightCategory::Ownership,
            title: "Executive Insider Selling".to_string(),
            description: format!(
                "Executives sold shares worth {} on the open market over recent reporting cycles",
                crate::display::format_large_number(recent_sells, r.currency.as_deref())
            ),
            metric: Some("Insider Selling".to_string()),
            importance: 3,
        });
    }

    // D) Short Interest
    if let Some(short_float) = r.fin.short_pct_float
        && short_float > 0.08
    {
        cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Ownership,
                title: "Elevated Short Interest".to_string(),
                description: format!(
                    "Short interest stands at {:.1}% of float ({} days to cover), reflecting bearish institutional bets or squeeze risk",
                    short_float * 100.0,
                    r.fin.short_ratio.map(|r| format!("{:.1}", r)).unwrap_or_else(|| "N/A".into())
                ),
                metric: Some(format!("{:.1}% short", short_float * 100.0)),
                importance: 3,
            });
    }

    // ──────────────── 2. Financial Growth & Momentum ────────────────
    if r.quarters.len() >= 2 {
        let q_now = r.quarters.last().unwrap();
        let q_prev = &r.quarters[r.quarters.len() - 2];
        let yago_target = q_now.end - chrono::Duration::days(365);
        let q_yago = r
            .quarters
            .iter()
            .find(|p| (p.end - yago_target).num_days().abs() <= 25);

        // Revenue YoY & QoQ
        if let Some(rev_now) = q_now.revenue {
            if let Some(rev_yago) = q_yago.and_then(|p| p.revenue).filter(|v| *v > 0.0) {
                let yoy_growth = (rev_now / rev_yago) - 1.0;
                if yoy_growth >= 0.12 {
                    cards.push(InsightCard {
                        impact: Impact::Positive,
                        category: InsightCategory::Growth,
                        title: "Revenue Rich".to_string(),
                        description: format!(
                            "Quarterly revenue grew {:+.1}% YoY to {} in {}, sustaining top-line expansion",
                            yoy_growth * 100.0,
                            crate::display::format_large_number(rev_now, r.currency.as_deref()),
                            r.quarter_label(q_now.end)
                        ),
                        metric: Some(format!("{:+.1}% YoY", yoy_growth * 100.0)),
                        importance: 5,
                    });
                } else if yoy_growth <= -0.05 {
                    cards.push(InsightCard {
                        impact: Impact::Negative,
                        category: InsightCategory::Growth,
                        title: "Top-Line Contraction".to_string(),
                        description: format!(
                            "Quarterly revenue declined {:+.1}% YoY to {}, pointing to demand headwinds",
                            yoy_growth * 100.0,
                            crate::display::format_large_number(rev_now, r.currency.as_deref())
                        ),
                        metric: Some(format!("{:+.1}% YoY", yoy_growth * 100.0)),
                        importance: 4,
                    });
                }
            } else if let Some(rev_prev) = q_prev.revenue.filter(|v| *v > 0.0) {
                let qoq_growth = (rev_now / rev_prev) - 1.0;
                if qoq_growth >= 0.10 {
                    cards.push(InsightCard {
                        impact: Impact::Positive,
                        category: InsightCategory::Growth,
                        title: "Sequential Revenue Surge".to_string(),
                        description: format!(
                            "Revenue surged {:+.1}% quarter-over-quarter to {}",
                            qoq_growth * 100.0,
                            crate::display::format_large_number(rev_now, r.currency.as_deref())
                        ),
                        metric: Some(format!("{:+.1}% QoQ", qoq_growth * 100.0)),
                        importance: 4,
                    });
                }
            }
        }

        // Net Profit (PAT) YoY
        if let Some(pat_now) = q_now.net_income
            && let Some(pat_yago) = q_yago.and_then(|p| p.net_income).filter(|v| *v > 0.0)
        {
            let pat_yoy = (pat_now / pat_yago) - 1.0;
            if pat_yoy >= 0.15 {
                cards.push(InsightCard {
                        impact: Impact::Positive,
                        category: InsightCategory::Growth,
                        title: "Earnings Surge".to_string(),
                        description: format!(
                            "Net profit jumped {:+.1}% YoY to {} in {}, showcasing robust operational leverage",
                            pat_yoy * 100.0,
                            crate::display::format_large_number(pat_now, r.currency.as_deref()),
                            r.quarter_label(q_now.end)
                        ),
                        metric: Some(format!("{:+.1}% YoY", pat_yoy * 100.0)),
                        importance: 5,
                    });
            } else if pat_yoy <= -0.12 {
                cards.push(InsightCard {
                        impact: Impact::Negative,
                        category: InsightCategory::Growth,
                        title: "Earnings Contraction".to_string(),
                        description: format!(
                            "Net profit fell {:+.1}% YoY to {} in {}, pressured by cost inflation or provisions",
                            pat_yoy * 100.0,
                            crate::display::format_large_number(pat_now, r.currency.as_deref()),
                            r.quarter_label(q_now.end)
                        ),
                        metric: Some(format!("{:+.1}% YoY", pat_yoy * 100.0)),
                        importance: 5,
                    });
            }
        }

        // Two consecutive quarters of revenue growth
        if r.quarters.len() >= 3 {
            let q_2ago = &r.quarters[r.quarters.len() - 3];
            if let (Some(r0), Some(r1), Some(r2)) = (q_2ago.revenue, q_prev.revenue, q_now.revenue)
                && r2 > r1
                && r1 > r0
            {
                let avg_inc = (((r2 - r0) / r0) / 2.0) * 100.0;
                cards.push(InsightCard {
                        impact: Impact::Positive,
                        category: InsightCategory::Growth,
                        title: "Consistent Revenue Growth".to_string(),
                        description: format!(
                            "Revenue is up for the last 2 consecutive quarters, expanding from {} → {} with an average increase of {:.1}% per quarter",
                            crate::display::format_large_number(r0, r.currency.as_deref()),
                            crate::display::format_large_number(r2, r.currency.as_deref()),
                            avg_inc
                        ),
                        metric: Some(format!("+{:.1}%/qtr", avg_inc)),
                        importance: 4,
                    });
            }
        }
    }

    // Surprises: Track record of beating estimates
    if !r.surprises.is_empty() {
        let n_surprises = r.surprises.len();
        let beats = r.surprises.iter().filter(|s| s.surprise_pct > 0.01).count();
        if n_surprises >= 3 && beats as f64 / n_surprises as f64 >= 0.75 {
            let avg_surprise: f64 =
                r.surprises.iter().map(|s| s.surprise_pct).sum::<f64>() / n_surprises as f64;
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Growth,
                title: "Consistent Earnings Outperformer".to_string(),
                description: format!(
                    "Beat Wall Street/analyst EPS estimates in {} of the last {} reported quarters (average surprise {:+.1}%)",
                    beats, n_surprises, avg_surprise * 100.0
                ),
                metric: Some(format!("{}/{} Beats", beats, n_surprises)),
                importance: 4,
            });
        } else if let Some(last_s) = r.surprises.last()
            && last_s.surprise_pct < -0.03
        {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Growth,
                title: "Quarterly Earnings Miss".to_string(),
                description: format!(
                    "Missed consensus EPS estimates by {:+.1}% in {} (Actual: {:.2} vs Est: {:.2})",
                    last_s.surprise_pct * 100.0,
                    r.quarter_label(last_s.quarter),
                    last_s.actual,
                    last_s.estimate
                ),
                metric: Some(format!("{:+.1}% surprise", last_s.surprise_pct * 100.0)),
                importance: 4,
            });
        }
    }

    // ──────────────── 3. Profitability & Margins ────────────────
    if let Some(pm) = r.fin.profit_margin {
        if pm >= 0.18 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Profitability,
                title: "High Net Margin".to_string(),
                description: format!(
                    "Healthy net profit margin of {:.1}%, signifying formidable pricing power and operational efficiency",
                    pm * 100.0
                ),
                metric: Some(format!("{:.1}% margin", pm * 100.0)),
                importance: 4,
            });
        } else if pm < 0.0 && !r.is_financial() {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Profitability,
                title: "Operating at a Net Loss".to_string(),
                description: format!(
                    "Net profit margin is negative ({:.1}%) — company is currently burning cash on the bottom line",
                    pm * 100.0
                ),
                metric: Some(format!("{:.1}% margin", pm * 100.0)),
                importance: 5,
            });
        }
    }

    // Margin Trajectory YoY
    if r.quarters.len() >= 2 {
        let q_now = r.quarters.last().unwrap();
        let yago_target = q_now.end - chrono::Duration::days(365);
        if let Some(q_yago) = r
            .quarters
            .iter()
            .find(|p| (p.end - yago_target).num_days().abs() <= 25)
            && let (Some(m_now), Some(m_yago)) = (q_now.net_margin(), q_yago.net_margin())
        {
            let bps_diff = (m_now - m_yago) * 10_000.0;
            if bps_diff >= 120.0 {
                cards.push(InsightCard {
                        impact: Impact::Positive,
                        category: InsightCategory::Profitability,
                        title: "Margin Expansion".to_string(),
                        description: format!(
                            "Net margin expanded by {:+.0} bps YoY to {:.1}%, reflecting higher efficiency and price realization",
                            bps_diff, m_now * 100.0
                        ),
                        metric: Some(format!("{:+.0} bps", bps_diff)),
                        importance: 4,
                    });
            } else if bps_diff <= -120.0 {
                cards.push(InsightCard {
                        impact: Impact::Negative,
                        category: InsightCategory::Profitability,
                        title: "Margin Pressure".to_string(),
                        description: format!(
                            "Net margin contracted by {:+.0} bps YoY to {:.1}%, squeezed by cost inflation or competitive pressures",
                            bps_diff.abs(), m_now * 100.0
                        ),
                        metric: Some(format!("-{:.0} bps", bps_diff.abs())),
                        importance: 4,
                    });
            }
        }
    }

    // Return on Equity
    if let Some(roe) = r.fin.roe {
        if roe >= 0.20 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Profitability,
                title: "Outstanding ROE".to_string(),
                description: format!(
                    "Return on Equity of {:.1}% highlights superior capital compounding and shareholder wealth creation",
                    roe * 100.0
                ),
                metric: Some(format!("{:.1}% ROE", roe * 100.0)),
                importance: 4,
            });
        } else if roe < 0.06 && !r.is_financial() {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Profitability,
                title: "Subdued Capital Productivity".to_string(),
                description: format!(
                    "Return on Equity of {:.1}% trails capital costs and broader peer group returns",
                    roe * 100.0
                ),
                metric: Some(format!("{:.1}% ROE", roe * 100.0)),
                importance: 3,
            });
        }
    }

    // ──────────────── 4. Valuation & Margin of Safety ────────────────
    // PEG Ratio
    if let Some(peg) = r.fin.peg {
        if peg > 0.0 && peg <= 1.1 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Valuation,
                title: "Attractive PEG Ratio".to_string(),
                description: format!(
                    "PEG ratio of {:.2}x indicates the stock is attractively priced relative to its projected earnings growth",
                    peg
                ),
                metric: Some(format!("{:.2}x PEG", peg)),
                importance: 4,
            });
        } else if peg >= 2.5 {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Valuation,
                title: "Elevated PEG Ratio".to_string(),
                description: format!(
                    "PEG ratio of {:.2}x suggests the market is pricing in aggressive growth assumptions with limited safety margin",
                    peg
                ),
                metric: Some(format!("{:.2}x PEG", peg)),
                importance: 3,
            });
        }
    }

    // Trailing vs Forward P/E
    if let (Some(t_pe), Some(f_pe)) = (r.fin.trailing_pe, r.fin.forward_pe)
        && t_pe > 0.0
        && f_pe > 0.0
    {
        let compression = (t_pe - f_pe) / t_pe;
        if compression >= 0.18 {
            cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Valuation,
                    title: "Forward P/E Rebound".to_string(),
                    description: format!(
                        "Forward P/E ({:.1}x) is {:.0}% lower than trailing P/E ({:.1}x), reflecting analyst expectations of strong earnings recovery",
                        f_pe, compression * 100.0, t_pe
                    ),
                    metric: Some(format!("{:.1}x vs {:.1}x", f_pe, t_pe)),
                    importance: 4,
                });
        } else if compression <= -0.15 {
            cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Valuation,
                    title: "Forward P/E Inflation".to_string(),
                    description: format!(
                        "Forward P/E ({:.1}x) is higher than trailing ({:.1}x), signaling consensus expectation of earnings deceleration",
                        f_pe, t_pe
                    ),
                    metric: Some(format!("{:.1}x vs {:.1}x", f_pe, t_pe)),
                    importance: 3,
                });
        }
    }

    // Absolute P/E extremes
    if let Some(pe) = r.fin.trailing_pe {
        if pe > 0.0 && pe < 12.0 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Valuation,
                title: "Deep Value P/E Multiple".to_string(),
                description: format!(
                    "Trailing P/E of {:.1}x is in deep value territory, providing a substantial margin of safety",
                    pe
                ),
                metric: Some(format!("{:.1}x P/E", pe)),
                importance: 4,
            });
        } else if pe > 55.0 {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Valuation,
                title: "Priced for Perfection".to_string(),
                description: format!(
                    "Trailing P/E of {:.1}x commands a steep valuation premium, leaving negligible room for quarterly misses",
                    pe
                ),
                metric: Some(format!("{:.1}x P/E", pe)),
                importance: 4,
            });
        }
    }

    // ──────────────── 5. Balance Sheet & Solvency ────────────────
    if !r.is_financial() {
        if let Some(de) = r.fin.de {
            if de <= 30.0 {
                cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Health,
                    title: "Low Debt Burden".to_string(),
                    description: format!(
                        "Debt-to-Equity ratio of {:.1}% highlights a pristine, conservative balance sheet with minimal solvency risk",
                        de
                    ),
                    metric: Some(format!("{:.1}% D/E", de)),
                    importance: 4,
                });
            } else if de >= 160.0 {
                cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Health,
                    title: "High Financial Leverage".to_string(),
                    description: format!(
                        "Debt-to-Equity of {:.1}% introduces elevated interest sensitivity and financial leverage risk",
                        de
                    ),
                    metric: Some(format!("{:.1}% D/E", de)),
                    importance: 4,
                });
            }
        }

        if let Some(cr) = r.fin.current_ratio {
            if cr >= 1.6 {
                cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Health,
                    title: "Healthy Working Capital".to_string(),
                    description: format!(
                        "Current ratio of {:.2}x indicates robust liquid buffer to cover short-term liabilities",
                        cr
                    ),
                    metric: Some(format!("{:.2}x CR", cr)),
                    importance: 3,
                });
            } else if cr < 1.0 {
                cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Health,
                    title: "Working Capital Deficit".to_string(),
                    description: format!(
                        "Current ratio of {:.2}x means short-term obligations exceed liquid assets",
                        cr
                    ),
                    metric: Some(format!("{:.2}x CR", cr)),
                    importance: 4,
                });
            }
        }
    }

    // Cash Reserves vs Price
    if let (Some(cash), Some(price)) = (r.fin.total_cash, r.price())
        && let Some(shares) = r.fin.shares
        && shares > 0.0
        && price > 0.0
    {
        let cash_per_share = cash / shares;
        let cash_pct = cash_per_share / price;
        if cash_pct >= 0.15 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Health,
                title: "Cash-Rich Balance Sheet".to_string(),
                description: format!(
                    "Cash reserves of {} equate to {}{:.2}/share ({:.1}% of current stock price)",
                    crate::display::format_large_number(cash, r.currency.as_deref()),
                    csym,
                    cash_per_share,
                    cash_pct * 100.0
                ),
                metric: Some(format!("{:.1}% cash/mktcap", cash_pct * 100.0)),
                importance: 4,
            });
        }
    }

    // ──────────────── 6. Analyst Consensus & Catalysts ────────────────
    if let (Some(target), Some(price)) = (r.fin.target_mean, r.price())
        && price > 0.0
    {
        let upside = (target - price) / price;
        if upside >= 0.16 {
            cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Analysts,
                    title: "Substantial Target Upside".to_string(),
                    description: format!(
                        "Consensus analyst price target of {}{:.2} implies {:+.1}% upside potential from current levels",
                        csym, target, upside * 100.0
                    ),
                    metric: Some(format!("{:+.1}% Upside", upside * 100.0)),
                    importance: 5,
                });
        } else if upside <= -0.06 {
            cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Analysts,
                    title: "Trading Above Price Target".to_string(),
                    description: format!(
                        "Current price trades {:+.1}% above mean analyst target ({}{:.2}), indicating limited upside room",
                        upside.abs() * 100.0, csym, target
                    ),
                    metric: Some(format!("-{:.1}% vs Target", upside.abs() * 100.0)),
                    importance: 3,
                });
        }
    }

    if let Some(rec) = r.fin.rec_mean {
        if rec <= 1.75 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Analysts,
                title: "Overwhelming Buy Consensus".to_string(),
                description: format!(
                    "Analyst rating score is {:.2}/5 ({}) backed by {} Wall St / Dalal St opinions",
                    rec,
                    r.fin.rec_key.as_deref().unwrap_or("Strong Buy"),
                    r.fin.n_analysts.unwrap_or(0.0) as u32
                ),
                metric: Some("Strong Buy".to_string()),
                importance: 4,
            });
        } else if rec >= 3.6 {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Analysts,
                title: "Cautious Analyst Sentiment".to_string(),
                description: format!(
                    "Consensus rating stands at {:.2}/5 ({}) with multiple analysts in Hold or Underperform territory",
                    rec, r.fin.rec_key.as_deref().unwrap_or("Underperform")
                ),
                metric: Some("Bearish Stance".to_string()),
                importance: 4,
            });
        }
    }

    // ──────────────── 7. Price Action & Technical Trend ────────────────
    if let (Some(price), Some(ma50), Some(ma200)) = (
        r.price(),
        r.quote.fifty_day_average,
        r.quote.two_hundred_day_average,
    ) {
        if price > ma50 && ma50 > ma200 {
            cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Technicals,
                title: "Golden Cross Uptrend".to_string(),
                description: format!(
                    "Price ({}{:.2}) trades cleanly above both 50-day ({}{:.2}) and 200-day ({}{:.2}) moving averages in a classic bull configuration",
                    csym, price, csym, ma50, csym, ma200
                ),
                metric: Some("Bull Trend".to_string()),
                importance: 4,
            });
        } else if price < ma50 && ma50 < ma200 {
            cards.push(InsightCard {
                impact: Impact::Negative,
                category: InsightCategory::Technicals,
                title: "Death Cross Downtrend".to_string(),
                description: format!(
                    "Price ({}{:.2}) trades below both 50-day ({}{:.2}) and 200-day ({}{:.2}) moving averages — prevailing bear phase",
                    csym, price, csym, ma50, csym, ma200
                ),
                metric: Some("Bear Trend".to_string()),
                importance: 4,
            });
        }
    }

    // RSI Momentum
    if r.prices.len() >= 20
        && let Some(rsi_val) = crate::technical::rsi(&r.prices.close, 14)
    {
        if rsi_val <= 32.0 {
            cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Technicals,
                    title: "Oversold Bounce Candidate".to_string(),
                    description: format!(
                        "14-day RSI is oversold at {:.1} — price is stretched to the downside, creating favorable mean-reversion conditions",
                        rsi_val
                    ),
                    metric: Some(format!("RSI {:.1}", rsi_val)),
                    importance: 4,
                });
        } else if rsi_val >= 74.0 {
            cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Technicals,
                    title: "Overbought Technical Territory".to_string(),
                    description: format!(
                        "14-day RSI has reached overbought levels at {:.1} — prone to short-term consolidation or profit booking",
                        rsi_val
                    ),
                    metric: Some(format!("RSI {:.1}", rsi_val)),
                    importance: 3,
                });
        }
    }

    // 52-Week Range Position
    if let (Some(price), Some(hi), Some(lo)) = (
        r.price(),
        r.quote.fifty_two_week_high,
        r.quote.fifty_two_week_low,
    ) {
        let range = hi - lo;
        if range > 0.0 {
            let pos = (price - lo) / range;
            if pos >= 0.90 {
                cards.push(InsightCard {
                    impact: Impact::Positive,
                    category: InsightCategory::Technicals,
                    title: "Momentum Near 52-Week High".to_string(),
                    description: format!(
                        "Trading within {:.0}% of its 52-week high ({}{:.2}) — leadership relative strength",
                        (1.0 - pos) * 100.0, csym, hi
                    ),
                    metric: Some(format!("{:.0}% of 52W range", pos * 100.0)),
                    importance: 3,
                });
            } else if pos <= 0.10 {
                cards.push(InsightCard {
                    impact: Impact::Negative,
                    category: InsightCategory::Technicals,
                    title: "Testing 52-Week Lows".to_string(),
                    description: format!(
                        "Trading near its 52-week low ({}{:.2}) — potential value entry if support holds, but currently lacking momentum",
                        csym, lo
                    ),
                    metric: Some(format!("{:.0}% of 52W range", pos * 100.0)),
                    importance: 3,
                });
            }
        }
    }

    // ──────────────── 8. Dividends ────────────────
    if let Some(dy) = r.fin.div_yield
        && dy >= 0.025
    {
        cards.push(InsightCard {
                impact: Impact::Positive,
                category: InsightCategory::Dividends,
                title: "Attractive Dividend Yield".to_string(),
                description: format!(
                    "Offers a solid annual dividend yield of {:.2}%, enhancing total shareholder returns",
                    dy * 100.0
                ),
                metric: Some(format!("{:.2}% Yield", dy * 100.0)),
                importance: 3,
            });
    }

    // Sort cards: Positive & Negative highest importance first
    cards.sort_by_key(|a| std::cmp::Reverse(a.importance));
    cards
}

/// Print formatted INDmoney-style insight cards with terminal styling
pub fn print_insight_cards(cards: &[InsightCard], limit: Option<usize>) {
    if cards.is_empty() {
        println!(
            "  {}",
            "No conclusive insight cards generated from available data.".dimmed()
        );
        return;
    }

    let positive_cnt = cards
        .iter()
        .filter(|c| c.impact == Impact::Positive)
        .count();
    let negative_cnt = cards
        .iter()
        .filter(|c| c.impact == Impact::Negative)
        .count();
    let neutral_cnt = cards.iter().filter(|c| c.impact == Impact::Neutral).count();

    println!(
        "  {} Insights found: {} · {} · {}\n",
        cards.len(),
        format!("{} Positive", positive_cnt).green().bold(),
        format!("{} Negative", negative_cnt).red().bold(),
        format!("{} Neutral", neutral_cnt).yellow()
    );

    let display_cards: &[InsightCard] = match limit {
        Some(l) => &cards[..l.min(cards.len())],
        None => cards,
    };

    for card in display_cards {
        println!(
            "  {}  {}  {}",
            card.impact.badge(),
            format!("[{}]", card.category.label()).cyan().dimmed(),
            card.metric
                .as_ref()
                .map(|m| format!("({})", m))
                .unwrap_or_default()
                .bold()
        );
        println!("  {}", card.title.bold());
        println!("  {}\n", card.description.dimmed());
    }
}

// ───────────────────────────────────────────────────────────────
// Backward-compatible Quote-based insights
// ───────────────────────────────────────────────────────────────

/// Generate investment insights based on available Quote data (classic)
pub fn generate_insights(q: &Quote) -> Vec<String> {
    let mut insights = Vec::new();

    // Valuation insights
    if let Some(pe) = q.trailing_pe {
        if pe < 0.0 {
            insights.push(
                "Company is currently unprofitable (negative P/E)"
                    .red()
                    .to_string(),
            );
        } else if pe < 10.0 {
            insights.push(format!(
                "P/E of {:.1} is very low — could be undervalued or facing challenges",
                pe
            ));
        } else if pe < 20.0 {
            insights.push(format!("P/E of {:.1} suggests reasonable valuation", pe));
        } else if pe < 35.0 {
            insights.push(format!(
                "P/E of {:.1} indicates growth expectations priced in",
                pe
            ));
        } else {
            insights.push(
                format!(
                    "P/E of {:.1} is very high — stock is priced for aggressive growth",
                    pe
                )
                .yellow()
                .to_string(),
            );
        }
    }

    // Forward vs trailing PE
    if let (Some(trailing), Some(forward)) = (q.trailing_pe, q.forward_pe)
        && trailing > 0.0
        && forward > 0.0
    {
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

    // Price vs moving averages
    if let (Some(price), Some(ma50), Some(ma200)) = (
        q.regular_market_price,
        q.fifty_day_average,
        q.two_hundred_day_average,
    ) {
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
            insights.push(
                "Price between MAs — possible short-term pullback in longer uptrend".to_string(),
            );
        } else if price < ma200 && price > ma50 {
            insights.push(
                "Short-term recovery but still in longer-term downtrend"
                    .yellow()
                    .to_string(),
            );
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
    if let (Some(price), Some(high), Some(low)) = (
        q.regular_market_price,
        q.fifty_two_week_high,
        q.fifty_two_week_low,
    ) {
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
    if let Some(div_yield) = q.trailing_annual_dividend_yield
        && div_yield > 0.0
    {
        let pct = div_yield * 100.0;
        if pct > 5.0 {
            insights.push(
                format!(
                    "High dividend yield of {:.2}% — check if it's sustainable",
                    pct
                )
                .yellow()
                .to_string(),
            );
        } else if pct > 2.0 {
            insights.push(
                format!("Solid dividend yield of {:.2}%", pct)
                    .green()
                    .to_string(),
            );
        } else {
            insights.push(format!("Modest dividend yield of {:.2}%", pct).to_string());
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
            insights.push(
                format!("Excellent profit margin of {:.1}%", pct)
                    .green()
                    .to_string(),
            );
        } else if pct < 0.0 {
            insights.push(
                format!(
                    "Negative profit margin ({:.1}%) — company is losing money",
                    pct
                )
                .red()
                .to_string(),
            );
        }
    }

    // Debt analysis
    if let Some(de) = q.debt_to_equity {
        if de > 200.0 {
            insights.push(
                format!(
                    "Very high debt-to-equity ratio of {:.0} — significant leverage risk",
                    de
                )
                .red()
                .to_string(),
            );
        } else if de < 30.0 {
            insights.push(
                format!(
                    "Low debt-to-equity of {:.0} — conservative balance sheet",
                    de
                )
                .green()
                .to_string(),
            );
        }
    }

    // Beta / volatility
    if let Some(beta) = q.beta {
        if beta > 1.5 {
            insights.push(
                format!(
                    "High beta of {:.2} — stock is significantly more volatile than the market",
                    beta
                )
                .yellow()
                .to_string(),
            );
        } else if beta < 0.5 {
            insights.push(
                format!(
                    "Low beta of {:.2} — defensive stock, less volatile than market",
                    beta
                )
                .to_string(),
            );
        }
    }

    // Analyst consensus
    if let (Some(rec), Some(target)) = (q.recommendation_mean, q.target_mean_price)
        && let Some(price) = q.regular_market_price
    {
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

    if insights.is_empty() {
        insights.push(
            "Limited data available for generating insights"
                .dimmed()
                .to_string(),
        );
    }

    insights
}

/// Generate an overall verdict based on Quote data (classic)
pub fn overall_verdict(q: &Quote) -> String {
    let mut score: i32 = 0;
    let mut factors = 0;

    // P/E valuation
    if let Some(pe) = q.trailing_pe {
        factors += 1;
        if pe > 0.0 && pe < 25.0 {
            score += 1;
        } else if !(0.0..=40.0).contains(&pe) {
            score -= 1;
        }
    }

    // Trend
    if let (Some(price), Some(ma50), Some(ma200)) = (
        q.regular_market_price,
        q.fifty_day_average,
        q.two_hundred_day_average,
    ) {
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
    use crate::research::{Holding, Period, Research};
    use chrono::NaiveDate;

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
        q.trailing_pe = Some(8.0);
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
        q.trailing_pe = Some(50.0);
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
        q.trailing_annual_dividend_yield = Some(0.07);
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("dividend") || joined.contains("yield"));
    }

    #[test]
    fn test_generate_insights_no_data() {
        let q = Quote::default();
        let insights = generate_insights(&q);
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
        q.earnings_quarterly_growth = Some(0.30);
        let insights = generate_insights(&q);
        let joined = insights.join(" ");
        assert!(joined.contains("earnings") || joined.contains("growth"));
    }

    #[test]
    fn test_overall_verdict_bullish() {
        let q = base_quote();
        let verdict = overall_verdict(&q);
        assert!(verdict.to_lowercase().contains("bullish"));
    }

    #[test]
    fn test_overall_verdict_bearish() {
        let mut q = Quote::default();
        q.trailing_pe = Some(-5.0);
        q.regular_market_price = Some(80.0);
        q.fifty_day_average = Some(90.0);
        q.two_hundred_day_average = Some(95.0);
        q.earnings_quarterly_growth = Some(-0.20);
        q.profit_margins = Some(-0.05);
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
        assert!(verdict.contains('/'));
    }

    #[test]
    fn test_rich_insights_fii_and_revenue() {
        let mut r = Research::default();
        r.symbol = "LT.NS".into();
        r.is_india = true;
        r.currency = Some("INR".into());
        r.quote.regular_market_price = Some(3800.0);
        r.quote.fifty_day_average = Some(3600.0);
        r.quote.two_hundred_day_average = Some(3400.0);
        r.fin.target_mean = Some(4500.0);
        r.fin.rec_mean = Some(1.6);
        r.fin.profit_margin = Some(0.06);

        // Holdings
        r.holdings = vec![
            Holding {
                date: NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
                fii: Some(18.96),
                dii: Some(41.0),
                pledged: Some(false),
                ..Default::default()
            },
            Holding {
                date: NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(),
                fii: Some(19.21),
                dii: Some(42.86),
                pledged: Some(false),
                ..Default::default()
            },
        ];

        // Quarters
        r.quarters = vec![
            Period {
                end: NaiveDate::from_ymd_opt(2025, 9, 30).unwrap(),
                revenue: Some(67_000_000_000.0),
                net_income: Some(3_900_000_000.0),
                ..Default::default()
            },
            Period {
                end: NaiveDate::from_ymd_opt(2026, 9, 30).unwrap(),
                revenue: Some(78_750_000_000.0),
                net_income: Some(5_202_000_000.0),
                ..Default::default()
            },
        ];

        let cards = generate_rich_insights(&r);
        assert!(!cards.is_empty());
        let titles: Vec<_> = cards.iter().map(|c| c.title.as_str()).collect();
        assert!(
            titles.contains(&"FII Holding Up"),
            "missing FII card: {:?}",
            titles
        );
        assert!(
            titles.contains(&"Revenue Rich") || titles.contains(&"Earnings Surge"),
            "{:?}",
            titles
        );
        assert!(titles.contains(&"Zero Promoter Pledge"), "{:?}", titles);
        assert!(
            titles.contains(&"Substantial Target Upside"),
            "{:?}",
            titles
        );
    }
}
