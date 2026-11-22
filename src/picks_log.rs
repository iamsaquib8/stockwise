//! The track record of `picks`: every run appends its list to
//! `signal_log.json` in the data directory (user data, unlike `cache.db`,
//! which may be deleted at any time), and `picks review` measures each
//! logged pick from the day it was first listed against the market's
//! benchmark.
//!
//! Fundamentals have no point-in-time history, so the scoring cannot be
//! backtested honestly; this forward record is the evidence.

use anyhow::Result;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const FILE: &str = "signal_log.json";

fn long() -> String {
    "long".to_string()
}
/// Oldest entries are dropped past this many.
const MAX_ENTRIES: usize = 50_000;

/// One listed stock of one run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Session date of the run (local date).
    pub date: NaiveDate,
    /// `pick` for `picks`; other kinds may follow.
    pub kind: String,
    /// `long` or `short` (the list's horizon); `long` for older entries.
    #[serde(default = "long")]
    pub horizon: String,
    /// Market code (`in`, `us`).
    pub market: String,
    /// The scan's scope ("India", "NIFTY 50", ...).
    pub universe: String,
    pub symbol: String,
    /// 1 for the highest score.
    pub rank: usize,
    pub score: f64,
    /// Price when listed. A market observation used only for returns
    /// (a ratio), never booked as money, so `f64` like every other quote.
    pub price: f64,
    pub currency: Option<String>,
    /// The benchmark's level at the same moment, so the review compares
    /// matching observations; absent in lists logged before 0.9.
    #[serde(default)]
    pub benchmark_price: Option<f64>,
    /// [`crate::picks::METHODOLOGY`] of the run.
    pub methodology: String,
}

/// Append `entries`; a stock already logged that day for the same kind,
/// market and scope is not logged again, so re-running a scan does not
/// count a pick twice.
pub fn record(entries: Vec<Entry>) -> Result<usize> {
    crate::store::update::<Vec<Entry>, _>(FILE, |log| Ok(merge(log, entries)))
}

fn merge(log: &mut Vec<Entry>, entries: Vec<Entry>) -> usize {
    let mut added = 0;
    for e in entries {
        let dup = log.iter().any(|o| {
            o.date == e.date
                && o.kind == e.kind
                && o.horizon == e.horizon
                && o.market == e.market
                && o.universe == e.universe
                && o.symbol == e.symbol
        });
        if !dup {
            log.push(e);
            added += 1;
        }
    }
    if log.len() > MAX_ENTRIES {
        let extra = log.len() - MAX_ENTRIES;
        log.drain(..extra);
    }
    added
}

/// Every logged entry, oldest first; empty when nothing was logged yet. An
/// unreadable file is an error: reporting it as "no picks" would hide it,
/// and the next scan could not append to it either.
pub fn load() -> Result<Vec<Entry>> {
    crate::store::load::<Vec<Entry>>(FILE)
}

/// Longest lookback `picks review` accepts: ten years.
pub const MAX_LOOKBACK_DAYS: u32 = 3_650;

/// `90d`, `12w`, `6m`, `1y` or a plain number of days.
pub fn parse_days(s: &str) -> Result<u32, String> {
    let s = s.trim().to_ascii_lowercase();
    let (num, mult) = match s.chars().last() {
        Some('d') => (&s[..s.len() - 1], 1),
        Some('w') => (&s[..s.len() - 1], 7),
        Some('m') => (&s[..s.len() - 1], 30),
        Some('y') => (&s[..s.len() - 1], 365),
        _ => (s.as_str(), 1),
    };
    num.parse::<u32>()
        .ok()
        .filter(|n| *n > 0)
        .and_then(|n| n.checked_mul(mult))
        .filter(|d| *d <= MAX_LOOKBACK_DAYS)
        .ok_or_else(|| format!("'{s}' is not a period like 90d, 12w, 6m or 1y (at most 10 years)"))
}

/// One pick measured since it was first listed.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub symbol: String,
    pub picked_on: NaiveDate,
    pub days: i64,
    pub rank: usize,
    pub score: f64,
    pub entry_price: f64,
    pub price: f64,
    pub currency: Option<String>,
    /// Price return since listing, as a fraction (dividends not included).
    pub return_pct: f64,
    /// The benchmark's return over the same days.
    pub benchmark_pct: Option<f64>,
    /// `return_pct - benchmark_pct`.
    pub excess_pct: Option<f64>,
}

/// The track record over a window.
#[derive(Debug, Clone, Serialize)]
pub struct Review {
    pub benchmark: Option<String>,
    pub since: NaiveDate,
    pub rows: Vec<Row>,
    /// Logged picks without a current price (delisted, renamed, failed).
    pub unpriced: Vec<String>,
    /// Logged picks whose splits since listing could not be checked: left
    /// out, since a split would read as a loss.
    pub unverified: Vec<String>,
    /// Share of picks that rose.
    pub hit_rate: Option<f64>,
    /// Share of picks that beat the benchmark.
    pub beat_rate: Option<f64>,
    pub mean_return: Option<f64>,
    pub median_return: Option<f64>,
    pub mean_excess: Option<f64>,
    pub median_excess: Option<f64>,
    pub methodologies: Vec<String>,
}

fn mean(v: &[f64]) -> Option<f64> {
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}

fn median(v: &[f64]) -> Option<f64> {
    if v.is_empty() {
        return None;
    }
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    let n = s.len();
    Some(if n % 2 == 1 {
        s[n / 2]
    } else {
        (s[n / 2 - 1] + s[n / 2]) / 2.0
    })
}

/// What a review compares with: the benchmark's name and its level now.
#[derive(Debug, Clone, Copy)]
pub struct Benchmark<'a> {
    pub name: &'a str,
    pub now: Option<f64>,
}

/// Measure the `pick` entries of `market` logged on or after `since`, each
/// from its first listing in the window, with `prices` (current price by
/// symbol) and `splits` (shares per listed share since each listing: 2.0
/// after a 2-for-1 split; `None` when that could not be checked; a symbol
/// without an entry has had none).
///
/// Each leg compares matching observations: the benchmark's live level
/// logged with the list against its live level now, next to the pick's
/// live prices. A pick without a logged level, or a review without a live
/// level now, has no benchmark comparison rather than a mismatched one.
pub fn review(
    entries: &[Entry],
    market: &str,
    horizon: &str,
    since: NaiveDate,
    today: NaiveDate,
    prices: &HashMap<String, f64>,
    splits: &HashMap<String, Option<f64>>,
    benchmark: Option<Benchmark>,
) -> Review {
    review_with_dates(
        entries,
        market,
        horizon,
        since,
        today,
        prices,
        &HashMap::new(),
        splits,
        benchmark,
    )
}

/// [`review`], using each quote's own session date (`quote_dates`) for that
/// pick's elapsed `days` and falling back to `today` when a quote has no
/// timestamp.
#[allow(clippy::too_many_arguments)]
pub fn review_with_dates(
    entries: &[Entry],
    market: &str,
    horizon: &str,
    since: NaiveDate,
    today: NaiveDate,
    prices: &HashMap<String, f64>,
    quote_dates: &HashMap<String, NaiveDate>,
    splits: &HashMap<String, Option<f64>>,
    benchmark: Option<Benchmark>,
) -> Review {
    let mut first: Vec<&Entry> = Vec::new();
    for e in entries.iter().filter(|e| {
        e.kind == "pick" && e.market == market && e.horizon == horizon && e.date >= since
    }) {
        match first.iter_mut().find(|f| f.symbol == e.symbol) {
            Some(f) if e.date < f.date => *f = e,
            Some(_) => {}
            None => first.push(e),
        }
    }
    let mut rows = Vec::new();
    let mut unpriced = Vec::new();
    let mut unverified = Vec::new();
    let mut methodologies: Vec<String> = Vec::new();
    for e in first {
        if !methodologies.contains(&e.methodology) {
            methodologies.push(e.methodology.clone());
        }
        let Some(&price) = prices.get(&e.symbol).filter(|p| **p > 0.0) else {
            unpriced.push(e.symbol.clone());
            continue;
        };
        if e.price <= 0.0 {
            unpriced.push(e.symbol.clone());
            continue;
        }
        // A 2-for-1 split halves the price without a loss: compare on the
        // shares a listed share became.
        let factor = match splits.get(&e.symbol) {
            None => 1.0,
            Some(Some(f)) if *f > 0.0 => *f,
            Some(_) => {
                unverified.push(e.symbol.clone());
                continue;
            }
        };
        let ret = price * factor / e.price - 1.0;
        let bench = benchmark.and_then(|b| {
            let then = e.benchmark_price.filter(|p| *p > 0.0)?;
            let now = b.now.filter(|p| *p > 0.0)?;
            Some(now / then - 1.0)
        });
        let end_date = quote_dates.get(&e.symbol).copied().unwrap_or(today);
        rows.push(Row {
            symbol: e.symbol.clone(),
            picked_on: e.date,
            days: (end_date - e.date).num_days(),
            rank: e.rank,
            score: e.score,
            entry_price: e.price,
            price,
            currency: e.currency.clone(),
            return_pct: ret,
            benchmark_pct: bench,
            excess_pct: bench.map(|b| ret - b),
        });
    }
    rows.sort_by(|a, b| {
        a.picked_on
            .cmp(&b.picked_on)
            .then_with(|| a.rank.cmp(&b.rank))
    });
    let returns: Vec<f64> = rows.iter().map(|r| r.return_pct).collect();
    let excess: Vec<f64> = rows.iter().filter_map(|r| r.excess_pct).collect();
    let share = |n: usize, d: usize| (d > 0).then(|| n as f64 / d as f64);
    Review {
        benchmark: benchmark.map(|b| b.name.to_string()),
        since,
        hit_rate: share(returns.iter().filter(|r| **r > 0.0).count(), returns.len()),
        beat_rate: share(excess.iter().filter(|r| **r > 0.0).count(), excess.len()),
        mean_return: mean(&returns),
        median_return: median(&returns),
        mean_excess: mean(&excess),
        median_excess: median(&excess),
        rows,
        unpriced,
        unverified,
        methodologies,
    }
}

impl crate::output::CsvRow for Row {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "picked_on",
        "days",
        "rank",
        "score",
        "entry_price",
        "price",
        "currency",
        "return_pct",
        "benchmark_pct",
        "excess_pct",
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn entry(date: &str, sym: &str, price: f64) -> Entry {
        Entry {
            date: d(date),
            kind: "pick".into(),
            horizon: "long".into(),
            market: "in".into(),
            universe: "India".into(),
            symbol: sym.into(),
            rank: 1,
            score: 80.0,
            price,
            currency: Some("INR".into()),
            benchmark_price: None,
            methodology: "2".into(),
        }
    }

    #[test]
    fn a_rerun_on_the_same_day_is_not_logged_twice() {
        let mut log = vec![entry("2026-01-05", "A.NS", 100.0)];
        let added = merge(
            &mut log,
            vec![
                entry("2026-01-05", "A.NS", 101.0),
                entry("2026-01-06", "A.NS", 102.0),
            ],
        );
        assert_eq!(added, 1);
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn review_measures_each_pick_from_its_first_listing() {
        let log = vec![
            entry("2026-01-05", "A.NS", 100.0),
            entry("2026-02-02", "A.NS", 130.0),
            entry("2026-01-05", "B.NS", 50.0),
            entry("2026-01-05", "GONE.NS", 10.0),
            // Before the window: ignored.
            entry("2025-06-01", "OLD.NS", 10.0),
        ];
        let prices = HashMap::from([
            ("A.NS".to_string(), 120.0),
            ("B.NS".to_string(), 45.0),
            ("OLD.NS".to_string(), 20.0),
        ]);
        // Each list was logged with the index at 20,000; it is 21,000 now.
        let log: Vec<Entry> = log
            .into_iter()
            .map(|mut e| {
                e.benchmark_price = Some(20_000.0);
                e
            })
            .collect();
        let r = review(
            &log,
            "in",
            "long",
            d("2026-01-01"),
            d("2026-03-02"),
            &prices,
            &HashMap::new(),
            Some(Benchmark {
                name: "NIFTY 50",
                now: Some(21_000.0),
            }),
        );
        assert_eq!(r.rows.len(), 2);
        assert_eq!(r.unpriced, ["GONE.NS"]);
        let a = &r.rows[0];
        assert_eq!(a.symbol, "A.NS");
        assert_eq!(a.picked_on, d("2026-01-05"));
        assert!((a.return_pct - 0.20).abs() < 1e-9);
        assert!((a.benchmark_pct.unwrap() - 0.05).abs() < 1e-9);
        assert!((a.excess_pct.unwrap() - 0.15).abs() < 1e-9);
        assert_eq!(a.days, 56);
        // A up 20%, B down 10%: half rose, half beat the index.
        assert_eq!(r.hit_rate, Some(0.5));
        assert_eq!(r.beat_rate, Some(0.5));
        assert!((r.mean_return.unwrap() - 0.05).abs() < 1e-9);
        assert!((r.median_excess.unwrap() - 0.0).abs() < 1e-9);
    }

    #[test]
    fn a_split_is_not_a_loss_and_an_unknown_split_is_left_out() {
        let log = vec![
            entry("2026-01-05", "SPLIT.NS", 100.0),
            entry("2026-01-05", "UNKNOWN.NS", 100.0),
        ];
        let prices = HashMap::from([
            ("SPLIT.NS".to_string(), 55.0),
            ("UNKNOWN.NS".to_string(), 55.0),
        ]);
        let splits = HashMap::from([
            ("SPLIT.NS".to_string(), Some(2.0)),
            ("UNKNOWN.NS".to_string(), None),
        ]);
        let r = review(
            &log,
            "in",
            "long",
            d("2026-01-01"),
            d("2026-03-02"),
            &prices,
            &splits,
            None,
        );
        assert_eq!(r.rows.len(), 1);
        assert!((r.rows[0].return_pct - 0.10).abs() < 1e-9);
        assert_eq!(r.unverified, ["UNKNOWN.NS"]);
    }

    #[test]
    fn the_benchmark_leg_uses_the_level_logged_with_the_pick() {
        let mut e = entry("2026-03-02", "A.NS", 100.0);
        e.benchmark_price = Some(20_000.0);
        let prices = HashMap::from([("A.NS".to_string(), 101.0)]);
        // Same day: the daily close would compare with itself (0%); the
        // levels logged and now show the index's own move.
        let r = review(
            &[e],
            "in",
            "long",
            d("2026-03-01"),
            d("2026-03-02"),
            &prices,
            &HashMap::new(),
            Some(Benchmark {
                name: "NIFTY 50",
                now: Some(20_200.0),
            }),
        );
        assert!((r.rows[0].benchmark_pct.unwrap() - 0.01).abs() < 1e-9);
        assert!(r.rows[0].excess_pct.unwrap().abs() < 1e-9);
    }

    #[test]
    fn no_benchmark_comparison_without_matched_levels() {
        // Logged without the index level: no mismatched comparison.
        let prices = HashMap::from([("A.NS".to_string(), 110.0)]);
        let r = review(
            &[entry("2026-01-05", "A.NS", 100.0)],
            "in",
            "long",
            d("2026-01-01"),
            d("2026-03-02"),
            &prices,
            &HashMap::new(),
            Some(Benchmark {
                name: "NIFTY 50",
                now: Some(21_000.0),
            }),
        );
        assert_eq!(r.rows[0].benchmark_pct, None);
        assert!((r.rows[0].return_pct - 0.10).abs() < 1e-9);
    }

    #[test]
    fn periods_parse() {
        assert_eq!(parse_days("90d"), Ok(90));
        assert_eq!(parse_days("12w"), Ok(84));
        assert_eq!(parse_days("6m"), Ok(180));
        assert_eq!(parse_days("1y"), Ok(365));
        assert_eq!(parse_days("30"), Ok(30));
        assert!(parse_days("0d").is_err());
        assert!(parse_days("soon").is_err());
        assert!(parse_days("11767034y").is_err(), "overflow is rejected");
        assert!(parse_days("1000000000d").is_err());
        assert_eq!(parse_days("10y"), Ok(3_650));
    }
}
