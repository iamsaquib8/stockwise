//! Split and bonus adjustment for price history that arrives unadjusted.
//!
//! Yahoo's `close` is already adjusted for splits. NSE's and BSE's daily
//! history and Alpha Vantage's free daily series are not: after a 1:1 bonus
//! the bars before the ex-date sit at twice the price, and every return,
//! volatility and drawdown figure computed across it shows a fake 50% crash.
//! [`adjust_chart`] divides the prices before each ex-date by the event's
//! factor (and multiplies the volumes), the way Yahoo does.
//!
//! A source that adjusted already must not be adjusted twice, so an event is
//! applied only when the bars around its ex-date still show the jump: the
//! last close before it and the first close on or after it differ by
//! something nearer the factor than 1.

use crate::api::ChartData;
use chrono::NaiveDate;

/// One split or bonus: `factor` new shares per old share (2:1 split = 2,
/// 1:1 bonus = 2, 10 → 2 face value = 5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SplitEvent {
    pub ex_date: NaiveDate,
    pub factor: f64,
}

/// The share factor of an NSE corporate-action subject, for splits and
/// bonuses only:
///
/// - `Face Value Split (Sub-Division) - From Rs 10/- Per Share To Rs 2/- Per Share` → 5
/// - `Bonus 1:1` → 2; `Bonus 3:2` (3 new for every 2 held) → 2.5
///
/// Dividends, rights and anything unreadable give `None`.
pub fn nse_action_factor(subject: &str) -> Option<f64> {
    let s = subject.to_ascii_lowercase();
    if s.contains("bonus") {
        let (a, b) = ratio_after(&s, "bonus")?;
        return (a > 0.0 && b > 0.0).then_some((a + b) / b);
    }
    if s.contains("split") || s.contains("sub-division") || s.contains("subdivision") {
        let from = number_after(&s, "from")?;
        let to = number_after(&s[s.find("from")? + 4..], " to ")?;
        return (from > 0.0 && to > 0.0 && from > to).then_some(from / to);
    }
    None
}

/// The first `a:b` after `word`.
fn ratio_after(s: &str, word: &str) -> Option<(f64, f64)> {
    let rest = &s[s.find(word)? + word.len()..];
    let colon = rest.find(':')?;
    let a: String = rest[..colon]
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let b: String = rest[colon + 1..]
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    Some((a.trim().parse().ok()?, b.parse().ok()?))
}

/// The first number after `word`.
fn number_after(s: &str, word: &str) -> Option<f64> {
    let rest = &s[s.find(word)? + word.len()..];
    let start = rest.find(|c: char| c.is_ascii_digit())?;
    let digits: String = rest[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    digits.trim_end_matches('.').parse().ok()
}

/// Events from Yahoo's chart `events.splits` (a bonus shows as a split).
pub fn from_yahoo(events: &crate::api::ChartEvents) -> Vec<SplitEvent> {
    let mut out: Vec<SplitEvent> = events
        .splits
        .values()
        .filter(|s| s.numerator > 0.0 && s.denominator > 0.0 && s.numerator != s.denominator)
        .filter_map(|s| {
            Some(SplitEvent {
                ex_date: chrono::DateTime::from_timestamp(s.date, 0)?.date_naive(),
                factor: s.numerator / s.denominator,
            })
        })
        .collect();
    out.sort_by_key(|e| e.ex_date);
    out
}

/// The session date of each bar (`gmtoffset` east of UTC).
fn bar_dates(chart: &ChartData) -> Vec<Option<NaiveDate>> {
    let offset = chart.meta.as_ref().and_then(|m| m.gmtoffset).unwrap_or(0);
    chart
        .timestamp
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|t| chrono::DateTime::from_timestamp(t + offset, 0).map(|d| d.date_naive()))
        .collect()
}

/// Whether the closes around `ex` still jump by about `factor`.
fn shows_jump(
    dates: &[Option<NaiveDate>],
    closes: &[Option<f64>],
    ex: NaiveDate,
    factor: f64,
) -> bool {
    let before = (0..dates.len())
        .rev()
        .find(|&i| dates[i].is_some_and(|d| d < ex) && closes.get(i).copied().flatten().is_some());
    let after = (0..dates.len())
        .find(|&i| dates[i].is_some_and(|d| d >= ex) && closes.get(i).copied().flatten().is_some());
    let (Some(b), Some(a)) = (before, after) else {
        // No bar on one side: nothing to adjust, or an action that has not
        // taken effect in these bars yet. Neither is applied.
        return false;
    };
    let (Some(pb), Some(pa)) = (closes[b], closes[a]) else {
        return false;
    };
    if pb <= 0.0 || pa <= 0.0 {
        return false;
    }
    let jump = (pb / pa).ln();
    (jump - factor.ln()).abs() < jump.abs()
}

/// The first bar whose close is less than 55% or more than 180% of the
/// previous one: in daily stock bars, the mark of a split or bonus left
/// unadjusted (bars stored before and after it on different bases).
pub fn split_like_jump(chart: &ChartData) -> Option<(NaiveDate, f64)> {
    let dates = bar_dates(chart);
    let closes = chart.indicators.quote.first()?.close.as_deref()?;
    let mut prev: Option<f64> = None;
    for (i, c) in closes.iter().enumerate() {
        let Some(c) = c.filter(|v| *v > 0.0) else {
            continue;
        };
        if let Some(p) = prev {
            let ratio = c / p;
            if !(0.55..=1.8).contains(&ratio) {
                return Some((dates.get(i).copied().flatten()?, ratio));
            }
        }
        prev = Some(c);
    }
    None
}

/// Adjust the bars before each event's ex-date. Returns the events applied.
pub fn adjust_chart(chart: &mut ChartData, events: &[SplitEvent]) -> usize {
    let dates = bar_dates(chart);
    let Some(q) = chart.indicators.quote.first_mut() else {
        return 0;
    };
    let closes = q.close.clone().unwrap_or_default();
    let mut applied = 0;
    for e in events.iter().filter(|e| e.factor > 0.0 && e.factor != 1.0) {
        if !shows_jump(&dates, &closes, e.ex_date, e.factor) {
            continue;
        }
        applied += 1;
        let before = |i: usize| {
            dates
                .get(i)
                .copied()
                .flatten()
                .is_some_and(|d| d < e.ex_date)
        };
        for series in [&mut q.open, &mut q.high, &mut q.low, &mut q.close] {
            if let Some(v) = series.as_mut() {
                for (i, p) in v.iter_mut().enumerate() {
                    if before(i)
                        && let Some(x) = p.as_mut()
                    {
                        *x /= e.factor;
                    }
                }
            }
        }
        if let Some(v) = q.volume.as_mut() {
            for (i, vol) in v.iter_mut().enumerate() {
                if before(i)
                    && let Some(x) = vol.as_mut()
                {
                    *x = (*x as f64 * e.factor).round() as u64;
                }
            }
        }
    }
    applied
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Indicators, QuoteIndicator};

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    fn chart(days: &[&str], closes: &[f64]) -> ChartData {
        let noon = |s: &str| d(s).and_hms_opt(12, 0, 0).unwrap().and_utc().timestamp();
        let some = |v: &[f64]| Some(v.iter().map(|c| Some(*c)).collect::<Vec<_>>());
        ChartData {
            meta: None,
            timestamp: Some(days.iter().map(|s| noon(s)).collect()),
            indicators: Indicators {
                quote: vec![QuoteIndicator {
                    open: some(closes),
                    high: some(closes),
                    low: some(closes),
                    close: some(closes),
                    volume: Some(closes.iter().map(|_| Some(100)).collect()),
                }],
                adjclose: None,
            },
            events: None,
        }
    }

    fn closes(c: &ChartData) -> Vec<f64> {
        c.indicators.quote[0]
            .close
            .as_ref()
            .unwrap()
            .iter()
            .flatten()
            .copied()
            .collect()
    }

    #[test]
    fn nse_subjects_give_split_and_bonus_factors() {
        assert_eq!(
            nse_action_factor(
                "Face Value Split (Sub-Division) - From Rs 10/- Per Share To Rs 2/- Per Share"
            ),
            Some(5.0)
        );
        assert_eq!(
            nse_action_factor(
                "Face Value Split (Sub-Division) - From Rs 2/- Per Share To Re 1/- Per Share"
            ),
            Some(2.0)
        );
        assert_eq!(nse_action_factor("Bonus 1:1"), Some(2.0));
        assert_eq!(nse_action_factor("Bonus 3:2"), Some(2.5));
        assert_eq!(
            nse_action_factor("Interim Dividend - Rs 10 Per Share"),
            None
        );
        assert_eq!(nse_action_factor("Rights 1:5 @ Premium Rs 1247/-"), None);
    }

    #[test]
    fn bars_before_a_bonus_are_halved_and_volumes_doubled() {
        // RELIANCE-style 1:1 bonus: ₹3000 → ₹1500 on the ex-date.
        let mut c = chart(
            &["2024-10-25", "2024-10-28", "2024-10-29"],
            &[3000.0, 3010.0, 1510.0],
        );
        let n = adjust_chart(
            &mut c,
            &[SplitEvent {
                ex_date: d("2024-10-28"),
                factor: 2.0,
            }],
        );
        // The jump is between 10-28 and 10-29 here, not at the given ex-date
        // 10-28: 3000 → 3010 shows no jump, so nothing is applied.
        assert_eq!(n, 0);
        let n = adjust_chart(
            &mut c,
            &[SplitEvent {
                ex_date: d("2024-10-29"),
                factor: 2.0,
            }],
        );
        assert_eq!(n, 1);
        assert_eq!(closes(&c), vec![1500.0, 1505.0, 1510.0]);
        assert_eq!(c.indicators.quote[0].volume.as_ref().unwrap()[0], Some(200));
    }

    #[test]
    fn an_announced_action_after_the_last_bar_is_not_applied() {
        let mut c = chart(&["2024-10-25", "2024-10-28"], &[3000.0, 3010.0]);
        let n = adjust_chart(
            &mut c,
            &[SplitEvent {
                ex_date: d("2024-11-15"),
                factor: 2.0,
            }],
        );
        assert_eq!(n, 0);
        assert_eq!(closes(&c), vec![3000.0, 3010.0]);
    }

    #[test]
    fn a_split_sized_jump_is_found() {
        let c = chart(
            &["2024-10-25", "2024-10-28", "2024-10-29"],
            &[3000.0, 3010.0, 1510.0],
        );
        let (day, ratio) = split_like_jump(&c).unwrap();
        assert_eq!(day, d("2024-10-29"));
        assert!((ratio - 1510.0 / 3010.0).abs() < 1e-12);
        let calm = chart(&["2024-10-25", "2024-10-28"], &[3000.0, 2900.0]);
        assert_eq!(split_like_jump(&calm), None);
    }

    #[test]
    fn already_adjusted_bars_are_left_alone() {
        let mut c = chart(&["2024-10-25", "2024-10-28"], &[1500.0, 1510.0]);
        let n = adjust_chart(
            &mut c,
            &[SplitEvent {
                ex_date: d("2024-10-28"),
                factor: 2.0,
            }],
        );
        assert_eq!(n, 0);
        assert_eq!(closes(&c), vec![1500.0, 1510.0]);
    }

    #[test]
    fn yahoo_split_events_become_factors() {
        let mut ev = crate::api::ChartEvents::default();
        ev.splits.insert(
            "1".into(),
            crate::api::SplitEvent {
                date: d("2024-10-28")
                    .and_hms_opt(3, 45, 0)
                    .unwrap()
                    .and_utc()
                    .timestamp(),
                numerator: 2.0,
                denominator: 1.0,
            },
        );
        assert_eq!(
            from_yahoo(&ev),
            vec![SplitEvent {
                ex_date: d("2024-10-28"),
                factor: 2.0
            }]
        );
    }
}
