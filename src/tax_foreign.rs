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

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
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

/// One row of Schedule FA Table A3.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FaRow {
    pub country: String,
    pub entity: String,
    pub nature: &'static str,
    pub acquired: NaiveDate,
    pub currency: String,
    pub quantity: f64,
    /// Cost on the purchase date, in rupees.
    pub initial_value: f64,
    /// Highest value of the units held on any trading day of the year.
    pub peak_value: f64,
    pub peak_date: Option<NaiveDate>,
    /// Value of the units held on 31 December (zero once all were sold).
    pub closing_value: f64,
    /// Dividends credited during the year (gross).
    pub dividends: f64,
    /// Proceeds of sales during the year.
    pub sale_proceeds: f64,
}

/// Schedule FA Table A3 for a calendar year.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScheduleFa {
    pub calendar_year: i32,
    pub rows: Vec<FaRow>,
    pub initial_value: f64,
    pub peak_value: f64,
    pub closing_value: f64,
    pub dividends: f64,
    pub sale_proceeds: f64,
    /// Holdings left out: no price history for the year.
    pub no_prices: Vec<String>,
    /// Holdings left out: no rupee rate for their currency.
    pub no_rate: Vec<String>,
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
/// the daily closes of the year in the share's currency, oldest first;
/// `fx(currency, day)` rupees per unit on a day.
pub fn schedule_fa<'a>(
    portfolio: &Portfolio,
    year: i32,
    closes: impl Fn(&str) -> Option<&'a [(NaiveDate, f64)]>,
    fx: impl Fn(&str, NaiveDate) -> Option<f64>,
) -> ScheduleFa {
    let start = NaiveDate::from_ymd_opt(year, 1, 1).expect("valid date");
    let end = NaiveDate::from_ymd_opt(year, 12, 31).expect("valid date");
    let mut fa = ScheduleFa {
        calendar_year: year,
        rows: Vec::new(),
        initial_value: 0.0,
        peak_value: 0.0,
        closing_value: 0.0,
        dividends: 0.0,
        sale_proceeds: 0.0,
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
        let currency = crate::portfolio::symbol_currency(&symbol)
            .unwrap_or("USD")
            .to_string();
        let rate = |day: NaiveDate| fx(&currency, day);
        let recorded = pieces.iter().find_map(|p| p.paid_rate);
        let Some(initial_rate) = recorded.or_else(|| rate(acquired)) else {
            fa.no_rate.push(symbol.clone());
            continue;
        };
        let Some(series) = closes(&symbol).filter(|s| !s.is_empty()) else {
            fa.no_prices.push(symbol.clone());
            continue;
        };
        let (scale, _) = crate::market::to_major(1.0, &currency);
        let quantity: f64 = pieces.iter().map(|p| p.qty).sum();
        let mut peak = (0.0, None);
        for (day, close) in series.iter().filter(|(d, _)| *d >= start && *d <= end) {
            let units = held_on(&pieces, *day);
            if units <= 1e-9 {
                continue;
            }
            let Some(r) = rate(*day) else { continue };
            let value = units * close * scale * r;
            if value > peak.0 {
                peak = (value, Some(*day));
            }
        }
        let closing_units = held_on(&pieces, end);
        let closing_value = match series.iter().rev().find(|(d, _)| *d <= end) {
            Some((d, close)) if closing_units > 1e-9 => {
                closing_units * close * scale * rate(end).or_else(|| rate(*d)).unwrap_or(0.0)
            }
            _ => 0.0,
        };
        let sale_proceeds: f64 = pieces
            .iter()
            .filter_map(|p| Some((p.qty, p.sold?)))
            .filter(|(_, (d, _))| *d >= start && *d <= end)
            .map(|(qty, (d, px))| qty * px * scale * rate(d).unwrap_or(0.0))
            .sum();
        // Dividends: this lot's share of each one, by the units it held on
        // the ex-date.
        let dividends: f64 = portfolio
            .dividends
            .iter()
            .filter(|d| d.symbol == symbol)
            .filter_map(|d| Some((d, date(&d.ex_date)?)))
            .filter(|(_, day)| *day >= start && *day <= end)
            .map(|(d, day)| {
                let mine = held_on(&pieces, day - chrono::Duration::days(1));
                let share = if d.qty > 1e-9 {
                    (mine / d.qty).min(1.0)
                } else {
                    0.0
                };
                d.amount * share * scale * rate(day).unwrap_or(0.0)
            })
            .sum();
        let row = FaRow {
            country: country_of(&symbol),
            entity: symbol.clone(),
            nature: "Company",
            acquired,
            currency: currency.clone(),
            quantity,
            initial_value: round2(quantity * price * scale * initial_rate),
            peak_value: round2(peak.0),
            peak_date: peak.1,
            closing_value: round2(closing_value),
            dividends: round2(dividends),
            sale_proceeds: round2(sale_proceeds),
        };
        fa.initial_value += row.initial_value;
        fa.peak_value += row.peak_value;
        fa.closing_value += row.closing_value;
        fa.dividends += row.dividends;
        fa.sale_proceeds += row.sale_proceeds;
        fa.rows.push(row);
    }
    for v in [
        &mut fa.initial_value,
        &mut fa.peak_value,
        &mut fa.closing_value,
        &mut fa.dividends,
        &mut fa.sale_proceeds,
    ] {
        *v = round2(*v);
    }
    fa.no_prices.sort();
    fa.no_prices.dedup();
    fa.no_rate.sort();
    fa.no_rate.dedup();
    fa
}

/// Schedule FA Table A3 as CSV.
pub fn schedule_fa_csv(fa: &ScheduleFa) -> String {
    use crate::output::csv_line;
    let money = |x: f64| format!("{x:.2}");
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
        ]));
    }
    out
}

/// The day Rule 128 takes the rate from for tax withheld on `day`: the last
/// day of the month before.
pub fn form67_rate_date(day: NaiveDate) -> NaiveDate {
    NaiveDate::from_ymd_opt(day.year(), day.month(), 1).expect("valid date")
        - chrono::Duration::days(1)
}

/// One foreign dividend for Form 67.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Form67Row {
    pub date: NaiveDate,
    pub payer: String,
    pub country: String,
    pub currency: String,
    pub gross: f64,
    pub tax_withheld: f64,
    pub rate_date: NaiveDate,
    pub fx_rate: f64,
    pub gross_inr: f64,
    pub tax_withheld_inr: f64,
    /// Indian tax on this income at the slab rate, with cess.
    pub indian_tax_inr: f64,
    /// The credit: the lower of the tax withheld and the Indian tax.
    pub credit_inr: f64,
}

/// Form 67 inputs for the financial year starting `fy_start`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Form67 {
    pub fy_label: String,
    /// The slab rate the Indian tax was taken at (a fraction, before cess).
    pub slab_rate: f64,
    pub rows: Vec<Form67Row>,
    pub gross_inr: f64,
    pub tax_withheld_inr: f64,
    pub credit_inr: f64,
    pub no_rate: Vec<String>,
}

/// Form 67 rows from the portfolio's foreign dividends received in the
/// financial year starting `fy_start`. Withholding is the US 25% treaty
/// rate for dollar dividends (30% without a W-8BEN on file), none known
/// for other currencies.
pub fn form67(
    portfolio: &Portfolio,
    fy_start: NaiveDate,
    slab_rate: f64,
    fx: impl Fn(&str, NaiveDate) -> Option<f64>,
) -> Form67 {
    let (from, to) = crate::tax::Country::India.tax_year(fy_start);
    let mut rows = Vec::new();
    let mut no_rate = Vec::new();
    for d in &portfolio.dividends {
        let Some(day) = date(&d.ex_date).filter(|x| *x >= from && *x <= to) else {
            continue;
        };
        let currency = d
            .currency
            .clone()
            .or_else(|| crate::portfolio::symbol_currency(&d.symbol).map(str::to_string))
            .unwrap_or_else(|| "INR".into());
        if currency.eq_ignore_ascii_case("INR") {
            continue;
        }
        let wht = if currency.eq_ignore_ascii_case("USD") {
            crate::income::DEFAULT_US_WHT_RATE
        } else {
            0.0
        };
        let rate_date = form67_rate_date(day);
        let Some(rate) = fx(&currency, rate_date) else {
            no_rate.push(d.symbol.clone());
            continue;
        };
        let gross_inr = d.amount * rate;
        let tax_inr = gross_inr * wht;
        let indian = gross_inr * slab_rate * (1.0 + crate::tax::INDIA_CESS_RATE);
        rows.push(Form67Row {
            date: day,
            payer: d.symbol.clone(),
            country: country_of(&d.symbol),
            currency: currency.to_uppercase(),
            gross: round2(d.amount),
            tax_withheld: round2(d.amount * wht),
            rate_date,
            fx_rate: rate,
            gross_inr: round2(gross_inr),
            tax_withheld_inr: round2(tax_inr),
            indian_tax_inr: round2(indian),
            credit_inr: round2(tax_inr.min(indian)),
        });
    }
    rows.sort_by(|a, b| (a.date, &a.payer).cmp(&(b.date, &b.payer)));
    no_rate.sort();
    no_rate.dedup();
    Form67 {
        fy_label: crate::tax::Country::India.tax_year_label(from),
        slab_rate,
        gross_inr: round2(rows.iter().map(|r| r.gross_inr).sum()),
        tax_withheld_inr: round2(rows.iter().map(|r| r.tax_withheld_inr).sum()),
        credit_inr: round2(rows.iter().map(|r| r.credit_inr).sum()),
        rows,
        no_rate,
    }
}

/// Form 67 rows as CSV.
pub fn form67_csv(f: &Form67) -> String {
    use crate::output::csv_line;
    let money = |x: f64| format!("{x:.2}");
    let mut out = csv_line(&[
        "date",
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
            r.payer.clone(),
            r.country.clone(),
            r.currency.clone(),
            money(r.gross),
            money(r.tax_withheld),
            r.rate_date.format("%Y-%m-%d").to_string(),
            format!("{:.4}", r.fx_rate),
            money(r.gross_inr),
            money(r.tax_withheld_inr),
            money(r.indian_tax_inr),
            money(r.credit_inr),
        ]));
    }
    out
}

#[cfg(test)]
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
        assert_eq!(r.initial_value, 10.0 * 190.0 * 83.0);
        // 1 Aug: 10 × 235 × 90 = 211,500 beats 1 Jul (10 × 240 × 85 = 204,000)
        // and October (6 units after the sale: 6 × 250 × 85 = 127,500).
        assert_eq!(r.peak_value, 211_500.0);
        assert_eq!(r.peak_date, Some(d("2025-08-01")));
        assert_eq!(r.closing_value, 6.0 * 245.0 * 85.0);
        assert_eq!(r.sale_proceeds, 4.0 * 230.0 * 85.0);
        assert_eq!(r.dividends, 10.0 * 85.0);
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
        assert_eq!(fa.rows[0].initial_value, 2.0 * 400.0 * 87.5);
        assert_eq!(fa.rows[0].peak_value, 2.0 * 450.0 * 85.0);
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
        // FY 2024-25 holds the 10 Feb 2025 dividend.
        let f = form67(&p, d("2024-04-01"), 0.30, fx);
        assert_eq!(f.rows.len(), 1);
        let r = &f.rows[0];
        assert_eq!(r.gross_inr, 860.0);
        assert_eq!(r.tax_withheld_inr, 215.0);
        // 30% + 4% cess = 268.32 > 215: the whole 215 is credited.
        assert_eq!(r.credit_inr, 215.0);
        // At a 5% slab the Indian tax (44.72) caps the credit.
        let low = form67(&p, d("2024-04-01"), 0.05, fx);
        assert_eq!(low.credit_inr, 44.72);
    }
}
