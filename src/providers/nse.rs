//! NSE (<https://www.nseindia.com>), from the JSON endpoints behind its
//! public website — not a documented or licensed API. NSE's terms restrict
//! automated access and redistribution; StockWise reads the pages one person
//! would, slowly (see `session.rs`), for personal use. NSE can change or
//! block these endpoints at any time: StockWise then says so once and Yahoo
//! answers.
//!
//! | Data | Endpoint |
//! |------|----------|
//! | Equity quote | `/api/quote-equity?symbol=X` (+ `&section=trade_info` for volume and delivery %) |
//! | Index quotes | `/api/allIndices` (every index in one answer) |
//! | Index members with quotes | `/api/equity-stockIndices?index=NIFTY 50` |
//! | Intraday prices (today) | `/api/chart-databyindex?index=XEQN` (`&indices=true` for an index) |
//! | Daily equity history | `/api/historical/cm/equity?symbol=X&series=["EQ","BE"]&from=&to=` |
//! | Daily index history | `/api/historical/indicesHistory?indexType=NIFTY 50&from=&to=` |
//! | Search | `/api/search/autocomplete?q=`; the `EQUITY_L.csv` symbol list when that fails |
//!
//! docs/exchange-responses.md records the fields StockWise reads from each.

use super::session::{FetchError, Limiter, Session, Site};
use super::{Bar, Series, TimedBar, csv_fields, int, num};
use crate::api::{ChartData, Quote, SearchResult};
use anyhow::Result;
use chrono::NaiveDate;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const BASE_URL: &str = "https://www.nseindia.com";
/// Static files (the equity list) live on NSE's archive host.
pub const ARCHIVE_URL: &str = "https://nsearchives.nseindia.com";
/// Where quotes go when NSE can't answer.
const FALLBACK: &str = "using Yahoo Finance instead";
/// IST is UTC+5:30.
const IST_OFFSET: i64 = 19_800;
/// NSE's historical endpoints answer at most a year per request.
const HISTORY_CHUNK_DAYS: i64 = 365;
/// Daily history NSE is asked for (six requests); longer ranges and `max`
/// go to Yahoo.
pub const HISTORY_DAYS: i64 = 5 * 366;
/// More uncached symbols than this in one quote request skip the
/// `trade_info` call (volume, delivery %), halving the requests.
const DETAIL_LIMIT: usize = 3;
/// Most uncached symbols one quote request may ask NSE for; bigger scans go
/// to Yahoo in one request instead of waiting on NSE's rate limit.
pub const QUOTE_BATCH: u32 = 12;
/// Quotes from an index snapshot (`equity-stockIndices`) count as fresh
/// this long.
const SNAPSHOT_FRESH: Duration = Duration::from_secs(60);

pub fn site(base: &str) -> Site {
    Site {
        name: "NSE",
        base: base.trim_end_matches('/').to_string(),
        home: Some(format!("{}/", base.trim_end_matches('/'))),
        referer: format!("{}/", base.trim_end_matches('/')),
        origin: None,
    }
}

/// The process-wide NSE session (one cookie handshake, one rate limit).
pub fn shared_session() -> Arc<Session> {
    static SHARED: OnceLock<Arc<Session>> = OnceLock::new();
    SHARED
        .get_or_init(|| Arc::new(Session::new(site(BASE_URL), Limiter::polite())))
        .clone()
}

/// Yahoo index symbols and NSE's names for them.
pub const INDICES: &[(&str, &str)] = &[
    ("^NSEI", "NIFTY 50"),
    ("^NSEBANK", "NIFTY BANK"),
    ("^CNXIT", "NIFTY IT"),
    ("^NSMIDCP", "NIFTY NEXT 50"),
    ("^CNX100", "NIFTY 100"),
    ("^CNX200", "NIFTY 200"),
    ("^CRSLDX", "NIFTY 500"),
    ("^NSEMDCP50", "NIFTY MIDCAP 50"),
    ("^CNXPHARMA", "NIFTY PHARMA"),
    ("^CNXAUTO", "NIFTY AUTO"),
    ("^CNXFMCG", "NIFTY FMCG"),
    ("^CNXMETAL", "NIFTY METAL"),
    ("^CNXREALTY", "NIFTY REALTY"),
    ("^CNXENERGY", "NIFTY ENERGY"),
    ("^CNXINFRA", "NIFTY INFRA"),
    ("^CNXPSUBANK", "NIFTY PSU BANK"),
    ("^CNXMEDIA", "NIFTY MEDIA"),
    ("^INDIAVIX", "INDIA VIX"),
];

/// What a Yahoo symbol is on NSE.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// An equity symbol (`RELIANCE`, `M&M`, `BAJAJ-AUTO`).
    Equity(String),
    /// An index by NSE's name (`NIFTY 50`).
    Index(&'static str),
}

/// `RELIANCE.NS` → `Equity("RELIANCE")`, `^NSEI` → `Index("NIFTY 50")`;
/// `None` for anything NSE doesn't list (other suffixes, BSE's SENSEX).
pub fn target(symbol: &str) -> Option<Target> {
    let upper = symbol.trim().to_ascii_uppercase();
    if upper.starts_with('^') {
        return INDICES
            .iter()
            .find(|(y, _)| *y == upper)
            .map(|(_, name)| Target::Index(name));
    }
    let base = upper.strip_suffix(".NS")?;
    (!base.is_empty() && !base.contains('.')).then(|| Target::Equity(base.to_string()))
}

/// NSE index name → Yahoo symbol (`NIFTY 50` → `^NSEI`).
pub fn index_symbol(name: &str) -> Option<&'static str> {
    let name = name.trim();
    INDICES
        .iter()
        .find(|(_, n)| n.eq_ignore_ascii_case(name))
        .map(|(y, _)| *y)
}

// ── Parsers (pure; fixture-tested) ──

/// A positive price; NSE writes 0 (or `"-"`) for "none".
fn pos(v: &Value) -> Option<f64> {
    num(v).filter(|x| *x > 0.0)
}

/// `24-Oct-2025`, `24-OCT-2025`, `2025-10-24` or `24-10-2025`.
pub fn parse_date(s: &str) -> Option<NaiveDate> {
    // Drop a time of day (`24-Oct-2025 16:00:00`).
    let day = s.split_whitespace().next()?;
    ["%d-%b-%Y", "%Y-%m-%d", "%d-%m-%Y"]
        .iter()
        .find_map(|f| NaiveDate::parse_from_str(day, f).ok())
}

fn base_quote() -> Quote {
    Quote {
        currency: Some("INR".into()),
        exchange: Some("NSE".into()),
        data_source: Some("NSE".into()),
        ..Default::default()
    }
}

/// `quote-equity` → a quote. `Ok(None)` when NSE has no price for the symbol
/// (unknown or suspended); `Err` when the answer isn't shaped like a quote
/// at all — NSE changed its format.
pub fn parse_quote(json: &Value) -> Result<Option<Quote>, &'static str> {
    let Some(obj) = json.as_object() else {
        return Err("quote: not an object");
    };
    let price_info = &json["priceInfo"];
    if !price_info.is_object() {
        // `{}` or `{"msg": "no data"}` for an unknown symbol; anything else
        // without `priceInfo` is a format NSE didn't use to send.
        let empty_or_message = obj.keys().all(|k| {
            matches!(
                k.as_str(),
                "msg" | "message" | "error" | "info" | "metadata"
            )
        });
        return if empty_or_message {
            Ok(None)
        } else {
            Err("quote: no priceInfo")
        };
    }
    let Some(price) = pos(&price_info["lastPrice"]) else {
        return Ok(None);
    };
    let mut q = base_quote();
    q.short_name = json["info"]["companyName"].as_str().map(str::to_string);
    q.regular_market_price = Some(price);
    q.regular_market_change = num(&price_info["change"]);
    q.regular_market_change_percent = num(&price_info["pChange"]);
    q.regular_market_previous_close = pos(&price_info["previousClose"]);
    q.regular_market_open = pos(&price_info["open"]);
    q.regular_market_day_high = pos(&price_info["intraDayHighLow"]["max"]);
    q.regular_market_day_low = pos(&price_info["intraDayHighLow"]["min"]);
    q.fifty_two_week_high = pos(&price_info["weekHighLow"]["max"]);
    q.fifty_two_week_low = pos(&price_info["weekHighLow"]["min"]);
    q.upper_circuit = pos(&price_info["upperCP"]);
    q.lower_circuit = pos(&price_info["lowerCP"]);
    Ok(Some(q))
}

/// Add volume, market cap and delivery % from `quote-equity?section=trade_info`.
/// Returns whether the answer had the expected shape.
pub fn apply_trade_info(json: &Value, q: &mut Quote) -> bool {
    let book = &json["marketDeptOrderBook"];
    let delivery = &json["securityWiseDP"];
    if !book.is_object() && !delivery.is_object() {
        return false;
    }
    let trade = &book["tradeInfo"];
    // Volume in lakhs of shares, market cap in crores of rupees.
    if let Some(lakhs) = num(&trade["totalTradedVolume"]).filter(|v| *v >= 0.0) {
        q.regular_market_volume = Some((lakhs * 1e5).round() as u64);
    }
    if let Some(crores) = pos(&trade["totalMarketCap"]) {
        q.market_cap = Some(crores * 1e7);
    }
    q.delivery_percent =
        num(&delivery["deliveryToTradedQuantity"]).filter(|p| (0.0..=100.0).contains(p));
    true
}

/// `allIndices` → `(Yahoo symbol, quote)` for every index StockWise maps.
pub fn parse_all_indices(json: &Value) -> Result<Vec<(&'static str, Quote)>, &'static str> {
    let rows = json["data"].as_array().ok_or("allIndices: no data list")?;
    Ok(rows
        .iter()
        .filter_map(|r| {
            let name = r["indexSymbol"].as_str().or(r["index"].as_str())?;
            let symbol = index_symbol(name)?;
            let price = pos(&r["last"])?;
            let mut q = base_quote();
            q.symbol = Some(symbol.to_string());
            q.short_name = Some(name.trim().to_string());
            q.regular_market_price = Some(price);
            q.regular_market_change = num(&r["variation"]);
            q.regular_market_change_percent = num(&r["percentChange"]);
            q.regular_market_open = pos(&r["open"]);
            q.regular_market_day_high = pos(&r["high"]);
            q.regular_market_day_low = pos(&r["low"]);
            q.regular_market_previous_close = pos(&r["previousClose"]);
            q.fifty_two_week_high = pos(&r["yearHigh"]);
            q.fifty_two_week_low = pos(&r["yearLow"]);
            Some((symbol, q))
        })
        .collect())
}

/// One index and its members, each with today's quote.
#[derive(Debug, Default)]
pub struct IndexSnapshot {
    pub name: String,
    pub index: Option<Quote>,
    /// In NSE's order (by free-float market cap).
    pub members: Vec<Quote>,
}

/// `equity-stockIndices?index=NIFTY 50` → the index and its members, with
/// Yahoo symbols (`RELIANCE.NS`).
pub fn parse_index_snapshot(json: &Value) -> Result<IndexSnapshot, &'static str> {
    let rows = json["data"]
        .as_array()
        .ok_or("index members: no data list")?;
    let name = json["name"]
        .as_str()
        .or(json["metadata"]["indexName"].as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let mut snap = IndexSnapshot {
        name: name.clone(),
        ..Default::default()
    };
    for r in rows {
        let Some(symbol) = r["symbol"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let is_index = r["priority"].as_i64() == Some(1) || symbol.eq_ignore_ascii_case(&name);
        let mut q = base_quote();
        q.regular_market_price = pos(&r["lastPrice"]);
        q.regular_market_change = num(&r["change"]);
        q.regular_market_change_percent = num(&r["pChange"]);
        q.regular_market_open = pos(&r["open"]);
        q.regular_market_day_high = pos(&r["dayHigh"]);
        q.regular_market_day_low = pos(&r["dayLow"]);
        q.regular_market_previous_close = pos(&r["previousClose"]);
        q.fifty_two_week_high = pos(&r["yearHigh"]);
        q.fifty_two_week_low = pos(&r["yearLow"]);
        q.regular_market_volume = int(&r["totalTradedVolume"]);
        if is_index {
            q.symbol = index_symbol(symbol).map(str::to_string);
            q.short_name = Some(symbol.to_string());
            snap.index = Some(q);
        } else {
            q.symbol = Some(format!("{}.NS", symbol.to_ascii_uppercase()));
            q.short_name = r["meta"]["companyName"].as_str().map(str::to_string);
            snap.members.push(q);
        }
    }
    Ok(snap)
}

/// Unix seconds for NSE chart points. NSE writes IST wall-clock time as if
/// it were UTC (09:15 IST arrives as 09:15Z); the session's first point
/// tells which: a real UTC stamp for 09:15 IST is 03:45Z.
fn chart_seconds(points: &mut [(i64, f64)]) {
    let Some(&(first, _)) = points.first() else {
        return;
    };
    let utc_hour = first.rem_euclid(86_400) / 3_600;
    if utc_hour >= 6 {
        for p in points.iter_mut() {
            p.0 -= IST_OFFSET;
        }
    }
}

/// `chart-databyindex` → `(unix seconds, price)` points of today's session,
/// pre-open points (`"PO"`) left out.
pub fn parse_chart_points(json: &Value) -> Result<Vec<(i64, f64)>, &'static str> {
    let raw = json
        .get("grapthData")
        .or_else(|| json.get("graphData"))
        .and_then(Value::as_array)
        .ok_or("intraday: no grapthData")?;
    let mut points: Vec<(i64, f64)> = raw
        .iter()
        .filter_map(|p| {
            let p = p.as_array()?;
            if p.get(2)
                .and_then(Value::as_str)
                .is_some_and(|tag| tag.eq_ignore_ascii_case("PO"))
            {
                return None;
            }
            let ms = num(p.first()?)?;
            let price = pos(p.get(1)?)?;
            Some(((ms / 1000.0) as i64, price))
        })
        .collect();
    points.sort_by_key(|p| p.0);
    chart_seconds(&mut points);
    Ok(points)
}

/// Seconds per bar for a Yahoo intraday interval.
pub fn interval_seconds(interval: &str) -> Option<i64> {
    match interval {
        "1m" => Some(60),
        "2m" => Some(120),
        "5m" => Some(300),
        "15m" => Some(900),
        "30m" => Some(1_800),
        "60m" | "1h" => Some(3_600),
        "90m" => Some(5_400),
        _ => None,
    }
}

/// OHLC bars of `step` seconds from price points, aligned to the 09:15 IST
/// open the way Yahoo aligns NSE bars. Points before the open are dropped.
/// No volume: NSE's intraday chart carries prices only.
pub fn bars_from_points(points: &[(i64, f64)], step: i64) -> Vec<TimedBar> {
    let mut bars: Vec<TimedBar> = Vec::new();
    for &(ts, price) in points {
        let local = ts + IST_OFFSET;
        let open = local - local.rem_euclid(86_400) + 9 * 3_600 + 15 * 60;
        if local < open {
            continue;
        }
        let start = open + (local - open) / step * step - IST_OFFSET;
        match bars.last_mut() {
            Some(bar) if bar.ts == start => {
                bar.high = bar.high.max(price);
                bar.low = bar.low.min(price);
                bar.close = price;
            }
            _ => bars.push(TimedBar {
                ts: start,
                open: price,
                high: price,
                low: price,
                close: price,
                volume: None,
            }),
        }
    }
    bars
}

/// Daily bars from `historical/cm/equity` (or the newer
/// `historicalOR/...` answer, which uses the same field names).
pub fn parse_equity_history(json: &Value) -> Result<Vec<Bar>, &'static str> {
    let rows = json["data"].as_array().ok_or("history: no data list")?;
    Ok(rows
        .iter()
        .filter(|r| {
            r["CH_SERIES"]
                .as_str()
                .is_none_or(|s| matches!(s.trim(), "EQ" | "BE" | "BZ" | "SM" | "ST"))
        })
        .filter_map(|r| {
            let date = r["CH_TIMESTAMP"]
                .as_str()
                .or(r["mTIMESTAMP"].as_str())
                .and_then(parse_date)?;
            Some(Bar {
                date,
                open: pos(&r["CH_OPENING_PRICE"]),
                high: pos(&r["CH_TRADE_HIGH_PRICE"]),
                low: pos(&r["CH_TRADE_LOW_PRICE"]),
                close: Some(pos(&r["CH_CLOSING_PRICE"])?),
                volume: int(&r["CH_TOT_TRADED_QTY"]),
            })
        })
        .collect())
}

/// Daily bars from `historical/indicesHistory`, with traded quantity from
/// its turnover records.
pub fn parse_index_history(json: &Value) -> Result<Vec<Bar>, &'static str> {
    let data = &json["data"];
    let rows = data["indexCloseOnlineRecords"]
        .as_array()
        .ok_or("index history: no indexCloseOnlineRecords")?;
    let volumes: HashMap<NaiveDate, u64> = data["indexTurnoverRecords"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| {
            Some((
                parse_date(r["HIT_TIMESTAMP"].as_str()?)?,
                int(&r["HIT_TRADED_QTY"])?,
            ))
        })
        .collect();
    Ok(rows
        .iter()
        .filter_map(|r| {
            let date = parse_date(r["EOD_TIMESTAMP"].as_str()?)?;
            Some(Bar {
                date,
                open: pos(&r["EOD_OPEN_INDEX_VAL"]),
                high: pos(&r["EOD_HIGH_INDEX_VAL"]),
                low: pos(&r["EOD_LOW_INDEX_VAL"]),
                close: Some(pos(&r["EOD_CLOSE_INDEX_VAL"])?),
                volume: volumes.get(&date).copied(),
            })
        })
        .collect())
}

/// `search/autocomplete` → equity matches as Yahoo symbols.
pub fn parse_autocomplete(json: &Value) -> Result<Vec<SearchResult>, &'static str> {
    let rows = json["symbols"]
        .as_array()
        .ok_or("search: no symbols list")?;
    let mut seen = std::collections::HashSet::new();
    Ok(rows
        .iter()
        .filter(|r| {
            ["type", "result_sub_type"].iter().all(|k| {
                r[*k]
                    .as_str()
                    .is_none_or(|t| t.to_ascii_lowercase().contains("equity"))
            })
        })
        .filter_map(|r| {
            let symbol = r["symbol"].as_str()?.trim().to_ascii_uppercase();
            if symbol.is_empty() || !seen.insert(symbol.clone()) {
                return None;
            }
            Some(SearchResult {
                symbol: Some(format!("{}.NS", symbol)),
                short_name: r["symbol_info"].as_str().map(str::to_string),
                long_name: None,
                quote_type: Some("EQUITY".into()),
                exchange: Some("NSE".into()),
            })
        })
        .take(10)
        .collect())
}

/// NSE's list of listed equities (`EQUITY_L.csv`: `SYMBOL,NAME OF COMPANY,
/// SERIES,...`) → `(symbol, company name)`.
pub fn parse_equity_list(csv: &str) -> Vec<(String, String)> {
    let mut lines = csv.lines();
    let Some(header) = lines.next() else {
        return Vec::new();
    };
    let cols: Vec<String> = csv_fields(header)
        .iter()
        .map(|c| c.trim().to_ascii_uppercase())
        .collect();
    let (Some(sym), Some(name)) = (
        cols.iter().position(|c| c == "SYMBOL"),
        cols.iter().position(|c| c.contains("NAME")),
    ) else {
        return Vec::new();
    };
    lines
        .filter_map(|line| {
            let f = csv_fields(line);
            let s = f.get(sym)?.trim().to_ascii_uppercase();
            (!s.is_empty()).then(|| (s, f.get(name).map_or("", |n| n.trim()).to_string()))
        })
        .collect()
}

/// Search a symbol list: exact symbol first, then symbols starting with the
/// query, then names containing it.
pub fn search_list(
    list: &[(String, String)],
    query: &str,
    suffix: &str,
    exchange: &str,
) -> Vec<SearchResult> {
    let q = query.trim().to_ascii_uppercase();
    if q.is_empty() {
        return Vec::new();
    }
    let rank = |(sym, name): &(String, String)| {
        if *sym == q {
            Some(0)
        } else if sym.starts_with(&q) {
            Some(1)
        } else if name.to_ascii_uppercase().contains(&q) {
            Some(2)
        } else {
            None
        }
    };
    let mut hits: Vec<(u8, &(String, String))> = list
        .iter()
        .filter_map(|e| rank(e).map(|r| (r, e)))
        .collect();
    hits.sort_by_key(|(r, e)| (*r, e.0.len()));
    hits.into_iter()
        .take(10)
        .map(|(_, (sym, name))| SearchResult {
            symbol: Some(format!("{}{}", sym, suffix)),
            short_name: (!name.is_empty()).then(|| name.clone()),
            long_name: None,
            quote_type: Some("EQUITY".into()),
            exchange: Some(exchange.into()),
        })
        .collect()
}

// ── Client ──

/// `dd-mm-yyyy` windows of at most `HISTORY_CHUNK_DAYS` from `start` to
/// `end`, oldest first.
pub fn history_windows(start: NaiveDate, end: NaiveDate) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut from = start;
    while from <= end {
        let to = (from + chrono::Duration::days(HISTORY_CHUNK_DAYS - 1)).min(end);
        out.push((
            from.format("%d-%m-%Y").to_string(),
            to.format("%d-%m-%Y").to_string(),
        ));
        from = to + chrono::Duration::days(1);
    }
    out
}

/// The NSE provider: quotes, intraday and daily charts, search and index
/// members, through one polite [`Session`].
pub struct Nse {
    session: Arc<Session>,
    /// Host of static files (`ARCHIVE_URL`; a mock in tests).
    archive: String,
    /// Quotes from index snapshots fetched this run, by Yahoo symbol.
    snapshot: Mutex<HashMap<String, (Instant, Quote)>>,
}

impl Nse {
    pub fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            archive: ARCHIVE_URL.to_string(),
            snapshot: Mutex::new(HashMap::new()),
        }
    }

    pub fn session(&self) -> &Session {
        &self.session
    }

    pub fn supports(&self, symbol: &str) -> bool {
        target(symbol).is_some()
    }

    fn snapshot_quote(&self, symbol: &str) -> Option<Quote> {
        let map = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        map.get(&symbol.to_ascii_uppercase())
            .filter(|(at, _)| at.elapsed() < SNAPSHOT_FRESH)
            .map(|(_, q)| q.clone())
    }

    /// NSE requests a quote of `symbols` would take: one per equity not in
    /// a fresh snapshot, plus one for all indices together.
    pub fn quote_cost(&self, symbols: &[&str]) -> u32 {
        let mut equities = 0;
        let mut index = false;
        for s in symbols {
            match target(s) {
                Some(Target::Equity(_)) if self.snapshot_quote(s).is_none() => equities += 1,
                Some(Target::Index(_)) => index = true,
                _ => {}
            }
        }
        equities + u32::from(index)
    }

    /// Requests NSE may still take this run before scans go to Yahoo.
    pub fn remaining(&self) -> u32 {
        if self.session.is_blocked() {
            0
        } else {
            QUOTE_BATCH
        }
    }

    /// Report a failure once and turn it into an error for the caller.
    fn fail(&self, e: FetchError) -> anyhow::Error {
        self.session.notice(&e, FALLBACK);
        anyhow::Error::new(e)
    }

    fn format_changed(&self, what: &str) -> anyhow::Error {
        self.fail(self.session.format_error(what))
    }

    /// Quotes for the symbols NSE lists, in request order; others are left out.
    pub async fn quotes(&self, symbols: &[&str]) -> Result<Vec<Quote>> {
        let uncached = symbols
            .iter()
            .filter(|s| {
                matches!(target(s), Some(Target::Equity(_))) && self.snapshot_quote(s).is_none()
            })
            .count();
        let detail = uncached <= DETAIL_LIMIT;
        let mut index_quotes: Option<Vec<(&'static str, Quote)>> = None;
        let mut out = Vec::new();
        let mut first_err = None;
        for symbol in symbols {
            let result = match target(symbol) {
                None => continue,
                Some(Target::Index(_)) => {
                    if index_quotes.is_none() {
                        index_quotes = Some(match self.all_indices().await {
                            Ok(list) => list,
                            Err(e) => {
                                first_err.get_or_insert(e);
                                Vec::new()
                            }
                        });
                    }
                    let upper = symbol.to_ascii_uppercase();
                    Ok(index_quotes
                        .as_ref()
                        .and_then(|l| l.iter().find(|(s, _)| *s == upper))
                        .map(|(_, q)| q.clone()))
                }
                Some(Target::Equity(nse)) => match self.snapshot_quote(symbol) {
                    Some(q) => Ok(Some(q)),
                    None => self.equity_quote(&nse, detail).await,
                },
            };
            match result {
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

    async fn equity_quote(&self, symbol: &str, detail: bool) -> Result<Option<Quote>> {
        let json = self
            .session
            .get_json(
                "/api/quote-equity",
                &[("symbol", symbol)],
                Duration::from_secs(30),
            )
            .await
            .map_err(|e| self.fail(e))?;
        let Some(mut q) = parse_quote(&json).map_err(|w| self.format_changed(w))? else {
            return Ok(None);
        };
        if detail {
            // Volume and delivery % are extras: the price stands without them.
            if let Ok(trade) = self
                .session
                .get_json(
                    "/api/quote-equity",
                    &[("symbol", symbol), ("section", "trade_info")],
                    Duration::from_secs(30),
                )
                .await
                && !apply_trade_info(&trade, &mut q)
            {
                self.session.notice(
                    &self.session.format_error("trade_info: no securityWiseDP"),
                    "quoting without delivery %",
                );
            }
        }
        Ok(Some(q))
    }

    async fn all_indices(&self) -> Result<Vec<(&'static str, Quote)>> {
        let json = self
            .session
            .get_json("/api/allIndices", &[], Duration::from_secs(30))
            .await
            .map_err(|e| self.fail(e))?;
        parse_all_indices(&json).map_err(|w| self.format_changed(w))
    }

    /// An index and its members with quotes; members' quotes are kept for
    /// this run so a scan of them costs no further NSE requests. Failures
    /// are returned, not announced: callers fall back differently.
    pub async fn index_snapshot(&self, index: &str) -> Result<IndexSnapshot> {
        let json = self
            .session
            .get_json(
                "/api/equity-stockIndices",
                &[("index", index)],
                Duration::from_secs(60),
            )
            .await?;
        let snap = parse_index_snapshot(&json)
            .map_err(|w| anyhow::Error::new(self.session.format_error(w)))?;
        let now = Instant::now();
        let mut map = self.snapshot.lock().unwrap_or_else(|e| e.into_inner());
        for q in snap.members.iter().chain(snap.index.iter()) {
            if let Some(sym) = &q.symbol
                && q.regular_market_price.is_some()
            {
                map.insert(sym.to_ascii_uppercase(), (now, q.clone()));
            }
        }
        Ok(snap)
    }

    /// Price history. Intraday intervals cover today's session only (range
    /// `1d`); daily, weekly and monthly bars reach back `HISTORY_DAYS`.
    pub async fn chart(&self, symbol: &str, range: &str, interval: &str) -> Result<ChartData> {
        let target =
            target(symbol).ok_or_else(|| anyhow::anyhow!("NSE does not list {}", symbol))?;
        if let Some(step) = interval_seconds(interval) {
            if range != "1d" {
                anyhow::bail!("NSE serves intraday prices for today's session only");
            }
            return self.intraday(symbol, &target, step).await;
        }
        let Some(step) = super::Resample::for_interval(interval) else {
            anyhow::bail!("NSE does not serve {} bars", interval);
        };
        let today = chrono::Utc::now()
            .with_timezone(&chrono::FixedOffset::east_opt(IST_OFFSET as i32).expect("valid offset"))
            .date_naive();
        let start = super::range_start(range, today)
            .filter(|s| *s >= today - chrono::Duration::days(HISTORY_DAYS))
            .ok_or_else(|| anyhow::anyhow!("NSE history requests stop at {} days", HISTORY_DAYS))?;
        let mut bars = Vec::new();
        for (from, to) in history_windows(start, today) {
            let chunk = match &target {
                Target::Equity(nse) => self.equity_history(nse, &from, &to).await?,
                Target::Index(name) => {
                    let json = self
                        .session
                        .get_json(
                            "/api/historical/indicesHistory",
                            &[("indexType", name), ("from", &from), ("to", &to)],
                            Duration::from_secs(600),
                        )
                        .await
                        .map_err(|e| self.fail(e))?;
                    parse_index_history(&json).map_err(|w| self.format_changed(w))?
                }
            };
            bars.extend(chunk);
        }
        let series = Series {
            bars,
            currency: Some("INR".into()),
        }
        .sorted()
        .since(Some(start));
        if !series.covers(Some(start)) {
            anyhow::bail!("NSE has no history back to {} for {}", start, symbol);
        }
        Ok(series.resample(step).into_chart(symbol))
    }

    async fn equity_history(&self, symbol: &str, from: &str, to: &str) -> Result<Vec<Bar>> {
        let ttl = Duration::from_secs(600);
        let classic = self
            .session
            .get_json(
                "/api/historical/cm/equity",
                &[
                    ("symbol", symbol),
                    ("series", r#"["EQ","BE"]"#),
                    ("from", from),
                    ("to", to),
                ],
                ttl,
            )
            .await;
        let json = match classic {
            Ok(json) => json,
            // The endpoint moved once before; try its newer home.
            Err(FetchError::NotFound(_)) => self
                .session
                .get_json(
                    "/api/historicalOR/generateSecurityWiseHistoricalData",
                    &[
                        ("from", from),
                        ("to", to),
                        ("symbol", symbol),
                        ("type", "priceVolumeDeliverable"),
                        ("series", "ALL"),
                    ],
                    ttl,
                )
                .await
                .map_err(|e| self.fail(e))?,
            Err(e) => return Err(self.fail(e)),
        };
        parse_equity_history(&json).map_err(|w| self.format_changed(w))
    }

    async fn intraday(&self, symbol: &str, target: &Target, step: i64) -> Result<ChartData> {
        let (key, index) = match target {
            Target::Equity(s) => (format!("{}EQN", s), false),
            Target::Index(name) => (name.to_string(), true),
        };
        let mut query = vec![("index", key.as_str())];
        if index {
            query.push(("indices", "true"));
        }
        let json = self
            .session
            .get_json("/api/chart-databyindex", &query, Duration::from_secs(60))
            .await
            .map_err(|e| self.fail(e))?;
        let points = parse_chart_points(&json).map_err(|w| self.format_changed(w))?;
        let bars = bars_from_points(&points, step);
        if bars.is_empty() {
            anyhow::bail!("NSE has no intraday prices for {} yet today", symbol);
        }
        Ok(super::timed_chart(symbol, &bars, Some("INR")))
    }

    /// Symbol search: NSE's autocomplete, or its equity list when the
    /// autocomplete endpoint has moved.
    pub async fn search(&self, query: &str) -> Result<Vec<SearchResult>> {
        let auto = self
            .session
            .get_json(
                "/api/search/autocomplete",
                &[("q", query)],
                Duration::from_secs(3_600),
            )
            .await;
        match auto.map(|json| parse_autocomplete(&json)) {
            Ok(Ok(results)) => return Ok(results),
            Ok(Err(_)) | Err(FetchError::NotFound(_) | FetchError::Format(_)) => {}
            Err(e) => return Err(self.fail(e)),
        }
        let csv = self
            .session
            .get_text_on_disk(
                "nse_equity_list.csv",
                &format!("{}/content/equities/EQUITY_L.csv", self.archive),
                &[],
                Duration::from_secs(24 * 3_600),
                |t| t.trim_start().to_ascii_uppercase().starts_with("SYMBOL"),
            )
            .await
            .map_err(|e| self.fail(e))?;
        Ok(search_list(&parse_equity_list(&csv), query, ".NS", "NSE"))
    }

    /// Yahoo symbols of an index's members (`NIFTY 50` → `RELIANCE.NS`, ...).
    pub async fn constituents(&self, index: &str) -> Result<Vec<String>> {
        let snap = self.index_snapshot(index).await?;
        let symbols: Vec<String> = snap.members.into_iter().filter_map(|q| q.symbol).collect();
        if symbols.is_empty() {
            anyhow::bail!("NSE listed no members for {}", index);
        }
        Ok(symbols)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::session::DiskCache;
    use crate::testutil::MockServer;

    fn fixture(name: &str) -> Value {
        let text = match name {
            "quote" => include_str!("fixtures/nse_quote_equity.json"),
            "trade" => include_str!("fixtures/nse_trade_info.json"),
            "indices" => include_str!("fixtures/nse_all_indices.json"),
            "snapshot" => include_str!("fixtures/nse_equity_stock_indices.json"),
            "intraday" => include_str!("fixtures/nse_chart_databyindex.json"),
            "history" => include_str!("fixtures/nse_historical_equity.json"),
            "index_history" => include_str!("fixtures/nse_indices_history.json"),
            "search" => include_str!("fixtures/nse_autocomplete.json"),
            _ => unreachable!(),
        };
        serde_json::from_str(text).unwrap()
    }

    fn d(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn symbols_map_to_nse_targets() {
        assert_eq!(
            target("RELIANCE.NS"),
            Some(Target::Equity("RELIANCE".into()))
        );
        assert_eq!(target("m&m.ns"), Some(Target::Equity("M&M".into())));
        assert_eq!(
            target("BAJAJ-AUTO.NS"),
            Some(Target::Equity("BAJAJ-AUTO".into()))
        );
        assert_eq!(target("^NSEI"), Some(Target::Index("NIFTY 50")));
        assert_eq!(target("^nsebank"), Some(Target::Index("NIFTY BANK")));
        assert_eq!(target("RELIANCE.BO"), None);
        assert_eq!(target("^BSESN"), None);
        assert_eq!(target("AAPL"), None);
        assert_eq!(target(".NS"), None);
        assert_eq!(index_symbol("nifty 50 "), Some("^NSEI"));
    }

    #[test]
    fn dates_parse_in_every_nse_spelling() {
        assert_eq!(parse_date("24-Oct-2025"), Some(d("2025-10-24")));
        assert_eq!(parse_date("24-OCT-2025"), Some(d("2025-10-24")));
        assert_eq!(parse_date("2025-10-24"), Some(d("2025-10-24")));
        assert_eq!(parse_date("24-10-2025"), Some(d("2025-10-24")));
        assert_eq!(parse_date("24-Oct-2025 16:00:00"), Some(d("2025-10-24")));
        assert_eq!(parse_date("someday"), None);
    }

    #[test]
    fn quote_equity_maps_prices_and_circuits() {
        let q = parse_quote(&fixture("quote")).unwrap().unwrap();
        assert_eq!(q.regular_market_price, Some(1451.6));
        assert_eq!(q.regular_market_change, Some(-0.8));
        assert_eq!(q.regular_market_change_percent, Some(-0.055));
        assert_eq!(q.regular_market_previous_close, Some(1452.4));
        assert_eq!(q.regular_market_open, Some(1455.0));
        assert_eq!(q.regular_market_day_high, Some(1460.0));
        assert_eq!(q.regular_market_day_low, Some(1443.1));
        assert_eq!(q.fifty_two_week_high, Some(1551.0));
        assert_eq!(q.fifty_two_week_low, Some(1114.85));
        // Circuit limits arrive as strings.
        assert_eq!(q.upper_circuit, Some(1597.6));
        assert_eq!(q.lower_circuit, Some(1307.2));
        assert_eq!(q.short_name.as_deref(), Some("Reliance Industries Limited"));
        assert_eq!(q.currency.as_deref(), Some("INR"));
        assert_eq!(q.data_source.as_deref(), Some("NSE"));
    }

    #[test]
    fn quote_equity_tells_unknown_symbols_from_format_changes() {
        assert!(parse_quote(&serde_json::json!({})).unwrap().is_none());
        assert!(
            parse_quote(&serde_json::json!({"msg": "no data"}))
                .unwrap()
                .is_none()
        );
        // A suspended stock: info but no price.
        let suspended = serde_json::json!({"info": {"symbol": "X"}, "priceInfo": {"lastPrice": 0}});
        assert!(parse_quote(&suspended).unwrap().is_none());
        assert!(parse_quote(&serde_json::json!({"equityQuote": {"ltp": 1}})).is_err());
        assert!(parse_quote(&serde_json::json!([1, 2])).is_err());
    }

    #[test]
    fn trade_info_adds_volume_market_cap_and_delivery() {
        let mut q = Quote::default();
        assert!(apply_trade_info(&fixture("trade"), &mut q));
        // 85.32 lakh shares; ₹19,64,352.23 crore.
        assert_eq!(q.regular_market_volume, Some(8_532_000));
        assert!((q.market_cap.unwrap() - 1.96435223e13).abs() < 1.0);
        assert_eq!(q.delivery_percent, Some(52.35));
        assert!(!apply_trade_info(
            &serde_json::json!({"other": 1}),
            &mut Quote::default()
        ));
    }

    #[test]
    fn all_indices_maps_known_indices_to_yahoo_symbols() {
        let list = parse_all_indices(&fixture("indices")).unwrap();
        let nifty = &list.iter().find(|(s, _)| *s == "^NSEI").unwrap().1;
        assert_eq!(nifty.regular_market_price, Some(25795.15));
        assert_eq!(nifty.regular_market_change, Some(-96.25));
        assert_eq!(nifty.regular_market_change_percent, Some(-0.37));
        assert_eq!(nifty.fifty_two_week_high, Some(26277.35));
        assert!(list.iter().any(|(s, _)| *s == "^NSEBANK"));
        // Indices StockWise has no Yahoo symbol for are skipped.
        assert!(list.iter().all(|(s, _)| s.starts_with('^')));
        assert!(parse_all_indices(&serde_json::json!({"rows": []})).is_err());
    }

    #[test]
    fn index_snapshot_lists_members_with_quotes() {
        let snap = parse_index_snapshot(&fixture("snapshot")).unwrap();
        assert_eq!(snap.name, "NIFTY 50");
        let index = snap.index.unwrap();
        assert_eq!(index.symbol.as_deref(), Some("^NSEI"));
        assert_eq!(index.regular_market_price, Some(25795.15));
        let symbols: Vec<&str> = snap
            .members
            .iter()
            .filter_map(|q| q.symbol.as_deref())
            .collect();
        assert_eq!(symbols, ["RELIANCE.NS", "HDFCBANK.NS", "M&M.NS"]);
        let rel = &snap.members[0];
        assert_eq!(rel.regular_market_price, Some(1451.6));
        assert_eq!(rel.regular_market_volume, Some(8_532_123));
        assert_eq!(
            rel.short_name.as_deref(),
            Some("Reliance Industries Limited")
        );
        assert!(parse_index_snapshot(&serde_json::json!({})).is_err());
    }

    #[test]
    fn intraday_points_become_ist_aligned_bars() {
        let points = parse_chart_points(&fixture("intraday")).unwrap();
        // The pre-open point is gone; stamps are real UTC (09:15 IST = 03:45Z).
        let first = chrono::DateTime::from_timestamp(points[0].0, 0).unwrap();
        assert_eq!(first.to_rfc3339(), "2025-10-24T03:45:00+00:00");
        let bars = bars_from_points(&points, 300);
        assert_eq!(bars.len(), 2);
        let b = &bars[0];
        assert_eq!(
            (b.open, b.high, b.low, b.close),
            (1455.0, 1457.5, 1453.0, 1454.2)
        );
        assert_eq!(b.volume, None);
        assert_eq!(bars[1].ts - bars[0].ts, 300);
        assert!(parse_chart_points(&serde_json::json!({"closePrice": 0})).is_err());
        let empty =
            parse_chart_points(&serde_json::json!({"grapthData": [], "closePrice": 0})).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn real_utc_chart_stamps_are_left_alone() {
        // 03:45Z is already 09:15 IST.
        let mut pts = vec![(1_761_277_500, 1.0)];
        chart_seconds(&mut pts);
        assert_eq!(pts[0].0, 1_761_277_500);
    }

    #[test]
    fn hourly_bars_start_at_the_open() {
        // 09:15, 10:14 and 10:15 IST.
        let open = 1_761_277_500;
        let pts = [(open, 10.0), (open + 3_540, 12.0), (open + 3_600, 11.0)];
        let bars = bars_from_points(&pts, 3_600);
        assert_eq!(bars.len(), 2);
        assert_eq!(bars[0].ts, open);
        assert_eq!(bars[0].close, 12.0);
        assert_eq!(bars[1].ts, open + 3_600);
    }

    #[test]
    fn equity_history_reads_daily_bars() {
        let bars = parse_equity_history(&fixture("history")).unwrap();
        assert_eq!(bars.len(), 2, "the BL series row is skipped");
        assert_eq!(bars[0].date, d("2025-10-23"));
        assert_eq!(bars[1].close, Some(1451.8));
        assert_eq!(bars[1].open, Some(1455.0));
        assert_eq!(bars[1].volume, Some(8_532_123));
        assert!(parse_equity_history(&serde_json::json!({"error": true})).is_err());
    }

    #[test]
    fn index_history_joins_turnover_volumes() {
        let bars = parse_index_history(&fixture("index_history")).unwrap();
        assert_eq!(bars.len(), 2);
        let last = bars.iter().max_by_key(|b| b.date).unwrap();
        assert_eq!(last.date, d("2025-10-24"));
        assert_eq!(last.close, Some(25795.15));
        assert_eq!(last.volume, Some(312_345_678));
        assert!(parse_index_history(&serde_json::json!({"data": {}})).is_err());
    }

    #[test]
    fn autocomplete_keeps_equities_once() {
        let results = parse_autocomplete(&fixture("search")).unwrap();
        let symbols: Vec<&str> = results.iter().filter_map(|r| r.symbol.as_deref()).collect();
        assert_eq!(symbols, ["RELIANCE.NS", "RELINFRA.NS"]);
        assert_eq!(
            results[0].short_name.as_deref(),
            Some("Reliance Industries Limited")
        );
        assert_eq!(results[0].exchange.as_deref(), Some("NSE"));
        assert!(parse_autocomplete(&serde_json::json!({"results": []})).is_err());
    }

    #[test]
    fn equity_list_search_ranks_symbols_before_names() {
        let csv = "SYMBOL,NAME OF COMPANY, SERIES, DATE OF LISTING\n\
                   RELINFRA,Reliance Infrastructure Limited,EQ,01-JAN-2000\n\
                   RELIANCE,Reliance Industries Limited,EQ,29-NOV-1995\n\
                   TCS,\"Tata Consultancy Services, Limited\",EQ,25-AUG-2004\n";
        let list = parse_equity_list(csv);
        assert_eq!(list.len(), 3);
        assert_eq!(list[2].1, "Tata Consultancy Services, Limited");
        let hits = search_list(&list, "reliance", ".NS", "NSE");
        assert_eq!(hits[0].symbol.as_deref(), Some("RELIANCE.NS"));
        assert_eq!(hits[1].symbol.as_deref(), Some("RELINFRA.NS"));
        let by_name = search_list(&list, "consultancy", ".NS", "NSE");
        assert_eq!(by_name[0].symbol.as_deref(), Some("TCS.NS"));
        assert!(search_list(&list, " ", ".NS", "NSE").is_empty());
        assert!(parse_equity_list("").is_empty());
    }

    #[test]
    fn history_windows_split_into_years() {
        let w = history_windows(d("2023-01-01"), d("2025-06-30"));
        assert_eq!(w.len(), 3);
        assert_eq!(w[0], ("01-01-2023".to_string(), "31-12-2023".to_string()));
        assert_eq!(w[2].1, "30-06-2025");
        assert_eq!(history_windows(d("2025-06-30"), d("2025-06-30")).len(), 1);
    }

    fn nse_at(server: &MockServer) -> Nse {
        let mut s = site(&server.url);
        s.home = None;
        Nse::new(Arc::new(Session::for_tests(s, DiskCache::Off)))
    }

    #[tokio::test]
    async fn quotes_one_symbol_with_trade_info() {
        let server = MockServer::start(vec![
            (200, include_str!("fixtures/nse_quote_equity.json").into()),
            (200, include_str!("fixtures/nse_trade_info.json").into()),
        ])
        .await;
        let nse = nse_at(&server);
        let quotes = nse.quotes(&["RELIANCE.NS", "AAPL"]).await.unwrap();
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].symbol.as_deref(), Some("RELIANCE.NS"));
        assert_eq!(quotes[0].delivery_percent, Some(52.35));
        let reqs = server.requests();
        assert_eq!(reqs.len(), 2);
        assert_eq!(reqs[0].target, "/api/quote-equity?symbol=RELIANCE");
        assert_eq!(
            reqs[1].target,
            "/api/quote-equity?symbol=RELIANCE&section=trade_info"
        );
    }

    #[tokio::test]
    async fn index_members_are_quoted_from_the_snapshot() {
        let server = MockServer::start(vec![(
            200,
            include_str!("fixtures/nse_equity_stock_indices.json").into(),
        )])
        .await;
        let nse = nse_at(&server);
        let members = nse.constituents("NIFTY 50").await.unwrap();
        assert_eq!(members.len(), 3);
        let refs: Vec<&str> = members.iter().map(String::as_str).collect();
        assert_eq!(nse.quote_cost(&refs), 0);
        let quotes = nse.quotes(&refs).await.unwrap();
        assert_eq!(quotes.len(), 3);
        assert_eq!(server.requests().len(), 1, "no request per member");
        assert_eq!(
            server.requests()[0].target,
            "/api/equity-stockIndices?index=NIFTY+50"
        );
    }

    /// NIFTY 500 prices all 500 members in one request, with no Yahoo call
    /// and no request per member.
    #[tokio::test]
    async fn nifty_500_is_one_bulk_request() {
        let mut data = vec![serde_json::json!({
            "symbol": "NIFTY 500", "priority": 1, "lastPrice": 22000.0, "pChange": 0.4
        })];
        for i in 0..500 {
            data.push(serde_json::json!({
                "symbol": format!("STK{i}"), "lastPrice": 100.0 + i as f64,
                "pChange": (i % 7) as f64 - 3.0, "totalTradedVolume": 1000 + i,
                "meta": {"companyName": format!("Stock {i}")}
            }));
        }
        let body = serde_json::json!({"name": "NIFTY 500", "data": data}).to_string();
        let server = MockServer::start(vec![(200, body)]).await;
        let nse = nse_at(&server);
        let snap = nse.index_snapshot("NIFTY 500").await.unwrap();
        assert_eq!(snap.members.len(), 500);
        assert!(snap.index.is_some());
        let syms: Vec<&str> = snap
            .members
            .iter()
            .filter_map(|q| q.symbol.as_deref())
            .collect();
        assert_eq!(syms[499], "STK499.NS");
        assert_eq!(nse.quote_cost(&syms), 0);
        assert_eq!(server.requests().len(), 1);
        assert_eq!(
            server.requests()[0].target,
            "/api/equity-stockIndices?index=NIFTY+500"
        );
    }

    #[tokio::test]
    async fn indices_share_one_request() {
        let server = MockServer::start(vec![(
            200,
            include_str!("fixtures/nse_all_indices.json").into(),
        )])
        .await;
        let nse = nse_at(&server);
        assert_eq!(nse.quote_cost(&["^NSEI", "^NSEBANK"]), 1);
        let quotes = nse.quotes(&["^NSEI", "^NSEBANK", "^BSESN"]).await.unwrap();
        assert_eq!(quotes.len(), 2);
        assert_eq!(quotes[1].symbol.as_deref(), Some("^NSEBANK"));
        assert_eq!(server.requests().len(), 1);
    }

    #[tokio::test]
    async fn a_refusal_is_an_error_and_stops_the_run() {
        let server = MockServer::start(vec![(403, "<HTML>Access Denied</HTML>".into())]).await;
        let nse = nse_at(&server);
        let err = nse.quotes(&["TCS.NS", "INFY.NS"]).await.unwrap_err();
        assert!(err.to_string().contains("NSE refused"), "{err}");
        assert_eq!(nse.remaining(), 0);
        assert_eq!(
            server.requests().len(),
            1,
            "no second symbol after a refusal"
        );
    }

    #[tokio::test]
    async fn a_changed_format_is_reported() {
        let server = MockServer::start(vec![(200, r#"{"equityQuote":{"ltp":1}}"#.into())]).await;
        let nse = nse_at(&server);
        let err = nse.quotes(&["TCS.NS"]).await.unwrap_err();
        assert!(
            err.to_string().contains("changed its response format"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn search_falls_back_to_the_equity_list() {
        let server = MockServer::start(vec![
            (404, "gone".into()),
            (
                200,
                "SYMBOL,NAME OF COMPANY\nTCS,Tata Consultancy Services Limited\n".into(),
            ),
        ])
        .await;
        let mut nse = nse_at(&server);
        nse.archive = server.url.clone();
        let results = nse.search("tcs").await.unwrap();
        assert_eq!(results[0].symbol.as_deref(), Some("TCS.NS"));
        let targets: Vec<String> = server.requests().into_iter().map(|r| r.target).collect();
        assert_eq!(
            targets,
            [
                "/api/search/autocomplete?q=tcs",
                "/content/equities/EQUITY_L.csv"
            ]
        );
    }

    #[tokio::test]
    async fn daily_history_is_fetched_in_year_windows() {
        let server = MockServer::start(vec![(
            200,
            include_str!("fixtures/nse_historical_equity.json").into(),
        )])
        .await;
        let nse = nse_at(&server);
        // The fixture's bars are old, so the coverage check fails — but the
        // request shape is what matters here.
        let _ = nse.chart("RELIANCE.NS", "2y", "1d").await;
        let reqs = server.requests();
        assert_eq!(reqs.len(), 3, "two years and a day = three windows");
        assert!(
            reqs[0]
                .target
                .starts_with("/api/historical/cm/equity?symbol=RELIANCE&series=")
        );
    }
}
