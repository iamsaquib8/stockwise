//! Dividend, coupon & passive-income analytics engine.
//!
//! Computes:
//! - Trailing 12-month received cashflows and forward N-month projected income.
//! - Per-holding and per-lot Yield-on-Cost (YoC) vs Current Market Yield.
//! - Withholding tax (WHT/TDS) and Net Spendable Income.
//! - Factual Dividend Sustainability & Payout Safety check (FCF coverage & payout ratio).
//! - Semi-annual coupons for Sovereign Gold Bonds (SGB 2.5% p.a.) and fixed-coupon bonds.

use crate::api::Quote;
use crate::output::CsvRow;
use crate::portfolio::{Fx, Holding, Portfolio};
use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};

/// Default projection period in months.
pub const DEFAULT_FORECAST_MONTHS: u32 = 12;

/// Standard withholding tax / TDS rate on dividend income.
/// In India, Section 194 / 194K mandates 10% TDS on dividends above ₹5,000.
pub const DEFAULT_INDIA_TDS_RATE: f64 = 0.10;

/// Standard foreign withholding tax rate on US dividends (without treaty relief / default).
pub const DEFAULT_US_WHT_RATE: f64 = 0.25;

/// Sovereign Gold Bond (SGB) annual coupon interest rate (2.5% p.a., paid half-yearly).
pub const SGB_ANNUAL_COUPON_RATE: f64 = 0.025;

/// Diagnostic safety status of dividend distributions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PayoutSafety {
    /// Safe payout ratio (< 75%) and positive free cash flow coverage.
    Safe,
    /// Elevated payout ratio (75% - 90%) or tight cash flow coverage.
    Caution,
    /// Unsustainable payout ratio (> 90%) or dividends exceed Free Cash Flow.
    Warning,
    /// Metrics missing or not applicable (e.g., bonds, funds, or lenders).
    Unknown,
}

impl PayoutSafety {
    pub fn badge(&self) -> &'static str {
        match self {
            Self::Safe => "SAFE",
            Self::Caution => "CAUTION",
            Self::Warning => "WARNING",
            Self::Unknown => "N/A",
        }
    }
}

/// Yield-on-cost metrics for an individual purchase lot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LotYoC {
    pub lot_no: usize,
    pub qty: f64,
    pub purchase_date: String,
    pub cost_per_share: f64,
    pub annual_dividend_per_share: f64,
    pub yoc_pct: f64,
}

/// Income and dividend sustainability breakdown for a single holding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncomeHoldingSummary {
    pub symbol: String,
    pub shares: f64,
    pub current_price: f64,
    pub avg_cost: f64,
    pub market_value: f64,
    pub cost_basis: f64,
    pub annual_dividend_per_share: f64,
    pub annual_gross_income: f64,
    pub current_yield_pct: f64,
    pub yoc_pct: f64,
    pub net_yield_pct: f64,
    pub wht_rate_pct: f64,
    pub annual_wht_amount: f64,
    pub annual_net_income: f64,
    pub payout_ratio_pct: Option<f64>,
    pub fcf: Option<f64>,
    pub fcf_coverage_ratio: Option<f64>,
    pub payout_safety: PayoutSafety,
    pub safety_reason: Option<String>,
    pub lots_yoc: Vec<LotYoC>,
    pub currency: Option<String>,
}

/// An individual cashflow event (received or projected).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CashflowItem {
    pub symbol: String,
    pub date: String,
    pub amount_gross: f64,
    pub wht_amount: f64,
    pub amount_net: f64,
    pub kind: String,
}

/// Cashflow totals for a given calendar month.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonthlyCashflow {
    /// Format: "YYYY-MM"
    pub month: String,
    pub label: String,
    pub gross_amount: f64,
    pub wht_amount: f64,
    pub net_amount: f64,
    pub is_projected: bool,
    pub items: Vec<CashflowItem>,
}

/// Overall portfolio passive income and dividend report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IncomeReport {
    pub currency: String,
    pub forecast_months: u32,
    pub portfolio_cost_basis: f64,
    pub portfolio_market_value: f64,
    pub trailing_12m_gross: f64,
    pub trailing_12m_wht: f64,
    pub trailing_12m_net: f64,
    pub projected_annual_gross: f64,
    pub projected_annual_wht: f64,
    pub projected_annual_net: f64,
    pub portfolio_current_yield_pct: f64,
    pub portfolio_yoc_pct: f64,
    pub portfolio_net_yield_pct: f64,
    pub monthly_schedule: Vec<MonthlyCashflow>,
    pub holdings: Vec<IncomeHoldingSummary>,
    pub payout_warnings: Vec<String>,
}

/// Input holding data for computing income analytics.
#[derive(Debug, Clone)]
pub struct HoldingIncomeData<'a> {
    pub holding: &'a Holding,
    pub current_price: f64,
    pub annual_dividend_per_share: f64,
    pub payout_ratio: Option<f64>,
    pub fcf: Option<f64>,
    pub currency: Option<&'a str>,
}

/// Detect whether a symbol represents an Indian Sovereign Gold Bond (e.g. SGBMAY29, SGB2425).
pub fn is_sgb(symbol: &str) -> bool {
    let clean = symbol.trim().to_uppercase();
    let s = clean
        .strip_suffix(".NS")
        .or_else(|| clean.strip_suffix(".BO"))
        .unwrap_or(&clean);
    s.starts_with("SGB")
}

/// Calculate standard withholding tax rate (percentage) based on currency / symbol.
///
/// Returns 10% flat TDS for Indian dividends (Section 194),
/// 25% flat withholding tax for US foreign dividends, and 0% for other currencies.
pub fn standard_wht_rate(symbol: &str, currency: Option<&str>) -> f64 {
    let cur = currency.unwrap_or("");
    if cur.eq_ignore_ascii_case("INR") || symbol.ends_with(".NS") || symbol.ends_with(".BO") {
        DEFAULT_INDIA_TDS_RATE * 100.0
    } else if cur.eq_ignore_ascii_case("USD")
        || (!symbol.contains('.') && (cur.is_empty() || cur.eq_ignore_ascii_case("USD")))
    {
        DEFAULT_US_WHT_RATE * 100.0
    } else {
        0.0
    }
}

/// Assemble holding income input data from portfolio, quotes, and resolved symbols.
pub fn assemble_holdings_data<'a>(
    portfolio: &'a Portfolio,
    quotes: &'a [Quote],
    resolved_symbols: &[String],
    base_currency: &'a str,
) -> Vec<HoldingIncomeData<'a>> {
    let mut holdings_data = Vec::with_capacity(portfolio.holdings.len());

    for (idx, holding) in portfolio.holdings.iter().enumerate() {
        let res_sym = resolved_symbols
            .get(idx)
            .map(String::as_str)
            .unwrap_or(&holding.symbol);
        let q_opt = quotes.iter().find(|q| {
            q.symbol.as_deref().is_some_and(|s| {
                s.eq_ignore_ascii_case(res_sym) || s.eq_ignore_ascii_case(&holding.symbol)
            })
        });

        let current_price = q_opt
            .and_then(|q| q.regular_market_price)
            .filter(|p| *p > 0.0)
            .unwrap_or(holding.avg_cost);

        let div_yield_frac = q_opt
            .and_then(|q| q.dividend_yield_fraction())
            .unwrap_or(0.0);
        let annual_div_rate = q_opt
            .and_then(|q| q.trailing_annual_dividend_rate)
            .filter(|r| *r > 0.0)
            .unwrap_or(current_price * div_yield_frac);

        let payout_ratio =
            q_opt.and_then(|q| match (q.eps_trailing_twelve_months, annual_div_rate) {
                (Some(eps), div) if eps > 0.0 && div > 0.0 => Some(div / eps),
                (Some(eps), div) if eps <= 0.0 && div > 0.0 => Some(2.0), // Loss-making dividend payer
                _ => None,
            });

        holdings_data.push(HoldingIncomeData {
            holding,
            current_price,
            annual_dividend_per_share: annual_div_rate,
            payout_ratio,
            fcf: None,
            currency: q_opt
                .and_then(|q| q.currency.as_deref())
                .or(Some(base_currency)),
        });
    }

    holdings_data
}

/// Evaluate dividend sustainability and safety for a single holding.
pub fn evaluate_payout_safety(
    payout_ratio: Option<f64>,
    annual_dividends_paid: f64,
    fcf: Option<f64>,
) -> (PayoutSafety, Option<String>) {
    let mut warnings = Vec::new();
    let mut is_warning = false;
    let mut is_caution = false;

    if let Some(ratio) = payout_ratio {
        let pct = ratio * 100.0;
        if pct < 0.0 || pct > 90.0 {
            is_warning = true;
            warnings.push(format!("Earnings payout ratio is {pct:.1}% (> 90%)"));
        } else if pct > 75.0 {
            is_caution = true;
            warnings.push(format!(
                "Earnings payout ratio is elevated at {pct:.1}% (75-90%)"
            ));
        }
    }

    if let Some(cash_flow) = fcf {
        if cash_flow <= 0.0 && annual_dividends_paid > 0.0 {
            is_warning = true;
            warnings.push(
                "Negative Free Cash Flow (dividends funded via debt or cash reserves)".to_string(),
            );
        } else if cash_flow > 0.0 && annual_dividends_paid > cash_flow {
            is_warning = true;
            warnings.push(format!(
                "Dividends exceed Free Cash Flow (FCF coverage: {:.2}x)",
                cash_flow / annual_dividends_paid
            ));
        } else if cash_flow > 0.0 && annual_dividends_paid > cash_flow * 0.8 {
            is_caution = true;
            warnings.push(format!(
                "Tight Free Cash Flow coverage ({:.2}x)",
                cash_flow / annual_dividends_paid
            ));
        }
    }

    if is_warning {
        (PayoutSafety::Warning, Some(warnings.join("; ")))
    } else if is_caution {
        (PayoutSafety::Caution, Some(warnings.join("; ")))
    } else if payout_ratio.is_some() || fcf.is_some() {
        (PayoutSafety::Safe, None)
    } else {
        (PayoutSafety::Unknown, None)
    }
}

/// Compute Yield-on-Cost (YoC) and current yield for a holding.
pub fn compute_holding_summary(data: &HoldingIncomeData<'_>) -> IncomeHoldingSummary {
    let holding = data.holding;
    let shares = holding.shares;
    let current_price = data.current_price;
    let avg_cost = holding.avg_cost;
    let market_value = shares * current_price;
    let cost_basis = holding.cost_basis();

    let annual_div_per_share = if is_sgb(&holding.symbol) {
        // SGB pays 2.5% p.a. on nominal issue price (~2500 per gram or cost)
        let nominal = if avg_cost > 0.0 {
            avg_cost
        } else {
            current_price
        };
        nominal * SGB_ANNUAL_COUPON_RATE
    } else {
        data.annual_dividend_per_share
    };

    let annual_gross = shares * annual_div_per_share;
    let current_yield = if current_price > 0.0 {
        (annual_div_per_share / current_price) * 100.0
    } else {
        0.0
    };
    let yoc = if avg_cost > 0.0 {
        (annual_div_per_share / avg_cost) * 100.0
    } else {
        0.0
    };

    let wht_rate = standard_wht_rate(&holding.symbol, data.currency);
    let annual_wht = annual_gross * (wht_rate / 100.0);
    let annual_net = annual_gross - annual_wht;

    let fcf_coverage = match (data.fcf, annual_gross) {
        (Some(f), g) if g > 0.0 => Some(f / g),
        _ => None,
    };

    let (safety, reason) = evaluate_payout_safety(data.payout_ratio, annual_gross, data.fcf);

    let mut lots_yoc = Vec::with_capacity(holding.lots.len());
    for (idx, lot) in holding.lots.iter().enumerate() {
        let lot_yoc_pct = if lot.price > 0.0 {
            (annual_div_per_share / lot.price) * 100.0
        } else {
            0.0
        };
        lots_yoc.push(LotYoC {
            lot_no: idx + 1,
            qty: lot.qty,
            purchase_date: lot.date.clone(),
            cost_per_share: lot.price,
            annual_dividend_per_share: annual_div_per_share,
            yoc_pct: lot_yoc_pct,
        });
    }

    let net_yield = if cost_basis > 0.0 {
        (annual_net / cost_basis) * 100.0
    } else {
        0.0
    };

    IncomeHoldingSummary {
        symbol: holding.symbol.clone(),
        shares,
        current_price,
        avg_cost,
        market_value,
        cost_basis,
        annual_dividend_per_share: annual_div_per_share,
        annual_gross_income: annual_gross,
        current_yield_pct: current_yield,
        yoc_pct: yoc,
        net_yield_pct: net_yield,
        wht_rate_pct: wht_rate,
        annual_wht_amount: annual_wht,
        annual_net_income: annual_net,
        payout_ratio_pct: data.payout_ratio.map(|r| r * 100.0),
        fcf: data.fcf,
        fcf_coverage_ratio: fcf_coverage,
        payout_safety: safety,
        safety_reason: reason,
        lots_yoc,
        currency: data.currency.map(str::to_string),
    }
}

/// Generate month-by-month cashflow calendar (12M trailing received + N-month projected).
pub fn build_monthly_schedule(
    portfolio: &Portfolio,
    holdings_summary: &[IncomeHoldingSummary],
    base_currency: &str,
    fx: Option<&Fx>,
    today: NaiveDate,
    forecast_months: u32,
) -> (Vec<MonthlyCashflow>, f64, f64, f64, f64, f64, f64) {
    let mut schedule: Vec<MonthlyCashflow> = Vec::new();
    let convert = |amt: f64, cur: Option<&str>| -> f64 {
        match (fx, cur) {
            (Some(fx_eng), Some(c)) => fx_eng.convert(amt, c).unwrap_or(amt),
            _ => amt,
        }
    };

    // 1. Trailing 12 months (actual received dividends recorded in portfolio.dividends)
    let start_trailing = today.checked_sub_months(Months::new(12)).unwrap_or(today);

    let mut trailing_gross = 0.0;
    let mut trailing_wht = 0.0;
    let mut trailing_net = 0.0;

    // Group actual dividends by year-month
    for m in 0..12 {
        let cur_date = start_trailing
            .checked_add_months(Months::new(m))
            .unwrap_or(start_trailing);
        let ym = format!("{:04}-{:02}", cur_date.year(), cur_date.month());
        let label = cur_date.format("%b %Y").to_string();

        let mut items = Vec::new();
        let mut m_gross = 0.0;
        let mut m_wht = 0.0;

        for d in &portfolio.dividends {
            if d.ex_date.starts_with(&ym) {
                let cur = d.currency.as_deref().or(Some(base_currency));
                let gross = convert(d.amount, cur);
                let wht_rate = standard_wht_rate(&d.symbol, cur);
                let wht = gross * (wht_rate / 100.0);
                let net = gross - wht;
                m_gross += gross;
                m_wht += wht;
                items.push(CashflowItem {
                    symbol: d.symbol.clone(),
                    date: d.ex_date.clone(),
                    amount_gross: gross,
                    wht_amount: wht,
                    amount_net: net,
                    kind: "Dividend".to_string(),
                });
            }
        }

        let m_net = m_gross - m_wht;
        trailing_gross += m_gross;
        trailing_wht += m_wht;
        trailing_net += m_net;

        schedule.push(MonthlyCashflow {
            month: ym,
            label,
            gross_amount: m_gross,
            wht_amount: m_wht,
            net_amount: m_net,
            is_projected: false,
            items,
        });
    }

    // 2. Forward forecast months
    let mut proj_total_gross = 0.0;
    let mut proj_total_wht = 0.0;
    let mut proj_total_net = 0.0;

    for m in 0..forecast_months {
        let fwd_date = today.checked_add_months(Months::new(m)).unwrap_or(today);
        let ym = format!("{:04}-{:02}", fwd_date.year(), fwd_date.month());
        let label = fwd_date.format("%b %Y").to_string();

        let mut items = Vec::new();
        let mut m_gross = 0.0;
        let mut m_wht = 0.0;

        for h in holdings_summary {
            if h.annual_gross_income <= 0.0 {
                continue;
            }

            // Distribute projected cashflows across months:
            // SGB pays half-yearly in May and Nov (or every 6 months);
            // General equity stocks distributed monthly (1/12th)
            let (applies, portion, kind) = if is_sgb(&h.symbol) {
                // SGB coupon frequency: twice a year
                let is_coupon_month = fwd_date.month() == 5 || fwd_date.month() == 11;
                (is_coupon_month, 0.5, "Coupon (SGB 2.5%)")
            } else {
                // Standard dividend distribution across months: annualized / 12
                (true, 1.0 / 12.0, "Dividend (Projected)")
            };

            if applies {
                let cur = h.currency.as_deref().or(Some(base_currency));
                let gross_item = convert(h.annual_gross_income * portion, cur);
                let wht_item = gross_item * (h.wht_rate_pct / 100.0);
                let net_item = gross_item - wht_item;
                m_gross += gross_item;
                m_wht += wht_item;
                items.push(CashflowItem {
                    symbol: h.symbol.clone(),
                    date: format!("{}-15", ym),
                    amount_gross: gross_item,
                    wht_amount: wht_item,
                    amount_net: net_item,
                    kind: kind.to_string(),
                });
            }
        }

        let m_net = m_gross - m_wht;
        proj_total_gross += m_gross;
        proj_total_wht += m_wht;
        proj_total_net += m_net;

        schedule.push(MonthlyCashflow {
            month: ym,
            label,
            gross_amount: m_gross,
            wht_amount: m_wht,
            net_amount: m_net,
            is_projected: true,
            items,
        });
    }

    (
        schedule,
        trailing_gross,
        trailing_wht,
        trailing_net,
        proj_total_gross,
        proj_total_wht,
        proj_total_net,
    )
}

/// Compute full passive income report from holding summaries and historical portfolio.
pub fn generate_income_report(
    portfolio: &Portfolio,
    holdings_data: &[HoldingIncomeData<'_>],
    currency: &str,
    fx: Option<&Fx>,
    today: NaiveDate,
    forecast_months: u32,
) -> IncomeReport {
    let mut summaries = Vec::with_capacity(holdings_data.len());
    let mut payout_warnings = Vec::new();

    let mut total_market_value = 0.0;
    let mut total_cost_basis = 0.0;

    let convert = |amt: f64, cur: Option<&str>| -> f64 {
        match (fx, cur) {
            (Some(fx_eng), Some(c)) => fx_eng.convert(amt, c).unwrap_or(amt),
            _ => amt,
        }
    };

    for data in holdings_data {
        let summary = compute_holding_summary(data);
        let cur = summary.currency.as_deref().or(Some(currency));
        total_market_value += convert(summary.market_value, cur);
        total_cost_basis += convert(summary.cost_basis, cur);

        if let Some(ref reason) = summary.safety_reason {
            payout_warnings.push(format!("{}: {}", summary.symbol, reason));
        }

        summaries.push(summary);
    }

    // Sort holdings by annual gross income descending
    summaries.sort_by(|a, b| b.annual_gross_income.total_cmp(&a.annual_gross_income));

    let (
        monthly_schedule,
        trailing_gross,
        trailing_wht,
        trailing_net,
        _proj_gross,
        _proj_wht,
        _proj_net,
    ) = build_monthly_schedule(portfolio, &summaries, currency, fx, today, forecast_months);

    let annual_proj_gross: f64 = summaries
        .iter()
        .map(|s| {
            convert(
                s.annual_gross_income,
                s.currency.as_deref().or(Some(currency)),
            )
        })
        .sum();
    let annual_proj_wht: f64 = summaries
        .iter()
        .map(|s| {
            convert(
                s.annual_wht_amount,
                s.currency.as_deref().or(Some(currency)),
            )
        })
        .sum();
    let annual_proj_net = annual_proj_gross - annual_proj_wht;

    let portfolio_current_yield = if total_market_value > 0.0 {
        (annual_proj_gross / total_market_value) * 100.0
    } else {
        0.0
    };

    let portfolio_yoc = if total_cost_basis > 0.0 {
        (annual_proj_gross / total_cost_basis) * 100.0
    } else {
        0.0
    };

    let portfolio_net_yield = if total_cost_basis > 0.0 {
        (annual_proj_net / total_cost_basis) * 100.0
    } else {
        0.0
    };

    IncomeReport {
        currency: currency.to_string(),
        forecast_months,
        portfolio_cost_basis: total_cost_basis,
        portfolio_market_value: total_market_value,
        trailing_12m_gross: trailing_gross,
        trailing_12m_wht: trailing_wht,
        trailing_12m_net: trailing_net,
        projected_annual_gross: annual_proj_gross,
        projected_annual_wht: annual_proj_wht,
        projected_annual_net: annual_proj_net,
        portfolio_current_yield_pct: portfolio_current_yield,
        portfolio_yoc_pct: portfolio_yoc,
        portfolio_net_yield_pct: portfolio_net_yield,
        monthly_schedule,
        holdings: summaries,
        payout_warnings,
    }
}

// ── CSV Export Implementations ──

/// CSV representation of an individual holding's income metrics.
#[derive(Debug, Clone, Serialize)]
pub struct IncomeHoldingCsvRow {
    pub symbol: String,
    pub shares: f64,
    pub current_price: f64,
    pub avg_cost: f64,
    pub annual_dividend_per_share: f64,
    pub annual_gross_income: f64,
    pub current_yield_pct: f64,
    pub yoc_pct: f64,
    pub net_yield_pct: f64,
    pub annual_wht: f64,
    pub annual_net_income: f64,
    pub payout_ratio_pct: String,
    pub safety_badge: String,
}

impl CsvRow for IncomeHoldingCsvRow {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "shares",
        "current_price",
        "avg_cost",
        "annual_dividend_per_share",
        "annual_gross_income",
        "current_yield_pct",
        "yoc_pct",
        "net_yield_pct",
        "annual_wht",
        "annual_net_income",
        "payout_ratio_pct",
        "safety_badge",
    ];
}

/// CSV representation of a monthly income schedule row.
#[derive(Debug, Clone, Serialize)]
pub struct MonthlyIncomeCsvRow {
    pub month: String,
    pub label: String,
    pub is_projected: bool,
    pub gross_amount: f64,
    pub wht_amount: f64,
    pub net_amount: f64,
}

impl CsvRow for MonthlyIncomeCsvRow {
    const COLUMNS: &'static [&'static str] = &[
        "month",
        "label",
        "is_projected",
        "gross_amount",
        "wht_amount",
        "net_amount",
    ];
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::portfolio::{DividendReceived, Lot, Portfolio};

    fn make_test_holding(symbol: &str, lots: Vec<(f64, f64, &str)>) -> Holding {
        let lot_vec: Vec<Lot> = lots
            .into_iter()
            .map(|(q, p, d)| Lot::new(q, p, d, "manual"))
            .collect();
        let mut h = Holding {
            symbol: symbol.to_string(),
            shares: 0.0,
            avg_cost: 0.0,
            added_at: "2024-01-01".to_string(),
            lots: lot_vec,
        };
        h.shares = h.lots.iter().map(|l| l.qty).sum();
        h.avg_cost = h.cost_basis() / h.shares;
        h
    }

    #[test]
    fn test_yoc_calculation_per_lot_and_holding() {
        // 100 shares bought @ 200, 100 shares bought @ 400. Avg cost = 300.
        // Current price = 500, Dividend per share = 15.
        // Current yield = 15 / 500 = 3.0%
        // Holding YoC = 15 / 300 = 5.0%
        // Lot 1 YoC = 15 / 200 = 7.5%
        // Lot 2 YoC = 15 / 400 = 3.75%
        let h = make_test_holding(
            "ITC.NS",
            vec![(100.0, 200.0, "2020-01-15"), (100.0, 400.0, "2023-01-15")],
        );
        let data = HoldingIncomeData {
            holding: &h,
            current_price: 500.0,
            annual_dividend_per_share: 15.0,
            payout_ratio: Some(0.65),
            fcf: Some(150_000.0),
            currency: Some("INR"),
        };

        let summary = compute_holding_summary(&data);
        assert_eq!(summary.shares, 200.0);
        assert_eq!(summary.avg_cost, 300.0);
        assert!((summary.current_yield_pct - 3.0).abs() < 1e-4);
        assert!((summary.yoc_pct - 5.0).abs() < 1e-4);
        assert_eq!(summary.annual_gross_income, 3000.0);
        assert_eq!(summary.annual_wht_amount, 300.0); // 10% TDS
        assert_eq!(summary.annual_net_income, 2700.0);

        assert_eq!(summary.lots_yoc.len(), 2);
        assert!((summary.lots_yoc[0].yoc_pct - 7.5).abs() < 1e-4);
        assert!((summary.lots_yoc[1].yoc_pct - 3.75).abs() < 1e-4);
        assert_eq!(summary.payout_safety, PayoutSafety::Safe);
    }

    #[test]
    fn test_sgb_coupon_detection_and_rate() {
        assert!(is_sgb("SGBMAY29.NS"));
        assert!(is_sgb("SGB2425"));
        assert!(!is_sgb("RELIANCE.NS"));

        let h = make_test_holding("SGBMAY29.NS", vec![(10.0, 5000.0, "2021-05-10")]);
        let data = HoldingIncomeData {
            holding: &h,
            current_price: 7500.0,
            annual_dividend_per_share: 0.0, // Quote doesn't report dividends for SGB
            payout_ratio: None,
            fcf: None,
            currency: Some("INR"),
        };

        let summary = compute_holding_summary(&data);
        // SGB 2.5% on cost 5000 = 125/unit/year
        assert_eq!(summary.annual_dividend_per_share, 125.0);
        assert_eq!(summary.annual_gross_income, 1250.0);
        assert!((summary.yoc_pct - 2.5).abs() < 1e-4);
    }

    #[test]
    fn test_payout_safety_classification() {
        // High payout ratio > 90%
        let (s1, r1) = evaluate_payout_safety(Some(0.95), 1000.0, Some(2000.0));
        assert_eq!(s1, PayoutSafety::Warning);
        assert!(r1.unwrap().contains("> 90%"));

        // Negative FCF
        let (s2, r2) = evaluate_payout_safety(Some(0.50), 1000.0, Some(-500.0));
        assert_eq!(s2, PayoutSafety::Warning);
        assert!(r2.unwrap().contains("Negative Free Cash Flow"));

        // Elevated payout 80%
        let (s3, _) = evaluate_payout_safety(Some(0.80), 1000.0, Some(5000.0));
        assert_eq!(s3, PayoutSafety::Caution);

        // Safe payout 50% with healthy FCF
        let (s4, r4) = evaluate_payout_safety(Some(0.50), 1000.0, Some(5000.0));
        assert_eq!(s4, PayoutSafety::Safe);
        assert!(r4.is_none());

        // Loss-making dividend payer (sentinel ratio = 2.0 / 200%)
        let (s5, r5) = evaluate_payout_safety(Some(2.0), 1000.0, None);
        assert_eq!(s5, PayoutSafety::Warning);
        assert!(r5.unwrap().contains("> 90%"));
    }

    #[test]
    fn test_full_report_and_monthly_schedule() {
        let mut portfolio = Portfolio::default();
        portfolio.dividends.push(DividendReceived {
            symbol: "TCS.NS".to_string(),
            ex_date: "2026-05-15".to_string(),
            per_share: 10.0,
            qty: 50.0,
            amount: 500.0,
            currency: Some("INR".to_string()),
        });

        let h1 = make_test_holding("TCS.NS", vec![(50.0, 3000.0, "2024-01-10")]);
        let data1 = HoldingIncomeData {
            holding: &h1,
            current_price: 3500.0,
            annual_dividend_per_share: 40.0,
            payout_ratio: Some(0.60),
            fcf: Some(400_000.0),
            currency: Some("INR"),
        };

        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let report = generate_income_report(&portfolio, &[data1], "INR", None, today, 12);

        assert!((report.portfolio_cost_basis - 150_000.0).abs() < 1e-4);
        assert!((report.portfolio_market_value - 175_000.0).abs() < 1e-4);
        assert!((report.projected_annual_gross - 2000.0).abs() < 1e-4); // 50 * 40
        assert!((report.projected_annual_wht - 200.0).abs() < 1e-4); // 10%
        assert!((report.projected_annual_net - 1800.0).abs() < 1e-4);
        assert!((report.portfolio_yoc_pct - (2000.0 / 150_000.0 * 100.0)).abs() < 1e-4);
        assert!((report.portfolio_net_yield_pct - (1800.0 / 150_000.0 * 100.0)).abs() < 1e-4);
        assert!((report.holdings[0].net_yield_pct - (1800.0 / 150_000.0 * 100.0)).abs() < 1e-4);

        // Trailing 12M received should contain the May 2026 dividend
        assert!((report.trailing_12m_gross - 500.0).abs() < 1e-4);
        assert!((report.trailing_12m_wht - 50.0).abs() < 1e-4);
        assert!((report.trailing_12m_net - 450.0).abs() < 1e-4);
    }

    #[test]
    fn test_multi_currency_income_report() {
        let mut fx = Fx::new("INR");
        fx.set_rate("USD", 85.0);

        let h_in = make_test_holding("ITC.NS", vec![(100.0, 200.0, "2024-01-10")]);
        let data_in = HoldingIncomeData {
            holding: &h_in,
            current_price: 400.0,
            annual_dividend_per_share: 10.0,
            payout_ratio: Some(0.60),
            fcf: None,
            currency: Some("INR"),
        };

        // 10 shares of AAPL @ $150, div = $2/sh -> annual gross = $20 -> in INR = 20 * 85 = 1700
        let h_us = make_test_holding("AAPL", vec![(10.0, 150.0, "2024-01-10")]);
        let data_us = HoldingIncomeData {
            holding: &h_us,
            current_price: 200.0,
            annual_dividend_per_share: 2.0,
            payout_ratio: Some(0.20),
            fcf: None,
            currency: Some("USD"),
        };

        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let report = generate_income_report(
            &Portfolio::default(),
            &[data_in, data_us],
            "INR",
            Some(&fx),
            today,
            12,
        );

        // ITC gross = 100 * 10 = 1000 INR
        // AAPL gross = 10 * 2 = $20 * 85 = 1700 INR
        // Total projected gross in INR = 2700 INR
        assert!((report.projected_annual_gross - 2700.0).abs() < 1e-4);

        // ITC cost = 100 * 200 = 20,000 INR
        // AAPL cost = 10 * 150 = $1,500 * 85 = 127,500 INR
        // Total cost in INR = 147,500 INR
        assert!((report.portfolio_cost_basis - 147_500.0).abs() < 1e-4);
    }
}
