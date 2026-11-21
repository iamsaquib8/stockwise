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

/// One dividend or coupon. Rupee amounts are in paise (shown and
/// serialised as rupees).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncomeRow {
    /// The ex-date for a dividend (the payment date is not recorded), the
    /// coupon date for SGB interest.
    pub date: NaiveDate,
    /// What `date` is: `ex_date` or `coupon_date`.
    pub date_basis: &'static str,
    pub payer: String,
    pub kind: IncomeKind,
    /// Index into [`QUARTER_LABELS`].
    pub quarter: usize,
    /// The currency the amount was paid in (major unit: `GBp` is `GBP`).
    pub currency: String,
    /// In `currency`, in its minor units (cents for dollars).
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub gross_original: i64,
    /// Rupees per unit of `currency` on `date` (1 for rupees).
    pub fx_rate: f64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub gross: i64,
    /// Indian TDS (estimate: 10% of every dividend of a payer whose year's
    /// dividends pass the threshold).
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub tds: i64,
    /// Tax withheld abroad, in rupees, at the `--withholding` rate given;
    /// `None` when no rate was given (unknown, not zero).
    #[serde(serialize_with = "opt_paise")]
    pub foreign_tax: Option<i64>,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub net: i64,
}

fn opt_paise<S: serde::Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(p) => s.serialize_f64(crate::tax::paise::to_rupees(*p)),
        None => s.serialize_none(),
    }
}

/// A financial year's dividend and interest income (paise, shown as rupees).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IncomeReport {
    pub fy_label: String,
    pub fy_start: NaiveDate,
    pub fy_end: NaiveDate,
    pub tds_threshold: f64,
    pub rows: Vec<IncomeRow>,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub dividends: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub foreign_dividends: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub interest: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub gross: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub tds: i64,
    /// Tax withheld abroad at the rate given; foreign dividends without one
    /// are in `withholding_unknown`.
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub foreign_tax: i64,
    #[serde(serialize_with = "crate::tax::paise::serialize")]
    pub net: i64,
    /// Gross income per [`QUARTER_LABELS`] period: dividends only (the ITR
    /// breakup is for dividends) and all income.
    #[serde(serialize_with = "crate::tax::paise::serialize_array")]
    pub dividend_by_quarter: [i64; 5],
    #[serde(serialize_with = "crate::tax::paise::serialize_array")]
    pub income_by_quarter: [i64; 5],
    /// Payers whose currency had no rupee rate for a date (left out).
    pub no_rate: Vec<String>,
    /// SGB holdings whose tranche StockWise does not know (no coupons).
    pub unknown_sgbs: Vec<String>,
    /// Foreign dividends whose tax withheld abroad is not known.
    pub withholding_unknown: Vec<String>,
}

impl IncomeReport {
    /// Some income could not be counted: the totals are incomplete.
    pub fn is_incomplete(&self) -> bool {
        !self.no_rate.is_empty() || !self.unknown_sgbs.is_empty()
    }
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
/// `fy_start`. `fx(currency, date)` gives rupees per unit of a (major)
/// currency on a day; a dividend whose rate is unknown is left out and
/// named. `withholding` is the rate of tax withheld abroad on foreign
/// dividends, when known from a statement (US 25% with a W-8BEN, 30%
/// without); `None` leaves it unknown.
pub fn income_report(
    portfolio: &Portfolio,
    fy_start: NaiveDate,
    withholding: Option<f64>,
    fx: impl Fn(&str, NaiveDate) -> Option<f64>,
) -> IncomeReport {
    use crate::tax::paise;
    let (from, to) = crate::tax::Country::India.tax_year(fy_start);
    let y = from.year();
    let threshold = dividend_tds_threshold(y);
    let mut rows: Vec<IncomeRow> = Vec::new();
    let mut no_rate = Vec::new();
    let mut withholding_unknown = Vec::new();

    let divs: Vec<(&DividendReceived, NaiveDate)> = portfolio
        .dividends
        .iter()
        .filter_map(|d| Some((d, date(&d.ex_date)?)))
        .filter(|(_, d)| *d >= from && *d <= to)
        .collect();
    for (d, day) in divs {
        let listed = d
            .currency
            .clone()
            .or_else(|| crate::portfolio::symbol_currency(&d.symbol).map(str::to_string))
            .unwrap_or_else(|| "INR".into());
        // A minor unit (`GBp`) becomes its major one, with the amount.
        let (amount, currency) = crate::market::to_major(d.amount, &listed);
        let currency = currency.to_uppercase();
        let domestic = currency == "INR";
        let Some(rate) = (if domestic {
            Some(1.0)
        } else {
            fx(&currency, day)
        }) else {
            no_rate.push(d.symbol.clone());
            continue;
        };
        let gross = paise::from_rupees(amount * rate);
        let (kind, foreign_tax) = if domestic {
            (IncomeKind::Dividend, Some(0))
        } else {
            match withholding {
                Some(w) => (IncomeKind::ForeignDividend, Some(paise::times(gross, w))),
                None => {
                    withholding_unknown.push(d.symbol.clone());
                    (IncomeKind::ForeignDividend, None)
                }
            }
        };
        rows.push(IncomeRow {
            date: day,
            date_basis: "ex_date",
            payer: payer(&d.symbol),
            kind,
            quarter: quarter_of(day),
            currency,
            gross_original: paise::from_rupees(amount),
            fx_rate: rate,
            gross,
            tds: 0,
            foreign_tax,
            net: gross - foreign_tax.unwrap_or(0),
        });
    }
    // TDS on a payer's year: once its dividends to you pass the threshold,
    // Sec. 194/194K applies to the aggregate (10% of each of its dividends).
    let mut payers: Vec<String> = rows
        .iter()
        .filter(|r| r.kind == IncomeKind::Dividend)
        .map(|r| r.payer.clone())
        .collect();
    payers.sort();
    payers.dedup();
    for who in payers {
        let total: i64 = rows
            .iter()
            .filter(|r| r.kind == IncomeKind::Dividend && r.payer == who)
            .map(|r| r.gross)
            .sum();
        if paise::to_rupees(total) > threshold {
            for r in rows
                .iter_mut()
                .filter(|r| r.kind == IncomeKind::Dividend && r.payer == who)
            {
                r.tds = paise::times(r.gross, DIVIDEND_TDS_RATE);
                r.net = r.gross - r.tds;
            }
        }
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
            let gross = paise::from_rupees(units * per_unit);
            rows.push(IncomeRow {
                date: day,
                date_basis: "coupon_date",
                payer: t.symbol.to_string(),
                kind: IncomeKind::SgbInterest,
                quarter: quarter_of(day),
                currency: "INR".into(),
                gross_original: gross,
                fx_rate: 1.0,
                gross,
                tds: 0,
                foreign_tax: Some(0),
                net: gross,
            });
        }
    }
    rows.sort_by(|a, b| (a.date, &a.payer).cmp(&(b.date, &b.payer)));

    let sum = |k: Option<IncomeKind>, f: fn(&IncomeRow) -> i64| -> i64 {
        rows.iter()
            .filter(|r| k.is_none_or(|k| r.kind == k))
            .map(f)
            .sum()
    };
    let mut dividend_by_quarter = [0i64; 5];
    let mut income_by_quarter = [0i64; 5];
    for r in &rows {
        income_by_quarter[r.quarter] += r.gross;
        if r.kind != IncomeKind::SgbInterest {
            dividend_by_quarter[r.quarter] += r.gross;
        }
    }
    for v in [&mut no_rate, &mut withholding_unknown] {
        v.sort();
        v.dedup();
    }
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
        foreign_tax: sum(None, |r| r.foreign_tax.unwrap_or(0)),
        net: sum(None, |r| r.net),
        dividend_by_quarter,
        income_by_quarter,
        rows,
        no_rate,
        unknown_sgbs,
        withholding_unknown,
    }
}

/// The income as advance tax sees it: every row at the slab `rate`;
/// dividends with the Sec. 234C proviso, interest without.
pub fn advance_income(report: &IncomeReport, rate: f64) -> Vec<crate::tax::AdvanceIncome> {
    report
        .rows
        .iter()
        .map(|r| crate::tax::AdvanceIncome {
            date: r.date,
            amount: r.gross,
            rate,
            deferrable: r.kind != IncomeKind::SgbInterest,
        })
        .collect()
}

/// Schedule OS as CSV: one row per dividend or coupon, then the dividend
/// quarterly breakup ITR asks for (the Sec. 234C periods), then any income
/// left out, so an incomplete file says so.
pub fn schedule_os_csv(r: &IncomeReport) -> String {
    use crate::output::csv_line;
    let money = |x: i64| format!("{:.2}", crate::tax::paise::to_rupees(x) + 0.0);
    let mut out = csv_line(&[
        "date",
        "date_basis",
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
            row.date_basis.to_string(),
            row.payer.clone(),
            row.kind.label().to_string(),
            QUARTER_LABELS[row.quarter].to_string(),
            row.currency.clone(),
            money(row.gross_original),
            format!("{:.4}", row.fx_rate),
            money(row.gross),
            money(row.tds),
            row.foreign_tax
                .map(money)
                .unwrap_or_else(|| "unknown".into()),
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
    let omitted: Vec<(&str, &Vec<String>)> = vec![
        ("omitted_no_rupee_rate", &r.no_rate),
        ("omitted_unknown_sgb_tranche", &r.unknown_sgbs),
        ("foreign_withholding_unknown", &r.withholding_unknown),
    ];
    if omitted.iter().any(|(_, v)| !v.is_empty()) {
        out.push('\n');
        out.push_str(&csv_line(&["incomplete", "payers"]));
        for (label, v) in omitted.into_iter().filter(|(_, v)| !v.is_empty()) {
            out.push_str(&csv_line(&[label.to_string(), v.join(" ")]));
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
    fn tds_applies_to_a_payers_whole_year_once_past_the_threshold() {
        assert_eq!(dividend_tds_threshold(2024), 5_000.0);
        assert_eq!(dividend_tds_threshold(2026), 10_000.0);
        let p = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [
                div("ITC.NS", "2026-05-28", 6_000.0, "INR"),
                // 6,000 + 5,000 = 11,000 > 10,000: 10% of the whole ₹11,000.
                div("ITC.NS", "2026-11-20", 5_000.0, "INR"),
                // Another company: its own threshold.
                div("TCS.NS", "2026-07-10", 9_000.0, "INR"),
                // Last year's: left out.
                div("TCS.NS", "2026-03-10", 50_000.0, "INR"),
            ],
        }));
        let r = income_report(&p, d("2026-04-01"), None, |_, _| None);
        assert_eq!(r.rows.len(), 3);
        let itc: Vec<&IncomeRow> = r.rows.iter().filter(|r| r.payer == "ITC").collect();
        assert_eq!(itc[0].tds, 600_00);
        assert_eq!(itc[1].tds, 500_00);
        assert_eq!(itc[1].net, 4_500_00);
        assert!(r.rows.iter().all(|r| r.payer != "TCS" || r.tds == 0));
        assert_eq!(r.dividends, 20_000_00);
        assert_eq!(r.tds, 1_100_00);
        assert_eq!(r.dividend_by_quarter, [6_000_00, 9_000_00, 5_000_00, 0, 0]);
        assert_eq!(r.rows[0].date_basis, "ex_date");
        // FY 2024-25: the old ₹5,000 threshold.
        let old = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [div("ITC.NS", "2024-05-28", 6_000.0, "INR")],
        }));
        let r = income_report(&old, d("2024-04-01"), None, |_, _| None);
        assert_eq!(r.tds, 600_00);
    }

    #[test]
    fn foreign_dividends_are_converted_and_withholding_is_only_what_was_given() {
        let p = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [div("AAPL", "2026-08-14", 100.0, "USD"),
                          div("NESN.SW", "2026-08-14", 50.0, "CHF"),
                          // 250 pence = £2.50.
                          div("ULVR.L", "2026-09-01", 250.0, "GBp")],
        }));
        let fx = |c: &str, _| match c {
            "USD" => Some(88.0),
            "GBP" => Some(110.0),
            _ => None,
        };
        let r = income_report(&p, d("2026-04-01"), None, fx);
        assert_eq!(r.rows.len(), 2);
        let aapl = &r.rows[0];
        assert_eq!(aapl.kind, IncomeKind::ForeignDividend);
        assert_eq!(aapl.gross, 8_800_00);
        // No rate given: the tax withheld is unknown, not assumed.
        assert_eq!(aapl.foreign_tax, None);
        assert_eq!(aapl.net, 8_800_00);
        assert_eq!(aapl.tds, 0);
        let ulvr = &r.rows[1];
        assert_eq!((ulvr.currency.as_str(), ulvr.gross), ("GBP", 275_00));
        assert_eq!(r.no_rate, vec!["NESN.SW".to_string()]);
        assert_eq!(r.withholding_unknown.len(), 2);
        assert!(r.is_incomplete());
        // With the 25% treaty rate from a statement.
        let r = income_report(&p, d("2026-04-01"), Some(0.25), fx);
        assert_eq!(r.rows[0].foreign_tax, Some(2_200_00));
        assert_eq!(r.rows[0].net, 6_600_00);
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
        let r = income_report(&p, d("2026-04-01"), None, |_, _| None);
        let coupons: Vec<&IncomeRow> = r
            .rows
            .iter()
            .filter(|r| r.kind == IncomeKind::SgbInterest)
            .collect();
        assert_eq!(coupons.len(), 2);
        // May: 14 units (4 sold later); Nov: 10. ₹39.7875 a unit.
        assert!(
            (coupons[0].gross - 557_02).abs() <= 1,
            "{}",
            coupons[0].gross
        );
        assert!(
            (coupons[1].gross - 397_88).abs() <= 1,
            "{}",
            coupons[1].gross
        );
        assert!(coupons.iter().all(|c| c.tds == 0));
        assert_eq!((coupons[0].quarter, coupons[1].quarter), (0, 2));
        // Interest has no Sec. 234C proviso; dividends do.
        let adv = advance_income(&r, 0.3);
        assert!(adv.iter().all(|a| !a.deferrable));
    }

    #[test]
    fn schedule_os_csv_has_rows_the_quarterly_breakup_and_omissions() {
        let p = portfolio(serde_json::json!({
            "holdings": [],
            "dividends": [div("ITC.NS", "2026-05-28", 6_000.0, "INR"),
                          div("NESN.SW", "2026-08-14", 50.0, "CHF")],
        }));
        let r = income_report(&p, d("2026-04-01"), None, |_, _| None);
        let csv = schedule_os_csv(&r);
        assert!(csv.starts_with("date,date_basis,payer,nature,itr_period,"));
        assert!(csv.contains("2026-05-28,ex_date,ITC,Dividend,Up to 15 Jun,INR,6000.00"));
        assert!(csv.contains("dividend_income,6000.00,0.00,0.00,0.00,0.00,6000.00"));
        assert!(csv.contains("omitted_no_rupee_rate,NESN.SW"));
    }
}
