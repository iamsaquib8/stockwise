//! Dividend and interest income for an Indian financial year, as ITR
//! Schedule OS wants it: gross, TDS deducted and net, per payer, split by
//! the advance-tax periods of Sec. 234C (dividends received after a due date
//! carry no interest for the earlier instalments).
//!
//! Dividends come from the portfolio's recorded corporate actions
//! (`portfolio actions --apply`), dated by their ex-date. Interest is the
//! 2.5% Sovereign Gold Bond coupon, paid every six months from the issue
//! date on the units held that day. Foreign dividends are converted to
//! rupees at the day's rate and carry the US withholding for Form 67.
//!
//! Estimates: a company's TDS can differ (a Form 15G/H, a lower-deduction
//! certificate, a missing PAN at 20%); verify against Form 26AS / AIS.

use crate::portfolio::{DividendReceived, Portfolio};
use chrono::{Datelike, Months, NaiveDate};
use serde::Serialize;

/// TDS on dividends (Sec. 194 for companies, 194K for mutual funds).
pub const DIVIDEND_TDS_RATE: f64 = 0.10;

/// A company or fund deducts TDS once its dividends to one shareholder in a
/// financial year exceed this: ₹5,000 up to FY 2024-25, ₹10,000 from FY
/// 2025-26 (Finance Act 2025).
pub fn dividend_tds_threshold(fy_start_year: i32) -> f64 {
    if fy_start_year >= 2025 {
        10_000.0
    } else {
        5_000.0
    }
}

/// The Schedule OS quarterly-breakup periods, which are also Sec. 234C's.
pub const QUARTER_LABELS: [&str; 5] = [
    "Up to 15 Jun",
    "16 Jun – 15 Sep",
    "16 Sep – 15 Dec",
    "16 Dec – 15 Mar",
    "16 Mar – 31 Mar",
];

/// Which of [`QUARTER_LABELS`] `date` (inside the FY) falls in.
pub fn quarter_of(date: NaiveDate) -> usize {
    let md = (date.month(), date.day());
    match md {
        (4..=5, _) | (6, 1..=15) => 0,
        (6, _) | (7..=8, _) | (9, 1..=15) => 1,
        (9, _) | (10..=11, _) | (12, 1..=15) => 2,
        (12, _) | (1..=2, _) | (3, 1..=15) => 3,
        _ => 4,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IncomeKind {
    /// A dividend from an Indian company or fund (Sec. 194/194K TDS).
    Dividend,
    /// A dividend paid in another currency: taxable in India, with the tax
    /// withheld abroad claimed as a foreign tax credit (Form 67).
    ForeignDividend,
    /// Sovereign Gold Bond interest: taxable, no TDS (Sec. 193 proviso).
    SgbInterest,
}

impl IncomeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Dividend => "Dividend",
            Self::ForeignDividend => "Foreign dividend",
            Self::SgbInterest => "SGB interest",
        }
    }
}

/// One dividend or coupon.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncomeRow {
    pub date: NaiveDate,
    pub payer: String,
    pub kind: IncomeKind,
    /// Index into [`QUARTER_LABELS`].
    pub quarter: usize,
    pub currency: String,
    /// In `currency` (equal to `gross` for rupees).
    pub gross_original: f64,
    /// Rupees per unit of `currency` on `date` (1 for rupees).
    pub fx_rate: f64,
    pub gross: f64,
    /// Indian TDS deducted (estimate).
    pub tds: f64,
    /// Tax withheld abroad, in rupees (US 25% under the DTAA with a W-8BEN).
    pub foreign_tax: f64,
    pub net: f64,
}

/// A financial year's dividend and interest income.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncomeReport {
    pub fy_label: String,
    pub fy_start: NaiveDate,
    pub fy_end: NaiveDate,
    pub tds_threshold: f64,
    pub rows: Vec<IncomeRow>,
    pub dividends: f64,
    pub foreign_dividends: f64,
    pub interest: f64,
    pub gross: f64,
    pub tds: f64,
    pub foreign_tax: f64,
    pub net: f64,
    /// Gross income per [`QUARTER_LABELS`] period: dividends only (the ITR
    /// breakup is for dividends) and all income.
    pub dividend_by_quarter: [f64; 5],
    pub income_by_quarter: [f64; 5],
    /// Payers whose currency had no rupee rate for a date (left out).
    pub no_rate: Vec<String>,
    /// SGB holdings whose tranche StockWise does not know (no coupons).
    pub unknown_sgbs: Vec<String>,
}

fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

fn date(s: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
}

/// The symbol a payer is shown as: `RELIANCE.NS` → `RELIANCE`.
fn payer(symbol: &str) -> String {
    let up = symbol.trim();
    up.strip_suffix(".NS")
        .or_else(|| up.strip_suffix(".BO"))
        .unwrap_or(up)
        .to_string()
}

/// SGB coupon dates from `issued` (every six months, up to the eight-year
/// maturity) that fall from `from` to `to`.
pub fn sgb_coupon_dates(issued: NaiveDate, from: NaiveDate, to: NaiveDate) -> Vec<NaiveDate> {
    (1..=16)
        .filter_map(|i| issued.checked_add_months(Months::new(6 * i)))
        .filter(|d| *d >= from && *d <= to)
        .collect()
}

/// Units of `symbol` held on `day` (bought before it, not sold by then).
fn units_held_on(portfolio: &Portfolio, symbol: &str, day: NaiveDate) -> f64 {
    let open: f64 = portfolio
        .holdings
        .iter()
        .filter(|h| h.symbol == symbol)
        .flat_map(|h| &h.lots)
        .filter(|l| date(l.held_from()).is_some_and(|d| d < day))
        .map(|l| l.qty)
        .sum();
    let sold_later: f64 = portfolio
        .realized
        .iter()
        .filter(|s| s.symbol == symbol)
        .filter(|s| date(s.held_from()).is_some_and(|d| d < day))
        .filter(|s| date(&s.sell_date).is_some_and(|d| d >= day))
        .map(|s| s.qty)
        .sum();
    open + sold_later
}

/// The dividend and interest income of the financial year starting
/// `fy_start`. `fx(currency, date)` gives rupees per unit of a currency on a
/// day; a dividend whose rate is unknown is left out and named.
pub fn income_report(
    portfolio: &Portfolio,
    fy_start: NaiveDate,
    fx: impl Fn(&str, NaiveDate) -> Option<f64>,
) -> IncomeReport {
    let (from, to) = crate::tax::Country::India.tax_year(fy_start);
    let y = from.year();
    let threshold = dividend_tds_threshold(y);
    let mut rows: Vec<IncomeRow> = Vec::new();
    let mut no_rate = Vec::new();

    // Dividends, oldest first so the TDS threshold is crossed in order.
    let mut divs: Vec<(&DividendReceived, NaiveDate)> = portfolio
        .dividends
        .iter()
        .filter_map(|d| Some((d, date(&d.ex_date)?)))
        .filter(|(_, d)| *d >= from && *d <= to)
        .collect();
    divs.sort_by_key(|(d, day)| (*day, d.symbol.clone()));
    let mut paid_by: Vec<(String, f64)> = Vec::new();
    for (d, day) in divs {
        let currency = d
            .currency
            .clone()
            .or_else(|| crate::portfolio::symbol_currency(&d.symbol).map(str::to_string))
            .unwrap_or_else(|| "INR".into());
        let domestic = currency.eq_ignore_ascii_case("INR");
        let rate = if domestic {
            Some(1.0)
        } else {
            fx(&currency, day)
        };
        let Some(rate) = rate else {
            no_rate.push(d.symbol.clone());
            continue;
        };
        let gross = d.amount * rate;
        let (kind, tds, foreign_tax) = if domestic {
            let who = payer(&d.symbol);
            let total = match paid_by.iter_mut().find(|(p, _)| *p == who) {
                Some((_, t)) => {
                    *t += gross;
                    *t
                }
                None => {
                    paid_by.push((who, gross));
                    gross
                }
            };
            let tds = if total > threshold {
                gross * DIVIDEND_TDS_RATE
            } else {
                0.0
            };
            (IncomeKind::Dividend, tds, 0.0)
        } else {
            let wht = if currency.eq_ignore_ascii_case("USD") {
                crate::income::DEFAULT_US_WHT_RATE
            } else {
                0.0
            };
            (IncomeKind::ForeignDividend, 0.0, gross * wht)
        };
        rows.push(IncomeRow {
            date: day,
            payer: payer(&d.symbol),
            kind,
            quarter: quarter_of(day),
            currency: currency.to_uppercase(),
            gross_original: round2(d.amount),
            fx_rate: rate,
            gross: round2(gross),
            tds: round2(tds),
            foreign_tax: round2(foreign_tax),
            net: round2(gross - tds - foreign_tax),
        });
    }

    // SGB interest on the units held on each coupon date.
    let mut sgbs: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .chain(portfolio.realized.iter().map(|s| s.symbol.clone()))
        .filter(|s| crate::offmarket::is_sgb(s))
        .collect();
    sgbs.sort();
    sgbs.dedup();
    let mut unknown_sgbs = Vec::new();
    for sym in sgbs {
        let id = crate::offmarket::sgb_id(&sym).unwrap_or_default();
        let Some(t) = crate::commodity::SGB_TRANCHES
            .iter()
            .find(|t| t.symbol.eq_ignore_ascii_case(&id) || t.isin.eq_ignore_ascii_case(&id))
        else {
            unknown_sgbs.push(sym);
            continue;
        };
        let per_unit = t.issue_price * crate::commodity::SGB_COUPON_PCT / 100.0 / 2.0;
        for day in sgb_coupon_dates(t.issue_date(), from, to) {
            let units = units_held_on(portfolio, &sym, day);
            if units <= 1e-9 {
                continue;
            }
            let gross = units * per_unit;
            rows.push(IncomeRow {
                date: day,
                payer: t.symbol.to_string(),
                kind: IncomeKind::SgbInterest,
                quarter: quarter_of(day),
                currency: "INR".into(),
                gross_original: round2(gross),
                fx_rate: 1.0,
                gross: round2(gross),
                tds: 0.0,
                foreign_tax: 0.0,
                net: round2(gross),
            });
        }
    }
    rows.sort_by(|a, b| (a.date, &a.payer).cmp(&(b.date, &b.payer)));

    let sum = |k: Option<IncomeKind>, f: fn(&IncomeRow) -> f64| -> f64 {
        round2(
            rows.iter()
                .filter(|r| k.is_none_or(|k| r.kind == k))
                .map(f)
                .sum(),
        )
    };
    let mut dividend_by_quarter = [0.0; 5];
    let mut income_by_quarter = [0.0; 5];
    for r in &rows {
        income_by_quarter[r.quarter] += r.gross;
        if r.kind != IncomeKind::SgbInterest {
            dividend_by_quarter[r.quarter] += r.gross;
        }
    }
    no_rate.sort();
    no_rate.dedup();
    IncomeReport {
        fy_label: crate::tax::Country::India.tax_year_label(from),
        fy_start: from,
        fy_end: to,
        tds_threshold: threshold,
        dividends: sum(Some(IncomeKind::Dividend), |r| r.gross),
        foreign_dividends: sum(Some(IncomeKind::ForeignDividend), |r| r.gross),
        interest: sum(Some(IncomeKind::SgbInterest), |r| r.gross),
        gross: sum(None, |r| r.gross),
        tds: sum(None, |r| r.tds),
        foreign_tax: sum(None, |r| r.foreign_tax),
        net: sum(None, |r| r.net),
        dividend_by_quarter: dividend_by_quarter.map(round2),
        income_by_quarter: income_by_quarter.map(round2),
        rows,
        no_rate,
        unknown_sgbs,
    }
}

/// The income as advance tax sees it: every row at the slab `rate`.
pub fn advance_income(report: &IncomeReport, rate: f64) -> Vec<crate::tax::AdvanceIncome> {
    report
        .rows
        .iter()
        .map(|r| crate::tax::AdvanceIncome {
            date: r.date,
            amount: r.gross,
            rate,
        })
        .collect()
}

/// Schedule OS as CSV: one row per dividend or coupon, then the dividend
/// quarterly breakup ITR asks for (the Sec. 234C periods).
pub fn schedule_os_csv(r: &IncomeReport) -> String {
    use crate::output::csv_line;
    let money = |x: f64| format!("{x:.2}");
    let mut out = csv_line(&[
        "date",
        "payer",
        "nature",
        "itr_period",
        "currency",
        "gross_in_currency",
        "fx_rate",
        "gross_inr",
        "tds_inr",
        "foreign_tax_inr",
        "net_inr",
    ]);
    for row in &r.rows {
        out.push_str(&csv_line(&[
            row.date.format("%Y-%m-%d").to_string(),
            row.payer.clone(),
            row.kind.label().to_string(),
            QUARTER_LABELS[row.quarter].to_string(),
            row.currency.clone(),
            money(row.gross_original),
            format!("{:.4}", row.fx_rate),
            money(row.gross),
            money(row.tds),
            money(row.foreign_tax),
            money(row.net),
        ]));
    }
    out.push('\n');
    let mut head = vec!["quarterly_breakup".to_string()];
    head.extend(QUARTER_LABELS.iter().map(|s| s.to_string()));
    head.push("total".into());
    out.push_str(&csv_line(&head));
    for (label, q) in [
        ("dividend_income", r.dividend_by_quarter),
        ("all_income", r.income_by_quarter),
    ] {
        let mut line = vec![label.to_string()];
        line.extend(q.iter().map(|x| money(*x)));
        line.push(money(q.iter().sum()));
        out.push_str(&csv_line(&line));
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

    fn div(symbol: &str, ex: &str, amount: f64, cur: &str) -> serde_json::Value {
        serde_json::json!({"symbol": symbol, "ex_date": ex, "per_share": 1.0, "qty": amount,
                           "amount": amount, "currency": cur})
    }

    #[test]
    fn quarters_follow_the_234c_periods() {
        assert_eq!(quarter_of(d("2026-04-01")), 0);
        assert_eq!(quarter_of(d("2026-06-15")), 0);
        assert_eq!(quarter_of(d("2026-06-16")), 1);
        assert_eq!(quarter_of(d("2026-09-15")), 1);
        assert_eq!(quarter_of(d("2026-12-15")), 2);
        assert_eq!(quarter_of(d("2026-12-16")), 3);
        assert_eq!(quarter_of(d("2027-03-15")), 3);
        assert_eq!(quarter_of(d("2027-03-16")), 4);
        assert_eq!(quarter_of(d("2027-03-31")), 4);
    }

    #[test]
    fn tds_starts_once_a_company_pays_more_than_the_threshold() {
        assert_eq!(dividend_tds_threshold(2024), 5_000.0);
        assert_eq!(dividend_tds_threshold(2026), 10_000.0);
        let p = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [
                div("ITC.NS", "2026-05-28", 6_000.0, "INR"),
                // 6,000 + 5,000 = 11,000 > 10,000: this one has TDS.
                div("ITC.NS", "2026-11-20", 5_000.0, "INR"),
                // Another company: its own threshold.
                div("TCS.NS", "2026-07-10", 9_000.0, "INR"),
                // Last year's: left out.
                div("TCS.NS", "2026-03-10", 50_000.0, "INR"),
            ],
        }));
        let r = income_report(&p, d("2026-04-01"), |_, _| None);
        assert_eq!(r.rows.len(), 3);
        let itc: Vec<&IncomeRow> = r.rows.iter().filter(|r| r.payer == "ITC").collect();
        assert_eq!(itc[0].tds, 0.0);
        assert_eq!(itc[1].tds, 500.0);
        assert_eq!(itc[1].net, 4_500.0);
        assert!(r.rows.iter().all(|r| r.payer != "TCS" || r.tds == 0.0));
        assert_eq!(r.dividends, 20_000.0);
        assert_eq!(r.tds, 500.0);
        assert_eq!(r.dividend_by_quarter, [6_000.0, 9_000.0, 5_000.0, 0.0, 0.0]);
        // The same payments in FY 2024-25 cross the old ₹5,000 threshold sooner.
        let old = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [div("ITC.NS", "2024-05-28", 6_000.0, "INR")],
        }));
        let r = income_report(&old, d("2024-04-01"), |_, _| None);
        assert_eq!(r.tds, 600.0);
    }

    #[test]
    fn foreign_dividends_are_converted_and_carry_us_withholding() {
        let p = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [div("AAPL", "2026-08-14", 100.0, "USD"),
                          div("NESN.SW", "2026-08-14", 50.0, "CHF")],
        }));
        let r = income_report(&p, d("2026-04-01"), |c, _| (c == "USD").then_some(88.0));
        assert_eq!(r.rows.len(), 1);
        let row = &r.rows[0];
        assert_eq!(row.kind, IncomeKind::ForeignDividend);
        assert_eq!(row.gross, 8_800.0);
        assert_eq!(row.foreign_tax, 2_200.0);
        assert_eq!(row.tds, 0.0);
        assert_eq!(r.no_rate, vec!["NESN.SW".to_string()]);
    }

    #[test]
    fn sgb_interest_is_paid_on_units_held_with_no_tds() {
        // SGBNOV26 (issued 13 Nov 2018 at ₹3,183): coupons on 13 May and 13 Nov.
        let p = portfolio(serde_json::json!({
            "holdings": [{"symbol": "SGBNOV26.NS", "shares": 10.0, "avg_cost": 3000.0,
                          "lots": [{"qty": 10.0, "price": 3000.0, "date": "2025-01-10",
                                    "source": "manual"}]}],
            "realized": [{"symbol": "SGBNOV26.NS", "qty": 4.0, "buy_date": "2025-01-10",
                          "buy_price": 3000.0, "sell_date": "2026-08-01",
                          "sell_price": 9000.0, "source": "manual"}],
        }));
        let r = income_report(&p, d("2026-04-01"), |_, _| None);
        let coupons: Vec<&IncomeRow> = r
            .rows
            .iter()
            .filter(|r| r.kind == IncomeKind::SgbInterest)
            .collect();
        assert_eq!(coupons.len(), 2);
        // May: 14 units (4 sold later); Nov: 10.
        let per_unit = 3183.0 * 0.0125;
        assert!((coupons[0].gross - round2(14.0 * per_unit)).abs() < 0.01);
        assert!((coupons[1].gross - round2(10.0 * per_unit)).abs() < 0.01);
        assert!(coupons.iter().all(|c| c.tds == 0.0));
        assert_eq!(coupons[0].quarter, 0);
        assert_eq!(coupons[1].quarter, 2);
    }

    #[test]
    fn schedule_os_csv_has_rows_and_the_quarterly_breakup() {
        let p = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [div("ITC.NS", "2026-05-28", 6_000.0, "INR")],
        }));
        let r = income_report(&p, d("2026-04-01"), |_, _| None);
        let csv = schedule_os_csv(&r);
        assert!(csv.starts_with("date,payer,nature,itr_period,"));
        assert!(csv.contains("2026-05-28,ITC,Dividend,Up to 15 Jun,INR,6000.00"));
        assert!(csv.contains("dividend_income,6000.00,0.00,0.00,0.00,0.00,6000.00"));
    }
}
