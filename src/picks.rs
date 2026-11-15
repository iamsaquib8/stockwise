//! `picks`: a 0-100 score for each scanned stock, made of four pillars.
//! Every measure is a percentile rank among the stocks of the same scan,
//! so the score says where a stock stands in that set, not whether a fixed
//! threshold was crossed. Measures come from the bulk quote alone (price,
//! averages, 52-week range, EPS, book value, dividend yield): no request
//! per stock.
//!
//! A score describes the data. It is not a recommendation.

use crate::api::Quote;
use crate::output::CsvRow;
use serde::Serialize;

/// Pillar weights; they add up to 1.
const W_VALUE: f64 = 0.30;
const W_QUALITY: f64 = 0.25;
const W_GROWTH: f64 = 0.20;
const W_TREND: f64 = 0.25;

/// Caps on raw measures. Past them a figure usually reflects a tiny
/// denominator (a sliver of book value, a one-off profit), not a better
/// company, so everything beyond a cap ranks the same.
const MAX_EARNINGS_YIELD: f64 = 0.25;
const MAX_BOOK_YIELD: f64 = 2.0;
const MAX_ROE: f64 = 0.40;
const MAX_EPS_GROWTH: f64 = 1.0;

/// A pillar at or above this rank is named among a stock's strongest.
const STRONG: f64 = 80.0;

/// One scored stock.
#[derive(Debug, Clone, Serialize)]
pub struct Pick {
    pub symbol: String,
    pub name: Option<String>,
    pub price: Option<f64>,
    pub change_percent: Option<f64>,
    pub currency: Option<String>,
    /// 0-100: the weighted pillars, pulled toward 50 by missing data.
    pub score: f64,
    /// Earnings yield, book value to price and dividend yield.
    pub value: Option<f64>,
    /// Return on equity (EPS over book value per share).
    pub quality: Option<f64>,
    /// Forward EPS against trailing EPS.
    pub growth: Option<f64>,
    /// Price against the 200-day average, the 50-day against the 200-day,
    /// and the place in the 52-week range.
    pub trend: Option<f64>,
    /// Share of the score's weight that had data, 0-1.
    pub coverage: f64,
}

impl CsvRow for Pick {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "name",
        "price",
        "change_percent",
        "currency",
        "score",
        "value",
        "quality",
        "growth",
        "trend",
        "coverage",
    ];
}

impl Pick {
    /// The pillars in the top fifth of the scan, for the table.
    pub fn strongest(&self) -> Vec<&'static str> {
        [
            ("Value", self.value),
            ("Quality", self.quality),
            ("Growth", self.growth),
            ("Trend", self.trend),
        ]
        .into_iter()
        .filter(|(_, v)| v.is_some_and(|v| v >= STRONG))
        .map(|(n, _)| n)
        .collect()
    }
}

/// The raw measures of one quote; `None` where the quote lacks the data.
struct Measures {
    earnings_yield: Option<f64>,
    book_yield: Option<f64>,
    dividend_yield: Option<f64>,
    roe: Option<f64>,
    eps_growth: Option<f64>,
    vs_200: Option<f64>,
    ma_slope: Option<f64>,
    range_pos: Option<f64>,
}

fn measures(q: &Quote) -> Measures {
    let price = q.regular_market_price.filter(|p| *p > 0.0);
    let pos = |v: Option<f64>| v.filter(|x| *x > 0.0);
    let eps = q.eps_trailing_twelve_months;
    let book = pos(q.book_value);
    Measures {
        // A loss is a negative yield: it ranks below every profit.
        earnings_yield: match (eps, price) {
            (Some(e), Some(p)) => Some(e / p),
            _ => pos(q.trailing_pe).map(|pe| 1.0 / pe),
        }
        .map(|y| y.min(MAX_EARNINGS_YIELD)),
        book_yield: match (book, price) {
            (Some(b), Some(p)) => Some(b / p),
            _ => pos(q.price_to_book).map(|pb| 1.0 / pb),
        }
        .map(|y| y.min(MAX_BOOK_YIELD)),
        dividend_yield: q.trailing_annual_dividend_yield.filter(|d| *d >= 0.0),
        roe: match (eps, book) {
            (Some(e), Some(b)) => Some(e / b),
            _ => q.return_on_equity,
        }
        .map(|r| r.min(MAX_ROE)),
        // From a loss or zero there is no growth rate to speak of.
        eps_growth: match (q.eps_forward, eps) {
            (Some(f), Some(t)) if t > 0.0 => Some(((f - t) / t).min(MAX_EPS_GROWTH)),
            _ => None,
        },
        vs_200: match (price, pos(q.two_hundred_day_average)) {
            (Some(p), Some(m)) => Some(p / m - 1.0),
            _ => None,
        },
        ma_slope: match (pos(q.fifty_day_average), pos(q.two_hundred_day_average)) {
            (Some(a), Some(b)) => Some(a / b - 1.0),
            _ => None,
        },
        range_pos: match (price, q.fifty_two_week_low, q.fifty_two_week_high) {
            (Some(p), Some(lo), Some(hi)) if hi > lo => {
                Some(((p - lo) / (hi - lo)).clamp(0.0, 1.0))
            }
            _ => None,
        },
    }
}

/// Percentile rank (0-100) of each value among those present: the share
/// below it plus half of those equal to it (itself included), so the
/// lowest of four ranks 12.5 and a lone value 50.
fn ranks(values: &[Option<f64>]) -> Vec<Option<f64>> {
    let mut sorted: Vec<f64> = values.iter().flatten().copied().collect();
    sorted.sort_by(f64::total_cmp);
    let n = sorted.len() as f64;
    values
        .iter()
        .map(|v| {
            let v = (*v)?;
            let below = sorted.partition_point(|x| *x < v) as f64;
            let not_above = sorted.partition_point(|x| *x <= v) as f64;
            Some((below + (not_above - below) / 2.0) / n * 100.0)
        })
        .collect()
}

/// Weighted mean of the parts that have a value; `None` when none has.
fn blend(parts: &[(Option<f64>, f64)]) -> Option<f64> {
    let weight: f64 = parts
        .iter()
        .filter(|(v, _)| v.is_some())
        .map(|(_, w)| w)
        .sum();
    (weight > 0.0).then(|| {
        parts
            .iter()
            .filter_map(|(v, w)| v.map(|v| v * w))
            .sum::<f64>()
            / weight
    })
}

/// Score every quote against the others, best first (ties by symbol).
pub fn score(quotes: &[Quote]) -> Vec<Pick> {
    let m: Vec<Measures> = quotes.iter().map(measures).collect();
    let col = |f: fn(&Measures) -> Option<f64>| ranks(&m.iter().map(f).collect::<Vec<_>>());
    let (ey, by, dy) = (
        col(|m| m.earnings_yield),
        col(|m| m.book_yield),
        col(|m| m.dividend_yield),
    );
    let (roe, growth) = (col(|m| m.roe), col(|m| m.eps_growth));
    let (vs200, slope, range) = (col(|m| m.vs_200), col(|m| m.ma_slope), col(|m| m.range_pos));
    let mut picks: Vec<Pick> = quotes
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let value = blend(&[(ey[i], 0.5), (by[i], 0.3), (dy[i], 0.2)]);
            let quality = roe[i];
            let growth = growth[i];
            let trend = blend(&[(vs200[i], 1.0), (slope[i], 1.0), (range[i], 1.0)]);
            let pillars = [
                (value, W_VALUE),
                (quality, W_QUALITY),
                (growth, W_GROWTH),
                (trend, W_TREND),
            ];
            let coverage: f64 = pillars
                .iter()
                .filter(|(v, _)| v.is_some())
                .map(|(_, w)| w)
                .sum();
            // Missing pillars pull the score toward the middle: a stock is
            // not ranked high on half the picture.
            let score = blend(&pillars).map_or(50.0, |raw| 50.0 + (raw - 50.0) * coverage);
            Pick {
                symbol: q.symbol.clone().unwrap_or_default(),
                name: q.short_name.clone().or_else(|| q.long_name.clone()),
                price: q.regular_market_price,
                change_percent: q.regular_market_change_percent,
                currency: q.currency.clone(),
                score,
                value,
                quality,
                growth,
                trend,
                coverage,
            }
        })
        .collect();
    picks.sort_by(|a, b| {
        b.score
            .total_cmp(&a.score)
            .then_with(|| a.symbol.cmp(&b.symbol))
    });
    picks
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(
        sym: &str,
        price: f64,
        eps: f64,
        eps_fwd: Option<f64>,
        book: f64,
        ma200: f64,
    ) -> Quote {
        Quote {
            symbol: Some(sym.into()),
            regular_market_price: Some(price),
            eps_trailing_twelve_months: Some(eps),
            eps_forward: eps_fwd,
            book_value: Some(book),
            fifty_day_average: Some((price + ma200) / 2.0),
            two_hundred_day_average: Some(ma200),
            fifty_two_week_low: Some(ma200 * 0.8),
            fifty_two_week_high: Some(ma200 * 1.3),
            trailing_annual_dividend_yield: Some(0.01),
            ..Default::default()
        }
    }

    #[test]
    fn ranks_are_percentiles_with_ties_and_gaps() {
        let r = ranks(&[Some(1.0), Some(3.0), None, Some(3.0), Some(9.0)]);
        // Mid-rank: a value counts half of itself and of its ties.
        assert_eq!(r[0], Some(12.5));
        assert_eq!(r[1], Some(50.0));
        assert_eq!(r[2], None);
        assert_eq!(r[3], Some(50.0));
        assert_eq!(r[4], Some(87.5));
        assert_eq!(ranks(&[Some(7.0)]), [Some(50.0)]);
    }

    #[test]
    fn the_stock_ahead_on_every_measure_scores_highest() {
        let quotes = [
            // Cheap, profitable, growing, rising.
            quote("BEST.NS", 100.0, 12.0, Some(15.0), 60.0, 80.0),
            quote("MID.NS", 100.0, 6.0, Some(6.5), 50.0, 100.0),
            // Loss-making and falling.
            quote("WORST.NS", 100.0, -3.0, None, 20.0, 130.0),
        ];
        let picks = score(&quotes);
        let order: Vec<&str> = picks.iter().map(|p| p.symbol.as_str()).collect();
        assert_eq!(order, ["BEST.NS", "MID.NS", "WORST.NS"]);
        assert!(picks[0].score > 60.0 && picks[2].score < 40.0, "{picks:?}");
        // No growth rate from a loss: the pillar is absent and the
        // coverage says so.
        assert_eq!(picks[2].growth, None);
        assert!((picks[2].coverage - 0.8).abs() < 1e-9);
        assert_eq!(picks[0].coverage, 1.0);
        assert!(picks.iter().all(|p| (0.0..=100.0).contains(&p.score)));
    }

    #[test]
    fn figures_past_a_cap_rank_the_same() {
        // ROE of 60% and of 400% (a sliver of book value) are both capped.
        let quotes = [
            quote("SOLID.NS", 100.0, 12.0, None, 20.0, 100.0),
            quote("SLIVER.NS", 100.0, 12.0, None, 3.0, 100.0),
            quote("PLAIN.NS", 100.0, 12.0, None, 120.0, 100.0),
        ];
        let picks = score(&quotes);
        let q = |s: &str| picks.iter().find(|p| p.symbol == s).unwrap().quality;
        assert_eq!(q("SOLID.NS"), q("SLIVER.NS"));
        assert!(q("SOLID.NS") > q("PLAIN.NS"));
    }

    #[test]
    fn missing_data_pulls_the_score_to_the_middle() {
        let bare = Quote {
            symbol: Some("BARE.NS".into()),
            regular_market_price: Some(10.0),
            ..Default::default()
        };
        let picks = score(&[bare, quote("FULL.NS", 100.0, 12.0, Some(15.0), 60.0, 80.0)]);
        let bare = picks.iter().find(|p| p.symbol == "BARE.NS").unwrap();
        assert_eq!(bare.score, 50.0);
        assert_eq!(bare.coverage, 0.0);
        assert!(bare.strongest().is_empty());
    }

    #[test]
    fn strongest_names_the_top_fifth_pillars_in_neutral_words() {
        let mut quotes: Vec<Quote> = (0..10)
            .map(|i| {
                let f = i as f64;
                quote(
                    &format!("S{i}.NS"),
                    100.0,
                    2.0 + f,
                    Some(2.0 + f),
                    50.0,
                    120.0 - 5.0 * f,
                )
            })
            .collect();
        quotes.reverse();
        let picks = score(&quotes);
        let top = &picks[0];
        assert_eq!(top.symbol, "S9.NS");
        // Earnings and trend rise with the index; book and dividend yield
        // are the same for all, so value stays below the top fifth.
        let words = top.strongest();
        assert_eq!(words, ["Quality", "Trend"]);
        crate::verdict::testing::assert_neutral(&words.join(", "));
    }
}
