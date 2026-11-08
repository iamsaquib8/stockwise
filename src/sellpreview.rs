//! Pre-sell preview: which FIFO lots would depart, the capital-gains tax the
//! sale adds, and which lots are close to turning long-term. Pure functions
//! over a [`Holding`]; nothing here reads or writes `portfolio.json`.
//!
//! The numbers are tax facts and arithmetic, not advice: the preview prices
//! every lot at the sale price given and says what the tax would be.

use crate::portfolio::{Holding, Lot, Portfolio};
use crate::tax::{self, Buckets, Country};
use crate::tax_export;
use chrono::{Days, Months, NaiveDate};
use serde::Serialize;

const EPS: f64 = 1e-9;

/// A lot closer than this many days to turning long-term gets a countdown.
pub const COUNTDOWN_DAYS: i64 = 45;

/// Days an equity fund unit is usually held to avoid an exit load. Schemes
/// differ (some 15 or 30 days); the factsheet has the real figure.
pub const EQUITY_EXIT_LOAD_DAYS: i64 = 365;
/// The typical equity-fund exit load, percent of the redemption value.
pub const EQUITY_EXIT_LOAD_PCT: f64 = 1.0;

/// Exit-load terms for a fund being redeemed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExitLoadTerms {
    pub days: i64,
    pub pct: f64,
}

impl ExitLoadTerms {
    /// The usual equity-fund terms (1% under 365 days).
    pub fn equity_default() -> Self {
        Self {
            days: EQUITY_EXIT_LOAD_DAYS,
            pct: EQUITY_EXIT_LOAD_PCT,
        }
    }
}

/// How the symbol is taxed, as far as the preview can tell.
#[derive(Debug, Clone, PartialEq)]
pub enum Rules {
    /// Listed-share rules (Sec. 111A/112A in India; the country's own
    /// short/long rates elsewhere): the preview computes the tax.
    Listed,
    /// Taxed under other rules; the preview shows lots and gains only.
    Other(String),
}

/// What the preview needs to know about the investor's tax position.
#[derive(Debug, Clone)]
pub struct TaxContext {
    pub country: Country,
    pub rules: Rules,
    /// Gains and losses already realized this tax year, in the tax currency.
    pub realized: Buckets,
    /// Scale from the quote currency to the tax currency.
    pub scale: f64,
    /// India: the symbol's 31 Jan 2018 price, for Sec. 112A grandfathering.
    pub fmv_2018: Option<f64>,
    /// Set for mutual funds: the exit-load window to check.
    pub exit_load: Option<ExitLoadTerms>,
}

impl TaxContext {
    pub fn listed(country: Country, realized: Buckets) -> Self {
        Self {
            country,
            rules: Rules::Listed,
            realized,
            scale: 1.0,
            fmv_2018: None,
            exit_load: None,
        }
    }
}

/// One slice of a lot that would be sold.
#[derive(Debug, Clone, Serialize)]
pub struct LotLine {
    /// The lot's number in `portfolio lots` (1-based, oldest first).
    pub lot_no: usize,
    pub bought: String,
    pub date_estimated: bool,
    pub days_held: i64,
    pub qty: f64,
    pub unit_cost: f64,
    pub cost: f64,
    pub proceeds: f64,
    pub gain: f64,
    pub long_term: Option<bool>,
    /// India: the gain uses the 31 Jan 2018 price as cost (Sec. 112A).
    pub grandfathered: bool,
    /// Exit load on this slice, in the quote currency.
    pub exit_load: f64,
}

/// A lot in the preview that is about to turn long-term, with the tax that
/// waiting would save.
#[derive(Debug, Clone, Serialize)]
pub struct CountdownLine {
    pub lot_no: usize,
    pub qty: f64,
    pub bought: String,
    pub days_held: i64,
    pub days_to_long_term: i64,
    pub long_term_on: String,
    /// Tax now minus the tax if this lot were already long-term, at the same
    /// price, in the tax currency.
    pub tax_saved: f64,
}

/// The tax a sale adds on top of what is already realized this year.
#[derive(Debug, Clone, Copy, Default, Serialize)]
pub struct SaleTax {
    pub st_gain: f64,
    pub lt_gain: f64,
    pub st_tax: f64,
    pub lt_tax: f64,
    pub cess: f64,
    pub total: f64,
    /// India: Sec. 112A exemption still unused before this sale.
    pub exemption_left_before: f64,
    /// The part of it this sale uses.
    pub exemption_used: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SellPreview {
    pub symbol: String,
    pub qty: f64,
    pub price: f64,
    pub date: String,
    pub lines: Vec<LotLine>,
    pub proceeds: f64,
    pub cost: f64,
    pub gain: f64,
    /// None when the symbol is taxed under rules the preview does not model
    /// (`other_rules` says which) or the country has no estimate.
    pub tax: Option<SaleTax>,
    pub other_rules: Option<String>,
    pub countdown: Vec<CountdownLine>,
    pub exit_load_total: f64,
    pub exit_load_pct: Option<f64>,
    pub exit_load_days: Option<i64>,
}

/// The first day a lot bought on `bought` is long-term: the day after the
/// same calendar date `months` later (held for *more than* `months`).
pub fn long_term_from(bought: NaiveDate, months: u32) -> Option<NaiveDate> {
    bought
        .checked_add_months(Months::new(months))?
        .checked_add_days(Days::new(1))
}

fn parse(d: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

/// The lots a FIFO sale of `qty` on `date` takes, as `(lot number, lot,
/// quantity taken)`. Mirrors `Portfolio::sell_fifo`: oldest first, only lots
/// bought on or before `date`.
pub fn fifo_slices(
    holding: &Holding,
    qty: f64,
    date: &str,
) -> Result<Vec<(usize, Lot, f64)>, String> {
    if qty <= EPS || !qty.is_finite() {
        return Err("Quantity to sell must be positive".into());
    }
    let eligible: f64 = holding
        .lots
        .iter()
        .filter(|l| l.date.as_str() <= date)
        .map(|l| l.qty)
        .sum();
    if qty > eligible + EPS {
        return Err(if eligible + EPS < holding.shares {
            format!(
                "Cannot sell {qty} {}: only {eligible} were bought by {date}",
                holding.symbol
            )
        } else {
            format!(
                "Cannot sell {qty} {}: you hold {}",
                holding.symbol, holding.shares
            )
        });
    }
    let mut remaining = qty;
    let mut out = Vec::new();
    for (i, lot) in holding.lots.iter().enumerate() {
        if remaining <= EPS {
            break;
        }
        if lot.date.as_str() > date {
            continue;
        }
        let take = if remaining >= lot.qty - EPS {
            lot.qty
        } else {
            remaining
        };
        out.push((i + 1, lot.clone(), take));
        remaining -= take;
    }
    Ok(out)
}

/// Preview selling `qty` of `holding` at `price` on `date`.
pub fn preview_sell(
    holding: &Holding,
    qty: f64,
    price: f64,
    date: NaiveDate,
    ctx: &TaxContext,
) -> Result<SellPreview, String> {
    if price < 0.0 || !price.is_finite() {
        return Err("Sale price can't be negative".into());
    }
    let date_s = date.to_string();
    let slices = fifo_slices(holding, qty, &date_s)?;
    let country = ctx.country;
    let months = country.long_term_months();

    struct Row {
        line: LotLine,
        bought: Option<NaiveDate>,
    }
    let mut rows = Vec::new();
    for (lot_no, lot, take) in &slices {
        let bought = parse(&lot.date);
        let long = bought.map(|b| country.is_long_term(b, date));
        let is_long = long.unwrap_or(false);
        let fmv = if country == Country::India {
            ctx.fmv_2018
        } else {
            None
        };
        let unit_cost = tax_export::lot_unit_cost(&lot.date, is_long, lot.price, price, fmv);
        let days_held = bought.map_or(0, |b| (date - b).num_days().max(0));
        let proceeds = take * price;
        let exit_load = match ctx.exit_load {
            Some(t) if days_held < t.days => proceeds * t.pct / 100.0,
            _ => 0.0,
        };
        rows.push(Row {
            line: LotLine {
                lot_no: *lot_no,
                bought: lot.date.clone(),
                date_estimated: lot.date_estimated,
                days_held,
                qty: *take,
                unit_cost,
                cost: take * unit_cost,
                proceeds,
                gain: take * (price - unit_cost),
                long_term: months.map(|_| is_long),
                grandfathered: fmv.is_some() && unit_cost != lot.price,
                exit_load,
            },
            bought,
        });
    }

    let buckets_of = |force_long: Option<usize>| {
        let mut b = Buckets::default();
        for r in &rows {
            let long = r.line.long_term.unwrap_or(false) || force_long == Some(r.line.lot_no);
            b.add(r.line.gain * ctx.scale, long);
        }
        b
    };
    let total_of =
        |sale: Buckets| -> tax::Estimate { tax::estimate(country, ctx.realized.plus(sale)) };

    let sale_buckets = buckets_of(None);
    let tax = (ctx.rules == Rules::Listed).then(|| {
        let before = tax::estimate(country, ctx.realized);
        let after = total_of(sale_buckets);
        SaleTax {
            st_gain: sale_buckets.st_gain - sale_buckets.st_loss,
            lt_gain: sale_buckets.lt_gain - sale_buckets.lt_loss,
            st_tax: after.st_tax - before.st_tax,
            lt_tax: after.lt_tax - before.lt_tax,
            cess: after.cess - before.cess,
            total: after.total() - before.total(),
            exemption_left_before: (country.exemption() - before.exemption_used).max(0.0),
            exemption_used: after.exemption_used - before.exemption_used,
        }
    });

    let mut countdown = Vec::new();
    if tax.is_some()
        && let Some(m) = months
    {
        let now_total = total_of(sale_buckets).total();
        for r in &rows {
            let (Some(b), Some(false)) = (r.bought, r.line.long_term) else {
                continue;
            };
            if r.line.date_estimated || r.line.gain <= 0.0 {
                continue;
            }
            let Some(on) = long_term_from(b, m) else {
                continue;
            };
            let days_to = (on - date).num_days();
            if days_to > COUNTDOWN_DAYS {
                continue;
            }
            let saved = now_total - total_of(buckets_of(Some(r.line.lot_no))).total();
            if saved > 0.005 {
                countdown.push(CountdownLine {
                    lot_no: r.line.lot_no,
                    qty: r.line.qty,
                    bought: r.line.bought.clone(),
                    days_held: r.line.days_held,
                    days_to_long_term: days_to,
                    long_term_on: on.to_string(),
                    tax_saved: saved,
                });
            }
        }
    }

    let lines: Vec<LotLine> = rows.into_iter().map(|r| r.line).collect();
    Ok(SellPreview {
        symbol: holding.symbol.clone(),
        qty,
        price,
        date: date_s,
        proceeds: lines.iter().map(|l| l.proceeds).sum(),
        cost: lines.iter().map(|l| l.cost).sum(),
        gain: lines.iter().map(|l| l.gain).sum(),
        exit_load_total: lines.iter().map(|l| l.exit_load).sum(),
        exit_load_pct: ctx.exit_load.map(|t| t.pct),
        exit_load_days: ctx.exit_load.map(|t| t.days),
        lines,
        tax,
        other_rules: match &ctx.rules {
            Rules::Other(r) => Some(r.clone()),
            Rules::Listed => None,
        },
        countdown,
    })
}

/// A lot that turns long-term soon.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LotCountdown {
    pub symbol: String,
    /// The lot's number in `portfolio lots` (1-based, oldest first).
    pub lot_no: usize,
    pub qty: f64,
    pub bought: String,
    pub days_held: i64,
    /// Days from `today` until the first long-term day.
    pub days_to_long_term: i64,
    /// The first day a sale is long-term.
    pub long_term_on: String,
}

/// Whether a symbol's holding period follows the listed-share rule for
/// `country` (so a countdown is meaningful without a price or fund category).
pub fn counts_for_countdown(country: Country, symbol: &str) -> bool {
    use crate::offmarket::{IndiaClass, india_class};
    if crate::portfolio::symbol_currency(symbol)
        .and_then(|c| country.currency_scale(c))
        .is_none()
    {
        return false;
    }
    match country {
        Country::India => india_class(symbol, None) == IndiaClass::Listed,
        _ => !crate::offmarket::is_off_market(symbol),
    }
}

/// Every open lot in `portfolio` within `within_days` of turning long-term
/// on `today`, nearest first. Lots whose purchase date is only an estimate
/// are skipped (their countdown would be made up), as are funds and anything
/// else whose rules need a price or AMFI category; see
/// [`ltcg_countdown_with`] to widen the filter. Countries without a holding
/// period (UK) give nothing. Pure: this is what the background worker calls
/// after market close.
pub fn ltcg_countdown(
    portfolio: &Portfolio,
    country: Country,
    today: NaiveDate,
    within_days: i64,
) -> Vec<LotCountdown> {
    ltcg_countdown_with(portfolio, country, today, within_days, |s| {
        counts_for_countdown(country, s)
    })
}

/// [`ltcg_countdown`] for the symbols `include` accepts.
pub fn ltcg_countdown_with(
    portfolio: &Portfolio,
    country: Country,
    today: NaiveDate,
    within_days: i64,
    include: impl Fn(&str) -> bool,
) -> Vec<LotCountdown> {
    let Some(months) = country.long_term_months() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for h in &portfolio.holdings {
        if !include(&h.symbol) {
            continue;
        }
        for (i, l) in h.lots.iter().enumerate() {
            if l.date_estimated {
                continue;
            }
            let Some(b) = parse(&l.date) else { continue };
            if country.is_long_term(b, today) {
                continue;
            }
            let Some(on) = long_term_from(b, months) else {
                continue;
            };
            let days_to = (on - today).num_days();
            if days_to < 0 || days_to > within_days {
                continue;
            }
            out.push(LotCountdown {
                symbol: h.symbol.clone(),
                lot_no: i + 1,
                qty: l.qty,
                bought: l.date.clone(),
                days_held: (today - b).num_days().max(0),
                days_to_long_term: days_to,
                long_term_on: on.to_string(),
            });
        }
    }
    out.sort_by(|a, b| {
        (a.days_to_long_term, &a.symbol, a.lot_no).cmp(&(b.days_to_long_term, &b.symbol, b.lot_no))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn holding(symbol: &str, lots: &[(f64, f64, &str)]) -> Portfolio {
        let mut p = Portfolio::default();
        for (q, pr, date) in lots {
            p.add_lot(symbol, Lot::new(*q, *pr, *date, "manual"));
        }
        p
    }

    /// The issue's example: 25 shares bought 2025-12-20, gain of ₹2,400 a
    /// share, ₹1.25 lakh exemption already used up. 20% → 12.5% on ₹60,000
    /// with 4% cess saves ₹4,680.
    #[test]
    fn issue_example_saves_4680() {
        let p = holding("TCS.NS", &[(25.0, 1000.0, "2025-12-20")]);
        let h = p.holding("TCS.NS").unwrap();
        let used_up = Buckets {
            lt_gain: 125_000.0,
            ..Buckets::default()
        };
        let ctx = TaxContext::listed(Country::India, used_up);
        let pv = preview_sell(h, 25.0, 3400.0, d("2026-12-03"), &ctx).unwrap();
        assert_eq!(pv.lines.len(), 1);
        assert_eq!(pv.lines[0].days_held, 348);
        assert_eq!(pv.lines[0].long_term, Some(false));
        assert!((pv.gain - 60_000.0).abs() < 1e-6);
        let t = pv.tax.unwrap();
        assert!((t.st_tax - 12_000.0).abs() < 1e-6);
        assert!((t.cess - 480.0).abs() < 1e-6);
        assert!((t.total - 12_480.0).abs() < 1e-6);
        assert_eq!(pv.countdown.len(), 1);
        let c = &pv.countdown[0];
        // The lot is long-term after 2026-12-20 (held *more than* 12 months).
        assert_eq!(c.long_term_on, "2026-12-21");
        assert_eq!(c.days_to_long_term, 18);
        assert!((c.tax_saved - 4_680.0).abs() < 1e-6, "{}", c.tax_saved);
    }

    #[test]
    fn exemption_remaining_covers_waiting_gain() {
        // Fresh year: waiting makes ₹60,000 LTCG fully exempt, saving all
        // ₹12,480 of STCG tax.
        let p = holding("TCS.NS", &[(25.0, 1000.0, "2025-12-20")]);
        let ctx = TaxContext::listed(Country::India, Buckets::default());
        let pv = preview_sell(
            p.holding("TCS.NS").unwrap(),
            25.0,
            3400.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert!((pv.countdown[0].tax_saved - 12_480.0).abs() < 1e-6);
        assert!((pv.tax.unwrap().exemption_left_before - 125_000.0).abs() < 1e-6);
    }

    #[test]
    fn long_term_lot_uses_exemption_and_lt_rate() {
        let p = holding("INFY.NS", &[(100.0, 1000.0, "2024-01-10")]);
        let ctx = TaxContext::listed(Country::India, Buckets::default());
        // gain 100 × 1,500 = 1,50,000; 1,25,000 exempt, 25,000 at 12.5% = 3,125; cess 125
        let pv = preview_sell(
            p.holding("INFY.NS").unwrap(),
            100.0,
            2500.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        let t = pv.tax.unwrap();
        assert_eq!(pv.lines[0].long_term, Some(true));
        assert!((t.exemption_used - 125_000.0).abs() < 1e-6);
        assert!((t.lt_tax - 3_125.0).abs() < 1e-6);
        assert!((t.cess - 125.0).abs() < 1e-6);
        assert!((t.total - 3_250.0).abs() < 1e-6);
        assert!(pv.countdown.is_empty());
    }

    #[test]
    fn fifo_splits_lots_oldest_first() {
        let p = holding(
            "X.NS",
            &[(10.0, 100.0, "2024-01-01"), (10.0, 200.0, "2026-06-01")],
        );
        let ctx = TaxContext::listed(Country::India, Buckets::default());
        let pv = preview_sell(
            p.holding("X.NS").unwrap(),
            15.0,
            300.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert_eq!(pv.lines.len(), 2);
        assert_eq!((pv.lines[0].lot_no, pv.lines[0].qty), (1, 10.0));
        assert_eq!((pv.lines[1].lot_no, pv.lines[1].qty), (2, 5.0));
        assert_eq!(pv.lines[0].long_term, Some(true));
        assert_eq!(pv.lines[1].long_term, Some(false));
        let t = pv.tax.unwrap();
        assert!((t.lt_gain - 2000.0).abs() < 1e-6);
        assert!((t.st_gain - 500.0).abs() < 1e-6);
    }

    #[test]
    fn preview_never_mutates() {
        let p = holding("X.NS", &[(10.0, 100.0, "2024-01-01")]);
        let before = serde_json::to_string(&p).unwrap();
        let ctx = TaxContext::listed(Country::India, Buckets::default());
        preview_sell(
            p.holding("X.NS").unwrap(),
            4.0,
            150.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert_eq!(serde_json::to_string(&p).unwrap(), before);
        assert!(p.realized.is_empty());
    }

    #[test]
    fn oversell_and_bad_input_refused() {
        let p = holding("X.NS", &[(10.0, 100.0, "2024-01-01")]);
        let h = p.holding("X.NS").unwrap();
        let ctx = TaxContext::listed(Country::India, Buckets::default());
        assert!(preview_sell(h, 11.0, 150.0, d("2026-12-03"), &ctx).is_err());
        assert!(preview_sell(h, 0.0, 150.0, d("2026-12-03"), &ctx).is_err());
        assert!(preview_sell(h, 1.0, -1.0, d("2026-12-03"), &ctx).is_err());
    }

    #[test]
    fn loss_lot_gets_no_countdown() {
        let p = holding("X.NS", &[(10.0, 100.0, "2025-12-20")]);
        let ctx = TaxContext::listed(Country::India, Buckets::default());
        let pv = preview_sell(
            p.holding("X.NS").unwrap(),
            10.0,
            80.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert!(pv.countdown.is_empty());
    }

    #[test]
    fn grandfathered_cost_applies_to_old_long_term_lot() {
        let p = holding("OLD.NS", &[(10.0, 100.0, "2017-05-01")]);
        let mut ctx = TaxContext::listed(Country::India, Buckets::default());
        ctx.fmv_2018 = Some(150.0);
        let pv = preview_sell(
            p.holding("OLD.NS").unwrap(),
            10.0,
            200.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert!(pv.lines[0].grandfathered);
        assert!((pv.lines[0].unit_cost - 150.0).abs() < 1e-9);
        assert!((pv.gain - 500.0).abs() < 1e-6);
    }

    #[test]
    fn mutual_fund_exit_load_window() {
        let p = holding(
            "MF:120503",
            &[(100.0, 50.0, "2026-06-01"), (100.0, 40.0, "2024-01-01")],
        );
        let mut ctx = TaxContext::listed(Country::India, Buckets::default());
        ctx.exit_load = Some(ExitLoadTerms::equity_default());
        // FIFO: the 2024 lot goes first and is out of the window.
        let pv = preview_sell(
            p.holding("MF:120503").unwrap(),
            150.0,
            60.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert_eq!(pv.lines[0].exit_load, 0.0);
        // 50 units × 60 × 1% = 30
        assert!((pv.exit_load_total - 30.0).abs() < 1e-6);
    }

    #[test]
    fn other_rules_skip_tax() {
        let p = holding("GOLDBEES.NS", &[(10.0, 100.0, "2025-01-01")]);
        let mut ctx = TaxContext::listed(Country::India, Buckets::default());
        ctx.rules = Rules::Other("gold ETF".into());
        let pv = preview_sell(
            p.holding("GOLDBEES.NS").unwrap(),
            5.0,
            120.0,
            d("2026-12-03"),
            &ctx,
        )
        .unwrap();
        assert!(pv.tax.is_none());
        assert_eq!(pv.other_rules.as_deref(), Some("gold ETF"));
    }

    #[test]
    fn countdown_window_and_filters() {
        let mut p = holding(
            "TCS.NS",
            &[
                (5.0, 100.0, "2025-12-20"), // 348 days on 2026-12-03 → 18 days left
                (5.0, 100.0, "2026-03-02"), // far from long-term
                (5.0, 100.0, "2025-12-03"), // long-term on 2026-12-04: 1 day left
                (5.0, 100.0, "2024-01-01"), // already long-term
            ],
        );
        p.add_lot("MF:120503", Lot::new(5.0, 10.0, "2025-12-10", "manual"));
        p.add_lot("GOLDBEES.NS", Lot::new(5.0, 10.0, "2025-12-10", "manual"));
        let mut est = Lot::new(5.0, 10.0, "2025-12-10", "manual");
        est.date_estimated = true;
        p.add_lot("EST.NS", est);
        let out = ltcg_countdown(&p, Country::India, d("2026-12-03"), 45);
        let got: Vec<(&str, usize, i64)> = out
            .iter()
            .map(|c| (c.symbol.as_str(), c.lot_no, c.days_to_long_term))
            .collect();
        assert_eq!(got, vec![("TCS.NS", 2, 1), ("TCS.NS", 3, 18)]);
        assert_eq!(out[0].long_term_on, "2026-12-04");
        // A tighter window drops the 18-day lot.
        assert_eq!(
            ltcg_countdown(&p, Country::India, d("2026-12-03"), 10).len(),
            1
        );
        // UK has no holding period.
        assert!(ltcg_countdown(&p, Country::Uk, d("2026-12-03"), 45).is_empty());
    }

    #[test]
    fn countdown_on_threshold_day_is_still_short_term() {
        // Held exactly 12 months is not "more than" 12 months.
        let p = holding("TCS.NS", &[(5.0, 100.0, "2025-12-20")]);
        let out = ltcg_countdown(&p, Country::India, d("2026-12-20"), 45);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].days_to_long_term, 1);
        assert!(ltcg_countdown(&p, Country::India, d("2026-12-21"), 45).is_empty());
    }
}
