//! Transparent accounting scorecards: Piotroski 9-Point F-Score and Altman Z-Score
//! with audit-grade formulas, pass/fail signals, and component breakdowns.

use crate::display::*;
use crate::research::Research;
use colored::Colorize;
use serde::{Deserialize, Serialize};

/// Piotroski F-Score financial health rating.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PiotroskiRating {
    /// Score 8–9: Superior balance sheet quality, low distress risk.
    VeryStrong,
    /// Score 5–7: Average / stable fundamental performance.
    Moderate,
    /// Score 0–4: Weak fundamentals, deteriorating liquidity or earnings.
    Weak,
}

impl PiotroskiRating {
    pub fn label(&self) -> &'static str {
        match self {
            Self::VeryStrong => "Very Strong",
            Self::Moderate => "Moderate / Stable",
            Self::Weak => "Weak / High Risk",
        }
    }

    pub fn badge(&self) -> String {
        match self {
            Self::VeryStrong => self.label().green().bold().to_string(),
            Self::Moderate => self.label().yellow().bold().to_string(),
            Self::Weak => self.label().red().bold().to_string(),
        }
    }
}

/// A single transparent criterion of the Piotroski 9-point F-Score.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PiotroskiCriterion {
    pub id: String,
    pub category: String,
    pub name: String,
    pub formula: String,
    pub formatted_value: String,
    pub passed: bool,
    pub points: u8,
    pub explanation: String,
    /// False when the data to run the test is missing: the signal is "n/a",
    /// earns no point and is left out of the evaluated count.
    pub evaluated: bool,
}

/// Complete Piotroski 9-Point Scorecard breakdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PiotroskiScorecard {
    /// Points earned by the signals that could be evaluated.
    pub score: u8,
    pub max_score: u8,
    /// How many of the nine signals had the data to be evaluated.
    pub evaluated: u8,
    /// Only set when all nine signals were evaluated; a partial score is not
    /// comparable with a full one.
    pub rating: Option<PiotroskiRating>,
    pub rating_label: String,
    pub profitability_score: u8,
    pub leverage_score: u8,
    pub efficiency_score: u8,
    pub criteria: Vec<PiotroskiCriterion>,
}

/// Altman Z-Score discrimination zones for bankruptcy / distress prediction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AltmanZone {
    /// Z > 2.99: Financially safe, minimal bankruptcy risk.
    Safe,
    /// 1.81 <= Z <= 2.99: Grey zone, moderate vulnerability / caution.
    Grey,
    /// Z < 1.81: Distress zone, high risk of insolvency within 2 years.
    Distress,
}

impl AltmanZone {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Safe => "Safe Zone (Low Default Risk)",
            Self::Grey => "Grey Zone (Moderate Caution)",
            Self::Distress => "Distress Zone (High Default Risk)",
        }
    }

    pub fn badge(&self) -> String {
        match self {
            Self::Safe => self.label().green().bold().to_string(),
            Self::Grey => self.label().yellow().bold().to_string(),
            Self::Distress => self.label().red().bold().to_string(),
        }
    }
}

/// Component factor of the Altman Z-score discriminant model.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AltmanComponent {
    pub factor: String,
    pub name: String,
    pub formula: String,
    pub weight: f64,
    pub raw_value: Option<f64>,
    pub weighted_value: Option<f64>,
    pub formatted_value: String,
}

/// Complete Altman Z-Score model breakdown.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AltmanScorecard {
    pub z_score: Option<f64>,
    pub zone: Option<AltmanZone>,
    pub zone_label: Option<String>,
    pub formula_str: String,
    pub is_financial: bool,
    pub note: Option<String>,
    /// Classic-model factors with no value, for example `["X1", "X5"]`.
    pub missing_factors: Vec<String>,
    pub components: Vec<AltmanComponent>,
    pub z_double_prime: Option<f64>,
    pub z_double_prime_zone: Option<AltmanZone>,
    /// The Z'' model's own factors. X4 here is book equity / total
    /// liabilities, unlike the classic model's market-cap X4 in `components`.
    pub z_double_prime_components: Vec<AltmanComponent>,
    pub z_double_prime_formula_str: String,
}

/// Combined transparent report holding both accounting scorecards.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ScorecardReport {
    pub symbol: String,
    pub name: String,
    pub currency: Option<String>,
    pub price: Option<f64>,
    pub market_cap: Option<f64>,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub piotroski: PiotroskiScorecard,
    pub altman: AltmanScorecard,
    pub evaluation_date: chrono::NaiveDate,
}

/// Visual horizontal score bar e.g. [████████░░] 8/9
fn score_bar(score: u8, max: u8) -> String {
    let filled_len = (((score as f64) / (max as f64)) * 10.0).round() as usize;
    let filled = "█".repeat(filled_len.min(10));
    let empty = "░".repeat(10 - filled_len.min(10));
    if score >= 8 {
        format!("{}{}", filled.green(), empty.dimmed())
    } else if score >= 5 {
        format!("{}{}", filled.yellow(), empty.dimmed())
    } else {
        format!("{}{}", filled.red(), empty.dimmed())
    }
}

fn make_criterion(
    id: &str,
    category: &str,
    name: &str,
    formula: &str,
    formatted_value: String,
    outcome: Option<bool>,
    explanation: &str,
) -> PiotroskiCriterion {
    let passed = outcome == Some(true);
    PiotroskiCriterion {
        id: id.to_string(),
        category: category.to_string(),
        name: name.to_string(),
        formula: formula.to_string(),
        formatted_value,
        passed,
        points: u8::from(passed),
        explanation: explanation.to_string(),
        evaluated: outcome.is_some(),
    }
}

fn make_altman_component(
    factor: &str,
    name: &str,
    formula: &str,
    weight: f64,
    raw_value: Option<f64>,
    formatted_value: String,
) -> AltmanComponent {
    AltmanComponent {
        factor: factor.to_string(),
        name: name.to_string(),
        formula: formula.to_string(),
        weight,
        raw_value,
        weighted_value: raw_value.map(|v| v * weight),
        formatted_value,
    }
}

/// Net income for the first Piotroski signal when no period reports it.
///
/// Net income = trailing EPS x shares outstanding, or, failing that, profit
/// margin x revenue (annual revenue, or the sum of the last four quarters).
/// Margin x *market cap* is never used: it is off by the price-to-sales ratio
/// (#68). `None` leaves the signal unscored.
fn estimate_net_income(r: &Research) -> Option<f64> {
    if let (Some(eps), Some(shares)) = (r.fin.trailing_eps, r.fin.shares)
        && shares > 0.0
    {
        return Some(eps * shares);
    }
    let revenue = r.annual.last().and_then(|p| p.revenue).or_else(|| {
        let q: Vec<f64> = r
            .quarters
            .iter()
            .rev()
            .take(4)
            .filter_map(|p| p.revenue)
            .collect();
        (q.len() == 4).then(|| q.iter().sum())
    })?;
    r.fin.profit_margin.map(|pm| pm * revenue)
}

/// Calculate the Piotroski 9-Point F-Score from corporate research data.
///
/// Piotroski (2000), "Value Investing: The Use of Historical Financial
/// Statement Information to Separate Winners from Losers": nine binary
/// signals, F_SCORE = sum of ROA, CFO, ΔROA, ACCRUAL, ΔLEVER, ΔLIQUID, EQ_OFFER,
/// ΔMARGIN and ΔTURN. A signal whose inputs are missing is n/a: it earns no
/// point and is not counted as evaluated, and no rating is given unless all
/// nine were evaluated.
///
/// Known deviation from the paper, kept so the nine-signal layout and the
/// `ni_pos` id do not change: the signal list has "net income > 0" where the
/// paper has ΔROA (ROA this year above last year). Net income > 0 and ROA > 0
/// are the same test, so the paper's ΔROA is not scored.
pub fn calculate_piotroski(r: &Research) -> PiotroskiScorecard {
    let cur = r.currency.as_deref();

    // Identify current period and prior period
    let (curr_opt, prev_opt) = if r.annual.len() >= 2 {
        (
            Some(&r.annual[r.annual.len() - 1]),
            Some(&r.annual[r.annual.len() - 2]),
        )
    } else if r.annual.len() == 1 {
        (Some(&r.annual[0]), None)
    } else if r.quarters.len() >= 4 {
        (
            Some(&r.quarters[r.quarters.len() - 1]),
            r.quarters.iter().rev().nth(4),
        )
    } else if !r.quarters.is_empty() {
        (Some(&r.quarters[r.quarters.len() - 1]), None)
    } else {
        (None, None)
    };

    let mut criteria = Vec::with_capacity(9);

    // ── 1. Profitability (4 points) ──
    // 1. Positive Net Income
    let reported_ni = curr_opt.and_then(|p| p.net_income);
    let net_income = reported_ni.or_else(|| estimate_net_income(r));
    let ni_estimated = reported_ni.is_none() && net_income.is_some();

    let ni_outcome = net_income.map(|ni| ni > 0.0);
    let ni_fmt = net_income.map_or("n/a".to_string(), |v| {
        format!(
            "{} ({}){}",
            format_large_number(v, cur),
            if v > 0.0 { "Profit" } else { "Loss" },
            if ni_estimated { " (est.)" } else { "" }
        )
    });
    criteria.push(make_criterion(
        "ni_pos",
        "Profitability",
        "Positive Net Income",
        "Net Income > 0",
        ni_fmt,
        ni_outcome,
        "Generates positive accounting bottom-line profit in the latest period.",
    ));

    // 2. Positive Return on Assets
    let roa = r.fin.roa.or_else(|| curr_opt.and_then(|p| p.roa()));
    let roa_fmt = roa.map_or("n/a".to_string(), |v| format!("{:.2}% ROA", v * 100.0));
    criteria.push(make_criterion(
        "roa_pos",
        "Profitability",
        "Positive Return on Assets",
        "ROA > 0%",
        roa_fmt,
        roa.map(|v| v > 0.0),
        "Productive return generated across the total asset base.",
    ));

    // 3. Positive Operating Cash Flow
    let ocf = curr_opt.and_then(|p| p.ocf).or(r.fin.ocf);
    let ocf_fmt = ocf.map_or("n/a".to_string(), |v| {
        format!("{} CFO", format_large_number(v, cur))
    });
    criteria.push(make_criterion(
        "cfo_pos",
        "Profitability",
        "Positive Operating Cash Flow",
        "CFO > 0",
        ocf_fmt,
        ocf.map(|v| v > 0.0),
        "Core day-to-day operations generate positive actual cash flow.",
    ));

    // 4. Quality of Earnings / Accrual. Piotroski: ACCRUAL = ROA - CFO/assets
    // < 0. Both terms share one denominator, so the test is CFO > net income.
    // Free cash flow is a different quantity and is not a stand-in.
    let (accrual_outcome, accrual_fmt) = match (ocf, net_income) {
        (Some(c), Some(n)) => (
            Some(c > n),
            format!(
                "CFO {} vs NI {}{}",
                format_large_number(c, cur),
                format_large_number(n, cur),
                if ni_estimated { " (est.)" } else { "" }
            ),
        ),
        _ => (None, "n/a".to_string()),
    };
    criteria.push(make_criterion(
        "accrual",
        "Profitability",
        "Cash Flow > Net Income (Low Accruals)",
        "CFO > Net Income",
        accrual_fmt,
        accrual_outcome,
        "Earnings backed by hard operating cash flow rather than non-cash accruals.",
    ));

    // ── 2. Leverage, Liquidity & Source of Funds (3 points) ──
    // 5. Lower leverage. Piotroski: long-term debt / average assets fell.
    // Yahoo gives total debt, so the ratio is total debt / year-end assets.
    // Two periods are required; equal ratios do not count as a fall, except
    // that a firm with no debt in both periods keeps the point.
    let debt_curr = curr_opt.and_then(|p| p.total_debt);
    let debt_prev = prev_opt.and_then(|p| p.total_debt);
    let assets_curr = curr_opt.and_then(|p| p.total_assets).filter(|a| *a > 0.0);
    let assets_prev = prev_opt.and_then(|p| p.total_assets).filter(|a| *a > 0.0);
    let (lev_outcome, lev_fmt) = match (debt_curr, debt_prev, assets_curr, assets_prev) {
        (Some(0.0), Some(0.0), _, _) => (
            Some(true),
            "Debt-free across consecutive periods (0 debt)".to_string(),
        ),
        (Some(dc), Some(dp), Some(ac), Some(ap)) => {
            let (lc, lp) = (dc / ac, dp / ap);
            (
                Some(lc < lp),
                format!(
                    "Debt/assets {:.1}% vs {:.1}% ({} vs {} debt)",
                    lc * 100.0,
                    lp * 100.0,
                    format_large_number(dc, cur),
                    format_large_number(dp, cur)
                ),
            )
        }
        _ => (None, "n/a".to_string()),
    };
    criteria.push(make_criterion(
        "leverage",
        "Leverage & Liquidity",
        "Lower Long-Term Leverage YoY",
        "Debt Ratio (t) < Debt Ratio (t-1)",
        lev_fmt,
        lev_outcome,
        "Debt ratio decreased YoY, reducing solvency vulnerability.",
    ));

    // 6. Higher current ratio. Piotroski: ΔCURRENT_RATIO > 0.
    let cr_curr = curr_opt
        .and_then(|p| p.current_ratio())
        .or(r.fin.current_ratio);
    let cr_prev = prev_opt.and_then(|p| p.current_ratio());
    let (liq_outcome, liq_fmt) = match (cr_curr, cr_prev) {
        (Some(cc), Some(cp)) => (
            Some(cc > cp),
            format!(
                "{:.2}x vs {:.2}x ({:+.1}% YoY)",
                cc,
                cp,
                ((cc - cp) / cp) * 100.0
            ),
        ),
        _ => (None, "n/a".to_string()),
    };
    criteria.push(make_criterion(
        "liquidity",
        "Leverage & Liquidity",
        "Higher Current Ratio YoY",
        "Current Ratio (t) > Current Ratio (t-1)",
        liq_fmt,
        liq_outcome,
        "Liquid buffer improved relative to short-term obligations.",
    ));

    // 7. No dilution. Piotroski: no common equity issued in the year. Needs a
    // share count for both periods; a 0.5% rise is tolerated as ESOP rounding.
    let shares_curr = curr_opt.and_then(|p| p.shares_outstanding).or(r.fin.shares);
    let shares_prev = prev_opt.and_then(|p| p.shares_outstanding);
    let (dil_outcome, dil_fmt) = match (shares_curr, shares_prev) {
        (Some(sc), Some(sp)) if sp > 0.0 => (
            Some(sc <= sp * 1.005),
            format!(
                "{} vs {} shares ({:+.1}% YoY)",
                format_volume(sc as u64),
                format_volume(sp as u64),
                ((sc - sp) / sp) * 100.0
            ),
        ),
        _ => (None, "n/a".to_string()),
    };
    criteria.push(make_criterion(
        "dilution",
        "Leverage & Liquidity",
        "No Common Share Dilution",
        "Shares (t) <= Shares (t-1)",
        dil_fmt,
        dil_outcome,
        "No dilutive secondary share issuance, preventing equity dilution.",
    ));

    // ── 3. Operating Efficiency (2 points) ──
    // 8. Higher gross margin. Piotroski: ΔMARGIN > 0. Compare like with like:
    // gross margin if both periods have it, else operating margin for both.
    let gm_curr = curr_opt
        .and_then(|p| p.gross_margin())
        .or(r.fin.gross_margin);
    let gm_prev = prev_opt.and_then(|p| p.gross_margin());
    let om_curr = curr_opt.and_then(|p| p.op_margin());
    let om_prev = prev_opt.and_then(|p| p.op_margin());
    let (margin_name, margin_pair) = match (gm_curr, gm_prev, om_curr, om_prev) {
        (Some(c), Some(p), _, _) => ("Gross Margin", Some((c, p))),
        (_, _, Some(c), Some(p)) => ("Operating Margin", Some((c, p))),
        _ => ("Gross Margin", None),
    };
    let (gm_outcome, gm_fmt) = match margin_pair {
        Some((c, p)) => (
            Some(c > p),
            format!(
                "{:.1}% vs {:.1}% {} ({:+.0} bps YoY)",
                c * 100.0,
                p * 100.0,
                if margin_name == "Gross Margin" {
                    "gross"
                } else {
                    "operating"
                },
                (c - p) * 10_000.0
            ),
        ),
        None => (None, "n/a".to_string()),
    };
    criteria.push(make_criterion(
        "gross_margin",
        "Operating Efficiency",
        "Higher Gross Margin YoY",
        &format!("{margin_name} (t) > {margin_name} (t-1)"),
        gm_fmt,
        gm_outcome,
        "Margin expanded YoY, verifying pricing power or product cost control (operating margin is used for both years when gross profit is not reported for both).",
    ));

    // 9. Higher asset turnover. Piotroski: ΔTURN > 0, sales / assets (the
    // paper uses beginning-of-year assets; Yahoo's year-end assets are used).
    let turn_curr = curr_opt.and_then(|p| p.asset_turnover());
    let turn_prev = prev_opt.and_then(|p| p.asset_turnover());
    let (turn_outcome, turn_fmt) = match (turn_curr, turn_prev) {
        (Some(tc), Some(tp)) => (
            Some(tc > tp),
            format!(
                "{:.2}x vs {:.2}x ({:+.1}% YoY)",
                tc,
                tp,
                ((tc - tp) / tp) * 100.0
            ),
        ),
        _ => (None, "n/a".to_string()),
    };
    criteria.push(make_criterion(
        "asset_turnover",
        "Operating Efficiency",
        "Higher Asset Turnover YoY",
        "Asset Turnover (t) > Asset Turnover (t-1)",
        turn_fmt,
        turn_outcome,
        "Asset turnover improved, generating more revenue per unit of capital assets.",
    ));

    let profitability_score: u8 = criteria[0..4].iter().map(|c| c.points).sum();
    let leverage_score: u8 = criteria[4..7].iter().map(|c| c.points).sum();
    let efficiency_score: u8 = criteria[7..9].iter().map(|c| c.points).sum();
    let total_score = profitability_score + leverage_score + efficiency_score;
    let evaluated = criteria.iter().filter(|c| c.evaluated).count() as u8;

    let rating = (evaluated == 9).then_some({
        if total_score >= 8 {
            PiotroskiRating::VeryStrong
        } else if total_score >= 5 {
            PiotroskiRating::Moderate
        } else {
            PiotroskiRating::Weak
        }
    });
    let rating_label = match rating {
        Some(rt) => rt.label().to_string(),
        None => format!("Not rated ({evaluated} of 9 signals evaluated)"),
    };

    PiotroskiScorecard {
        score: total_score,
        max_score: 9,
        evaluated,
        rating,
        rating_label,
        profitability_score,
        leverage_score,
        efficiency_score,
        criteria,
    }
}

/// Calculate the Altman bankruptcy-risk models from reported figures.
///
/// - Z (Altman 1968, public manufacturers): 1.2*X1 + 1.4*X2 + 3.3*X3 +
///   0.6*X4 + 0.999*X5, zones 1.81 / 2.99. Needs all five ratios; X4 uses
///   the MARKET value of equity.
/// - Z'' (Altman 1995, non-manufacturers and emerging markets): 6.56*X1 +
///   3.26*X2 + 6.72*X3 + 1.05*X4, zones 1.10 / 2.60, X4 using BOOK equity,
///   no X5. It is computed whenever its own four ratios exist.
/// - Z' (Altman 1983, private firms) is not computed.
///
/// X1 = working capital / total assets, X2 = retained earnings / total assets,
/// X3 = EBIT / total assets, X5 = sales / total assets. A ratio whose inputs
/// are not reported is missing; it is never estimated or set to zero, and a
/// model with a missing ratio gives no score and no zone.
pub fn calculate_altman(r: &Research) -> AltmanScorecard {
    let is_financial = r.is_financial();

    // Latest period figures
    let latest_annual = r.annual.last();
    let latest_quarter = r.quarters.last();

    // Reported figures only. Nothing is invented: no fixed share of assets, no
    // "retained earnings = 70% of equity", no ROA in place of EBIT / assets.
    // A missing input leaves its ratio missing (#69).
    let assets = latest_annual
        .and_then(|p| p.total_assets)
        .or_else(|| latest_quarter.and_then(|p| p.total_assets));

    // Flow items need a full year: the latest annual figure, or the sum of the
    // last four quarters when all four report it. One quarter is not annualised.
    let ttm = |f: &dyn Fn(&crate::research::Period) -> Option<f64>| -> Option<f64> {
        let q: Vec<f64> = r.quarters.iter().rev().take(4).filter_map(f).collect();
        (q.len() == 4).then(|| q.iter().sum())
    };
    let revenue = latest_annual
        .and_then(|p| p.revenue)
        .or_else(|| ttm(&|p| p.revenue));
    let ebit = latest_annual
        .and_then(|p| p.ebit.or(p.operating_income))
        .or_else(|| ttm(&|p| p.ebit.or(p.operating_income)));

    let working_capital = latest_annual
        .and_then(|p| p.working_capital())
        .or_else(|| latest_quarter.and_then(|p| p.working_capital()));

    let retained_earnings = latest_annual
        .and_then(|p| p.retained_earnings)
        .or_else(|| latest_quarter.and_then(|p| p.retained_earnings));

    let market_cap = r.quote.market_cap.or_else(|| {
        if let (Some(price), Some(shares)) = (r.price(), r.fin.shares) {
            Some(price * shares)
        } else {
            None
        }
    });

    // Total liabilities as reported, or assets - equity (the balance-sheet
    // identity) when only those two are reported.
    let total_liabilities = latest_annual
        .and_then(|p| p.total_liabilities)
        .or_else(|| latest_quarter.and_then(|p| p.total_liabilities))
        .or_else(|| match (assets, latest_annual.and_then(|p| p.equity)) {
            (Some(a), Some(eq)) if a > eq => Some(a - eq),
            _ => None,
        });

    // 5 Discriminant Factor Ratios:
    // X1 = Working Capital / Total Assets (weight: 1.2)
    let x1 = match (working_capital, assets) {
        (Some(wc), Some(a)) if a > 0.0 => Some(wc / a),
        _ => None,
    };
    // X2 = Retained Earnings / Total Assets (weight: 1.4)
    let x2 = match (retained_earnings, assets) {
        (Some(re), Some(a)) if a > 0.0 => Some(re / a),
        _ => None,
    };
    // X3 = EBIT / Total Assets (weight: 3.3)
    let x3 = match (ebit, assets) {
        (Some(eb), Some(a)) if a > 0.0 => Some(eb / a),
        _ => None,
    };
    // X4 = Market Value of Equity / Total Liabilities (weight: 0.6)
    let x4 = match (market_cap, total_liabilities) {
        (Some(mc), Some(tl)) if tl > 0.0 => Some(mc / tl),
        _ => None,
    };
    // X5 = Sales / Total Assets (weight: 0.999)
    let x5 = match (revenue, assets) {
        (Some(rev), Some(a)) if a > 0.0 => Some(rev / a),
        _ => None,
    };

    let mut components = Vec::with_capacity(5);

    components.push(make_altman_component(
        "X1",
        "Working Capital / Total Assets",
        "Working Capital / Total Assets",
        1.20,
        x1,
        x1.map_or("N/A".to_string(), |v| {
            format!("{:.3} ({:.1}%)", v, v * 100.0)
        }),
    ));

    components.push(make_altman_component(
        "X2",
        "Retained Earnings / Total Assets",
        "Retained Earnings / Total Assets",
        1.40,
        x2,
        x2.map_or("N/A".to_string(), |v| {
            format!("{:.3} ({:.1}%)", v, v * 100.0)
        }),
    ));

    components.push(make_altman_component(
        "X3",
        "EBIT / Total Assets",
        "EBIT / Total Assets",
        3.30,
        x3,
        x3.map_or("N/A".to_string(), |v| {
            format!("{:.3} ({:.1}%)", v, v * 100.0)
        }),
    ));

    components.push(make_altman_component(
        "X4",
        "Market Equity / Total Liabilities",
        "Market Cap / Total Liabilities",
        0.60,
        x4,
        x4.map_or("N/A".to_string(), |v| format!("{:.3}x", v)),
    ));

    components.push(make_altman_component(
        "X5",
        "Sales / Total Assets",
        "Total Revenue / Total Assets",
        0.999,
        x5,
        x5.map_or("N/A".to_string(), |v| format!("{:.3}x Turnover", v)),
    ));

    // Altman (1968) Z, the original public-manufacturer model, is defined only
    // when all five ratios exist. A partial sum is not comparable with the
    // 1.81 / 2.99 zones, so with any ratio missing there is no Z and no zone.
    let missing: Vec<&str> = components
        .iter()
        .filter(|c| c.raw_value.is_none())
        .map(|c| c.factor.as_str())
        .collect();
    let z_score = if missing.is_empty() {
        components
            .iter()
            .map(|c| c.weighted_value)
            .sum::<Option<f64>>()
    } else {
        None
    };

    let zone = z_score.map(|z| {
        if z > 2.99 {
            AltmanZone::Safe
        } else if z >= 1.81 {
            AltmanZone::Grey
        } else {
            AltmanZone::Distress
        }
    });

    // Z'' (Altman 1995; Altman, Hartzell & Peck): non-manufacturers and
    // emerging markets, Z'' = 6.56*X1 + 3.26*X2 + 6.72*X3 + 1.05*X4 with X4 =
    // BOOK value of equity / total liabilities (market value is not used), and
    // zones 1.10 / 2.60. X5 is not part of the model.
    let book_equity = latest_annual
        .and_then(|p| p.equity)
        .or_else(|| latest_quarter.and_then(|p| p.equity));
    let x4_book = match (book_equity, total_liabilities) {
        (Some(eq), Some(tl)) if tl > 0.0 => Some(eq / tl),
        _ => None,
    };
    let z2_components = vec![
        make_altman_component(
            "X1",
            "Working Capital / Total Assets",
            "Working Capital / Total Assets",
            6.56,
            x1,
            x1.map_or("N/A".to_string(), |v| {
                format!("{:.3} ({:.1}%)", v, v * 100.0)
            }),
        ),
        make_altman_component(
            "X2",
            "Retained Earnings / Total Assets",
            "Retained Earnings / Total Assets",
            3.26,
            x2,
            x2.map_or("N/A".to_string(), |v| {
                format!("{:.3} ({:.1}%)", v, v * 100.0)
            }),
        ),
        make_altman_component(
            "X3",
            "EBIT / Total Assets",
            "EBIT / Total Assets",
            6.72,
            x3,
            x3.map_or("N/A".to_string(), |v| {
                format!("{:.3} ({:.1}%)", v, v * 100.0)
            }),
        ),
        make_altman_component(
            "X4",
            "Book Equity / Total Liabilities",
            "Book Equity / Total Liabilities",
            1.05,
            x4_book,
            x4_book.map_or("N/A".to_string(), |v| format!("{:.3}x", v)),
        ),
    ];
    let z_double_prime = z2_components
        .iter()
        .map(|c| c.weighted_value)
        .sum::<Option<f64>>();

    let z_double_prime_zone = z_double_prime.map(|z| {
        if z > 2.60 {
            AltmanZone::Safe
        } else if z >= 1.10 {
            AltmanZone::Grey
        } else {
            AltmanZone::Distress
        }
    });

    let mut notes: Vec<String> = Vec::new();
    if is_financial {
        notes.push("Financial institution: Classical industrial Altman Z-score is not designed for banking and lending institutions due to deposit liability structures.".into());
    }
    if !missing.is_empty() {
        notes.push(format!(
            "Z-score not computed: missing {} (the model needs all five ratios; none are estimated).",
            missing.join(", ")
        ));
    }
    if z_double_prime.is_none() {
        let z2_missing: Vec<&str> = z2_components
            .iter()
            .filter(|c| c.raw_value.is_none())
            .map(|c| c.factor.as_str())
            .collect();
        notes.push(format!(
            "Z'' not computed: missing {} (book equity, not market cap, is X4).",
            z2_missing.join(", ")
        ));
    }
    let note = (!notes.is_empty()).then(|| notes.join(" "));
    let missing_factors: Vec<String> = missing.iter().map(|m| m.to_string()).collect();

    AltmanScorecard {
        z_score,
        zone,
        zone_label: zone.map(|z| z.label().to_string()),
        formula_str: "Z = 1.20*X1 + 1.40*X2 + 3.30*X3 + 0.60*X4 + 0.999*X5".to_string(),
        is_financial,
        note,
        missing_factors,
        components,
        z_double_prime,
        z_double_prime_zone,
        z_double_prime_components: z2_components,
        z_double_prime_formula_str:
            "Z'' = 6.56*X1 + 3.26*X2 + 6.72*X3 + 1.05*X4 (X4 = book equity / total liabilities)"
                .to_string(),
    }
}

/// Generate the combined accounting scorecards report for a company.
pub fn generate_scorecard_report(r: &Research) -> ScorecardReport {
    let piotroski = calculate_piotroski(r);
    let altman = calculate_altman(r);

    ScorecardReport {
        symbol: r.symbol.clone(),
        name: r.name.clone(),
        currency: r.currency.clone(),
        price: r.price(),
        market_cap: r.quote.market_cap,
        sector: r.sector.clone(),
        industry: r.industry.clone(),
        piotroski,
        altman,
        evaluation_date: chrono::Utc::now().date_naive(),
    }
}

/// Print the comprehensive transparent scorecard in the terminal.
pub fn print_scorecard_report(report: &ScorecardReport) {
    let cur = report.currency.as_deref();
    let border = "═".repeat(68);
    let subline = "─".repeat(68);

    println!("\n{}", border.cyan());
    println!(
        "  TRANSPARENT ACCOUNTING SCORECARDS: {} ({})",
        report.symbol.bold(),
        report.name
    );
    if let Some(price) = report.price {
        let mcap_str = report
            .market_cap
            .map(|m| format!(" · Market Cap: {}", format_large_number(m, cur)))
            .unwrap_or_default();
        let sector_str = report
            .sector
            .as_deref()
            .map(|s| format!(" · Sector: {}", s))
            .unwrap_or_default();
        println!(
            "  Price: {}{}{}{}",
            format_price(price, cur).bold(),
            mcap_str,
            sector_str,
            format!(" · Evaluated: {}", report.evaluation_date).dimmed()
        );
    }
    println!("{}", border.cyan());

    // ── Scorecard Summary Badges ──
    println!("\n  {}", "EXECUTIVE AUDIT SUMMARY".bold().yellow());
    println!("  {}", subline.dimmed());
    if report.piotroski.evaluated == 0 {
        println!(
            "  Piotroski F-Score:   {}",
            "n/a (no signal has data)".dimmed()
        );
    } else {
        let rating = report
            .piotroski
            .rating
            .map(|r| r.badge())
            .unwrap_or_else(|| report.piotroski.rating_label.yellow().to_string());
        println!(
            "  Piotroski F-Score:   {}/{}{}  {}  {}",
            report.piotroski.score,
            report.piotroski.evaluated,
            if report.piotroski.evaluated < report.piotroski.max_score {
                " evaluated"
            } else {
                ""
            },
            score_bar(report.piotroski.score, report.piotroski.evaluated),
            rating
        );
    }
    if let Some(z) = report.altman.z_score {
        let zone_str = report
            .altman
            .zone
            .map(|z| z.badge())
            .unwrap_or_else(|| "N/A".to_string());
        println!("  Altman Z-Score:      {:.2}  {}", z, zone_str);
    } else {
        let why = if report.altman.missing_factors.is_empty() {
            "n/a".to_string()
        } else {
            format!("n/a (missing {})", report.altman.missing_factors.join(", "))
        };
        println!("  Altman Z-Score:      {}", why.dimmed());
    }
    if let Some(note) = &report.altman.note {
        println!("  {}", note.italic().dimmed());
    }

    // ── 1. Piotroski 9-Point F-Score Table ──
    println!(
        "\n  {}",
        "1. PIOTROSKI 9-POINT F-SCORE BREAKDOWN".bold().yellow()
    );
    println!("  {}", subline.dimmed());
    println!(
        "  {:<24} {:<24} {:<12} {:>6}",
        "Criterion", "Formula", "Actual Value", "Points"
    );
    println!("  {}", subline.dimmed());

    let mut current_cat = "";
    for c in &report.piotroski.criteria {
        if c.category != current_cat {
            current_cat = &c.category;
            let (score, max) = match current_cat {
                "Profitability" => (report.piotroski.profitability_score, 4),
                "Leverage & Liquidity" => (report.piotroski.leverage_score, 3),
                "Operating Efficiency" => (report.piotroski.efficiency_score, 2),
                _ => (0, 0),
            };
            let n_eval = report
                .piotroski
                .criteria
                .iter()
                .filter(|x| x.category == current_cat && x.evaluated)
                .count();
            let cat_score = if n_eval == max {
                format!("{score}/{max}")
            } else {
                format!("{score}/{n_eval} evaluated, {max} signals")
            };
            println!(
                "\n  ▶ {} ({})",
                current_cat.bold().cyan(),
                cat_score.bold().green()
            );
        }

        let pts_str = if !c.evaluated {
            "n/a".dimmed().to_string()
        } else if c.passed {
            "+1".green().bold().to_string()
        } else {
            " 0".dimmed().to_string()
        };

        println!(
            "  • {:<22} {:<24} {:<12} {:>6}",
            c.name,
            c.formula.dimmed(),
            c.formatted_value,
            pts_str
        );
    }

    println!("\n  {}", subline.dimmed());
    println!(
        "  Piotroski Score Subtotals: Profitability {} · Leverage {} · Efficiency {} = {}/{} ({} of 9 signals evaluated)",
        report.piotroski.profitability_score,
        report.piotroski.leverage_score,
        report.piotroski.efficiency_score,
        report.piotroski.score.to_string().bold().green(),
        report.piotroski.evaluated,
        report.piotroski.evaluated
    );
    if report.piotroski.evaluated < report.piotroski.max_score {
        println!(
            "  {}",
            "n/a = the data for that year-over-year test is missing; it earns no point and no rating is given."
                .dimmed()
        );
    }

    // ── 2. Altman Z-Score Breakdown Table ──
    println!(
        "\n  {}",
        "2. ALTMAN Z-SCORE DISCRIMINANT MODEL BREAKDOWN"
            .bold()
            .yellow()
    );
    println!("  {}", subline.dimmed());
    println!("  Formula: {}", report.altman.formula_str.dimmed());
    println!("  {}", subline.dimmed());
    println!(
        "  {:<6} {:<32} {:>8} {:>12} {:>10}",
        "Factor", "Component Ratio", "Weight", "Value", "Contrib"
    );
    println!("  {}", subline.dimmed());

    for comp in &report.altman.components {
        let contrib_str = comp
            .weighted_value
            .map(|wv| format!("{:+.3}", wv))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  {:<6} {:<32} {:>7.2}x {:>12} {:>10}",
            comp.factor.bold(),
            comp.name,
            comp.weight,
            comp.formatted_value,
            contrib_str.cyan()
        );
    }

    println!("  {}", subline.dimmed());
    if report.altman.is_financial {
        println!(
            "  Altman Z-Score:        n/a for banks and lenders (deposits are not industrial debt)"
        );
    } else if let Some(z) = report.altman.z_score {
        let zone_desc = match report.altman.zone {
            Some(AltmanZone::Safe) => {
                "Safe Zone (Z > 2.99) — Low probability of financial distress"
                    .green()
                    .bold()
                    .to_string()
            }
            Some(AltmanZone::Grey) => "Grey Zone (1.81 <= Z <= 2.99) — Moderate financial caution"
                .yellow()
                .bold()
                .to_string(),
            Some(AltmanZone::Distress) => {
                "Distress Zone (Z < 1.81) — High insolvency / default risk within 24 months"
                    .red()
                    .bold()
                    .to_string()
            }
            None => "N/A".to_string(),
        };
        println!("  Total Altman Z-Score:  {:.2}  ▶  {}", z, zone_desc);
    } else {
        println!(
            "  Total Altman Z-Score:  n/a (needs all five ratios; missing {})",
            report.altman.missing_factors.join(", ")
        );
    }
    println!(
        "\n  Z'' model (non-manufacturers): {}",
        report.altman.z_double_prime_formula_str.dimmed()
    );
    for comp in &report.altman.z_double_prime_components {
        let contrib_str = comp
            .weighted_value
            .map(|wv| format!("{:+.3}", wv))
            .unwrap_or_else(|| "N/A".to_string());
        println!(
            "  {:<6} {:<32} {:>7.2}x {:>12} {:>10}",
            comp.factor.bold(),
            comp.name,
            comp.weight,
            comp.formatted_value,
            contrib_str.cyan()
        );
    }
    if let (Some(z_prime), Some(zone_prime)) = (
        report.altman.z_double_prime,
        report.altman.z_double_prime_zone,
    ) {
        println!(
            "  Non-Mfg / Service Z'': {:.2}  ▶  {}",
            z_prime,
            zone_prime.badge()
        );
    }
    println!("{}\n", border.cyan());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Quote;
    use crate::research::{Fin, Period, Research};
    use chrono::NaiveDate;

    fn build_test_research() -> Research {
        let mut r = Research::default();
        r.symbol = "TEST.NS".into();
        r.name = "Test Enterprise Ltd".into();
        r.currency = Some("INR".into());

        r.quote = Quote {
            market_cap: Some(1_000_000_000_000.0), // 1 Lakh Cr
            regular_market_price: Some(2500.0),
            ..Default::default()
        };

        r.fin = Fin {
            roa: Some(0.12),
            current_ratio: Some(1.85),
            gross_margin: Some(0.45),
            profit_margin: Some(0.20),
            de: Some(15.0),
            ocf: Some(18_000_000_000.0),
            shares: Some(400_000_000.0),
            revenue_growth: Some(0.15),
            ..Default::default()
        };

        // Prior year (2025)
        let p_prev = Period {
            end: NaiveDate::from_ymd_opt(2025, 3, 31).unwrap(),
            revenue: Some(100_000_000_000.0),
            net_income: Some(15_000_000_000.0),
            operating_income: Some(22_000_000_000.0),
            ocf: Some(16_000_000_000.0),
            equity: Some(60_000_000_000.0),
            total_debt: Some(10_000_000_000.0),
            total_assets: Some(90_000_000_000.0),
            gross_profit: Some(42_000_000_000.0),
            current_assets: Some(45_000_000_000.0),
            current_liabilities: Some(28_000_000_000.0),
            retained_earnings: Some(40_000_000_000.0),
            shares_outstanding: Some(400_000_000.0),
            total_liabilities: Some(30_000_000_000.0),
            ..Default::default()
        };

        // Current year (2026) - strong improvement across all 9 criteria
        let p_curr = Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            revenue: Some(120_000_000_000.0),
            net_income: Some(20_000_000_000.0),
            operating_income: Some(28_000_000_000.0),
            ebit: Some(28_000_000_000.0),
            ocf: Some(24_000_000_000.0),
            equity: Some(75_000_000_000.0),
            total_debt: Some(8_000_000_000.0),
            total_assets: Some(100_000_000_000.0),
            gross_profit: Some(54_000_000_000.0),
            current_assets: Some(55_000_000_000.0),
            current_liabilities: Some(30_000_000_000.0),
            retained_earnings: Some(52_000_000_000.0),
            working_capital: Some(25_000_000_000.0),
            shares_outstanding: Some(400_000_000.0),
            total_liabilities: Some(25_000_000_000.0),
            ..Default::default()
        };

        r.annual = vec![p_prev, p_curr];
        r
    }

    #[test]
    fn test_piotroski_perfect_nine_score() {
        let r = build_test_research();
        let sc = calculate_piotroski(&r);

        assert_eq!(sc.score, 9);
        assert_eq!(sc.max_score, 9);
        assert_eq!(sc.profitability_score, 4);
        assert_eq!(sc.leverage_score, 3);
        assert_eq!(sc.efficiency_score, 2);
        assert_eq!(sc.rating, Some(PiotroskiRating::VeryStrong));
        assert_eq!(sc.evaluated, 9);

        for c in &sc.criteria {
            assert!(c.passed, "criterion {} should pass", c.id);
            assert_eq!(c.points, 1);
        }
    }

    #[test]
    fn test_piotroski_distressed_low_score() {
        let mut r = Research::default();
        r.symbol = "DISTRESS.NS".into();

        let p_prev = Period {
            end: NaiveDate::from_ymd_opt(2025, 3, 31).unwrap(),
            revenue: Some(100_000_000.0),
            net_income: Some(10_000_000.0),
            total_debt: Some(10_000_000.0),
            total_assets: Some(80_000_000.0),
            current_assets: Some(40_000_000.0),
            current_liabilities: Some(20_000_000.0),
            shares_outstanding: Some(10_000_000.0),
            gross_profit: Some(40_000_000.0),
            ..Default::default()
        };

        // Deteriorated on every dimension
        let p_curr = Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            revenue: Some(80_000_000.0),
            net_income: Some(-5_000_000.0), // Loss
            ocf: Some(-15_000_000.0),       // Negative CFO worse than NI
            total_debt: Some(40_000_000.0), // Massive debt jump
            total_assets: Some(70_000_000.0),
            current_assets: Some(20_000_000.0),
            current_liabilities: Some(30_000_000.0), // CR < 1
            shares_outstanding: Some(15_000_000.0),  // Dilution (+50%)
            gross_profit: Some(20_000_000.0),        // GM dropped from 40% to 25%
            ..Default::default()
        };

        r.annual = vec![p_prev, p_curr];
        let sc = calculate_piotroski(&r);

        assert_eq!(sc.score, 0);
        assert_eq!(sc.rating, Some(PiotroskiRating::Weak));
        assert_eq!(sc.evaluated, 9);
        assert_eq!(sc.profitability_score, 0);
        assert_eq!(sc.leverage_score, 0);
        assert_eq!(sc.efficiency_score, 0);
    }

    #[test]
    fn test_piotroski_debt_free_passes_leverage() {
        let mut r = Research::default();
        r.symbol = "DEBTFREE.NS".into();

        let p_prev = Period {
            end: NaiveDate::from_ymd_opt(2025, 3, 31).unwrap(),
            total_debt: Some(0.0),
            ..Default::default()
        };
        let p_curr = Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            total_debt: Some(0.0),
            ..Default::default()
        };

        r.annual = vec![p_prev, p_curr];
        let sc = calculate_piotroski(&r);
        let lev = sc.criteria.iter().find(|c| c.id == "leverage").unwrap();
        assert!(lev.passed);
        assert_eq!(lev.points, 1);
        assert!(lev.formatted_value.contains("Debt-free"));
    }

    #[test]
    fn test_altman_z_score_calculation() {
        let r = build_test_research();
        let altman = calculate_altman(&r);

        assert!(altman.z_score.is_some());
        let z = altman.z_score.unwrap();
        // With high market cap to liabilities (1000B / 25B = 40x), Z should be comfortably Safe (> 2.99)
        assert!(z > 2.99);
        assert_eq!(altman.zone, Some(AltmanZone::Safe));
        assert_eq!(altman.components.len(), 5);

        // Verify factor weights
        assert!((altman.components[0].weight - 1.20).abs() < 1e-6);
        assert!((altman.components[1].weight - 1.40).abs() < 1e-6);
        assert!((altman.components[2].weight - 3.30).abs() < 1e-6);
        assert!((altman.components[3].weight - 0.60).abs() < 1e-6);
        assert!((altman.components[4].weight - 0.999).abs() < 1e-6);
    }

    #[test]
    fn test_altman_distress_zone() {
        let mut r = Research::default();
        r.symbol = "BANKRUPT.NS".into();

        r.annual = vec![Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            total_assets: Some(100_000_000.0),
            working_capital: Some(-20_000_000.0), // Negative working capital
            retained_earnings: Some(-30_000_000.0), // Accumulated losses
            ebit: Some(-10_000_000.0),            // Operating losses
            revenue: Some(40_000_000.0),
            total_liabilities: Some(120_000_000.0),
            ..Default::default()
        }];

        r.quote = Quote {
            market_cap: Some(10_000_000.0), // Sinking equity value
            ..Default::default()
        };

        let altman = calculate_altman(&r);
        assert!(altman.z_score.is_some());
        let z = altman.z_score.unwrap();
        assert!(z < 1.81, "z-score was {}", z);
        assert_eq!(altman.zone, Some(AltmanZone::Distress));
    }

    #[test]
    fn test_scorecard_report_serialization() {
        let r = build_test_research();
        let report = generate_scorecard_report(&r);
        let json_str = serde_json::to_string_pretty(&report).expect("must serialize");
        assert!(json_str.contains("TEST.NS"));
        assert!(json_str.contains("VeryStrong"));
        assert!(json_str.contains("Safe"));
        assert!(json_str.contains("piotroski"));
        assert!(json_str.contains("altman"));
    }

    #[test]
    fn altman_x4_uses_one_currency_for_usd_reporters() {
        // Infosys-like: ₹ market cap, statements filed in US dollars (#207).
        use serde_json::json;
        let item = |d: &str, v: f64| json!({"asOfDate": d, "currencyCode": "USD", "reportedValue": {"raw": v}});
        let ts = vec![
            json!({"meta": {"type": ["annualTotalAssets"]},
                   "annualTotalAssets": [item("2026-03-31", 16_446_000_000.0)]}),
            json!({"meta": {"type": ["annualTotalLiabilitiesNetMinorityInterest"]},
                   "annualTotalLiabilitiesNetMinorityInterest": [item("2026-03-31", 6_000_000_000.0)]}),
        ];
        let fx = crate::research::StatementFx::new(Some("INR"), [("USD".to_string(), 85.0)]);
        let (_, annual) = crate::research::parse_timeseries(&ts, &fx);
        let mut r = Research::default();
        r.currency = Some("INR".into());
        r.quote = Quote {
            market_cap: Some(4.8e12),
            ..Default::default()
        };
        r.annual = annual;
        let report = generate_scorecard_report(&r);
        let x4 = report
            .altman
            .components
            .iter()
            .find(|c| c.factor == "X4")
            .and_then(|c| c.raw_value)
            .unwrap();
        // 4.8e12 / (6e9 × 85) ≈ 9.4, not ≈ 800.
        assert!((x4 - 9.41).abs() < 0.1, "X4 = {x4}");
    }

    // #68. Hand-worked, no periods at all:
    //   trailing EPS 50 x 400,000,000 shares = 20,000,000,000 net income.
    //   Old code: margin 0.10 x market cap 1e12 = 100,000,000,000 > CFO, so the
    //   accrual point was lost. Right answer: CFO 30,000,000,000 > 20,000,000,000.
    #[test]
    fn piotroski_net_income_estimate_uses_eps_times_shares_not_market_cap() {
        let mut r = Research::default();
        r.quote = Quote {
            market_cap: Some(1_000_000_000_000.0),
            ..Default::default()
        };
        r.fin = Fin {
            profit_margin: Some(0.10),
            trailing_eps: Some(50.0),
            shares: Some(400_000_000.0),
            ocf: Some(30_000_000_000.0),
            ..Default::default()
        };
        let sc = calculate_piotroski(&r);
        let ni = sc.criteria.iter().find(|c| c.id == "ni_pos").unwrap();
        assert!(ni.passed);
        assert!(
            ni.formatted_value.contains("(est.)"),
            "{}",
            ni.formatted_value
        );
        let acc = sc.criteria.iter().find(|c| c.id == "accrual").unwrap();
        assert!(acc.passed, "{}", acc.formatted_value);
        // The shown figure is 20 billion, not 100 billion.
        assert!(
            acc.formatted_value
                .contains(&format_large_number(2.0e10, None))
        );
        assert!(
            !acc.formatted_value
                .contains(&format_large_number(1.0e11, None))
        );
    }

    // #68. No EPS and no revenue: margin x revenue is unavailable, and market
    // cap must not stand in, so net income is n/a and nothing is scored.
    #[test]
    fn piotroski_net_income_is_na_without_eps_or_revenue() {
        let mut r = Research::default();
        r.quote = Quote {
            market_cap: Some(1_000_000_000_000.0),
            ..Default::default()
        };
        r.fin = Fin {
            profit_margin: Some(0.10),
            ..Default::default()
        };
        let sc = calculate_piotroski(&r);
        let ni = sc.criteria.iter().find(|c| c.id == "ni_pos").unwrap();
        assert_eq!(ni.formatted_value, "n/a");
        assert!(!ni.evaluated);
        assert!(!ni.passed);
    }

    // #68. Margin x revenue path: four quarters of 50,000,000,000 sum to
    // 200,000,000,000; x 0.10 margin = 20,000,000,000.
    #[test]
    fn piotroski_net_income_estimate_from_margin_and_ttm_revenue() {
        let mut r = Research::default();
        r.fin = Fin {
            profit_margin: Some(0.10),
            ..Default::default()
        };
        for m in [3u32, 6, 9, 12] {
            r.quarters.push(Period {
                end: NaiveDate::from_ymd_opt(2025, m, 28).unwrap(),
                revenue: Some(50_000_000_000.0),
                ..Default::default()
            });
        }
        let ni = estimate_net_income(&r).unwrap();
        assert!((ni - 2.0e10).abs() < 1.0, "{ni}");
    }

    // #71. One annual period, no share data, no prior year. Hand count:
    //   NI 10 > 0 -> 1 point; ROA = 10 / 200 = 0.05 > 0 -> 1 point;
    //   CFO 15 > 0 -> 1 point; CFO 15 > NI 10 -> 1 point.
    //   Signals 5-9 need a prior year (or shares) and are n/a: 4 evaluated,
    //   4 points, no rating. The old code also gave dilution a free point and
    //   scored D/E 10% <= 40% and revenue growth 20% >= 8% as levels.
    #[test]
    fn piotroski_one_period_leaves_year_over_year_signals_unscored() {
        let mut r = Research::default();
        r.fin = Fin {
            de: Some(10.0),
            revenue_growth: Some(0.20),
            gross_margin: Some(0.45),
            current_ratio: Some(2.0),
            ..Default::default()
        };
        r.annual = vec![Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            revenue: Some(100.0),
            net_income: Some(10.0),
            ocf: Some(15.0),
            total_assets: Some(200.0),
            total_debt: Some(0.0),
            ..Default::default()
        }];
        let sc = calculate_piotroski(&r);
        assert_eq!((sc.score, sc.evaluated, sc.max_score), (4, 4, 9));
        assert_eq!(sc.rating, None);
        assert!(sc.rating_label.contains("4 of 9"), "{}", sc.rating_label);
        for id in [
            "leverage",
            "liquidity",
            "dilution",
            "gross_margin",
            "asset_turnover",
        ] {
            let c = sc.criteria.iter().find(|c| c.id == id).unwrap();
            assert!(!c.evaluated && !c.passed && c.points == 0, "{id}");
            assert_eq!(c.formatted_value, "n/a", "{id}");
        }
        let json = serde_json::to_value(&sc).unwrap();
        assert_eq!(json["evaluated"], 4);
        assert_eq!(json["criteria"][4]["evaluated"], false);
        assert!(json["rating"].is_null());
    }

    #[test]
    fn piotroski_with_no_data_scores_nothing() {
        let sc = calculate_piotroski(&Research::default());
        assert_eq!((sc.score, sc.evaluated), (0, 0));
        assert!(sc.criteria.iter().all(|c| !c.evaluated && c.points == 0));
    }

    // #71. Current year reports gross profit, prior year does not, so the two
    // years are compared on operating margin:
    //   operating margin now 10 / 100 = 0.10, prior 15 / 100 = 0.15 -> fell, 0 points.
    // Mixing gross margin now (30 / 100 = 0.30) with operating margin before
    // (0.15) would have shown 0.30 > 0.15 and awarded the point.
    #[test]
    fn piotroski_margin_compares_like_with_like() {
        let mut r = Research::default();
        r.annual = vec![
            Period {
                end: NaiveDate::from_ymd_opt(2025, 3, 31).unwrap(),
                revenue: Some(100.0),
                operating_income: Some(15.0),
                ..Default::default()
            },
            Period {
                end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
                revenue: Some(100.0),
                gross_profit: Some(30.0),
                operating_income: Some(10.0),
                ..Default::default()
            },
        ];
        let sc = calculate_piotroski(&r);
        let gm = sc.criteria.iter().find(|c| c.id == "gross_margin").unwrap();
        assert!(gm.evaluated);
        assert!(!gm.passed, "{}", gm.formatted_value);
        assert!(gm.formula.contains("Operating Margin"), "{}", gm.formula);
        assert!(
            gm.formatted_value.contains("10.0% vs 15.0%"),
            "{}",
            gm.formatted_value
        );
    }

    // #71. Dilution needs both share counts: 400,000,000 -> 440,000,000 is
    // +10%, above the 0.5% tolerance, so it is evaluated and fails; a missing
    // prior count is n/a, never a pass.
    #[test]
    fn piotroski_dilution_fails_on_issuance_and_is_na_when_unknown() {
        let mut r = Research::default();
        r.annual = vec![
            Period {
                end: NaiveDate::from_ymd_opt(2025, 3, 31).unwrap(),
                shares_outstanding: Some(400_000_000.0),
                ..Default::default()
            },
            Period {
                end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
                shares_outstanding: Some(440_000_000.0),
                ..Default::default()
            },
        ];
        let dil = |r: &Research| {
            calculate_piotroski(r)
                .criteria
                .into_iter()
                .find(|c| c.id == "dilution")
                .unwrap()
        };
        let c = dil(&r);
        assert!(c.evaluated && !c.passed);
        r.annual[0].shares_outstanding = None;
        let c = dil(&r);
        assert!(!c.evaluated && !c.passed && c.points == 0);
    }

    // #70. Hand-worked, amounts in crore: assets 160,000 = equity 100,000 +
    // liabilities 60,000; market cap 800,000 (P/B 8).
    //   X1 = -16,000 / 160,000 = -0.10    X2 = 16,000 / 160,000 = 0.10
    //   X3 =   3,200 / 160,000 =  0.02    X4 book = 100,000 / 60,000 = 1.6667
    //   Z'' = 6.56(-0.10) + 3.26(0.10) + 6.72(0.02) + 1.05(1.6667)
    //       = -0.656 + 0.326 + 0.1344 + 1.75 = 1.5544  -> Grey (1.10 to 2.60)
    // The old code used market X4 = 800,000 / 60,000 = 13.333 and got
    //   -0.656 + 0.326 + 0.1344 + 14.0 = 13.8044 -> "Safe".
    // The classic Z keeps market X4: 0.6 x 13.333 = 8.0, so
    //   Z = 1.2(-0.10) + 1.4(0.10) + 3.3(0.02) + 8.0 + 0.999(1.0) = 9.085.
    fn pb8_research() -> Research {
        let mut r = Research::default();
        r.quote = Quote {
            market_cap: Some(800_000.0),
            ..Default::default()
        };
        r.annual = vec![Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            total_assets: Some(160_000.0),
            total_liabilities: Some(60_000.0),
            equity: Some(100_000.0),
            working_capital: Some(-16_000.0),
            retained_earnings: Some(16_000.0),
            ebit: Some(3_200.0),
            revenue: Some(160_000.0),
            ..Default::default()
        }];
        r
    }

    #[test]
    fn altman_z_double_prime_uses_book_equity_for_x4() {
        let a = calculate_altman(&pb8_research());
        let x4 = a
            .z_double_prime_components
            .iter()
            .find(|c| c.factor == "X4")
            .unwrap();
        assert!((x4.raw_value.unwrap() - 100_000.0 / 60_000.0).abs() < 1e-9);
        let z2 = a.z_double_prime.unwrap();
        assert!((z2 - 1.5544).abs() < 1e-3, "Z'' = {z2}");
        assert_eq!(a.z_double_prime_zone, Some(AltmanZone::Grey));
        // The classic Z still uses market cap for X4.
        let x4m = a.components.iter().find(|c| c.factor == "X4").unwrap();
        assert!((x4m.raw_value.unwrap() - 800_000.0 / 60_000.0).abs() < 1e-9);
        let z = a.z_score.unwrap();
        assert!((z - 9.085).abs() < 1e-3, "Z = {z}");
    }

    #[test]
    fn altman_z_double_prime_is_none_without_book_equity() {
        let mut r = pb8_research();
        r.annual[0].equity = None;
        // Assets are known, so total liabilities stay as reported; only equity is gone.
        let a = calculate_altman(&r);
        assert_eq!(a.z_double_prime, None);
        assert_eq!(a.z_double_prime_zone, None);
        assert!(a.z_score.is_some());
    }

    // #69. Hand-worked, assets 100, X1 and X5 unreported (no working capital,
    // no revenue): X2 = 30/100 = 0.30, X3 = 15/100 = 0.15, X4 = 100/50 = 2.0.
    // The old code summed the three it had: 1.4(0.30) + 3.3(0.15) + 0.6(2.0)
    // = 0.42 + 0.495 + 1.2 = 2.115 and called it "Grey Zone". With sales/assets
    // of 1.0 the full model gives 3.114 ("Safe"), so the verdict hung on a
    // ratio the code never had. Now: no Z, no zone, and the note names X1, X5.
    // Z'' also needs X1, so it is absent too.
    fn three_of_five() -> Research {
        let mut r = Research::default();
        r.quote = Quote {
            market_cap: Some(100.0),
            ..Default::default()
        };
        r.annual = vec![Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            total_assets: Some(100.0),
            retained_earnings: Some(30.0),
            ebit: Some(15.0),
            total_liabilities: Some(50.0),
            ..Default::default()
        }];
        r
    }

    #[test]
    fn altman_with_three_of_five_ratios_gives_no_zone() {
        let a = calculate_altman(&three_of_five());
        assert_eq!(a.z_score, None);
        assert_eq!(a.zone, None);
        assert_eq!(a.zone_label, None);
        assert_eq!(a.missing_factors, vec!["X1", "X5"]);
        let note = a.note.unwrap();
        assert!(note.contains("X1, X5"), "{note}");
        assert_eq!(a.z_double_prime, None);
        // The three known ratios are still shown.
        assert!((a.components[3].raw_value.unwrap() - 2.0).abs() < 1e-9);
    }

    // #69. Z'' needs X1-X4 only. X5 (revenue) is missing here, so Z is absent
    // but Z'' is computed: X1 = 10/100 = 0.10, X2 = 0.30, X3 = 0.15, book
    // X4 = 50/50 = 1.0, so Z'' = 0.656 + 0.978 + 1.008 + 1.05 = 3.692 (Safe).
    #[test]
    fn altman_z_double_prime_survives_a_missing_x5() {
        let mut r = three_of_five();
        r.annual[0].working_capital = Some(10.0);
        r.annual[0].equity = Some(50.0);
        let a = calculate_altman(&r);
        assert_eq!(a.z_score, None);
        assert_eq!(a.missing_factors, vec!["X5"]);
        let z2 = a.z_double_prime.unwrap();
        assert!((z2 - 3.692).abs() < 1e-9, "Z'' = {z2}");
        assert_eq!(a.z_double_prime_zone, Some(AltmanZone::Safe));
    }

    // #69. Full data, hand-worked: assets 100, WC 20, RE 30, EBIT 15, market
    // cap 100, liabilities 50, sales 100 -> X = 0.20, 0.30, 0.15, 2.0, 1.0.
    // Z = 1.2(0.20) + 1.4(0.30) + 3.3(0.15) + 0.6(2.0) + 0.999(1.0)
    //   = 0.24 + 0.42 + 0.495 + 1.2 + 0.999 = 3.354 -> Safe (> 2.99).
    #[test]
    fn altman_full_model_is_scored() {
        let mut r = three_of_five();
        r.annual[0].working_capital = Some(20.0);
        r.annual[0].revenue = Some(100.0);
        let a = calculate_altman(&r);
        let z = a.z_score.unwrap();
        assert!((z - 3.354).abs() < 1e-9, "Z = {z}");
        assert_eq!(a.zone, Some(AltmanZone::Safe));
        assert!(a.missing_factors.is_empty());
    }

    // #69. The fixed-ratio guesses are gone: with only equity, assets, EBITDA,
    // ROA and one quarter of revenue, retained earnings (was 70% of equity),
    // EBIT (was 85% of EBITDA, or ROA) and sales (was quarter x 4) stay missing.
    #[test]
    fn altman_does_not_guess_missing_inputs() {
        let mut r = Research::default();
        r.fin = Fin {
            ebitda: Some(20.0),
            roa: Some(0.10),
            current_ratio: Some(1.5),
            total_debt: Some(10.0),
            ..Default::default()
        };
        r.quote = Quote {
            market_cap: Some(100.0),
            ..Default::default()
        };
        r.annual = vec![Period {
            end: NaiveDate::from_ymd_opt(2026, 3, 31).unwrap(),
            total_assets: Some(100.0),
            equity: Some(40.0),
            ..Default::default()
        }];
        r.quarters = vec![Period {
            end: NaiveDate::from_ymd_opt(2026, 6, 30).unwrap(),
            revenue: Some(25.0),
            ..Default::default()
        }];
        let a = calculate_altman(&r);
        let raw = |f: &str| {
            a.components
                .iter()
                .find(|c| c.factor == f)
                .unwrap()
                .raw_value
        };
        assert_eq!(raw("X1"), None);
        assert_eq!(raw("X2"), None);
        assert_eq!(raw("X3"), None);
        assert_eq!(raw("X5"), None);
        // Liabilities come from the balance-sheet identity: 100 - 40 = 60.
        assert!((raw("X4").unwrap() - 100.0 / 60.0).abs() < 1e-9);
        assert_eq!(a.z_score, None);
    }
}
