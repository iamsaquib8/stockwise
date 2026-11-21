//! `stockwise statusbar`: one compact line for tmux, waybar, sketchybar and
//! starship, e.g. `▲ ₹+12.3k (+0.8%) · RELIANCE +1.2% · TCS −0.4%`.
//!
//! Status bars run it every few seconds, so it answers from
//! `statusbar_cache.json` in the data directory and goes to the network at
//! most once per `--refresh` seconds, with a 3 second budget. When that fails
//! or times out, the cached line is shown marked stale. With every relevant
//! market closed it fetches once after the close and then stays offline.

use super::*;
use chrono::{DateTime, FixedOffset, Utc};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::time::Duration;

/// The cache file in the data directory.
pub const CACHE_FILE: &str = "statusbar_cache.json";
/// The whole network refresh (client, quotes, exchange rates) must fit here.
const FETCH_TIMEOUT: Duration = Duration::from_secs(3);
/// Closing prices settle a few minutes after the bell: keep refreshing (at
/// the usual rate) this long after the close, then stop until the next open.
const SETTLE_SECS: i64 = 15 * 60;
/// Cache entries not used for this long are dropped.
const PRUNE_SECS: i64 = 7 * 24 * 3600;
/// A change smaller than this (in percent) reads as flat.
const FLAT_PCT: f64 = 0.05;

/// Output dialect of `statusbar --format`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum StatusbarFormat {
    /// `#[fg=green]` colour codes for tmux's `status-right`
    Tmux,
    /// One JSON object `{"text","tooltip","class"}` for a waybar custom module
    Waybar,
    /// `label='…' label.color=0xAARRGGBB`, for `sketchybar --set "$NAME"`
    Sketchybar,
    /// Plain text without ANSI codes, for a starship `custom` module
    Starship,
    /// Structured JSON: portfolio value, day change, rows, as_of, stale
    Json,
}

/// The portfolio's value and day change in its base currency.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PortfolioLine {
    pub currency: String,
    pub value: f64,
    pub day_change: f64,
    pub day_change_pct: Option<f64>,
}

/// One symbol on the line.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Row {
    pub symbol: String,
    pub price: Option<f64>,
    pub change_pct: Option<f64>,
    pub currency: Option<String>,
}

/// What one request (portfolio, market, symbols, max) last showed.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// When the data was last fetched successfully (the line's "as of").
    pub fetched_at: Option<DateTime<Utc>>,
    /// When a refresh was last tried, successful or not. Rate-limits retries
    /// so a dead network costs one 3 s attempt per `--refresh`, not one per call.
    pub attempted_at: Option<DateTime<Utc>>,
    /// Why the last attempt failed; None after a success.
    pub error: Option<String>,
    pub portfolio: Option<PortfolioLine>,
    pub rows: Vec<Row>,
    /// Every symbol priced, to know which markets' hours matter.
    pub symbols: Vec<String>,
}

#[derive(Serialize, Deserialize, Default)]
struct CacheFile {
    entries: BTreeMap<String, Snapshot>,
}

/// The markets' state at one moment, for the refresh and stale decisions.
#[derive(Clone, Copy, Debug)]
pub struct MarketState {
    pub now: DateTime<Utc>,
    /// Any market of the shown symbols is in its regular session.
    pub open: bool,
    /// The latest regular-session close at or before `now` among them.
    pub last_close: Option<DateTime<Utc>>,
}

impl MarketState {
    pub fn of(symbols: &[String], fallback: Market, now: DateTime<Utc>) -> Self {
        let mut markets: Vec<Market> = Vec::new();
        for s in symbols {
            let m = market::market_of_symbol(s, fallback);
            if !markets.contains(&m) {
                markets.push(m);
            }
        }
        if markets.is_empty() {
            markets.push(fallback);
        }
        let open = markets.iter().any(|m| crate::clock::is_open(*m, now));
        let last_close = markets
            .iter()
            .filter_map(|m| {
                let today = crate::clock::session_date(*m, now);
                (0..10).find_map(|i| {
                    crate::clock::close_at(*m, today - chrono::Duration::days(i))
                        .filter(|c| *c <= now)
                })
            })
            .max();
        MarketState {
            now,
            open,
            last_close,
        }
    }
}

/// Should this call go to the network? Never more than once per
/// `refresh_secs` (counting failed attempts); while a market is open, once
/// the data is that old; while all are closed, only until the data covers the
/// last close (plus a settling margin), or when the portfolio or watchlist
/// file changed after the last fetch (`inputs_modified`).
pub fn needs_refresh(
    snap: Option<&Snapshot>,
    state: &MarketState,
    refresh_secs: u64,
    inputs_modified: Option<DateTime<Utc>>,
) -> bool {
    let refresh = refresh_secs as i64;
    let age = |t: DateTime<Utc>| (state.now - t).num_seconds();
    let Some(snap) = snap else {
        return true;
    };
    if snap.attempted_at.is_some_and(|t| age(t) < refresh) {
        return false;
    }
    let Some(fetched) = snap.fetched_at else {
        return true;
    };
    if inputs_modified.is_some_and(|m| m > fetched) {
        return true;
    }
    if state.open {
        return age(fetched) >= refresh;
    }
    match state.last_close {
        Some(close) => fetched < close + chrono::Duration::seconds(SETTLE_SECS),
        None => age(fetched) >= refresh,
    }
}

/// Is the shown data out of date? The last refresh failed, there is no data,
/// a market is open and the data is well past `refresh_secs`, or all markets
/// are closed and the data predates the last close.
pub fn is_stale(snap: &Snapshot, state: &MarketState, refresh_secs: u64) -> bool {
    let Some(fetched) = snap.fetched_at else {
        return true;
    };
    if snap.error.is_some() {
        return true;
    }
    if state.open {
        let limit = (refresh_secs as i64 * 3).max(180);
        (state.now - fetched).num_seconds() > limit
    } else {
        state.last_close.is_some_and(|close| fetched < close)
    }
}

/// How the portfolio part of the line moved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
    Flat,
}

impl Direction {
    fn of(pct: Option<f64>) -> Self {
        match pct {
            Some(p) if p >= FLAT_PCT => Direction::Up,
            Some(p) if p <= -FLAT_PCT => Direction::Down,
            _ => Direction::Flat,
        }
    }

    fn arrow(self) -> &'static str {
        match self {
            Direction::Up => "▲",
            Direction::Down => "▼",
            Direction::Flat => "▬",
        }
    }

    fn class(self) -> &'static str {
        match self {
            Direction::Up => "up",
            Direction::Down => "down",
            Direction::Flat => "flat",
        }
    }
}

/// Everything a renderer needs besides the snapshot.
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub stale: bool,
    pub market_open: bool,
    /// Offset the "as of" time is shown in (the machine's local zone).
    pub tz: FixedOffset,
}

/// `+` or the true minus sign `−` (U+2212), which lines up in proportional fonts.
fn sign(v: f64) -> &'static str {
    if v < 0.0 { "\u{2212}" } else { "+" }
}

fn signed_pct(p: f64) -> String {
    format!("{}{:.1}%", sign(p), p.abs())
}

/// `12.3k`, `4.6L`, `1.2Cr` (rupees) or `12.3k`, `4.6M`, `1.2B`, unsigned.
pub fn compact_amount(v: f64, currency: &str) -> String {
    let a = v.abs();
    let steps: &[(f64, &str)] = if currency == "INR" {
        &[(1e7, "Cr"), (1e5, "L"), (1e3, "k")]
    } else {
        &[(1e9, "B"), (1e6, "M"), (1e3, "k")]
    };
    for (size, unit) in steps {
        if a >= *size {
            return format!("{:.1}{}", a / size, unit);
        }
    }
    format!("{:.0}", a)
}

/// `RELIANCE.NS` → `RELIANCE`; other symbols as they are.
pub fn short_symbol(symbol: &str) -> &str {
    symbol
        .strip_suffix(".NS")
        .or_else(|| symbol.strip_suffix(".BO"))
        .unwrap_or(symbol)
}

/// A styled piece of the line: text plus the direction that colours it.
struct Piece {
    text: String,
    dir: Option<Direction>,
}

/// The line as pieces separated by ` · `; the first piece of each segment
/// is the uncoloured label, the second the coloured change.
fn segments(snap: &Snapshot) -> Vec<Vec<Piece>> {
    let mut out = Vec::new();
    if let Some(p) = &snap.portfolio {
        let dir = Direction::of(p.day_change_pct);
        let cur = market::currency_symbol(Some(&p.currency));
        let mut text = format!(
            "{}{}{}",
            cur,
            sign(p.day_change),
            compact_amount(p.day_change, &p.currency)
        );
        if let Some(pct) = p.day_change_pct {
            text.push_str(&format!(" ({})", signed_pct(pct)));
        }
        out.push(vec![
            Piece {
                text: dir.arrow().to_string(),
                dir: Some(dir),
            },
            Piece {
                text,
                dir: Some(dir),
            },
        ]);
    }
    for r in &snap.rows {
        let label = Piece {
            text: short_symbol(&r.symbol).to_string(),
            dir: None,
        };
        let change = match r.change_pct {
            Some(p) => Piece {
                text: signed_pct(p),
                dir: Some(Direction::of(Some(p))),
            },
            None => Piece {
                text: "—".into(),
                dir: None,
            },
        };
        out.push(vec![label, change]);
    }
    out
}

/// The overall class: the portfolio's direction, else the first row's.
fn direction(snap: &Snapshot) -> Direction {
    match &snap.portfolio {
        Some(p) => Direction::of(p.day_change_pct),
        None => Direction::of(snap.rows.first().and_then(|r| r.change_pct)),
    }
}

/// `as of 14:05` in the view's zone, or why there is nothing to show.
fn stale_note(snap: &Snapshot, view: &View) -> Option<String> {
    if !view.stale {
        return None;
    }
    Some(match snap.fetched_at {
        Some(t) => format!("as of {}", t.with_timezone(&view.tz).format("%H:%M")),
        None => "no data".into(),
    })
}

fn empty_text(snap: &Snapshot) -> &'static str {
    if snap.fetched_at.is_none() && snap.error.is_some() {
        "stockwise: offline"
    } else {
        "stockwise: no holdings (add some, or pass --symbols)"
    }
}

/// The plain line: no colour codes.
fn plain_line(snap: &Snapshot, view: &View) -> String {
    let segs = segments(snap);
    let mut line = if segs.is_empty() {
        empty_text(snap).to_string()
    } else {
        segs.iter()
            .map(|s| {
                s.iter()
                    .map(|p| p.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    if let Some(note) = stale_note(snap, view) {
        line.push_str(&format!(" ({note})"));
    }
    line
}

/// tmux treats `#` as the start of a format; a literal one is `##`.
fn tmux_escape(s: &str) -> String {
    s.replace('#', "##")
}

fn tmux_line(snap: &Snapshot, view: &View) -> String {
    let segs = segments(snap);
    let mut line = if segs.is_empty() {
        tmux_escape(empty_text(snap))
    } else {
        segs.iter()
            .map(|s| {
                s.iter()
                    .map(|p| {
                        let text = tmux_escape(&p.text);
                        match p.dir {
                            Some(Direction::Up) => format!("#[fg=green]{text}#[default]"),
                            Some(Direction::Down) => format!("#[fg=red]{text}#[default]"),
                            _ => text,
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    if let Some(note) = stale_note(snap, view) {
        line.push_str(&format!(" #[fg=yellow]({})#[default]", tmux_escape(&note)));
    }
    line
}

/// Waybar parses Pango markup unless the module sets `escape`.
fn pango_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Multi-line detail: value, each symbol's price, the as-of time.
fn tooltip(snap: &Snapshot, view: &View) -> String {
    let mut lines = Vec::new();
    if let Some(p) = &snap.portfolio {
        let mut day = format!(
            "{}{}",
            sign(p.day_change),
            format_price(p.day_change.abs(), Some(&p.currency))
        );
        if let Some(pct) = p.day_change_pct {
            day.push_str(&format!(" ({})", signed_pct(pct)));
        }
        lines.push(format!(
            "Portfolio {} · day {}",
            format_price(p.value, Some(&p.currency)),
            day
        ));
    }
    for r in &snap.rows {
        let price = r
            .price
            .map(|v| format_price(v, r.currency.as_deref()))
            .unwrap_or_else(|| "n/a".into());
        let change = r.change_pct.map(signed_pct).unwrap_or_else(|| "—".into());
        lines.push(format!("{} {} {}", short_symbol(&r.symbol), price, change));
    }
    let when = match snap.fetched_at {
        Some(t) => format!("As of {}", t.with_timezone(&view.tz).format("%d %b %H:%M")),
        None => "No data yet".into(),
    };
    let market = if view.market_open {
        "market open"
    } else {
        "market closed"
    };
    let mut last = format!("{when} · {market}");
    if view.stale {
        last.push_str(" · stale");
    }
    lines.push(last);
    if let Some(e) = &snap.error {
        lines.push(format!("Last refresh failed: {e}"));
    }
    lines.join("\n")
}

/// Quote a value for `sh`: single quotes, with `'` written as `'\''`.
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The `JSON` shape of `--format json` (documented in docs/commands.md).
#[derive(Serialize)]
struct JsonOut<'a> {
    text: String,
    as_of: Option<DateTime<Utc>>,
    stale: bool,
    market_open: bool,
    class: &'static str,
    error: Option<&'a str>,
    portfolio: Option<&'a PortfolioLine>,
    rows: &'a [Row],
}

/// The class waybar styles on: `up`, `down`, `flat`, or `stale`.
fn class(snap: &Snapshot, view: &View) -> &'static str {
    if view.stale {
        "stale"
    } else {
        direction(snap).class()
    }
}

/// Render `snap` in `format`. Pure: everything comes from the arguments.
pub fn render(snap: &Snapshot, format: StatusbarFormat, view: &View) -> String {
    match format {
        StatusbarFormat::Tmux => tmux_line(snap, view),
        StatusbarFormat::Starship => plain_line(snap, view),
        StatusbarFormat::Waybar => serde_json::json!({
            "text": pango_escape(&plain_line(snap, view)),
            "tooltip": pango_escape(&tooltip(snap, view)),
            "class": class(snap, view),
        })
        .to_string(),
        StatusbarFormat::Sketchybar => {
            let color = if view.stale {
                "0xffd29922"
            } else {
                match direction(snap) {
                    Direction::Up => "0xff3fb950",
                    Direction::Down => "0xfff85149",
                    Direction::Flat => "0xffc9d1d9",
                }
            };
            format!(
                "label={} label.color={}",
                sh_quote(&plain_line(snap, view)),
                color
            )
        }
        StatusbarFormat::Json => serde_json::to_string(&JsonOut {
            text: plain_line(snap, view),
            as_of: snap.fetched_at,
            stale: view.stale,
            market_open: view.market_open,
            class: class(snap, view),
            error: snap.error.as_deref(),
            portfolio: snap.portfolio.as_ref(),
            rows: &snap.rows,
        })
        .unwrap_or_else(|_| "{}".into()),
    }
}

/// A holding's day change in the base currency from its value now and its
/// percentage change: what it was worth at the previous close is
/// `value / (1 + pct/100)`. Same rule as `portfolio analyze`.
pub fn day_change(value: f64, change_pct: f64) -> f64 {
    let denom = 1.0 + change_pct / 100.0;
    if !value.is_finite() || !change_pct.is_finite() || denom <= 0.0 {
        return 0.0;
    }
    value - value / denom
}

/// The portfolio line from each priced holding's (value in base, % change).
/// Holdings without a change (fund NAVs) count in the value, not the change.
pub fn portfolio_line(currency: &str, holdings: &[(f64, Option<f64>)]) -> PortfolioLine {
    let value: f64 = holdings.iter().map(|(v, _)| v).sum();
    let change: f64 = holdings
        .iter()
        .filter_map(|(v, c)| c.map(|c| day_change(*v, c)))
        .sum();
    let prev = value - change;
    PortfolioLine {
        currency: currency.to_string(),
        value,
        day_change: change,
        day_change_pct: (prev > 0.0).then(|| change / prev * 100.0),
    }
}

/// Fetch a fresh snapshot: the given symbols, else the portfolio's day
/// change, its largest holdings and then watchlist symbols, up to `max` rows.
async fn fetch(market: Market, symbols: &[String], max: usize) -> Result<Snapshot> {
    let row = |q: Option<&crate::api::Quote>, symbol: &str| Row {
        symbol: symbol.to_string(),
        price: q.and_then(|q| q.regular_market_price),
        change_pct: q.and_then(|q| q.regular_market_change_percent),
        currency: q
            .and_then(|q| q.currency.clone())
            .or_else(|| crate::portfolio::symbol_currency(symbol).map(str::to_string)),
    };
    let find = |quotes: &[crate::api::Quote], s: &str| -> Option<crate::api::Quote> {
        quotes
            .iter()
            .find(|q| {
                q.symbol
                    .as_deref()
                    .is_some_and(|q| q.eq_ignore_ascii_case(s))
            })
            .cloned()
    };
    if !symbols.is_empty() {
        let client = YahooClient::new().await?;
        let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
        let quotes = views::quotes_with_off_market(&client, &refs).await?.0;
        let rows: Vec<Row> = symbols
            .iter()
            .take(max)
            .map(|s| row(find(&quotes, s).as_ref(), s))
            .collect();
        if rows.iter().all(|r| r.price.is_none()) {
            bail!("no quotes for {}", symbols.join(", "));
        }
        return Ok(Snapshot {
            rows,
            symbols: symbols.to_vec(),
            ..Default::default()
        });
    }

    let portfolio = Portfolio::load()?;
    let watchlist = Watchlist::load().unwrap_or_default();
    let held: Vec<String> = portfolio
        .holdings
        .iter()
        .map(|h| h.symbol.clone())
        .collect();
    let wl: Vec<String> = watchlist
        .symbols
        .iter()
        .filter(|s| !held.contains(s))
        .take(max.saturating_sub(held.len()))
        .cloned()
        .collect();
    if held.is_empty() && wl.is_empty() {
        // Nothing to price: no request at all.
        return Ok(Snapshot::default());
    }
    let client = YahooClient::new().await?;
    let wl_refs: Vec<&str> = wl.iter().map(String::as_str).collect();
    let (val, wl_quotes) = tokio::join!(
        async {
            if held.is_empty() {
                Ok(None)
            } else {
                super::portfolio::value_holdings(&client, &held, market, None)
                    .await
                    .map(Some)
            }
        },
        async {
            if wl_refs.is_empty() {
                Vec::new()
            } else {
                views::quotes_with_off_market(&client, &wl_refs)
                    .await
                    .map(|r| r.0)
                    .unwrap_or_default()
            }
        }
    );
    let val = val?;

    let mut snap = Snapshot::default();
    if let Some(val) = &val {
        // (symbol, value in base, % change) for every priced, converted holding.
        let mut priced: Vec<(&str, f64, Option<f64>)> = portfolio
            .holdings
            .iter()
            .filter_map(|h| {
                let price = val.price(&h.symbol)?;
                let value = val.to_base(&h.symbol, h.shares * price)?;
                let pct = val
                    .quote(&h.symbol)
                    .and_then(|q| q.regular_market_change_percent);
                Some((h.symbol.as_str(), value, pct))
            })
            .collect();
        if priced.is_empty() {
            bail!("no live price for any holding");
        }
        let pairs: Vec<(f64, Option<f64>)> = priced.iter().map(|(_, v, c)| (*v, *c)).collect();
        snap.portfolio = Some(portfolio_line(val.base(), &pairs));
        priced.sort_by(|a, b| b.1.total_cmp(&a.1));
        snap.rows = priced
            .iter()
            .take(max)
            .map(|(s, _, _)| row(val.quote(s), s))
            .collect();
    }
    for s in &wl {
        if snap.rows.len() >= max {
            break;
        }
        snap.rows.push(row(find(&wl_quotes, s).as_ref(), s));
    }
    snap.symbols = held.into_iter().chain(wl).collect();
    Ok(snap)
}

/// The cache key: everything that changes what the line shows.
fn cache_key(market: Market, symbols: &[String], max: usize) -> String {
    format!(
        "{}|{}|{}|{}",
        crate::portfolio::active().unwrap_or("default"),
        market.code(),
        symbols.join(","),
        max
    )
}

/// When the active portfolio's file or the watchlist last changed: a holding
/// added while markets are closed still shows up at once.
fn inputs_modified() -> Option<DateTime<Utc>> {
    [
        crate::portfolio::file_name(crate::portfolio::active()),
        "watchlist.json".to_string(),
    ]
    .iter()
    .filter_map(|f| crate::store::path(f).ok())
    .filter_map(|p| std::fs::metadata(p).and_then(|m| m.modified()).ok())
    .map(DateTime::<Utc>::from)
    .max()
}

fn load_cache(path: Option<&std::path::Path>) -> CacheFile {
    path.and_then(|p| crate::store::load_from(p).ok())
        .unwrap_or_default()
}

/// Write `snap` under `key`, re-reading the file first so another status
/// bar's entry written meanwhile is kept. Best effort: a read-only data
/// directory only costs the caching.
fn save_entry(path: Option<&std::path::Path>, key: &str, snap: &Snapshot, now: DateTime<Utc>) {
    let Some(path) = path else {
        return;
    };
    let mut cache = load_cache(Some(path));
    cache.entries.retain(|_, s| {
        s.attempted_at
            .is_some_and(|t| (now - t).num_seconds() < PRUNE_SECS)
    });
    cache.entries.insert(key.to_string(), snap.clone());
    let _ = crate::store::save_to(path, &cache, false);
}

/// `statusbar`: print one line in `format`. Never fails for want of data or
/// network: the worst case is the cached line marked stale, or a short note.
pub async fn cmd_statusbar(
    market: Market,
    format: StatusbarFormat,
    symbols: &[String],
    max: usize,
    refresh_secs: u64,
) -> Result<()> {
    let mut wanted: Vec<String> = Vec::new();
    for s in resolve_symbols(
        &symbols
            .iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>(),
        market,
    ) {
        if !wanted.contains(&s) {
            wanted.push(s);
        }
    }
    let refresh_secs = refresh_secs.max(5);
    let key = cache_key(market, &wanted, max);
    let path = crate::store::path(CACHE_FILE).ok();
    let now = Utc::now();
    let mut snap = load_cache(path.as_deref()).entries.remove(&key);

    let known = |snap: &Option<Snapshot>| match snap {
        Some(s) if !s.symbols.is_empty() => s.symbols.clone(),
        _ => wanted.clone(),
    };
    let state = MarketState::of(&known(&snap), market, now);
    // `--symbols` does not read the portfolio or watchlist.
    let modified = if wanted.is_empty() {
        inputs_modified()
    } else {
        None
    };
    if needs_refresh(snap.as_ref(), &state, refresh_secs, modified) {
        // Claim the refresh first so status bars polling meanwhile use the
        // cache instead of all fetching at once.
        let mut claimed = snap.clone().unwrap_or_default();
        claimed.attempted_at = Some(now);
        save_entry(path.as_deref(), &key, &claimed, now);
        let updated = match tokio::time::timeout(FETCH_TIMEOUT, fetch(market, &wanted, max)).await {
            Ok(Ok(mut fresh)) => {
                fresh.fetched_at = Some(Utc::now());
                fresh.attempted_at = Some(now);
                fresh
            }
            Ok(Err(e)) => Snapshot {
                error: Some(format!("{e:#}")),
                ..claimed
            },
            Err(_) => Snapshot {
                error: Some(format!("timed out after {} s", FETCH_TIMEOUT.as_secs())),
                ..claimed
            },
        };
        save_entry(path.as_deref(), &key, &updated, now);
        snap = Some(updated);
    }
    let snap = snap.unwrap_or_default();
    let state = MarketState::of(&known(&Some(snap.clone())), market, Utc::now());
    let view = View {
        stale: is_stale(&snap, &state, refresh_secs),
        market_open: state.open,
        tz: *chrono::Local::now().offset(),
    };
    println!("{}", render(&snap, format, &view));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn t(h: u32, m: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 20, h, m, 0).unwrap()
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            fetched_at: Some(t(8, 35)),
            attempted_at: Some(t(8, 35)),
            error: None,
            portfolio: Some(PortfolioLine {
                currency: "INR".into(),
                value: 1_550_000.0,
                day_change: 12_300.0,
                day_change_pct: Some(0.8),
            }),
            rows: vec![
                Row {
                    symbol: "RELIANCE.NS".into(),
                    price: Some(2950.1),
                    change_pct: Some(1.2),
                    currency: Some("INR".into()),
                },
                Row {
                    symbol: "TCS.NS".into(),
                    price: Some(4100.0),
                    change_pct: Some(-0.4),
                    currency: Some("INR".into()),
                },
            ],
            symbols: vec!["RELIANCE.NS".into(), "TCS.NS".into()],
        }
    }

    fn view(stale: bool) -> View {
        View {
            stale,
            market_open: true,
            tz: FixedOffset::east_opt(5 * 3600 + 1800).unwrap(),
        }
    }

    #[test]
    fn starship_is_plain_text() {
        let line = render(&snapshot(), StatusbarFormat::Starship, &view(false));
        assert_eq!(
            line,
            "▲ ₹+12.3k (+0.8%) · RELIANCE +1.2% · TCS \u{2212}0.4%"
        );
        assert!(!line.contains('\x1b'));
    }

    #[test]
    fn tmux_colours_the_changes() {
        let line = render(&snapshot(), StatusbarFormat::Tmux, &view(false));
        assert_eq!(
            line,
            "#[fg=green]▲#[default] #[fg=green]₹+12.3k (+0.8%)#[default] · RELIANCE \
             #[fg=green]+1.2%#[default] · TCS #[fg=red]\u{2212}0.4%#[default]"
        );
    }

    #[test]
    fn tmux_escapes_hashes() {
        let mut s = snapshot();
        s.portfolio = None;
        s.rows.truncate(1);
        s.rows[0].symbol = "A#B".into();
        let line = render(&s, StatusbarFormat::Tmux, &view(false));
        assert!(line.starts_with("A##B "), "{line}");
    }

    #[test]
    fn waybar_is_one_json_object_with_a_class() {
        let out = render(&snapshot(), StatusbarFormat::Waybar, &view(false));
        assert!(!out.contains('\n'));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["class"], "up");
        assert_eq!(
            v["text"],
            "▲ ₹+12.3k (+0.8%) · RELIANCE +1.2% · TCS \u{2212}0.4%"
        );
        let tip = v["tooltip"].as_str().unwrap();
        assert!(tip.contains("Portfolio ₹15,50,000.00"), "{tip}");
        assert!(tip.contains("RELIANCE ₹2,950.10 +1.2%"), "{tip}");
        assert!(tip.contains("As of 20 Nov 14:05 · market open"), "{tip}");
    }

    #[test]
    fn waybar_escapes_pango_markup() {
        let mut s = snapshot();
        s.rows[0].symbol = "M&M.NS".into();
        let v: serde_json::Value =
            serde_json::from_str(&render(&s, StatusbarFormat::Waybar, &view(false))).unwrap();
        assert!(v["text"].as_str().unwrap().contains("M&amp;M +1.2%"));
    }

    #[test]
    fn sketchybar_prints_quoted_args() {
        let mut s = snapshot();
        s.portfolio = None;
        s.rows.truncate(1);
        s.rows[0].symbol = "O'NEIL".into();
        s.rows[0].change_pct = Some(-2.0);
        let out = render(&s, StatusbarFormat::Sketchybar, &view(false));
        assert_eq!(
            out,
            "label='O'\\''NEIL \u{2212}2.0%' label.color=0xfff85149"
        );
    }

    #[test]
    fn json_has_the_documented_fields() {
        let out = render(&snapshot(), StatusbarFormat::Json, &view(false));
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        for key in [
            "text",
            "as_of",
            "stale",
            "market_open",
            "class",
            "error",
            "portfolio",
            "rows",
        ] {
            assert!(v.get(key).is_some(), "missing {key}");
        }
        assert_eq!(v["portfolio"]["value"], 1_550_000.0);
        assert_eq!(v["portfolio"]["day_change_pct"], 0.8);
        assert_eq!(v["rows"][1]["symbol"], "TCS.NS");
        assert_eq!(v["rows"][1]["change_pct"], -0.4);
        assert_eq!(v["stale"], false);
    }

    #[test]
    fn stale_lines_say_so_in_every_format() {
        let s = snapshot();
        let v = view(true);
        assert!(render(&s, StatusbarFormat::Starship, &v).ends_with(" (as of 14:05)"));
        assert!(
            render(&s, StatusbarFormat::Tmux, &v).ends_with("#[fg=yellow](as of 14:05)#[default]")
        );
        let w: serde_json::Value =
            serde_json::from_str(&render(&s, StatusbarFormat::Waybar, &v)).unwrap();
        assert_eq!(w["class"], "stale");
        assert!(render(&s, StatusbarFormat::Sketchybar, &v).ends_with("label.color=0xffd29922"));
        let j: serde_json::Value =
            serde_json::from_str(&render(&s, StatusbarFormat::Json, &v)).unwrap();
        assert_eq!(j["stale"], true);
    }

    #[test]
    fn empty_and_offline_lines() {
        let empty = Snapshot {
            fetched_at: Some(t(8, 0)),
            ..Default::default()
        };
        assert!(render(&empty, StatusbarFormat::Starship, &view(false)).contains("no holdings"));
        let offline = Snapshot {
            error: Some("timed out".into()),
            ..Default::default()
        };
        assert_eq!(
            render(&offline, StatusbarFormat::Starship, &view(true)),
            "stockwise: offline (no data)"
        );
    }

    #[test]
    fn flat_and_missing_changes() {
        let mut s = snapshot();
        s.portfolio.as_mut().unwrap().day_change_pct = Some(0.01);
        s.portfolio.as_mut().unwrap().day_change = -40.0;
        s.rows[1].change_pct = None;
        let line = render(&s, StatusbarFormat::Starship, &view(false));
        assert_eq!(line, "▬ ₹\u{2212}40 (+0.0%) · RELIANCE +1.2% · TCS —");
        let w: serde_json::Value =
            serde_json::from_str(&render(&s, StatusbarFormat::Waybar, &view(false))).unwrap();
        assert_eq!(w["class"], "flat");
    }

    #[test]
    fn compact_amounts() {
        assert_eq!(compact_amount(950.0, "INR"), "950");
        assert_eq!(compact_amount(-12_345.0, "INR"), "12.3k");
        assert_eq!(compact_amount(456_000.0, "INR"), "4.6L");
        assert_eq!(compact_amount(12_000_000.0, "INR"), "1.2Cr");
        assert_eq!(compact_amount(456_000.0, "USD"), "456.0k");
        assert_eq!(compact_amount(4_560_000.0, "USD"), "4.6M");
        assert_eq!(compact_amount(1.2e9, "USD"), "1.2B");
    }

    #[test]
    fn portfolio_line_sums_day_change() {
        // 101 now after +1% (was 100), 99 now after −1% (was 100), a fund without a change.
        let p = portfolio_line(
            "INR",
            &[(101.0, Some(1.0)), (99.0, Some(-1.0)), (50.0, None)],
        );
        assert!((p.value - 250.0).abs() < 1e-9);
        assert!(p.day_change.abs() < 1e-9);
        let p = portfolio_line("INR", &[(110.0, Some(10.0))]);
        assert!((p.day_change - 10.0).abs() < 1e-9);
        assert!((p.day_change_pct.unwrap() - 10.0).abs() < 1e-9);
        assert_eq!(portfolio_line("INR", &[]).day_change_pct, None);
    }

    fn state(now: DateTime<Utc>, open: bool, last_close: Option<DateTime<Utc>>) -> MarketState {
        MarketState {
            now,
            open,
            last_close,
        }
    }

    fn needs_refresh_t(snap: Option<&Snapshot>, st: &MarketState, refresh: u64) -> bool {
        needs_refresh(snap, st, refresh, None)
    }

    #[test]
    fn a_changed_portfolio_refreshes_even_when_closed() {
        let mut s = snapshot(); // fetched 08:35
        s.fetched_at = Some(t(10, 20));
        s.attempted_at = s.fetched_at;
        let st = state(t(20, 0), false, Some(t(10, 0)));
        assert!(!needs_refresh(Some(&s), &st, 60, Some(t(9, 0))));
        assert!(needs_refresh(Some(&s), &st, 60, Some(t(19, 0))));
        // Still rate-limited.
        s.attempted_at = Some(t(19, 59));
        assert!(!needs_refresh(Some(&s), &st, 120, Some(t(19, 0))));
    }

    #[test]
    fn refreshes_when_open_and_old() {
        let s = snapshot(); // fetched 08:35
        assert!(!needs_refresh_t(Some(&s), &state(t(8, 35), true, None), 60));
        assert!(needs_refresh_t(Some(&s), &state(t(8, 36), true, None), 60));
        assert!(!needs_refresh_t(
            Some(&s),
            &state(t(8, 36), true, None),
            120
        ));
        assert!(needs_refresh_t(None, &state(t(8, 36), true, None), 60));
    }

    #[test]
    fn a_failed_attempt_waits_out_the_refresh_interval() {
        let mut s = snapshot();
        s.attempted_at = Some(t(8, 40));
        s.error = Some("timed out".into());
        assert!(!needs_refresh_t(Some(&s), &state(t(8, 40), true, None), 60));
        assert!(needs_refresh_t(Some(&s), &state(t(8, 41), true, None), 60));
        // No data at all is rate-limited too.
        let none = Snapshot {
            attempted_at: Some(t(8, 40)),
            ..Default::default()
        };
        assert!(!needs_refresh_t(
            Some(&none),
            &state(t(8, 40), false, None),
            60
        ));
    }

    #[test]
    fn closed_markets_fetch_the_close_once() {
        let close = t(10, 0); // 15:30 IST
        let mut s = snapshot();
        // Fetched before the close: refresh after it.
        assert!(needs_refresh_t(
            Some(&s),
            &state(t(12, 0), false, Some(close)),
            60
        ));
        // Fetched within the settling window: keep refreshing (rate-limited).
        s.fetched_at = Some(t(10, 5));
        s.attempted_at = s.fetched_at;
        assert!(needs_refresh_t(
            Some(&s),
            &state(t(10, 7), false, Some(close)),
            60
        ));
        // Fetched after it settled: no more requests until the next open.
        s.fetched_at = Some(t(10, 20));
        s.attempted_at = s.fetched_at;
        assert!(!needs_refresh_t(
            Some(&s),
            &state(t(20, 0), false, Some(close)),
            60
        ));
    }

    #[test]
    fn stale_marking() {
        let mut s = snapshot(); // fetched 08:35
        let open_at = |now| state(now, true, None);
        assert!(!is_stale(&s, &open_at(t(8, 37)), 60));
        assert!(is_stale(&s, &open_at(t(8, 39)), 60));
        // Closed: the last close is fine however old, data before it is not.
        assert!(!is_stale(&s, &state(t(20, 0), false, Some(t(8, 0))), 60));
        assert!(is_stale(&s, &state(t(20, 0), false, Some(t(10, 0))), 60));
        s.error = Some("timed out".into());
        assert!(is_stale(&s, &open_at(t(8, 36)), 60));
        assert!(is_stale(&Snapshot::default(), &open_at(t(8, 36)), 60));
    }

    #[test]
    fn market_state_follows_the_symbols() {
        // Fri 20 Nov 2026 08:35 UTC = 14:05 IST (open), 03:35 New York (closed).
        let st = MarketState::of(&["RELIANCE.NS".into()], Market::Us, t(8, 35));
        assert!(st.open);
        let st = MarketState::of(&["AAPL".into()], Market::Us, t(8, 35));
        assert!(!st.open);
        // Thursday's 16:00 New York close.
        assert_eq!(
            st.last_close,
            Some(Utc.with_ymd_and_hms(2026, 11, 19, 21, 0, 0).unwrap())
        );
    }

    #[test]
    fn short_symbols() {
        assert_eq!(short_symbol("RELIANCE.NS"), "RELIANCE");
        assert_eq!(short_symbol("500325.BO"), "500325");
        assert_eq!(short_symbol("BRK.B"), "BRK.B");
    }

    #[test]
    fn cache_keys_differ_by_request() {
        let a = cache_key(Market::In, &[], 3);
        let b = cache_key(Market::In, &["TCS.NS".into()], 3);
        let c = cache_key(Market::In, &[], 5);
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn day_change_guards_minus_100_percent_or_worse() {
        assert_eq!(day_change(10_000.0, -100.0), 0.0);
        assert_eq!(day_change(10_000.0, -101.0), 0.0);
        let line = portfolio_line("INR", &[(10_000.0, Some(-100.0))]);
        assert!(line.day_change.is_finite());
        assert_eq!(line.day_change, 0.0);
        assert_eq!(line.day_change_pct, Some(0.0));
    }
}
