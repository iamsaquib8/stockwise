//! Where the numbers came from (issue #338).
//!
//! Every command that shows fundamentals or price-based figures carries a
//! [`Meta`]: the sources that supplied data, when the data is from, which key
//! fields are still missing after every fallback was tried, what failed on
//! the way, and how much of the key data is present (`coverage`, 0 to 100).
//!
//! Text output ends with one line built by [`Meta::footer`]:
//!
//! ```text
//! Sources: Yahoo Finance, SEC EDGAR · as of 2026-11-21 15:30 · missing: ROE, D/E
//! ```
//!
//! `--json` output has the same data as a `meta` object. A later rule layer
//! reads `coverage` and declines to make a call when too much is missing, so
//! a field that was silently defaulted would hide exactly what it needs.

use crate::api::Quote;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Source names, as they appear in `meta.sources` and the footer.
pub const YAHOO: &str = "Yahoo Finance";
pub const EDGAR: &str = "SEC EDGAR";
pub const NSE: &str = "NSE";
pub const NSE_RESULTS: &str = "NSE results";
pub const BSE: &str = "BSE";
pub const CACHE: &str = "local cache";

/// Provenance of one answer.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Meta {
    /// Every source that supplied at least one value, in the order they
    /// answered (`["Yahoo Finance", "SEC EDGAR"]`).
    pub sources: Vec<String>,
    /// When the data is from: the quote time, or the newest bar or filing
    /// (RFC 3339, UTC).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub as_of: Option<String>,
    /// Key fields no source could supply ("ROE", "D/E", ...).
    pub missing: Vec<String>,
    /// Sources that failed or data that is stale, in plain words.
    pub warnings: Vec<String>,
    /// Share of the key fields that are present, 0 to 100.
    pub coverage: f64,
    /// Which source supplied each key field that did not come from the first
    /// source (`{"ROE": "SEC EDGAR"}`).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub field_sources: BTreeMap<String, String>,
}

impl Meta {
    /// A meta that starts from `source`.
    pub fn from_source(source: &str) -> Self {
        let mut m = Meta::default();
        m.add_source(source);
        m
    }

    /// Name `source` once, keeping the order sources answered in.
    pub fn add_source(&mut self, source: &str) {
        if !source.is_empty() && !self.sources.iter().any(|s| s == source) {
            self.sources.push(source.to_string());
        }
    }

    /// Record that `source` supplied `field`.
    pub fn record(&mut self, field: &str, source: &str) {
        self.add_source(source);
        self.field_sources
            .insert(field.to_string(), source.to_string());
    }

    /// Add a warning once.
    pub fn warn(&mut self, warning: impl Into<String>) {
        let w = warning.into();
        if !self.warnings.contains(&w) {
            self.warnings.push(w);
        }
    }

    /// Set `as_of` from Unix seconds, keeping a newer value already there.
    pub fn set_as_of_unix(&mut self, ts: i64) {
        let Some(t) = chrono::DateTime::from_timestamp(ts, 0) else {
            return;
        };
        let new = t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        if self.as_of.as_deref().is_none_or(|old| new.as_str() > old) {
            self.as_of = Some(new);
        }
    }

    /// Set `missing` and `coverage` from the checks of every key field.
    pub fn set_fields(&mut self, checks: &[(&str, bool)]) {
        self.missing = checks
            .iter()
            .filter(|(_, present)| !present)
            .map(|(name, _)| name.to_string())
            .collect();
        self.coverage = coverage_pct(checks.iter().filter(|(_, p)| *p).count(), checks.len());
    }

    /// Mark one more field missing (and lower the coverage accordingly),
    /// for data checked outside [`Meta::set_fields`] such as price history.
    pub fn add_missing(&mut self, field: &str, total_fields: usize) {
        if !self.missing.iter().any(|m| m == field) {
            self.missing.push(field.to_string());
        }
        let total = total_fields.max(self.missing.len());
        self.coverage = coverage_pct(total - self.missing.len(), total);
    }

    /// Fold another meta in: its sources, warnings and field sources.
    pub fn merge(&mut self, other: &Meta) {
        for s in &other.sources {
            self.add_source(s);
        }
        for w in &other.warnings {
            self.warn(w.clone());
        }
        for (k, v) in &other.field_sources {
            self.field_sources.entry(k.clone()).or_insert(v.clone());
        }
        if let Some(a) = &other.as_of
            && self.as_of.as_deref().is_none_or(|old| a.as_str() > old)
        {
            self.as_of = Some(a.clone());
        }
    }

    /// The shared one-line footer:
    /// `Sources: Yahoo Finance, SEC EDGAR · as of 2026-11-21 15:30 · missing: ROE, D/E`.
    /// Parts without data are left out; `as_of` is shown in local time.
    pub fn footer(&self) -> String {
        let sources = if self.sources.is_empty() {
            "none".to_string()
        } else {
            self.sources.join(", ")
        };
        let mut parts = vec![format!("Sources: {sources}")];
        if let Some(t) = self.as_of.as_deref().and_then(local_time) {
            parts.push(format!("as of {t}"));
        }
        if !self.missing.is_empty() {
            parts.push(format!("missing: {}", self.missing.join(", ")));
        }
        parts.join(" · ")
    }
}

fn coverage_pct(present: usize, total: usize) -> f64 {
    if total == 0 {
        return 100.0;
    }
    (present as f64 / total as f64 * 1000.0).round() / 10.0
}

/// An RFC 3339 instant as `YYYY-MM-DD HH:MM` in the local time zone; a bare
/// date stays a date.
fn local_time(s: &str) -> Option<String> {
    if let Ok(t) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(
            t.with_timezone(&chrono::Local)
                .format("%Y-%m-%d %H:%M")
                .to_string(),
        );
    }
    chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .map(|d| d.to_string())
}

// ── Key fields ──

/// Field names, as they appear in `meta.missing` and `meta.field_sources`.
pub const F_MARKET_CAP: &str = "market cap";
pub const F_PE: &str = "P/E";
pub const F_EPS: &str = "EPS";
pub const F_PB: &str = "P/B";
pub const F_ROE: &str = "ROE";
pub const F_DE: &str = "D/E";
pub const F_MARGIN: &str = "profit margin";
pub const F_GROWTH: &str = "revenue growth";
pub const F_BETA: &str = "beta";
pub const F_SECTOR: &str = "sector";
pub const F_HISTORY: &str = "price history";

/// The fundamentals a stock's score depends on, each with whether `q` has it.
/// Dividend yield is left out: a company that pays none has no value, and
/// that is not missing data.
pub fn stock_field_checks(q: &Quote) -> Vec<(&'static str, bool)> {
    let ok = |v: Option<f64>| v.is_some_and(f64::is_finite);
    vec![
        (F_MARKET_CAP, ok(q.market_cap)),
        (
            F_PE,
            ok(q.trailing_pe) || q.eps_trailing_twelve_months.is_some_and(|e| e <= 0.0),
        ),
        (F_EPS, ok(q.eps_trailing_twelve_months)),
        (F_PB, ok(q.price_to_book)),
        (F_ROE, ok(q.return_on_equity)),
        (F_DE, ok(q.debt_to_equity)),
        (F_MARGIN, ok(q.profit_margins)),
        (F_GROWTH, ok(q.revenue_growth)),
        (F_BETA, ok(q.beta)),
        (F_SECTOR, q.sector.as_deref().is_some_and(|s| !s.is_empty())),
    ]
}

/// The fields `stockwise quote` shows for any instrument.
pub fn quote_field_checks(q: &Quote) -> Vec<(&'static str, bool)> {
    let ok = |v: Option<f64>| v.is_some_and(f64::is_finite);
    let mut checks = vec![
        ("price", ok(q.regular_market_price)),
        ("previous close", ok(q.regular_market_previous_close)),
    ];
    if crate::market::AssetClass::of(q.symbol.as_deref().unwrap_or_default())
        == crate::market::AssetClass::Stock
    {
        checks.push((F_MARKET_CAP, ok(q.market_cap)));
        checks.push((
            F_PE,
            ok(q.trailing_pe) || q.eps_trailing_twelve_months.is_some_and(|e| e <= 0.0),
        ));
        checks.push((F_EPS, ok(q.eps_trailing_twelve_months)));
    }
    checks
}

/// The sources named on a quote: its merged `meta.sources`, else the
/// provider that priced it, else Yahoo.
pub fn quote_sources(q: &Quote) -> Vec<String> {
    if let Some(m) = &q.meta
        && !m.sources.is_empty()
    {
        return m.sources.clone();
    }
    vec![q.data_source.clone().unwrap_or_else(|| YAHOO.to_string())]
}

/// The meta of a plain quote (`stockwise quote`): its sources, its time and
/// the quote fields it lacks.
pub fn for_quote(q: &Quote) -> Meta {
    let mut m = q.meta.clone().unwrap_or_default();
    for s in quote_sources(q) {
        m.add_source(&s);
    }
    if let Some(ts) = q.regular_market_time {
        m.set_as_of_unix(ts);
    }
    m.set_fields(&quote_field_checks(q));
    m
}

/// The meta of a stock's fundamentals: sources, time, and the key fields.
pub fn for_fundamentals(q: &Quote) -> Meta {
    let mut m = for_quote(q);
    m.set_fields(&stock_field_checks(q));
    m
}

/// The meta of a scan over many stocks (`picks`, `longterm`): every source
/// named on a row, the newest quote time, and per key field how many rows
/// have it. A field fewer than half the rows have is listed in `missing`;
/// any field some rows lack gets a warning with the count. `coverage` is
/// the mean of the rows' coverage.
pub fn for_scan<'a>(quotes: impl IntoIterator<Item = &'a Quote>) -> Meta {
    let mut m = Meta::default();
    let mut rows = 0usize;
    let mut present: Vec<(&'static str, usize)> = Vec::new();
    let mut coverage_sum = 0.0;
    for q in quotes {
        rows += 1;
        for s in quote_sources(q) {
            m.add_source(&s);
        }
        if let Some(ts) = q.regular_market_time {
            m.set_as_of_unix(ts);
        }
        if let Some(qm) = &q.meta {
            for w in &qm.warnings {
                m.warn(w.clone());
            }
        }
        let checks = stock_field_checks(q);
        coverage_sum += coverage_pct(checks.iter().filter(|(_, ok)| *ok).count(), checks.len());
        for (name, ok) in checks {
            match present.iter_mut().find(|(n, _)| *n == name) {
                Some(slot) => slot.1 += ok as usize,
                None => present.push((name, ok as usize)),
            }
        }
    }
    if rows == 0 {
        return m;
    }
    for (name, have) in &present {
        if have * 2 < rows {
            m.missing.push(name.to_string());
        }
        if *have < rows {
            m.warn(format!(
                "{name} missing for {} of {rows} stocks",
                rows - have
            ));
        }
    }
    m.coverage = (coverage_sum / rows as f64 * 10.0).round() / 10.0;
    m
}

/// The meta for `picks --json` and the picks footer: [`for_scan`] over the
/// quotes the ranking used.
pub fn for_picks(quotes: &[Quote]) -> Meta {
    for_scan(quotes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_lists_fields_most_rows_lack_and_counts_the_rest() {
        let full = Quote {
            symbol: Some("A.NS".into()),
            market_cap: Some(1.0),
            trailing_pe: Some(10.0),
            eps_trailing_twelve_months: Some(1.0),
            price_to_book: Some(1.0),
            return_on_equity: Some(0.1),
            debt_to_equity: Some(10.0),
            profit_margins: Some(0.1),
            revenue_growth: Some(0.1),
            beta: Some(1.0),
            sector: Some("X".into()),
            ..Default::default()
        };
        let v7 = Quote {
            symbol: Some("B.NS".into()),
            market_cap: Some(1.0),
            trailing_pe: Some(10.0),
            eps_trailing_twelve_months: Some(1.0),
            price_to_book: Some(1.0),
            ..Default::default()
        };
        let m = for_picks(&[full, v7.clone(), v7]);
        assert_eq!(m.sources, vec![YAHOO]);
        assert_eq!(
            m.missing,
            vec![F_ROE, F_DE, F_MARGIN, F_GROWTH, F_BETA, F_SECTOR]
        );
        assert!(
            m.warnings
                .contains(&"ROE missing for 2 of 3 stocks".to_string())
        );
        assert_eq!(m.coverage, 60.0);
        assert_eq!(for_picks(&[]).coverage, 0.0);
    }

    #[test]
    fn footer_names_sources_time_and_missing_fields() {
        let mut m = Meta::from_source(YAHOO);
        m.record(F_ROE, EDGAR);
        m.as_of = Some("2026-11-21".into());
        m.set_fields(&[(F_PE, true), (F_ROE, true), (F_DE, false), (F_BETA, false)]);
        assert_eq!(
            m.footer(),
            "Sources: Yahoo Finance, SEC EDGAR · as of 2026-11-21 · missing: D/E, beta"
        );
        assert_eq!(m.coverage, 50.0);
        assert_eq!(m.field_sources.get(F_ROE).map(String::as_str), Some(EDGAR));
    }

    #[test]
    fn footer_leaves_out_parts_without_data() {
        let mut m = Meta::from_source(NSE);
        m.set_fields(&[(F_PE, true)]);
        assert_eq!(m.footer(), "Sources: NSE");
        assert_eq!(m.coverage, 100.0);
        assert_eq!(Meta::default().footer(), "Sources: none");
    }

    #[test]
    fn footer_shows_an_instant_in_local_time() {
        let mut m = Meta::from_source(YAHOO);
        m.set_as_of_unix(1_795_000_000);
        let expected = chrono::DateTime::from_timestamp(1_795_000_000, 0)
            .unwrap()
            .with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string();
        assert_eq!(
            m.footer(),
            format!("Sources: Yahoo Finance · as of {expected}")
        );
    }

    #[test]
    fn sources_and_warnings_are_named_once() {
        let mut m = Meta::from_source(YAHOO);
        m.add_source(YAHOO);
        m.warn("SEC EDGAR unavailable");
        m.warn("SEC EDGAR unavailable");
        let mut other = Meta::from_source(EDGAR);
        other.warn("x");
        m.merge(&other);
        assert_eq!(m.sources, vec![YAHOO, EDGAR]);
        assert_eq!(m.warnings, vec!["SEC EDGAR unavailable", "x"]);
    }

    #[test]
    fn a_missing_history_lowers_coverage() {
        let mut m = Meta::default();
        m.set_fields(&[(F_PE, true), (F_ROE, true), (F_DE, true)]);
        m.add_missing(F_HISTORY, 4);
        assert_eq!(m.missing, vec![F_HISTORY]);
        assert_eq!(m.coverage, 75.0);
    }

    #[test]
    fn a_loss_maker_without_a_pe_is_not_missing_one() {
        let q = Quote {
            eps_trailing_twelve_months: Some(-2.0),
            ..Default::default()
        };
        let checks = stock_field_checks(&q);
        assert!(checks.iter().any(|(f, ok)| *f == F_PE && *ok));
    }

    #[test]
    fn json_has_stable_keys() {
        let m = Meta::from_source(YAHOO);
        let v = serde_json::to_value(&m).unwrap();
        for k in ["sources", "missing", "warnings", "coverage"] {
            assert!(v.get(k).is_some(), "{k}");
        }
    }
}
