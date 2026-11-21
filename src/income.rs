//! Dividend, coupon & passive-income analytics engine.
//!
//! Computes:
//! - Trailing 12-month received cashflows and forward N-month projected income.
//! - Per-holding and per-lot Yield-on-Cost (YoC) vs Current Market Yield.
//! - Withholding tax (WHT/TDS) and Net Spendable Income. Indian TDS only
//!   above ₹10,000 per company per financial year; none on SGB interest.
//! - Factual Dividend Sustainability & Payout Safety check (FCF coverage & payout ratio).
//! - Semi-annual coupons for Sovereign Gold Bonds (SGB 2.5% p.a.) and fixed-coupon bonds.

use crate::api::Quote;
use crate::output::CsvRow;
use crate::portfolio::{Fx, Holding, Portfolio};
use chrono::{Datelike, Months, NaiveDate};
use serde::{Deserialize, Serialize};

/// Default projection period in months.
pub const DEFAULT_FORECAST_MONTHS: u32 = 12;

/// TDS rate on Indian dividends (Section 194 / 194K), once a company's
/// dividends to one shareholder in a financial year pass
/// [`INDIA_TDS_THRESHOLD`].
pub const DEFAULT_INDIA_TDS_RATE: f64 = 0.10;

/// Dividends from one company in one financial year (April–March) up to
/// this amount have no TDS (₹10,000 from FY 2025-26; ₹5,000 before).
pub const INDIA_TDS_THRESHOLD: f64 = 10_000.0;

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
    /// Holdings left out of the report: no live price (never valued at
    /// their purchase price).
    #[serde(default)]
    pub excluded: Vec<String>,
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

/// The SGB tranche `symbol` names (`SGBMAY29`, `SGBMAY29.NS`), from the
/// shipped tranche table.
pub fn sgb_tranche(symbol: &str) -> Option<&'static crate::commodity::Tranche> {
    let clean = symbol.trim().to_uppercase();
    let s = clean
        .strip_suffix(".NS")
        .or_else(|| clean.strip_suffix(".BO"))
        .unwrap_or(&clean);
    crate::commodity::SGB_TRANCHES
        .iter()
        .find(|t| t.symbol.eq_ignore_ascii_case(s))
}

/// Whether a dividend is Indian (TDS rules apply).
fn is_indian(symbol: &str, currency: Option<&str>) -> bool {
    currency.is_some_and(|c| c.eq_ignore_ascii_case("INR"))
        || symbol.ends_with(".NS")
        || symbol.ends_with(".BO")
}

/// Whether Indian TDS is deducted on a company's dividends to one
/// shareholder that add up to `fy_total` (in rupees) in a financial year.
pub fn india_tds_applies(fy_total: f64) -> bool {
    fy_total > INDIA_TDS_THRESHOLD
}

/// The Indian financial year (starting year) `date` falls in: April–March.
pub fn india_fy(date: NaiveDate) -> i32 {
    if date.month() >= 4 {
        date.year()
    } else {
        date.year() - 1
    }
}

/// Calculate standard withholding tax rate (percentage) based on currency / symbol.
///
/// Returns 10% TDS for Indian dividends (Section 194; whether it applies
/// depends on the ₹10,000 yearly threshold, see [`india_tds_applies`]), 0%
/// for SGB interest (no TDS is deducted on it), 25% withholding for US
/// dividends, and 0% for other currencies.
pub fn standard_wht_rate(symbol: &str, currency: Option<&str>) -> f64 {
    let cur = currency.unwrap_or("");
    if is_sgb(symbol) {
        0.0
    } else if is_indian(symbol, currency) {
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

        // No live price: left out (and named in the report), never valued
        // at its purchase price, which may be in another currency.
        let Some(current_price) = q_opt
            .and_then(|q| q.regular_market_price)
            .filter(|p| *p > 0.0)
        else {
            continue;
        };

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
                .or_else(|| crate::portfolio::symbol_currency(&holding.symbol))
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
        // SGB interest is 2.5% a year on the issue price, whatever was paid
        // for the bond later. Without the tranche, the purchase price (an
        // original subscription paid the issue price); never the market price.
        let nominal = sgb_tranche(&holding.symbol)
            .map(|t| t.issue_price)
            .unwrap_or(avg_cost);
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

    let mut wht_rate = standard_wht_rate(&holding.symbol, data.currency);
    if is_indian(&holding.symbol, data.currency) && !india_tds_applies(annual_gross) {
        wht_rate = 0.0;
    }
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

/// Convert `amt` in `cur` to the portfolio's `base` currency.
///
/// Returns `Some(amt)` if `cur` matches `base` (or is omitted),
/// converts via `fx` when a rate is known, or returns `None` if `cur` differs
/// from `base` and no conversion rate is available.
pub fn to_base(fx: Option<&Fx>, amt: f64, cur: Option<&str>, base: &str) -> Option<f64> {
    match cur {
        None => Some(amt),
        Some(c) if c.eq_ignore_ascii_case(base) => Some(amt),
        Some(c) => fx.and_then(|f| f.convert(amt, c)),
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

    // 1. Trailing 12 months, the current one included (actual received
    // dividends recorded in portfolio.dividends, up to today).
    let start_trailing = today.checked_sub_months(Months::new(11)).unwrap_or(today);
    let today_s = today.to_string();
    // Indian TDS applies per company per financial year, once its dividends
    // pass the threshold: add them up first (in rupees).
    let mut fy_totals: std::collections::HashMap<(String, i32), f64> = Default::default();
    for d in &portfolio.dividends {
        let cur = d.currency.as_deref().or(Some(base_currency));
        if let (true, Ok(day)) = (
            is_indian(&d.symbol, cur),
            NaiveDate::parse_from_str(&d.ex_date, "%Y-%m-%d"),
        ) {
            *fy_totals
                .entry((d.symbol.clone(), india_fy(day)))
                .or_default() += d.amount;
        }
    }
    let tds_applies = |symbol: &str, ex_date: &str| {
        NaiveDate::parse_from_str(ex_date, "%Y-%m-%d")
            .ok()
            .and_then(|day| fy_totals.get(&(symbol.to_string(), india_fy(day))))
            .is_some_and(|t| india_tds_applies(*t))
    };

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
            if d.ex_date.starts_with(&ym) && d.ex_date.as_str() <= today_s.as_str() {
                let cur = d.currency.as_deref().or(Some(base_currency));
                if let Some(gross) = to_base(fx, d.amount, cur, base_currency) {
                    let mut wht_rate = standard_wht_rate(&d.symbol, cur);
                    if is_indian(&d.symbol, cur) && !tds_applies(&d.symbol, &d.ex_date) {
                        wht_rate = 0.0;
                    }
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

    // The current month is in the trailing part: the forecast starts with the next.
    for m in 1..=forecast_months {
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
                // Interest is paid every six months from the issue date
                // (May and November when the tranche is unknown).
                let first = sgb_tranche(&h.symbol).map_or(5, |t| t.issue_date().month());
                let second = (first + 5) % 12 + 1;
                let is_coupon_month = fwd_date.month() == first || fwd_date.month() == second;
                (is_coupon_month, 0.5, "Coupon (SGB 2.5%)")
            } else {
                (true, 1.0 / 12.0, "Dividend (Projected)")
            };

            if applies {
                let cur = h.currency.as_deref().or(Some(base_currency));
                if let Some(gross_item) =
                    to_base(fx, h.annual_gross_income * portion, cur, base_currency)
                {
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
    let excluded: Vec<String> = portfolio
        .holdings
        .iter()
        .filter(|h| {
            !holdings_data
                .iter()
                .any(|d| d.holding.symbol.eq_ignore_ascii_case(&h.symbol))
        })
        .map(|h| h.symbol.clone())
        .collect();
    for d in holdings_data {
        if is_sgb(&d.holding.symbol) && sgb_tranche(&d.holding.symbol).is_none() {
            payout_warnings.push(format!(
                "{}: issue price not known; interest estimated on the purchase price",
                d.holding.symbol
            ));
        }
    }

    let mut total_market_value = 0.0;
    let mut total_cost_basis = 0.0;

    for data in holdings_data {
        let summary = compute_holding_summary(data);
        let cur = summary.currency.as_deref().or(Some(currency));
        match (
            to_base(fx, summary.market_value, cur, currency),
            to_base(fx, summary.cost_basis, cur, currency),
        ) {
            (Some(mv), Some(cb)) => {
                total_market_value += mv;
                total_cost_basis += cb;
            }
            _ => {
                payout_warnings.push(format!(
                    "{}: No FX conversion rate for {} to {}; omitted from totals",
                    summary.symbol,
                    cur.unwrap_or("unknown"),
                    currency
                ));
            }
        }

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

    let mut annual_proj_gross = 0.0;
    let mut annual_proj_wht = 0.0;
    for s in &summaries {
        let cur = s.currency.as_deref().or(Some(currency));
        if let (Some(g), Some(w)) = (
            to_base(fx, s.annual_gross_income, cur, currency),
            to_base(fx, s.annual_wht_amount, cur, currency),
        ) {
            annual_proj_gross += g;
            annual_proj_wht += w;
        }
    }
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
        excluded,
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
        // ₹3,000 a year from one company is under the ₹10,000 TDS threshold.
        assert_eq!(summary.annual_wht_amount, 0.0);
        assert_eq!(summary.annual_net_income, 3000.0);

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
        // Unknown tranche: 2.5% on the purchase price 5000 = 125/unit/year
        assert_eq!(summary.annual_dividend_per_share, 125.0);
        assert_eq!(summary.annual_gross_income, 1250.0);
        assert!((summary.yoc_pct - 2.5).abs() < 1e-4);
        // No TDS on SGB interest.
        assert_eq!(summary.annual_wht_amount, 0.0);
    }

    #[test]
    fn sgb_interest_is_on_the_issue_price_not_the_market_or_purchase_price() {
        // SGBJUN27: 2019-20 Series I, issued 11 Jun 2019 at ₹3,196.
        let h = make_test_holding("SGBJUN27.NS", vec![(100.0, 6000.0, "2023-01-10")]);
        let data = HoldingIncomeData {
            holding: &h,
            current_price: 9000.0,
            annual_dividend_per_share: 0.0,
            payout_ratio: None,
            fcf: None,
            currency: Some("INR"),
        };
        let summary = compute_holding_summary(&data);
        assert!((summary.annual_dividend_per_share - 3196.0 * 0.025).abs() < 1e-9);
        assert!((summary.annual_gross_income - 7990.0).abs() < 1e-6);
        assert_eq!(summary.wht_rate_pct, 0.0);
        // Paid in June and December, the issue month and six months on.
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let report = generate_income_report(&Portfolio::default(), &[data], "INR", None, today, 12);
        let paid: Vec<&str> = report
            .monthly_schedule
            .iter()
            .filter(|m| m.is_projected && m.gross_amount > 0.0)
            .map(|m| m.month.as_str())
            .collect();
        assert_eq!(paid, vec!["2026-12", "2027-06"]);
    }

    #[test]
    fn indian_tds_only_above_ten_thousand_per_company_per_year() {
        assert!(!india_tds_applies(10_000.0));
        assert!(india_tds_applies(10_000.01));
        assert_eq!(
            india_fy(NaiveDate::from_ymd_opt(2026, 3, 31).unwrap()),
            2025
        );
        assert_eq!(india_fy(NaiveDate::from_ymd_opt(2026, 4, 1).unwrap()), 2026);

        // Projected: ₹12,000 a year passes the threshold, 10% deducted.
        let big = make_test_holding("ITC.NS", vec![(1000.0, 300.0, "2020-01-15")]);
        let data = HoldingIncomeData {
            holding: &big,
            current_price: 400.0,
            annual_dividend_per_share: 12.0,
            payout_ratio: None,
            fcf: None,
            currency: Some("INR"),
        };
        let s = compute_holding_summary(&data);
        assert!((s.annual_wht_amount - 1200.0).abs() < 1e-9);

        // Received: two TCS dividends of ₹6,000 in one FY add up past the
        // threshold; one INFY dividend of ₹6,000 does not.
        let mut p = Portfolio::default();
        for (sym, date) in [
            ("TCS.NS", "2026-05-15"),
            ("TCS.NS", "2026-10-15"),
            ("INFY.NS", "2026-06-15"),
        ] {
            p.dividends.push(DividendReceived {
                symbol: sym.into(),
                ex_date: date.into(),
                per_share: 60.0,
                qty: 100.0,
                amount: 6000.0,
                currency: Some("INR".into()),
            });
        }
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let report = generate_income_report(&p, &[], "INR", None, today, 12);
        assert!((report.trailing_12m_gross - 18_000.0).abs() < 1e-9);
        assert!((report.trailing_12m_wht - 1200.0).abs() < 1e-9);
    }

    #[test]
    fn the_current_month_is_in_the_trailing_year_and_not_projected_twice() {
        let mut p = Portfolio::default();
        p.dividends.push(DividendReceived {
            symbol: "TCS.NS".into(),
            ex_date: "2026-11-03".into(),
            per_share: 10.0,
            qty: 50.0,
            amount: 500.0,
            currency: Some("INR".into()),
        });
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let report = generate_income_report(&p, &[], "INR", None, today, 12);
        assert!((report.trailing_12m_gross - 500.0).abs() < 1e-9);
        let months: Vec<(&str, bool)> = report
            .monthly_schedule
            .iter()
            .map(|m| (m.month.as_str(), m.is_projected))
            .collect();
        assert_eq!(months.first(), Some(&("2025-12", false)));
        assert!(months.contains(&("2026-11", false)));
        assert!(!months.contains(&("2026-11", true)));
        assert_eq!(months.iter().filter(|m| m.1).count(), 12);
        assert_eq!(months.last(), Some(&("2027-11", true)));
    }

    #[test]
    fn a_holding_without_a_price_is_excluded_not_valued_at_cost() {
        let mut p = Portfolio::default();
        p.add_lot("AAPL", Lot::new(10.0, 150.0, "2024-01-10", "manual"));
        p.add_lot("ITC.NS", Lot::new(100.0, 200.0, "2024-01-10", "manual"));
        let mut q = Quote::default();
        q.symbol = Some("ITC.NS".into());
        q.regular_market_price = Some(400.0);
        q.currency = Some("INR".into());
        let quotes = vec![q];
        let resolved = vec!["AAPL".to_string(), "ITC.NS".to_string()];
        let data = assemble_holdings_data(&p, &quotes, &resolved, "INR");
        assert_eq!(data.len(), 1);
        assert_eq!(data[0].holding.symbol, "ITC.NS");
        let today = NaiveDate::from_ymd_opt(2026, 11, 8).unwrap();
        let report = generate_income_report(&p, &data, "INR", None, today, 12);
        assert_eq!(report.excluded, vec!["AAPL".to_string()]);
        assert!((report.portfolio_cost_basis - 20_000.0).abs() < 1e-9);
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
        // Under the ₹10,000 threshold: no TDS.
        assert!((report.projected_annual_wht - 0.0).abs() < 1e-4);
        assert!((report.projected_annual_net - 2000.0).abs() < 1e-4);
        assert!((report.portfolio_yoc_pct - (2000.0 / 150_000.0 * 100.0)).abs() < 1e-4);
        assert!((report.portfolio_net_yield_pct - (2000.0 / 150_000.0 * 100.0)).abs() < 1e-4);
        assert!((report.holdings[0].net_yield_pct - (2000.0 / 150_000.0 * 100.0)).abs() < 1e-4);

        // Trailing 12M received should contain the May 2026 dividend
        assert!((report.trailing_12m_gross - 500.0).abs() < 1e-4);
        assert!((report.trailing_12m_wht - 0.0).abs() < 1e-4);
        assert!((report.trailing_12m_net - 500.0).abs() < 1e-4);
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

    #[test]
    fn test_unconverted_currency_omitted_with_warning() {
        // When no FX rate is available for a foreign currency, it must be omitted from totals
        // and reported in payout_warnings rather than corrupting the totals with mixed currencies.
        let fx = Fx::new("INR"); // No USD rate configured

        let h_in = make_test_holding("ITC.NS", vec![(100.0, 200.0, "2024-01-10")]);
        let data_in = HoldingIncomeData {
            holding: &h_in,
            current_price: 400.0,
            annual_dividend_per_share: 10.0,
            payout_ratio: Some(0.60),
            fcf: None,
            currency: Some("INR"),
        };

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

        // Only ITC is included in the totals (100 * 10 = 1000 INR)
        assert!((report.projected_annual_gross - 1000.0).abs() < 1e-4);
        assert!((report.portfolio_cost_basis - 20_000.0).abs() < 1e-4);

        // A warning is raised for the unconvertible holding
        assert!(
            report
                .payout_warnings
                .iter()
                .any(|w| w.contains("No FX conversion rate for USD to INR; omitted from totals"))
        );
    }
}
