//! The track record of IPO verdicts: every `stockwise ipo` run (and MCP
//! `ipo_board` / `ipo_analyze` call) appends the verdicts it gave to
//! `ipo_signal_log.json` in the data directory, and `stockwise ipo review`
//! measures each issue's last verdict against its listing day and its first
//! three months, next to NIFTY 50 over the same days.
//!
//! The book changes while an issue is open (WAIT can become APPLY on the
//! last afternoon), so one entry per issue per day is kept, the latest, and
//! the review reads the last verdict logged before listing.

use anyhow::Result;
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

const FILE: &str = "ipo_signal_log.json";
/// Oldest entries are dropped past this many.
const MAX_ENTRIES: usize = 20_000;
/// Trading sessions in about three months.
pub const THREE_MONTH_SESSIONS: usize = 63;

/// One verdict as given.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    /// Day the verdict was given (IST).
    pub date: NaiveDate,
    pub symbol: String,
    /// `mainboard` or `sme`.
    pub board: String,
    /// The verdict kind as in JSON (`APPLY`, `APPLY_SELL_ON_LISTING`, `WAIT`, `AVOID`, `NO_CALL`).
    pub verdict: String,
    /// Upper band (the issue price for nearly every book-built issue).
    pub issue_price: Option<f64>,
    pub close_date: Option<NaiveDate>,
    pub listing_date: Option<NaiveDate>,
    pub expected_gain_pct: Option<f64>,
    pub p_loss: Option<f64>,
    pub qib_x: Option<f64>,
    pub coverage: f64,
    /// [`crate::ipo_verdict::METHODOLOGY`] of the verdict.
    pub methodology: String,
}

/// Append `entries`, replacing an entry for the same issue on the same day.
pub fn record(entries: Vec<Entry>) -> Result<usize> {
    crate::store::update::<Vec<Entry>, _>(FILE, |log| Ok(merge(log, entries)))
}

fn merge(log: &mut Vec<Entry>, entries: Vec<Entry>) -> usize {
    let mut changed = 0;
    for e in entries {
        match log
            .iter_mut()
            .find(|o| o.date == e.date && o.symbol == e.symbol && o.board == e.board)
        {
            Some(o) if *o == e => {}
            Some(o) => {
                *o = e;
                changed += 1;
            }
            None => {
                log.push(e);
                changed += 1;
            }
        }
    }
    if log.len() > MAX_ENTRIES {
        let extra = log.len() - MAX_ENTRIES;
        log.drain(..extra);
    }
    changed
}

/// The log entry for a verdict given today; `None` for LISTED.
#[allow(clippy::too_many_arguments)]
pub fn entry_for(
    today: NaiveDate,
    symbol: &str,
    board: crate::ipo::BoardKind,
    issue_price: Option<f64>,
    close_date: Option<NaiveDate>,
    listing_date: Option<NaiveDate>,
    qib_x: Option<f64>,
    v: &crate::ipo_verdict::Verdict,
) -> Option<Entry> {
    if v.kind == crate::ipo_verdict::Kind::Listed {
        return None;
    }
    let kind = serde_json::to_value(v.kind).ok()?.as_str()?.to_string();
    Some(Entry {
        date: today,
        symbol: symbol.to_string(),
        board: match board {
            crate::ipo::BoardKind::Mainboard => "mainboard",
            crate::ipo::BoardKind::Sme => "sme",
        }
        .to_string(),
        verdict: kind,
        issue_price,
        close_date,
        listing_date,
        expected_gain_pct: v.outlook.as_ref().map(|o| o.expected_gain_pct),
        p_loss: v.outlook.as_ref().map(|o| o.p_loss),
        qib_x,
        coverage: v.coverage,
        methodology: v.methodology.to_string(),
    })
}

pub fn load() -> Result<Vec<Entry>> {
    crate::store::load::<Vec<Entry>>(FILE)
}

/// Each issue's verdict to judge: the last one logged on or before its
/// close date (or the last one at all when the close date is unknown).
pub fn final_verdicts(log: &[Entry], since: NaiveDate) -> Vec<Entry> {
    let mut sorted: Vec<&Entry> = log.iter().filter(|e| e.date >= since).collect();
    sorted.sort_by_key(|e| e.date);
    let mut out: Vec<Entry> = Vec::new();
    for e in sorted {
        if e.verdict == "LISTED" {
            continue;
        }
        if let Some(c) = e.close_date
            && e.date > c
        {
            // After close the book is final; post-close entries are not pre-listing verdicts.
            continue;
        }
        match out.iter_mut().find(|o| o.symbol == e.symbol) {
            Some(o) => *o = e.clone(),
            None => out.push(e.clone()),
        }
    }
    out.sort_by(|a, b| a.date.cmp(&b.date).then_with(|| a.symbol.cmp(&b.symbol)));
    out
}

/// Prices after listing for one issue and NIFTY 50 over the same sessions.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outcome {
    pub listing_date: Option<NaiveDate>,
    pub listing_open: Option<f64>,
    pub listing_close: Option<f64>,
    /// Close about 63 sessions after listing; `None` before then.
    pub close_3m: Option<f64>,
    /// NIFTY 50 close on the day before listing and 63 sessions later.
    pub nifty_before: Option<f64>,
    pub nifty_3m: Option<f64>,
}

/// One issue's verdict and what followed.
#[derive(Debug, Clone, Serialize)]
pub struct Row {
    pub symbol: String,
    pub verdict: String,
    pub logged_on: NaiveDate,
    pub issue_price: Option<f64>,
    pub expected_gain_pct: Option<f64>,
    pub listing_date: Option<NaiveDate>,
    /// Listing-day close over the issue price, percent.
    pub listing_return_pct: Option<f64>,
    /// Close ~3 months after listing over the issue price, percent.
    pub return_3m_pct: Option<f64>,
    /// NIFTY 50 over the same three months, percent.
    pub nifty_3m_pct: Option<f64>,
    pub excess_3m_pct: Option<f64>,
    pub methodology: String,
}

impl crate::output::CsvRow for Row {
    const COLUMNS: &'static [&'static str] = &[
        "symbol",
        "verdict",
        "logged_on",
        "issue_price",
        "expected_gain_pct",
        "listing_date",
        "listing_return_pct",
        "return_3m_pct",
        "nifty_3m_pct",
        "excess_3m_pct",
        "methodology",
    ];
}

/// Figures for one verdict bucket.
#[derive(Debug, Clone, Serialize)]
pub struct Bucket {
    pub verdict: String,
    pub issues: usize,
    pub listed: usize,
    /// Share of listed issues that closed listing day above the issue price, 0 to 1.
    pub hit_rate: Option<f64>,
    pub mean_listing_pct: Option<f64>,
    pub median_listing_pct: Option<f64>,
    /// Mean expected listing gain the model gave these issues.
    pub mean_expected_pct: Option<f64>,
    pub with_3m: usize,
    pub mean_3m_pct: Option<f64>,
    pub mean_excess_3m_pct: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Review {
    pub since: NaiveDate,
    pub benchmark: &'static str,
    pub buckets: Vec<Bucket>,
    pub rows: Vec<Row>,
    /// Issues not listed yet, or without prices.
    pub pending: Vec<String>,
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
    let m = s.len() / 2;
    Some(if s.len().is_multiple_of(2) {
        (s[m - 1] + s[m]) / 2.0
    } else {
        s[m]
    })
}

/// Bucket order: the verdict ladder.
const ORDER: [&str; 5] = ["APPLY", "APPLY_SELL_ON_LISTING", "WAIT", "AVOID", "NO_CALL"];

/// The review of `finals` given each symbol's outcome.
pub fn review(
    finals: &[Entry],
    outcomes: &std::collections::HashMap<String, Outcome>,
    since: NaiveDate,
) -> Review {
    let mut rows = Vec::new();
    let mut pending = Vec::new();
    let mut methodologies: Vec<String> = Vec::new();
    for e in finals {
        if !methodologies.contains(&e.methodology) {
            methodologies.push(e.methodology.clone());
        }
        let o = outcomes.get(&e.symbol).cloned().unwrap_or_default();
        let ret = |p: Option<f64>| match (p, e.issue_price) {
            (Some(p), Some(i)) if i > 0.0 => Some((p / i - 1.0) * 100.0),
            _ => None,
        };
        let listing_return_pct = ret(o.listing_close);
        if listing_return_pct.is_none() {
            pending.push(e.symbol.clone());
        }
        let return_3m_pct = ret(o.close_3m);
        let nifty_3m_pct = match (o.nifty_before, o.nifty_3m) {
            (Some(a), Some(b)) if a > 0.0 && o.close_3m.is_some() => Some((b / a - 1.0) * 100.0),
            _ => None,
        };
        rows.push(Row {
            symbol: e.symbol.clone(),
            verdict: e.verdict.clone(),
            logged_on: e.date,
            issue_price: e.issue_price,
            expected_gain_pct: e.expected_gain_pct,
            listing_date: o.listing_date.or(e.listing_date),
            listing_return_pct,
            return_3m_pct,
            nifty_3m_pct,
            excess_3m_pct: match (return_3m_pct, nifty_3m_pct) {
                (Some(r), Some(n)) => Some(r - n),
                _ => None,
            },
            methodology: e.methodology.clone(),
        });
    }
    let buckets = ORDER
        .iter()
        .filter_map(|k| {
            let rs: Vec<&Row> = rows.iter().filter(|r| r.verdict == *k).collect();
            if rs.is_empty() {
                return None;
            }
            let listing: Vec<f64> = rs.iter().filter_map(|r| r.listing_return_pct).collect();
            let three: Vec<f64> = rs.iter().filter_map(|r| r.return_3m_pct).collect();
            let excess: Vec<f64> = rs.iter().filter_map(|r| r.excess_3m_pct).collect();
            let expected: Vec<f64> = rs.iter().filter_map(|r| r.expected_gain_pct).collect();
            Some(Bucket {
                verdict: k.to_string(),
                issues: rs.len(),
                listed: listing.len(),
                hit_rate: (!listing.is_empty()).then(|| {
                    listing.iter().filter(|r| **r > 0.0).count() as f64 / listing.len() as f64
                }),
                mean_listing_pct: mean(&listing),
                median_listing_pct: median(&listing),
                mean_expected_pct: mean(&expected),
                with_3m: three.len(),
                mean_3m_pct: mean(&three),
                mean_excess_3m_pct: mean(&excess),
            })
        })
        .collect();
    Review {
        since,
        benchmark: "NIFTY 50",
        buckets,
        rows,
        pending,
        methodologies,
    }
}

/// Outcome from daily bars `(date, open, close)` of the issue and of NIFTY
/// 50, for an issue that closed bidding on `close_date`.
pub fn outcome_from_bars(
    bars: &[(NaiveDate, f64, f64)],
    nifty: &[(NaiveDate, f64, f64)],
    close_date: Option<NaiveDate>,
) -> Outcome {
    outcome_from_bars_on(bars, nifty, close_date, None)
}

/// Outcome from daily bars `(date, open, close)` of the issue and of NIFTY
/// 50, requiring an exact bar on `listing_date` when `listing_date` is known.
pub fn outcome_from_bars_on(
    bars: &[(NaiveDate, f64, f64)],
    nifty: &[(NaiveDate, f64, f64)],
    close_date: Option<NaiveDate>,
    listing_date: Option<NaiveDate>,
) -> Outcome {
    let first = match listing_date {
        Some(expected) => bars.iter().position(|(d, _, _)| *d == expected),
        None => {
            let after = close_date.unwrap_or(NaiveDate::MIN);
            bars.iter().position(|(d, _, _)| *d > after)
        }
    };
    let Some(first) = first else {
        return Outcome {
            listing_date,
            ..Outcome::default()
        };
    };
    let (ld, open, close) = bars[first];
    let three = bars.get(first + THREE_MONTH_SESSIONS).map(|b| b.2);
    let nifty_before = nifty.iter().rev().find(|(d, _, _)| *d < ld).map(|b| b.2);
    let nifty_3m = bars
        .get(first + THREE_MONTH_SESSIONS)
        .and_then(|(d3, _, _)| nifty.iter().rev().find(|(d, _, _)| d == d3))
        .map(|b| b.2);
    Outcome {
        listing_date: Some(ld),
        listing_open: Some(open),
        listing_close: Some(close),
        close_3m: three,
        nifty_before,
        nifty_3m,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn entry(date: &str, sym: &str, verdict: &str) -> Entry {
        Entry {
            date: d(date),
            symbol: sym.into(),
            board: "mainboard".into(),
            verdict: verdict.into(),
            issue_price: Some(100.0),
            close_date: Some(d("2026-10-10")),
            listing_date: None,
            expected_gain_pct: Some(12.0),
            p_loss: Some(0.2),
            qib_x: Some(5.0),
            coverage: 80.0,
            methodology: "ipo-1".into(),
        }
    }

    #[test]
    fn one_entry_per_issue_per_day_latest_wins() {
        let mut log = Vec::new();
        assert_eq!(merge(&mut log, vec![entry("2026-10-08", "A", "WAIT")]), 1);
        assert_eq!(merge(&mut log, vec![entry("2026-10-08", "A", "WAIT")]), 0);
        assert_eq!(merge(&mut log, vec![entry("2026-10-08", "A", "APPLY")]), 1);
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].verdict, "APPLY");
        merge(&mut log, vec![entry("2026-10-10", "A", "AVOID")]);
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn final_verdict_is_the_last_before_close() {
        let log = vec![
            entry("2026-10-08", "A", "WAIT"),
            entry("2026-10-10", "A", "APPLY"),
            // Logged after close (the board was opened again): not the verdict.
            entry("2026-10-12", "A", "AVOID"),
            // Post-close with no earlier entry: still excluded.
            entry("2026-10-12", "POST_ONLY", "AVOID"),
            entry("2026-10-12", "B", "LISTED"),
            entry("2026-09-01", "C", "AVOID"),
        ];
        let f = final_verdicts(&log, d("2026-10-01"));
        assert_eq!(f.len(), 1);
        assert_eq!(
            (f[0].symbol.as_str(), f[0].verdict.as_str()),
            ("A", "APPLY")
        );
    }

    #[test]
    fn outcome_reads_listing_day_and_three_months() {
        let start = d("2026-10-13");
        let bars: Vec<(NaiveDate, f64, f64)> = (0..70)
            .map(|i| {
                (
                    start + chrono::Duration::days(i),
                    120.0 + i as f64,
                    125.0 + i as f64,
                )
            })
            .collect();
        let nifty: Vec<(NaiveDate, f64, f64)> = (-2..70)
            .map(|i| {
                (
                    start + chrono::Duration::days(i),
                    0.0,
                    20_000.0 + i as f64 * 10.0,
                )
            })
            .collect();
        let o = outcome_from_bars(&bars, &nifty, Some(d("2026-10-10")));
        assert_eq!(o.listing_date, Some(start));
        assert_eq!(
            (o.listing_open, o.listing_close),
            (Some(120.0), Some(125.0))
        );
        assert_eq!(o.close_3m, Some(125.0 + 63.0));
        assert_eq!(o.nifty_before, Some(19_990.0));
        assert_eq!(o.nifty_3m, Some(20_630.0));
        // Too early for three months.
        let o = outcome_from_bars(&bars[..10], &nifty, Some(d("2026-10-10")));
        assert_eq!(o.close_3m, None);
        // Known listing date without a matching bar does not substitute a later session.
        let missing_day1 =
            outcome_from_bars_on(&bars[1..], &nifty, Some(d("2026-10-10")), Some(start));
        assert_eq!(missing_day1.listing_date, Some(start));
        assert_eq!(missing_day1.listing_close, None);
        assert_eq!(missing_day1.close_3m, None);
        // NIFTY stopping before the stock's 63-session endpoint leaves nifty_3m absent.
        let nifty_short: Vec<(NaiveDate, f64, f64)> = nifty
            .iter()
            .copied()
            .filter(|(d, _, _)| *d < start + chrono::Duration::days(63))
            .collect();
        let o_short = outcome_from_bars_on(&bars, &nifty_short, Some(d("2026-10-10")), Some(start));
        assert_eq!(o_short.close_3m, Some(125.0 + 63.0));
        assert_eq!(o_short.nifty_3m, None);
        // Not listed yet.
        assert_eq!(outcome_from_bars(&[], &nifty, None), Outcome::default());
    }

    #[test]
    fn review_buckets_by_verdict() {
        let finals = vec![
            entry("2026-10-10", "A", "APPLY"),
            entry("2026-10-10", "B", "APPLY"),
            entry("2026-10-10", "C", "AVOID"),
            entry("2026-10-10", "D", "WAIT"),
        ];
        let mut out = HashMap::new();
        let o = |close: f64, three: Option<f64>| Outcome {
            listing_date: Some(d("2026-10-15")),
            listing_open: Some(close),
            listing_close: Some(close),
            close_3m: three,
            nifty_before: Some(100.0),
            nifty_3m: Some(105.0),
        };
        out.insert("A".to_string(), o(130.0, Some(140.0)));
        out.insert("B".to_string(), o(95.0, None));
        out.insert("C".to_string(), o(90.0, Some(80.0)));
        let r = review(&finals, &out, d("2026-10-01"));
        assert_eq!(r.buckets.len(), 3);
        let apply = &r.buckets[0];
        assert_eq!(apply.verdict, "APPLY");
        assert_eq!((apply.issues, apply.listed, apply.with_3m), (2, 2, 1));
        assert_eq!(apply.hit_rate, Some(0.5));
        let near = |v: Option<f64>, want: f64| assert!((v.unwrap() - want).abs() < 1e-9, "{v:?}");
        near(apply.mean_listing_pct, 12.5);
        near(apply.mean_3m_pct, 40.0);
        near(apply.mean_excess_3m_pct, 35.0);
        let avoid = r.buckets.iter().find(|b| b.verdict == "AVOID").unwrap();
        assert_eq!(avoid.hit_rate, Some(0.0));
        assert_eq!(r.pending, ["D"]);
        assert_eq!(r.methodologies, ["ipo-1"]);
    }
}
