//! Capital-gains schedules built from realized sales, laid out like the
//! forms they feed: India's ITR Schedule CG / Schedule 112A (sections 111A
//! and 112A, listed equity with STT) and the US Form 8949 with its Schedule D
//! totals. Pure: `stockwise tax export` looks up ISINs and writes the file.
//!
//! Every schedule is an estimate. It knows only the sales StockWise recorded
//! (`portfolio sell`, tradebook and CAS imports), at the recorded prices,
//! without brokerage, STT or other transfer costs.

use crate::output::{csv_line, round};
use crate::portfolio::{Sale, symbol_currency};
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
    /// Cost of acquisition (actual — no grandfathering applied).
    pub cost: f64,
    pub holding_days: Option<i64>,
    pub gain: f64,
    /// Bought on or before 31 Jan 2018: the cost may be raised to that day's
    /// fair market value, which would lower the gain.
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
    pub stcg_tax: f64,
    pub ltcg_tax: f64,
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

/// The Indian schedule for the tax year starting `start`. `isin` looks up a
/// symbol's ISIN (None leaves it blank).
pub fn india_schedule(
    sales: &[Sale],
    start: NaiveDate,
    isin: impl Fn(&str) -> Option<String>,
) -> IndiaSchedule {
    let c = Country::India;
    let (from, to) = c.tax_year(start);
    let (all, other_currency) = sales_in_year(c, sales, start);
    let (vda, kept): (Vec<_>, Vec<_>) = all.into_iter().partition(|(s, _)| {
        crate::market::AssetClass::of(&s.symbol) == crate::market::AssetClass::Crypto
    });
    let mut virtual_digital_assets: Vec<String> =
        vda.iter().map(|(s, _)| s.symbol.clone()).collect();
    virtual_digital_assets.sort();
    virtual_digital_assets.dedup();
    // Bought and sold on one day: speculative income (Sec. 43(5)), not 111A/112A.
    let (intraday, kept): (Vec<_>, Vec<_>) = kept.into_iter().partition(|(s, _)| s.is_intraday());
    // Gold, silver, liquid, debt and international ETFs are not listed shares.
    let (other, kept): (Vec<_>, Vec<_>) = kept
        .into_iter()
        .partition(|(s, _)| crate::offmarket::non_equity_etf(&s.symbol).is_some());
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
    let gf = ymd(GRANDFATHER_DATE);
    for (s, scale) in kept {
        let long = is_long_term(c, s);
        let sale_value = s.qty * s.sell_price * scale;
        let cost = s.qty * s.buy_price * scale;
        let gain = sale_value - cost;
        buckets.add(gain, long);
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
            holding_days: holding_days(s),
            gain: round(gain, 2),
            grandfathered: long && date(&s.buy_date).is_some_and(|d| d <= gf),
            purchase_date_estimated: s.buy_date_estimated,
        });
    }
    let e = tax::estimate(c, buckets);
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
    sum.total_tax = e.total();
    sum.st_loss_carried_forward = e.st_carry;
    sum.lt_loss_carried_forward = e.lt_carry;
    let sum = round_india(sum);

    let mut notes = vec![format!(
        "STCG under section 111A at {}%, LTCG under section 112A at {}% above the ₹1.25 lakh exemption; \
         no surcharge, cess, transfer costs or brought-forward losses",
        c.st_rate(),
        c.lt_rate()
    )];
    if rows.iter().any(|r| r.grandfathered) {
        notes.push(
            "Shares bought on or before 31 Jan 2018 use their actual cost; Schedule 112A lets you use \
             the 31 Jan 2018 fair market value instead, which may lower the gain"
                .into(),
        );
    }
    if from < ymd(FY_2024_25) {
        notes.push(
            "Before FY 2024-25 the LTCG exemption was ₹1 lakh and the rates 15% (STCG) and 10% \
             (LTCG); this estimate uses the current ones"
                .into(),
        );
    } else if rows
        .iter()
        .any(|r| date(&r.sale_date).is_some_and(|d| d < ymd(INDIA_RATE_CHANGE)))
    {
        notes.push(
            "Sales before 23 Jul 2024 were taxed at 15% (STCG) and 10% (LTCG); this estimate uses \
             the current rates for every sale"
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
            "Mutual fund rows belong in 111A/112A only for equity-oriented funds; debt fund units \
             bought after 31 Mar 2023 are taxed at your slab rate"
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
             holdings at your slab rate; liquid and debt ETFs: slab rate): {}",
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
        stcg_tax: r(s.stcg_tax),
        ltcg_tax: r(s.ltcg_tax),
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
        "Cost of acquisition",
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
            &money(r.cost),
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
        ("Estimated STCG tax", m.stcg_tax),
        ("Estimated LTCG tax", m.ltcg_tax),
        ("Estimated total tax", m.total_tax),
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
    /// (e) Cost or other basis.
    pub cost_basis: f64,
    /// (h) Gain or (loss).
    pub gain_or_loss: f64,
    pub date_acquired_estimated: bool,
}

/// Schedule D totals and the netting between short- and long-term results.
#[derive(Debug, Clone, Default, Serialize)]
pub struct UsSummary {
    pub short_term_proceeds: f64,
    pub short_term_cost: f64,
    pub short_term_gain: f64,
    pub long_term_proceeds: f64,
    pub long_term_cost: f64,
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
    pub other_currency: Vec<String>,
    pub notes: Vec<String>,
}

/// Yearly cap on a net capital loss deducted against other income.
const US_LOSS_DEDUCTION_CAP: f64 = 3_000.0;

fn us_date(s: &str) -> String {
    date(s).map_or_else(|| s.to_string(), |d| d.format("%m/%d/%Y").to_string())
}

/// Form 8949 rows for the tax year starting `start`.
pub fn us_schedule(sales: &[Sale], start: NaiveDate) -> UsSchedule {
    let c = Country::Us;
    let (from, to) = c.tax_year(start);
    let (kept, other_currency) = sales_in_year(c, sales, start);
    let mut rows = Vec::new();
    let mut sum = UsSummary::default();
    let mut buckets = tax::Buckets::default();
    // Part I first, then Part II, as on the form.
    let mut kept = kept;
    kept.sort_by_key(|(s, _)| is_long_term(c, s));
    for (s, scale) in kept {
        let long = is_long_term(c, s);
        let proceeds = s.qty * s.sell_price * scale;
        let cost = s.qty * s.buy_price * scale;
        let gain = proceeds - cost;
        buckets.add(gain, long);
        if long {
            sum.long_term_proceeds += proceeds;
            sum.long_term_cost += cost;
        } else {
            sum.short_term_proceeds += proceeds;
            sum.short_term_cost += cost;
        }
        rows.push(UsRow {
            part: if long { "II" } else { "I" },
            description: format!("{} sh {}", qty(s.qty), s.symbol),
            symbol: s.symbol.clone(),
            quantity: s.qty,
            date_acquired: us_date(&s.buy_date),
            date_sold: us_date(&s.sell_date),
            proceeds: round(proceeds, 2),
            cost_basis: round(cost, 2),
            gain_or_loss: round(gain, 2),
            date_acquired_estimated: s.buy_date_estimated,
        });
    }
    let e = tax::estimate(c, buckets);
    sum.short_term_gain = sum.short_term_proceeds - sum.short_term_cost;
    sum.long_term_gain = sum.long_term_proceeds - sum.long_term_cost;
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
        short_term_gain: r(sum.short_term_gain),
        long_term_proceeds: r(sum.long_term_proceeds),
        long_term_cost: r(sum.long_term_cost),
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
         1099-B; wash sales and adjustments (columns f and g) are not detected"
            .into(),
    ];
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
            "",
            "",
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
        ("Short-term gain or (loss)", m.short_term_gain),
        ("Long-term proceeds", m.long_term_proceeds),
        ("Long-term cost basis", m.long_term_cost),
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
        }
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
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
    fn india_schedule_worked_example() {
        let isin = |s: &str| (s == "TCS.NS").then(|| "INE467B01029".to_string());
        let s = india_schedule(&fy26_sales(), d("2025-04-01"), isin);
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
        assert_eq!(m.total_tax, 1_525.0);
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
        let s = india_schedule(&[intraday, loss], d("2025-04-01"), |_| None);
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
        let s = india_schedule(&sales, d("2025-04-01"), |_| None);
        assert_eq!(s.summary.st_loss_set_off_against_ltcg, 50_000.0);
        assert_eq!(s.summary.ltcg_after_set_off, 150_000.0);
        assert_eq!(s.summary.ltcg_tax, 3_125.0);
        assert_eq!(s.summary.st_loss_carried_forward, 0.0);
    }

    #[test]
    fn india_csv_has_title_rows_header_and_summary() {
        let s = india_schedule(&fy26_sales(), d("2025-04-01"), |_| None);
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
        assert!(lines.contains(&"112A,LTCG,,TCS,10,2025-06-02,4100.0000,41000.00,2024-01-10,3500.0000,35000.00,509,6000.00,No,No"));
        assert!(lines.contains(&"Taxable LTCG (112A),1000.00"));
        assert!(lines.contains(&"LTCG exemption (up to ₹1.25 lakh),125000.00"));
        assert!(lines.contains(&"Estimated total tax,1525.00"));
        // Notes with commas are quoted.
        assert!(
            lines
                .iter()
                .any(|l| l.starts_with("\"STCG under section 111A"))
        );
    }

    #[test]
    fn rate_change_note_for_fy_2024_25() {
        let sales = vec![sale(
            "A.NS",
            1.0,
            ("2024-01-01", 100.0),
            ("2024-05-01", 120.0),
        )];
        let s = india_schedule(&sales, d("2024-04-01"), |_| None);
        assert!(s.notes.iter().any(|n| n.contains("23 Jul 2024")));
        assert_eq!(s.sales[0].term, "STCG");
        // Earlier years had a ₹1 lakh exemption.
        let sales = vec![sale(
            "A.NS",
            1.0,
            ("2022-01-01", 100.0),
            ("2023-05-01", 120.0),
        )];
        let s = india_schedule(&sales, d("2023-04-01"), |_| None);
        assert!(s.notes.iter().any(|n| n.contains("₹1 lakh")));
        assert!(!s.notes.iter().any(|n| n.contains("23 Jul 2024")));
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
        let s = us_schedule(&sales, d("2025-01-01"));
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
        let m = us_schedule(&sales, d("2025-01-01")).summary;
        assert_eq!(m.short_term_gain, -6_000.0);
        assert_eq!(m.loss_deductible_this_year, 3_000.0);
        assert_eq!(m.loss_carryover, 3_000.0);
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
