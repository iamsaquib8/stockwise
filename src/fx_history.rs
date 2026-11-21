//! Historical exchange rates: what a currency was worth on the day money
//! moved, so a US share bought with rupees is costed at that day's rupees,
//! not today's.
//!
//! Rates are the ECB's daily reference rates from Frankfurter
//! ([`crate::providers::frankfurter`]), kept in `cache.db` (`ohlcv_bars`,
//! interval `ecb1d`) so each day is fetched once. A date with no rate (a
//! weekend, a TARGET holiday) takes the nearest earlier published day, up to
//! [`MAX_GAP_DAYS`] back. Dates before the first rate have none: callers
//! fall back to today's rate and say so.

use crate::providers::{Provider, ProviderKind, frankfurter};
use chrono::NaiveDate;
use std::collections::BTreeMap;

/// How far back a missing day may borrow the last published rate.
pub const MAX_GAP_DAYS: i64 = 10;

/// The `ohlcv_bars` interval the reference rates are cached under.
const CACHE_INTERVAL: &str = "ecb1d";

/// Daily rates `(date, units of quote per unit of base)`, oldest first.
pub type Points = Vec<(NaiveDate, f64)>;

/// The rate on `date`, or on the nearest earlier day with one (up to
/// [`MAX_GAP_DAYS`] back). `points` must be sorted oldest first.
pub fn rate_at(points: &[(NaiveDate, f64)], date: NaiveDate) -> Option<f64> {
    let idx = points.partition_point(|(d, _)| *d <= date);
    let (d, r) = *points.get(idx.checked_sub(1)?)?;
    ((date - d).num_days() <= MAX_GAP_DAYS && r.is_finite() && r > 0.0).then_some(r)
}

/// Historical rates into one base currency, for every currency loaded.
#[derive(Debug, Clone, Default)]
pub struct FxHistory {
    pub base: String,
    /// Major currency (`USD`, `GBP`) → units of base per unit, by day.
    series: BTreeMap<String, Points>,
}

impl FxHistory {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.to_uppercase(),
            series: BTreeMap::new(),
        }
    }

    /// Set the daily rates of `currency` into the base (tests, the cache).
    pub fn insert(&mut self, currency: &str, mut points: Points) {
        points.sort_by_key(|p| p.0);
        points.dedup_by_key(|p| p.0);
        self.series.insert(currency.to_uppercase(), points);
    }

    pub fn has(&self, currency: &str) -> bool {
        let (_, major) = crate::market::to_major(1.0, currency);
        major.eq_ignore_ascii_case(&self.base) || self.series.contains_key(&major.to_uppercase())
    }

    /// Units of base per unit of `currency` on `date` (pence handled:
    /// `GBp` is a hundredth of the GBP rate). `None` when no rate is known
    /// for that day.
    pub fn rate_on(&self, currency: &str, date: NaiveDate) -> Option<f64> {
        let (per_unit, major) = crate::market::to_major(1.0, currency);
        if major.eq_ignore_ascii_case(&self.base) {
            return Some(per_unit);
        }
        let points = self.series.get(&major.to_uppercase())?;
        rate_at(points, date).map(|r| r * per_unit)
    }

    /// `amount` in `currency` on `date`, in the base currency.
    pub fn convert_on(&self, amount: f64, currency: &str, date: NaiveDate) -> Option<f64> {
        self.rate_on(currency, date).map(|r| amount * r)
    }

    /// The first day with a rate for `currency`.
    pub fn first_day(&self, currency: &str) -> Option<NaiveDate> {
        let (_, major) = crate::market::to_major(1.0, currency);
        self.series
            .get(&major.to_uppercase())
            .and_then(|p| p.first())
            .map(|p| p.0)
    }

    /// Load the rates of `needs` (currency, earliest date wanted) into
    /// `base`, from `cache.db` and Frankfurter. Currencies the ECB does not
    /// publish, and a network failure with nothing cached, leave that
    /// currency without history (callers fall back to today's rate).
    pub async fn load(base: &str, needs: &[(String, NaiveDate)]) -> Self {
        let mut hist = Self::new(base);
        let mut wanted: BTreeMap<String, NaiveDate> = BTreeMap::new();
        for (cur, from) in needs {
            let (_, major) = crate::market::to_major(1.0, cur);
            let major = major.to_uppercase();
            if major == hist.base {
                continue;
            }
            let e = wanted.entry(major).or_insert(*from);
            *e = (*e).min(*from);
        }
        for (cur, from) in wanted {
            if let Some(points) = daily_rates(&cur, &hist.base, from).await {
                hist.insert(&cur, points);
            }
        }
        hist
    }
}

/// The rate from `from` to `to` on `date` (nearest earlier published day):
/// units of `to` per unit of `from`.
pub async fn rate_on(date: NaiveDate, from: &str, to: &str) -> Option<f64> {
    let hist = FxHistory::load(to, &[(from.to_string(), date)]).await;
    hist.rate_on(from, date)
}

fn cache_key(from: &str, to: &str) -> String {
    format!("FX:{from}{to}")
}

fn ts_of(d: NaiveDate) -> i64 {
    d.and_hms_opt(0, 0, 0)
        .map(|t| t.and_utc().timestamp())
        .unwrap_or_default()
}

fn day_of(ts: i64) -> Option<NaiveDate> {
    chrono::DateTime::from_timestamp(ts, 0).map(|d| d.date_naive())
}

/// Whether `points` (oldest first) cover `from` up to about `today`.
fn covers(points: &[(NaiveDate, f64)], from: NaiveDate, today: NaiveDate) -> bool {
    // The ECB's first rates are from 4 Jan 1999.
    let ecb_start = NaiveDate::from_ymd_opt(1999, 1, 4).unwrap_or(from);
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return false;
    };
    first.0 <= from.max(ecb_start) + chrono::Duration::days(MAX_GAP_DAYS)
        && last.0 >= today - chrono::Duration::days(4)
}

/// Daily `from`→`to` rates since `from_day` (a little earlier, so the first
/// day can borrow the previous rate): the cache when it covers the range,
/// else Frankfurter (saved to the cache), else whatever the cache holds.
async fn daily_rates(from: &str, to: &str, from_day: NaiveDate) -> Option<Points> {
    let today = chrono::Local::now().date_naive();
    let start = from_day - chrono::Duration::days(MAX_GAP_DAYS);
    let key = cache_key(from, to);
    let cached: Points = crate::db::with_db(|db| {
        db.get_bars(&key, CACHE_INTERVAL, ts_of(start), ts_of(today) + 86_400)
    })
    .unwrap_or_default()
    .into_iter()
    .filter_map(|b| Some((day_of(b.ts)?, b.close)))
    .filter(|(_, r)| r.is_finite() && *r > 0.0)
    .collect();
    if covers(&cached, from_day, today) || cfg!(test) {
        return (!cached.is_empty()).then_some(cached);
    }
    let symbol = format!("{from}{to}=X");
    frankfurter::pair(&symbol)?;
    let Provider::Frankfurter(f) = Provider::new(ProviderKind::Frankfurter, String::new(), None)
    else {
        return (!cached.is_empty()).then_some(cached);
    };
    match f.daily(&symbol, Some(start)).await {
        Ok(series) => {
            let points: Points = series
                .bars
                .iter()
                .filter_map(|b| Some((b.date, b.close?)))
                .collect();
            let rows: Vec<crate::db::Bar> = points
                .iter()
                .map(|(d, r)| crate::db::Bar {
                    ts: ts_of(*d),
                    open: *r,
                    high: *r,
                    low: *r,
                    close: *r,
                    volume: 0.0,
                })
                .collect();
            crate::db::with_db(|db| db.put_bars(&key, CACHE_INTERVAL, &rows));
            (!points.is_empty()).then_some(points)
        }
        Err(_) => (!cached.is_empty()).then_some(cached),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn a_missing_day_takes_the_nearest_earlier_rate() {
        let pts = vec![(d("2024-01-05"), 83.0), (d("2024-01-08"), 83.2)];
        assert_eq!(rate_at(&pts, d("2024-01-05")), Some(83.0));
        // Saturday and Sunday borrow Friday's rate.
        assert_eq!(rate_at(&pts, d("2024-01-07")), Some(83.0));
        assert_eq!(rate_at(&pts, d("2024-01-09")), Some(83.2));
        // Before the first rate, and long after the last: none.
        assert_eq!(rate_at(&pts, d("2024-01-04")), None);
        assert_eq!(rate_at(&pts, d("2024-02-28")), None);
    }

    #[test]
    fn pence_and_the_base_itself_convert() {
        let mut h = FxHistory::new("INR");
        h.insert("GBP", vec![(d("2024-01-05"), 105.0)]);
        assert_eq!(h.rate_on("INR", d("2020-01-01")), Some(1.0));
        assert_eq!(h.rate_on("GBP", d("2024-01-06")), Some(105.0));
        assert_eq!(h.rate_on("GBp", d("2024-01-06")), Some(1.05));
        assert_eq!(h.rate_on("USD", d("2024-01-06")), None);
        assert!(h.has("GBp") && h.has("INR") && !h.has("USD"));
    }

    #[test]
    fn coverage_needs_both_ends() {
        let today = d("2026-11-20");
        let pts = vec![(d("2024-01-02"), 83.0), (d("2026-11-19"), 84.0)];
        assert!(covers(&pts, d("2024-01-01"), today));
        assert!(!covers(&pts, d("2023-06-01"), today));
        assert!(!covers(&pts, d("2024-01-01"), d("2026-12-31")));
        assert!(!covers(&[], d("2024-01-01"), today));
    }
}
