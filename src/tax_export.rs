//! Capital-gains schedules built from realized sales, laid out like the
//! forms they feed: India's ITR Schedule CG / Schedule 112A (sections 111A
//! and 112A, listed equity with STT) and the US Form 8949 with its Schedule D
//! totals. Pure: `stockwise tax export` looks up ISINs and writes the file.
//!
//! Every schedule is an estimate. It knows only the sales StockWise recorded
//! (`portfolio sell`, tradebook and CAS imports), at the recorded prices,
//! without brokerage, STT or other transfer costs.

use crate::output::{csv_line, round};
use crate::portfolio::{Holding, Sale, symbol_currency};
use crate::tax::{self, Country};
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

/// Shown on every export, in the file and on screen.
pub const DISCLAIMER: &str = "estimate — verify with your broker's tax P&L";

/// India's grandfathering date: for shares bought on or before it, the cost
/// of acquisition may be raised to the fair market value on this day.
const GRANDFATHER_DATE: (i32, u32, u32) = (2018, 1, 31);

/// Rates changed on this day (STCG 15% → 20%, LTCG 10% → 12.5%).
const INDIA_RATE_CHANGE: (i32, u32, u32) = (2024, 7, 23);

/// The first year with the ₹1.25 lakh LTCG exemption (for the whole year).
const FY_2024_25: (i32, u32, u32) = (2024, 4, 1);

fn ymd((y, m, d): (i32, u32, u32)) -> NaiveDate {
    NaiveDate::from_ymd_opt(y, m, d).expect("valid date")
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The tax year named by `label`, as the date it starts on. India and the
/// UK take `2025-26` (also `2025-2026`, `FY2025-26`, or `AY2026-27` for the
/// assessment year); the US takes `2025` (also `TY2025`).
pub fn parse_tax_year(country: Country, label: &str) -> Option<NaiveDate> {
    let s: String = label
        .trim()
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    let (assessment, rest) = match s.strip_prefix("ay") {
        Some(r) => (true, r),
        None => (
            false,
            s.strip_prefix("fy")
                .or_else(|| s.strip_prefix("ty"))
                .unwrap_or(&s),
        ),
    };
    let start: i32 = match country {
        Country::Us if !assessment => {
            if rest.len() != 4 {
                return None;
            }
            rest.parse().ok()?
        }
        Country::Us => return None,
        Country::India | Country::Uk => {
            let (a, b) = rest.split_once(['-', '/'])?;
            if a.len() != 4 {
                return None;
            }
            let a: i32 = a.parse().ok()?;
            let b: i32 = b.parse().ok()?;
            let next = match b.to_string().len() {
                _ if b == a + 1 => true,
                1 | 2 if (a + 1) % 100 == b => true,
                _ => false,
            };
            if !next {
                return None;
            }
            if assessment { a - 1 } else { a }
        }
    };
    if !(1990..=2100).contains(&start) {
        return None;
    }
    match country {
        Country::Us => NaiveDate::from_ymd_opt(start, 1, 1),
        Country::India => NaiveDate::from_ymd_opt(start, 4, 1),
        Country::Uk => NaiveDate::from_ymd_opt(start, 4, 6),
    }
}

/// "FY 2025-26 (AY 2026-27)" for India; the country's own label otherwise.
pub fn year_label(country: Country, start: NaiveDate) -> String {
    let label = country.tax_year_label(start);
    match country {
        Country::India => {
            let y = country.tax_year(start).0.year() + 1;
            format!("{} (AY {}-{:02})", label, y, (y + 1) % 100)
        }
        _ => label,
    }
}

/// Sales whose sale date falls in the tax year starting `start`, split into
/// those in the country's currency (scaled into it) and the symbols of the
/// rest, which another schedule covers.
fn sales_in_year(
    country: Country,
    sales: &[Sale],
    start: NaiveDate,
) -> (Vec<(&Sale, f64)>, Vec<String>) {
    let (from, to) = country.tax_year(start);
    let mut kept = Vec::new();
    let mut excluded: Vec<String> = Vec::new();
    for s in sales
        .iter()
        .filter(|s| date(&s.sell_date).is_some_and(|d| d >= from && d <= to))
    {
        match symbol_currency(&s.symbol).and_then(|c| country.currency_scale(c)) {
            Some(scale) => kept.push((s, scale)),
            None => excluded.push(s.symbol.clone()),
        }
    }
    kept.sort_by(|a, b| {
        (
            a.0.sell_date.as_str(),
            a.0.symbol.as_str(),
            a.0.buy_date.as_str(),
        )
            .cmp(&(
                b.0.sell_date.as_str(),
                b.0.symbol.as_str(),
                b.0.buy_date.as_str(),
            ))
    });
    excluded.sort();
    excluded.dedup();
    (kept, excluded)
}

fn holding_days(s: &Sale) -> Option<i64> {
    Some((date(&s.sell_date)? - date(&s.buy_date)?).num_days())
}

fn is_long_term(country: Country, s: &Sale) -> bool {
    match (date(&s.buy_date), date(&s.sell_date)) {
        (Some(b), Some(d)) => country.is_long_term(b, d),
        _ => false,
    }
}

/// The scrip name for the schedule: the ticker without its exchange suffix.
fn scrip(symbol: &str) -> String {
    symbol
        .strip_suffix(".NS")
        .or_else(|| symbol.strip_suffix(".BO"))
        .unwrap_or(symbol)
        .to_string()
}

// ── India: Schedule CG / Schedule 112A ──

/// One sale (one lot sold) in the Indian schedule.
#[derive(Debug, Clone, Serialize)]
pub struct IndiaRow {
    /// `111A` (short-term) or `112A` (long-term).
    pub section: &'static str,
    /// `STCG` or `LTCG`.
    pub term: &'static str,
    /// Blank when unknown.
    pub isin: String,
    pub scrip: String,
    pub symbol: String,
    pub quantity: f64,
    pub sale_date: String,
    pub sale_price: f64,
    /// Full value of consideration.
    pub sale_value: f64,
    pub purchase_date: String,
    pub purchase_price: f64,
    /// Cost of acquisition for the schedule: the actual cost, or for a
    /// grandfathered lot the higher of the actual cost and the lower of the
    /// 31 Jan 2018 fair market value and the sale value (Section 112A).
    pub cost: f64,
    /// What the shares actually cost.
    pub actual_cost: f64,
    /// Fair market value per share on 31 Jan 2018, for a grandfathered lot
    /// whose price is known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fmv_31_jan_2018: Option<f64>,
    pub holding_days: Option<i64>,
    pub gain: f64,
    /// Long-term and bought on or before 31 Jan 2018: Section 112A lets the
    /// cost rise to that day's fair market value (see `fmv_31_jan_2018`).
    pub grandfathered: bool,
    pub purchase_date_estimated: bool,
}

/// Totals for Schedule CG and the set-off, after the ₹1.25 lakh exemption.
#[derive(Debug, Clone, Default, Serialize)]
pub struct IndiaSummary {
    pub stcg_111a_sale_value: f64,
    pub stcg_111a_cost: f64,
    pub stcg_gains: f64,
    pub stcg_losses: f64,
    pub ltcg_112a_sale_value: f64,
    pub ltcg_112a_cost: f64,
    pub ltcg_gains: f64,
    pub ltcg_losses: f64,
    /// Net short-term loss set off against long-term gains.
    pub st_loss_set_off_against_ltcg: f64,
    pub taxable_stcg: f64,
    pub ltcg_after_set_off: f64,
    pub ltcg_exemption: f64,
    pub taxable_ltcg: f64,
    /// Of `taxable_stcg`, the part from sales before 23 Jul 2024 (taxed at 15%).
    pub taxable_stcg_before_23_jul_2024: f64,
    /// Of `taxable_ltcg`, the part from sales before 23 Jul 2024 (taxed at 10%).
    pub taxable_ltcg_before_23_jul_2024: f64,
    pub stcg_tax: f64,
    pub ltcg_tax: f64,
    /// 4% health and education cess on the STCG and LTCG tax.
    pub cess: f64,
    /// STCG tax, LTCG tax and cess.
    pub total_tax: f64,
    pub st_loss_carried_forward: f64,
    pub lt_loss_carried_forward: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndiaSchedule {
    pub country: &'static str,
    pub tax_year: String,
    pub period_start: String,
    pub period_end: String,
    pub currency: &'static str,
    pub disclaimer: &'static str,
    pub sales: Vec<IndiaRow>,
    pub summary: IndiaSummary,
    /// Symbols sold this year in another currency (not in 111A/112A).
    pub other_currency: Vec<String>,
    /// Crypto sold this year: taxed under section 115BBH (Schedule VDA), not here.
    pub virtual_digital_assets: Vec<String>,
    /// Same-day (intraday) sales this year: speculative business income at
    /// slab rates, left out of 111A/112A.
    pub intraday_sales: usize,
    /// Net gain or loss of those sales.
    pub intraday_gain: f64,
    /// Symbols sold this year that 111A/112A do not cover (gold, silver,
    /// liquid, debt and international ETFs), left out.
    pub other_rules: Vec<String>,
    pub notes: Vec<String>,
}

/// The fair market value of a listed share on 31 Jan 2018 from its daily
/// bars: the highest price of that trading day (Section 112A(2)(b)). Bar times
/// are read in India Standard Time. None when no bar falls on that day.
pub fn fmv_from_bars(bars: &[crate::views::Bar]) -> Option<f64> {
    let day = ymd(GRANDFATHER_DATE);
    bars.iter()
        .find(|b| {
            chrono::DateTime::from_timestamp(b.ts + 19_800, 0)
                .is_some_and(|t| t.date_naive() == day)
        })
        .and_then(|b| b.high.or(b.close))
        .filter(|p| p.is_finite() && *p > 0.0)
}

/// The Unix-second window around 31 Jan 2018 to ask for daily bars.
pub fn fmv_window() -> (i64, i64) {
    let day = ymd(GRANDFATHER_DATE);
    let at = |d: NaiveDate| {
        d.and_hms_opt(0, 0, 0)
            .expect("midnight")
            .and_utc()
            .timestamp()
    };
    (
        at(day - chrono::Duration::days(2)),
        at(day + chrono::Duration::days(3)),
    )
}

/// Section 112A cost of acquisition per share for a lot bought on or before
/// 31 Jan 2018: the higher of the actual cost and the lower of the fair market
/// value on that day and the sale price. A loss is never created or enlarged
/// by the FMV, because the FMV is capped at the sale price.
pub fn grandfathered_unit_cost(buy_price: f64, fmv: f64, sell_price: f64) -> f64 {
    buy_price.max(fmv.min(sell_price))
}

/// Cost per share of a lot for an Indian gain: the Section 112A grandfathered
/// cost when the lot is long-term, was bought on or before 31 Jan 2018 and its
/// 31 Jan 2018 price (`fmv`) is known; the actual cost otherwise. `ref_price`
/// is the sale price, or today's price for an open lot.
pub fn lot_unit_cost(
    buy_date: &str,
    long: bool,
    buy_price: f64,
    ref_price: f64,
    fmv: Option<f64>,
) -> f64 {
    match fmv {
        Some(f) if long && is_grandfathered_date(buy_date) => {
            grandfathered_unit_cost(buy_price, f, ref_price)
        }
        _ => buy_price,
    }
}

/// Bought on or before 31 Jan 2018.
pub fn is_grandfathered_date(buy_date: &str) -> bool {
    date(buy_date).is_some_and(|d| d <= ymd(GRANDFATHER_DATE))
}

/// The Indian schedule for the tax year starting `start`, with every cost at
/// its actual value. `isin` looks up a symbol's ISIN (None leaves it blank).
pub fn india_schedule(
    sales: &[Sale],
    start: NaiveDate,
    isin: impl Fn(&str) -> Option<String>,
    marks: &crate::offmarket::OffMarket,
) -> IndiaSchedule {
    india_schedule_with_fmv(sales, start, isin, marks, |_| None)
}

/// Symbols with a long-term sale in the tax year starting `start` of a lot
/// bought on or before 31 Jan 2018: the ones whose 31 Jan 2018 price is
/// needed for [`india_schedule_with_fmv`].
pub fn grandfathered_symbols(sales: &[Sale], start: NaiveDate) -> Vec<String> {
    let c = Country::India;
    let gf = ymd(GRANDFATHER_DATE);
    let mut out: Vec<String> = sales_in_year(c, sales, start)
        .0
        .into_iter()
        .filter(|(s, _)| {
            !s.is_intraday() && is_long_term(c, s) && date(&s.buy_date).is_some_and(|d| d <= gf)
        })
        .map(|(s, _)| s.symbol.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Like [`india_schedule`], but a long-term lot bought on or before 31 Jan
/// 2018 uses the Section 112A grandfathered cost. `fmv` gives a symbol's price
/// on 31 Jan 2018 (highest price for listed shares, NAV for funds); None
/// keeps the actual cost and says so in the notes.
pub fn india_schedule_with_fmv(
    sales: &[Sale],
    start: NaiveDate,
    isin: impl Fn(&str) -> Option<String>,
    marks: &crate::offmarket::OffMarket,
    fmv: impl Fn(&str) -> Option<f64>,
) -> IndiaSchedule {
    use crate::offmarket::{IndiaClass, india_class};
    let c = Country::India;
    let (from, to) = c.tax_year(start);
    let (all, other_currency) = sales_in_year(c, sales, start);
    let class = |symbol: &str| india_class(symbol, marks.mark(symbol));
    let (vda, kept): (Vec<_>, Vec<_>) = all
        .into_iter()
        .partition(|(s, _)| class(&s.symbol) == IndiaClass::Vda);
    let mut virtual_digital_assets: Vec<String> =
        vda.iter().map(|(s, _)| s.symbol.clone()).collect();
    virtual_digital_assets.sort();
    virtual_digital_assets.dedup();
    // Bought and sold on one day: speculative income (Sec. 43(5)), not 111A/112A.
    let (intraday, kept): (Vec<_>, Vec<_>) = kept.into_iter().partition(|(s, _)| s.is_intraday());
    // Debt and other funds, SGBs, and gold, silver, liquid, debt and
    // international ETFs are not listed shares or equity-oriented funds.
    let (other, kept): (Vec<_>, Vec<_>) = kept
        .into_iter()
        .partition(|(s, _)| matches!(class(&s.symbol), IndiaClass::Other(_)));
    let mut other_rules: Vec<String> = other.iter().map(|(s, _)| s.symbol.clone()).collect();
    other_rules.sort();
    other_rules.dedup();
    let intraday_sales = intraday.len();
    let intraday_gain = round(
        intraday
            .iter()
            .map(|(s, scale)| s.gain() * scale)
            .sum::<f64>(),
        2,
    );
    let mut rows = Vec::new();
    let mut sum = IndiaSummary::default();
    let mut buckets = tax::Buckets::default();
    // Sales before and from 23 Jul 2024 are taxed at different rates.
    let (mut before_change, mut after_change) = (tax::Buckets::default(), tax::Buckets::default());
    let rate_change = ymd(INDIA_RATE_CHANGE);
    let gf = ymd(GRANDFATHER_DATE);
    for (s, scale) in kept {
        let long = is_long_term(c, s);
        let sale_value = s.qty * s.sell_price * scale;
        let actual_cost = s.qty * s.buy_price * scale;
        let grandfathered = long && date(&s.buy_date).is_some_and(|d| d <= gf);
        let fmv_price = if grandfathered { fmv(&s.symbol) } else { None };
        let unit_cost = match fmv_price {
            Some(f) => grandfathered_unit_cost(s.buy_price, f, s.sell_price),
            None => s.buy_price,
        };
        let cost = s.qty * unit_cost * scale;
        let gain = sale_value - cost;
        buckets.add(gain, long);
        if date(&s.sell_date).is_some_and(|d| d < rate_change) {
            before_change.add(gain, long);
        } else {
            after_change.add(gain, long);
        }
        if long {
            sum.ltcg_112a_sale_value += sale_value;
            sum.ltcg_112a_cost += cost;
        } else {
            sum.stcg_111a_sale_value += sale_value;
            sum.stcg_111a_cost += cost;
        }
        rows.push(IndiaRow {
            section: if long { "112A" } else { "111A" },
            term: if long { "LTCG" } else { "STCG" },
            isin: isin(&s.symbol).unwrap_or_default(),
            scrip: scrip(&s.symbol),
            symbol: s.symbol.clone(),
            quantity: s.qty,
            sale_date: s.sell_date.clone(),
            sale_price: round(s.sell_price * scale, 4),
            sale_value: round(sale_value, 2),
            purchase_date: s.buy_date.clone(),
            purchase_price: round(s.buy_price * scale, 4),
            cost: round(cost, 2),
            actual_cost: round(actual_cost, 2),
            fmv_31_jan_2018: fmv_price.map(|f| round(f * scale, 4)),
            holding_days: holding_days(s),
            gain: round(gain, 2),
            grandfathered,
            purchase_date_estimated: s.buy_date_estimated,
        });
    }
    // Rates by sale date, and the exemption of the year (₹1 lakh before
    // FY 2024-25, ₹1.25 lakh from it).
    let e = tax::estimate_india_by_period(
        before_change,
        after_change,
        tax::india_exemption_for_year(from.year()),
    );
    let mixed_rates = !before_change.is_empty() && !after_change.is_empty();
    sum.taxable_stcg_before_23_jul_2024 = e.taxable_before.0;
    sum.taxable_ltcg_before_23_jul_2024 = e.taxable_before.1;
    sum.stcg_gains = buckets.st_gain;
    sum.stcg_losses = buckets.st_loss;
    sum.ltcg_gains = buckets.lt_gain;
    sum.ltcg_losses = buckets.lt_loss;
    sum.st_loss_set_off_against_ltcg = e.st_loss_against_lt;
    sum.taxable_stcg = e.st_taxable;
    sum.ltcg_after_set_off = e.lt_net;
    sum.ltcg_exemption = e.exemption_used;
    sum.taxable_ltcg = e.lt_taxable;
    sum.stcg_tax = e.st_tax;
    sum.ltcg_tax = e.lt_tax;
    sum.cess = e.cess;
    sum.total_tax = e.total();
    sum.st_loss_carried_forward = e.st_carry;
    sum.lt_loss_carried_forward = e.lt_carry;
    let sum = round_india(sum);

    let mut notes = vec![if !before_change.is_empty() && after_change.is_empty() {
        let exemption = if from < ymd(FY_2024_25) {
            "₹1 lakh"
        } else {
            "₹1.25 lakh"
        };
        format!(
            "STCG under section 111A at 15%, LTCG under section 112A at 10% above the {exemption} \
             exemption (the rates for sales before 23 Jul 2024); 4% cess included, but no surcharge, \
             transfer costs or brought-forward losses"
        )
    } else if mixed_rates {
        "Sales before 23 Jul 2024: STCG under section 111A at 15%, LTCG under section 112A at 10%; \
         from that day 20% and 12.5%. The ₹1.25 lakh exemption covers the whole year, and losses and \
         the exemption are used against the higher-rate gains first. 4% cess included, but no \
         surcharge, transfer costs or brought-forward losses"
            .to_string()
    } else {
        format!(
            "STCG under section 111A at {}%, LTCG under section 112A at {}% above the ₹1.25 lakh exemption; \
             4% cess included, but no surcharge, transfer costs or brought-forward losses",
            c.st_rate(),
            c.lt_rate()
        )
    }];
    if rows.iter().any(|r| r.fmv_31_jan_2018.is_some()) {
        notes.push(
            "Shares bought on or before 31 Jan 2018 use the grandfathered cost (Section 112A): the \
             higher of the actual cost and the lower of the 31 Jan 2018 fair market value and the \
             sale value"
                .into(),
        );
    }
    if rows
        .iter()
        .any(|r| r.grandfathered && r.fmv_31_jan_2018.is_none())
    {
        notes.push(
            "Some shares bought on or before 31 Jan 2018 have no 31 Jan 2018 price here, so they use \
             their actual cost; Schedule 112A lets you use the fair market value instead, which may \
             lower the gain"
                .into(),
        );
    }
    if rows.iter().any(|r| r.purchase_date_estimated) {
        notes.push(
            "Some purchase dates are estimated — fix them with `stockwise portfolio lots SYMBOL \
             --lot N --date YYYY-MM-DD` before filing"
                .into(),
        );
    }
    if rows.iter().any(|r| r.symbol.starts_with("MF:")) {
        notes.push(
            "Mutual fund rows are equity-oriented funds (by AMFI category), taxed like listed shares"
                .into(),
        );
    }
    if intraday_sales > 0 {
        notes.push(format!(
            "Not included: {} same-day (intraday) sale(s) with a net {} of ₹{:.2} are speculative \
             business income under section 43(5), taxed at your slab rate and reported in Schedule \
             BP; speculative losses can only be set off against speculative gains",
            intraday_sales,
            if intraday_gain < 0.0 { "loss" } else { "gain" },
            intraday_gain.abs()
        ));
    }
    if !other_rules.is_empty() {
        notes.push(format!(
            "Not included (not listed shares or equity-oriented funds, so not 111A/112A — gold, silver \
             and international ETF units: 12.5% after 12 months with no ₹1.25 lakh exemption, shorter \
             holdings at your slab rate; liquid and debt ETFs, debt and other funds without an AMFI \
             category, and Sovereign Gold Bonds: other rules, see `stockwise tax`): {}",
            other_rules.join(", ")
        ));
    }
    if !virtual_digital_assets.is_empty() {
        notes.push(format!(
            "Not included (crypto is a virtual digital asset — 30% under section 115BBH, reported \
             in Schedule VDA): {}",
            virtual_digital_assets.join(", ")
        ));
    }
    if !other_currency.is_empty() {
        notes.push(format!(
            "Not included (not INR — foreign shares go in Schedule CG with a 24-month holding \
             period): {}",
            other_currency.join(", ")
        ));
    }
    IndiaSchedule {
        country: c.name(),
        tax_year: year_label(c, start),
        period_start: from.to_string(),
        period_end: to.to_string(),
        currency: c.tax_currency(),
        disclaimer: DISCLAIMER,
        sales: rows,
        summary: sum,
        other_currency,
        virtual_digital_assets,
        intraday_sales,
        intraday_gain,
        other_rules,
        notes,
    }
}

fn round_india(s: IndiaSummary) -> IndiaSummary {
    let r = |x: f64| round(x, 2);
    IndiaSummary {
        stcg_111a_sale_value: r(s.stcg_111a_sale_value),
        stcg_111a_cost: r(s.stcg_111a_cost),
        stcg_gains: r(s.stcg_gains),
        stcg_losses: r(s.stcg_losses),
        ltcg_112a_sale_value: r(s.ltcg_112a_sale_value),
        ltcg_112a_cost: r(s.ltcg_112a_cost),
        ltcg_gains: r(s.ltcg_gains),
        ltcg_losses: r(s.ltcg_losses),
        st_loss_set_off_against_ltcg: r(s.st_loss_set_off_against_ltcg),
        taxable_stcg: r(s.taxable_stcg),
        ltcg_after_set_off: r(s.ltcg_after_set_off),
        ltcg_exemption: r(s.ltcg_exemption),
        taxable_ltcg: r(s.taxable_ltcg),
        taxable_stcg_before_23_jul_2024: r(s.taxable_stcg_before_23_jul_2024),
        taxable_ltcg_before_23_jul_2024: r(s.taxable_ltcg_before_23_jul_2024),
        stcg_tax: r(s.stcg_tax),
        ltcg_tax: r(s.ltcg_tax),
        cess: r(s.cess),
        total_tax: r(s.total_tax),
        st_loss_carried_forward: r(s.st_loss_carried_forward),
        lt_loss_carried_forward: r(s.lt_loss_carried_forward),
    }
}

/// A quantity without trailing zeros: 10, 2.5, 0.125.
fn qty(q: f64) -> String {
    let s = format!("{:.6}", q);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

fn money(x: f64) -> String {
    // `+ 0.0` turns a negative zero into 0.00 instead of -0.00.
    format!("{:.2}", x + 0.0)
}

fn yes_no(b: bool) -> &'static str {
    if b { "Yes" } else { "No" }
}

/// The schedule as a spreadsheet: a title block, one row per sale (Schedule
/// 112A column order), the Schedule CG summary and the notes, separated by
/// blank lines. Every line is RFC 4180 CSV.
pub fn india_csv(s: &IndiaSchedule) -> String {
    let mut out = String::new();
    let mut line = |fields: &[&str]| out.push_str(&csv_line(fields));
    line(&[
        "StockWise capital-gains schedule (ITR Schedule CG / 112A)",
        DISCLAIMER,
    ]);
    line(&["Tax year", &s.tax_year]);
    line(&["Sales from", &s.period_start, "to", &s.period_end]);
    line(&["Currency", s.currency]);
    line(&[]);
    line(&[
        "Section",
        "Type",
        "ISIN",
        "Scrip",
        "Quantity",
        "Sale date",
        "Sale price per share",
        "Full value of consideration",
        "Purchase date",
        "Purchase price per share",
        "Fair market value per share on 31-Jan-2018",
        "Cost of acquisition",
        "Actual cost of acquisition",
        "Holding period (days)",
        "Gain / loss",
        "Acquired on or before 31-Jan-2018",
        "Purchase date estimated",
    ]);
    for r in &s.sales {
        line(&[
            r.section,
            r.term,
            &r.isin,
            &r.scrip,
            &qty(r.quantity),
            &r.sale_date,
            &format!("{:.4}", r.sale_price),
            &money(r.sale_value),
            &r.purchase_date,
            &format!("{:.4}", r.purchase_price),
            &r.fmv_31_jan_2018
                .map(|f| format!("{:.4}", f))
                .unwrap_or_default(),
            &money(r.cost),
            &money(r.actual_cost),
            &r.holding_days.map(|d| d.to_string()).unwrap_or_default(),
            &money(r.gain),
            yes_no(r.grandfathered),
            yes_no(r.purchase_date_estimated),
        ]);
    }
    line(&[]);
    let m = &s.summary;
    line(&["Summary", "Amount (INR)"]);
    for (label, value) in [
        (
            "STCG 111A — full value of consideration",
            m.stcg_111a_sale_value,
        ),
        ("STCG 111A — cost of acquisition", m.stcg_111a_cost),
        ("Short-term gains", m.stcg_gains),
        ("Short-term losses", m.stcg_losses),
        (
            "LTCG 112A — full value of consideration",
            m.ltcg_112a_sale_value,
        ),
        ("LTCG 112A — cost of acquisition", m.ltcg_112a_cost),
        ("Long-term gains", m.ltcg_gains),
        ("Long-term losses", m.ltcg_losses),
        (
            "Short-term loss set off against LTCG",
            m.st_loss_set_off_against_ltcg,
        ),
        ("Taxable STCG (111A)", m.taxable_stcg),
        ("LTCG after set-off", m.ltcg_after_set_off),
        ("LTCG exemption (up to ₹1.25 lakh)", m.ltcg_exemption),
        ("Taxable LTCG (112A)", m.taxable_ltcg),
    ] {
        line(&[label, &money(value)]);
    }
    if m.taxable_stcg_before_23_jul_2024 > 0.0 || m.taxable_ltcg_before_23_jul_2024 > 0.0 {
        line(&[
            "  of which sales before 23 Jul 2024 — STCG (15%)",
            &money(m.taxable_stcg_before_23_jul_2024),
        ]);
        line(&[
            "  of which sales before 23 Jul 2024 — LTCG (10%)",
            &money(m.taxable_ltcg_before_23_jul_2024),
        ]);
    }
    for (label, value) in [
        ("Estimated STCG tax", m.stcg_tax),
        ("Estimated LTCG tax", m.ltcg_tax),
        ("Health and education cess (4%)", m.cess),
        ("Estimated total tax (with cess)", m.total_tax),
        (
            "Short-term loss to carry forward",
            m.st_loss_carried_forward,
        ),
        ("Long-term loss to carry forward", m.lt_loss_carried_forward),
    ] {
        line(&[label, &money(value)]);
    }
    if !s.notes.is_empty() {
        line(&[]);
        line(&["Notes"]);
        for n in &s.notes {
            line(&[n]);
        }
    }
    out
}

// ── US: Form 8949 / Schedule D ──

/// One sale (one lot sold) on Form 8949.
#[derive(Debug, Clone, Serialize)]
pub struct UsRow {
    /// `I` (short-term, held one year or less) or `II` (long-term).
    pub part: &'static str,
    /// (a) Description of property, e.g. `10 sh AAPL`.
    pub description: String,
    pub symbol: String,
    pub quantity: f64,
    /// (b) Date acquired, MM/DD/YYYY.
    pub date_acquired: String,
    /// (c) Date sold, MM/DD/YYYY.
    pub date_sold: String,
    /// (d) Proceeds.
    pub proceeds: f64,
    /// (e) Cost or other basis, including a loss disallowed by an earlier
    /// wash sale of other shares that these shares replaced.
    pub cost_basis: f64,
    /// (f) Code: `W` for a wash sale, empty otherwise.
    pub code: &'static str,
    /// (g) Amount of adjustment: the disallowed wash-sale loss, positive.
    pub adjustment: f64,
    /// (h) Gain or (loss): (d) − (e) + (g).
    pub gain_or_loss: f64,
    pub date_acquired_estimated: bool,
    /// Days added to the holding period because these shares replaced shares
    /// sold in a wash sale; they decide Part I or II with (b) and (c).
    pub holding_days_added: i64,
}

/// Schedule D totals and the netting between short- and long-term results.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsSummary {
    pub short_term_proceeds: f64,
    pub short_term_cost: f64,
    /// Column (g) total for Part I: disallowed wash-sale losses.
    pub short_term_adjustment: f64,
    pub short_term_gain: f64,
    pub long_term_proceeds: f64,
    pub long_term_cost: f64,
    /// Column (g) total for Part II: disallowed wash-sale losses.
    pub long_term_adjustment: f64,
    pub long_term_gain: f64,
    pub net_gain: f64,
    pub taxable_short_term: f64,
    pub taxable_long_term: f64,
    /// A net loss is deductible against other income up to $3,000 a year.
    pub loss_deductible_this_year: f64,
    pub loss_carryover: f64,
    pub short_term_tax: f64,
    pub long_term_tax: f64,
    pub total_tax: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct UsSchedule {
    pub country: &'static str,
    pub tax_year: String,
    pub period_start: String,
    pub period_end: String,
    pub currency: &'static str,
    pub disclaimer: &'static str,
    pub sales: Vec<UsRow>,
    pub summary: UsSummary,
    /// Wash sales in this tax year: losses disallowed because the same
    /// shares were bought within 30 days before or after the sale.
    pub wash_sales: Vec<tax::Wash>,
    pub other_currency: Vec<String>,
    pub notes: Vec<String>,
}

/// Yearly cap on a net capital loss deducted against other income.
const US_LOSS_DEDUCTION_CAP: f64 = 3_000.0;

fn us_date(s: &str) -> String {
    date(s).map_or_else(|| s.to_string(), |d| d.format("%m/%d/%Y").to_string())
}

/// Form 8949 rows for the tax year starting `start`. `holdings` are the open
/// lots, which can be replacement shares in a wash sale.
pub fn us_schedule(sales: &[Sale], holdings: &[Holding], start: NaiveDate) -> UsSchedule {
    let c = Country::Us;
    let (from, to) = c.tax_year(start);
    let (kept, other_currency) = sales_in_year(c, sales, start);
    let wash = tax::wash_sales(sales, holdings);
    let index_of = |s: &Sale| sales.iter().position(|x| std::ptr::eq(x, s));
    let mut rows = Vec::new();
    let mut sum = UsSummary::default();
    let mut buckets = tax::Buckets::default();
    for (s, scale) in kept {
        // A sale whose shares partly replaced shares sold in a wash sale is
        // split into parts: each has its own basis and holding period.
        let whole = [tax::SalePart {
            qty: s.qty,
            basis_added: 0.0,
            days_added: 0,
            disallowed: 0.0,
        }];
        let parts = match index_of(s).map(|i| wash.parts[i].as_slice()) {
            Some(parts) if !parts.is_empty() => parts,
            _ => &whole,
        };
        // Split the sale's proceeds and cost between the parts in whole
        // cents by running share count, so the rows add up to the sale.
        let to_cents = |x: f64| (x * 100.0).round() as i64;
        let proceeds_total = to_cents(s.qty * s.sell_price * scale);
        let cost_total = to_cents(s.qty * s.buy_price * scale);
        let (mut done_qty, mut done_proceeds, mut done_cost) = (0.0, 0, 0);
        for (n, p) in parts.iter().enumerate() {
            let long = match (date(&s.buy_date), date(&s.sell_date)) {
                (Some(b), Some(d)) => c.is_long_term(b - chrono::Duration::days(p.days_added), d),
                _ => false,
            };
            done_qty += p.qty;
            let share = |total: i64| {
                if n + 1 == parts.len() || s.qty <= 0.0 {
                    total
                } else {
                    (total as f64 * done_qty / s.qty).round() as i64
                }
            };
            let (upto_proceeds, upto_cost) = (share(proceeds_total), share(cost_total));
            let proceeds_cents = upto_proceeds - done_proceeds;
            let cost_cents = upto_cost - done_cost + to_cents(p.basis_added * scale);
            (done_proceeds, done_cost) = (upto_proceeds, upto_cost);
            let adjustment_cents = to_cents(p.disallowed * scale);
            let gain_cents = proceeds_cents - cost_cents + adjustment_cents;
            let dollars = |x: i64| x as f64 / 100.0;
            let (proceeds, cost, adjustment, gain) = (
                dollars(proceeds_cents),
                dollars(cost_cents),
                dollars(adjustment_cents),
                dollars(gain_cents),
            );
            buckets.add(gain, long);
            if long {
                sum.long_term_proceeds += proceeds;
                sum.long_term_cost += cost;
                sum.long_term_adjustment += adjustment;
            } else {
                sum.short_term_proceeds += proceeds;
                sum.short_term_cost += cost;
                sum.short_term_adjustment += adjustment;
            }
            rows.push(UsRow {
                part: if long { "II" } else { "I" },
                description: format!("{} sh {}", qty(p.qty), s.symbol),
                symbol: s.symbol.clone(),
                quantity: p.qty,
                date_acquired: us_date(&s.buy_date),
                date_sold: us_date(&s.sell_date),
                proceeds: round(proceeds, 2),
                cost_basis: round(cost, 2),
                code: if adjustment > 0.0 { "W" } else { "" },
                adjustment: round(adjustment, 2),
                gain_or_loss: round(gain, 2),
                date_acquired_estimated: s.buy_date_estimated,
                holding_days_added: p.days_added,
            });
        }
    }
    // Part I first, then Part II, as on the form.
    rows.sort_by_key(|r| r.part == "II");
    let e = tax::estimate(c, buckets);
    sum.short_term_gain = sum.short_term_proceeds - sum.short_term_cost + sum.short_term_adjustment;
    sum.long_term_gain = sum.long_term_proceeds - sum.long_term_cost + sum.long_term_adjustment;
    sum.net_gain = sum.short_term_gain + sum.long_term_gain;
    sum.taxable_short_term = e.st_taxable;
    sum.taxable_long_term = e.lt_taxable;
    let loss = e.st_carry + e.lt_carry;
    sum.loss_deductible_this_year = loss.min(US_LOSS_DEDUCTION_CAP);
    sum.loss_carryover = loss - sum.loss_deductible_this_year;
    sum.short_term_tax = e.st_tax;
    sum.long_term_tax = e.lt_tax;
    sum.total_tax = e.total();
    let r = |x: f64| round(x, 2);
    let sum = UsSummary {
        short_term_proceeds: r(sum.short_term_proceeds),
        short_term_cost: r(sum.short_term_cost),
        short_term_adjustment: r(sum.short_term_adjustment),
        short_term_gain: r(sum.short_term_gain),
        long_term_proceeds: r(sum.long_term_proceeds),
        long_term_cost: r(sum.long_term_cost),
        long_term_adjustment: r(sum.long_term_adjustment),
        long_term_gain: r(sum.long_term_gain),
        net_gain: r(sum.net_gain),
        taxable_short_term: r(sum.taxable_short_term),
        taxable_long_term: r(sum.taxable_long_term),
        loss_deductible_this_year: r(sum.loss_deductible_this_year),
        loss_carryover: r(sum.loss_carryover),
        short_term_tax: r(sum.short_term_tax),
        long_term_tax: r(sum.long_term_tax),
        total_tax: r(sum.total_tax),
    };

    let mut notes = vec![
        format!(
            "Tax at the top rates ({}% short-term, {}% long-term), before state tax and NIIT",
            c.st_rate(),
            c.lt_rate()
        ),
        "Check box A/D (basis reported to the IRS), B/E or C/F on Form 8949 against your \
         1099-B; column (g) holds only wash-sale adjustments (code W) found in your \
         StockWise records"
            .into(),
    ];
    let in_year = |d: &str| date(d).is_some_and(|d| d >= from && d <= to);
    let washes: Vec<tax::Wash> = wash
        .washes
        .into_iter()
        .filter(|w| in_year(&w.sell_date))
        .collect();
    for w in &washes {
        for r in w.replacements.iter().filter(|r| r.sale.is_none()) {
            notes.push(format!(
                "Wash sale: ${} of the loss on {} sold {} is added to the basis of {} shares \
                 bought {} and still held, and {} days to their holding period",
                money(r.basis_added),
                w.symbol,
                us_date(&w.sell_date),
                qty(r.qty),
                us_date(&r.date),
                r.days_added
            ));
        }
    }
    if rows.iter().any(|r| r.holding_days_added > 0) {
        notes.push(
            "Replacement shares sold this year carry the basis and holding period of the \
             shares sold in the wash sale (Part and column e include them); \
             `holding_days_added` in the JSON gives the days added"
                .into(),
        );
    }
    let estimated: Vec<String> = washes
        .iter()
        .filter(|w| w.replacements.iter().any(|r| r.date_estimated))
        .map(|w| format!("{} sold {}", w.symbol, us_date(&w.sell_date)))
        .collect();
    if !estimated.is_empty() {
        notes.push(format!(
            "Wash sales found through a replacement with an estimated purchase date may not \
             be wash sales; fix the date with `stockwise portfolio lots SYMBOL --lot N --date \
             YYYY-MM-DD`: {}",
            estimated.join(", ")
        ));
    }
    if rows.iter().any(|r| r.date_acquired_estimated) {
        notes.push(
            "Some acquisition dates are estimated — fix them with `stockwise portfolio lots \
             SYMBOL --lot N --date YYYY-MM-DD`"
                .into(),
        );
    }
    if !other_currency.is_empty() {
        notes.push(format!(
            "Not included (not USD): {}",
            other_currency.join(", ")
        ));
    }
    UsSchedule {
        country: c.name(),
        tax_year: year_label(c, start),
        period_start: from.to_string(),
        period_end: to.to_string(),
        currency: c.tax_currency(),
        disclaimer: DISCLAIMER,
        sales: rows,
        summary: sum,
        wash_sales: washes,
        other_currency,
        notes,
    }
}

/// Form 8949 as a spreadsheet: title block, Part I and Part II rows in the
/// form's column order, Schedule D totals, notes.
pub fn us_csv(s: &UsSchedule) -> String {
    let mut out = String::new();
    let mut line = |fields: &[&str]| out.push_str(&csv_line(fields));
    line(&["StockWise Form 8949 worksheet", DISCLAIMER]);
    line(&["Tax year", &s.tax_year]);
    line(&["Currency", s.currency]);
    line(&[]);
    line(&[
        "Part",
        "(a) Description of property",
        "(b) Date acquired",
        "(c) Date sold or disposed of",
        "(d) Proceeds",
        "(e) Cost or other basis",
        "(f) Code(s)",
        "(g) Amount of adjustment",
        "(h) Gain or (loss)",
        "Date acquired estimated",
    ]);
    for r in &s.sales {
        line(&[
            r.part,
            &r.description,
            &r.date_acquired,
            &r.date_sold,
            &money(r.proceeds),
            &money(r.cost_basis),
            r.code,
            &if r.adjustment > 0.0 {
                money(r.adjustment)
            } else {
                String::new()
            },
            &money(r.gain_or_loss),
            yes_no(r.date_acquired_estimated),
        ]);
    }
    line(&[]);
    let m = &s.summary;
    line(&["Schedule D", "Amount (USD)"]);
    for (label, value) in [
        ("Short-term proceeds", m.short_term_proceeds),
        ("Short-term cost basis", m.short_term_cost),
        ("Short-term adjustments (g)", m.short_term_adjustment),
        ("Short-term gain or (loss)", m.short_term_gain),
        ("Long-term proceeds", m.long_term_proceeds),
        ("Long-term cost basis", m.long_term_cost),
        ("Long-term adjustments (g)", m.long_term_adjustment),
        ("Long-term gain or (loss)", m.long_term_gain),
        ("Net gain or (loss)", m.net_gain),
        ("Taxable short-term gain", m.taxable_short_term),
        ("Taxable long-term gain", m.taxable_long_term),
        (
            "Loss deductible this year (max $3,000)",
            m.loss_deductible_this_year,
        ),
        ("Loss carryover", m.loss_carryover),
        ("Estimated short-term tax", m.short_term_tax),
        ("Estimated long-term tax", m.long_term_tax),
        ("Estimated total tax", m.total_tax),
    ] {
        line(&[label, &money(value)]);
    }
    if !s.notes.is_empty() {
        line(&[]);
        line(&["Notes"]);
        for n in &s.notes {
            line(&[n]);
        }
    }
    out
}

// ── India: audit pack (one workpaper per sale) ──

/// One sale's workpaper: how its gain was worked out and which rule taxes
/// it. Amounts are in paise, so the workpaper's equation balances to the
/// paisa (they are shown and serialised as rupees); per-unit prices keep
/// four decimals.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AuditRow {
    /// 1-based, in sale-date order; the workpaper file is numbered by it.
    pub n: usize,
    pub symbol: String,
    pub isin: String,
    pub account: String,
    /// Where the lot came from (`manual`, `import:kite`, `bonus`, ...).
    pub lot_source: String,
    pub quantity: f64,
    pub purchase_date: String,
    pub purchase_date_estimated: bool,
    pub sale_date: String,
    pub holding_days: Option<i64>,
    /// `short`, `long` or `intraday`.
    pub term: &'static str,
    pub purchase_price: f64,
    #[serde(serialize_with = "tax::paise::serialize")]
    pub actual_cost: i64,
    /// Fair market value per unit on 31 Jan 2018, where grandfathering applies.
    pub fmv_31_jan_2018: Option<f64>,
    #[serde(serialize_with = "opt_paise")]
    pub fmv_value: Option<i64>,
    /// The cost the gain is worked out with.
    #[serde(serialize_with = "tax::paise::serialize")]
    pub cost_used: i64,
    /// How `cost_used` was chosen, with the figures.
    pub cost_rule: String,
    pub sale_price: f64,
    #[serde(serialize_with = "tax::paise::serialize")]
    pub sale_value: i64,
    /// Charges taken off the gain: the recorded charges of an intraday sale
    /// (business income, as in the Schedule CG export's intraday total);
    /// zero for capital gains, which are before charges.
    #[serde(serialize_with = "tax::paise::serialize")]
    pub charges_deducted: i64,
    #[serde(serialize_with = "tax::paise::serialize")]
    pub gain: i64,
    /// The section and rate that tax the gain.
    pub rule: String,
    pub file: String,
}

fn opt_paise<S: serde::Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(p) => s.serialize_f64(tax::paise::to_rupees(*p)),
        None => s.serialize_none(),
    }
}

fn paise_str(p: i64) -> String {
    format!("{:.2}", tax::paise::to_rupees(p) + 0.0)
}

/// The columns of `summary.csv`, also the order of a workpaper's fields.
pub const AUDIT_COLUMNS: &[&str] = &[
    "n",
    "file",
    "symbol",
    "isin",
    "account",
    "lot_source",
    "quantity",
    "purchase_date",
    "purchase_date_estimated",
    "sale_date",
    "holding_days",
    "term",
    "purchase_price",
    "actual_cost",
    "fmv_31_jan_2018_per_unit",
    "fmv_31_jan_2018_value",
    "cost_used",
    "cost_rule",
    "sale_price",
    "sale_value",
    "charges_deducted",
    "gain",
    "rule_applied",
];

impl AuditRow {
    fn fields(&self) -> Vec<String> {
        let price = |x: f64| format!("{x:.4}");
        vec![
            self.n.to_string(),
            self.file.clone(),
            self.symbol.clone(),
            self.isin.clone(),
            self.account.clone(),
            self.lot_source.clone(),
            qty(self.quantity),
            self.purchase_date.clone(),
            yes_no(self.purchase_date_estimated).to_string(),
            self.sale_date.clone(),
            self.holding_days.map(|d| d.to_string()).unwrap_or_default(),
            self.term.to_string(),
            price(self.purchase_price),
            paise_str(self.actual_cost),
            self.fmv_31_jan_2018.map(price).unwrap_or_default(),
            self.fmv_value.map(paise_str).unwrap_or_default(),
            paise_str(self.cost_used),
            self.cost_rule.clone(),
            price(self.sale_price),
            paise_str(self.sale_value),
            paise_str(self.charges_deducted),
            paise_str(self.gain),
            self.rule.clone(),
        ]
    }

    /// The workpaper: one `field,value` line per column, then the
    /// computation in words and the disclaimer.
    pub fn workpaper_csv(&self, tax_year: &str) -> String {
        let mut out = csv_line(&["field", "value"]);
        out.push_str(&csv_line(&["tax_year", tax_year]));
        for (k, v) in AUDIT_COLUMNS.iter().zip(self.fields()) {
            out.push_str(&csv_line(&[k.to_string(), v]));
        }
        let computation = if self.charges_deducted != 0 {
            format!(
                "gain = sale_value {} - cost_used {} - charges {} = {}",
                paise_str(self.sale_value),
                paise_str(self.cost_used),
                paise_str(self.charges_deducted),
                paise_str(self.gain)
            )
        } else {
            format!(
                "gain = sale_value {} - cost_used {} = {}",
                paise_str(self.sale_value),
                paise_str(self.cost_used),
                paise_str(self.gain)
            )
        };
        out.push_str(&csv_line(&["computation".to_string(), computation]));
        out.push_str(&csv_line(&["disclaimer", DISCLAIMER]));
        out
    }
}

/// How an Indian sale is taxed, in words, and its term.
fn india_rule_of(
    s: &Sale,
    long: bool,
    grandfathered: bool,
    marks: &crate::offmarket::OffMarket,
) -> (String, &'static str) {
    use crate::offmarket::{IndiaClass, india_asset, india_class};
    let before = date(&s.sell_date).is_some_and(|d| d < ymd(INDIA_RATE_CHANGE));
    let class = india_class(&s.symbol, marks.mark(&s.symbol));
    // Crypto is Sec. 115BBH even when bought and sold the same day.
    if class == IndiaClass::Vda {
        let term = if s.is_intraday() {
            "intraday"
        } else if long {
            "long"
        } else {
            "short"
        };
        return (tax::IndiaRule::Vda115BBH.label().to_string(), term);
    }
    if s.is_intraday() {
        return (
            "Sec. 43(5) speculative business income — slab rate".into(),
            "intraday",
        );
    }
    let term = if long { "long" } else { "short" };
    match class {
        IndiaClass::Listed | IndiaClass::EquityFund if long => {
            let mut r = format!(
                "Sec. 112A — LTCG {} above the yearly exemption",
                if before { "10%" } else { "12.5%" }
            );
            if grandfathered {
                r.push_str("; cost grandfathered to 31 Jan 2018 (Sec. 55(2)(ac))");
            }
            (r, term)
        }
        IndiaClass::Listed | IndiaClass::EquityFund => (
            format!("Sec. 111A — STCG {}", if before { "15%" } else { "20%" }),
            term,
        ),
        IndiaClass::Vda => (tax::IndiaRule::Vda115BBH.label().to_string(), term),
        IndiaClass::Other(reason) => {
            let asset = india_asset(&s.symbol, marks.mark(&s.symbol));
            match (asset, date(&s.buy_date), date(&s.sell_date)) {
                (Some(a), Some(b), Some(d)) => {
                    let rule = tax::india_rule(a, b, d);
                    let term = match rule {
                        tax::IndiaRule::LongTerm112 => "long",
                        tax::IndiaRule::Vda115BBH => term,
                        _ => "short",
                    };
                    (rule.label_on(d).to_string(), term)
                }
                _ => (reason, term),
            }
        }
    }
}

/// One workpaper per rupee sale of the financial year starting `start`
/// (every rule: Sec. 111A/112A, 50AA, 112, 115BBH and intraday), in sale
/// order. Sales in another currency are not included. `isin` and `fmv`
/// look up a symbol's ISIN and 31 Jan 2018 price.
pub fn india_audit_rows(
    sales: &[Sale],
    start: NaiveDate,
    isin: impl Fn(&str) -> Option<String>,
    marks: &crate::offmarket::OffMarket,
    fmv: impl Fn(&str) -> Option<f64>,
) -> Vec<AuditRow> {
    use tax::paise;
    let c = Country::India;
    let gf = ymd(GRANDFATHER_DATE);
    let (kept, _) = sales_in_year(c, sales, start);
    let mut rows = Vec::new();
    for (i, (s, scale)) in kept.into_iter().enumerate() {
        let class = crate::offmarket::india_class(&s.symbol, marks.mark(&s.symbol));
        let vda = class == crate::offmarket::IndiaClass::Vda;
        let intraday = s.is_intraday() && !vda;
        let long = !s.is_intraday() && is_long_term(c, s);
        let equity = matches!(
            class,
            crate::offmarket::IndiaClass::Listed | crate::offmarket::IndiaClass::EquityFund
        );
        let old = long && equity && date(&s.buy_date).is_some_and(|d| d <= gf);
        let fmv_unit = if old { fmv(&s.symbol) } else { None };
        let actual_cost = paise::from_rupees(s.qty * s.buy_price * scale);
        let sale_value = paise::from_rupees(s.qty * s.sell_price * scale);
        let fmv_value = fmv_unit.map(|f| paise::from_rupees(s.qty * f * scale));
        // Sec. 55(2)(ac) on the totals: max(cost, min(FMV, sale value)).
        let cost_used = match fmv_value {
            Some(f) => actual_cost.max(f.min(sale_value)),
            None => actual_cost,
        };
        let raised = cost_used != actual_cost;
        let cost_rule = match fmv_value {
            Some(f) => format!(
                "max(actual cost {}, min(31 Jan 2018 FMV {}, sale value {})) = {} — Sec. 55(2)(ac)",
                paise_str(actual_cost),
                paise_str(f),
                paise_str(sale_value),
                paise_str(cost_used)
            ),
            None if old => "actual cost — the 31 Jan 2018 price could not be found".into(),
            None => "actual cost".to_string(),
        };
        let charges_deducted = if intraday {
            paise::from_rupees(s.charges() * scale)
        } else {
            0
        };
        let (rule, term) = india_rule_of(s, long, raised, marks);
        let n = i + 1;
        let sym = scrip(&s.symbol);
        let file_sym: String = sym
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        rows.push(AuditRow {
            n,
            file: format!("sale-{n:03}-{file_sym}-{}.csv", s.sell_date),
            symbol: s.symbol.clone(),
            isin: isin(&s.symbol).unwrap_or_default(),
            account: s.account_key().unwrap_or("").to_string(),
            lot_source: s
                .origin
                .as_ref()
                .map(|o| o.source.clone())
                .unwrap_or_else(|| s.source.clone()),
            quantity: s.qty,
            purchase_date: s.buy_date.clone(),
            purchase_date_estimated: s.buy_date_estimated,
            sale_date: s.sell_date.clone(),
            holding_days: holding_days(s),
            term,
            purchase_price: round(s.buy_price * scale, 4),
            actual_cost,
            fmv_31_jan_2018: fmv_unit.map(|f| round(f * scale, 4)),
            fmv_value,
            cost_used,
            cost_rule,
            sale_price: round(s.sell_price * scale, 4),
            sale_value,
            charges_deducted,
            gain: sale_value - cost_used - charges_deducted,
            rule,
        });
    }
    rows
}

/// `summary.csv` of an audit pack: one line per workpaper, then the totals
/// of each rule.
pub fn audit_summary_csv(rows: &[AuditRow], tax_year: &str) -> String {
    let mut out = csv_line(&["StockWise audit pack", tax_year]);
    out.push_str(&csv_line(&["Note", DISCLAIMER]));
    out.push_str(&csv_line(&[
        "Scope",
        "rupee sales only: sales in another currency (US shares) are not in this pack",
    ]));
    out.push('\n');
    out.push_str(&csv_line(AUDIT_COLUMNS));
    for r in rows {
        out.push_str(&csv_line(&r.fields()));
    }
    out.push('\n');
    out.push_str(&csv_line(&[
        "rule_applied",
        "sales",
        "sale_value",
        "cost_used",
        "charges_deducted",
        "gain",
    ]));
    let mut rules: Vec<&str> = rows.iter().map(|r| r.rule.as_str()).collect();
    rules.sort();
    rules.dedup();
    for rule in rules {
        let these: Vec<&AuditRow> = rows.iter().filter(|r| r.rule == rule).collect();
        let total = |f: fn(&AuditRow) -> i64| paise_str(these.iter().map(|r| f(r)).sum::<i64>());
        out.push_str(&csv_line(&[
            rule.to_string(),
            these.len().to_string(),
            total(|r| r.sale_value),
            total(|r| r.cost_used),
            total(|r| r.charges_deducted),
            total(|r| r.gain),
        ]));
    }
    out
}

/// Parse the NSE equity list (`EQUITY_L.csv`: SYMBOL, NAME OF COMPANY,
/// SERIES, ..., ISIN NUMBER, ...) into (symbol, ISIN) pairs.
pub fn parse_nse_equity_list(text: &str) -> Vec<(String, String)> {
    let mut lines = text.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let cols: Vec<String> = header
        .trim_start_matches('\u{feff}')
        .split(',')
        .map(|c| c.trim().to_uppercase())
        .collect();
    let (Some(sym), Some(isin)) = (
        cols.iter().position(|c| c == "SYMBOL"),
        cols.iter().position(|c| c.starts_with("ISIN")),
    ) else {
        return Vec::new();
    };
    lines
        .filter_map(|l| {
            let f: Vec<&str> = l.split(',').map(str::trim).collect();
            let s = f.get(sym)?;
            let i = f.get(isin)?;
            (!s.is_empty() && i.len() == 12).then(|| (s.to_uppercase(), i.to_uppercase()))
        })
        .collect()
}

#[cfg(test)]
// Amounts in paise are written rupees_paise: 1_200_00 is ₹1,200.00.
#[allow(clippy::inconsistent_digit_grouping)]
mod tests {
    use super::*;

    fn sale(symbol: &str, qty: f64, buy: (&str, f64), sell: (&str, f64)) -> Sale {
        Sale {
            symbol: symbol.into(),
            qty,
            buy_date: buy.0.into(),
            buy_price: buy.1,
            sell_date: sell.0.into(),
            sell_price: sell.1,
            buy_date_estimated: false,
            source: "manual".into(),
            order_id: None,
            intraday: false,
            account: None,
            origin: None,
            bonuses: Vec::new(),
            bonus_shares: false,
            charges: None,
            fx_currency: None,
            buy_fx_rate: None,
        }
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn audit_workpapers_balance_and_name_the_bound_and_the_vda_rule() {
        let mut intraday = sale(
            "SBIN.NS",
            100.0,
            ("2026-07-10", 800.0),
            ("2026-07-10", 805.0),
        );
        intraday.charges = Some(42.5);
        let sales = vec![
            // Same-day crypto: still Sec. 115BBH, not Sec. 43(5).
            sale(
                "BTC-INR",
                0.01,
                ("2026-06-01", 5_000_000.0),
                ("2026-06-01", 5_100_000.0),
            ),
            intraday,
            // FMV ₹1,000 above cost ₹800 but the sale (₹700) caps it: cost stays.
            sale("ITC.NS", 1.0, ("2017-01-02", 800.0), ("2026-08-01", 700.0)),
            // Sub-paisa prices: the equation still balances to the paisa.
            sale(
                "YESBANK.NS",
                1.0,
                ("2026-01-02", 100.004),
                ("2026-08-02", 100.006),
            ),
        ];
        let rows = india_audit_rows(
            &sales,
            d("2026-04-01"),
            |_| None,
            &Default::default(),
            |s| (s == "ITC.NS").then_some(1_000.0),
        );
        let by = |s: &str| rows.iter().find(|r| r.symbol == s).unwrap();
        assert!(by("BTC-INR").rule.contains("115BBH"));
        assert_eq!(by("BTC-INR").term, "intraday");
        let sbin = by("SBIN.NS");
        assert_eq!(sbin.charges_deducted, 42_50);
        assert_eq!(sbin.gain, 500_00 - 42_50);
        assert!(
            sbin.workpaper_csv("FY")
                .contains("- charges 42.50 = 457.50")
        );
        let itc = by("ITC.NS");
        assert_eq!(itc.cost_used, 800_00);
        assert!(
            itc.cost_rule.contains(
                "max(actual cost 800.00, min(31 Jan 2018 FMV 1000.00, sale value 700.00)) = 800.00"
            ),
            "{}",
            itc.cost_rule
        );
        assert!(!itc.rule.contains("grandfathered"));
        for r in &rows {
            assert_eq!(r.gain, r.sale_value - r.cost_used - r.charges_deducted);
        }
    }

    #[test]
    fn audit_pack_has_one_workpaper_per_sale_with_the_rule_applied() {
        let sales = vec![
            // Grandfathered: bought 2017, sold 2026 (FMV ₹948 on 31 Jan 2018).
            sale(
                "HDFCBANK.NS",
                10.0,
                ("2017-03-01", 700.0),
                ("2026-07-20", 1_800.0),
            ),
            sale(
                "INFY.NS",
                5.0,
                ("2026-01-10", 1_500.0),
                ("2026-05-12", 1_650.0),
            ),
            sale(
                "SBIN.NS",
                100.0,
                ("2026-07-10", 800.0),
                ("2026-07-10", 805.0),
            ),
            sale(
                "BTC-INR",
                0.01,
                ("2025-01-01", 5_000_000.0),
                ("2026-06-01", 6_000_000.0),
            ),
            sale(
                "LIQUIDBEES.NS",
                10.0,
                ("2023-01-02", 1_000.0),
                ("2026-06-01", 1_000.5),
            ),
            sale(
                "LIQUIDCASE.NS",
                10.0,
                ("2024-01-02", 1_000.0),
                ("2026-06-01", 1_000.5),
            ),
            // Another year: no workpaper.
            sale(
                "TCS.NS",
                1.0,
                ("2024-01-10", 3_000.0),
                ("2025-05-12", 3_500.0),
            ),
        ];
        let rows = india_audit_rows(
            &sales,
            d("2026-04-01"),
            |s| (s == "HDFCBANK.NS").then(|| "INE040A01034".into()),
            &Default::default(),
            |s| (s == "HDFCBANK.NS").then_some(948.0),
        );
        assert_eq!(rows.len(), 6);
        let by = |s: &str| rows.iter().find(|r| r.symbol == s).unwrap();
        let hdfc = by("HDFCBANK.NS");
        assert_eq!(hdfc.isin, "INE040A01034");
        assert_eq!(hdfc.fmv_31_jan_2018, Some(948.0));
        assert_eq!(hdfc.fmv_value, Some(9_480_00));
        assert_eq!(hdfc.actual_cost, 7_000_00);
        assert_eq!(hdfc.cost_used, 9_480_00);
        assert_eq!(hdfc.gain, 8_520_00);
        assert!(hdfc.rule.contains("112A") && hdfc.rule.contains("grandfathered"));
        assert!(by("INFY.NS").rule.contains("111A"));
        assert_eq!(by("INFY.NS").fmv_31_jan_2018, None);
        assert_eq!(by("SBIN.NS").term, "intraday");
        assert!(by("BTC-INR").rule.contains("115BBH"));
        // A liquid ETF bought before 1 Apr 2023, held over 12 months: Sec. 112.
        assert!(
            by("LIQUIDBEES.NS").rule.contains("Sec. 112 "),
            "{}",
            by("LIQUIDBEES.NS").rule
        );
        assert_eq!(by("LIQUIDBEES.NS").term, "long");
        // Bought from 1 Apr 2023: Sec. 50AA, short-term however long it is held.
        assert!(by("LIQUIDCASE.NS").rule.contains("50AA"));
        assert_eq!(by("LIQUIDCASE.NS").term, "short");
        assert!(rows.windows(2).all(|w| w[0].sale_date <= w[1].sale_date));
        assert_eq!(rows[0].n, 1);
        assert!(rows[0].file.starts_with("sale-001-"));

        let wp = hdfc.workpaper_csv("FY 2026-27");
        let fields: Vec<&str> = wp.lines().filter_map(|l| l.split(',').next()).collect();
        for col in AUDIT_COLUMNS {
            assert!(fields.contains(col), "workpaper lacks {col}");
        }
        assert!(wp.contains("fmv_31_jan_2018_per_unit,948.0000"));
        assert!(wp.contains("gain = sale_value 18000.00 - cost_used 9480.00 = 8520.00"));

        let summary = audit_summary_csv(&rows, "FY 2026-27");
        assert!(summary.contains(&AUDIT_COLUMNS.join(",")));
        assert!(summary.contains("rule_applied,sales,sale_value,cost_used,charges_deducted,gain"));
    }

    #[test]
    fn tax_year_labels_parse() {
        let i = Country::India;
        assert_eq!(parse_tax_year(i, "2025-26"), Some(d("2025-04-01")));
        assert_eq!(parse_tax_year(i, "FY 2025-26"), Some(d("2025-04-01")));
        assert_eq!(parse_tax_year(i, "fy2025-2026"), Some(d("2025-04-01")));
        assert_eq!(parse_tax_year(i, "AY2026-27"), Some(d("2025-04-01")));
        assert_eq!(parse_tax_year(i, "1999-00"), Some(d("1999-04-01")));
        for bad in ["2025", "2025-27", "25-26", "2025-2027", "x", ""] {
            assert_eq!(parse_tax_year(i, bad), None, "{bad:?}");
        }
        assert_eq!(parse_tax_year(Country::Us, "2025"), Some(d("2025-01-01")));
        assert_eq!(parse_tax_year(Country::Us, "TY2025"), Some(d("2025-01-01")));
        assert_eq!(parse_tax_year(Country::Us, "2025-26"), None);
        assert_eq!(year_label(i, d("2025-04-01")), "FY 2025-26 (AY 2026-27)");
        assert_eq!(year_label(Country::Us, d("2025-01-01")), "Tax year 2025");
    }

    /// FY 2025-26 worked example (amounts in ₹):
    /// - TCS: 10 bought 2024-01-10 at 3,500, sold 2025-06-02 at 4,100 →
    ///   held 509 days, LTCG 6,000 (112A).
    /// - INFY: 20 bought 2025-01-15 at 1,900, sold 2025-08-20 at 1,500 →
    ///   STCL 8,000 (111A).
    /// - HDFCBANK: 100 bought 2017-06-01 at 800, sold 2025-09-10 at 2,000 →
    ///   LTCG 1,20,000, grandfathered.
    /// - RELIANCE: 50 bought 2025-04-10 at 1,200, sold 2025-12-01 at 1,500 →
    ///   STCG 15,000.
    /// - Outside the year (sold 2025-03-31) and a US stock: left out.
    ///
    /// Set-off: STCG 15,000 − STCL 8,000 = 7,000 taxable at 20% = 1,400.
    /// LTCG 1,26,000 − 1,25,000 exempt = 1,000 at 12.5% = 125. Total 1,525.
    fn fy26_sales() -> Vec<Sale> {
        vec![
            sale(
                "TCS.NS",
                10.0,
                ("2024-01-10", 3500.0),
                ("2025-06-02", 4100.0),
            ),
            sale(
                "INFY.NS",
                20.0,
                ("2025-01-15", 1900.0),
                ("2025-08-20", 1500.0),
            ),
            sale(
                "HDFCBANK.NS",
                100.0,
                ("2017-06-01", 800.0),
                ("2025-09-10", 2000.0),
            ),
            sale(
                "RELIANCE.NS",
                50.0,
                ("2025-04-10", 1200.0),
                ("2025-12-01", 1500.0),
            ),
            sale("ITC.NS", 10.0, ("2024-01-01", 400.0), ("2025-03-31", 450.0)),
            sale("AAPL", 5.0, ("2024-01-01", 180.0), ("2025-07-01", 210.0)),
            sale("BTC-INR", 0.01, ("2025-01-01", 8e6), ("2025-07-01", 9e6)),
        ]
    }

    #[test]
    fn fmv_is_the_high_of_31_jan_2018_in_ist() {
        let bar = |date: &str, high: f64| {
            // 09:15 IST is 03:45 UTC.
            let ts = NaiveDate::parse_from_str(date, "%Y-%m-%d")
                .unwrap()
                .and_hms_opt(3, 45, 0)
                .unwrap()
                .and_utc()
                .timestamp();
            crate::views::Bar {
                time: None,
                open: None,
                high: Some(high),
                low: None,
                close: Some(high - 5.0),
                volume: None,
                ts,
            }
        };
        let bars = [
            bar("2018-01-30", 1010.0),
            bar("2018-01-31", 1030.0),
            bar("2018-02-01", 1100.0),
        ];
        assert_eq!(fmv_from_bars(&bars), Some(1030.0));
        assert_eq!(fmv_from_bars(&bars[..1]), None);
        let (from, to) = fmv_window();
        assert!(bars.iter().all(|b| b.ts >= from && b.ts < to));
    }

    fn hdfc_fmv(fmv: f64) -> impl Fn(&str) -> Option<f64> {
        move |s| (s == "HDFCBANK.NS").then_some(fmv)
    }

    fn hdfc_row(fmv: Option<f64>) -> IndiaRow {
        let s = india_schedule_with_fmv(
            &fy26_sales(),
            d("2025-04-01"),
            |_| None,
            &Default::default(),
            move |s| fmv.filter(|_| s == "HDFCBANK.NS"),
        );
        s.sales
            .into_iter()
            .find(|r| r.symbol == "HDFCBANK.NS")
            .unwrap()
    }

    #[test]
    fn grandfathered_cost_is_higher_of_cost_and_lower_of_fmv_and_sale() {
        assert_eq!(grandfathered_unit_cost(800.0, 1000.0, 2000.0), 1000.0);
        assert_eq!(grandfathered_unit_cost(800.0, 3000.0, 2000.0), 2000.0);
        assert_eq!(grandfathered_unit_cost(800.0, 500.0, 2000.0), 800.0);
        // Sold below cost: the FMV never turns the loss into a bigger one.
        assert_eq!(grandfathered_unit_cost(800.0, 1000.0, 700.0), 800.0);
    }

    #[test]
    fn a_lot_bought_in_2017_uses_the_grandfathered_cost() {
        let r = hdfc_row(Some(1000.0));
        assert!(r.grandfathered);
        assert_eq!(r.fmv_31_jan_2018, Some(1000.0));
        assert_eq!(r.actual_cost, 80_000.0);
        assert_eq!(r.cost, 100_000.0);
        assert_eq!(r.gain, 100_000.0, "200,000 sold less 100,000, not 120,000");
    }

    #[test]
    fn grandfathering_without_a_price_keeps_actual_cost_and_says_so() {
        let r = hdfc_row(None);
        assert!(r.grandfathered);
        assert_eq!(
            (r.fmv_31_jan_2018, r.cost, r.gain),
            (None, 80_000.0, 120_000.0)
        );
        let s = india_schedule(
            &fy26_sales(),
            d("2025-04-01"),
            |_| None,
            &Default::default(),
        );
        assert!(s.notes.iter().any(|n| n.contains("no 31 Jan 2018 price")));
    }

    #[test]
    fn grandfathering_lowers_the_tax_and_skips_later_lots() {
        let plain = india_schedule(
            &fy26_sales(),
            d("2025-04-01"),
            |_| None,
            &Default::default(),
        );
        let gf = india_schedule_with_fmv(
            &fy26_sales(),
            d("2025-04-01"),
            |_| None,
            &Default::default(),
            hdfc_fmv(1000.0),
        );
        assert_eq!(
            plain.summary.ltcg_112a_cost - gf.summary.ltcg_112a_cost,
            -20_000.0
        );
        assert!(gf.summary.total_tax < plain.summary.total_tax);
        let tcs = gf.sales.iter().find(|r| r.symbol == "TCS.NS").unwrap();
        assert_eq!(tcs.fmv_31_jan_2018, None, "bought in 2024");
        assert_eq!(
            grandfathered_symbols(&fy26_sales(), d("2025-04-01")),
            vec!["HDFCBANK.NS".to_string()]
        );
    }

    #[test]
    fn india_schedule_worked_example() {
        let isin = |s: &str| (s == "TCS.NS").then(|| "INE467B01029".to_string());
        let s = india_schedule(&fy26_sales(), d("2025-04-01"), isin, &Default::default());
        assert_eq!(s.tax_year, "FY 2025-26 (AY 2026-27)");
        assert_eq!(
            s.sales.len(),
            4,
            "ITC (FY 2024-25) and AAPL (USD) are left out"
        );
        assert_eq!(s.other_currency, vec!["AAPL".to_string()]);
        assert_eq!(s.virtual_digital_assets, vec!["BTC-INR".to_string()]);
        assert!(s.notes.iter().any(|n| n.contains("115BBH")));

        let tcs = &s.sales[0];
        assert_eq!((tcs.section, tcs.term), ("112A", "LTCG"));
        assert_eq!(tcs.isin, "INE467B01029");
        assert_eq!(tcs.scrip, "TCS");
        assert_eq!(tcs.sale_value, 41_000.0);
        assert_eq!(tcs.cost, 35_000.0);
        assert_eq!(tcs.holding_days, Some(509));
        assert_eq!(tcs.gain, 6_000.0);
        assert!(!tcs.grandfathered);

        let infy = &s.sales[1];
        assert_eq!((infy.section, infy.gain), ("111A", -8_000.0));
        assert!(infy.isin.is_empty());
        assert!(
            s.sales[2].grandfathered,
            "HDFCBANK was bought before Feb 2018"
        );

        let m = &s.summary;
        assert_eq!(m.stcg_gains, 15_000.0);
        assert_eq!(m.stcg_losses, 8_000.0);
        assert_eq!(m.stcg_111a_sale_value, 30_000.0 + 75_000.0);
        assert_eq!(m.ltcg_gains, 126_000.0);
        assert_eq!(m.ltcg_112a_sale_value, 41_000.0 + 200_000.0);
        assert_eq!(m.taxable_stcg, 7_000.0);
        assert_eq!(m.ltcg_exemption, 125_000.0);
        assert_eq!(m.taxable_ltcg, 1_000.0);
        assert_eq!(m.stcg_tax, 1_400.0);
        assert_eq!(m.ltcg_tax, 125.0);
        // Cess: 4% of 1,525 = 61.
        assert_eq!(m.cess, 61.0);
        assert_eq!(m.total_tax, 1_586.0);
        assert!(s.notes.iter().any(|n| n.contains("31 Jan 2018")));
        assert!(s.notes.iter().any(|n| n.contains("AAPL")));
    }

    #[test]
    fn india_schedule_keeps_intraday_out_of_111a_and_112a() {
        let mut intraday = sale(
            "TCS.NS",
            50.0,
            ("2025-06-02", 4000.0),
            ("2025-06-02", 4010.0),
        );
        intraday.intraday = true;
        let loss = sale(
            "INFY.NS",
            10.0,
            ("2025-01-15", 1900.0),
            ("2025-08-20", 1800.0),
        );
        let s = india_schedule(
            &[intraday, loss],
            d("2025-04-01"),
            |_| None,
            &Default::default(),
        );
        // Only the delivery sale is in the schedule and the summary.
        assert_eq!(s.sales.len(), 1);
        assert_eq!(s.sales[0].symbol, "INFY.NS");
        assert_eq!(
            (s.summary.stcg_gains, s.summary.stcg_losses),
            (0.0, 1_000.0)
        );
        // 50 × (4010 − 4000) = ₹500 of speculative income, reported apart.
        assert_eq!((s.intraday_sales, s.intraday_gain), (1, 500.0));
        assert!(
            s.notes
                .iter()
                .any(|n| n.contains("43(5)") && n.contains("500.00"))
        );
    }

    #[test]
    fn india_net_short_term_loss_reduces_ltcg() {
        // STCL 50,000 against LTCG 2,00,000 → 1,50,000; 1,25,000 exempt;
        // 25,000 at 12.5% = 3,125.
        let sales = vec![
            sale("A.NS", 100.0, ("2025-05-01", 1000.0), ("2025-10-01", 500.0)),
            sale(
                "B.NS",
                100.0,
                ("2023-05-01", 1000.0),
                ("2025-10-01", 3000.0),
            ),
        ];
        let s = india_schedule(&sales, d("2025-04-01"), |_| None, &Default::default());
        assert_eq!(s.summary.st_loss_set_off_against_ltcg, 50_000.0);
        assert_eq!(s.summary.ltcg_after_set_off, 150_000.0);
        assert_eq!(s.summary.ltcg_tax, 3_125.0);
        assert_eq!(s.summary.st_loss_carried_forward, 0.0);
    }

    #[test]
    fn india_csv_has_title_rows_header_and_summary() {
        let s = india_schedule(
            &fy26_sales(),
            d("2025-04-01"),
            |_| None,
            &Default::default(),
        );
        let csv = india_csv(&s);
        let lines: Vec<&str> = csv.lines().collect();
        assert!(lines[0].contains("estimate — verify with your broker's tax P&L"));
        assert_eq!(lines[1], "Tax year,FY 2025-26 (AY 2026-27)");
        assert_eq!(lines[4], "", "one blank line between blocks");
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("Section,Type,ISIN,Scrip,Quantity"))
        );
        assert!(lines.contains(&"112A,LTCG,,TCS,10,2025-06-02,4100.0000,41000.00,2024-01-10,3500.0000,,35000.00,35000.00,509,6000.00,No,No"));
        assert!(lines.contains(&"Taxable LTCG (112A),1000.00"));
        assert!(lines.contains(&"LTCG exemption (up to ₹1.25 lakh),125000.00"));
        assert!(lines.contains(&"Health and education cess (4%),61.00"));
        assert!(lines.contains(&"Estimated total tax (with cess),1586.00"));
        // Notes with commas are quoted.
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("\"STCG under section 111A"))
        );
    }

    #[test]
    fn export_uses_the_rates_in_force_on_the_sale_date() {
        // The issue's repro: STCG ₹1,00,000 sold 2024-06-10 (FY 2024-25): 15%, not 20%.
        let sales = vec![sale(
            "A.NS",
            1.0,
            ("2024-01-01", 100.0),
            ("2024-06-10", 100_100.0),
        )];
        let s = india_schedule(&sales, d("2024-04-01"), |_| None, &Default::default());
        assert_eq!(s.sales[0].term, "STCG");
        assert_eq!(s.summary.stcg_tax, 15_000.0);
        assert_eq!(s.summary.cess, 600.0);
        assert_eq!(s.summary.taxable_stcg_before_23_jul_2024, 100_000.0);
        assert!(s.notes.iter().any(|n| n.contains("23 Jul 2024")));
        // The same gain from 23 Jul 2024 is taxed at 20%.
        let sales = vec![sale(
            "A.NS",
            1.0,
            ("2024-01-01", 100.0),
            ("2024-07-23", 100_100.0),
        )];
        let s = india_schedule(&sales, d("2024-04-01"), |_| None, &Default::default());
        assert_eq!(s.summary.stcg_tax, 20_000.0);
        assert_eq!(s.summary.taxable_stcg_before_23_jul_2024, 0.0);
    }

    #[test]
    fn export_mixed_year_and_older_exemption() {
        // FY 2024-25: ₹1,00,000 STCG before 23 Jul (15,000) and after (20,000).
        let sales = vec![
            sale(
                "A.NS",
                1.0,
                ("2024-01-01", 100.0),
                ("2024-06-10", 100_100.0),
            ),
            sale(
                "B.NS",
                1.0,
                ("2024-01-01", 100.0),
                ("2024-09-10", 100_100.0),
            ),
        ];
        let s = india_schedule(&sales, d("2024-04-01"), |_| None, &Default::default());
        assert_eq!(s.summary.stcg_tax, 35_000.0);
        assert_eq!(s.summary.taxable_stcg, 200_000.0);
        assert!(
            s.notes
                .iter()
                .any(|n| n.contains("higher-rate gains first"))
        );
        let csv = india_csv(&s);
        assert!(csv.contains("of which sales before 23 Jul 2024 — STCG (15%),100000.00"));

        // FY 2023-24: ₹3,00,000 LTCG, ₹1 lakh exempt, 10% on 2,00,000 = 20,000.
        let sales = vec![sale(
            "A.NS",
            1.0,
            ("2021-01-01", 100.0),
            ("2023-05-01", 300_100.0),
        )];
        let s = india_schedule(&sales, d("2023-04-01"), |_| None, &Default::default());
        assert_eq!(s.summary.ltcg_exemption, 100_000.0);
        assert_eq!(s.summary.ltcg_tax, 20_000.0);
        assert!(s.notes.iter().any(|n| n.contains("₹1 lakh")));
        assert!(!csv.contains("FY 2023"));

        // FY 2025-26 is unchanged: 20% and 12.5% above ₹1.25 lakh.
        let sales = vec![sale(
            "A.NS",
            1.0,
            ("2025-02-01", 100.0),
            ("2025-08-01", 100_100.0),
        )];
        let s = india_schedule(&sales, d("2025-04-01"), |_| None, &Default::default());
        assert_eq!(s.summary.stcg_tax, 20_000.0);
    }

    #[test]
    fn negative_zero_prints_as_zero() {
        assert_eq!(money(-0.0), "0.00");
        assert_eq!(money(-1.5), "-1.50");
    }

    #[test]
    fn us_form_8949_worked_example() {
        // 2025: AAPL 5 sh bought 01/01/2024 at 180, sold 07/01/2025 at 210 →
        // long-term +150 (Part II). MSFT 10 sh bought 03/01/2025 at 400, sold
        // 06/02/2025 at 380 → short-term −200 (Part I). Net −50: the ST loss
        // offsets the LT gain, $50 deductible, no tax.
        let sales = vec![
            sale("AAPL", 5.0, ("2024-01-01", 180.0), ("2025-07-01", 210.0)),
            sale("MSFT", 10.0, ("2025-03-01", 400.0), ("2025-06-02", 380.0)),
            sale(
                "TCS.NS",
                1.0,
                ("2024-01-01", 3000.0),
                ("2025-07-01", 4000.0),
            ),
        ];
        let s = us_schedule(&sales, &[], d("2025-01-01"));
        assert_eq!(s.sales.len(), 2);
        let (st, lt) = (&s.sales[0], &s.sales[1]);
        assert_eq!((st.part, st.description.as_str()), ("I", "10 sh MSFT"));
        assert_eq!(st.date_acquired, "03/01/2025");
        assert_eq!(st.gain_or_loss, -200.0);
        assert_eq!((lt.part, lt.proceeds, lt.cost_basis), ("II", 1050.0, 900.0));
        let m = &s.summary;
        assert_eq!(m.net_gain, -50.0);
        assert_eq!(m.loss_deductible_this_year, 50.0);
        assert_eq!(m.loss_carryover, 0.0);
        assert_eq!(m.total_tax, 0.0);
        assert_eq!(s.other_currency, vec!["TCS.NS".to_string()]);

        let csv = us_csv(&s);
        assert!(csv.contains("I,10 sh MSFT,03/01/2025,06/02/2025,3800.00,4000.00,,,-200.00,No"));
        assert!(csv.contains("\"Loss deductible this year (max $3,000)\",50.00"));
    }

    #[test]
    fn us_loss_above_3000_carries_over() {
        let sales = vec![sale(
            "X",
            100.0,
            ("2025-01-02", 100.0),
            ("2025-02-03", 40.0),
        )];
        let m = us_schedule(&sales, &[], d("2025-01-01")).summary;
        assert_eq!(m.short_term_gain, -6_000.0);
        assert_eq!(m.loss_deductible_this_year, 3_000.0);
        assert_eq!(m.loss_carryover, 3_000.0);
    }

    #[test]
    fn us_wash_sale_is_marked_w_with_the_disallowed_loss() {
        // 10 MSFT bought 03/01/2025 at 400, sold 06/02/2025 at 380 (loss 200);
        // 10 bought back on 06/12/2025 and sold on 09/02/2025 at 390. The loss
        // is disallowed (W, +200) and added to the replacement's basis:
        // 3,900 − (3,850 + 200) = −150.
        let mut sales = vec![
            sale("MSFT", 10.0, ("2025-03-01", 400.0), ("2025-06-02", 380.0)),
            sale("MSFT", 10.0, ("2025-06-12", 385.0), ("2025-09-02", 390.0)),
        ];
        let s = us_schedule(&sales, &[], d("2025-01-01"));
        let (washed, repl) = (&s.sales[0], &s.sales[1]);
        assert_eq!((washed.code, washed.adjustment), ("W", 200.0));
        assert_eq!(washed.gain_or_loss, 0.0);
        assert_eq!((repl.code, repl.cost_basis), ("", 4050.0));
        assert_eq!(repl.gain_or_loss, -150.0);
        let m = &s.summary;
        assert_eq!(m.short_term_adjustment, 200.0);
        assert_eq!(m.short_term_cost, 8050.0);
        assert_eq!(m.short_term_gain, -150.0);
        assert_eq!(s.wash_sales.len(), 1);
        assert!(!s.notes.iter().any(|n| n.contains("not detected")));
        let csv = us_csv(&s);
        assert!(
            csv.contains("I,10 sh MSFT,03/01/2025,06/02/2025,3800.00,4000.00,W,200.00,0.00,No"),
            "{csv}"
        );
        assert!(csv.contains("Short-term adjustments (g),200.00"));

        // Half the shares bought back and still held: half the loss is
        // disallowed, and a note names the lot that takes on the basis.
        sales.pop();
        let held = Holding {
            symbol: "MSFT".into(),
            shares: 5.0,
            avg_cost: 385.0,
            added_at: String::new(),
            lots: vec![crate::portfolio::Lot::new(
                5.0,
                385.0,
                "2025-06-12",
                "manual",
            )],
        };
        let s = us_schedule(&sales, &[held], d("2025-01-01"));
        assert_eq!(
            (s.sales[0].adjustment, s.sales[0].gain_or_loss),
            (100.0, -100.0)
        );
        assert_eq!(s.summary.short_term_gain, -100.0);
        assert!(
            s.notes
                .iter()
                .any(|n| n.contains("$100.00") && n.contains("5 shares bought 06/12/2025")),
            "{:?}",
            s.notes
        );
    }

    #[test]
    fn us_replacement_shares_carry_the_holding_period() {
        // Bought 01/02/2023, sold at a loss 06/02/2025; the replacement,
        // bought 06/12/2025 and sold 09/02/2025, is held over a year once the
        // first lot's days are added, so it goes in Part II.
        let sales = vec![
            sale("MSFT", 10.0, ("2023-01-02", 400.0), ("2025-06-02", 380.0)),
            sale("MSFT", 10.0, ("2025-06-12", 385.0), ("2025-09-02", 390.0)),
        ];
        let s = us_schedule(&sales, &[], d("2025-01-01"));
        let repl = s
            .sales
            .iter()
            .find(|r| r.date_acquired == "06/12/2025")
            .unwrap();
        assert_eq!(repl.part, "II");
        assert_eq!(repl.holding_days_added, 882);
        assert_eq!((repl.cost_basis, repl.gain_or_loss), (4050.0, -150.0));
        assert_eq!(s.summary.long_term_gain, -150.0);
        assert_eq!(s.summary.long_term_adjustment, 200.0);
    }

    #[test]
    fn us_sale_partly_of_replacement_shares_is_split() {
        // 10 washed shares are replaced by 10 of a 20-share purchase; selling
        // all 20 gives two rows: 10 with the added basis, 10 without.
        let sales = vec![
            sale("X", 10.0, ("2025-01-02", 30.0), ("2025-03-01", 20.0)),
            sale("X", 20.0, ("2025-03-05", 10.0), ("2025-05-01", 12.0)),
        ];
        let s = us_schedule(&sales, &[], d("2025-01-01"));
        let later: Vec<_> = s
            .sales
            .iter()
            .filter(|r| r.date_sold == "05/01/2025")
            .collect();
        assert_eq!(later.len(), 2);
        assert_eq!((later[0].quantity, later[0].cost_basis), (10.0, 100.0));
        assert_eq!((later[1].quantity, later[1].cost_basis), (10.0, 200.0));
        assert_eq!(later[1].gain_or_loss, -80.0);
    }

    #[test]
    fn us_split_rows_add_up_to_the_summary() {
        // A $1 loss moves to half of a one-share purchase at 1.01. The two
        // rows of the later sale split its 1.01 of cost in whole cents.
        let sales = vec![
            sale("X", 0.5, ("2025-01-02", 3.0), ("2025-02-01", 1.0)),
            sale("X", 1.0, ("2025-02-05", 1.01), ("2025-05-01", 1.01)),
        ];
        let s = us_schedule(&sales, &[], d("2025-01-01"));
        let later: Vec<_> = s
            .sales
            .iter()
            .filter(|r| r.date_sold == "05/01/2025")
            .collect();
        assert_eq!(later.len(), 2);
        let cost: f64 = later.iter().map(|r| r.cost_basis).sum();
        let proceeds: f64 = later.iter().map(|r| r.proceeds).sum();
        assert_eq!(round(cost, 2), 2.01);
        assert_eq!(round(proceeds, 2), 1.01);
        let all_cost: f64 = s.sales.iter().map(|r| r.cost_basis).sum();
        let all_gain: f64 = s.sales.iter().map(|r| r.gain_or_loss).sum();
        assert_eq!(round(all_cost, 2), s.summary.short_term_cost);
        assert_eq!(round(all_gain, 2), s.summary.short_term_gain);
    }

    #[test]
    fn nse_equity_list_parses() {
        let text = "SYMBOL,NAME OF COMPANY, SERIES, DATE OF LISTING, PAID UP VALUE, MARKET LOT, ISIN NUMBER, FACE VALUE\n\
                    TCS,Tata Consultancy Services Limited,EQ,25-AUG-2004,1,1,INE467B01029,1\n\
                    BAD,No ISIN,EQ,01-JAN-2000,1,1,,1\n";
        assert_eq!(
            parse_nse_equity_list(text),
            vec![("TCS".to_string(), "INE467B01029".to_string())]
        );
        let bom = format!("\u{feff}{text}");
        assert_eq!(parse_nse_equity_list(&bom).len(), 1);
        assert!(parse_nse_equity_list("").is_empty());
        assert!(parse_nse_equity_list("A,B\n1,2").is_empty());
    }
}
