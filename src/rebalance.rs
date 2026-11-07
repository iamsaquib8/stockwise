//! Portfolio rebalancing engine: target-allocation weights, drift bands, and
//! inflow-only ("buy-only") rebalancing.
//!
//! Provides core mathematical algorithms and data structures to:
//! 1. Resolve and normalize custom target weights by symbol and sector.
//! 2. Evaluate drift tolerance bands to eliminate unnecessary portfolio churn.
//! 3. Calculate optimal buy-only allocations for fresh capital inflows (SIPs, bonuses)
//!    using water-filling, guaranteeing no sell orders are issued.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Small epsilon for floating-point comparisons.
const EPS: f64 = 1e-9;

/// Singular or plural noun for a whole-share order.
fn shares_word(shares: f64) -> &'static str {
    if (shares - 1.0).abs() < 1e-9 {
        "share"
    } else {
        "shares"
    }
}

/// Single holding's market data passed into the rebalancer.
#[derive(Debug, Clone, PartialEq)]
pub struct HoldingInput {
    pub symbol: String,
    pub shares: f64,
    pub value: f64,
    pub price: f64,
    pub sector: Option<String>,
    pub is_fund: bool,
}

/// Action classification for rebalancing orders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RebalanceActionType {
    Buy,
    Sell,
    Ok,
}

/// One row in the rebalance plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebalanceItem {
    pub symbol: String,
    pub current_value: f64,
    pub current_pct: f64,
    pub target_pct: f64,
    pub projected_value: f64,
    pub projected_pct: f64,
    pub price: f64,
    pub diff_value: f64,
    pub diff_shares: f64,
    pub action_type: RebalanceActionType,
    pub action_display: String,
    pub is_fund: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sector: Option<String>,
}

/// Complete rebalance recommendation plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RebalancePlan {
    pub total_value: f64,
    pub projected_value: f64,
    pub currency: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inflow: Option<f64>,
    pub drift_tolerance: f64,
    pub items: Vec<RebalanceItem>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl RebalancePlan {
    pub fn empty(currency: impl Into<String>) -> Self {
        Self {
            total_value: 0.0,
            projected_value: 0.0,
            currency: currency.into(),
            inflow: None,
            drift_tolerance: 0.0,
            items: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

/// Parse a target weight specification string: `KEY:PERCENT` or `KEY=PERCENT`
/// (e.g., `TCS.NS:25`, `Technology=30%`, `INFY: 20.5`).
pub fn parse_target(s: &str) -> Result<(String, f64)> {
    let s = s.trim();
    let (key, val) = s
        .split_once(':')
        .or_else(|| s.split_once('='))
        .context("Target must be in KEY:PERCENTAGE or KEY=PERCENTAGE format (e.g. TCS:25)")?;
    let key = key.trim().to_string();
    if key.is_empty() {
        bail!("Target key cannot be empty in '{}'", s);
    }
    let val_str = val.trim().trim_end_matches('%');
    let pct: f64 = val_str
        .parse()
        .with_context(|| format!("Invalid percentage '{}' in target '{}'", val.trim(), s))?;
    if !(0.0..=100.0).contains(&pct) {
        bail!(
            "Target percentage must be between 0 and 100, got {} in '{}'",
            pct,
            s
        );
    }
    Ok((key, pct))
}

/// Parse multiple comma-separated or space-separated target arguments.
pub fn parse_targets(inputs: &[String]) -> Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    for input in inputs {
        for part in input.split(',') {
            let part = part.trim();
            if !part.is_empty() {
                out.push(parse_target(part)?);
            }
        }
    }
    Ok(out)
}

/// Check if a target key matches a holding symbol (case-insensitive, bare or suffixed).
fn matches_symbol(target_key: &str, symbol: &str) -> bool {
    if target_key.eq_ignore_ascii_case(symbol) {
        return true;
    }
    // Match bare key e.g. "TCS" matches "TCS.NS" or "TCS.BO"
    if symbol
        .split_once('.')
        .is_some_and(|(bare, _)| target_key.eq_ignore_ascii_case(bare))
    {
        return true;
    }
    // Match if key has suffix and symbol is bare: "TCS.NS" matches "TCS"
    if target_key
        .split_once('.')
        .is_some_and(|(bare, _)| bare.eq_ignore_ascii_case(symbol))
    {
        return true;
    }
    false
}

/// Resolve and normalize target weights for all holdings.
///
/// Precedence:
/// 1. Explicit symbol targets.
/// 2. Sector targets distributed pro-rata across holdings in that sector.
/// 3. Equal-weight distribution of remaining percentage for untargeted holdings.
/// 4. If targets sum != 100%, normalize to 100% and generate informational warnings.
pub fn resolve_target_weights(
    holdings: &[HoldingInput],
    configured: &BTreeMap<String, f64>,
) -> (Vec<f64>, Vec<String>) {
    if holdings.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let mut warnings = Vec::new();
    let mut targets: Vec<Option<f64>> = vec![None; holdings.len()];

    // 1. Match explicit symbol targets first
    for (i, h) in holdings.iter().enumerate() {
        for (key, &pct) in configured {
            if matches_symbol(key, &h.symbol) {
                targets[i] = Some(pct);
                break;
            }
        }
    }

    // 2. Match sector targets for holdings without explicit symbol targets
    let mut sector_targets: BTreeMap<String, f64> = BTreeMap::new();
    for (key, &pct) in configured {
        // If not already consumed as a symbol match
        let is_sym = holdings.iter().any(|h| matches_symbol(key, &h.symbol));
        if !is_sym {
            sector_targets.insert(key.to_lowercase(), pct);
        }
    }

    for (sec_key, &sec_pct) in &sector_targets {
        let matching_indices: Vec<usize> = holdings
            .iter()
            .enumerate()
            .filter(|(i, h)| {
                targets[*i].is_none()
                    && h.sector
                        .as_deref()
                        .map(|s| s.to_lowercase() == *sec_key)
                        .unwrap_or(false)
            })
            .map(|(i, _)| i)
            .collect();

        if !matching_indices.is_empty() {
            let total_sector_val: f64 = matching_indices
                .iter()
                .map(|&idx| holdings[idx].value)
                .sum();

            for &idx in &matching_indices {
                let weight = if total_sector_val > EPS {
                    sec_pct * (holdings[idx].value / total_sector_val)
                } else {
                    sec_pct / matching_indices.len() as f64
                };
                targets[idx] = Some(weight);
            }
        }
    }

    // 3. Handle unassigned holdings
    let unassigned_indices: Vec<usize> = targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.is_none())
        .map(|(i, _)| i)
        .collect();

    let assigned_sum: f64 = targets.iter().flatten().sum();

    if unassigned_indices.is_empty() {
        // All holdings have targets
        if (assigned_sum - 100.0).abs() > 0.01 && assigned_sum > EPS {
            warnings.push(format!(
                "Configured targets sum to {:.1}%; normalized to 100.0%",
                assigned_sum
            ));
            let factor = 100.0 / assigned_sum;
            for val in targets.iter_mut().flatten() {
                *val *= factor;
            }
        }
    } else if assigned_sum < 100.0 - EPS {
        // Remaining weight distributed equally among untargeted holdings
        let remaining = 100.0 - assigned_sum;
        let per_unassigned = remaining / unassigned_indices.len() as f64;
        if assigned_sum > EPS {
            warnings.push(format!(
                "Configured targets sum to {:.1}%; remaining {:.1}% allocated equally among {} untargeted holding(s)",
                assigned_sum,
                remaining,
                unassigned_indices.len()
            ));
        }
        for idx in unassigned_indices {
            targets[idx] = Some(per_unassigned);
        }
    } else {
        // Configured targets already meet or exceed 100%
        if (assigned_sum - 100.0).abs() > 0.01 {
            warnings.push(format!(
                "Configured targets sum to {:.1}% (exceeds 100%); normalized among targeted holdings, {} untargeted set to 0.0%",
                assigned_sum,
                unassigned_indices.len()
            ));
            let factor = 100.0 / assigned_sum;
            for val in targets.iter_mut().flatten() {
                *val *= factor;
            }
        } else {
            warnings.push(format!(
                "Configured targets sum to 100.0%; {} untargeted holding(s) set to 0.0% target",
                unassigned_indices.len()
            ));
        }
        for idx in unassigned_indices {
            targets[idx] = Some(0.0);
        }
    }

    // Unwrap targets, defaulting to equal weight if somehow still None
    let equal = 100.0 / holdings.len() as f64;
    let final_targets: Vec<f64> = targets.into_iter().map(|t| t.unwrap_or(equal)).collect();
    (final_targets, warnings)
}

/// Calculate optimal buy-only cash allocation from fresh capital inflow.
///
/// Distributes `inflow` across underweight holdings to minimize quadratic deviation
/// from target weights without issuing any sell orders (inflow-only / buy-only mode).
///
/// Solves:
///   min sum_i ((v_i + c_i) / V_new - t_i)^2
///   subject to: c_i >= 0, sum_i c_i = inflow
///
/// Uses continuous water-filling over underweight assets.
pub fn compute_inflow_allocation(
    holdings: &[HoldingInput],
    target_pcts: &[f64],
    inflow: f64,
) -> Result<Vec<f64>> {
    if inflow <= 0.0 {
        bail!("Inflow amount must be positive, got {:.2}", inflow);
    }
    if holdings.is_empty() {
        return Ok(Vec::new());
    }

    let total_current: f64 = holdings.iter().map(|h| h.value).sum();
    let projected_total = total_current + inflow;

    // Ideal target values in the new portfolio
    let ideal_targets: Vec<f64> = target_pcts
        .iter()
        .map(|&pct| projected_total * (pct / 100.0))
        .collect();

    // Deficits to reach ideal targets
    let deficits: Vec<f64> = holdings
        .iter()
        .zip(&ideal_targets)
        .map(|(h, &tgt)| (tgt - h.value).max(0.0))
        .collect();

    let total_deficit: f64 = deficits.iter().sum();

    if inflow >= total_deficit - EPS {
        // Full deficit can be covered for every underweight asset!
        let mut allocations = deficits;
        let surplus = inflow - total_deficit;
        if surplus > EPS {
            // Distribute remaining surplus proportionally to targets for all holdings with target > 0
            let target_sum: f64 = target_pcts.iter().filter(|&&t| t > 0.0).sum();
            if target_sum > EPS {
                for (i, alloc) in allocations.iter_mut().enumerate() {
                    if target_pcts[i] > 0.0 {
                        *alloc += surplus * (target_pcts[i] / target_sum);
                    }
                }
            } else {
                let share = surplus / holdings.len() as f64;
                for alloc in &mut allocations {
                    *alloc += share;
                }
            }
        }
        return Ok(allocations);
    }

    // Inflow is less than total deficit: water-fill to bring the most underweight
    // assets up to a uniform target scale factor lambda in [0.0, 1.0].
    let mut low = 0.0;
    let mut high = 1.0;
    for _ in 0..60 {
        let mid = (low + high) / 2.0;
        let sum_alloc: f64 = holdings
            .iter()
            .zip(&ideal_targets)
            .map(|(h, &tgt)| (mid * tgt - h.value).max(0.0))
            .sum();
        if sum_alloc < inflow {
            low = mid;
        } else {
            high = mid;
        }
    }

    let lambda = low;
    let mut allocations: Vec<f64> = holdings
        .iter()
        .zip(&ideal_targets)
        .map(|(h, &tgt)| (lambda * tgt - h.value).max(0.0))
        .collect();

    // Reconcile minor floating-point difference
    let sum_alloc: f64 = allocations.iter().sum();
    let diff = inflow - sum_alloc;
    let best_deficit = deficits
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1));
    if let Some((max_idx, _)) = best_deficit
        && diff.abs() > EPS
    {
        allocations[max_idx] = (allocations[max_idx] + diff).max(0.0);
    }

    Ok(allocations)
}

/// Build the full rebalance recommendation plan for a portfolio.
pub fn compute_rebalance_plan(
    holdings: &[HoldingInput],
    configured_targets: &BTreeMap<String, f64>,
    inflow: Option<f64>,
    drift_tolerance: f64,
    currency: &str,
) -> Result<RebalancePlan> {
    if holdings.is_empty() {
        return Ok(RebalancePlan::empty(currency));
    }

    let (target_pcts, warnings) = resolve_target_weights(holdings, configured_targets);
    let total_value: f64 = holdings.iter().map(|h| h.value).sum();
    let currency_sym = crate::market::currency_symbol(Some(currency));

    let (projected_value, allocations) = if let Some(cash) = inflow {
        if cash <= 0.0 {
            bail!("Inflow amount must be positive, got {:.2}", cash);
        }
        let allocs = compute_inflow_allocation(holdings, &target_pcts, cash)?;
        (total_value + cash, Some(allocs))
    } else {
        (total_value, None)
    };

    let mut items = Vec::with_capacity(holdings.len());

    for (i, h) in holdings.iter().enumerate() {
        let cur_val = h.value;
        let cur_pct = if total_value > EPS {
            (cur_val / total_value) * 100.0
        } else {
            0.0
        };
        let tgt_pct = target_pcts[i];

        if let Some(ref allocs) = allocations {
            // Inflow-only mode: buy-only recommendations
            let alloc_cash = allocs[i];
            let proj_val = cur_val + alloc_cash;
            let proj_pct = if projected_value > EPS {
                (proj_val / projected_value) * 100.0
            } else {
                0.0
            };
            let diff_val = alloc_cash;

            let (action_type, action_display, diff_shares) = if alloc_cash < 1.0 || h.price <= EPS {
                (RebalanceActionType::Ok, "OK".to_string(), 0.0)
            } else if h.is_fund {
                let shares = alloc_cash / h.price;
                (
                    RebalanceActionType::Buy,
                    format!("BUY {:.3} units", shares),
                    shares,
                )
            } else {
                let raw_shares = alloc_cash / h.price;
                let shares = if (raw_shares.round() - raw_shares).abs() < 1e-5 {
                    raw_shares.round()
                } else {
                    raw_shares.floor()
                };
                if shares >= 1.0 {
                    (
                        RebalanceActionType::Buy,
                        format!("BUY {:.0} shares", shares),
                        shares,
                    )
                } else {
                    (
                        RebalanceActionType::Ok,
                        format!("OK (< 1 sh: {}{:.0})", currency_sym, alloc_cash),
                        0.0,
                    )
                }
            };

            items.push(RebalanceItem {
                symbol: h.symbol.clone(),
                current_value: cur_val,
                current_pct: cur_pct,
                target_pct: tgt_pct,
                projected_value: proj_val,
                projected_pct: proj_pct,
                price: h.price,
                diff_value: diff_val,
                diff_shares,
                action_type,
                action_display,
                is_fund: h.is_fund,
                sector: h.sector.clone(),
            });
        } else {
            // Standard rebalancing with drift band tolerance
            let target_val = total_value * (tgt_pct / 100.0);
            let diff_val = target_val - cur_val;
            let drift = cur_pct - tgt_pct;

            // In drift band or diff < 1 currency unit
            let (action_type, action_display, diff_shares) =
                if drift.abs() <= drift_tolerance || diff_val.abs() < 1.0 || h.price <= EPS {
                    (RebalanceActionType::Ok, "OK".to_string(), 0.0)
                } else if diff_val > 0.0 {
                    // Underweight -> Buy
                    if h.is_fund {
                        let shares = diff_val / h.price;
                        (
                            RebalanceActionType::Buy,
                            format!("BUY {:.3} units", shares),
                            shares,
                        )
                    } else {
                        let raw_shares = diff_val / h.price;
                        let shares = if (raw_shares.round() - raw_shares).abs() < 1e-5 {
                            raw_shares.round()
                        } else {
                            raw_shares.ceil()
                        };
                        (
                            RebalanceActionType::Buy,
                            format!("BUY {:.0} {}", shares, shares_word(shares)),
                            shares,
                        )
                    }
                } else {
                    // Overweight -> Sell
                    if h.is_fund {
                        let shares = diff_val.abs() / h.price;
                        (
                            RebalanceActionType::Sell,
                            format!("SELL {:.3} units", shares),
                            shares,
                        )
                    } else {
                        let raw_shares = diff_val.abs() / h.price;
                        let shares = if (raw_shares.round() - raw_shares).abs() < 1e-5 {
                            raw_shares.round()
                        } else {
                            raw_shares.floor()
                        };
                        // A position less than one share over target cannot be
                        // sold, so report it as OK instead of "SELL 0 shares".
                        if shares < 1.0 {
                            (
                                RebalanceActionType::Ok,
                                format!("OK (< 1 sh over: {}{:.0})", currency_sym, diff_val.abs()),
                                0.0,
                            )
                        } else {
                            (
                                RebalanceActionType::Sell,
                                format!("SELL {:.0} {}", shares, shares_word(shares)),
                                shares,
                            )
                        }
                    }
                };

            items.push(RebalanceItem {
                symbol: h.symbol.clone(),
                current_value: cur_val,
                current_pct: cur_pct,
                target_pct: tgt_pct,
                projected_value: target_val,
                projected_pct: tgt_pct,
                price: h.price,
                diff_value: diff_val,
                diff_shares,
                action_type,
                action_display,
                is_fund: h.is_fund,
                sector: h.sector.clone(),
            });
        }
    }

    Ok(RebalancePlan {
        total_value,
        projected_value,
        currency: currency.to_string(),
        inflow,
        drift_tolerance,
        items,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_holding(symbol: &str, shares: f64, price: f64, sector: Option<&str>) -> HoldingInput {
        HoldingInput {
            symbol: symbol.to_string(),
            shares,
            value: shares * price,
            price,
            sector: sector.map(String::from),
            is_fund: symbol.starts_with("MF:"),
        }
    }

    #[test]
    fn parse_target_valid_formats() {
        assert_eq!(parse_target("TCS:25").unwrap(), ("TCS".to_string(), 25.0));
        assert_eq!(
            parse_target("RELIANCE.NS: 35.5%").unwrap(),
            ("RELIANCE.NS".to_string(), 35.5)
        );
        assert_eq!(
            parse_target("Technology=40").unwrap(),
            ("Technology".to_string(), 40.0)
        );
        assert_eq!(
            parse_target("Financial Services: 20%").unwrap(),
            ("Financial Services".to_string(), 20.0)
        );
    }

    #[test]
    fn parse_target_invalid_formats() {
        assert!(parse_target("TCS").is_err());
        assert!(parse_target("TCS:105").is_err());
        assert!(parse_target("TCS:-5").is_err());
        assert!(parse_target(":25").is_err());
        assert!(parse_target("TCS:abc").is_err());
    }

    #[test]
    fn parse_targets_multiple_inputs() {
        let inputs = vec!["TCS:20, INFY:30".to_string(), "RELIANCE:50".to_string()];
        let res = parse_targets(&inputs).unwrap();
        assert_eq!(res.len(), 3);
        assert_eq!(res[0], ("TCS".to_string(), 20.0));
        assert_eq!(res[1], ("INFY".to_string(), 30.0));
        assert_eq!(res[2], ("RELIANCE".to_string(), 50.0));
    }

    #[test]
    fn target_normalization_when_sum_differs_from_100() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 3000.0, Some("Technology")),
            sample_holding("INFY.NS", 20.0, 1500.0, Some("Technology")),
        ];
        let mut targets = BTreeMap::new();
        targets.insert("TCS.NS".to_string(), 60.0);
        targets.insert("INFY.NS".to_string(), 60.0); // Sum = 120%

        let (resolved, warnings) = resolve_target_weights(&holdings, &targets);
        assert_eq!(resolved.len(), 2);
        assert!((resolved[0] - 50.0).abs() < 1e-6);
        assert!((resolved[1] - 50.0).abs() < 1e-6);
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("normalized to 100.0%"));
    }

    #[test]
    fn partial_targets_fill_untargeted_holdings_equally() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 3000.0, Some("Technology")),
            sample_holding("INFY.NS", 20.0, 1500.0, Some("Technology")),
            sample_holding("HDFCBANK.NS", 30.0, 1500.0, Some("Financial Services")),
        ];
        let mut targets = BTreeMap::new();
        targets.insert("TCS.NS".to_string(), 50.0); // 50% explicit, remaining 50% split 25% & 25%

        let (resolved, warnings) = resolve_target_weights(&holdings, &targets);
        assert_eq!(resolved.len(), 3);
        assert!((resolved[0] - 50.0).abs() < 1e-6);
        assert!((resolved[1] - 25.0).abs() < 1e-6);
        assert!((resolved[2] - 25.0).abs() < 1e-6);
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("allocated equally"));
    }

    #[test]
    fn sector_target_distributed_pro_rata() {
        let holdings = vec![
            sample_holding("TCS.NS", 10.0, 1000.0, Some("Technology")), // Value = 10,000 (25% of Tech)
            sample_holding("INFY.NS", 30.0, 1000.0, Some("Technology")), // Value = 30,000 (75% of Tech)
            sample_holding("RELIANCE.NS", 20.0, 2000.0, Some("Energy")), // Value = 40,000
        ];
        let mut targets = BTreeMap::new();
        targets.insert("Technology".to_string(), 40.0);
        targets.insert("Energy".to_string(), 60.0);

        let (resolved, _) = resolve_target_weights(&holdings, &targets);
        // Tech is 40% total: TCS gets 25% of 40% = 10%, INFY gets 75% of 40% = 30%
        assert!((resolved[0] - 10.0).abs() < 1e-6);
        assert!((resolved[1] - 30.0).abs() < 1e-6);
        assert!((resolved[2] - 60.0).abs() < 1e-6);
    }

    #[test]
    fn drift_tolerance_filters_small_movements() {
        let holdings = vec![
            sample_holding("STOCK_A", 100.0, 51.0, None), // Value: 5100 (51.0%)
            sample_holding("STOCK_B", 100.0, 49.0, None), // Value: 4900 (49.0%)
        ];
        let mut targets = BTreeMap::new();
        targets.insert("STOCK_A".to_string(), 50.0);
        targets.insert("STOCK_B".to_string(), 50.0);

        // Drift band = 2.0% -> 51% and 49% are both within +/-2.0%
        let plan = compute_rebalance_plan(&holdings, &targets, None, 2.0, "INR").unwrap();
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Ok);
        assert_eq!(plan.items[1].action_type, RebalanceActionType::Ok);

        // Drift band = 0.5% -> 51% has drift 1.0% > 0.5% -> triggers SELL / BUY
        let plan_strict = compute_rebalance_plan(&holdings, &targets, None, 0.5, "INR").unwrap();
        assert_eq!(plan_strict.items[0].action_type, RebalanceActionType::Sell);
        assert_eq!(plan_strict.items[1].action_type, RebalanceActionType::Buy);
    }

    #[test]
    fn inflow_only_never_issues_sell_recommendations() {
        let holdings = vec![
            sample_holding("HEAVILY_OVERWEIGHT", 80.0, 1000.0, None), // 80,000 (80%)
            sample_holding("UNDERWEIGHT", 20.0, 1000.0, None),        // 20,000 (20%)
        ];
        let mut targets = BTreeMap::new();
        targets.insert("HEAVILY_OVERWEIGHT".to_string(), 50.0);
        targets.insert("UNDERWEIGHT".to_string(), 50.0);

        // Inflow of 30,000
        let plan = compute_rebalance_plan(&holdings, &targets, Some(30_000.0), 2.0, "INR").unwrap();
        assert_eq!(plan.projected_value, 130_000.0);

        // The overweight stock MUST NOT be sold
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Ok);
        assert_eq!(plan.items[0].diff_shares, 0.0);

        // The entire inflow is directed to UNDERWEIGHT
        assert_eq!(plan.items[1].action_type, RebalanceActionType::Buy);
        assert_eq!(plan.items[1].diff_shares, 30.0); // 30,000 / 1000 = 30 shares
        assert!((plan.items[1].diff_value - 30_000.0).abs() < 1e-4);
    }

    #[test]
    fn inflow_water_filling_multiple_underweight() {
        let holdings = vec![
            sample_holding("A", 10.0, 1000.0, None), // 10,000 (target 33.33%)
            sample_holding("B", 20.0, 1000.0, None), // 20,000 (target 33.33%)
            sample_holding("C", 60.0, 1000.0, None), // 60,000 (target 33.33%)
        ];
        // Total = 90,000. Equal targets = 30k each.
        // Inflow = 15,000. Total new = 105,000. Target each = 35,000.
        // A is at 10k (deficit 25k). B is at 20k (deficit 15k). C is at 60k (overweight).
        // Inflow 15k: first brings A from 10k to 20k (using 10k), then remaining 5k split equally between A and B (2.5k each).
        // Total allocation: A gets 12.5k, B gets 2.5k, C gets 0.
        let allocs = compute_inflow_allocation(
            &holdings,
            &[33.33333333, 33.33333333, 33.33333333],
            15_000.0,
        )
        .unwrap();

        assert!((allocs[0] - 12_500.0).abs() < 1.0);
        assert!((allocs[1] - 2_500.0).abs() < 1.0);
        assert_eq!(allocs[2], 0.0);
        assert!((allocs.iter().sum::<f64>() - 15_000.0).abs() < 1e-6);
    }

    #[test]
    fn mutual_fund_fractional_units() {
        let holdings = vec![
            sample_holding("MF:122639", 100.0, 52.345, None), // Fund with NAV 52.345
        ];
        let targets = BTreeMap::new();
        let plan = compute_rebalance_plan(&holdings, &targets, Some(5000.0), 0.0, "INR").unwrap();
        assert_eq!(plan.items[0].action_type, RebalanceActionType::Buy);
        assert!(plan.items[0].is_fund);
        assert!(plan.items[0].action_display.contains("units"));
        assert!((plan.items[0].diff_shares - (5000.0 / 52.345)).abs() < 1e-3);
    }

    #[test]
    fn overweight_under_one_share_reports_ok_not_a_zero_share_sell() {
        // 2 x 100 = 200 (16.7%) against a 10% target leaves 80 over, less
        // than one 100-rupee share, so nothing can be sold.
        let holdings = vec![
            sample_holding("OVERWEIGHT", 2.0, 100.0, None),
            sample_holding("BASE", 10.0, 100.0, None),
        ];
        let mut targets = BTreeMap::new();
        targets.insert("OVERWEIGHT".to_string(), 10.0);
        targets.insert("BASE".to_string(), 90.0);

        let plan = compute_rebalance_plan(&holdings, &targets, None, 2.0, "INR").unwrap();
        let item = &plan.items[0];

        assert_eq!(item.action_type, RebalanceActionType::Ok);
        assert_eq!(item.diff_shares, 0.0);
        assert!(!item.action_display.contains("SELL"));
    }

    #[test]
    fn single_share_orders_use_the_singular_noun() {
        // 4 x 100 = 400 (20%) against targets of 15% (sell) and 25% (buy)
        // leaves exactly 100 rupees, i.e. one share, either way.
        let holdings = vec![
            sample_holding("ONE", 4.0, 100.0, None),
            sample_holding("BASE", 16.0, 100.0, None),
        ];

        let mut sell_targets = BTreeMap::new();
        sell_targets.insert("ONE".to_string(), 15.0);
        sell_targets.insert("BASE".to_string(), 85.0);
        let sell = compute_rebalance_plan(&holdings, &sell_targets, None, 2.0, "INR").unwrap();
        assert_eq!(sell.items[0].action_type, RebalanceActionType::Sell);
        assert_eq!(sell.items[0].diff_shares, 1.0);
        assert_eq!(sell.items[0].action_display, "SELL 1 share");

        let mut buy_targets = BTreeMap::new();
        buy_targets.insert("ONE".to_string(), 25.0);
        buy_targets.insert("BASE".to_string(), 75.0);
        let buy = compute_rebalance_plan(&holdings, &buy_targets, None, 2.0, "INR").unwrap();
        assert_eq!(buy.items[0].action_type, RebalanceActionType::Buy);
        assert_eq!(buy.items[0].diff_shares, 1.0);
        assert_eq!(buy.items[0].action_display, "BUY 1 share");
    }
}
