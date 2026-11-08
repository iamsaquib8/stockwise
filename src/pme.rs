//! Public Market Equivalent (PME) / Index Twin analysis.
//!
//! Evaluates whether an investor's active trades, stock picks, and timing beat
//! putting every dollar/rupee into a passive index fund or ETF on the exact same
//! trade dates.
//!
//! Key metrics:
//! - **Index Twin Value**: Terminal value of the benchmark portfolio had every
//!   buy purchased benchmark shares, and every sell/dividend sold/distributed
//!   benchmark shares on the exact same dates.
//! - **Value Added / Lost**: Terminal actual value minus Index Twin value
//!   (in exact base currency units: +$ / +₹).
//! - **Kaplan-Schoar PME (KS-PME)**: Future value of distributions + terminal
//!   value divided by future value of investments, compounded by the benchmark return.
//!   `> 1.0` indicates active outperformance; `1.0` is parity.
//! - **Direct Alpha (Gredil, Griffiths, Stucke 2014)**: Annualized IRR of the
//!   benchmark-discounted cash flow series. `0.00%` indicates exact parity with the
//!   benchmark.
//! - **Portfolio XIRR vs Benchmark Twin XIRR**: Annualized money-weighted returns.
//! - **Holding-Level Attribution**: Direct Alpha and Value Added breakdown by holding.

use chrono::NaiveDate;
use serde::Serialize;

/// Lookup the benchmark price on or immediately preceding `date`.
pub fn benchmark_price_on(closes: &[(NaiveDate, f64)], date: NaiveDate) -> Option<f64> {
    if closes.is_empty() {
        return None;
    }
    if date <= closes[0].0 {
        return (closes[0].1.is_finite() && closes[0].1 > 0.0).then_some(closes[0].1);
    }
    let p = match closes.binary_search_by_key(&date, |(d, _)| *d) {
        Ok(idx) => closes[idx].1,
        Err(idx) => {
            if idx > 0 {
                closes[idx - 1].1
            } else {
                closes[0].1
            }
        }
    };
    (p.is_finite() && p > 0.0).then_some(p)
}

/// Accumulated shares and terminal value of the benchmark "Index Twin".
/// `flows`: `(date, amount)` in base currency where investments/buys are negative
/// and sales/dividends are positive.
pub fn twin_shares_and_value(
    flows: &[(NaiveDate, f64)],
    benchmark_closes: &[(NaiveDate, f64)],
    today: NaiveDate,
) -> Option<(f64, f64)> {
    let end_price = benchmark_price_on(benchmark_closes, today)?;
    let mut twin_shares = 0.0;
    for &(date, amount) in flows {
        let p = benchmark_price_on(benchmark_closes, date)?;
        // Negative amount (buy) buys benchmark shares (+); positive amount (sell/div) sells (-).
        twin_shares += -amount / p;
    }
    let twin_value = twin_shares * end_price;
    Some((twin_shares, twin_value))
}

/// Kaplan-Schoar PME ratio:
/// Future value of distributions + terminal portfolio value, divided by
/// future value of investments (all compounded at the benchmark's return).
///
/// `> 1.0` means active outperformance; `1.0` is parity.
pub fn kaplan_schoar_pme(
    flows: &[(NaiveDate, f64)],
    actual_value: f64,
    benchmark_closes: &[(NaiveDate, f64)],
    today: NaiveDate,
) -> Option<f64> {
    let end_price = benchmark_price_on(benchmark_closes, today)?;
    let mut fv_distributions = 0.0;
    let mut fv_investments = 0.0;

    for &(date, amount) in flows {
        let p = benchmark_price_on(benchmark_closes, date)?;
        let factor = end_price / p;
        if amount > 0.0 {
            // Distribution, sale proceed, or dividend
            fv_distributions += amount * factor;
        } else if amount < 0.0 {
            // Investment / buy
            fv_investments += -amount * factor;
        }
    }

    if fv_investments <= 1e-9 {
        return None;
    }

    let pme = (fv_distributions + actual_value) / fv_investments;
    if pme.is_finite() && pme >= 0.0 {
        Some(pme)
    } else {
        None
    }
}

/// Direct Alpha (Gredil, Griffiths, Stucke 2014):
/// Annualized excess return rate solved as the XIRR of cash flows discounted
/// by the benchmark's cumulative return to the portfolio start date.
///
/// An active portfolio matching the benchmark produces exactly `0.00%`.
pub fn direct_alpha(
    flows: &[(NaiveDate, f64)],
    actual_value: f64,
    benchmark_closes: &[(NaiveDate, f64)],
    today: NaiveDate,
) -> Option<f64> {
    let first = flows.iter().map(|f| f.0).min()?;
    let span = (today - first).num_days();
    if span < 30 {
        return None;
    }

    let start_price = benchmark_price_on(benchmark_closes, first)?;
    let end_price = benchmark_price_on(benchmark_closes, today)?;

    let years = |d: NaiveDate| (d - first).num_days() as f64 / 365.0;
    let mut discounted: Vec<(f64, f64)> = Vec::with_capacity(flows.len() + 1);

    let mut sum_discounted = 0.0;
    for &(date, amount) in flows {
        let p = benchmark_price_on(benchmark_closes, date)?;
        // Discount factor relative to benchmark start price
        let cf_star = amount * (start_price / p);
        sum_discounted += cf_star;
        discounted.push((years(date), cf_star));
    }

    let terminal_star = actual_value * (start_price / end_price);
    sum_discounted += terminal_star;
    discounted.push((years(today), terminal_star));

    // If sum of discounted cash flows is zero within float precision,
    // NPV(0.0) == 0 so the internal rate of return is exactly 0.0%.
    if sum_discounted.abs() < 1e-6 {
        return Some(0.0);
    }

    // Solve for IRR of discounted cash flows
    crate::technical::xirr(&discounted)
}

/// PME metrics for a single holding or the entire portfolio.
#[derive(Debug, Clone, Serialize)]
pub struct PmeResult {
    /// Actual current terminal value
    pub actual_value: f64,
    /// Value had the cash flows been invested in the benchmark twin
    pub twin_value: f64,
    /// Actual value minus twin value (+Alpha / -Drag in currency units)
    pub value_added: f64,
    /// Kaplan-Schoar PME (> 1.0 outperformance, 1.0 parity)
    pub pme_ratio: Option<f64>,
    /// Direct Alpha annualized percentage (e.g. 3.25 = +3.25% p.a.)
    pub direct_alpha_percent: Option<f64>,
    /// Actual portfolio annualized money-weighted return (XIRR %)
    pub actual_xirr_percent: Option<f64>,
    /// Benchmark twin annualized money-weighted return (XIRR %)
    pub twin_xirr_percent: Option<f64>,
}

/// Compute PME metrics for a stream of dated cash flows and ending value.
pub fn compute_pme(
    flows: &[(NaiveDate, f64)],
    actual_value: f64,
    benchmark_closes: &[(NaiveDate, f64)],
    today: NaiveDate,
) -> Option<PmeResult> {
    if flows.is_empty() {
        return None;
    }
    let (_, twin_value) = twin_shares_and_value(flows, benchmark_closes, today)?;
    let value_added = actual_value - twin_value;
    let pme_ratio = kaplan_schoar_pme(flows, actual_value, benchmark_closes, today);
    let direct_alpha_percent =
        direct_alpha(flows, actual_value, benchmark_closes, today).map(|a| a * 100.0);
    let actual_xirr_percent = crate::portfolio::xirr(flows, actual_value, today).map(|r| r * 100.0);
    let twin_xirr_percent = crate::portfolio::xirr(flows, twin_value, today).map(|r| r * 100.0);

    Some(PmeResult {
        actual_value,
        twin_value,
        value_added,
        pme_ratio,
        direct_alpha_percent,
        actual_xirr_percent,
        twin_xirr_percent,
    })
}

/// Attribution for a single holding compared against the benchmark twin.
#[derive(Debug, Clone, Serialize)]
pub struct HoldingPme {
    pub symbol: String,
    pub actual_value: f64,
    pub twin_value: f64,
    pub value_added: f64,
    pub pme_ratio: Option<f64>,
    pub direct_alpha_percent: Option<f64>,
    pub actual_xirr_percent: Option<f64>,
    pub twin_xirr_percent: Option<f64>,
    pub cashflows_count: usize,
}

/// Summary of the whole-portfolio Public Market Equivalent analysis.
#[derive(Debug, Clone, Serialize)]
pub struct PmeSummary {
    pub benchmark_symbol: String,
    pub benchmark_name: String,
    pub base_currency: String,
    pub start_date: String,
    pub end_date: String,
    pub total_invested: f64,
    pub total_distributed: f64,
    pub actual_value: f64,
    pub twin_value: f64,
    pub value_added: f64,
    pub kaplan_schoar_pme: Option<f64>,
    pub direct_alpha_percent: Option<f64>,
    pub portfolio_xirr_percent: Option<f64>,
    pub twin_xirr_percent: Option<f64>,
    pub twin_shares: f64,
    pub benchmark_end_price: f64,
}

/// Complete report of the Public Market Equivalent (Index Twin) analysis.
#[derive(Debug, Clone, Serialize)]
pub struct PmeReport {
    pub summary: PmeSummary,
    pub holdings: Vec<HoldingPme>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn benchmark_price_lookup_handles_exact_and_interpolated_dates() {
        let closes = vec![
            (d("2024-01-02"), 100.0),
            (d("2024-01-03"), 102.0),
            (d("2024-01-05"), 105.0),
        ];
        // Before first
        assert_eq!(benchmark_price_on(&closes, d("2024-01-01")), Some(100.0));
        // Exact matches
        assert_eq!(benchmark_price_on(&closes, d("2024-01-02")), Some(100.0));
        assert_eq!(benchmark_price_on(&closes, d("2024-01-03")), Some(102.0));
        // Weekend or gap (Jan 4 takes Jan 3)
        assert_eq!(benchmark_price_on(&closes, d("2024-01-04")), Some(102.0));
        // After last
        assert_eq!(benchmark_price_on(&closes, d("2024-01-06")), Some(105.0));
    }

    #[test]
    fn identical_cashflows_produce_pme_one_and_zero_direct_alpha() {
        let closes = vec![
            (d("2024-01-01"), 100.0),
            (d("2024-06-01"), 110.0),
            (d("2025-01-01"), 120.0),
        ];
        let flows = vec![(d("2024-01-01"), -1000.0)];
        let today = d("2025-01-01");
        let actual_value = 1200.0;

        let (shares, twin_val) = twin_shares_and_value(&flows, &closes, today).unwrap();
        assert_eq!(shares, 10.0);
        assert_eq!(twin_val, 1200.0);

        let pme = kaplan_schoar_pme(&flows, actual_value, &closes, today).unwrap();
        assert!((pme - 1.000).abs() < 1e-6, "PME should be 1.000, got {pme}");

        let alpha = direct_alpha(&flows, actual_value, &closes, today).unwrap();
        assert!(
            (alpha - 0.00).abs() < 1e-6,
            "Direct Alpha should be 0.00%, got {alpha}"
        );

        let res = compute_pme(&flows, actual_value, &closes, today).unwrap();
        assert_eq!(res.value_added, 0.0);
        assert!((res.pme_ratio.unwrap() - 1.000).abs() < 1e-6);
        assert!((res.direct_alpha_percent.unwrap() - 0.00).abs() < 1e-4);
        assert!((res.actual_xirr_percent.unwrap() - res.twin_xirr_percent.unwrap()).abs() < 1e-4);
    }

    #[test]
    fn outperformance_produces_positive_alpha_and_pme_above_one() {
        let closes = vec![(d("2024-01-01"), 100.0), (d("2025-01-01"), 120.0)];
        let flows = vec![(d("2024-01-01"), -1000.0)];
        let today = d("2025-01-01");
        let actual_value = 1350.0;

        let res = compute_pme(&flows, actual_value, &closes, today).unwrap();
        assert_eq!(res.twin_value, 1200.0);
        assert_eq!(res.value_added, 150.0);
        assert!(res.pme_ratio.unwrap() > 1.0);
        assert_eq!(res.pme_ratio.unwrap(), 1350.0 / 1200.0);
        assert!(res.direct_alpha_percent.unwrap() > 0.0);
        assert!(res.actual_xirr_percent.unwrap() > res.twin_xirr_percent.unwrap());
    }

    #[test]
    fn underperformance_produces_negative_alpha_and_pme_below_one() {
        let closes = vec![(d("2024-01-01"), 100.0), (d("2025-01-01"), 120.0)];
        let flows = vec![(d("2024-01-01"), -1000.0)];
        let today = d("2025-01-01");
        let actual_value = 1100.0;

        let res = compute_pme(&flows, actual_value, &closes, today).unwrap();
        assert_eq!(res.twin_value, 1200.0);
        assert_eq!(res.value_added, -100.0);
        assert!(res.pme_ratio.unwrap() < 1.0);
        assert!(res.direct_alpha_percent.unwrap() < 0.0);
        assert!(res.actual_xirr_percent.unwrap() < res.twin_xirr_percent.unwrap());
    }

    #[test]
    fn pme_with_staggered_buys_and_dividends_maintains_parity() {
        let closes = vec![
            (d("2024-01-01"), 100.0),
            (d("2024-04-01"), 105.0),
            (d("2024-07-01"), 110.0),
            (d("2025-01-01"), 120.0),
        ];
        let today = d("2025-01-01");
        let flows = vec![
            (d("2024-01-01"), -1000.0),
            (d("2024-04-01"), -525.0),
            (d("2024-07-01"), 110.0),
        ];
        let (shares, twin_val) = twin_shares_and_value(&flows, &closes, today).unwrap();
        assert_eq!(shares, 14.0);
        assert_eq!(twin_val, 1680.0);

        let pme = kaplan_schoar_pme(&flows, 1680.0, &closes, today).unwrap();
        assert!(
            (pme - 1.000).abs() < 1e-6,
            "Staggered flows should produce PME 1.000, got {pme}"
        );

        let alpha = direct_alpha(&flows, 1680.0, &closes, today).unwrap();
        assert!(
            (alpha - 0.00).abs() < 1e-5,
            "Staggered flows should produce Direct Alpha 0.00%, got {alpha}"
        );
    }
}
