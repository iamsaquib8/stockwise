//! Goal planning math: the monthly SIP needed for a target corpus (with an
//! optional yearly step-up), a retirement corpus that pays inflation-linked
//! monthly expenses, and an education goal. Amounts can be typed the way
//! Indians write them (`1cr`, `25L`, `50k`). Pure functions; `stockwise goal`
//! prints them.
//!
//! Conventions, as Indian SIP calculators use them: the monthly rate is the
//! annual return ÷ 12, SIPs are paid at the start of each month, and a
//! step-up raises the SIP once a year.

use crate::tax::{Country, INDIA_CESS_RATE};
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// Parse an amount such as `1cr`, `1.5 crore`, `25L`, `25 lakh`, `50k`,
/// `2m` (million), `₹2,50,000` or `250000`.
pub fn parse_amount(s: &str) -> Result<f64, String> {
    let cleaned: String = s
        .trim()
        .trim_start_matches(['₹', '$'])
        .chars()
        .filter(|c| !matches!(c, ',' | '_' | ' '))
        .collect::<String>()
        .to_lowercase();
    let split = cleaned
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(cleaned.len());
    let (number, unit) = cleaned.split_at(split);
    let scale = match unit {
        "" => 1.0,
        "k" | "thousand" => 1e3,
        "l" | "lakh" | "lakhs" | "lac" | "lacs" => 1e5,
        "m" | "mn" | "million" => 1e6,
        "cr" | "crore" | "crores" => 1e7,
        "b" | "bn" | "billion" => 1e9,
        _ => return Err(amount_error(s)),
    };
    match number.parse::<f64>() {
        Ok(n) if n.is_finite() && n > 0.0 && n * scale <= 1e15 => Ok(n * scale),
        _ => Err(amount_error(s)),
    }
}

fn amount_error(s: &str) -> String {
    format!("`{s}` is not an amount: use e.g. 1cr, 25L, 50k or 250000")
}

/// Monthly rate for an annual return in percent: annual ÷ 12.
pub fn monthly_rate(annual_pct: f64) -> f64 {
    annual_pct / 100.0 / 12.0
}

/// What `n` monthly payments of 1, each at the start of a month, grow to by
/// the end of the last month at monthly rate `i`.
fn annuity_due_fv(i: f64, n: u32) -> f64 {
    if i.abs() < 1e-12 {
        n as f64
    } else {
        ((1.0 + i).powi(n as i32) - 1.0) / i * (1.0 + i)
    }
}

/// What a monthly SIP of 1 in its first year grows to after `years`, when
/// the SIP rises by `step_up_pct` at the start of every year.
pub fn sip_growth_factor(years: u32, annual_return_pct: f64, step_up_pct: f64) -> f64 {
    let i = monthly_rate(annual_return_pct);
    let year = annuity_due_fv(i, 12);
    let g = 1.0 + step_up_pct / 100.0;
    (0..years)
        .map(|y| g.powi(y as i32) * year * (1.0 + i).powi((12 * (years - 1 - y)) as i32))
        .sum()
}

// ── Horizons ──

/// Longest goal horizon, in months (60 years).
pub const MAX_MONTHS: u32 = 720;

/// Parse a horizon such as `18mo`, `6 months`, `2y`, `1.5y` or a bare number
/// of years, into whole months (1 to [`MAX_MONTHS`]).
pub fn parse_duration(s: &str) -> Result<u32, String> {
    let bad = || format!("`{s}` is not a horizon: use e.g. 6mo, 18mo, 2y or 15y");
    let cleaned: String = s
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| *c != ' ')
        .collect();
    let split = cleaned
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(cleaned.len());
    let (number, unit) = cleaned.split_at(split);
    let n: f64 = number.parse().map_err(|_| bad())?;
    let months = match unit {
        "" | "y" | "yr" | "yrs" | "year" | "years" => n * 12.0,
        "m" | "mo" | "mos" | "month" | "months" => n,
        _ => return Err(bad()),
    };
    let rounded = months.round();
    if (months - rounded).abs() > 1e-6 || !(1.0..=MAX_MONTHS as f64).contains(&rounded) {
        return Err(format!(
            "`{s}` must be a whole number of months from 1 to {MAX_MONTHS} (60 years)"
        ));
    }
    Ok(rounded as u32)
}

/// Whole months from `today` to `by` (`YYYY-MM` is the 1st of that month).
pub fn months_until(today: NaiveDate, by: &str) -> Result<u32, String> {
    let bad = || format!("`{by}` is not a date: use YYYY-MM or YYYY-MM-DD");
    let parts: Vec<&str> = by.trim().split('-').collect();
    let num = |i: usize| parts.get(i).and_then(|p| p.parse::<u32>().ok());
    let (y, m, d) = match parts.len() {
        2 => (num(0), num(1), Some(1)),
        3 => (num(0), num(1), num(2)),
        _ => return Err(bad()),
    };
    let date = NaiveDate::from_ymd_opt(
        y.ok_or_else(bad)? as i32,
        m.ok_or_else(bad)?,
        d.ok_or_else(bad)?,
    )
    .ok_or_else(bad)?;
    months_between(today, date).ok_or_else(|| {
        if date <= today {
            format!("`{by}` is not in the future")
        } else {
            format!("`{by}` must be from 1 month to 60 years ahead")
        }
    })
}

/// Whole months from `from` to `to`; `None` when that is below one month or
/// above [`MAX_MONTHS`].
pub fn months_between(from: NaiveDate, to: NaiveDate) -> Option<u32> {
    let mut months = (to.year() - from.year()) * 12 + to.month() as i32 - from.month() as i32;
    if to.day() < from.day() {
        months -= 1;
    }
    (1..=MAX_MONTHS as i32)
        .contains(&months)
        .then_some(months as u32)
}

/// Whole years and the months left over.
pub fn duration_parts(months: u32) -> (u32, u32) {
    (months / 12, months % 12)
}

// ── Return regimes ──

/// Which market's baselines and tax rules apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    India,
    Us,
    /// Any other market: US-style baselines, no tax by default.
    Other,
}

impl Region {
    fn country(self) -> Country {
        if self == Region::India {
            Country::India
        } else {
            Country::Us
        }
    }

    /// A return at or above this, in percent, is equity risk.
    pub fn equity_return_pct(self) -> f64 {
        if self == Region::India { 10.0 } else { 8.0 }
    }

    fn hybrid_return_pct(self) -> f64 {
        if self == Region::India { 8.0 } else { 5.5 }
    }

    /// Yearly inflation shown for purchasing power when none is given.
    pub fn default_cpi_pct(self) -> f64 {
        if self == Region::India { 6.0 } else { 3.0 }
    }

    /// Default marginal slab rate, in percent.
    pub fn default_slab_pct(self) -> f64 {
        if self == Region::India { 30.0 } else { 24.0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RegimeKind {
    /// 24 months or less.
    Short,
    /// 25 to 60 months.
    Medium,
    /// More than 60 months.
    Long,
}

pub fn regime_kind(months: u32) -> RegimeKind {
    match months {
        0..=24 => RegimeKind::Short,
        25..=60 => RegimeKind::Medium,
        _ => RegimeKind::Long,
    }
}

/// The return and volatility a plan assumes, and where they came from.
#[derive(Debug, Clone, Serialize)]
pub struct Regime {
    pub kind: RegimeKind,
    pub name: &'static str,
    pub return_pct: f64,
    /// Yearly standard deviation, in percent.
    pub vol_pct: f64,
    /// `baseline`, `tbill` (cached 364-day T-bill yield) or `user`.
    pub source: &'static str,
}

/// Volatility that goes with a return level (yearly, percent).
pub fn vol_for_return(return_pct: f64, region: Region) -> f64 {
    if return_pct >= region.equity_return_pct() {
        15.0
    } else if return_pct >= region.hybrid_return_pct() {
        8.0
    } else {
        2.0
    }
}

/// Horizon-matched baseline: capital preservation up to 24 months,
/// conservative hybrid to 60, long-term growth beyond. `tbill_pct` replaces
/// the short-term India default when a cached 364-day yield is known.
pub fn baseline(months: u32, region: Region, tbill_pct: Option<f64>) -> Regime {
    let india = region == Region::India;
    let (kind, name, return_pct, vol_pct) = match regime_kind(months) {
        RegimeKind::Short => (
            RegimeKind::Short,
            "Capital preservation (T-bill / liquid / arbitrage)",
            if india { 6.75 } else { 4.25 },
            2.0,
        ),
        RegimeKind::Medium => (
            RegimeKind::Medium,
            "Conservative hybrid (debt + equity)",
            if india { 9.0 } else { 6.5 },
            8.0,
        ),
        RegimeKind::Long => (
            RegimeKind::Long,
            "Long-term growth",
            if india { 12.0 } else { 9.0 },
            15.0,
        ),
    };
    match tbill_pct {
        Some(y) if kind == RegimeKind::Short && india && (0.0..=50.0).contains(&y) => Regime {
            kind,
            name,
            return_pct: y,
            vol_pct,
            source: "tbill",
        },
        _ => Regime {
            kind,
            name,
            return_pct,
            vol_pct,
            source: "baseline",
        },
    }
}

/// The regime of a plan: the baseline, or the user's return with the
/// volatility that return level implies.
pub fn regime_for(
    months: u32,
    region: Region,
    tbill_pct: Option<f64>,
    user: Option<f64>,
) -> Regime {
    let base = baseline(months, region, tbill_pct);
    match user {
        Some(r) => Regime {
            return_pct: r,
            vol_pct: vol_for_return(r, region),
            source: "user",
            ..base
        },
        None => base,
    }
}

// ── Month-by-month engine ──

/// A glide path: the return slides linearly to `to_pct` over the last
/// `months` months.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct Glide {
    pub months: u32,
    pub to_pct: f64,
}

fn glide_weight(m: u32, months: u32, glide_months: u32) -> f64 {
    let span = glide_months.min(months).max(1);
    let start = months - span;
    if m < start {
        0.0
    } else {
        ((m - start + 1) as f64 / span as f64).min(1.0)
    }
}

/// The monthly rate of each of `months` months.
pub fn monthly_rates(months: u32, return_pct: f64, glide: Option<Glide>) -> Vec<f64> {
    let i = monthly_rate(return_pct);
    (0..months)
        .map(|m| match glide {
            Some(g) => i + (monthly_rate(g.to_pct) - i) * glide_weight(m, months, g.months),
            None => i,
        })
        .collect()
}

/// Monthly standard deviation of each month, as a fraction.
fn monthly_vols(months: u32, vol_pct: f64, glide: Option<Glide>) -> Vec<f64> {
    let base = vol_pct / 100.0 / 12f64.sqrt();
    let low = 2.0 / 100.0 / 12f64.sqrt();
    (0..months)
        .map(|m| match glide {
            Some(g) => base + (low.min(base) - base) * glide_weight(m, months, g.months),
            None => base,
        })
        .collect()
}

/// `g[m]`: what 1 paid at the start of month `m` is worth at the end of the
/// last month. `g[months]` is 1.
fn growth_to_end(rates: &[f64]) -> Vec<f64> {
    let mut g = vec![1.0; rates.len() + 1];
    for m in (0..rates.len()).rev() {
        g[m] = g[m + 1] * (1.0 + rates[m]);
    }
    g
}

/// The SIP in month `m` (0-based) as a multiple of the first year's SIP.
fn step_mult(m: u32, step_up_pct: f64) -> f64 {
    (1.0 + step_up_pct / 100.0).powi((m / 12) as i32)
}

fn sip_factor(g: &[f64], step_up_pct: f64) -> f64 {
    (0..g.len() as u32 - 1)
        .map(|m| step_mult(m, step_up_pct) * g[m as usize])
        .sum()
}

/// One month of a SIP plan.
#[derive(Debug, Clone, Serialize)]
pub struct SipPeriod {
    pub month: u32,
    pub monthly_sip: f64,
    /// SIP amounts paid so far (not counting existing savings).
    pub invested: f64,
    /// Value at the end of the month, existing savings included.
    pub value: f64,
    /// Instalments held 12 months or more at the end of this month.
    pub lots_long_term: u32,
}

/// One year of a SIP plan.
#[derive(Debug, Clone, Serialize)]
pub struct SipYear {
    pub year: u32,
    pub monthly_sip: f64,
    /// SIP amounts paid so far (not counting existing savings).
    pub invested: f64,
    /// Value at the end of the year, existing savings included.
    pub value: f64,
}

/// The monthly SIP that reaches `target` after `months`.
#[derive(Debug, Clone, Serialize)]
pub struct SipPlan {
    pub target: f64,
    /// The horizon rounded up to whole years.
    pub years: u32,
    pub months: u32,
    pub annual_return_pct: f64,
    /// The same return compounded monthly, as a yearly rate: 12% a year is
    /// 12.68% effective.
    pub effective_annual_pct: f64,
    pub step_up_pct: f64,
    /// Money already invested towards the goal; it grows at the same return.
    pub savings: f64,
    /// What `savings` grows to by the end.
    pub savings_future_value: f64,
    /// SIP in the first year (0 when savings already cover the target).
    pub monthly_sip: f64,
    /// SIP in the last year, after every step-up.
    pub final_monthly_sip: f64,
    /// Total of all SIP payments.
    pub total_invested: f64,
    /// Final value minus everything paid in (savings and SIPs).
    pub gains: f64,
    /// A single investment today that reaches the target instead.
    pub lump_sum_today: f64,
    pub schedule: Vec<SipYear>,
    /// Month by month; only for goals of 24 months or less.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub monthly_schedule: Vec<SipPeriod>,
    #[serde(skip)]
    pub periods: Vec<SipPeriod>,
}

fn effective_annual_pct(return_pct: f64) -> f64 {
    ((1.0 + monthly_rate(return_pct)).powi(12) - 1.0) * 100.0
}

/// A plan paying `sip` a month (first year) for `months`, month by month.
fn build_plan(
    target: f64,
    months: u32,
    return_pct: f64,
    step_up_pct: f64,
    savings: f64,
    rates: &[f64],
    sip: f64,
) -> SipPlan {
    let g = growth_to_end(rates);
    let (mut value, mut invested) = (savings, 0.0);
    let mut periods = Vec::with_capacity(months as usize);
    let mut schedule = Vec::new();
    for m in 0..months {
        let pay = sip * step_mult(m, step_up_pct);
        invested += pay;
        value = (value + pay) * (1.0 + rates[m as usize]);
        periods.push(SipPeriod {
            month: m + 1,
            monthly_sip: pay,
            invested,
            value,
            lots_long_term: (m + 1).saturating_sub(11),
        });
        if (m + 1) % 12 == 0 || m + 1 == months {
            schedule.push(SipYear {
                year: (m + 1).div_ceil(12),
                monthly_sip: pay,
                invested,
                value,
            });
        }
    }
    let final_monthly_sip = schedule.last().map_or(0.0, |y| y.monthly_sip);
    SipPlan {
        target,
        years: months.div_ceil(12),
        months,
        annual_return_pct: return_pct,
        effective_annual_pct: effective_annual_pct(return_pct),
        step_up_pct,
        savings,
        savings_future_value: savings * g[0],
        monthly_sip: sip,
        final_monthly_sip,
        total_invested: invested,
        gains: value - invested - savings,
        lump_sum_today: target / g[0],
        schedule,
        monthly_schedule: if months <= 24 {
            periods.clone()
        } else {
            Vec::new()
        },
        periods,
    }
}

/// The monthly SIP (before tax) that reaches `target` after `months`.
pub fn plan_sip_months(
    target: f64,
    months: u32,
    return_pct: f64,
    step_up_pct: f64,
    savings: f64,
    glide: Option<Glide>,
) -> SipPlan {
    let rates = monthly_rates(months, return_pct, glide);
    let sip = gross_sip(target, step_up_pct, savings, &growth_to_end(&rates));
    build_plan(
        target,
        months,
        return_pct,
        step_up_pct,
        savings,
        &rates,
        sip,
    )
}

fn gross_sip(target: f64, step_up_pct: f64, savings: f64, g: &[f64]) -> f64 {
    let needed = (target - savings * g[0]).max(0.0);
    let factor = sip_factor(g, step_up_pct);
    if needed > 0.0 && factor > 0.0 {
        needed / factor
    } else {
        0.0
    }
}

pub fn plan_sip(
    target: f64,
    years: u32,
    annual_return_pct: f64,
    step_up_pct: f64,
    savings: f64,
) -> SipPlan {
    plan_sip_months(
        target,
        12 * years,
        annual_return_pct,
        step_up_pct,
        savings,
        None,
    )
}

/// `amount` after `years` of inflation at `inflation_pct` a year.
pub fn inflate(amount: f64, inflation_pct: f64, years: u32) -> f64 {
    amount * (1.0 + inflation_pct / 100.0).powi(years as i32)
}

/// `amount` after `months` of inflation at `inflation_pct` a year.
pub fn inflate_months(amount: f64, inflation_pct: f64, months: u32) -> f64 {
    amount * (1.0 + inflation_pct / 100.0).powf(months as f64 / 12.0)
}

// ── Tax and exit load at redemption ──

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TaxKind {
    /// Equity fund: short-term rate under 12 months, long-term rate above
    Equity,
    /// Arbitrage fund: taxed as equity
    Arbitrage,
    /// Debt / liquid fund or FD: gains at your slab rate (Sec. 50AA)
    Debt,
    /// No tax
    None,
}

impl TaxKind {
    pub fn label(self) -> &'static str {
        match self {
            TaxKind::Equity => "equity",
            TaxKind::Arbitrage => "arbitrage",
            TaxKind::Debt => "debt / liquid",
            TaxKind::None => "none",
        }
    }
}

/// Tax a regime usually means: debt-like for goals up to 60 months, equity
/// beyond. No tax outside India and the US.
pub fn default_tax_kind(kind: RegimeKind, region: Region) -> TaxKind {
    match (region, kind) {
        (Region::Other, _) => TaxKind::None,
        (_, RegimeKind::Long) => TaxKind::Equity,
        _ => TaxKind::Debt,
    }
}

/// Exit load on lots held under 12 months: 1% on equity funds in India.
pub fn default_exit_load_pct(kind: TaxKind, region: Region) -> f64 {
    if kind == TaxKind::Equity && region == Region::India {
        1.0
    } else {
        0.0
    }
}

#[derive(Debug, Clone, Copy)]
pub struct TaxRules {
    pub country: Country,
    pub kind: TaxKind,
    pub slab_pct: f64,
    pub exit_load_pct: f64,
}

impl TaxRules {
    pub fn new(region: Region, kind: TaxKind, slab_pct: f64, exit_load_pct: f64) -> Self {
        TaxRules {
            country: region.country(),
            kind,
            slab_pct,
            exit_load_pct,
        }
    }

    /// Short-term rate, long-term rate (both as fractions, cess included)
    /// and the yearly tax-free long-term gain.
    fn rates(&self) -> (f64, f64, f64) {
        let cess = if self.country == Country::India {
            1.0 + INDIA_CESS_RATE
        } else {
            1.0
        };
        match self.kind {
            TaxKind::None => (0.0, 0.0, 0.0),
            TaxKind::Debt => (
                self.slab_pct / 100.0 * cess,
                self.slab_pct / 100.0 * cess,
                0.0,
            ),
            TaxKind::Equity | TaxKind::Arbitrage => (
                self.country.st_rate() / 100.0 * cess,
                self.country.lt_rate() / 100.0 * cess,
                self.country.exemption(),
            ),
        }
    }
}

/// One instalment (or lump sum) at redemption.
#[derive(Debug, Clone, Copy)]
pub struct RedeemLot {
    pub cost: f64,
    pub value: f64,
    pub held_months: u32,
}

#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct Redemption {
    pub gross: f64,
    pub exit_load: f64,
    pub tax: f64,
    /// `gross` minus exit load and tax.
    pub net: f64,
    pub short_term_lots: u32,
    pub long_term_lots: u32,
    pub short_term_gain: f64,
    pub long_term_gain: f64,
}

/// Redeem every lot at once: exit load on lots held under 12 months, then
/// tax per lot by holding period. Equity and arbitrage: short-term rate
/// under 12 months, long-term above with the yearly exemption; debt: slab
/// rate throughout. Gains of each kind are not set off against losses.
pub fn redeem(lots: &[RedeemLot], rules: &TaxRules) -> Redemption {
    let (st_rate, lt_rate, exemption) = rules.rates();
    let mut r = Redemption::default();
    for l in lots {
        let long = l.held_months >= 12;
        let load = if long {
            0.0
        } else {
            l.value * rules.exit_load_pct / 100.0
        };
        let gain = l.value - load - l.cost;
        r.gross += l.value;
        r.exit_load += load;
        if long {
            r.long_term_lots += 1;
            r.long_term_gain += gain;
        } else {
            r.short_term_lots += 1;
            r.short_term_gain += gain;
        }
    }
    r.tax =
        r.short_term_gain.max(0.0) * st_rate + (r.long_term_gain - exemption).max(0.0) * lt_rate;
    r.net = r.gross - r.exit_load - r.tax;
    r
}

/// The lots a SIP of `sip` a month (first year) leaves at the end, plus the
/// savings, held for the whole horizon.
fn lots_for(sip: f64, step_up_pct: f64, savings: f64, g: &[f64]) -> Vec<RedeemLot> {
    let months = g.len() as u32 - 1;
    let mut lots: Vec<RedeemLot> = (0..months)
        .map(|m| {
            let pay = sip * step_mult(m, step_up_pct);
            RedeemLot {
                cost: pay,
                value: pay * g[m as usize],
                held_months: months - m,
            }
        })
        .collect();
    if savings > 0.0 {
        lots.push(RedeemLot {
            cost: savings,
            value: savings * g[0],
            held_months: months,
        });
    }
    lots
}

/// The monthly SIP whose redemption leaves `target` in hand after tax and
/// exit load.
fn sip_for_net(target: f64, step_up_pct: f64, savings: f64, g: &[f64], rules: &TaxRules) -> f64 {
    let net = |s: f64| redeem(&lots_for(s, step_up_pct, savings, g), rules).net;
    if net(0.0) >= target {
        return 0.0;
    }
    let mut hi = gross_sip(target, step_up_pct, savings, g).max(1.0);
    while net(hi) < target && hi < 1e16 {
        hi *= 2.0;
    }
    let mut lo = 0.0;
    for _ in 0..80 {
        let mid = (lo + hi) / 2.0;
        if net(mid) < target {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

// ── Outcome bands ──

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Band {
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct Percentiles {
    /// The 10th, 50th and 90th percentile of the value the target is
    /// measured on (`basis`).
    pub p10: f64,
    pub p50: f64,
    pub p90: f64,
    /// Chance, in percent, of reaching the target.
    pub success_prob_pct: f64,
    /// `gross` or `net` (after tax and exit load).
    pub basis: &'static str,
    pub gross: Band,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net: Option<Band>,
}

/// Deterministic generator, so the same plan always prints the same bands.
struct Rng(u64);

impl Rng {
    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal pair (Box–Muller).
    fn normal_pair(&mut self) -> (f64, f64) {
        let (u1, u2) = (self.uniform(), self.uniform());
        let r = (-2.0 * u1.ln()).sqrt();
        let a = 2.0 * std::f64::consts::PI * u2;
        (r * a.cos(), r * a.sin())
    }
}

pub struct SimSpec<'a> {
    pub sip: f64,
    pub step_up_pct: f64,
    pub savings: f64,
    pub rates: &'a [f64],
    pub vols: &'a [f64],
    pub tax: Option<&'a TaxRules>,
    pub target: f64,
    /// The target is what is left after tax and exit load.
    pub target_is_net: bool,
}

const PATHS: usize = 2000;

fn pick(sorted: &[f64], p: f64) -> f64 {
    sorted[((sorted.len() - 1) as f64 * p).round() as usize]
}

fn band(mut v: Vec<f64>) -> Band {
    v.sort_by(|a, b| a.total_cmp(b));
    Band {
        p10: pick(&v, 0.10),
        p50: pick(&v, 0.50),
        p90: pick(&v, 0.90),
    }
}

/// Monte Carlo over monthly log returns with drift `ln(1 + monthly rate)`,
/// so the median path is the plan's compound growth. Lots are the SIP
/// instalments and the savings, redeemed at the end.
pub fn simulate(s: &SimSpec) -> Percentiles {
    let months = s.rates.len();
    let sips: Vec<f64> = (0..months as u32)
        .map(|m| s.sip * step_mult(m, s.step_up_pct))
        .collect();
    let mut rng = Rng(0x5710_C0DE);
    let (mut gross, mut net) = (Vec::with_capacity(PATHS), Vec::with_capacity(PATHS));
    let mut lots: Vec<RedeemLot> = Vec::with_capacity(months + 1);
    let mut hits = 0usize;
    let mut spare: Option<f64> = None;
    for _ in 0..PATHS {
        lots.clear();
        let mut growth = 0.0f64;
        for m in (0..months).rev() {
            let z = match spare.take() {
                Some(z) => z,
                None => {
                    let (a, b) = rng.normal_pair();
                    spare = Some(b);
                    a
                }
            };
            growth += (1.0 + s.rates[m]).ln() + s.vols[m] * z;
            lots.push(RedeemLot {
                cost: sips[m],
                value: sips[m] * growth.exp(),
                held_months: (months - m) as u32,
            });
        }
        if s.savings > 0.0 {
            lots.push(RedeemLot {
                cost: s.savings,
                value: s.savings * growth.exp(),
                held_months: months as u32,
            });
        }
        let total: f64 = lots.iter().map(|l| l.value).sum();
        gross.push(total);
        let measured = match s.tax {
            Some(rules) => {
                let n = redeem(&lots, rules).net;
                net.push(n);
                if s.target_is_net { n } else { total }
            }
            None => total,
        };
        if measured >= s.target * (1.0 - 1e-9) {
            hits += 1;
        }
    }
    let gross = band(gross);
    let net = (!net.is_empty()).then(|| band(net));
    let basis_band = match (&net, s.target_is_net) {
        (Some(n), true) => *n,
        _ => gross,
    };
    Percentiles {
        p10: basis_band.p10,
        p50: basis_band.p50,
        p90: basis_band.p90,
        success_prob_pct: hits as f64 / PATHS as f64 * 100.0,
        basis: if net.is_some() && s.target_is_net {
            "net"
        } else {
            "gross"
        },
        gross,
        net,
    }
}

// ── The SIP report ──

#[derive(Debug, Clone, Copy)]
pub struct SipRequest {
    pub target: f64,
    pub months: u32,
    pub return_pct: Option<f64>,
    pub step_up_pct: f64,
    pub savings: f64,
    /// `target` is in today's money and grows by this much a year.
    pub inflation_pct: Option<f64>,
    pub region: Region,
    /// Cached 364-day T-bill yield, when known.
    pub tbill_pct: Option<f64>,
    pub tax_kind: Option<TaxKind>,
    pub slab_pct: Option<f64>,
    pub exit_load_pct: Option<f64>,
    /// `target` is what must be left after tax and exit load.
    pub net: bool,
    pub glide: bool,
}

impl SipRequest {
    pub fn new(target: f64, months: u32, region: Region) -> Self {
        SipRequest {
            target,
            months,
            return_pct: None,
            step_up_pct: 0.0,
            savings: 0.0,
            inflation_pct: None,
            region,
            tbill_pct: None,
            tax_kind: None,
            slab_pct: None,
            exit_load_pct: None,
            net: false,
            glide: false,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TaxSummary {
    pub kind: TaxKind,
    pub slab_pct: f64,
    pub exit_load_pct: f64,
    pub short_term_lots: u32,
    pub long_term_lots: u32,
    pub short_term_gain: f64,
    pub long_term_gain: f64,
    pub exit_load: f64,
    pub tax: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SensPoint {
    pub return_pct: f64,
    pub monthly_sip: f64,
}

/// What an equity-like return on a short goal could leave, next to the
/// short-term yield.
#[derive(Debug, Clone, Serialize)]
pub struct Mismatch {
    pub return_pct: f64,
    /// 10th-percentile value, on the target's basis.
    pub p10: f64,
    pub shortfall_prob_pct: f64,
    pub safe_return_pct: f64,
    pub safe_monthly_sip: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TaxCompare {
    pub kind: TaxKind,
    pub net_after_tax: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SipReport {
    #[serde(flatten)]
    pub plan: SipPlan,
    pub regime: Regime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub glide: Option<Glide>,
    /// Target in today's money, when `--inflation` made `plan.target` larger.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_today: Option<f64>,
    pub inflation_pct: f64,
    /// `plan.target` in today's purchasing power, at `real_value_cpi_pct`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_value_today: Option<f64>,
    pub real_value_cpi_pct: f64,
    pub net_target: bool,
    pub tax: Option<TaxSummary>,
    /// What is left after tax and exit load when `monthly_sip` is paid.
    pub net_after_tax: Option<f64>,
    /// The monthly SIP that leaves `plan.target` after tax and exit load.
    pub monthly_sip_for_net_target: Option<f64>,
    pub percentiles: Percentiles,
    pub sensitivity: Vec<SensPoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mismatch: Option<Mismatch>,
    /// India, up to 36 months: the same plan taxed as debt and as arbitrage.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tax_compare: Vec<TaxCompare>,
    /// Filled in by the command layer from the fields above.
    pub warnings: Vec<String>,
}

struct Solved {
    plan: SipPlan,
    net_sip: Option<f64>,
    rates: Vec<f64>,
    g: Vec<f64>,
}

fn solve(
    req: &SipRequest,
    target: f64,
    return_pct: f64,
    glide: Option<Glide>,
    rules: Option<&TaxRules>,
) -> Solved {
    let rates = monthly_rates(req.months, return_pct, glide);
    let g = growth_to_end(&rates);
    let gross_sip = gross_sip(target, req.step_up_pct, req.savings, &g);
    let net_sip = rules.map(|r| sip_for_net(target, req.step_up_pct, req.savings, &g, r));
    let sip = if req.net {
        net_sip.unwrap_or(gross_sip)
    } else {
        gross_sip
    };
    let plan = build_plan(
        target,
        req.months,
        return_pct,
        req.step_up_pct,
        req.savings,
        &rates,
        sip,
    );
    Solved {
        plan,
        net_sip,
        rates,
        g,
    }
}

/// The monthly SIP for `req`: horizon-matched return and volatility, tax and
/// exit load at redemption, outcome bands, and return sensitivity.
pub fn report_sip(req: &SipRequest) -> SipReport {
    let months = req.months;
    let regime = regime_for(months, req.region, req.tbill_pct, req.return_pct);
    let infl = req.inflation_pct.unwrap_or(0.0);
    let target = if infl > 0.0 {
        inflate_months(req.target, infl, months)
    } else {
        req.target
    };
    let glide = (req.glide && months > 60).then(|| Glide {
        months: 24,
        to_pct: baseline(months, req.region, req.tbill_pct)
            .return_pct
            .min(regime.return_pct)
            .min(baseline(12, req.region, req.tbill_pct).return_pct),
    });
    let tax_kind = req
        .tax_kind
        .unwrap_or_else(|| default_tax_kind(regime.kind, req.region));
    let slab = req
        .slab_pct
        .unwrap_or_else(|| req.region.default_slab_pct());
    let load = req
        .exit_load_pct
        .unwrap_or_else(|| default_exit_load_pct(tax_kind, req.region));
    let rules =
        (tax_kind != TaxKind::None).then(|| TaxRules::new(req.region, tax_kind, slab, load));

    let main = solve(req, target, regime.return_pct, glide, rules.as_ref());
    let vols = monthly_vols(months, regime.vol_pct, glide);
    let sim_for = |sip: f64, rates: &[f64], vols: &[f64]| {
        simulate(&SimSpec {
            sip,
            step_up_pct: req.step_up_pct,
            savings: req.savings,
            rates,
            vols,
            tax: rules.as_ref(),
            target,
            target_is_net: req.net,
        })
    };
    let percentiles = sim_for(main.plan.monthly_sip, &main.rates, &vols);

    let redemption = rules.as_ref().map(|r| {
        redeem(
            &lots_for(main.plan.monthly_sip, req.step_up_pct, req.savings, &main.g),
            r,
        )
    });
    let tax = redemption.zip(rules.as_ref()).map(|(r, rules)| TaxSummary {
        kind: rules.kind,
        slab_pct: rules.slab_pct,
        exit_load_pct: rules.exit_load_pct,
        short_term_lots: r.short_term_lots,
        long_term_lots: r.long_term_lots,
        short_term_gain: r.short_term_gain,
        long_term_gain: r.long_term_gain,
        exit_load: r.exit_load,
        tax: r.tax,
    });

    let sensitivity = [-2.0, 0.0, 2.0]
        .iter()
        .map(|d| {
            let r = (regime.return_pct + d).max(0.0);
            let s = solve(req, target, r, glide, rules.as_ref());
            SensPoint {
                return_pct: r,
                monthly_sip: s.plan.monthly_sip,
            }
        })
        .collect();

    let mismatch =
        (months <= 36 && regime.return_pct >= req.region.equity_return_pct()).then(|| {
            let safe = baseline(months, req.region, req.tbill_pct);
            let s = solve(req, target, safe.return_pct, None, rules.as_ref());
            Mismatch {
                return_pct: regime.return_pct,
                p10: percentiles.p10,
                shortfall_prob_pct: 100.0 - percentiles.success_prob_pct,
                safe_return_pct: safe.return_pct,
                safe_monthly_sip: s.plan.monthly_sip,
            }
        });

    let tax_compare = if req.region == Region::India && months <= 36 && rules.is_some() {
        [TaxKind::Debt, TaxKind::Arbitrage]
            .iter()
            .map(|k| {
                let r = TaxRules::new(req.region, *k, slab, default_exit_load_pct(*k, req.region));
                TaxCompare {
                    kind: *k,
                    net_after_tax: redeem(
                        &lots_for(main.plan.monthly_sip, req.step_up_pct, req.savings, &main.g),
                        &r,
                    )
                    .net,
                }
            })
            .collect()
    } else {
        Vec::new()
    };

    let cpi = if infl > 0.0 {
        infl
    } else {
        req.region.default_cpi_pct()
    };
    let real_value_today = (months > 12 || infl > 0.0)
        .then(|| target / (1.0 + cpi / 100.0).powf(months as f64 / 12.0));
    SipReport {
        net_after_tax: redemption.map(|r| r.net),
        monthly_sip_for_net_target: main.net_sip,
        plan: main.plan,
        regime,
        glide,
        target_today: (infl > 0.0).then_some(req.target),
        inflation_pct: infl,
        real_value_today,
        real_value_cpi_pct: cpi,
        net_target: req.net && rules.is_some(),
        tax,
        percentiles,
        sensitivity,
        mismatch,
        tax_compare,
        warnings: Vec::new(),
    }
}

// ── Retirement ──

/// Corpus needed at retirement to pay `monthly_expense` (in money of the
/// first retirement year) at the start of every month for `years`, with the
/// expense rising by `inflation_pct` once a year while the rest earns
/// `return_pct` (monthly rate = annual ÷ 12). The corpus runs out exactly at
/// the end.
pub fn retirement_corpus(
    monthly_expense: f64,
    years: u32,
    inflation_pct: f64,
    return_pct: f64,
) -> f64 {
    let v = 1.0 / (1.0 + monthly_rate(return_pct));
    // Present value of 12 payments of 1 at the start of each month.
    let year: f64 = (0..12).map(|m| v.powi(m)).sum();
    // Each year's payments are `inflation` higher and 12 months further away.
    let q = (1.0 + inflation_pct / 100.0) * v.powi(12);
    let years_sum: f64 = (0..years).map(|y| q.powi(y as i32)).sum();
    monthly_expense * year * years_sum
}

/// Months the corpus pays `first_monthly` (rising by `inflation_pct` a year,
/// paid at the start of each month) after a one-off drop of `crash_pct` at
/// the start, up to `max_months`.
pub fn survival_months(
    corpus: f64,
    first_monthly: f64,
    inflation_pct: f64,
    return_pct: f64,
    crash_pct: f64,
    max_months: u32,
) -> u32 {
    let i = monthly_rate(return_pct);
    let mut left = corpus * (1.0 - crash_pct / 100.0);
    for m in 0..max_months {
        let pay = first_monthly * (1.0 + inflation_pct / 100.0).powi((m / 12) as i32);
        // Float noise at the last payment must not cost a month.
        if left < pay - corpus.abs() * 1e-9 - 1e-6 {
            return m;
        }
        left = (left - pay) * (1.0 + i);
    }
    max_months
}

/// Year-one tax on a yearly withdrawal `annual` from a corpus that is
/// `gain_fraction` gain: an equity SWP (gain share only, above the exemption)
/// against the same income as FD interest at the slab rate.
#[derive(Debug, Clone, Serialize)]
pub struct SwpDrag {
    pub annual_withdrawal: f64,
    pub gain_fraction: f64,
    pub swp_tax: f64,
    pub fd_tax: f64,
}

pub fn swp_drag(annual: f64, gain_fraction: f64, slab_pct: f64, region: Region) -> SwpDrag {
    let kind = if region == Region::Other {
        TaxKind::None
    } else {
        TaxKind::Equity
    };
    let rules = TaxRules::new(region, kind, slab_pct, 0.0);
    let (_, lt, exemption) = rules.rates();
    let cess = if region == Region::India {
        1.0 + INDIA_CESS_RATE
    } else {
        1.0
    };
    SwpDrag {
        annual_withdrawal: annual,
        gain_fraction,
        swp_tax: (annual * gain_fraction - exemption).max(0.0) * lt,
        fd_tax: annual * slab_pct / 100.0 * cess,
    }
}

/// Optional extras for [`plan_retirement_with`].
#[derive(Debug, Clone, Copy)]
pub struct RetireOpts {
    /// Pension, rent or similar, in today's money; it rises with inflation.
    pub monthly_income: f64,
    /// Extra years the corpus must last.
    pub buffer_years: u32,
    /// Amount to leave at the end, in today's money.
    pub legacy: f64,
    pub region: Region,
    pub slab_pct: f64,
}

impl Default for RetireOpts {
    fn default() -> Self {
        RetireOpts {
            monthly_income: 0.0,
            buffer_years: 0,
            legacy: 0.0,
            region: Region::Other,
            slab_pct: 30.0,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct RetirePlan {
    pub monthly_expense_today: f64,
    pub years_to_retire: u32,
    pub years_in_retirement: u32,
    pub inflation_pct: f64,
    pub return_pre_pct: f64,
    pub return_post_pct: f64,
    /// Today's monthly expense in money of the first retirement year.
    pub monthly_expense_at_retirement: f64,
    pub monthly_income_today: f64,
    /// Expense minus income, in money of the first retirement year.
    pub monthly_need_at_retirement: f64,
    pub buffer_years: u32,
    pub legacy: f64,
    /// Corpus needed on the day you retire.
    pub corpus: f64,
    /// First year's withdrawals as a share of the corpus, in percent.
    pub initial_withdrawal_rate_pct: f64,
    /// Years the corpus lasts at the assumed return.
    pub survival_years: f64,
    /// Years it lasts if it falls 30% on the first day of retirement.
    pub crash_survival_years: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swp_drag: Option<SwpDrag>,
    /// How to build `corpus` before retiring.
    pub sip: SipPlan,
}

pub fn plan_retirement(
    monthly_expense_today: f64,
    years_to_retire: u32,
    years_in_retirement: u32,
    inflation_pct: f64,
    return_pre_pct: f64,
    return_post_pct: f64,
    step_up_pct: f64,
    savings: f64,
) -> RetirePlan {
    plan_retirement_with(
        monthly_expense_today,
        years_to_retire,
        years_in_retirement,
        inflation_pct,
        return_pre_pct,
        return_post_pct,
        step_up_pct,
        savings,
        &RetireOpts::default(),
    )
}

#[allow(clippy::too_many_arguments)]
pub fn plan_retirement_with(
    monthly_expense_today: f64,
    years_to_retire: u32,
    years_in_retirement: u32,
    inflation_pct: f64,
    return_pre_pct: f64,
    return_post_pct: f64,
    step_up_pct: f64,
    savings: f64,
    opts: &RetireOpts,
) -> RetirePlan {
    let at_retirement = inflate(monthly_expense_today, inflation_pct, years_to_retire);
    let need = (monthly_expense_today - opts.monthly_income).max(0.0);
    let need_at_retirement = inflate(need, inflation_pct, years_to_retire);
    let lasts = years_in_retirement + opts.buffer_years;
    let mut corpus = retirement_corpus(need_at_retirement, lasts, inflation_pct, return_post_pct);
    if opts.legacy > 0.0 {
        let left = inflate(opts.legacy, inflation_pct, years_to_retire + lasts);
        corpus += left / (1.0 + monthly_rate(return_post_pct)).powi((12 * lasts) as i32);
    }
    let sip = plan_sip(
        corpus,
        years_to_retire,
        return_pre_pct,
        step_up_pct,
        savings,
    );
    let max = 1200;
    let years = |crash| {
        survival_months(
            corpus,
            need_at_retirement,
            inflation_pct,
            return_post_pct,
            crash,
            max,
        ) as f64
            / 12.0
    };
    let annual = need_at_retirement * 12.0;
    let gain_fraction = if corpus > 0.0 {
        (1.0 - (sip.total_invested + savings) / corpus).clamp(0.0, 1.0)
    } else {
        0.0
    };
    RetirePlan {
        monthly_expense_today,
        years_to_retire,
        years_in_retirement,
        inflation_pct,
        return_pre_pct,
        return_post_pct,
        monthly_expense_at_retirement: at_retirement,
        monthly_income_today: opts.monthly_income,
        monthly_need_at_retirement: need_at_retirement,
        buffer_years: opts.buffer_years,
        legacy: opts.legacy,
        corpus,
        initial_withdrawal_rate_pct: if corpus > 0.0 {
            annual / corpus * 100.0
        } else {
            0.0
        },
        survival_years: years(0.0),
        crash_survival_years: years(30.0),
        swp_drag: (opts.region != Region::Other && corpus > 0.0)
            .then(|| swp_drag(annual, gain_fraction, opts.slab_pct, opts.region)),
        sip,
    }
}

// ── Education ──

#[derive(Debug, Clone, Serialize)]
pub struct EducationPlan {
    pub cost_today: f64,
    pub years: u32,
    pub months: u32,
    pub inflation_pct: f64,
    /// Today's cost after the horizon of education inflation.
    pub future_cost: f64,
    pub sip: SipReport,
}

pub fn plan_education(
    cost_today: f64,
    months: u32,
    inflation_pct: f64,
    mut req: SipRequest,
) -> EducationPlan {
    let future_cost = inflate_months(cost_today, inflation_pct, months);
    req.target = future_cost;
    req.months = months;
    req.inflation_pct = None;
    EducationPlan {
        cost_today,
        years: months.div_ceil(12),
        months,
        inflation_pct,
        future_cost,
        sip: report_sip(&req),
    }
}

// ── SWP ──

/// How one withdrawal plan plays out.
#[derive(Debug, Clone, Serialize)]
pub struct SwpPath {
    /// Months of full withdrawals paid before the money ran out.
    pub runway_months: u32,
    /// The money lasted the whole horizon.
    pub lasts: bool,
    pub final_value: f64,
    pub total_withdrawn: f64,
    pub total_tax: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SwpPlan {
    pub corpus: f64,
    pub cost_basis: f64,
    pub monthly_withdrawal: f64,
    pub inflation_pct: f64,
    pub return_pct: f64,
    pub years: u32,
    pub fd_return_pct: f64,
    pub slab_pct: f64,
    pub initial_withdrawal_rate_pct: f64,
    /// Equity funds: tax on the gain share of each withdrawal only.
    pub swp: SwpPath,
    /// An FD: interest taxed at the slab rate every year.
    pub fd: SwpPath,
}

/// Withdraw `monthly` (rising by `inflation_pct` a year, at the start of each
/// month) for up to `years`. With `gain_taxed`, each withdrawal sells units:
/// its gain share is taxed at the long-term rate above the yearly exemption.
/// Otherwise the year's interest is taxed at the slab rate.
#[allow(clippy::too_many_arguments)]
fn swp_path(
    corpus: f64,
    cost: f64,
    monthly: f64,
    inflation_pct: f64,
    return_pct: f64,
    years: u32,
    gain_taxed: bool,
    slab_pct: f64,
    region: Region,
) -> SwpPath {
    let i = monthly_rate(return_pct);
    let kind = if region == Region::Other {
        TaxKind::None
    } else {
        TaxKind::Equity
    };
    let eq = TaxRules::new(region, kind, slab_pct, 0.0);
    let (_, lt, exemption) = eq.rates();
    let cess = if region == Region::India {
        1.0 + INDIA_CESS_RATE
    } else {
        1.0
    };
    let (mut value, mut cost) = (corpus, cost.min(corpus));
    let (mut withdrawn, mut total_tax) = (0.0, 0.0);
    let (mut year_gain, mut year_start) = (0.0, corpus);
    let months = years * 12;
    let mut paid = 0;
    for m in 0..months {
        let pay = monthly * (1.0 + inflation_pct / 100.0).powi((m / 12) as i32);
        if value < pay - 1e-6 {
            break;
        }
        if gain_taxed {
            let frac = ((value - cost) / value).max(0.0);
            year_gain += pay * frac;
            cost -= pay * (1.0 - frac);
        }
        value = (value - pay) * (1.0 + i);
        withdrawn += pay;
        paid += 1;
        if (m + 1) % 12 == 0 {
            let tax = if gain_taxed {
                (year_gain - exemption).max(0.0) * lt
            } else {
                ((value - year_start + withdrawn_in_year(monthly, inflation_pct, m)).max(0.0))
                    * slab_pct
                    / 100.0
                    * cess
            };
            value = (value - tax).max(0.0);
            total_tax += tax;
            year_gain = 0.0;
            year_start = value;
        }
    }
    SwpPath {
        runway_months: paid,
        lasts: paid == months,
        final_value: value,
        total_withdrawn: withdrawn,
        total_tax,
    }
}

/// Withdrawals paid in the 12 months ending with month `m` (0-based).
fn withdrawn_in_year(monthly: f64, inflation_pct: f64, m: u32) -> f64 {
    12.0 * monthly * (1.0 + inflation_pct / 100.0).powi((m / 12) as i32)
}

#[allow(clippy::too_many_arguments)]
pub fn plan_swp(
    corpus: f64,
    cost_basis: Option<f64>,
    monthly_withdrawal: f64,
    inflation_pct: f64,
    return_pct: f64,
    years: u32,
    fd_return_pct: f64,
    slab_pct: f64,
    region: Region,
) -> SwpPlan {
    let cost = cost_basis.unwrap_or(corpus);
    SwpPlan {
        corpus,
        cost_basis: cost,
        monthly_withdrawal,
        inflation_pct,
        return_pct,
        years,
        fd_return_pct,
        slab_pct,
        initial_withdrawal_rate_pct: monthly_withdrawal * 12.0 / corpus * 100.0,
        swp: swp_path(
            corpus,
            cost,
            monthly_withdrawal,
            inflation_pct,
            return_pct,
            years,
            true,
            slab_pct,
            region,
        ),
        fd: swp_path(
            corpus,
            corpus,
            monthly_withdrawal,
            inflation_pct,
            fd_return_pct,
            years,
            false,
            slab_pct,
            region,
        ),
    }
}

// ── Emergency fund ──

#[derive(Debug, Clone, Serialize)]
pub struct EmergencyPlan {
    pub monthly_expense: f64,
    pub months_cover: u32,
    /// Reserve to hold: expenses × months of cover.
    pub target: f64,
    /// Liquid money already set aside.
    pub liquid: f64,
    pub funded_pct: f64,
    pub horizon_months: u32,
    /// Monthly amount that closes the gap in `horizon_months`.
    pub build: SipPlan,
}

pub fn plan_emergency(
    monthly_expense: f64,
    months_cover: u32,
    liquid: f64,
    horizon_months: u32,
    return_pct: f64,
) -> EmergencyPlan {
    let target = monthly_expense * months_cover as f64;
    EmergencyPlan {
        monthly_expense,
        months_cover,
        target,
        liquid,
        funded_pct: (liquid / target * 100.0).min(100.0),
        horizon_months,
        build: plan_sip_months(target, horizon_months, return_pct, 0.0, liquid, None),
    }
}

// ── Loan prepayment against a SIP ──

#[derive(Debug, Clone, Serialize)]
pub struct PrepayPlan {
    pub principal: f64,
    pub loan_rate_pct: f64,
    pub remaining_months: u32,
    pub emi: f64,
    pub extra_monthly: f64,
    pub lump_sum: f64,
    pub new_tenure_months: u32,
    pub tenure_saved_months: u32,
    pub interest_without_prepay: f64,
    pub interest_saved: f64,
    pub sip_return_pct: f64,
    pub sip_tax: TaxKind,
    /// What the same money leaves in a SIP after tax and exit load.
    pub sip_net: f64,
    /// What the same money saves by prepaying, grown at the loan rate.
    pub prepay_equivalent: f64,
    pub sip_percentiles: Percentiles,
    /// Return, before tax, the SIP needs to match prepaying; `None` when no
    /// return up to 60% does.
    pub breakeven_return_pct: Option<f64>,
}

/// EMI of a loan of `principal` over `months` at `rate_pct` a year.
pub fn emi(principal: f64, rate_pct: f64, months: u32) -> f64 {
    let i = rate_pct / 1200.0;
    if i.abs() < 1e-12 {
        principal / months as f64
    } else {
        principal * i / (1.0 - (1.0 + i).powi(-(months as i32)))
    }
}

/// Months to clear `balance` paying `payment` a month at `rate_pct`, and the
/// interest paid; `None` when the payment never covers the interest.
fn loan_runoff(balance: f64, rate_pct: f64, payment: f64) -> Option<(u32, f64)> {
    let i = rate_pct / 1200.0;
    let (mut bal, mut interest, mut n) = (balance, 0.0, 0u32);
    while bal > 1e-6 {
        let int = bal * i;
        if payment <= int + 1e-9 || n > 1200 {
            return None;
        }
        interest += int;
        bal = bal + int - payment;
        n += 1;
    }
    Some((n, interest))
}

#[allow(clippy::too_many_arguments)]
pub fn plan_prepay(
    principal: f64,
    loan_rate_pct: f64,
    remaining_months: u32,
    extra_monthly: f64,
    lump_sum: f64,
    sip_return_pct: f64,
    vol_pct: f64,
    rules: &TaxRules,
) -> PrepayPlan {
    let emi_amount = emi(principal, loan_rate_pct, remaining_months);
    let original = emi_amount * remaining_months as f64 - principal;
    let (months, interest) = loan_runoff(
        (principal - lump_sum).max(0.0),
        loan_rate_pct,
        emi_amount + extra_monthly,
    )
    .unwrap_or((remaining_months, original));
    let months = months.min(remaining_months);

    let rates = monthly_rates(remaining_months, sip_return_pct, None);
    let g = growth_to_end(&rates);
    let lots = lots_for(extra_monthly, 0.0, lump_sum, &g);
    let net = redeem(&lots, rules).net;
    let i_loan = loan_rate_pct / 1200.0;
    let equivalent = extra_monthly * annuity_due_fv(i_loan, remaining_months)
        + lump_sum * (1.0 + i_loan).powi(remaining_months as i32);

    let net_at = |r: f64| {
        let g = growth_to_end(&monthly_rates(remaining_months, r, None));
        redeem(&lots_for(extra_monthly, 0.0, lump_sum, &g), rules).net
    };
    let breakeven = if net_at(60.0) < equivalent {
        None
    } else {
        let (mut lo, mut hi) = (0.0, 60.0);
        for _ in 0..60 {
            let mid = (lo + hi) / 2.0;
            if net_at(mid) < equivalent {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some(hi)
    };

    let vols = monthly_vols(remaining_months, vol_pct, None);
    let percentiles = simulate(&SimSpec {
        sip: extra_monthly,
        step_up_pct: 0.0,
        savings: lump_sum,
        rates: &rates,
        vols: &vols,
        tax: Some(rules),
        target: equivalent,
        target_is_net: true,
    });
    PrepayPlan {
        principal,
        loan_rate_pct,
        remaining_months,
        emi: emi_amount,
        extra_monthly,
        lump_sum,
        new_tenure_months: months,
        tenure_saved_months: remaining_months - months,
        interest_without_prepay: original,
        interest_saved: (original - interest).max(0.0),
        sip_return_pct,
        sip_tax: rules.kind,
        sip_net: net,
        prepay_equivalent: equivalent,
        sip_percentiles: percentiles,
        breakeven_return_pct: breakeven,
    }
}

// ── Saved goals ──

/// A goal kept in `goals.json`.
#[derive(Debug, Clone, Serialize, serde::Deserialize, PartialEq)]
pub struct SavedGoal {
    pub name: String,
    /// In today's money when `inflation_pct` is set, else the future bill.
    pub target: f64,
    /// Due date, `YYYY-MM` or `YYYY-MM-DD`.
    pub by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inflation_pct: Option<f64>,
    /// Holdings that count towards the goal (`MF:122639`, `NIFTYBEES`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub holdings: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_pct: Option<f64>,
    #[serde(default)]
    pub created: String,
}

#[derive(Debug, Default, Clone, Serialize, serde::Deserialize)]
pub struct GoalStore {
    #[serde(default)]
    pub goals: Vec<SavedGoal>,
}

impl GoalStore {
    pub fn find(&self, name: &str) -> Option<&SavedGoal> {
        self.goals
            .iter()
            .find(|g| g.name.eq_ignore_ascii_case(name))
    }
}

/// Indian digit grouping: 12345678.4 → "1,23,45,678".
pub fn indian_grouping(amount: f64) -> String {
    let n = amount.abs().round() as u64;
    let digits = n.to_string();
    let grouped = if digits.len() <= 3 {
        digits
    } else {
        let (head, last3) = digits.split_at(digits.len() - 3);
        let mut parts: Vec<String> = Vec::new();
        let mut rest = head;
        while rest.len() > 2 {
            let (a, b) = rest.split_at(rest.len() - 2);
            parts.push(b.to_string());
            rest = a;
        }
        parts.push(rest.to_string());
        parts.reverse();
        format!("{},{}", parts.join(","), last3)
    };
    if amount < 0.0 && n > 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// Thousands grouping: 1234567.4 → "1,234,567".
pub fn western_grouping(amount: f64) -> String {
    let n = amount.abs().round() as u64;
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    if amount < 0.0 && n > 0 {
        format!("-{out}")
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f64, b: f64, tol: f64) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn indian_amounts_parse() {
        assert_eq!(parse_amount("1cr"), Ok(1e7));
        assert_eq!(parse_amount("1.5 Crore"), Ok(1.5e7));
        assert_eq!(parse_amount("25L"), Ok(25e5));
        assert_eq!(parse_amount("25 lakh"), Ok(25e5));
        assert_eq!(parse_amount("7lacs"), Ok(7e5));
        assert_eq!(parse_amount("50k"), Ok(5e4));
        assert_eq!(parse_amount("2m"), Ok(2e6));
        assert_eq!(parse_amount("₹2,50,000"), Ok(250_000.0));
        assert_eq!(parse_amount("80000"), Ok(80_000.0));
        for bad in ["", "abc", "1x", "-5L", "0", "1.2.3cr", "cr"] {
            assert!(parse_amount(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn sip_for_one_crore_in_fifteen_years() {
        // i = 1% a month, n = 180: FV factor = ((1.01^180 − 1) / 0.01) × 1.01
        // = (5.995802 − 1) / 0.01 × 1.01 = 504.576; ₹1 Cr / 504.576 = ₹19,819.
        let p = plan_sip(1e7, 15, 12.0, 0.0, 0.0);
        assert!(near(p.monthly_sip, 19_818.6, 1.0), "{}", p.monthly_sip);
        assert!(near(p.total_invested, 19_818.6 * 180.0, 200.0));
        assert!(near(p.schedule.last().unwrap().value, 1e7, 1e-3));
        // Lump sum today: 1 Cr / 1.01^180 = ₹16.68 L.
        assert!(
            near(p.lump_sum_today, 1_667_833.0, 5.0),
            "{}",
            p.lump_sum_today
        );
        assert_eq!(p.final_monthly_sip, p.monthly_sip);
    }

    #[test]
    fn step_up_lowers_the_first_sip() {
        // 10% yearly step-up: factor = 12.80933 × Σ 1.1^y × 1.126825^(14−y)
        // = 12.80933 × 67.794 = 868.39, so ₹1 Cr needs ₹11,516 a month at first.
        let p = plan_sip(1e7, 15, 12.0, 10.0, 0.0);
        assert!(near(p.monthly_sip, 11_515.6, 1.0), "{}", p.monthly_sip);
        assert!(near(
            p.final_monthly_sip,
            p.monthly_sip * 1.1f64.powi(14),
            1e-6
        ));
        // Month by month, the stepped-up SIP lands on the target.
        let i = 0.01;
        let mut value = 0.0;
        for y in 0..15 {
            for _ in 0..12 {
                value = (value + p.monthly_sip * 1.1f64.powi(y)) * (1.0 + i);
            }
        }
        assert!(near(value, 1e7, 1e-3), "{value}");
    }

    #[test]
    fn zero_return_is_just_saving() {
        let p = plan_sip(1_200_000.0, 10, 0.0, 0.0, 0.0);
        assert!(near(p.monthly_sip, 10_000.0, 1e-9));
        assert!(near(p.gains, 0.0, 1e-6));
    }

    #[test]
    fn savings_reduce_or_cover_the_sip() {
        // ₹10 L saved grows to 10 L × 1.01^180 = ₹59.96 L of the ₹1 Cr.
        let p = plan_sip(1e7, 15, 12.0, 0.0, 1e6);
        assert!(near(p.savings_future_value, 5_995_802.0, 5.0));
        assert!(near(p.monthly_sip, (1e7 - 5_995_802.0) / 504.576, 1.0));
        let covered = plan_sip(1e6, 15, 12.0, 0.0, 1e6);
        assert_eq!(covered.monthly_sip, 0.0);
        assert!(covered.schedule.iter().all(|y| y.monthly_sip == 0.0));
    }

    #[test]
    fn retirement_corpus_matches_the_closed_form() {
        // ₹80,000 today at 6% for 20 years: 80,000 × 1.06^20 = ₹2,56,571 a month.
        let plan = plan_retirement(80_000.0, 20, 30, 6.0, 12.0, 7.0, 0.0, 0.0);
        assert!(near(plan.monthly_expense_at_retirement, 256_570.8, 0.5));
        // v = 1/1.0058333; a year of payments = (1 − v^12)/(1 − v) = 11.62454;
        // q = 1.06 × v^12 = 0.988538; Σ q^y (30 years) = 25.50867;
        // corpus = 2,56,570.84 × 11.62454 × 25.50867 = ₹7.608 Cr.
        assert!(near(plan.corpus, 76_080_039.0, 50.0), "{}", plan.corpus);
        // Drawing the expense month by month (rising 6% a year) empties it.
        let i = 0.07 / 12.0;
        let mut left = plan.corpus;
        for y in 0..30 {
            for _ in 0..12 {
                left = (left - plan.monthly_expense_at_retirement * 1.06f64.powi(y)) * (1.0 + i);
            }
        }
        assert!(left.abs() < 1.0, "{left}");
        // And the SIP builds exactly that corpus in 20 years at 12%.
        assert!(near(
            plan.sip.schedule.last().unwrap().value,
            plan.corpus,
            1e-2
        ));
    }

    #[test]
    fn retirement_when_return_equals_inflation_is_expense_times_months() {
        // With a 0% return and 0% inflation the corpus is every payment summed.
        let c = retirement_corpus(50_000.0, 25, 0.0, 0.0);
        assert!(near(c, 50_000.0 * 12.0 * 25.0, 1e-6));
    }

    #[test]
    fn education_goal_inflates_the_cost() {
        // ₹25 L at 8% for 10 years: 25 L × 1.08^10 = ₹53.97 L. At 12%, the
        // 120-month factor is ((1.01^120 − 1)/0.01) × 1.01 = 232.339, so the
        // SIP is 53,97,312 / 232.339 = ₹23,230.
        let mut req = SipRequest::new(0.0, 0, Region::India);
        req.return_pct = Some(12.0);
        req.tax_kind = Some(TaxKind::None);
        let e = plan_education(25e5, 120, 8.0, req);
        assert!(near(e.future_cost, 5_397_312.5, 1.0), "{}", e.future_cost);
        assert!(
            near(e.sip.plan.monthly_sip, 23_230.0, 2.0),
            "{}",
            e.sip.plan.monthly_sip
        );
    }

    #[test]
    fn indian_grouping_puts_commas_in_lakhs_and_crores() {
        assert_eq!(indian_grouping(0.0), "0");
        assert_eq!(indian_grouping(999.0), "999");
        assert_eq!(indian_grouping(19_818.6), "19,819");
        assert_eq!(indian_grouping(250_000.0), "2,50,000");
        assert_eq!(indian_grouping(12_345_678.4), "1,23,45,678");
        assert_eq!(indian_grouping(-1_500.0), "-1,500");
        assert_eq!(western_grouping(999.0), "999");
        assert_eq!(western_grouping(1_234_567.4), "1,234,567");
        assert_eq!(western_grouping(-1_000.0), "-1,000");
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn horizons_parse_in_months_years_and_dates() {
        assert_eq!(parse_duration("18mo"), Ok(18));
        assert_eq!(parse_duration("6 months"), Ok(6));
        assert_eq!(parse_duration("2y"), Ok(24));
        assert_eq!(parse_duration("1.5y"), Ok(18));
        assert_eq!(parse_duration("15"), Ok(180));
        for bad in ["", "0mo", "x", "5w", "1.3mo", "61y"] {
            assert!(parse_duration(bad).is_err(), "{bad:?}");
        }
        let today = d("2026-11-08");
        assert_eq!(months_until(today, "2027-08"), Ok(8));
        assert_eq!(months_until(today, "2027-08-08"), Ok(9));
        assert_eq!(months_until(today, "2027-11-07"), Ok(11));
        assert!(
            months_until(today, "2026-11-20")
                .unwrap_err()
                .contains("1 month to 60")
        );
        assert!(months_until(today, "2090-01").is_err());
        assert!(
            months_until(today, "2026-01")
                .unwrap_err()
                .contains("not in the future")
        );
        assert!(months_until(today, "2020-01").is_err());
        assert!(months_until(today, "soon").is_err());
        assert_eq!(duration_parts(18), (1, 6));
    }

    #[test]
    fn regimes_follow_the_horizon() {
        let r = baseline(12, Region::India, None);
        assert_eq!(
            (r.kind, r.return_pct, r.vol_pct),
            (RegimeKind::Short, 6.75, 2.0)
        );
        assert_eq!(baseline(12, Region::India, Some(6.4)).return_pct, 6.4);
        assert_eq!(baseline(12, Region::India, Some(6.4)).source, "tbill");
        assert_eq!(baseline(12, Region::Us, Some(6.4)).return_pct, 4.25);
        assert_eq!(baseline(24, Region::India, None).kind, RegimeKind::Short);
        assert_eq!(baseline(25, Region::India, None).return_pct, 9.0);
        assert_eq!(baseline(60, Region::Us, None).return_pct, 6.5);
        let long = baseline(61, Region::India, None);
        assert_eq!(
            (long.kind, long.return_pct, long.vol_pct),
            (RegimeKind::Long, 12.0, 15.0)
        );
        assert_eq!(baseline(120, Region::Us, None).return_pct, 9.0);
        // A user return on a short goal keeps its own risk level.
        let u = regime_for(12, Region::India, None, Some(12.0));
        assert_eq!((u.source, u.vol_pct), ("user", 15.0));
    }

    #[test]
    fn sub_year_plans_land_on_the_target() {
        // 6 months at 12%: factor = ((1.01^6 − 1)/0.01) × 1.01 = 6.2135.
        let p = plan_sip_months(100_000.0, 6, 12.0, 0.0, 0.0, None);
        assert_eq!((p.months, p.years), (6, 1));
        assert!(
            near(p.monthly_sip, 100_000.0 / 6.213_54, 0.5),
            "{}",
            p.monthly_sip
        );
        assert!(near(p.periods.last().unwrap().value, 100_000.0, 1e-6));
        assert_eq!(p.monthly_schedule.len(), 6);
        assert_eq!(p.schedule.len(), 1);
        // 18 months: step-up applies from month 13.
        let p = plan_sip_months(500_000.0, 18, 10.0, 10.0, 0.0, None);
        assert!(near(p.periods.last().unwrap().value, 500_000.0, 1e-6));
        assert!(near(p.final_monthly_sip, p.monthly_sip * 1.1, 1e-9));
        assert_eq!(p.schedule.len(), 2);
        // The 12% label is 12.68% once compounded monthly.
        assert!(near(p.effective_annual_pct, 10.4713, 1e-3));
    }

    #[test]
    fn a_glide_path_lowers_the_late_return() {
        let flat = plan_sip_months(1e7, 120, 12.0, 0.0, 0.0, None);
        let glide = plan_sip_months(
            1e7,
            120,
            12.0,
            0.0,
            0.0,
            Some(Glide {
                months: 24,
                to_pct: 6.0,
            }),
        );
        assert!(glide.monthly_sip > flat.monthly_sip);
        assert!(near(glide.periods.last().unwrap().value, 1e7, 1e-3));
    }

    fn equity_india(load: f64) -> TaxRules {
        TaxRules::new(Region::India, TaxKind::Equity, 30.0, load)
    }

    #[test]
    fn one_year_sip_has_eleven_short_term_lots() {
        // ₹10,000 a month for 12 months at 12%: the first instalment is held
        // 12 months (long-term, gain ₹1,268 — inside the ₹1.25 L exemption);
        // the other 11 are held 11..1 months (STCG 20% + 4% cess, 1% load).
        let rates = monthly_rates(12, 12.0, None);
        let g = growth_to_end(&rates);
        let r = redeem(&lots_for(10_000.0, 0.0, 0.0, &g), &equity_india(1.0));
        assert_eq!((r.short_term_lots, r.long_term_lots), (11, 1));
        let (mut gain, mut load, mut gross) = (0.0, 0.0, 0.0);
        for m in 1..12 {
            let v = 10_000.0 * 1.01f64.powi(12 - m);
            load += v * 0.01;
            gain += v * 0.99 - 10_000.0;
            gross += v;
        }
        let first = 10_000.0 * 1.01f64.powi(12);
        assert!(near(r.exit_load, load, 1e-6));
        assert!(near(r.short_term_gain, gain, 1e-6));
        assert!(near(r.tax, gain * 0.2 * 1.04, 1e-6), "{}", r.tax);
        assert!(near(r.gross, gross + first, 1e-6));
        assert!(near(r.net, r.gross - r.exit_load - r.tax, 1e-9));
    }

    #[test]
    fn ten_year_sip_uses_the_exemption_and_taxes_the_last_eleven_lots() {
        let rates = monthly_rates(120, 12.0, None);
        let g = growth_to_end(&rates);
        let r = redeem(&lots_for(50_000.0, 0.0, 0.0, &g), &equity_india(0.0));
        assert_eq!((r.short_term_lots, r.long_term_lots), (11, 109));
        let (mut st, mut lt) = (0.0, 0.0);
        for m in 0..120 {
            let v = 50_000.0 * 1.01f64.powi(120 - m);
            if 120 - m >= 12 {
                lt += v - 50_000.0
            } else {
                st += v - 50_000.0
            }
        }
        let want = st * 0.2 * 1.04 + (lt - 125_000.0) * 0.125 * 1.04;
        assert!(near(r.tax, want, 1e-4), "{} vs {want}", r.tax);
        // Equity LTCG is 13% with cess.
        assert!(near(
            (lt - 125_000.0) * 0.125 * 1.04 / (lt - 125_000.0),
            0.13,
            1e-12
        ));
    }

    #[test]
    fn debt_is_taxed_at_the_slab_rate_and_none_is_untaxed() {
        let lots = [RedeemLot {
            cost: 100_000.0,
            value: 110_000.0,
            held_months: 3,
        }];
        let debt = TaxRules::new(Region::India, TaxKind::Debt, 30.0, 0.0);
        assert!(near(redeem(&lots, &debt).tax, 10_000.0 * 0.30 * 1.04, 1e-9));
        let long = [RedeemLot {
            cost: 100_000.0,
            value: 110_000.0,
            held_months: 40,
        }];
        assert!(near(redeem(&long, &debt).tax, 3_120.0, 1e-9));
        let none = TaxRules::new(Region::India, TaxKind::None, 30.0, 0.0);
        assert_eq!(redeem(&long, &none).net, 110_000.0);
        let us = TaxRules::new(Region::Us, TaxKind::Equity, 24.0, 0.0);
        assert!(near(redeem(&lots, &us).tax, 10_000.0 * 0.37, 1e-9));
        assert!(near(redeem(&long, &us).tax, 10_000.0 * 0.20, 1e-9));
    }

    #[test]
    fn net_target_sip_leaves_the_target_in_hand() {
        let mut req = SipRequest::new(1_000_000.0, 12, Region::India);
        req.tax_kind = Some(TaxKind::Equity);
        req.return_pct = Some(12.0);
        let r = report_sip(&req);
        let gross = r.plan.monthly_sip;
        let net_sip = r.monthly_sip_for_net_target.unwrap();
        assert!(net_sip > gross);
        assert!(r.net_after_tax.unwrap() < 1_000_000.0);
        req.net = true;
        let n = report_sip(&req);
        assert!(near(n.plan.monthly_sip, net_sip, 1e-6));
        assert!(
            near(n.net_after_tax.unwrap(), 1_000_000.0, 0.01),
            "{:?}",
            n.net_after_tax
        );
        assert!(n.net_target);
    }

    #[test]
    fn bands_collapse_without_volatility_and_follow_the_median_with_it() {
        let rates = monthly_rates(120, 12.0, None);
        let flat = vec![0.0; 120];
        let spec = |vols: &[f64]| {
            simulate(&SimSpec {
                sip: 10_000.0,
                step_up_pct: 0.0,
                savings: 0.0,
                rates: &rates,
                vols,
                tax: None,
                target: 2_300_000.0,
                target_is_net: false,
            })
        };
        let p = spec(&flat);
        let want = 10_000.0 * annuity_due_fv(0.01, 120);
        assert!(near(p.p10, want, 1e-3) && near(p.p90, want, 1e-3), "{p:?}");
        assert!(p.success_prob_pct > 99.0 || want < 2_300_000.0);
        let vols = vec![0.15 / 12f64.sqrt(); 120];
        let p = spec(&vols);
        assert!(p.p10 < p.p50 && p.p50 < p.p90, "{p:?}");
        assert!((p.p50 / want - 1.0).abs() < 0.06, "{} vs {want}", p.p50);
        assert_eq!(spec(&vols).p50, p.p50, "deterministic");
    }

    #[test]
    fn a_default_one_year_goal_is_capital_preservation() {
        let r = report_sip(&SipRequest::new(1_000_000.0, 12, Region::India));
        assert_eq!(r.regime.kind, RegimeKind::Short);
        assert_eq!(r.plan.annual_return_pct, 6.75);
        assert_eq!(r.tax.as_ref().unwrap().kind, TaxKind::Debt);
        assert!(r.mismatch.is_none());
        assert_eq!(r.plan.monthly_schedule.len(), 12);
        assert_eq!(r.sensitivity.len(), 3);
        assert!(r.sensitivity[0].monthly_sip > r.sensitivity[2].monthly_sip);
        assert_eq!(r.tax_compare.len(), 2);
        // Forcing 12% on a year warns with the downside next to the safe SIP.
        let mut req = SipRequest::new(1_000_000.0, 12, Region::India);
        req.return_pct = Some(12.0);
        let r = report_sip(&req);
        let m = r.mismatch.unwrap();
        assert!(m.p10 < 1_000_000.0 && m.shortfall_prob_pct > 20.0, "{m:?}");
        assert_eq!(m.safe_return_pct, 6.75);
        assert!(m.safe_monthly_sip > r.plan.monthly_sip);
        // Long goals default to equity with the 1% load.
        let l = report_sip(&SipRequest::new(1e7, 180, Region::India));
        assert_eq!(l.tax.unwrap().kind, TaxKind::Equity);
        assert!(
            report_sip(&SipRequest::new(1e7, 180, Region::Other))
                .tax
                .is_none()
        );
    }

    #[test]
    fn inflation_raises_the_target_and_shows_purchasing_power() {
        let mut req = SipRequest::new(1e7, 120, Region::India);
        req.inflation_pct = Some(6.0);
        let r = report_sip(&req);
        assert!(near(r.plan.target, 1e7 * 1.06f64.powi(10), 1e-3));
        assert_eq!(r.target_today, Some(1e7));
        assert!(near(r.real_value_today.unwrap(), 1e7, 1e-3));
        let plain = report_sip(&SipRequest::new(1e7, 120, Region::India));
        assert!(near(
            plain.real_value_today.unwrap(),
            1e7 / 1.06f64.powi(10),
            1e-3
        ));
        assert!(
            report_sip(&SipRequest::new(1e6, 12, Region::India))
                .real_value_today
                .is_none()
        );
    }

    #[test]
    fn retirement_reports_withdrawal_rate_and_a_crash() {
        let plan = plan_retirement(80_000.0, 20, 30, 6.0, 12.0, 7.0, 0.0, 0.0);
        let rate = plan.monthly_expense_at_retirement * 12.0 / plan.corpus * 100.0;
        assert!(near(plan.initial_withdrawal_rate_pct, rate, 1e-9));
        assert!(
            near(plan.survival_years, 30.0, 1e-9),
            "{}",
            plan.survival_years
        );
        assert!(
            plan.crash_survival_years < 25.0,
            "{}",
            plan.crash_survival_years
        );
        assert!(plan.crash_survival_years > 10.0);
        assert!(plan.swp_drag.is_none());
        let opts = RetireOpts {
            monthly_income: 20_000.0,
            buffer_years: 5,
            legacy: 1_000_000.0,
            region: Region::India,
            slab_pct: 30.0,
        };
        let e = plan_retirement_with(80_000.0, 20, 30, 6.0, 12.0, 7.0, 0.0, 0.0, &opts);
        assert!(near(
            e.monthly_need_at_retirement,
            plan.monthly_expense_at_retirement * 0.75,
            1e-6
        ));
        assert!(e.survival_years >= 35.0);
        assert!(e.swp_drag.as_ref().unwrap().fd_tax > e.swp_drag.as_ref().unwrap().swp_tax);
    }

    #[test]
    fn swp_runway_matches_the_closed_form_without_tax() {
        // ₹1 Cr at 0% return paying ₹50,000 a month lasts exactly 200 months.
        let p = plan_swp(1e7, None, 50_000.0, 0.0, 0.0, 30, 0.0, 0.0, Region::Other);
        assert_eq!(p.swp.runway_months, 200);
        assert!(!p.swp.lasts);
        assert!(near(p.initial_withdrawal_rate_pct, 6.0, 1e-9));
        // A return that outruns the withdrawals lasts, and gain-only tax beats FD slab tax.
        let p = plan_swp(1e7, None, 40_000.0, 0.0, 10.0, 10, 7.0, 30.0, Region::India);
        assert!(p.swp.lasts && p.fd.lasts);
        assert!(
            p.swp.total_tax < p.fd.total_tax,
            "{} {}",
            p.swp.total_tax,
            p.fd.total_tax
        );
    }

    #[test]
    fn emergency_fund_builds_the_gap() {
        let e = plan_emergency(50_000.0, 6, 100_000.0, 12, 6.75);
        assert_eq!(e.target, 300_000.0);
        assert!(near(e.funded_pct, 33.333, 1e-2));
        assert!(near(e.build.periods.last().unwrap().value, 300_000.0, 1e-6));
        let done = plan_emergency(50_000.0, 6, 400_000.0, 12, 6.75);
        assert_eq!(done.build.monthly_sip, 0.0);
        assert_eq!(done.funded_pct, 100.0);
    }

    #[test]
    fn prepay_saves_the_exact_interest_and_finds_a_break_even() {
        // ₹50 L, 9% a year, 240 months: EMI = ₹44,986.
        assert!(
            near(emi(5e6, 9.0, 240), 44_986.0, 1.0),
            "{}",
            emi(5e6, 9.0, 240)
        );
        assert!(near(emi(1.2e6, 0.0, 12), 100_000.0, 1e-9));
        let rules = TaxRules::new(Region::India, TaxKind::Equity, 30.0, 0.0);
        let p = plan_prepay(5e6, 9.0, 240, 10_000.0, 0.0, 12.0, 15.0, &rules);
        assert!(p.new_tenure_months < 240 && p.tenure_saved_months > 0);
        assert_eq!(p.new_tenure_months + p.tenure_saved_months, 240);
        assert!(p.interest_saved > 0.0 && p.interest_saved < p.interest_without_prepay);
        // Paying EMI + extra clears the loan: replay it month by month.
        let (mut bal, mut n) = (5e6, 0);
        while bal > 1e-6 {
            bal = bal * 1.0075 - (p.emi + 10_000.0);
            n += 1;
        }
        assert_eq!(n, p.new_tenure_months);
        // To beat a 9% guaranteed saving after tax, a fund needs more than 9%.
        let be = p.breakeven_return_pct.unwrap();
        assert!(be > 9.0 && be < 15.0, "{be}");
        assert!(p.sip_percentiles.p10 < p.sip_percentiles.p90);
        // A lump sum shortens the tenure too.
        let l = plan_prepay(5e6, 9.0, 240, 0.0, 500_000.0, 12.0, 15.0, &rules);
        assert!(l.tenure_saved_months > 0);
    }

    #[test]
    fn saved_goals_are_found_by_name() {
        let store = GoalStore {
            goals: vec![SavedGoal {
                name: "House".into(),
                target: 5e6,
                by: "2031-06".into(),
                inflation_pct: Some(6.0),
                holdings: vec!["NIFTYBEES".into()],
                return_pct: None,
                created: "2026-11-08".into(),
            }],
        };
        assert!(store.find("house").is_some());
        assert!(store.find("car").is_none());
        let json = serde_json::to_string(&store).unwrap();
        let back: GoalStore = serde_json::from_str(&json).unwrap();
        assert_eq!(back.goals, store.goals);
    }
}
