//! Foreign assets and foreign tax credit for an Indian resident's ITR.
//!
//! - **Schedule FA, Table A3** (foreign equity held at any time in the
//!   calendar year — January to December, not the financial year): per lot,
//!   the date acquired, the initial value, the peak value during the year,
//!   the closing value on 31 December, dividends credited and sale proceeds,
//!   each converted to rupees at the rate on the day it refers to.
//! - **Form 67** (foreign tax credit, Rule 128): each foreign dividend with
//!   the tax withheld abroad, converted at the rate on the last day of the
//!   month before the one in which it was withheld, and the credit allowed —
//!   the lower of that tax and the Indian tax on the same income.
//!
//! The rules ask for the SBI telegraphic-transfer buying rate. StockWise
//! uses the ECB reference rate for the day (Frankfurter), which is close to
//! it but not the same: an estimate to verify with your CA.

use crate::portfolio::Portfolio;
use chrono::{Datelike, NaiveDate};
use serde::Serialize;

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// Whether `symbol` is a foreign asset for Schedule FA: a share or fund
/// quoted in a currency other than the rupee (crypto is reported in
/// Schedule VDA, not here).
pub fn is_foreign(symbol: &str) -> bool {
    crate::market::AssetClass::of(symbol) != crate::market::AssetClass::Crypto
        && crate::portfolio::symbol_currency(symbol).is_some_and(|c| c != "INR")
}

/// The country a foreign listing is in, for the schedule.
pub fn country_of(symbol: &str) -> String {
    match crate::market::exchange_of(symbol) {
        Some(x) => x.info().country.to_string(),
        None => "United States".to_string(),
    }
}

/// One acquisition of a foreign holding: what was bought that day, and
/// what became of it.
#[derive(Debug, Clone, PartialEq)]
struct Piece {
    acquired: NaiveDate,
    qty: f64,
    /// `None` while still held.
    sold: Option<(NaiveDate, f64)>,
    /// Rupees actually paid per unit of the share's currency, when the lot
    /// was bought with rupees and the rate was recorded.
    paid_rate: Option<f64>,
}

/// The recorded rupee rate of a purchase: `fx_rate` when `fx_currency` is INR.
fn inr_rate(currency: Option<&str>, rate: Option<f64>) -> Option<f64> {
    currency
        .filter(|c| c.eq_ignore_ascii_case("INR"))
        .and(rate)
        .filter(|r| r.is_finite() && *r > 0.0)
}

/// One row of Schedule FA Table A3. Rupee amounts are in paise (shown and
/// serialised as rupees).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FaRow {
    pub country: String,
    pub entity: String,
    pub nature: &'static str,
    pub acquired: NaiveDate,
    pub currency: String,
    pub quantity: f64,
    /// Cost on the purchase date (at the rupee rate paid, when recorded).
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub initial_value: i64,
    /// Highest value of the units held on any trading day of the year.
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub peak_value: i64,
    pub peak_date: Option<NaiveDate>,
    /// Value of the units held on 31 December (zero once all were sold).
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub closing_value: i64,
    /// Dividends credited during the year (gross), dated by ex-date.
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub dividends: i64,
    /// Proceeds of sales during the year.
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub sale_proceeds: i64,
    /// Fields that lack a rupee rate for some day (`peak_value`,
    /// `closing_value`, `dividends`, `sale_proceeds`): their amounts are
    /// incomplete, not zero.
    pub missing_rates: Vec<&'static str>,
}

/// Schedule FA Table A3 for a calendar year.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScheduleFa {
    pub calendar_year: i32,
    pub rows: Vec<FaRow>,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub initial_value: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub peak_value: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub closing_value: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub dividends: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub sale_proceeds: i64,
    /// Holdings left out: no price history for the year.
    pub no_prices: Vec<String>,
    /// Holdings left out: no rupee rate for their purchase date.
    pub no_rate: Vec<String>,
}

impl ScheduleFa {
    /// Something is left out or missing a rate: the totals are incomplete.
    pub fn is_incomplete(&self) -> bool {
        !self.no_prices.is_empty()
            || !self.no_rate.is_empty()
            || self.rows.iter().any(|r| !r.missing_rates.is_empty())
    }
}

/// Units of `pieces` held at the end of `day`.
fn held_on(pieces: &[Piece], day: NaiveDate) -> f64 {
    pieces
        .iter()
        .filter(|p| p.acquired <= day && p.sold.is_none_or(|(s, _)| s > day))
        .map(|p| p.qty)
        .sum()
}

/// Every foreign acquisition in `portfolio`, by (symbol, purchase date):
/// open lots and sold lots alike.
fn acquisitions(portfolio: &Portfolio) -> Vec<(String, NaiveDate, f64, Vec<Piece>)> {
    let mut out: Vec<(String, NaiveDate, f64, Vec<Piece>)> = Vec::new();
    let mut add = |sym: &str, acquired: NaiveDate, price: f64, piece: Piece| match out
        .iter_mut()
        .find(|(s, d, p, _)| s == sym && *d == acquired && (*p - price).abs() < 1e-9)
    {
        Some((_, _, _, pieces)) => pieces.push(piece),
        None => out.push((sym.to_string(), acquired, price, vec![piece])),
    };
    for h in portfolio.holdings.iter().filter(|h| is_foreign(&h.symbol)) {
        for l in &h.lots {
            let Some(acquired) = date(&l.date) else {
                continue;
            };
            add(
                &h.symbol,
                acquired,
                l.price,
                Piece {
                    acquired,
                    qty: l.qty,
                    sold: None,
                    paid_rate: inr_rate(l.fx_currency.as_deref(), l.fx_rate),
                },
            );
        }
    }
    for s in portfolio.realized.iter().filter(|s| is_foreign(&s.symbol)) {
        let (Some(acquired), Some(sold)) = (date(&s.buy_date), date(&s.sell_date)) else {
            continue;
        };
        add(
            &s.symbol,
            acquired,
            s.buy_price,
            Piece {
                acquired,
                qty: s.qty,
                sold: Some((sold, s.sell_price)),
                paid_rate: inr_rate(s.fx_currency.as_deref(), s.buy_fx_rate),
            },
        );
    }
    out.sort_by(|a, b| (&a.0, a.1).cmp(&(&b.0, b.1)));
    out
}

/// Schedule FA Table A3 for `year` (1 Jan – 31 Dec). `closes(symbol)` gives
/// the daily closes of the year in the share's quote currency, oldest
/// first; `fx(currency, day)` rupees per unit of a major currency on a day.
pub fn schedule_fa<'a>(
    portfolio: &Portfolio,
    year: i32,
    closes: impl Fn(&str) -> Option<&'a [(NaiveDate, f64)]>,
    fx: impl Fn(&str, NaiveDate) -> Option<f64>,
) -> ScheduleFa {
    use crate::tax::paise;
    let start = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date");
    let end = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid date");
    let mut fa = ScheduleFa {
        calendar_year: year,
        rows: Vec::new(),
        initial_value: 0,
        peak_value: 0,
        closing_value: 0,
        dividends: 0,
        sale_proceeds: 0,
        no_prices: Vec::new(),
        no_rate: Vec::new(),
    };
    for (symbol, acquired, price, pieces) in acquisitions(portfolio) {
        // Held at some time during the year: bought by its end, and not all
        // sold before it began.
        let in_year = acquired <= end
            && pieces
                .iter()
                .any(|p| p.sold.is_none_or(|(s, _)| s >= start));
        if !in_year {
            continue;
        }
        let quote = crate::portfolio::symbol_currency(&symbol)
            .unwrap_or("USD")
            .to_string();
        // Prices in a minor unit (`GBp`) are scaled to the major one.
        let (scale, major) = crate::market::to_major(1.0, &quote);
        let rate = |day: NaiveDate| fx(&major, day);
        // Each purchase at its own recorded rate, else the day's rate.
        let mut initial = 0i64;
        let mut no_initial = false;
        for p in &pieces {
            match p.paid_rate.or_else(|| rate(acquired)) {
                Some(r) => initial += paise::from_rupees(p.qty * price * scale * r),
                None => no_initial = true,
            }
        }
        if no_initial {
            fa.no_rate.push(symbol.clone());
            continue;
        }
        let Some(series) = closes(&symbol).filter(|s| !s.is_empty()) else {
            fa.no_prices.push(symbol.clone());
            continue;
        };
        let mut missing: Vec<&'static str> = Vec::new();
        let mut miss = |field: &'static str| {
            if !missing.contains(&field) {
                missing.push(field);
            }
        };
        let quantity: f64 = pieces.iter().map(|p| p.qty).sum();
        let mut peak = (0i64, None);
        for (day, close) in series.iter().filter(|(d, _)| *d >= start && *d <= end) {
            let units = held_on(&pieces, *day);
            if units <= 1e-9 {
                continue;
            }
            let Some(r) = rate(*day) else {
                miss("peak_value");
                continue;
            };
            let value = paise::from_rupees(units * close * scale * r);
            if value > peak.0 {
                peak = (value, Some(*day));
            }
        }
        let closing_units = held_on(&pieces, end);
        let closing_value = match series.iter().rev().find(|(d, _)| *d <= end) {
            Some((d, close)) if closing_units > 1e-9 => match rate(end).or_else(|| rate(*d)) {
                Some(r) => paise::from_rupees(closing_units * close * scale * r),
                None => {
                    miss("closing_value");
                    0
                }
            },
            _ => 0,
        };
        let mut sale_proceeds = 0i64;
        for (qty, (d, px)) in pieces
            .iter()
            .filter_map(|p| Some((p.qty, p.sold?)))
            .filter(|(_, (d, _))| *d >= start && *d <= end)
        {
            match rate(d) {
                Some(r) => sale_proceeds += paise::from_rupees(qty * px * scale * r),
                None => miss("sale_proceeds"),
            }
        }
        // Dividends: this lot's share of each one, by the units it held on
        // the ex-date, in the currency the dividend was paid in.
        let mut dividends = 0i64;
        for (d, day) in portfolio
            .dividends
            .iter()
            .filter(|d| d.symbol == symbol)
            .filter_map(|d| Some((d, date(&d.ex_date)?)))
            .filter(|(_, day)| *day >= start && *day <= end)
        {
            let mine = held_on(&pieces, day - chrono::Duration::days(1));
            let share = if d.qty > 1e-9 {
                (mine / d.qty).min(1.0)
            } else {
                0.0
            };
            let paid_in = d.currency.clone().unwrap_or_else(|| quote.clone());
            let (amount, cur) = crate::market::to_major(d.amount * share, &paid_in);
            match fx(&cur, day) {
                Some(r) => dividends += paise::from_rupees(amount * r),
                None => miss("dividends"),
            }
        }
        let row = FaRow {
            country: country_of(&symbol),
            entity: symbol.clone(),
            nature: "Company",
            acquired,
            currency: major.to_uppercase(),
            quantity,
            initial_value: initial,
            peak_value: peak.0,
            peak_date: peak.1,
            closing_value,
            dividends,
            sale_proceeds,
            missing_rates: missing,
        };
        fa.initial_value += row.initial_value;
        fa.peak_value += row.peak_value;
        fa.closing_value += row.closing_value;
        fa.dividends += row.dividends;
        fa.sale_proceeds += row.sale_proceeds;
        fa.rows.push(row);
    }
    fa.no_prices.sort();
    fa.no_prices.dedup();
    fa.no_rate.sort();
    fa.no_rate.dedup();
    fa
}

fn money(p: i64) -> String {
    format!("{:.2}", crate::tax::paise::to_rupees(p) + 0.0)
}

/// Schedule FA Table A3 as CSV; holdings left out follow the rows, so an
/// incomplete file says so.
pub fn schedule_fa_csv(fa: &ScheduleFa) -> String {
    use crate::output::csv_line;
    let mut out = csv_line(&[
        "country",
        "name_of_entity",
        "nature_of_entity",
        "date_of_acquiring",
        "currency",
        "quantity",
        "initial_value_inr",
        "peak_value_inr",
        "peak_value_date",
        "closing_balance_inr",
        "gross_amount_credited_inr",
        "gross_sale_proceeds_inr",
        "missing_rates",
    ]);
    for r in &fa.rows {
        out.push_str(&csv_line(&[
            r.country.clone(),
            r.entity.clone(),
            r.nature.to_string(),
            r.acquired.format("%Y-%m-%d").to_string(),
            r.currency.clone(),
            format!("{}", r.quantity),
            money(r.initial_value),
            money(r.peak_value),
            r.peak_date
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default(),
            money(r.closing_value),
            money(r.dividends),
            money(r.sale_proceeds),
            r.missing_rates.join(" "),
        ]));
    }
    if !fa.no_prices.is_empty() || !fa.no_rate.is_empty() {
        out.push('\n');
        out.push_str(&csv_line(&["incomplete", "holdings"]));
        for (label, v) in [
            ("omitted_no_prices", &fa.no_prices),
            ("omitted_no_rupee_rate", &fa.no_rate),
        ] {
            if !v.is_empty() {
                out.push_str(&csv_line(&[label.to_string(), v.join(" ")]));
            }
        }
    }
    out
}

/// The day Rule 128 takes the rate from for tax withheld on `day`: the last
/// day of the month before.
pub fn form67_rate_date(day: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(day.year(), day.month(), 1).expect("valid date")
        - chrono::Duration::days(1)
}

/// One foreign dividend for Form 67 (rupee amounts in paise, shown as
/// rupees; foreign amounts in their minor units).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Form67Row {
    /// The ex-date: the withholding date is not recorded, so it stands in
    /// for it (`date_basis`).
    pub date: NaiveDate,
    pub date_basis: &'static str,
    pub payer: String,
    pub country: String,
    pub currency: String,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub gross: i64,
    /// `None`: no withholding rate was given, so the credit is unknown.
    #[serde(serialize_with = "opt_paise")]
    pub tax_withheld: Option<i64>,
    pub rate_date: NaiveDate,
    pub fx_rate: f64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub gross_inr: i64,
    #[serde(serialize_with = "opt_paise")]
    pub tax_withheld_inr: Option<i64>,
    /// Indian tax on this income at the slab rate, with cess.
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub indian_tax_inr: i64,
    /// The credit: the lower of the tax withheld and the Indian tax.
    #[serde(serialize_with = "opt_paise")]
    pub credit_inr: Option<i64>,
}

fn opt_paise<S: serde::Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(p) => s.serialize_f64(crate::tax::paise::to_rupees(*p)),
        None => s.serialize_none(),
    }
}

/// Form 67 inputs for the financial year starting `fy_start`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Form67 {
    pub fy_label: String,
    /// The slab rate the Indian tax was taken at (a fraction, before cess).
    pub slab_rate: f64,
    /// The rate of tax withheld abroad, as given; `None` when unknown.
    pub withholding_rate: Option<f64>,
    pub rows: Vec<Form67Row>,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub gross_inr: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub tax_withheld_inr: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub credit_inr: i64,
    /// Dividends left out: no rupee rate for the Rule 128 date.
    pub no_rate: Vec<String>,
}

impl Form67 {
    pub fn is_incomplete(&self) -> bool {
        !self.no_rate.is_empty() || self.withholding_rate.is_none() && !self.rows.is_empty()
    }
}

/// Form 67 rows from the portfolio's foreign dividends received in the
/// financial year starting `fy_start`. `withholding` is the rate of tax
/// withheld abroad as your statement shows it (US 25% with a W-8BEN, 30%
/// without); `None` leaves the tax and the credit unknown rather than
/// assuming a treaty rate.
pub fn form67(
    portfolio: &Portfolio,
    fy_start: NaiveDate,
    slab_rate: f64,
    withholding: Option<f64>,
    fx: impl Fn(&str, NaiveDate) -> Option<f64>,
) -> Form67 {
    use crate::tax::paise;
    let (from, to) = crate::tax::Country::India.tax_year(fy_start);
    let mut rows = Vec::new();
    let mut no_rate = Vec::new();
    for d in &portfolio.dividends {
        let Some(day) = date(&d.ex_date).filter(|x| *x >= from && *x <= to) else {
            continue;
        };
        let listed = d
            .currency
            .clone()
            .or_else(|| crate::portfolio::symbol_currency(&d.symbol).map(str::to_string))
            .unwrap_or_else(|| "INR".into());
        let (amount, currency) = crate::market::to_major(d.amount, &listed);
        let currency = currency.to_uppercase();
        if currency == "INR" {
            continue;
        }
        let rate_date = form67_rate_date(day);
        let Some(rate) = fx(&currency, rate_date) else {
            no_rate.push(d.symbol.clone());
            continue;
        };
        let gross = paise::from_rupees(amount);
        let gross_inr = paise::from_rupees(amount * rate);
        let indian = paise::times(
            paise::times(gross_inr, slab_rate),
            1.0 + crate::tax::INDIA_CESS_RATE,
        );
        let withheld = withholding.map(|w| paise::times(gross, w));
        let withheld_inr = withholding.map(|w| paise::times(gross_inr, w));
        rows.push(Form67Row {
            date: day,
            date_basis: "ex_date",
            payer: d.symbol.clone(),
            country: country_of(&d.symbol),
            currency,
            gross,
            tax_withheld: withheld,
            rate_date,
            fx_rate: rate,
            gross_inr,
            tax_withheld_inr: withheld_inr,
            indian_tax_inr: indian,
            credit_inr: withheld_inr.map(|t| t.min(indian)),
        });
    }
    rows.sort_by(|a, b| (a.date, &a.payer).cmp(&(b.date, &b.payer)));
    no_rate.sort();
    no_rate.dedup();
    Form67 {
        fy_label: crate::tax::Country::India.tax_year_label(from),
        slab_rate,
        withholding_rate: withholding,
        gross_inr: rows.iter().map(|r| r.gross_inr).sum(),
        tax_withheld_inr: rows.iter().filter_map(|r| r.tax_withheld_inr).sum(),
        credit_inr: rows.iter().filter_map(|r| r.credit_inr).sum(),
        rows,
        no_rate,
    }
}

/// Form 67 rows as CSV; dividends left out, and an unknown withholding,
/// follow the rows.
pub fn form67_csv(f: &Form67) -> String {
    use crate::output::csv_line;
    let opt = |x: Option<i64>| x.map(money).unwrap_or_else(|| "unknown".into());
    let mut out = csv_line(&[
        "date",
        "date_basis",
        "payer",
        "country",
        "currency",
        "gross",
        "tax_withheld",
        "rate_date",
        "fx_rate",
        "gross_inr",
        "tax_withheld_inr",
        "indian_tax_inr",
        "credit_inr",
    ]);
    for r in &f.rows {
        out.push_str(&csv_line(&[
            r.date.format("%Y-%m-%d").to_string(),
            r.date_basis.to_string(),
            r.payer.clone(),
            r.country.clone(),
            r.currency.clone(),
            money(r.gross),
            opt(r.tax_withheld),
            r.rate_date.format("%Y-%m-%d").to_string(),
            format!("{:.4}", r.fx_rate),
            money(r.gross_inr),
            opt(r.tax_withheld_inr),
            money(r.indian_tax_inr),
            opt(r.credit_inr),
        ]));
    }
    if f.is_incomplete() {
        out.push('\n');
        out.push_str(&csv_line(&["incomplete", "detail"]));
        if !f.no_rate.is_empty() {
            out.push_str(&csv_line(&[
                "omitted_no_rupee_rate".to_string(),
                f.no_rate.join(" "),
            ]));
        }
        if f.withholding_rate.is_none() && !f.rows.is_empty() {
            out.push_str(&csv_line(&[
                "withholding_unknown",
                "give the rate withheld (--withholding 25) from your broker's statement",
            ]));
        }
    }
    out
}

#[cfg(test)]
// Amounts in paise are written rupees_paise: 1_200_00 is ₹1,200.00.
#[allow(clippy::inconsistent_digit_grouping)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn portfolio(json: serde_json::Value) -> Portfolio {
        serde_json::from_value(json).unwrap()
    }

    /// 10 AAPL bought 2024-06-03 at $190; 4 sold 2025-09-01 at $230; a $10
    /// dividend on all 10 in February.
    fn sample() -> Portfolio {
        portfolio(serde_json::json!({
            "holdings": [{"symbol": "AAPL", "shares": 6.0, "avg_cost": 190.0,
                          "lots": [{"qty": 6.0, "price": 190.0, "date": "2024-06-03",
                                    "source": "manual"}]}],
            "realized": [{"symbol": "AAPL", "qty": 4.0, "buy_date": "2024-06-03",
                          "buy_price": 190.0, "sell_date": "2025-09-01",
                          "sell_price": 230.0, "source": "manual"}],
            "dividends": [{"symbol": "AAPL", "ex_date": "2025-02-10", "per_share": 1.0,
                           "qty": 10.0, "amount": 10.0, "currency": "USD"}],
        }))
    }

    #[test]
    fn peak_value_uses_units_held_price_and_that_days_rate() {
        let p = sample();
        // The price peaks on 1 Jul with 10 units; the rupee is weakest on 1 Aug.
        let closes = [
            (d("2025-01-02"), 200.0),
            (d("2025-07-01"), 240.0),
            (d("2025-08-01"), 235.0),
            (d("2025-10-01"), 250.0),
            (d("2025-12-31"), 245.0),
        ];
        let fx = |_: &str, day: NaiveDate| {
            Some(if day == d("2025-08-01") {
                90.0
            } else if day < d("2025-01-01") {
                83.0
            } else {
                85.0
            })
        };
        let fa = schedule_fa(&p, 2025, |_| Some(&closes[..]), fx);
        assert_eq!(fa.rows.len(), 1);
        let r = &fa.rows[0];
        assert_eq!(r.country, "United States");
        assert_eq!(r.acquired, d("2024-06-03"));
        assert_eq!(r.quantity, 10.0);
        assert_eq!(r.initial_value, 10 * 190 * 83 * 100);
        // 1 Aug: 10 × 235 × 90 = 211,500 beats 1 Jul (10 × 240 × 85 = 204,000)
        // and October (6 units after the sale: 6 × 250 × 85 = 127,500).
        assert_eq!(r.peak_value, 211_500_00);
        assert_eq!(r.peak_date, Some(d("2025-08-01")));
        assert_eq!(r.closing_value, 6 * 245 * 85 * 100);
        assert_eq!(r.sale_proceeds, 4 * 230 * 85 * 100);
        assert_eq!(r.dividends, 10 * 85 * 100);
        assert!(r.missing_rates.is_empty() && !fa.is_incomplete());
        let csv = schedule_fa_csv(&fa);
        assert!(csv.starts_with("country,name_of_entity,nature_of_entity,date_of_acquiring,"));
        assert!(csv.contains("United States,AAPL,Company,2024-06-03,USD,10,"));
    }

    #[test]
    fn the_initial_value_uses_the_rupee_rate_paid_when_recorded() {
        let p = portfolio(serde_json::json!({
            "holdings": [{"symbol": "MSFT", "shares": 2.0, "avg_cost": 400.0,
                          "lots": [{"qty": 2.0, "price": 400.0, "date": "2025-03-03",
                                    "source": "manual", "fx_currency": "INR",
                                    "fx_rate": 87.5}]}],
        }));
        let closes = [(d("2025-06-02"), 450.0)];
        let fa = schedule_fa(&p, 2025, |_| Some(&closes[..]), |_, _| Some(85.0));
        assert_eq!(fa.rows[0].initial_value, 70_000_00);
        assert_eq!(fa.rows[0].peak_value, 2 * 450 * 85 * 100);
    }

    #[test]
    fn holdings_outside_the_year_and_rupee_holdings_are_not_reported() {
        let p = portfolio(serde_json::json!({
            "holdings": [
                {"symbol": "MSFT", "shares": 1.0, "avg_cost": 400.0,
                 "lots": [{"qty": 1.0, "price": 400.0, "date": "2026-02-01", "source": "manual"}]},
                {"symbol": "TCS.NS", "shares": 1.0, "avg_cost": 3000.0,
                 "lots": [{"qty": 1.0, "price": 3000.0, "date": "2024-02-01", "source": "manual"}]}],
            "realized": [{"symbol": "NVDA", "qty": 1.0, "buy_date": "2023-01-01",
                          "buy_price": 100.0, "sell_date": "2024-12-01",
                          "sell_price": 130.0, "source": "manual"}],
        }));
        let closes = [(d("2025-06-02"), 1.0)];
        let fa = schedule_fa(&p, 2025, |_| Some(&closes[..]), |_, _| Some(85.0));
        assert!(fa.rows.is_empty(), "{:?}", fa.rows);
    }

    #[test]
    fn form67_converts_at_the_previous_month_end_and_caps_the_credit() {
        assert_eq!(form67_rate_date(d("2025-08-14")), d("2025-07-31"));
        assert_eq!(form67_rate_date(d("2025-03-01")), d("2025-02-28"));
        let p = sample();
        let fx = |_: &str, day: NaiveDate| (day == d("2025-01-31")).then_some(86.0);
        // FY 2024-25 holds the 10 Feb 2025 dividend; 25% withheld per the statement.
        let f = form67(&p, d("2024-04-01"), 0.30, Some(0.25), fx);
        assert_eq!(f.rows.len(), 1);
        let r = &f.rows[0];
        assert_eq!(r.date_basis, "ex_date");
        assert_eq!(r.gross_inr, 860_00);
        assert_eq!(r.tax_withheld_inr, Some(215_00));
        // 30% + 4% cess = 268.32 > 215: the whole 215 is credited.
        assert_eq!(r.credit_inr, Some(215_00));
        // At a 5% slab the Indian tax (44.72) caps the credit.
        let low = form67(&p, d("2024-04-01"), 0.05, Some(0.25), fx);
        assert_eq!(low.credit_inr, 44_72);
        // No withholding rate given: the credit is unknown, not assumed.
        let unknown = form67(&p, d("2024-04-01"), 0.30, None, fx);
        assert_eq!(unknown.rows[0].credit_inr, None);
        assert_eq!(unknown.credit_inr, 0);
        assert!(unknown.is_incomplete());
        assert!(form67_csv(&unknown).contains("withholding_unknown"));
    }

    #[test]
    fn each_purchase_keeps_its_own_rupee_rate() {
        let p = portfolio(serde_json::json!({
            "holdings": [{"symbol": "MSFT", "shares": 2.0, "avg_cost": 100.0,
                          "lots": [{"qty": 1.0, "price": 100.0, "date": "2025-03-03",
                                    "source": "manual", "fx_currency": "INR", "fx_rate": 80.0},
                                   {"qty": 1.0, "price": 100.0, "date": "2025-03-03",
                                    "source": "manual", "fx_currency": "INR", "fx_rate": 90.0}]}],
        }));
        let closes = [(d("2025-06-02"), 110.0)];
        let fa = schedule_fa(&p, 2025, |_| Some(&closes[..]), |_, _| Some(85.0));
        assert_eq!(fa.rows[0].initial_value, 17_000_00);
    }

    #[test]
    fn missing_rates_are_flagged_not_counted_as_zero() {
        let p = sample();
        let closes = [(d("2025-07-01"), 240.0), (d("2025-12-31"), 245.0)];
        // A rate only for the purchase day and 1 July.
        let fx = |_: &str, day: NaiveDate| {
            (day == d("2024-06-03") || day == d("2025-07-01")).then_some(85.0)
        };
        let fa = schedule_fa(&p, 2025, |_| Some(&closes[..]), fx);
        let r = &fa.rows[0];
        assert_eq!(r.peak_value, 10 * 240 * 85 * 100);
        for field in ["closing_value", "sale_proceeds", "dividends"] {
            assert!(r.missing_rates.contains(&field), "{field}");
        }
        assert!(fa.is_incomplete());
        assert!(schedule_fa_csv(&fa).contains("closing_value"));
    }

    #[test]
    fn a_dividend_is_converted_in_the_currency_it_was_paid_in() {
        // A London share quoted in pence with a dividend recorded in pounds.
        let p = portfolio(serde_json::json!({
            "holdings": [{"symbol": "ULVR.L", "shares": 10.0, "avg_cost": 4000.0,
                          "lots": [{"qty": 10.0, "price": 4000.0, "date": "2024-01-02",
                                    "source": "manual"}]}],
            "dividends": [{"symbol": "ULVR.L", "ex_date": "2025-05-01", "per_share": 1.0,
                           "qty": 10.0, "amount": 10.0, "currency": "GBP"}],
        }));
        let closes = [(d("2025-06-02"), 4500.0)];
        let fa = schedule_fa(&p, 2025, |_| Some(&closes[..]), |_, _| Some(110.0));
        let r = &fa.rows[0];
        assert_eq!(r.currency, "GBP");
        // £10 at ₹110, not 10 pence.
        assert_eq!(r.dividends, 1_100_00);
        // 10 × 4,000 pence = £400 at ₹110.
        assert_eq!(r.initial_value, 44_000_00);
    }
}
