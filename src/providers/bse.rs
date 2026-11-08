//! BSE (<https://www.bseindia.com>), from the JSON endpoints behind its
//! public website at `api.bseindia.com` — not a documented or licensed API.
//! BSE's terms restrict automated access and redistribution; StockWise reads
//! what one person would, slowly (see `session.rs`), for personal use, and
//! Yahoo answers whenever BSE refuses or changes its answers.
//!
//! BSE identifies equities by a numeric scrip code (`500325`); Yahoo accepts
//! both `500325.BO` and `RELIANCE.BO`. The scrip list (`ListofScripData`,
//! kept on disk for a day) maps one to the other and serves search.
//!
//! | Data | Endpoint |
//! |------|----------|
//! | Scrip list (search, `RELIANCE.BO` → `500325`) | `ListofScripData/w?segment=Equity&status=Active` |
//! | Quote | `getScripHeaderData/w?scripcode=500325` |
//! | Intraday prices (today) | `StockReachGraph/w?scripcode=500325&flag=0` |
//! | Daily history | `StockPriceCSVDownload/w?rbType=D&Scode=500325&FDates=&TDates=` (CSV) |
//!
//! docs/exchange-responses.md records the fields StockWise reads from each.

use super::nse::{bars_from_points, interval_seconds, search_list};
use super::session::{FetchError, Limiter, Session, Site};
use super::{Bar, Series, csv_fields};
use crate::api::{ChartData, Quote, SearchResult};
use anyhow::Result;
use chrono::{NaiveDate, NaiveDateTime};
use serde_json::Value;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub const BASE_URL: &str = "https://api.bseindia.com/BseIndiaAPI/api";
const WEBSITE: &str = "https://www.bseindia.com";
const FALLBACK: &str = "using Yahoo Finance instead";
const IST_OFFSET: i64 = 19_800;
/// Daily history BSE is asked for (one request); longer ranges go to Yahoo.
pub const HISTORY_DAYS: i64 = 5 * 366;
/// Most symbols one quote request may ask BSE for (one request each).
pub const QUOTE_BATCH: u32 = 8;

pub fn site(base: &str) -> Site {
    Site {
        name: "BSE",
        base: base.trim_end_matches('/').to_string(),
        // The API answers without cookies but checks where calls come from.
        home: None,
        referer: format!("{}/", WEBSITE),
        origin: Some(WEBSITE.to_string()),
    }
}

/// The process-wide BSE session (one rate limit for every caller).
pub fn shared_session() -> Arc<Session> {
    static SHARED: OnceLock<Arc<Session>> = OnceLock::new();
    SHARED
        .get_or_init(|| Arc::new(Session::new(site(BASE_URL), Limiter::polite())))
        .clone()
}

/// How a Yahoo `.BO` symbol names a BSE equity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A scrip code (`500325.BO`).
    Code(String),
    /// A scrip id (`RELIANCE.BO`), looked up in the scrip list.
    Id(String),
}

pub fn target(symbol: &str) -> Option<Target> {
    let upper = symbol.trim().to_ascii_uppercase();
    let base = upper.strip_suffix(".BO")?;
    if base.is_empty() || base.contains('.') || base.starts_with('^') {
        return None;
    }
    Some(if base.chars().all(|c| c.is_ascii_digit()) {
        Target::Code(base.to_string())
    } else {
        Target::Id(base.to_string())
    })
}

// ── Parsers (pure; fixture-tested) ──

/// A number BSE may send as a string with thousands separators
/// (`"1,451.65"`), or 0 / `"-"` for "none".
fn price(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| {
            v.as_str()
                .and_then(|s| s.trim().replace(',', "").parse().ok())
        })
        .filter(|x: &f64| x.is_finite() && *x > 0.0)
}

fn signed(v: &Value) -> Option<f64> {
    v.as_f64()
        .or_else(|| {
            v.as_str()
                .and_then(|s| s.trim().replace(',', "").parse().ok())
        })
        .filter(|x: &f64| x.is_finite())
}

/// One listed equity.
#[derive(Clone, Debug, PartialEq)]
pub struct Scrip {
    pub code: String,
    /// The ticker-like id Yahoo uses (`RELIANCE`).
    pub id: String,
    pub name: String,
}

/// `ListofScripData` → scrips. BSE sends a bare array; older answers wrap it
/// in `{"Table": [...]}`.
pub fn parse_scrip_list(json: &Value) -> Result<Vec<Scrip>, &'static str> {
    let rows = json
        .as_array()
        .or_else(|| json["Table"].as_array())
        .ok_or("scrip list: not a list")?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let code = match &r["SCRIP_CD"] {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            if code.is_empty() {
                return None;
            }
            let name = r["Scrip_Name"]
                .as_str()
                .or(r["Issuer_Name"].as_str())
                .unwrap_or_default()
                .trim()
                .to_string();
            Some(Scrip {
                code,
                id: r["scrip_id"]
                    .as_str()
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_uppercase(),
                name,
            })
        })
        .collect())
}

/// `getScripHeaderData` → a quote. `Ok(None)` for a code BSE doesn't price;
/// `Err` when the answer has neither `Header` nor `CurrRate`.
pub fn parse_header(json: &Value) -> Result<Option<Quote>, &'static str> {
    if json.get("Header").is_none() && json.get("CurrRate").is_none() {
        return Err("quote: no Header or CurrRate");
    }
    let (header, rate) = (&json["Header"], &json["CurrRate"]);
    let Some(ltp) = price(&rate["LTP"]).or_else(|| price(&header["LTP"])) else {
        return Ok(None);
    };
    let prev = price(&header["PrevClose"]);
    let mut q = Quote {
        short_name: json["Cmpname"]["FullN"]
            .as_str()
            .or(json["Cmpname"]["ShortN"].as_str())
            .map(|s| s.trim().to_string()),
        regular_market_price: Some(ltp),
        regular_market_change: signed(&rate["Chg"]),
        regular_market_change_percent: signed(&rate["PcChg"]),
        regular_market_previous_close: prev,
        regular_market_open: price(&header["Open"]),
        regular_market_day_high: price(&header["High"]),
        regular_market_day_low: price(&header["Low"]),
        currency: Some("INR".into()),
        exchange: Some("BSE".into()),
        data_source: Some("BSE".into()),
        ..Default::default()
    };
    if q.regular_market_change.is_none()
        && let Some(prev) = prev
    {
        q.regular_market_change = Some(ltp - prev);
        q.regular_market_change_percent = Some((ltp - prev) / prev * 100.0);
    }
    Ok(Some(q))
}

/// `StockReachGraph` (flag 0) → `(unix seconds, price)` points of today's
/// session. `Data` is a JSON list sent as a string (sometimes as a list);
/// times (`Fri Oct 24 2025 09:15:00`) are IST.
pub fn parse_graph(json: &Value) -> Result<Vec<(i64, f64)>, &'static str> {
    let data = json.get("Data").ok_or("intraday: no Data")?;
    let parsed: Value;
    let rows = match data {
        Value::String(s) if s.trim().is_empty() => return Ok(Vec::new()),
        Value::String(s) => {
            parsed = serde_json::from_str(s).map_err(|_| "intraday: Data is not JSON")?;
            parsed.as_array().ok_or("intraday: Data is not a list")?
        }
        Value::Array(a) => a,
        Value::Null => return Ok(Vec::new()),
        _ => return Err("intraday: Data is not a list"),
    };
    let mut points: Vec<(i64, f64)> = rows
        .iter()
        .filter_map(|r| {
            let at =
                NaiveDateTime::parse_from_str(r["dttm"].as_str()?.trim(), "%a %b %d %Y %H:%M:%S")
                    .ok()?;
            let p = price(&r["vale1"])?;
            Some((at.and_utc().timestamp() - IST_OFFSET, p))
        })
        .collect();
    points.sort_by_key(|p| p.0);
    Ok(points)
}

/// `24-October-2025`, `24-Oct-2025`, `24-Oct-25` or `24/10/2025`.
fn parse_csv_date(s: &str) -> Option<NaiveDate> {
    let s = s.trim();
    ["%d-%B-%Y", "%d-%b-%Y", "%d-%b-%y", "%d/%m/%Y", "%Y-%m-%d"]
        .iter()
        .find_map(|f| NaiveDate::parse_from_str(s, f).ok())
}

/// The "Stock Prices" CSV download → daily bars. Columns are found by name
/// (`Date`, `Open Price`, `High Price`, `Low Price`, `Close Price`,
/// `No.of Shares`), so added or reordered columns don't matter.
pub fn parse_price_csv(csv: &str) -> Result<Vec<Bar>, &'static str> {
    let mut lines = csv.lines().filter(|l| !l.trim().is_empty());
    let header: Vec<String> = csv_fields(lines.next().ok_or("history: empty")?)
        .iter()
        .map(|c| c.trim().to_ascii_lowercase())
        .collect();
    let col = |want: &dyn Fn(&str) -> bool| header.iter().position(|c| want(c));
    let (Some(date), Some(close)) = (col(&|c| c == "date"), col(&|c| c.starts_with("close")))
    else {
        return Err("history: no Date / Close Price columns");
    };
    let open = col(&|c| c.starts_with("open"));
    let high = col(&|c| c.starts_with("high"));
    let low = col(&|c| c.starts_with("low"));
    let shares = col(&|c| c.contains("no.of shares") || c.contains("no. of shares"));
    let cell = |f: &[String], i: Option<usize>| {
        i.and_then(|i| f.get(i))
            .and_then(|v| price(&Value::String(v.clone())))
    };
    Ok(lines
        .filter_map(|line| {
            let f = csv_fields(line);
            Some(Bar {
                date: parse_csv_date(f.get(date)?)?,
                open: cell(&f, open),
                high: cell(&f, high),
                low: cell(&f, low),
                close: Some(cell(&f, Some(close))?),
                volume: cell(&f, shares).map(|v| v as u64),
            })
        })
        .collect())
}

/// One corporate announcement filed with BSE.
#[derive(Clone, Debug, PartialEq)]
pub struct Announcement {
    pub code: String,
    pub company: String,
    pub headline: String,
    /// BSE's category (`Result`, `Board Meeting`, ...).
    pub category: String,
    /// Day it was filed.
    pub date: NaiveDate,
    /// Link to the filed PDF, when there is one.
    pub attachment: Option<String>,
}

/// Where BSE keeps announcement PDFs.
const ATTACHMENTS: &str = "https://www.bseindia.com/xml-data/corpfiling/AttachLive";

/// `AnnGetData` → announcements, newest first as BSE sends them. BSE wraps
/// the rows in `{"Table": [...]}`; each has `SCRIP_CD`, `SLONGNAME`,
/// `NEWSSUB` (or `HEADLINE`), `CATEGORYNAME`, `NEWS_DT` (an ISO date-time)
/// and `ATTACHMENTNAME`. Built from the format open-source BSE clients read;
/// not confirmed against the live site. Anything else is a format change.
pub fn parse_announcements(json: &Value) -> Result<Vec<Announcement>, &'static str> {
    let rows = json["Table"]
        .as_array()
        .ok_or("announcements: no Table list")?;
    let parsed: Vec<Announcement> = rows
        .iter()
        .filter_map(|r| {
            let code = match &r["SCRIP_CD"] {
                Value::String(s) => s.trim().to_string(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            let date =
                NaiveDate::parse_from_str(r["NEWS_DT"].as_str()?.get(..10)?, "%Y-%m-%d").ok()?;
            let text = |k: &str| r[k].as_str().map(str::trim).filter(|t| !t.is_empty());
            Some(Announcement {
                code,
                company: text("SLONGNAME").unwrap_or_default().to_string(),
                headline: text("NEWSSUB")
                    .or_else(|| text("HEADLINE"))
                    .unwrap_or_default()
                    .to_string(),
                category: text("CATEGORYNAME").unwrap_or_default().to_string(),
                date,
                attachment: text("ATTACHMENTNAME").map(|a| format!("{ATTACHMENTS}/{a}")),
            })
        })
        .collect();
    // Rows but none readable: the format changed, not "no filings".
    if parsed.is_empty() && !rows.is_empty() {
        return Err("announcements: no row has a readable date and scrip");
    }
    Ok(parsed)
}

// ── Client ──

/// The BSE provider: quotes, intraday and daily charts and search through
/// one polite [`Session`].
pub struct Bse {
    session: Arc<Session>,
    scrips: tokio::sync::Mutex<Option<Arc<Vec<Scrip>>>>,
}

impl Bse {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            scrips: tokio::sync::Mutex::new(None),
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn supports(&self, symbol: &str) -> bool {
        target(symbol).is_some()
    }

    pub fn quote_cost(&self, symbols: &[&str]) -> u32 {
        symbols.iter().filter(|s| target(s).is_some()).count() as u32
    }

    pub fn remaining(&self) -> u32 {
        if self.session.is_blocked() {
            0
        } else {
            QUOTE_BATCH
        }
    }

    fn fail(&self, e: FetchError) -> anyhow::Error {
        self.session.notice(&e, FALLBACK);
        anyhow::Error::new(e)
    }

    fn format_changed(&self, what: &str) -> anyhow::Error {
        self.fail(self.session.format_error(what))
    }

    /// Every active equity scrip (kept on disk for a day).
    pub async fn scrips(&self) -> Result<Arc<Vec<Scrip>>> {
        let mut cached = self.scrips.lock().await;
        if let Some(list) = cached.as_ref() {
            return Ok(list.clone());
        }
        let text = self
            .session
            .get_text_on_disk(
                "bse_scrips.json",
                "/ListofScripData/w",
                &[
                    ("Group", ""),
                    ("Scripcode", ""),
                    ("industry", ""),
                    ("segment", "Equity"),
                    ("status", "Active"),
                ],
                Duration::from_secs(24 * 3_600),
                |t| matches!(t.trim_start().chars().next(), Some('[' | '{')),
            )
            .await
            .map_err(|e| self.fail(e))?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|_| self.format_changed("scrip list is not JSON"))?;
        let list = Arc::new(parse_scrip_list(&json).map_err(|w| self.format_changed(w))?);
        if list.is_empty() {
            return Err(self.format_changed("scrip list is empty"));
        }
        *cached = Some(list.clone());
        Ok(list)
    }

    /// The scrip code for a `.BO` symbol; `None` if BSE lists no such id.
    pub async fn code(&self, symbol: &str) -> Result<Option<String>> {
        match target(symbol) {
            None => Ok(None),
            Some(Target::Code(code)) => Ok(Some(code)),
            Some(Target::Id(id)) => Ok(self
                .scrips()
                .await?
                .iter()
                .find(|s| s.id == id)
                .map(|s| s.code.clone())),
        }
    }

    /// `.BO` symbols of one BSE group's active equities (`A`, `B`, `T`, ...):
    /// a snapshot of who is in the group, from the scrip list endpoint's
    /// `Group` filter. Prices come separately.
    pub async fn group_members(&self, group: &str) -> Result<Vec<String>> {
        let group = group.trim().to_ascii_uppercase();
        if group.is_empty() || !group.chars().all(|c| c.is_ascii_alphanumeric()) {
            anyhow::bail!("BSE group must be a letter or two, like A or B");
        }
        let json = self
            .session
            .get_json(
                "/ListofScripData/w",
                &[
                    ("Group", &group),
                    ("Scripcode", ""),
                    ("industry", ""),
                    ("segment", "Equity"),
                    ("status", "Active"),
                ],
                Duration::from_secs(3_600),
            )
            .await
            .map_err(|e| self.fail(e))?;
        let scrips = parse_scrip_list(&json).map_err(|w| self.format_changed(w))?;
        if scrips.is_empty() {
            return Err(self.format_changed("group list is empty"));
        }
        Ok(scrips
            .into_iter()
            .map(|s| format!("{}.BO", if s.id.is_empty() { &s.code } else { &s.id }))
            .collect())
    }

    /// Announcements a company filed in the last `days` days; with
    /// `results_only`, just the quarterly and annual results.
    pub async fn announcements(
        &self,
        symbol: &str,
        days: i64,
        results_only: bool,
    ) -> Result<Vec<Announcement>> {
        let code = self
            .code(symbol)
            .await?
            .ok_or_else(|| anyhow::anyhow!("BSE lists no scrip {}", symbol))?;
        let today = (chrono::Utc::now() + chrono::Duration::seconds(IST_OFFSET)).date_naive();
        let from = (today - chrono::Duration::days(days.clamp(1, 366)))
            .format("%Y%m%d")
            .to_string();
        let to = today.format("%Y%m%d").to_string();
        let json = self
            .session
            .get_json(
                "/AnnGetData/w",
                &[
                    ("strCat", if results_only { "Result" } else { "-1" }),
                    ("strPrevDate", &from),
                    ("strScrip", &code),
                    ("strSearch", "P"),
                    ("strToDate", &to),
                    ("strType", "C"),
                    ("pageno", "1"),
                ],
                Duration::from_secs(600),
            )
            .await
            .map_err(|e| self.fail(e))?;
        parse_announcements(&json).map_err(|w| self.format_changed(w))
    }

    pub async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let mut out = Vec::new();
        let mut first_err = None;
        for symbol in symbols {
            if target(symbol).is_none() {
                continue;
            }
            match self.quote(symbol).await {
                Ok(Some(mut q)) => {
                    q.symbol = Some(symbol.to_string());
                    out.push(q);
                }
                Ok(None) => {}
                Err(e) => {
                    let stop = self.session.is_blocked();
                    first_err.get_or_insert(e);
                    if stop {
                        break;
                    }
                }
            }
        }
        match first_err {
            Some(e) if out.is_empty() => Err(e),
            _ => Ok(out),
        }
    }

    async fn quote(&self, symbol: &str) -> Result<Option<Quote>> {
        let Some(code) = self.code(symbol).await? else {
            return Ok(None);
        };
        let json = self
            .session
            .get_json(
                "/getScripHeaderData/w",
                &[("Debtflag", ""), ("scripcode", &code), ("seriesid", "")],
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| self.fail(e))?;
        parse_header(&json).map_err(|w| self.format_changed(w))
    }

    pub async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        let code = self
            .code(symbol)
            .await?
            .ok_or_else(|| anyhow::anyhow!("BSE lists no scrip {}", symbol))?;
        if let Some(step) = interval_seconds(interval) {
            if range != "1d" {
                anyhow::bail!("BSE serves intraday prices for today's session only");
            }
            let json = self
                .session
                .get_json(
                    "/StockReachGraph/w",
                    &[
                        ("scripcode", &code),
                        ("flag", "0"),
                        ("fromdate", ""),
                        ("todate", ""),
                        ("seriesid", ""),
                    ],
                    Duration::from_secs(60),
                )
                .await
                .map_err(|e| self.fail(e))?;
            let points = parse_graph(&json).map_err(|w| self.format_changed(w))?;
            let bars = bars_from_points(&points, step);
            if bars.is_empty() {
                anyhow::bail!("BSE has no intraday prices for {} yet today", symbol);
            }
            return Ok(super::timed_chart(symbol, &bars, Some("INR")));
        }
        let Some(step) = super::Resample::for_interval(interval) else {
            anyhow::bail!("BSE does not serve {} bars", interval);
        };
        let today = (chrono::Utc::now() + chrono::Duration::seconds(IST_OFFSET)).date_naive();
        let start = super::range_start(range, today)
            .filter(|s| *s >= today - chrono::Duration::days(HISTORY_DAYS))
            .ok_or_else(|| anyhow::anyhow!("BSE history requests stop at {} days", HISTORY_DAYS))?;
        let (from, to) = (
            start.format("%d/%m/%Y").to_string(),
            today.format("%d/%m/%Y").to_string(),
        );
        let csv = self
            .session
            .get_text(
                "/StockPriceCSVDownload/w",
                &[
                    ("pageType", "0"),
                    ("rbType", "D"),
                    ("Scode", &code),
                    ("FDates", &from),
                    ("TDates", &to),
                ],
                Duration::from_secs(600),
            )
            .await
            .map_err(|e| self.fail(e))?;
        let bars = parse_price_csv(&csv).map_err(|w| self.format_changed(w))?;
        let series = Series {
            bars,
            currency: Some("INR".into()),
        }
        .sorted()
        .since(Some(start));
        if !series.covers(Some(start)) {
            anyhow::bail!("BSE has no history back to {} for {}", start, symbol);
        }
        Ok(series.resample(step).into_chart(symbol))
    }

    /// Search the scrip list by id, code or company name.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let scrips = self.scrips().await?;
        let q = query.trim();
        if !q.is_empty() && q.chars().all(|c| c.is_ascii_digit()) {
            return Ok(scrips
                .iter()
                .filter(|s| s.code.starts_with(q))
                .take(10)
                .map(|s| SearchResult {
                    symbol: Some(format!(
                        "{}.BO",
                        if s.id.is_empty() { &s.code } else { &s.id }
                    )),
                    short_name: Some(s.name.clone()),
                    long_name: None,
                    quote_type: Some("EQUITY".into()),
                    exchange: Some("BSE".into()),
                })
                .collect());
        }
        let list: Vec<(String, String)> = scrips
            .iter()
            .map(|s| {
                (
                    if s.id.is_empty() {
                        s.code.clone()
                    } else {
                        s.id.clone()
                    },
                    s.name.clone(),
                )
            })
            .collect();
        Ok(search_list(&list, q, ".BO", "BSE"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::session::DiskCache;
    use crate::testutil::MockServer;

    fn json(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    #[test]
    fn symbols_map_to_codes_and_ids() {
        assert_eq!(target("500325.BO"), Some(Target::Code("500325".into())));
        assert_eq!(target("reliance.bo"), Some(Target::Id("RELIANCE".into())));
        assert_eq!(target("RELIANCE.NS"), None);
        assert_eq!(target("^BSESN"), None);
        assert_eq!(target(".BO"), None);
    }

    #[test]
    fn scrip_list_reads_codes_ids_and_names() {
        let list = parse_scrip_list(&json(include_str!("fixtures/bse_scrip_list.json"))).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(
            list[0],
            Scrip {
                code: "500325".into(),
                id: "RELIANCE".into(),
                name: "Reliance Industries Ltd".into()
            }
        );
        // Numeric codes and the older wrapped shape both parse.
        let wrapped = json(
            r#"{"Table":[{"SCRIP_CD":532540,"scrip_id":"TCS","Scrip_Name":"Tata Consultancy Services Ltd"}]}"#,
        );
        assert_eq!(parse_scrip_list(&wrapped).unwrap()[0].code, "532540");
        assert!(parse_scrip_list(&json(r#"{"rows":[]}"#)).is_err());
    }

    #[test]
    fn header_maps_the_quote() {
        let q = parse_header(&json(include_str!("fixtures/bse_header.json")))
            .unwrap()
            .unwrap();
        assert_eq!(q.regular_market_price, Some(1451.65));
        assert_eq!(q.regular_market_change, Some(-0.75));
        assert_eq!(q.regular_market_change_percent, Some(-0.05));
        assert_eq!(q.regular_market_previous_close, Some(1452.4));
        assert_eq!(q.regular_market_open, Some(1455.0));
        assert_eq!(q.regular_market_day_high, Some(1460.0));
        assert_eq!(q.regular_market_day_low, Some(1443.15));
        assert_eq!(q.short_name.as_deref(), Some("Reliance Industries Ltd"));
        assert_eq!(q.data_source.as_deref(), Some("BSE"));
    }

    #[test]
    fn header_without_price_is_none_and_unknown_shape_is_an_error() {
        let empty = json(r#"{"Header":null,"CurrRate":null}"#);
        assert!(parse_header(&empty).unwrap().is_none());
        assert!(parse_header(&json(r#"{"quote":{"ltp":1}}"#)).is_err());
        // Thousands separators and a missing change.
        let q = parse_header(&json(
            r#"{"Header":{"PrevClose":"2,000.00","LTP":"2,100.00"},"CurrRate":{}}"#,
        ))
        .unwrap()
        .unwrap();
        assert_eq!(q.regular_market_price, Some(2100.0));
        assert_eq!(q.regular_market_change, Some(100.0));
        assert_eq!(q.regular_market_change_percent, Some(5.0));
    }

    #[test]
    fn graph_points_are_ist_and_may_be_a_string() {
        let points =
            parse_graph(&json(include_str!("fixtures/bse_stock_reach_graph.json"))).unwrap();
        assert_eq!(points.len(), 4);
        let first = chrono::DateTime::from_timestamp(points[0].0, 0).unwrap();
        assert_eq!(first.to_rfc3339(), "2025-10-24T03:45:00+00:00");
        let bars = bars_from_points(&points, 300);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].high, 1457.0);
        let as_list = json(r#"{"Data":[{"dttm":"Fri Oct 24 2025 09:15:00","vale1":10.5}]}"#);
        assert_eq!(parse_graph(&as_list).unwrap().len(), 1);
        assert!(parse_graph(&json(r#"{"Data":""}"#)).unwrap().is_empty());
        assert!(parse_graph(&json(r#"{"points":[]}"#)).is_err());
    }

    #[test]
    fn price_csv_reads_columns_by_name() {
        let bars = parse_price_csv(include_str!("fixtures/bse_stock_prices.csv")).unwrap();
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].date, NaiveDate::from_ymd_opt(2025, 10, 24).unwrap());
        assert_eq!(bars[0].open, Some(1455.0));
        assert_eq!(bars[0].close, Some(1451.65));
        assert_eq!(bars[0].volume, Some(412_345));
        assert_eq!(bars[1].high, Some(1462.0));
        assert!(parse_price_csv("<html>error</html>").is_err());
        assert!(parse_price_csv("").is_err());
    }

    fn bse_at(server: &MockServer) -> Bse {
        Bse::new(Arc::new(Session::for_tests(
            site(&server.url),
            DiskCache::Off,
        )))
    }

    #[tokio::test]
    async fn quotes_by_id_through_the_scrip_list() {
        let server = MockServer::start(vec![
            (200, include_str!("fixtures/bse_scrip_list.json").into()),
            (200, include_str!("fixtures/bse_header.json").into()),
        ])
        .await;
        let bse = bse_at(&server);
        let quotes = bse.quotes(&["RELIANCE.BO", "TCS.NS"]).await.unwrap();
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].symbol.as_deref(), Some("RELIANCE.BO"));
        let reqs = server.requests();
        assert!(reqs[0].target.starts_with("/ListofScripData/w?"));
        assert_eq!(
            reqs[1].target,
            "/getScripHeaderData/w?Debtflag=&scripcode=500325&seriesid="
        );
        assert_eq!(reqs[1].header("origin"), Some("https://www.bseindia.com"));
        assert_eq!(reqs[1].header("referer"), Some("https://www.bseindia.com/"));
    }

    #[tokio::test]
    async fn numeric_codes_skip_the_scrip_list() {
        let server =
            MockServer::start(vec![(200, include_str!("fixtures/bse_header.json").into())]).await;
        let bse = bse_at(&server);
        let quotes = bse.quotes(&["500325.BO"]).await.unwrap();
        assert_eq!(quotes[0].regular_market_price, Some(1451.65));
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn search_matches_ids_codes_and_names() {
        let server = MockServer::start(vec![(
            200,
            include_str!("fixtures/bse_scrip_list.json").into(),
        )])
        .await;
        let bse = bse_at(&server);
        let by_name = bse.search("consultancy").await.unwrap();
        assert_eq!(by_name[0].symbol.as_deref(), Some("TCS.BO"));
        let by_code = bse.search("5003").await.unwrap();
        assert_eq!(by_code[0].symbol.as_deref(), Some("RELIANCE.BO"));
        let by_id = bse.search("reliance").await.unwrap();
        assert_eq!(by_id[0].exchange.as_deref(), Some("BSE"));
        assert_eq!(server.requests().len(), 1, "the scrip list is loaded once");
    }
}

#[cfg(test)]
mod filings_tests {
    use super::*;
    use crate::providers::session::DiskCache;
    use crate::testutil::MockServer;

    const ANNOUNCEMENTS: &str = r#"{"Table":[
        {"SCRIP_CD":500325,"SLONGNAME":"Reliance Industries Ltd","NEWSSUB":"Unaudited Financial Results for the quarter ended 30 September 2025","CATEGORYNAME":"Result","NEWS_DT":"2025-10-17T15:08:35.43","ATTACHMENTNAME":"abc-123.pdf"},
        {"SCRIP_CD":"500325","SLONGNAME":"Reliance Industries Ltd","HEADLINE":"Board Meeting Intimation","CATEGORYNAME":"Board Meeting","NEWS_DT":"2025-10-03T10:00:00"},
        {"SCRIP_CD":"500325","NEWSSUB":"no date"}
    ],"Table1":[{"ROWCNT":2}]}"#;

    fn bse_at(server: &MockServer) -> Bse {
        Bse::new(Arc::new(Session::for_tests(
            site(&server.url),
            DiskCache::Off,
        )))
    }

    #[test]
    fn announcements_read_the_table_and_skip_undated_rows() {
        let rows = parse_announcements(&serde_json::from_str(ANNOUNCEMENTS).unwrap()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].code, "500325");
        assert_eq!(rows[0].category, "Result");
        assert_eq!(rows[0].date.to_string(), "2025-10-17");
        assert_eq!(
            rows[0].attachment.as_deref(),
            Some("https://www.bseindia.com/xml-data/corpfiling/AttachLive/abc-123.pdf")
        );
        assert_eq!(rows[1].headline, "Board Meeting Intimation");
        assert_eq!(rows[1].attachment, None);
        assert!(parse_announcements(&serde_json::json!({"rows": []})).is_err());
        assert!(
            parse_announcements(&serde_json::json!({"Table": []}))
                .unwrap()
                .is_empty()
        );
        let moved = serde_json::json!({"Table": [{"SCRIP_CD": 1, "NEWS_DT": "17 Oct 2025"}]});
        assert!(parse_announcements(&moved).is_err());
    }

    #[tokio::test]
    async fn results_ask_for_the_result_category_of_one_scrip() {
        let server = MockServer::start(vec![(200, ANNOUNCEMENTS.into())]).await;
        let rows = bse_at(&server)
            .announcements("500325.BO", 90, true)
            .await
            .unwrap();
        assert_eq!(rows.len(), 2);
        let target = &server.requests()[0].target;
        assert!(target.starts_with("/AnnGetData/w?"));
        assert!(target.contains("strCat=Result"));
        assert!(target.contains("strScrip=500325"));
    }

    #[tokio::test]
    async fn a_group_lists_its_members_as_bo_symbols() {
        let server = MockServer::start(vec![(
            200,
            include_str!("fixtures/bse_scrip_list.json").into(),
        )])
        .await;
        let bse = bse_at(&server);
        let members = bse.group_members("a").await.unwrap();
        assert_eq!(members[0], "RELIANCE.BO");
        assert!(server.requests()[0].target.contains("Group=A"));
        assert!(bse.group_members("A; drop").await.is_err());
    }
}
