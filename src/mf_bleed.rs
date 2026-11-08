//! Mutual Fund Commission Bleed & Regular-to-Direct Switch Analyzer.
//!
//! Compares held Regular mutual funds against their Direct twin schemes:
//! - Exact TER gap from AMFI disclosures (e.g. 1.85% regular vs 0.95% direct = 0.90% gap).
//! - Annual commission bleed in base currency (Current Value × TER gap).
//! - 10-year compounded wealth drag at category baseline CAGR.
//! - FIFO Tax-Free Switch breakdown:
//!   - Units held > 365 days (LTCG eligible & exit-load free) and units switchable
//!     today with ₹0 tax using remaining Section 112A ₹1.25L exemption.
//!   - Units held ≤ 365 days (locked in STCG/exit-load window), countdown to 1 year,
//!     and breakeven months if switched early.

use chrono::{Months, NaiveDate};
use serde::Serialize;

use crate::funds::{Scheme, TerTable, base_scheme_name};
use crate::output::CsvRow;
use crate::portfolio::{Lot, Portfolio};

/// One held Regular mutual fund analyzed against its Direct counterpart.
#[derive(Debug, Clone, Serialize)]
pub struct MfBleedScheme {
    pub regular_code: u32,
    pub regular_name: String,
    pub direct_code: Option<u32>,
    pub direct_name: Option<String>,
    pub units: f64,
    pub current_nav: f64,
    pub current_value: f64,
    pub regular_ter_pct: f64,
    pub direct_ter_pct: f64,
    pub ter_gap_pct: f64,
    pub annual_bleed: f64,
    pub ten_year_drag: f64,
    pub units_eligible_ltcg: f64,
    pub ltcg_on_eligible_units: f64,
    pub tax_free_switch_units: f64,
    pub tax_free_switch_value: f64,
    pub units_locked_stcg: f64,
    pub earliest_stcg_unlock_date: Option<String>,
    pub days_to_stcg_unlock: Option<i64>,
    pub breakeven_months_early_switch: Option<f64>,
}

impl CsvRow for MfBleedScheme {
    const COLUMNS: &'static [&'static str] = &[
        "regular_code",
        "regular_name",
        "direct_code",
        "direct_name",
        "units",
        "current_nav",
        "current_value",
        "regular_ter_pct",
        "direct_ter_pct",
        "ter_gap_pct",
        "annual_bleed",
        "ten_year_drag",
        "units_eligible_ltcg",
        "ltcg_on_eligible_units",
        "tax_free_switch_units",
        "tax_free_switch_value",
        "units_locked_stcg",
        "earliest_stcg_unlock_date",
        "days_to_stcg_unlock",
        "breakeven_months_early_switch",
    ];
}

/// Overall portfolio mutual fund bleed report.
#[derive(Debug, Clone, Serialize)]
pub struct MfBleedReport {
    pub total_regular_value: f64,
    pub total_annual_bleed: f64,
    pub total_10y_drag: f64,
    pub total_tax_free_switch_value: f64,
    pub schemes: Vec<MfBleedScheme>,
}

/// Find matching Direct twin for a Regular scheme.
pub fn find_direct_twin<'a>(regular: &Scheme, all_schemes: &'a [Scheme]) -> Option<&'a Scheme> {
    let base = base_scheme_name(&regular.name).to_lowercase();
    let is_growth = regular.is_growth();

    // 1. First attempt: exact base name match + growth/dividend matching + is_direct
    let candidates: Vec<&'a Scheme> = all_schemes
        .iter()
        .filter(|s| {
            s.is_direct()
                && s.is_growth() == is_growth
                && base_scheme_name(&s.name).to_lowercase() == base
        })
        .collect();

    if let Some(&first) = candidates.first() {
        return Some(first);
    }

    // 2. Fallback: contains base name or AMC match
    all_schemes.iter().find(|s| {
        s.is_direct()
            && s.is_growth() == is_growth
            && (s.name.to_lowercase().contains(&base)
                || base.contains(&base_scheme_name(&s.name).to_lowercase()))
            && regular.amc.is_some()
            && s.amc == regular.amc
    })
}

/// Compute 10-year compounded drag on wealth at baseline CAGR.
pub fn compounded_10y_drag(value: f64, ter_gap_pct: f64, baseline_cagr: f64) -> f64 {
    let r = baseline_cagr;
    let gap = (ter_gap_pct / 100.0).max(0.0);
    let direct_growth = (1.0 + r).powi(10);
    let regular_growth = (1.0 + (r - gap).max(0.0)).powi(10);
    let drag_factor = (direct_growth - regular_growth).max(0.0);
    value * drag_factor
}

/// Baseline CAGR for a mutual fund category (equity ~12%, debt ~7%).
pub fn baseline_cagr_for_category(category: Option<&str>) -> f64 {
    let cat = category.unwrap_or("").to_lowercase();
    if cat.contains("debt")
        || cat.contains("liquid")
        || cat.contains("gilt")
        || cat.contains("overnight")
        || cat.contains("money market")
    {
        0.07
    } else {
        0.12
    }
}

/// Analyze FIFO lots for a single regular scheme.
pub fn analyze_scheme_lots(
    lots: &[Lot],
    nav: f64,
    ter_gap_pct: f64,
    remaining_ltcg_exemption: f64,
    today: NaiveDate,
) -> (
    f64,
    f64,
    f64,
    f64,
    f64,
    Option<String>,
    Option<i64>,
    Option<f64>,
) {
    let mut units_eligible_ltcg = 0.0;
    let mut cost_eligible = 0.0;
    let mut units_locked_stcg = 0.0;
    let mut cost_locked = 0.0;
    let mut earliest_unlock: Option<NaiveDate> = None;

    for lot in lots {
        if lot.qty <= 0.0 {
            continue;
        }
        let buy_date = match NaiveDate::parse_from_str(&lot.date, "%Y-%m-%d") {
            Ok(d) => d,
            Err(_) => today,
        };
        let days = (today - buy_date).num_days();
        if days > 365 {
            units_eligible_ltcg += lot.qty;
            cost_eligible += lot.qty * lot.price;
        } else {
            units_locked_stcg += lot.qty;
            cost_locked += lot.qty * lot.price;
            let unlock = buy_date
                .checked_add_months(Months::new(12))
                .and_then(|d| d.succ_opt())
                .unwrap_or_else(|| buy_date + chrono::Duration::days(366));
            earliest_unlock = Some(match earliest_unlock {
                Some(existing) => existing.min(unlock),
                None => unlock,
            });
        }
    }

    let val_eligible = units_eligible_ltcg * nav;
    let ltcg_gain = (val_eligible - cost_eligible).max(0.0);

    let (tax_free_units, tax_free_value) = if units_eligible_ltcg <= 0.0 {
        (0.0, 0.0)
    } else if ltcg_gain <= remaining_ltcg_exemption || ltcg_gain <= 0.0 {
        (units_eligible_ltcg, val_eligible)
    } else {
        let ratio = (remaining_ltcg_exemption / ltcg_gain).clamp(0.0, 1.0);
        (
            units_eligible_ltcg * ratio,
            units_eligible_ltcg * ratio * nav,
        )
    };

    let (earliest_date_str, days_to_unlock) = match earliest_unlock {
        Some(date) => {
            let days = (date - today).num_days().max(0);
            (Some(date.format("%Y-%m-%d").to_string()), Some(days))
        }
        None => (None, None),
    };

    let breakeven_months = if units_locked_stcg > 0.0 && ter_gap_pct > 0.0 {
        let val_locked = units_locked_stcg * nav;
        let stcg_gain = (val_locked - cost_locked).max(0.0);
        let stcg_tax = stcg_gain * 0.20 * 1.04;
        let monthly_savings = (val_locked * (ter_gap_pct / 100.0)) / 12.0;
        if monthly_savings > 1e-4 && stcg_tax > 0.0 {
            Some(stcg_tax / monthly_savings)
        } else if stcg_tax <= 0.0 {
            Some(0.0)
        } else {
            None
        }
    } else {
        None
    };

    (
        units_eligible_ltcg,
        ltcg_gain,
        tax_free_units,
        tax_free_value,
        units_locked_stcg,
        earliest_date_str,
        days_to_unlock,
        breakeven_months,
    )
}

/// Compute Mutual Fund Bleed Report for the user's portfolio.
pub fn compute_mf_bleed(
    portfolio: &Portfolio,
    schemes: &[Scheme],
    ter_table: Option<&TerTable>,
    today: NaiveDate,
    remaining_ltcg_exemption: f64,
) -> MfBleedReport {
    let mut bleed_schemes = Vec::new();
    let mut total_regular_value = 0.0;
    let mut total_annual_bleed = 0.0;
    let mut total_10y_drag = 0.0;
    let mut total_tax_free_switch_value = 0.0;

    for holding in &portfolio.holdings {
        let fund_code: Option<u32> = if let Some(code_str) = holding.symbol.strip_prefix("MF:") {
            code_str.parse().ok()
        } else {
            None
        };

        let Some(code) = fund_code else {
            continue;
        };

        let Some(reg_scheme) = schemes.iter().find(|s| s.code == code) else {
            continue;
        };

        if reg_scheme.is_direct() {
            // Already direct; no commission bleed!
            continue;
        }

        let nav = reg_scheme
            .nav
            .filter(|n| *n > 0.0)
            .unwrap_or(holding.avg_cost);
        let value = holding.shares * nav;

        let direct_twin = find_direct_twin(reg_scheme, schemes);
        let direct_code = direct_twin.map(|d| d.code);
        let direct_name = direct_twin.map(|d| d.name.clone());

        // TER calculations
        let scheme_ter = ter_table.and_then(|t| t.for_scheme(reg_scheme));
        let direct_scheme_ter = direct_twin.and_then(|d| ter_table.and_then(|t| t.for_scheme(d)));

        let reg_ter_pct = scheme_ter
            .as_ref()
            .and_then(|t| t.regular.or(Some(t.costs.total)))
            .unwrap_or(0.0150)
            * 100.0;

        let direct_ter_pct = scheme_ter
            .as_ref()
            .and_then(|t| t.direct)
            .or_else(|| direct_scheme_ter.as_ref().map(|t| t.costs.total))
            .unwrap_or_else(|| (reg_ter_pct / 100.0 - 0.0075).max(0.0010))
            * 100.0;

        let ter_gap_pct = (reg_ter_pct - direct_ter_pct).max(0.0);
        let annual_bleed = value * (ter_gap_pct / 100.0);

        let baseline_cagr = baseline_cagr_for_category(reg_scheme.category.as_deref());
        let ten_year_drag = compounded_10y_drag(value, ter_gap_pct, baseline_cagr);

        let (
            units_eligible_ltcg,
            ltcg_on_eligible_units,
            tax_free_switch_units,
            tax_free_switch_value,
            units_locked_stcg,
            earliest_stcg_unlock_date,
            days_to_stcg_unlock,
            breakeven_months_early_switch,
        ) = analyze_scheme_lots(
            &holding.lots,
            nav,
            ter_gap_pct,
            remaining_ltcg_exemption,
            today,
        );

        total_regular_value += value;
        total_annual_bleed += annual_bleed;
        total_10y_drag += ten_year_drag;
        total_tax_free_switch_value += tax_free_switch_value;

        bleed_schemes.push(MfBleedScheme {
            regular_code: reg_scheme.code,
            regular_name: reg_scheme.name.clone(),
            direct_code,
            direct_name,
            units: holding.shares,
            current_nav: nav,
            current_value: value,
            regular_ter_pct: reg_ter_pct,
            direct_ter_pct,
            ter_gap_pct,
            annual_bleed,
            ten_year_drag,
            units_eligible_ltcg,
            ltcg_on_eligible_units,
            tax_free_switch_units,
            tax_free_switch_value,
            units_locked_stcg,
            earliest_stcg_unlock_date,
            days_to_stcg_unlock,
            breakeven_months_early_switch,
        });
    }

    MfBleedReport {
        total_regular_value,
        total_annual_bleed,
        total_10y_drag,
        total_tax_free_switch_value,
        schemes: bleed_schemes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn direct_twin_matches_growth_plan_by_base_name() {
        let schemes = vec![
            Scheme {
                code: 118955,
                name: "Nippon India Small Cap Fund - Regular Plan - Growth".into(),
                plan: Some("Regular".into()),
                option: Some("Growth".into()),
                nav: Some(180.0),
                date: Some(d("2026-11-08")),
                isin_growth: Some("INF179K01UT0".into()),
                isin_reinvest: None,
                amc: Some("Nippon India Mutual Fund".into()),
                category: Some("Equity Scheme - Small Cap Fund".into()),
                scheme_type: Some("Open Ended Schemes".into()),
            },
            Scheme {
                code: 118956,
                name: "Nippon India Small Cap Fund - Direct Plan - Growth".into(),
                plan: Some("Direct".into()),
                option: Some("Growth".into()),
                nav: Some(195.0),
                date: Some(d("2026-11-08")),
                isin_growth: Some("INF179K01UU8".into()),
                isin_reinvest: None,
                amc: Some("Nippon India Mutual Fund".into()),
                category: Some("Equity Scheme - Small Cap Fund".into()),
                scheme_type: Some("Open Ended Schemes".into()),
            },
        ];

        let direct = find_direct_twin(&schemes[0], &schemes).unwrap();
        assert_eq!(direct.code, 118956);
        assert!(direct.is_direct());
    }

    #[test]
    fn annual_bleed_and_ten_year_drag_computation() {
        let val = 100_000.0;
        let ter_gap = 1.0; // 1%
        let bleed = val * (ter_gap / 100.0);
        assert_eq!(bleed, 1000.0);

        let drag = compounded_10y_drag(val, ter_gap, 0.12);
        // Direct: (1.12)^10 = 3.105848
        // Regular: (1.11)^10 = 2.839420
        // Diff = ~0.266428 -> ~26,642 on 100,000
        assert!(drag > 26_000.0 && drag < 27_000.0, "drag was {drag}");
    }

    #[test]
    fn fifo_switch_with_partial_and_full_exemption() {
        let today = d("2026-11-08");
        let lots = vec![
            Lot::new(100.0, 100.0, "2024-01-01", "manual"), // > 1y: eligible
            Lot::new(50.0, 150.0, "2026-06-01", "manual"),  // < 1y: locked
        ];
        let nav = 200.0;
        let ter_gap = 0.8;
        let rem_exemption = 125_000.0;

        let (
            units_lt,
            ltcg_gain,
            tax_free_units,
            tax_free_val,
            units_st,
            earliest_unlock,
            days_unlock,
            breakeven,
        ) = analyze_scheme_lots(&lots, nav, ter_gap, rem_exemption, today);

        assert_eq!(units_lt, 100.0);
        assert_eq!(ltcg_gain, 10_000.0); // 100 * (200 - 100)
        assert_eq!(tax_free_units, 100.0);
        assert_eq!(tax_free_val, 20_000.0);
        assert_eq!(units_st, 50.0);
        assert!(earliest_unlock.is_some());
        assert!(days_unlock.unwrap() > 0);
        assert!(breakeven.is_some());
    }
}
