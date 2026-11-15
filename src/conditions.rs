//! `conditions`: where a market stands on trend, volatility, breadth and
//! valuation. Every figure is a reading of the data (the index against
//! its averages, the share of stocks above theirs, earnings yield against
//! the bond yield), some with the share of past days that were lower.
//!
//! The readings describe the market. They are not a signal to invest or
//! to wait, and nothing here forecasts returns.

use crate::api::Quote;
use serde::Serialize;

/// Sessions in a year, for the one-year return and 52-week high.
const YEAR: usize = 252;

/// The benchmark index against its own history.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Trend {
    pub last: f64,
    /// Percent above (+) or below (-) the 200-day average.
    pub vs_200_day_pct: Option<f64>,
    /// Share of days in the history when that distance was lower, 0-100.
    pub vs_200_day_percentile: Option<f64>,
    /// Percent above or below the 50-day average.
    pub vs_50_day_pct: Option<f64>,
    /// Percent below the highest close of the last 252 sessions (0 at it).
    pub from_52_week_high_pct: f64,
    /// Percent below the highest close of the whole history.
    pub from_peak_pct: f64,
    /// Change over the last 252 sessions, percent.
    pub return_1y_pct: Option<f64>,
    /// Years of daily closes the percentile looks back over.
    pub history_years: f64,
}

/// The volatility index (India VIX, VIX) against its own history.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Volatility {
    pub name: String,
    pub last: f64,
    /// Share of days in the history when it was lower, 0-100.
    pub percentile: f64,
    pub history_years: f64,
}

/// How many of the scanned stocks share the index's direction.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Breadth {
    /// Stocks with the figures.
    pub stocks: usize,
    pub above_200_day_pct: Option<f64>,
    pub above_50_day_pct: Option<f64>,
    /// Within 5% of the 52-week high.
    pub near_52_week_high_pct: Option<f64>,
    /// Within 5% of the 52-week low.
    pub near_52_week_low_pct: Option<f64>,
    /// Up on the day.
    pub advancing_pct: Option<f64>,
}

/// What the scanned stocks cost against their earnings and against bonds.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Valuation {
    /// Profitable stocks with a market cap behind the aggregate.
    pub stocks: usize,
    /// Total market cap over total trailing earnings of those stocks.
    pub aggregate_pe: Option<f64>,
    /// Middle trailing P/E of the profitable stocks.
    pub median_pe: Option<f64>,
    /// 100 / aggregate P/E.
    pub earnings_yield_pct: Option<f64>,
    /// The 10-year government bond yield, percent.
    pub bond_yield_pct: Option<f64>,
    /// Earnings yield minus the bond yield, percentage points.
    pub earnings_minus_bond_pct: Option<f64>,
    /// Middle dividend yield of the stocks that pay one, percent.
    pub median_dividend_yield_pct: Option<f64>,
}

/// Everything `conditions` reports.
#[derive(Debug, Clone, Serialize)]
pub struct Report {
    pub market: String,
    pub index: String,
    pub trend: Option<Trend>,
    pub volatility: Option<Volatility>,
    pub breadth: Breadth,
    pub valuation: Valuation,
    /// Stocks the breadth and valuation figures cover.
    pub scanned: usize,
}

fn mean(xs: &[f64]) -> Option<f64> {
    (!xs.is_empty()).then(|| xs.iter().sum::<f64>() / xs.len() as f64)
}

fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(f64::total_cmp);
    let mid = xs.len() / 2;
    Some(if xs.len().is_multiple_of(2) {
        (xs[mid - 1] + xs[mid]) / 2.0
    } else {
        xs[mid]
    })
}

/// Share (0-100) of `series` that is below `value`.
fn share_below(series: &[f64], value: f64) -> f64 {
    if series.is_empty() {
        return 50.0;
    }
    series.iter().filter(|x| **x < value).count() as f64 / series.len() as f64 * 100.0
}

/// Trend figures from daily closes, oldest first. `None` without a close.
pub fn trend(closes: &[f64]) -> Option<Trend> {
    let last = *closes.last().filter(|c| **c > 0.0)?;
    let n = closes.len();
    let avg = |days: usize| (n >= days).then(|| mean(&closes[n - days..])).flatten();
    let vs = |days: usize| {
        avg(days)
            .filter(|a| *a > 0.0)
            .map(|a| (last / a - 1.0) * 100.0)
    };
    // The distance from the 200-day average on every day that has one.
    let distances: Vec<f64> = (200..=n)
        .filter_map(|end| {
            let a = mean(&closes[end - 200..end])?;
            (a > 0.0).then(|| (closes[end - 1] / a - 1.0) * 100.0)
        })
        .collect();
    let vs_200 = vs(200);
    let peak = closes.iter().copied().fold(f64::MIN, f64::max);
    let year_high = closes[n.saturating_sub(YEAR)..]
        .iter()
        .copied()
        .fold(f64::MIN, f64::max);
    Some(Trend {
        last,
        vs_200_day_pct: vs_200,
        vs_200_day_percentile: vs_200
            .filter(|_| distances.len() >= YEAR)
            .map(|d| share_below(&distances, d)),
        vs_50_day_pct: vs(50),
        from_52_week_high_pct: (1.0 - last / year_high) * 100.0,
        from_peak_pct: (1.0 - last / peak) * 100.0,
        return_1y_pct: (n > YEAR)
            .then(|| closes[n - 1 - YEAR])
            .filter(|c| *c > 0.0)
            .map(|c| (last / c - 1.0) * 100.0),
        history_years: n as f64 / YEAR as f64,
    })
}

/// The volatility index against its closes, oldest first.
pub fn volatility(name: &str, closes: &[f64]) -> Option<Volatility> {
    let last = *closes.last().filter(|c| **c > 0.0)?;
    Some(Volatility {
        name: name.to_string(),
        last,
        percentile: share_below(closes, last),
        history_years: closes.len() as f64 / YEAR as f64,
    })
}

/// Percent of `flags` that are true; `None` when there are none.
fn pct_true(flags: Vec<bool>) -> Option<f64> {
    (!flags.is_empty())
        .then(|| flags.iter().filter(|f| **f).count() as f64 / flags.len() as f64 * 100.0)
}

/// Breadth of the scanned stocks.
pub fn breadth(quotes: &[Quote]) -> Breadth {
    let priced: Vec<(&Quote, f64)> = quotes
        .iter()
        .filter_map(|q| Some((q, q.regular_market_price.filter(|p| *p > 0.0)?)))
        .collect();
    let above = |avg: fn(&Quote) -> Option<f64>| {
        pct_true(
            priced
                .iter()
                .filter_map(|(q, p)| avg(q).filter(|a| *a > 0.0).map(|a| *p > a))
                .collect(),
        )
    };
    Breadth {
        stocks: priced.len(),
        above_200_day_pct: above(|q| q.two_hundred_day_average),
        above_50_day_pct: above(|q| q.fifty_day_average),
        near_52_week_high_pct: pct_true(
            priced
                .iter()
                .filter_map(|(q, p)| {
                    q.fifty_two_week_high
                        .filter(|h| *h > 0.0)
                        .map(|h| *p >= h * 0.95)
                })
                .collect(),
        ),
        near_52_week_low_pct: pct_true(
            priced
                .iter()
                .filter_map(|(q, p)| {
                    q.fifty_two_week_low
                        .filter(|l| *l > 0.0)
                        .map(|l| *p <= l * 1.05)
                })
                .collect(),
        ),
        advancing_pct: pct_true(
            priced
                .iter()
                .filter_map(|(q, _)| q.regular_market_change_percent.map(|c| c > 0.0))
                .collect(),
        ),
    }
}

/// Valuation of the scanned stocks; `bond_yield_pct` is the 10-year
/// government yield when known.
pub fn valuation(quotes: &[Quote], bond_yield_pct: Option<f64>) -> Valuation {
    // Market cap and trailing earnings (cap / P/E) of the profitable ones.
    let profitable: Vec<(f64, f64)> = quotes
        .iter()
        .filter_map(|q| {
            let cap = q.market_cap.filter(|c| *c > 0.0)?;
            let pe = q.trailing_pe.filter(|p| *p > 0.0)?;
            Some((cap, cap / pe))
        })
        .collect();
    let cap: f64 = profitable.iter().map(|(c, _)| c).sum();
    let earnings: f64 = profitable.iter().map(|(_, e)| e).sum();
    let aggregate_pe = (earnings > 0.0).then(|| cap / earnings);
    let earnings_yield_pct = aggregate_pe.map(|pe| 100.0 / pe);
    Valuation {
        stocks: profitable.len(),
        aggregate_pe,
        median_pe: median(
            quotes
                .iter()
                .filter_map(|q| q.trailing_pe.filter(|p| *p > 0.0))
                .collect(),
        ),
        earnings_yield_pct,
        bond_yield_pct,
        earnings_minus_bond_pct: earnings_yield_pct.zip(bond_yield_pct).map(|(e, b)| e - b),
        median_dividend_yield_pct: median(
            quotes
                .iter()
                .filter_map(|q| q.trailing_annual_dividend_yield.filter(|d| *d > 0.0))
                .map(|d| d * 100.0)
                .collect(),
        ),
    }
}

/// One line of plain description per reading, for the table.
pub fn describe(r: &Report) -> Vec<String> {
    let mut out = Vec::new();
    let side = |v: f64| if v >= 0.0 { "above" } else { "below" };
    if let Some(t) = &r.trend {
        if let Some(d) = t.vs_200_day_pct {
            let mut line = format!(
                "{} is {:.1}% {} its 200-day average",
                r.index,
                d.abs(),
                side(d)
            );
            if let Some(p) = t.vs_200_day_percentile {
                line.push_str(&format!(
                    "; the distance was lower on {:.0}% of days in the last {:.0} years",
                    p, t.history_years
                ));
            }
            out.push(line + ".");
        }
        out.push(format!(
            "It is {:.1}% below its 52-week high and {:.1}% below its highest close of the last {:.0} years.",
            t.from_52_week_high_pct, t.from_peak_pct, t.history_years
        ));
    }
    if let Some(v) = &r.volatility {
        out.push(format!(
            "{} is {:.1}; it was lower on {:.0}% of days in the last {:.0} years.",
            v.name, v.last, v.percentile, v.history_years
        ));
    }
    if let Some(a) = r.breadth.above_200_day_pct {
        out.push(format!(
            "{:.0}% of the {} stocks scanned are above their 200-day average.",
            a, r.breadth.stocks
        ));
    }
    if let (Some(e), Some(b)) = (r.valuation.earnings_yield_pct, r.valuation.bond_yield_pct) {
        out.push(format!(
            "The scanned stocks earn {:.1}% of their market value a year; the 10-year government bond yields {:.1}%.",
            e, b
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trend_measures_the_index_against_its_averages_and_highs() {
        // 300 flat sessions at 100, then a climb to 120 and a slip to 114.
        let mut closes = vec![100.0; 300];
        closes.extend((1..=20).map(|i| 100.0 + i as f64));
        closes.extend([118.0, 116.0, 114.0]);
        let t = trend(&closes).unwrap();
        assert_eq!(t.last, 114.0);
        assert!((t.from_peak_pct - 5.0).abs() < 1e-9, "{t:?}");
        assert!((t.from_52_week_high_pct - 5.0).abs() < 1e-9);
        // Above both averages, and the one-year change is from 100.
        assert!(t.vs_200_day_pct.unwrap() > 10.0);
        assert!(t.vs_50_day_pct.unwrap() > 0.0);
        assert!((t.return_1y_pct.unwrap() - 14.0).abs() < 1e-9);
        // Too little history for a percentile of the 200-day distance.
        assert_eq!(t.vs_200_day_percentile, None);
        assert_eq!(trend(&[]), None);
    }

    #[test]
    fn percentile_needs_a_year_of_distances_and_counts_lower_days() {
        // A steady climb: today's distance is the largest seen.
        let closes: Vec<f64> = (0..800).map(|i| 100.0 * 1.001_f64.powi(i)).collect();
        let t = trend(&closes).unwrap();
        let p = t.vs_200_day_percentile.unwrap();
        assert!((0.0..=100.0).contains(&p));
        let v = volatility("VIX", &[10.0, 20.0, 30.0, 40.0, 25.0]).unwrap();
        assert_eq!(v.percentile, 40.0);
        assert_eq!(volatility("VIX", &[]), None);
    }

    fn stock(price: f64, ma200: f64, pe: Option<f64>, cap: f64, change: f64) -> Quote {
        Quote {
            regular_market_price: Some(price),
            two_hundred_day_average: Some(ma200),
            fifty_day_average: Some(ma200),
            fifty_two_week_high: Some(120.0),
            fifty_two_week_low: Some(60.0),
            trailing_pe: pe,
            market_cap: Some(cap),
            regular_market_change_percent: Some(change),
            trailing_annual_dividend_yield: Some(0.02),
            ..Default::default()
        }
    }

    #[test]
    fn breadth_and_valuation_of_a_small_scan() {
        let quotes = [
            stock(118.0, 100.0, Some(10.0), 1000.0, 1.0),
            stock(90.0, 100.0, Some(30.0), 3000.0, -1.0),
            // Loss-making: in the breadth, out of the valuation.
            stock(62.0, 100.0, None, 500.0, -2.0),
            Quote::default(),
        ];
        let b = breadth(&quotes);
        assert_eq!(b.stocks, 3);
        assert!((b.above_200_day_pct.unwrap() - 100.0 / 3.0).abs() < 1e-9);
        assert!((b.near_52_week_high_pct.unwrap() - 100.0 / 3.0).abs() < 1e-9);
        assert!((b.near_52_week_low_pct.unwrap() - 100.0 / 3.0).abs() < 1e-9);
        assert!((b.advancing_pct.unwrap() - 100.0 / 3.0).abs() < 1e-9);

        let v = valuation(&quotes, Some(7.0));
        assert_eq!(v.stocks, 2);
        // 4000 of market cap over 100 + 100 of earnings.
        assert!((v.aggregate_pe.unwrap() - 20.0).abs() < 1e-9);
        assert_eq!(v.median_pe, Some(20.0));
        assert!((v.earnings_yield_pct.unwrap() - 5.0).abs() < 1e-9);
        assert!((v.earnings_minus_bond_pct.unwrap() + 2.0).abs() < 1e-9);
        assert!((v.median_dividend_yield_pct.unwrap() - 2.0).abs() < 1e-9);
        assert_eq!(valuation(&[], None).aggregate_pe, None);
    }

    #[test]
    fn descriptions_state_readings_in_neutral_words() {
        let closes: Vec<f64> = (0..800).map(|i| 100.0 + (i % 50) as f64).collect();
        let quotes = [stock(118.0, 100.0, Some(10.0), 1000.0, 1.0)];
        let report = Report {
            market: "India".into(),
            index: "NIFTY 50".into(),
            trend: trend(&closes),
            volatility: volatility("India VIX", &[12.0, 15.0, 14.0]),
            breadth: breadth(&quotes),
            valuation: valuation(&quotes, Some(7.0)),
            scanned: 1,
        };
        let lines = describe(&report);
        assert_eq!(lines.len(), 5, "{lines:?}");
        for line in &lines {
            crate::verdict::testing::assert_neutral(line);
        }
    }
}
