//! Upcoming results, dividends and splits for a set of symbols, for the
//! `calendar` command and `results-in` alerts.
//!
//! Sources:
//! - Yahoo Finance `quoteSummary` `calendarEvents`, for US and Indian
//!   listings alike: the next earnings (results) date or window, the
//!   ex-dividend date and the dividend payment date.
//! - NSE, for `.NS` / `.BO` stocks: board meetings
//!   (`/api/corporate-board-meetings`), whose purpose says when results are
//!   approved, and corporate actions (`/api/corporates-corporateActions`):
//!   ex-dates of dividends, splits, bonus and rights issues. NSE's API serves
//!   browser sessions: StockWise opens one session per run, waits a second
//!   between requests and caches every answer. When NSE refuses, the
//!   calendar says so and keeps Yahoo's dates.
//!
//! Upcoming splits only come from NSE; Yahoo lists splits after they happen.
//! Answers are cached in `calendar_cache.json` in the data directory — a day
//! for answers, an hour for failures — so a watch loop asks once a day.

use crate::api::{YahooClient, build_url};
use crate::i18n::{Msg, t};
use crate::market::{AssetClass, Market};
use crate::output::CsvRow;
use anyhow::{Result, anyhow, bail};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
use std::time::Duration;

/// What happens on a calendar date.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventKind {
    /// Quarterly or annual results (an earnings date, or a board meeting
    /// that approves results).
    Results,
    /// Any other board meeting (fund raising, buyback, ...).
    BoardMeeting,
    ExDividend,
    /// The dividend's payment date.
    DividendPaid,
    Split,
    Bonus,
    Rights,
    /// Any other corporate action (AGM, buyback, demerger, ...).
    Other,
}

impl EventKind {
    /// The kind in the active language.
    pub fn label(self) -> &'static str {
        t(match self {
            EventKind::Results => Msg::EventResults,
            EventKind::BoardMeeting => Msg::EventBoardMeeting,
            EventKind::ExDividend => Msg::EventExDividend,
            EventKind::DividendPaid => Msg::EventDividendPaid,
            EventKind::Split => Msg::EventSplit,
            EventKind::Bonus => Msg::EventBonus,
            EventKind::Rights => Msg::EventRights,
            EventKind::Other => Msg::EventOther,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Yahoo,
    Nse,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Source::Yahoo => "Yahoo",
            Source::Nse => "NSE",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub symbol: String,
    pub date: NaiveDate,
    pub kind: EventKind,
    /// The source's own words: the board meeting's purpose, the corporate
    /// action's subject, the EPS estimate.
    pub detail: String,
    pub source: Source,
    /// The source marks the date as an estimate.
    #[serde(default)]
    pub estimate: bool,
}

// ── Parsing ──

/// A Yahoo `{raw, fmt}` date.
fn yahoo_date(v: &Value) -> Option<NaiveDate> {
    if let Some(s) = v.get("fmt").and_then(Value::as_str)
        && let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d")
    {
        return Some(d);
    }
    let raw = v
        .get("raw")
        .and_then(Value::as_i64)
        .or_else(|| v.as_i64())?;
    DateTime::from_timestamp(raw, 0).map(|d| d.date_naive())
}

/// Events in a `quoteSummary?modules=calendarEvents` response.
pub fn parse_yahoo(symbol: &str, v: &Value) -> Vec<Event> {
    let Some(cal) = v.pointer("/quoteSummary/result/0/calendarEvents") else {
        return Vec::new();
    };
    let event = |date, kind, detail: String, estimate| Event {
        symbol: symbol.to_string(),
        date,
        kind,
        detail,
        source: Source::Yahoo,
        estimate,
    };
    let mut out = Vec::new();
    if let Some(e) = cal.get("earnings") {
        let dates: Vec<NaiveDate> = e
            .get("earningsDate")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(yahoo_date).collect())
            .unwrap_or_default();
        if let (Some(&first), Some(&last)) = (dates.first(), dates.last()) {
            // Two dates are the window an unconfirmed report falls in.
            let estimate = e
                .get("isEarningsDateEstimate")
                .and_then(Value::as_bool)
                .unwrap_or(last != first);
            let mut detail = Vec::new();
            if last != first {
                detail.push(format!("expected {first} to {last}"));
            }
            if let Some(eps) = e.pointer("/earningsAverage/raw").and_then(Value::as_f64) {
                detail.push(format!("EPS estimate {eps:.2}"));
            }
            out.push(event(
                first,
                EventKind::Results,
                detail.join("; "),
                estimate,
            ));
        }
    }
    if let Some(d) = cal.get("exDividendDate").and_then(yahoo_date) {
        out.push(event(d, EventKind::ExDividend, String::new(), false));
    }
    if let Some(d) = cal.get("dividendDate").and_then(yahoo_date) {
        out.push(event(d, EventKind::DividendPaid, String::new(), false));
    }
    out
}

/// An NSE date such as `08-Oct-2026`; `-` and blanks are none.
fn nse_date(v: Option<&Value>) -> Option<NaiveDate> {
    let s = v?.as_str()?.trim();
    NaiveDate::parse_from_str(s, "%d-%b-%Y").ok()
}

fn text(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// The NSE trading symbol for a Yahoo ticker (`TCS.NS` → `TCS`), or None
/// for anything that isn't an Indian stock.
pub fn nse_symbol(symbol: &str) -> Option<String> {
    let s = symbol.to_uppercase();
    let base = s.strip_suffix(".NS").or_else(|| s.strip_suffix(".BO"))?;
    (!base.is_empty() && !base.starts_with('^')).then(|| base.to_string())
}

/// Board meetings from `/api/corporate-board-meetings`. A meeting whose
/// purpose or description mentions results is a results date.
pub fn parse_nse_meetings(symbol: &str, v: &Value) -> Vec<Event> {
    let nse = nse_symbol(symbol).unwrap_or_default();
    let rows = v
        .as_array()
        .or_else(|| v.get("data").and_then(Value::as_array));
    let mut out: Vec<Event> = Vec::new();
    for row in rows.into_iter().flatten() {
        if !text(row, "bm_symbol").eq_ignore_ascii_case(&nse) {
            continue;
        }
        let Some(date) = nse_date(row.get("bm_date")) else {
            continue;
        };
        let purpose = text(row, "bm_purpose");
        let desc = text(row, "bm_desc");
        let results = format!("{purpose} {desc}")
            .to_lowercase()
            .contains("result");
        let kind = if results {
            EventKind::Results
        } else {
            EventKind::BoardMeeting
        };
        let event = Event {
            symbol: symbol.to_string(),
            date,
            kind,
            detail: purpose,
            source: Source::Nse,
            estimate: false,
        };
        // A meeting is often filed twice: as an "intimation" and with its
        // purpose. Keep one, with the more telling purpose.
        match out.iter_mut().find(|e| e.date == date && e.kind == kind) {
            Some(seen) if is_intimation(&seen.detail) && !is_intimation(&event.detail) => {
                *seen = event
            }
            Some(_) => {}
            None => out.push(event),
        }
    }
    out
}

fn is_intimation(purpose: &str) -> bool {
    purpose.is_empty() || purpose.to_lowercase().contains("intimation")
}

/// What a corporate action's subject announces.
pub fn classify_action(subject: &str) -> EventKind {
    let s = subject.to_lowercase();
    if s.contains("split") || s.contains("sub-division") || s.contains("subdivision") {
        EventKind::Split
    } else if s.contains("bonus") {
        EventKind::Bonus
    } else if s.contains("dividend") || s.contains("distribution") {
        EventKind::ExDividend
    } else if s.contains("rights") {
        EventKind::Rights
    } else {
        EventKind::Other
    }
}

/// Corporate actions from `/api/corporates-corporateActions`, dated by
/// their ex-date (the record date when there is none).
pub fn parse_nse_actions(symbol: &str, v: &Value) -> Vec<Event> {
    let nse = nse_symbol(symbol).unwrap_or_default();
    let rows = v
        .as_array()
        .or_else(|| v.get("data").and_then(Value::as_array));
    let mut out: Vec<Event> = Vec::new();
    for row in rows.into_iter().flatten() {
        if !text(row, "symbol").eq_ignore_ascii_case(&nse) {
            continue;
        }
        let Some(date) = nse_date(row.get("exDate")).or_else(|| nse_date(row.get("recDate")))
        else {
            continue;
        };
        let subject = text(row, "subject");
        let event = Event {
            symbol: symbol.to_string(),
            date,
            kind: classify_action(&subject),
            detail: subject,
            source: Source::Nse,
            estimate: false,
        };
        if !out.contains(&event) {
            out.push(event);
        }
    }
    out
}

/// What NSE's filings said about the splits Yahoo reported for a symbol.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BonusCheck {
    /// Not an Indian stock: every split stays a split.
    NotIndian,
    /// NSE answered; this many splits were bonus issues.
    Checked(usize),
    /// NSE could not be asked: every split stays a split.
    Unavailable,
}

/// Mark the bonus issues among `splits` for `symbol`. Yahoo reports an
/// Indian bonus as a split (a 1:1 bonus is a 2:1 split). A split whose date
/// is within three days of an NSE "Bonus" filing gets [`Split::bonus`],
/// dated on that filing's ex-date, since NSE's feed has no allotment date.
/// A split NSE files as a split, or does not list, stays a split, and so
/// does every split of a non-Indian stock. `actions` is NSE's answer
/// ([`Calendar::nse_actions`]), `None` when NSE could not be asked.
pub fn check_bonuses(
    symbol: &str,
    splits: &mut [crate::portfolio::Split],
    actions: Option<&[Event]>,
) -> BonusCheck {
    if nse_symbol(symbol).is_none() {
        return BonusCheck::NotIndian;
    }
    let Some(actions) = actions else {
        return BonusCheck::Unavailable;
    };
    let mut marked = 0;
    for split in splits.iter_mut() {
        let Ok(date) = NaiveDate::parse_from_str(&split.date, "%Y-%m-%d") else {
            continue;
        };
        // The closest bonus or split filing: a stock rarely has both
        // within days of each other, but the nearer one is the event.
        let filing = actions
            .iter()
            .filter(|e| matches!(e.kind, EventKind::Bonus | EventKind::Split))
            .map(|e| ((e.date - date).num_days().abs(), e))
            .filter(|(gap, _)| *gap <= 3)
            .min_by_key(|(gap, _)| *gap)
            .map(|(_, e)| e);
        if let Some(e) = filing.filter(|e| e.kind == EventKind::Bonus) {
            split.bonus = Some(e.date.to_string());
            marked += 1;
        }
    }
    BonusCheck::Checked(marked)
}

// ── Merging and rows ──

/// Events from `today` to `days` days later (each symbol's own exchange
/// date from `today_for`), with official NSE dates replacing Yahoo's for
/// the same stock, sorted by date.
pub fn upcoming(
    events: Vec<Event>,
    days: u32,
    today_for: impl Fn(&str) -> NaiveDate,
) -> Vec<Event> {
    let mut window: Vec<Event> = events
        .into_iter()
        .filter(|e| {
            let days_until = (e.date - today_for(&e.symbol)).num_days();
            (0..=days as i64).contains(&days_until)
        })
        .collect();
    // NSE's board meeting is the results date the company filed; Yahoo's is
    // an estimate. The same goes for NSE's ex-dates.
    let official: Vec<(String, EventKind)> = window
        .iter()
        .filter(|e| e.source == Source::Nse)
        .map(|e| (e.symbol.clone(), e.kind))
        .collect();
    window.retain(|e| {
        e.source == Source::Nse
            || !matches!(e.kind, EventKind::Results | EventKind::ExDividend)
            || !official.contains(&(e.symbol.clone(), e.kind))
    });
    // NSE first among duplicates, which `dedup_by` then drops.
    let key = |e: &Event| {
        (
            e.date,
            e.symbol.clone(),
            e.kind,
            e.source != Source::Nse,
            e.detail.clone(),
        )
    };
    window.sort_by_key(key);
    // The same event from both sources goes; two filings of one kind on one
    // day (an interim and a special dividend) stay.
    window.dedup_by(|a, b| {
        a.date == b.date
            && a.symbol == b.symbol
            && a.kind == b.kind
            && (a.source != b.source || a.detail == b.detail)
    });
    window
}

/// The next results date on or after `today`: NSE's filed date when there
/// is one, else Yahoo's.
pub fn next_results_in(events: &[Event], today: NaiveDate) -> Option<NaiveDate> {
    let next = |source| {
        events
            .iter()
            .filter(|e| e.kind == EventKind::Results && e.source == source && e.date >= today)
            .map(|e| e.date)
            .min()
    };
    next(Source::Nse).or_else(|| next(Source::Yahoo))
}

/// One calendar entry as `calendar --json/--csv` prints it. `when` is
/// `today`, `tomorrow`, `this_week` (within 7 days) or `later`.
#[derive(Debug, Serialize)]
pub struct CalendarRow {
    pub date: String,
    pub days_until: i64,
    pub when: &'static str,
    pub symbol: String,
    pub event: EventKind,
    pub detail: String,
    pub source: Source,
    pub estimate: bool,
}

impl CsvRow for CalendarRow {
    const COLUMNS: &'static [&'static str] = &[
        "date",
        "days_until",
        "when",
        "symbol",
        "event",
        "detail",
        "source",
        "estimate",
    ];
}

pub fn when(days_until: i64) -> &'static str {
    match days_until {
        0 => "today",
        1 => "tomorrow",
        2..=6 => "this_week",
        _ => "later",
    }
}

pub fn rows(events: &[Event], today_for: impl Fn(&str) -> NaiveDate) -> Vec<CalendarRow> {
    events
        .iter()
        .map(|e| {
            let days_until = (e.date - today_for(&e.symbol)).num_days();
            CalendarRow {
                date: e.date.format("%Y-%m-%d").to_string(),
                days_until,
                when: when(days_until),
                symbol: e.symbol.clone(),
                event: e.kind,
                detail: e.detail.clone(),
                source: e.source,
                estimate: e.estimate,
            }
        })
        .collect()
}

/// The date at the symbol's exchange: IST for Indian listings, New York for
/// US ones, UTC otherwise.
pub fn exchange_today(symbol: &str, now: DateTime<Utc>) -> NaiveDate {
    crate::alerts::alert_market(symbol)
        .map_or(now.date_naive(), |m| crate::clock::session_date(m, now))
}

// ── Fetching and the day cache ──

const CACHE_FILE: &str = "calendar_cache.json";
/// Cache key of NSE's last refusal.
const NSE_BLOCKED: &str = "nse:blocked";
/// How long an answer is reused.
const FRESH_SECS: i64 = 24 * 3600;
/// How long a failure is remembered before asking again.
const RETRY_SECS: i64 = 3600;
/// Entries older than this are dropped when the cache is saved.
const KEEP_SECS: i64 = 7 * 24 * 3600;

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    #[serde(default)]
    entries: HashMap<String, Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    /// Unix seconds.
    fetched_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(default)]
    events: Vec<Event>,
}

impl Entry {
    fn fresh(&self, now: i64) -> bool {
        let ttl = if self.error.is_some() {
            RETRY_SECS
        } else {
            FRESH_SECS
        };
        (0..ttl).contains(&(now - self.fetched_at))
    }
}

/// Events for symbols, from the day cache or the network. Create one per
/// run, ask for every symbol, then [`Calendar::save`].
pub struct Calendar<'a> {
    yahoo: &'a YahooClient,
    nse: Option<NseSession>,
    /// Why NSE is not asked again this run.
    nse_blocked: Option<String>,
    cache: Cache,
    dirty: bool,
    /// Symbols Yahoo had no calendar for.
    pub yahoo_missing: Vec<String>,
}

impl<'a> Calendar<'a> {
    pub fn new(yahoo: &'a YahooClient) -> Self {
        Self {
            yahoo,
            nse: None,
            nse_blocked: None,
            // A damaged cache is just refetched.
            cache: crate::store::load(CACHE_FILE).unwrap_or_default(),
            dirty: false,
            yahoo_missing: Vec::new(),
        }
    }

    /// Why NSE dates are missing, if NSE refused this run (or within the
    /// last hour).
    pub fn nse_note(&self) -> Option<&str> {
        self.nse_blocked.as_deref()
    }

    /// Every event the sources list for `symbol`, past and future. Only
    /// stocks have a calendar.
    pub async fn events_for(&mut self, symbol: &str) -> Vec<Event> {
        if AssetClass::of(symbol) != AssetClass::Stock {
            return Vec::new();
        }
        let now = Utc::now().timestamp();
        let mut events = Vec::new();
        // A refusal recorded by any run in the last hour (see `store`).
        if self.nse_blocked.is_none()
            && let Some(e) = self.cached(NSE_BLOCKED, now)
        {
            self.nse_blocked = e.error;
        }

        let key = format!("yahoo:{symbol}");
        let entry = match self.cached(&key, now) {
            Some(e) => e,
            None => {
                let fetched = self.fetch_yahoo(symbol).await;
                self.store(key, now, fetched)
            }
        };
        if entry.error.is_some() {
            self.yahoo_missing.push(symbol.to_string());
        }
        events.extend(entry.events);

        if let Some(nse) = nse_symbol(symbol) {
            for (path, referer) in NSE_ENDPOINTS {
                let key = format!("nse:{path}:{nse}");
                let entry = match self.cached(&key, now) {
                    Some(e) => e,
                    None => {
                        let fetched = self.fetch_nse(path, referer, symbol, &nse).await;
                        // Not asked (blocked earlier this run): nothing to cache.
                        let Some(fetched) = fetched else { continue };
                        self.store(key, now, fetched)
                    }
                };
                if let Some(err) = &entry.error
                    && self.nse_blocked.is_none()
                {
                    self.nse_blocked = Some(err.clone());
                }
                // `TCS.NS` and `TCS.BO` share NSE's answer; label it with the symbol asked for.
                events.extend(entry.events.into_iter().map(|mut e| {
                    e.symbol = symbol.to_string();
                    e
                }));
            }
        }
        events
    }

    /// NSE's corporate-action filings (splits, bonus issues, dividends, ...)
    /// for an Indian stock, from the day cache or the network. `None` for
    /// other stocks, and when NSE refused or could not be reached (see
    /// [`Calendar::nse_note`]).
    pub async fn nse_actions(&mut self, symbol: &str) -> Option<Vec<Event>> {
        let nse = nse_symbol(symbol)?;
        let now = Utc::now().timestamp();
        if self.nse_blocked.is_none()
            && let Some(e) = self.cached(NSE_BLOCKED, now)
        {
            self.nse_blocked = e.error;
        }
        let (path, referer) = NSE_ENDPOINTS[1];
        let key = format!("nse:{path}:{nse}");
        let entry = match self.cached(&key, now) {
            Some(e) => e,
            None => {
                let fetched = self.fetch_nse(path, referer, symbol, &nse).await?;
                self.store(key, now, fetched)
            }
        };
        if let Some(err) = entry.error {
            if self.nse_blocked.is_none() {
                self.nse_blocked = Some(err);
            }
            return None;
        }
        Some(entry.events)
    }

    /// Indian stocks among `symbols` whose NSE filings aren't cached yet;
    /// each costs two requests a second apart.
    pub fn nse_to_fetch(&self, symbols: &[String]) -> usize {
        if self.nse_blocked.is_some() {
            return 0;
        }
        let now = Utc::now().timestamp();
        let mut names: Vec<String> = symbols.iter().filter_map(|s| nse_symbol(s)).collect();
        names.dedup();
        names
            .iter()
            .filter(|n| {
                NSE_ENDPOINTS
                    .iter()
                    .any(|(path, _)| self.cached(&format!("nse:{path}:{n}"), now).is_none())
            })
            .count()
    }

    fn cached(&self, key: &str, now: i64) -> Option<Entry> {
        self.cache
            .entries
            .get(key)
            .filter(|e| e.fresh(now))
            .cloned()
    }

    fn store(&mut self, key: String, now: i64, fetched: Result<Vec<Event>>) -> Entry {
        // NSE refusing or unreachable holds for every symbol: remember it once
        // for an hour, so other runs and `results-in` lookups don't each try.
        if key.starts_with("nse:")
            && let Err(e) = &fetched
        {
            self.cache.entries.insert(
                NSE_BLOCKED.to_string(),
                Entry {
                    fetched_at: now,
                    error: Some(format!("{e:#}")),
                    events: Vec::new(),
                },
            );
        }
        let entry = match fetched {
            Ok(events) => Entry {
                fetched_at: now,
                error: None,
                events,
            },
            Err(e) => Entry {
                fetched_at: now,
                error: Some(format!("{e:#}")),
                events: Vec::new(),
            },
        };
        self.cache.entries.insert(key, entry.clone());
        self.dirty = true;
        entry
    }

    async fn fetch_yahoo(&self, symbol: &str) -> Result<Vec<Event>> {
        let url = build_url(
            "https://query2.finance.yahoo.com/v10/finance/quoteSummary",
            Some(symbol),
            &[("modules", "calendarEvents"), ("crumb", self.yahoo.crumb())],
        );
        let v = self.yahoo.raw_get(&url).await?;
        if v.pointer("/quoteSummary/result/0").is_none() {
            bail!("Yahoo Finance has no calendar for {symbol}");
        }
        Ok(parse_yahoo(symbol, &v))
    }

    /// `None` when NSE already refused this run.
    async fn fetch_nse(
        &mut self,
        path: &str,
        referer: &str,
        symbol: &str,
        nse: &str,
    ) -> Option<Result<Vec<Event>>> {
        if self.nse_blocked.is_some() {
            return None;
        }
        if self.nse.is_none() {
            match NseSession::open().await {
                Ok(s) => self.nse = Some(s),
                Err(e) => return Some(Err(e)),
            }
        }
        let session = self.nse.as_mut()?;
        let fetched = session.get_json(path, referer, nse).await.map(|v| {
            if path == MEETINGS {
                parse_nse_meetings(symbol, &v)
            } else {
                parse_nse_actions(symbol, &v)
            }
        });
        Some(fetched)
    }

    /// Write the cache back, dropping week-old entries. Failing to save only
    /// costs a refetch next time.
    pub fn save(&mut self) {
        if !self.dirty {
            return;
        }
        let now = Utc::now().timestamp();
        self.cache
            .entries
            .retain(|_, e| now - e.fetched_at < KEEP_SECS);
        let _ = crate::store::save(CACHE_FILE, &self.cache, false);
        self.dirty = false;
    }
}

/// The next results date for `symbol` on or after `today`, from the cache
/// or the network (for `results-in` alerts).
pub async fn next_results(
    yahoo: &YahooClient,
    symbol: &str,
    today: NaiveDate,
) -> Option<NaiveDate> {
    let mut cal = Calendar::new(yahoo);
    let events = cal.events_for(symbol).await;
    cal.save();
    next_results_in(&events, today)
}

// ── NSE ──

const NSE_BASE: &str = "https://www.nseindia.com";
const NSE_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
                      (KHTML, like Gecko) Chrome/129.0 Safari/537.36";
const MEETINGS: &str = "/api/corporate-board-meetings";
const ACTIONS: &str = "/api/corporates-corporateActions";
/// The API paths and the page that uses each.
const NSE_ENDPOINTS: [(&str, &str); 2] = [
    (
        MEETINGS,
        "/companies-listing/corporate-filings-board-meetings",
    ),
    (ACTIONS, "/companies-listing/corporate-filings-actions"),
];
/// Minimum gap between two NSE requests.
const NSE_GAP: Duration = Duration::from_secs(1);

fn nse_refused(status: reqwest::StatusCode) -> anyhow::Error {
    anyhow!(
        "NSE refused the request (HTTP {status}); its API serves browser sessions and \
         sometimes blocks scripts. Showing Yahoo Finance dates only — NSE is asked again \
         in an hour"
    )
}

/// NSE's corporate-calendar API through the process-wide NSE session (one
/// cookie handshake, one rate limit and refusal state for every NSE request).
struct NseSession {
    session: std::sync::Arc<crate::providers::session::Session>,
}

impl NseSession {
    async fn open() -> Result<Self> {
        let session = crate::providers::nse::shared_session();
        if session.is_blocked() {
            return Err(nse_refused(reqwest::StatusCode::FORBIDDEN));
        }
        Ok(Self { session })
    }

    async fn get_json(&mut self, path: &str, _referer: &str, symbol: &str) -> Result<Value> {
        use crate::providers::session::FetchError;
        self.session
            .get_json(
                path,
                &[("index", "equities"), ("symbol", symbol)],
                Duration::from_secs(3600),
            )
            .await
            .map_err(|e| match e {
                FetchError::Blocked(_) => nse_refused(reqwest::StatusCode::FORBIDDEN),
                FetchError::Format(_) => nse_refused(reqwest::StatusCode::OK),
                other => anyhow!("Request to nseindia.com failed ({other})"),
            })
    }
}

/// Markets a calendar symbol may belong to, for the footnote.
pub fn has_indian(symbols: &[String]) -> bool {
    symbols
        .iter()
        .any(|s| crate::alerts::alert_market(s) == Some(Market::In) && nse_symbol(s).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    /// A trimmed `quoteSummary?modules=calendarEvents` answer for AAPL.
    fn yahoo_fixture() -> Value {
        json!({"quoteSummary":{"result":[{"calendarEvents":{
            "maxAge":1,
            "earnings":{
                "earningsDate":[{"raw":1793995200,"fmt":"2026-11-06"},{"raw":1794340800,"fmt":"2026-11-10"}],
                "earningsCallDate":[],
                "isEarningsDateEstimate":true,
                "earningsAverage":{"raw":1.77,"fmt":"1.77"},
                "earningsLow":{"raw":1.7,"fmt":"1.70"},
                "revenueAverage":{"raw":102000000000_i64,"fmt":"102B"}
            },
            "exDividendDate":{"raw":1793664000,"fmt":"2026-11-03"},
            "dividendDate":{"raw":1794009600,"fmt":"2026-11-06"}
        }}],"error":null}})
    }

    /// `/api/corporate-board-meetings?index=equities&symbol=TCS`, as NSE
    /// answers it (fields trimmed): the same meeting filed twice.
    fn meetings_fixture() -> Value {
        json!([
            {"bm_symbol":"TCS","bm_date":"08-Oct-2026","bm_purpose":"Board Meeting Intimation",
             "bm_desc":"TATA CONSULTANCY SERVICES LIMITED has informed the Exchange about Board Meeting to be held on 08-Oct-2026 to consider and approve the Quarterly Audited Financial results of the Company for the period ended September 2026 and Dividend.",
             "bm_timestamp":"22-Sep-2026 18:40:42","sm_name":"Tata Consultancy Services Limited"},
            {"bm_symbol":"TCS","bm_date":"08-Oct-2026","bm_purpose":"Financial Results/Dividend",
             "bm_desc":"Approval of the Financial Results and consideration of declaration of second interim dividend",
             "bm_timestamp":"22-Sep-2026 18:24:21","sm_name":"Tata Consultancy Services Limited"},
            {"bm_symbol":"TCS","bm_date":"09-Jan-2027","bm_purpose":"Financial Results",
             "bm_desc":"To consider and approve the financial results for the quarter ending December 2026",
             "bm_timestamp":"10-Dec-2026 18:00:00"},
            {"bm_symbol":"TCS","bm_date":"20-Nov-2026","bm_purpose":"Fund Raising",
             "bm_desc":"To consider raising of funds","bm_timestamp":"01-Nov-2026 10:00:00"},
            {"bm_symbol":"TCSX","bm_date":"20-Nov-2026","bm_purpose":"Financial Results","bm_desc":""}
        ])
    }

    /// `/api/corporates-corporateActions?index=equities&symbol=TCS`.
    fn actions_fixture() -> Value {
        json!([
            {"symbol":"TCS","series":"EQ","faceVal":"1","subject":"Interim Dividend - Rs 11 Per Share",
             "exDate":"16-Oct-2026","recDate":"16-Oct-2026","comp":"Tata Consultancy Services Limited"},
            {"symbol":"TCS","series":"EQ","faceVal":"1","subject":"Face Value Split (Sub-Division) - From Rs 2/- Per Share To Rs 1/- Per Share",
             "exDate":"-","recDate":"05-Nov-2026"},
            {"symbol":"TCS","series":"EQ","faceVal":"1","subject":"Bonus 1:1","exDate":"12-Nov-2026","recDate":"12-Nov-2026"},
            {"symbol":"TCS","series":"EQ","faceVal":"1","subject":"Annual General Meeting","exDate":"-","recDate":"-"}
        ])
    }

    #[test]
    fn yahoo_calendar_events_parse() {
        let events = parse_yahoo("AAPL", &yahoo_fixture());
        assert_eq!(events.len(), 3);
        let results = &events[0];
        assert_eq!(results.kind, EventKind::Results);
        assert_eq!(results.date, d("2026-11-06"));
        assert!(results.estimate);
        assert_eq!(
            results.detail,
            "expected 2026-11-06 to 2026-11-10; EPS estimate 1.77"
        );
        assert_eq!(events[1].kind, EventKind::ExDividend);
        assert_eq!(events[1].date, d("2026-11-03"));
        assert_eq!(events[2].kind, EventKind::DividendPaid);
        assert!(events.iter().all(|e| e.source == Source::Yahoo));
        // Raw timestamps alone are enough; no calendar is no events.
        let raw = json!({"quoteSummary":{"result":[{"calendarEvents":{
            "earnings":{"earningsDate":[{"raw":1793995200}]}}}]}});
        let events = parse_yahoo("AAPL", &raw);
        assert_eq!(events[0].date, d("2026-11-06"));
        assert!(!events[0].estimate);
        assert!(parse_yahoo("AAPL", &json!({"quoteSummary":{"result":[]}})).is_empty());
    }

    #[test]
    fn nse_board_meetings_parse_and_dedupe() {
        let events = parse_nse_meetings("TCS.NS", &meetings_fixture());
        let summary: Vec<(NaiveDate, EventKind, &str)> = events
            .iter()
            .map(|e| (e.date, e.kind, e.detail.as_str()))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    d("2026-10-08"),
                    EventKind::Results,
                    "Financial Results/Dividend"
                ),
                (d("2027-01-09"), EventKind::Results, "Financial Results"),
                (d("2026-11-20"), EventKind::BoardMeeting, "Fund Raising"),
            ]
        );
        assert!(events.iter().all(|e| e.symbol == "TCS.NS"));
        // A `.BO` ticker asks NSE for the same company.
        assert_eq!(parse_nse_meetings("TCS.BO", &meetings_fixture()).len(), 3);
        // Wrapped in {"data": [...]} works too; garbage is empty.
        let wrapped = json!({"data": meetings_fixture()});
        assert_eq!(parse_nse_meetings("TCS.NS", &wrapped).len(), 3);
        assert!(parse_nse_meetings("TCS.NS", &json!("<html>")).is_empty());
    }

    #[test]
    fn nse_corporate_actions_parse() {
        let events = parse_nse_actions("TCS.NS", &actions_fixture());
        let summary: Vec<(NaiveDate, EventKind)> =
            events.iter().map(|e| (e.date, e.kind)).collect();
        assert_eq!(
            summary,
            [
                (d("2026-10-16"), EventKind::ExDividend),
                // No ex-date: the record date.
                (d("2026-11-05"), EventKind::Split),
                (d("2026-11-12"), EventKind::Bonus),
            ]
        );
        assert_eq!(events[0].detail, "Interim Dividend - Rs 11 Per Share");
    }

    #[test]
    fn actions_are_classified_by_subject() {
        assert_eq!(
            classify_action("Dividend - Rs 5.50 Per Share"),
            EventKind::ExDividend
        );
        assert_eq!(
            classify_action("Annual General Meeting/Dividend - Rs 2 Per Share"),
            EventKind::ExDividend
        );
        assert_eq!(classify_action("Bonus 1:2"), EventKind::Bonus);
        assert_eq!(
            classify_action("Rights 1:5 @ Premium Rs 100/-"),
            EventKind::Rights
        );
        assert_eq!(classify_action("Buyback"), EventKind::Other);
        assert_eq!(
            classify_action("Face Value Split (Sub-Division) - From Rs 10/- To Rs 2/-"),
            EventKind::Split
        );
    }

    #[test]
    fn bonus_issues_are_told_from_splits_by_nse_filings() {
        use crate::portfolio::Split;
        // TCS fixture: a split with ex-date 2026-11-05, a 1:1 bonus on 2026-11-12.
        let actions = parse_nse_actions("TCS.NS", &actions_fixture());
        let yahoo = || {
            vec![
                Split::new("2026-11-05", 2.0, 1.0),
                // Yahoo's date a day off NSE's ex-date still matches.
                Split::new("2026-11-13", 2.0, 1.0),
                // Nothing filed near it: stays a split.
                Split::new("2025-01-01", 2.0, 1.0),
            ]
        };
        let mut splits = yahoo();
        assert_eq!(
            check_bonuses("TCS.NS", &mut splits, Some(&actions)),
            BonusCheck::Checked(1)
        );
        let bonus: Vec<Option<&str>> = splits.iter().map(|s| s.bonus.as_deref()).collect();
        assert_eq!(bonus, vec![None, Some("2026-11-12"), None]);

        // Not an Indian stock: never a bonus lot.
        let mut splits = yahoo();
        assert_eq!(
            check_bonuses("AAPL", &mut splits, Some(&actions)),
            BonusCheck::NotIndian
        );
        assert!(splits.iter().all(|s| s.bonus.is_none()));

        // NSE unreachable: every split stays a split.
        let mut splits = yahoo();
        assert_eq!(
            check_bonuses("TCS.NS", &mut splits, None),
            BonusCheck::Unavailable
        );
        assert_eq!(splits, yahoo());
    }

    #[test]
    fn nse_symbols_only_for_indian_stocks() {
        assert_eq!(nse_symbol("tcs.ns").as_deref(), Some("TCS"));
        assert_eq!(nse_symbol("RELIANCE.BO").as_deref(), Some("RELIANCE"));
        assert_eq!(nse_symbol("AAPL"), None);
        assert_eq!(nse_symbol("^NSEI"), None);
    }

    #[test]
    fn upcoming_keeps_the_window_and_prefers_nse() {
        let today = d("2026-10-25");
        let mut events = parse_nse_meetings("TCS.NS", &meetings_fixture());
        events.extend(parse_nse_actions("TCS.NS", &actions_fixture()));
        // NSE has no TCS results date in the window, so Yahoo's estimate
        // stays; AAPL is Yahoo-only.
        events.push(Event {
            symbol: "TCS.NS".into(),
            date: d("2026-11-21"),
            kind: EventKind::Results,
            detail: String::new(),
            source: Source::Yahoo,
            estimate: true,
        });
        events.extend(parse_yahoo("AAPL", &yahoo_fixture()));
        let list = upcoming(events, 30, |_| today);
        let summary: Vec<(String, &str, EventKind, Source)> = list
            .iter()
            .map(|e| (e.date.to_string(), e.symbol.as_str(), e.kind, e.source))
            .collect();
        assert_eq!(
            summary,
            [
                (
                    "2026-11-03".into(),
                    "AAPL",
                    EventKind::ExDividend,
                    Source::Yahoo
                ),
                ("2026-11-05".into(), "TCS.NS", EventKind::Split, Source::Nse),
                (
                    "2026-11-06".into(),
                    "AAPL",
                    EventKind::Results,
                    Source::Yahoo
                ),
                (
                    "2026-11-06".into(),
                    "AAPL",
                    EventKind::DividendPaid,
                    Source::Yahoo
                ),
                ("2026-11-12".into(), "TCS.NS", EventKind::Bonus, Source::Nse),
                (
                    "2026-11-20".into(),
                    "TCS.NS",
                    EventKind::BoardMeeting,
                    Source::Nse
                ),
                (
                    "2026-11-21".into(),
                    "TCS.NS",
                    EventKind::Results,
                    Source::Yahoo
                ),
            ]
        );
        // The Yahoo estimate stays when NSE has no results date in the window.
        let rows = rows(&list, |_| today);
        assert_eq!(rows[0].days_until, 9);
        assert_eq!(rows[0].when, "later");
        let (missing, extra) = crate::output::missing_columns(&rows[0]);
        assert!(
            missing.is_empty() && extra.is_empty(),
            "{missing:?} {extra:?}"
        );
    }

    #[test]
    fn nse_results_replace_yahoo_estimates() {
        let today = d("2026-10-01");
        let mut events = parse_nse_meetings("TCS.NS", &meetings_fixture());
        events.push(Event {
            symbol: "TCS.NS".into(),
            date: d("2026-10-10"),
            kind: EventKind::Results,
            detail: String::new(),
            source: Source::Yahoo,
            estimate: true,
        });
        let list = upcoming(events.clone(), 30, |_| today);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].source, Source::Nse);
        assert_eq!(list[0].date, d("2026-10-08"));
        assert_eq!(next_results_in(&events, today), Some(d("2026-10-08")));
        // After the NSE meeting, the next filed one wins over Yahoo's.
        assert_eq!(
            next_results_in(&events, d("2026-10-09")),
            Some(d("2027-01-09"))
        );
        assert_eq!(next_results_in(&events, d("2027-02-01")), None);
    }

    #[test]
    fn distinct_filings_on_one_day_both_stay() {
        let today = d("2026-10-01");
        let action = |subject: &str| Event {
            symbol: "ITC.NS".into(),
            date: d("2026-10-10"),
            kind: EventKind::ExDividend,
            detail: subject.into(),
            source: Source::Nse,
            estimate: false,
        };
        let events = vec![
            action("Interim Dividend - Rs 6 Per Share"),
            action("Special Dividend - Rs 2 Per Share"),
            action("Interim Dividend - Rs 6 Per Share"),
        ];
        assert_eq!(upcoming(events, 30, |_| today).len(), 2);
    }

    #[test]
    fn when_marks_today_and_this_week() {
        assert_eq!(when(0), "today");
        assert_eq!(when(1), "tomorrow");
        assert_eq!(when(6), "this_week");
        assert_eq!(when(7), "later");
    }

    #[test]
    fn cache_entries_expire() {
        let ok = Entry {
            fetched_at: 1_000_000,
            error: None,
            events: Vec::new(),
        };
        assert!(ok.fresh(1_000_000 + 3600));
        assert!(!ok.fresh(1_000_000 + FRESH_SECS));
        let failed = Entry {
            error: Some("refused".into()),
            ..ok.clone()
        };
        assert!(failed.fresh(1_000_000 + 60));
        assert!(!failed.fresh(1_000_000 + RETRY_SECS));
        // A clock that went backwards refetches.
        assert!(!ok.fresh(999_000));
    }

    #[test]
    fn exchange_dates_follow_the_listing() {
        // 20:00 UTC on 24 Oct is 01:30 IST on the 25th, 16:00 in New York on the 24th.
        let now = DateTime::parse_from_rfc3339("2026-10-24T20:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        assert_eq!(exchange_today("TCS.NS", now), d("2026-10-25"));
        assert_eq!(exchange_today("AAPL", now), d("2026-10-24"));
    }
}
