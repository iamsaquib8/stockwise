//! Background work: a market-aware ticker, a persistent task executor and
//! the end-of-day sync, all backed by `cache.db`.
//!
//! Two ways to run it:
//! - **In process** ([`spawn_ticker`], [`spawn_task_watcher`]): `tui`, `serve`
//!   and `mcp` start a light loop that keeps held and watched quotes warm and
//!   broadcasts changes on a [`Bus`], so screens update without a reload.
//! - **As a service** (`stockwise worker start|stop|status`): [`run_worker`]
//!   adds the task executor, alert checks and the post-close sync.
//!
//! Everything is best-effort and read-only toward the market: no orders, no
//! advice. A failed fetch keeps the last data and backs off.

use crate::api::{Quote, YahooClient};
use crate::clock;
use crate::db::{self, CapTier, Snapshot, Task};
use crate::market::Market;
use crate::portfolio::Portfolio;
use anyhow::{Context, Result, bail};
use chrono::Utc;
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::Duration;
use tokio::sync::broadcast;

const STATE_FILE: &str = "worker_state.json";
const PID_FILE: &str = "worker.pid";
const LOCK_FILE: &str = "worker.lock";
/// Written by `worker stop` where there is no signal to send (Windows): the
/// worker sees it and shuts down through its normal cleanup.
const STOP_FILE: &str = "worker.stop";
const LOG_FILE: &str = "worker.log";
/// Refresh period while the exchange is open, and when it is closed.
const OPEN_TICK: Duration = Duration::from_secs(30);
const CLOSED_TICK: Duration = Duration::from_secs(300);
/// Quotes per request when warming a universe.
const WARM_CHUNK: usize = 50;
/// Longest stdout kept for a `run` task.
const MAX_TASK_OUTPUT: usize = 256 * 1024;

// ── Live events ──

/// A price change seen by the ticker.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LiveQuote {
    pub symbol: String,
    pub price: f64,
    pub change_percent: Option<f64>,
}

/// A task's state, as the task watcher saw it change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TaskUpdate {
    pub id: i64,
    pub title: String,
    pub status: String,
    pub progress_pct: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LiveEvent {
    Quotes { quotes: Vec<LiveQuote> },
    Task { task: TaskUpdate },
}

/// Fan-out channel for [`LiveEvent`]s. Sending with no subscriber is fine.
pub type Bus = broadcast::Sender<LiveEvent>;

/// A new live-event channel.
pub fn bus() -> Bus {
    broadcast::channel(256).0
}

// ── Symbols and snapshots ──

/// Symbols the ticker keeps warm: holdings and watchlist, minus funds and
/// bonds Yahoo cannot price (`MF:` and similar namespaces).
pub fn tracked_symbols() -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut add = |s: &str| {
        if !s.contains(':') && !out.iter().any(|o| o == s) {
            out.push(s.to_string());
        }
    };
    if let Ok(p) = Portfolio::load() {
        p.holdings.iter().for_each(|h| add(&h.symbol));
    }
    if let Ok(w) = crate::watchlist::Watchlist::load() {
        w.symbols.iter().for_each(|s| add(s));
    }
    out
}

/// A `market_snapshots` row for a quote; `None` without a symbol or price.
pub fn snapshot_from_quote(default_market: &str, q: &Quote) -> Option<Snapshot> {
    let symbol = q.symbol.clone()?;
    let price = q.regular_market_price?;
    // The row's market comes from the symbol or the quote's currency, not
    // from the worker's market: a held `AAPL` is a US row under `-m in`.
    let currency = q.currency.as_deref().map(str::to_ascii_uppercase);
    let market = if symbol.ends_with(".NS") || symbol.ends_with(".BO") {
        "in".to_string()
    } else if let Some(exchange) = crate::market::exchange_of(&symbol) {
        // `SAP.DE` is a German row whichever market the worker was started in.
        crate::value_name(Market::Global(exchange))
    } else {
        match currency.as_deref() {
            Some("INR") => "in".to_string(),
            Some("USD") => "us".to_string(),
            // No currency to go by, or a global market's own label.
            None => default_market.to_string(),
            // Another currency: keep a global market's own label, but never
            // file it under the home market of the worker.
            _ if !matches!(default_market, "in" | "us") => default_market.to_string(),
            _ => "other".to_string(),
        }
    };
    let rvol = match (q.regular_market_volume, q.average_daily_volume_3_month) {
        (Some(v), Some(avg)) if avg > 0 => Some(v as f64 / avg as f64),
        _ => None,
    };
    Some(Snapshot {
        name: q
            .short_name
            .clone()
            .or_else(|| q.long_name.clone())
            .unwrap_or_default(),
        sector: q.sector.clone(),
        industry: q.industry.clone(),
        cap_tier: Some(
            CapTier::classify(
                &symbol,
                q.market_cap,
                q.currency.as_deref(),
                crate::market::IndiaMembership::shared().as_deref(),
            )
            .as_str()
            .to_string(),
        )
        .filter(|t| t != "unknown"),
        mcap: q.market_cap,
        price: Some(price),
        change_1d: q.regular_market_change_percent,
        rvol,
        pe: q.trailing_pe,
        roe: q.return_on_equity.map(|r| r * 100.0),
        div_yield: q.dividend_yield_fraction().map(|d| d * 100.0),
        updated_at: Utc::now().timestamp(),
        symbol,
        market,
    })
}

/// A quote rebuilt from a `market_snapshots` row, for scans served from the
/// cache. Volume and the other live fields stay empty; the currency is the
/// market's own (`in` rows are rupees, `us` rows dollars) and `None` for a
/// market whose rows may be in any currency.
pub fn quote_from_snapshot(s: &Snapshot) -> Quote {
    Quote {
        symbol: Some(s.symbol.clone()),
        short_name: (!s.name.is_empty()).then(|| s.name.clone()),
        sector: s.sector.clone(),
        industry: s.industry.clone(),
        currency: match s.market.as_str() {
            "in" => Some("INR".into()),
            "us" => Some("USD".into()),
            _ => None,
        },
        regular_market_price: s.price,
        regular_market_change_percent: s.change_1d,
        market_cap: s.mcap,
        trailing_pe: s.pe,
        return_on_equity: s.roe.map(|r| r / 100.0),
        trailing_annual_dividend_yield: s.div_yield.map(|d| d / 100.0),
        data_source: Some("cache.db".into()),
        ..Default::default()
    }
}

/// Store usable quotes as `market_snapshots` rows; returns how many.
fn store_snapshots(market: Market, quotes: &[Quote]) -> usize {
    let label = crate::value_name(market);
    let rows: Vec<Snapshot> = quotes
        .iter()
        .filter(|q| q.is_usable())
        .filter_map(|q| snapshot_from_quote(&label, q))
        .collect();
    let n = rows.len();
    db::with_db(|d| d.upsert_snapshots(&rows));
    n
}

/// Fetch quotes for `symbols` in chunks and store them as snapshots.
/// `progress` gets the percentage after each chunk and returns false to stop
/// (a cancelled task). Returns how many snapshots were stored.
pub async fn refresh_snapshots(
    client: &YahooClient,
    market: Market,
    symbols: &[String],
    mut progress: impl FnMut(f64) -> bool,
) -> Result<usize> {
    let mut stored = 0;
    let chunks = symbols.len().div_ceil(WARM_CHUNK).max(1);
    for (i, chunk) in symbols.chunks(WARM_CHUNK).enumerate() {
        let refs: Vec<&str> = chunk.iter().map(String::as_str).collect();
        let quotes = client.get_quote(&refs).await?;
        stored += store_snapshots(market, &quotes);
        if !progress((i + 1) as f64 / chunks as f64 * 100.0) {
            break;
        }
    }
    Ok(stored)
}

/// The market's built-in universe plus everything tracked.
pub fn warm_symbols(market: Market) -> Vec<String> {
    let mut all = market.universe();
    for s in tracked_symbols() {
        if !all.contains(&s) {
            all.push(s);
        }
    }
    all
}

// ── Ticker ──

/// How long to wait before the next quote refresh.
pub fn tick_interval(market: Market, now: chrono::DateTime<Utc>) -> Duration {
    if let Some(secs) = std::env::var("STOCKWISE_WORKER_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
    {
        return Duration::from_secs(secs.max(1));
    }
    if clock::is_open(market, now) {
        OPEN_TICK
    } else {
        CLOSED_TICK
    }
}

/// Price changes between two passes: only symbols whose price moved (or that
/// are new) are returned.
pub fn price_diff(last: &mut HashMap<String, f64>, quotes: &[Quote]) -> Vec<LiveQuote> {
    let mut changed = Vec::new();
    for q in quotes.iter().filter(|q| q.is_usable()) {
        let (Some(symbol), Some(price)) = (q.symbol.as_ref(), q.regular_market_price) else {
            continue;
        };
        if last.get(symbol) != Some(&price) {
            last.insert(symbol.clone(), price);
            changed.push(LiveQuote {
                symbol: symbol.clone(),
                price,
                change_percent: q.regular_market_change_percent,
            });
        }
    }
    changed
}

/// Keep held and watched quotes fresh and broadcast changes. The task ends
/// with the runtime (or when the handle is aborted).
pub fn spawn_ticker(market: Market, bus: Bus) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut client: Option<YahooClient> = None;
        let mut last: HashMap<String, f64> = HashMap::new();
        loop {
            let symbols = tracked_symbols();
            if !symbols.is_empty() {
                if client.is_none() {
                    client = YahooClient::new().await.ok();
                }
                if let Some(c) = &client {
                    let refs: Vec<&str> = symbols.iter().map(String::as_str).collect();
                    if let Ok(quotes) = c.get_quote(&refs).await {
                        store_snapshots(market, &quotes);
                        let changed = price_diff(&mut last, &quotes);
                        if !changed.is_empty() {
                            let _ = bus.send(LiveEvent::Quotes { quotes: changed });
                        }
                    }
                }
            }
            tokio::time::sleep(tick_interval(market, Utc::now())).await;
        }
    })
}

/// Broadcast task changes seen in `cache.db`, whichever process made them.
pub fn spawn_task_watcher(bus: Bus) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut seen: HashMap<i64, (String, i64)> = HashMap::new();
        // The first completed poll only records what exists; it is not news.
        let mut primed = false;
        loop {
            if let Some(tasks) = db::with_db(|d| d.list_tasks(50)) {
                for t in tasks {
                    let key = (t.status.clone(), t.progress_pct as i64);
                    if seen.get(&t.id) != Some(&key) {
                        seen.insert(t.id, key);
                        if primed {
                            let _ = bus.send(LiveEvent::Task {
                                task: TaskUpdate {
                                    id: t.id,
                                    title: t.title,
                                    status: t.status,
                                    progress_pct: t.progress_pct,
                                },
                            });
                        }
                    }
                }
                primed = true;
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    })
}

// ── Tasks ──

/// Kinds of task `stockwise tasks add` accepts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum TaskKind {
    /// Refresh the market's universe into `market_snapshots`
    Warm,
    /// The post-close sync: snapshots, fund NAVs, wealth history
    Eod,
    /// Run a stockwise command, e.g. `tasks add run -- backtest TCS`
    Run,
}

impl TaskKind {
    /// The name stored in the `tasks.kind` column.
    pub fn as_str(self) -> &'static str {
        match self {
            TaskKind::Warm => "warm",
            TaskKind::Eod => "eod",
            TaskKind::Run => "run",
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Default)]
struct Payload {
    #[serde(default)]
    market: String,
    #[serde(default)]
    args: Vec<String>,
}

/// Queue a task; returns its id.
pub fn enqueue(kind: TaskKind, market: Market, args: &[String]) -> Result<i64> {
    if kind == TaskKind::Run && args.is_empty() {
        bail!("Give the command to run, e.g. `stockwise tasks add run -- backtest TCS`");
    }
    let payload = serde_json::to_string(&Payload {
        market: crate::value_name(market),
        args: args.to_vec(),
    })?;
    let title = match kind {
        TaskKind::Warm => format!("Warm {} snapshots", market.label()),
        TaskKind::Eod => "End-of-day sync".to_string(),
        TaskKind::Run => format!("stockwise {}", args.join(" ")),
    };
    db::with_db(|d| d.enqueue_task(kind.as_str(), &title, &payload))
        .context("The task store (cache.db) is off or cannot be opened")
}

/// Read a pipe to the end, keeping at most [`MAX_TASK_OUTPUT`] bytes; the rest
/// is drained so the child never blocks on a full pipe.
async fn read_capped<R: tokio::io::AsyncRead + Unpin>(mut r: R) -> String {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    let _ = (&mut r)
        .take(MAX_TASK_OUTPUT as u64)
        .read_to_end(&mut buf)
        .await;
    let _ = tokio::io::copy(&mut r, &mut tokio::io::sink()).await;
    String::from_utf8_lossy(&buf).into_owned()
}

/// Run `stockwise <args>` and keep its output. Stops (killing the child)
/// when the task is cancelled.
async fn run_command(task_id: i64, args: &[String]) -> Result<String> {
    let exe = std::env::current_exe().context("Cannot find the stockwise executable")?;
    let mut child = tokio::process::Command::new(exe)
        .args(args)
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .context("Cannot start the command")?;
    let stdout = child.stdout.take().context("no stdout")?;
    let stderr = child.stderr.take().context("no stderr")?;
    let out_task = tokio::spawn(read_capped(stdout));
    let err_task = tokio::spawn(read_capped(stderr));
    let status = loop {
        tokio::select! {
            s = child.wait() => break s?,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                let cancelled = db::with_db(|d| d.get_task(task_id))
                    .flatten()
                    .is_some_and(|t| t.status == "cancelled");
                if cancelled {
                    let _ = child.kill().await;
                    bail!("cancelled");
                }
            }
        }
    };
    let (out, err) = (out_task.await?, err_task.await?);
    Ok(serde_json::json!({
        "exit_code": status.code(),
        "stdout": out,
        "stderr": err,
    })
    .to_string())
}

/// Execute one claimed task and return its result JSON.
pub async fn execute(task: &Task) -> Result<String> {
    let payload: Payload = serde_json::from_str(&task.payload_json).unwrap_or_default();
    let market = Market::from_str(&payload.market, true).unwrap_or(Market::In);
    let id = task.id;
    match task.kind.as_str() {
        "warm" => {
            let client = YahooClient::new().await?;
            let symbols = warm_symbols(market);
            let n = refresh_snapshots(&client, market, &symbols, |pct| {
                db::with_db(|d| d.set_task_progress(id, pct)).unwrap_or(true)
            })
            .await?;
            Ok(serde_json::json!({ "snapshots": n, "symbols": symbols.len() }).to_string())
        }
        "eod" => {
            let client = YahooClient::new().await?;
            eod_sync(&client, market, |pct| {
                db::with_db(|d| d.set_task_progress(id, pct)).unwrap_or(true)
            })
            .await
        }
        "run" => run_command(id, &payload.args).await,
        other => bail!("Unknown task kind `{other}`"),
    }
}

/// Claim and run queued tasks until none are left.
pub async fn drain_tasks(current: &std::sync::atomic::AtomicI64) {
    use std::sync::atomic::Ordering;
    while let Some(task) = db::with_db(|d| d.claim_task()).flatten() {
        current.store(task.id, Ordering::SeqCst);
        let outcome = execute(&task).await;
        current.store(0, Ordering::SeqCst);
        db::with_db(|d| match &outcome {
            Ok(r) => d.finish_task(task.id, Ok(r)),
            Err(e) => d.finish_task(task.id, Err(&format!("{e:#}"))),
        });
    }
}

// ── End-of-day sync ──

#[derive(Debug, Default, Serialize, Deserialize)]
struct WorkerState {
    /// Session date (YYYY-MM-DD) of the last post-close sync.
    #[serde(default)]
    last_eod: Option<String>,
    #[serde(default)]
    last_alert_pass: Option<i64>,
    /// LTCG notices already sent (`SYMBOL#lot:date:kind`), each sent once.
    #[serde(default)]
    ltcg_notified: Vec<String>,
    /// Newest quarter end seen per held symbol, to spot a new result.
    #[serde(default)]
    last_results: HashMap<String, String>,
    /// Where the next sync starts in the sorted list of held symbols.
    #[serde(default)]
    result_cursor: usize,
    /// Ids of exchange filings already seen per held symbol (see
    /// [`filing_id`]); a symbol absent here has not been looked at yet.
    #[serde(default)]
    filings_seen: HashMap<String, Vec<String>>,
    /// Where the next sync resumes over already-seen symbols for filing checks.
    #[serde(default)]
    filing_cursor: usize,
}

/// After the close: refresh tracked snapshots, append today's NAV for held
/// funds, and add a wealth-history snapshot. Returns a result summary.
pub async fn eod_sync(
    client: &YahooClient,
    market: Market,
    mut progress: impl FnMut(f64) -> bool,
) -> Result<String> {
    let symbols = tracked_symbols();
    let mut cancelled = false;
    let snapshots = refresh_snapshots(client, market, &symbols, |p| {
        let keep = progress(p * 0.5);
        cancelled |= !keep;
        keep
    })
    .await?;
    if cancelled {
        bail!("cancelled");
    }

    // Fund NAVs for held `MF:<code>` symbols, from the cached AMFI file.
    let portfolio = Portfolio::load()?;
    let codes: Vec<u32> = portfolio
        .holdings
        .iter()
        .filter_map(|h| h.symbol.strip_prefix("MF:")?.parse().ok())
        .collect();
    let mut navs = 0;
    if !codes.is_empty()
        && let Ok(schemes) = crate::funds::load_amfi().await
    {
        // Cancellation can arrive while AMFI loads, after the last snapshot
        // check and before any write; with only `MF:` holdings there was no
        // snapshot chunk to notice it at all.
        if !progress(55.0) {
            bail!("cancelled");
        }
        for s in schemes.iter().filter(|s| codes.contains(&s.code)) {
            if let (Some(nav), Some(date)) = (s.nav, s.date) {
                let row = vec![(date.format("%Y-%m-%d").to_string(), nav)];
                db::with_db(|d| d.put_navs(&s.code.to_string(), &row));
                navs += 1;
            }
        }
    }
    // Nothing below (wealth history) is written for a cancelled task.
    if !progress(75.0) {
        bail!("cancelled");
    }

    // Wealth history, only when every holding has a price.
    let mut wealth = false;
    if !portfolio.holdings.is_empty() {
        let all: Vec<String> = portfolio
            .holdings
            .iter()
            .map(|h| h.symbol.clone())
            .collect();
        let base = crate::commands::market_currency(market);
        if let Ok(val) = crate::commands::value_holdings(client, &all, market, Some(base)).await {
            // Valuation takes a while: check again before the wealth write.
            if !progress(95.0) {
                bail!("cancelled");
            }
            let totals = val.totals(&portfolio);
            if totals.unpriced.is_empty() && totals.unconverted.is_empty() {
                crate::wealth::WealthHistory::update(|h| {
                    h.add_snapshot(totals.value, totals.cost, portfolio.holdings.len());
                    Ok(())
                })?;
                wealth = true;
            }
        }
    }
    let notices = notify_after_close(client, market, || progress(97.0)).await?;
    progress(100.0);
    Ok(serde_json::json!({
        "snapshots": snapshots,
        "fund_navs": navs,
        "wealth_snapshot": wealth,
        "notifications": notices.total,
        "filings": notices.filings,
    })
    .to_string())
}

// ── Post-close notifications ──

/// How many days ahead a lot's turn to long-term is announced.
const LTCG_HEADS_UP_DAYS: i64 = 7;
/// Held symbols checked for new results per post-close sync.
const RESULT_CHECKS_PER_SYNC: usize = 20;
/// Sent keys kept, so the state file stays small.
const MAX_NOTIFIED: usize = 500;
/// Held `.NS` symbols checked for new exchange filings per post-close sync.
const FILING_CHECKS_PER_SYNC: usize = 20;
/// Days of announcements asked for on each check.
const FILING_LOOKBACK_DAYS: i64 = 7;
/// Filing ids remembered per symbol.
const MAX_FILINGS_SEEN: usize = 200;
/// New filings of one symbol sent one by one; the rest go in one summary.
const MAX_FILING_NOTICES: usize = 3;

/// The tax country whose holding-period rule applies to a market.
fn tax_country(market: Market) -> Option<crate::tax::Country> {
    match market {
        Market::In => Some(crate::tax::Country::India),
        Market::Us => Some(crate::tax::Country::Us),
        Market::Global(crate::market::Exchange::Uk) => Some(crate::tax::Country::Uk),
        _ => None,
    }
}

/// A notice, and the state to remember once it has been delivered. Nothing is
/// remembered before delivery, so a notice that could not be sent is offered
/// again by the next sync.
struct Pending {
    msg: crate::notify::Message,
    /// `ltcg_notified` keys to add.
    ltcg_keys: Vec<String>,
    /// `(symbol, quarter end)` to record in `last_results`.
    result: Option<(String, String)>,
    /// `(symbol, filing ids)` to record in `filings_seen`.
    filings: Option<(String, Vec<String>)>,
}

/// Forget the oldest sent keys beyond [`MAX_NOTIFIED`].
fn trim_notified(notified: &mut Vec<String>) {
    if notified.len() > MAX_NOTIFIED {
        let drop = notified.len() - MAX_NOTIFIED;
        notified.drain(..drop);
    }
}

/// Notices for lots about to turn long-term (within [`LTCG_HEADS_UP_DAYS`])
/// and for lots that have turned since a heads-up, skipping what `notified`
/// already holds (`SYMBOL#lot:bought:long_term_on:kind`). The countdown skips
/// a lot once it is long-term, so the "now" notice comes from a remembered
/// heads-up. Pure: the caller records each notice's keys after delivery.
/// Facts and dates only; what to do about them is the holder's call.
fn ltcg_notices(
    portfolio: &Portfolio,
    country: crate::tax::Country,
    today: chrono::NaiveDate,
    notified: &[String],
) -> Vec<Pending> {
    let mut out = Vec::new();
    for c in crate::sellpreview::ltcg_countdown(portfolio, country, today, LTCG_HEADS_UP_DAYS) {
        let key = format!(
            "{}#{}:{}:{}:soon",
            c.symbol, c.lot_no, c.bought, c.long_term_on
        );
        if notified.contains(&key) {
            continue;
        }
        out.push(Pending {
            msg: crate::notify::Message::new(
                format!("Lot turns long-term soon: {}", c.symbol),
                format!(
                    "Lot #{} of {} ({} units, bought {}, held {} days) turns long-term on {} (in {} days). \
                     `stockwise portfolio sell {} <QTY> --preview` shows the tax on a sale.",
                    c.lot_no, c.symbol, c.qty, c.bought, c.days_held, c.long_term_on,
                    c.days_to_long_term, c.symbol
                ),
            ),
            ltcg_keys: vec![key],
            result: None,
            filings: None,
        });
    }
    // A remembered heads-up whose date has come: the lot is long-term now.
    for key in notified.iter().filter(|k| k.ends_with(":soon")) {
        let Some((head, rest)) = key.split_once('#') else {
            continue;
        };
        let parts: Vec<&str> = rest.trim_end_matches(":soon").split(':').collect();
        let [lot_no, bought, on] = parts[..] else {
            continue;
        };
        let (Ok(lot_no), Ok(on_date)) = (
            lot_no.parse::<usize>(),
            chrono::NaiveDate::parse_from_str(on, "%Y-%m-%d"),
        ) else {
            continue;
        };
        if on_date > today {
            continue;
        }
        let done = format!("{head}#{rest}").replace(":soon", ":now");
        if notified.contains(&done) {
            continue;
        }
        // Only if that very lot is still held.
        let still_held = portfolio
            .holding(head)
            .and_then(|h| h.lots.get(lot_no - 1))
            .filter(|l| l.date == bought);
        match still_held {
            Some(l) => out.push(Pending {
                msg: crate::notify::Message::new(
                    format!("Lot is now long-term: {head}"),
                    format!(
                        "Lot #{lot_no} of {head} ({} units, bought {bought}) is long-term from {on}. \
                         `stockwise portfolio sell {head} <QTY> --preview` shows the tax on a sale.",
                        l.qty
                    ),
                ),
                ltcg_keys: vec![done],
                result: None,
                filings: None,
            }),
            // Sold before it turned: nothing to announce, nothing to retry.
            None => out.push(Pending {
                msg: crate::notify::Message::new(String::new(), String::new()),
                ltcg_keys: vec![done],
                result: None,
                filings: None,
            }),
        }
    }
    out
}

/// What to do about `collected`: the record to keep (`last_results`) and a
/// notice, if its latest quarter is newer than the recorded one. A pass that
/// returns fewer quarters (fallback source, partial payload) neither
/// announces an old quarter nor lowers the record. The first sight of a
/// symbol only records, so turning the worker on announces no old results.
fn result_check(
    seen: &HashMap<String, String>,
    collected: &crate::results::Collected,
) -> Option<Pending> {
    let latest = collected.quarterly.iter().max_by_key(|p| p.end)?;
    let end = latest.end.to_string();
    let previous = seen
        .get(&collected.symbol)
        .and_then(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok());
    let record = Some((collected.symbol.clone(), end.clone()));
    let silent = |record| Pending {
        msg: crate::notify::Message::new(String::new(), String::new()),
        ltcg_keys: Vec::new(),
        result: record,
        filings: None,
    };
    match previous {
        None => return Some(silent(record)),
        Some(p) if latest.end <= p => return None,
        Some(_) => {}
    }
    let year_ago = collected
        .quarterly
        .iter()
        .find(|p| p.end == latest.end - chrono::Months::new(12));
    let pct = |cur: Option<f64>, prev: Option<f64>| {
        crate::results::growth(cur, prev)
            .map_or("n/a".to_string(), |g| format!("{:+.1}%", g * 100.0))
    };
    let num = |v: Option<f64>| v.map_or("n/a".to_string(), |v| format!("{v:.0}"));
    let cur = collected.currency.as_deref().unwrap_or("");
    Some(Pending {
        msg: crate::notify::Message::new(
            format!("New quarterly result: {}", collected.symbol),
            format!(
                "{} reported the quarter ended {end}: revenue {cur} {} ({} YoY), net profit {cur} {} ({} YoY). \
                 `stockwise results {}` shows the full table.",
                collected.name,
                num(latest.revenue),
                pct(latest.revenue, year_ago.and_then(|p| p.revenue)),
                num(latest.pat),
                pct(latest.pat, year_ago.and_then(|p| p.pat)),
                collected.symbol
            ),
        ),
        ltcg_keys: Vec::new(),
        result: record,
        filings: None,
    })
}

/// `count` symbols starting at `cursor` (wrapping), and the next cursor, so
/// a per-sync limit still reaches every holding over successive syncs.
fn rotate(symbols: &[String], cursor: usize, count: usize) -> (Vec<String>, usize) {
    if symbols.is_empty() {
        return (Vec::new(), 0);
    }
    let start = cursor % symbols.len();
    let n = count.min(symbols.len());
    let picked = (0..n)
        .map(|i| symbols[(start + i) % symbols.len()].clone())
        .collect();
    (picked, (start + n) % symbols.len())
}

/// A stable id for a filing: its UTC date and an FNV-1a hash of the
/// lower-cased title (`DefaultHasher` may change between releases).
fn filing_id(h: &crate::research::Headline) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in h.title.trim().to_lowercase().bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    let date = chrono::DateTime::from_timestamp(h.ts, 0)
        .map(|d| d.date_naive().to_string())
        .unwrap_or_default();
    format!("{date}:{hash:016x}")
}

/// `text` cut to at most `max` characters, with an ellipsis when cut.
fn clip(text: &str, max: usize) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= max {
        return text;
    }
    let cut: String = text.chars().take(max).collect();
    format!("{}…", cut.trim_end())
}

/// What a post-close notification pass produced.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
struct NoticeCounts {
    /// All notices sent.
    total: usize,
    /// Of those, new exchange filings.
    filings: usize,
}

/// Notices for exchange filings of `symbol` not in `seen`, oldest first.
/// Routine filings (trading window, newspaper ads, ...) are ignored. The
/// first sight of a symbol yields one silent record of its filings, so
/// turning the worker on does not announce old ones; later each new filing
/// is one notice, at most [`MAX_FILING_NOTICES`] singly and the rest in one
/// summary. Each carries the ids to remember once it is delivered. Facts
/// only: date, title and category; what a filing means is the reader's call.
fn filing_pending(
    symbol: &str,
    filings: &[crate::research::Headline],
    seen: &HashMap<String, Vec<String>>,
) -> Vec<Pending> {
    let known = seen.get(symbol);
    let mut fresh: Vec<(&crate::research::Headline, String)> = Vec::new();
    for h in filings {
        if crate::events::is_routine_filing(h) {
            continue;
        }
        let id = filing_id(h);
        if known.is_some_and(|k| k.contains(&id)) || fresh.iter().any(|(_, f)| *f == id) {
            continue;
        }
        fresh.push((h, id));
    }
    fresh.sort_by_key(|(h, _)| h.ts);
    let pending = |title: String, body: String, ids: Vec<String>| Pending {
        msg: crate::notify::Message::new(title, body),
        ltcg_keys: Vec::new(),
        result: None,
        filings: Some((symbol.to_string(), ids)),
    };
    if known.is_none() {
        let ids = fresh.into_iter().map(|(_, id)| id).collect();
        return vec![pending(String::new(), String::new(), ids)];
    }
    let command = format!("`stockwise events {symbol}` lists recent events.");
    let extra = fresh.len().saturating_sub(MAX_FILING_NOTICES);
    let mut out = Vec::new();
    if extra > 0 {
        out.push(pending(
            format!("New filings: {symbol}"),
            format!("{symbol} has {extra} more new filings. {command}"),
            fresh[..extra].iter().map(|(_, id)| id.clone()).collect(),
        ));
    }
    for (h, id) in &fresh[extra..] {
        let kind = crate::events::classify(&format!(
            "{} {}",
            h.category.as_deref().unwrap_or(""),
            h.title
        ));
        out.push(pending(
            format!("New filing: {symbol}"),
            format!(
                "{symbol} filed on {}: {} [{}]. {command}",
                crate::events::format_date(h.ts),
                clip(&h.title, 200),
                kind.label()
            ),
            vec![id.clone()],
        ));
    }
    out
}

/// Remember delivered filing ids, keeping the newest [`MAX_FILINGS_SEEN`].
fn record_filings(seen: &mut HashMap<String, Vec<String>>, symbol: String, ids: Vec<String>) {
    let list = seen.entry(symbol).or_default();
    list.extend(ids);
    if list.len() > MAX_FILINGS_SEEN {
        let drop = list.len() - MAX_FILINGS_SEEN;
        list.drain(..drop);
    }
}

/// Which holdings to check for filings this sync: symbols never looked at
/// first (a new holding is recorded promptly), then the remaining slots go
/// round-robin over the seen ones from `cursor`, so a per-sync cap still
/// reaches every holding over successive syncs. `held` is sorted. Returns the
/// picks and the next cursor.
fn pick_filing_checks(
    held: &[String],
    seen: &HashMap<String, Vec<String>>,
    cursor: usize,
    cap: usize,
) -> (Vec<String>, usize) {
    let (unseen, seen_list): (Vec<String>, Vec<String>) =
        held.iter().cloned().partition(|s| !seen.contains_key(s));
    let mut picked: Vec<String> = unseen.into_iter().take(cap).collect();
    let room = cap - picked.len();
    let (rest, next) = rotate(&seen_list, cursor, room);
    picked.extend(rest);
    (picked, next)
}

/// After the close: LTCG notices for held lots and a notice for each newly
/// reported quarter of a held stock, sent through the configured channels.
/// A notice is remembered only once delivered (or when no channel is
/// configured, so nothing queues up forever); one that failed is offered
/// again next sync. Returns how many notices were produced. `keep_going` is
/// the task's cancel check, asked before each network step.
async fn notify_after_close(
    client: &YahooClient,
    market: Market,
    mut keep_going: impl FnMut() -> bool,
) -> Result<NoticeCounts> {
    let portfolio = Portfolio::load()?;
    if portfolio.holdings.is_empty() {
        return Ok(NoticeCounts::default());
    }
    let notifier = crate::notify::Notifier::from_env();
    let mut state: WorkerState = crate::store::load(STATE_FILE).unwrap_or_default();
    let mut pending: Vec<Pending> = Vec::new();

    if let Some(country) = tax_country(market) {
        let today = clock::session_date(market, Utc::now());
        pending.extend(ltcg_notices(
            &portfolio,
            country,
            today,
            &state.ltcg_notified,
        ));
    }

    let store = crate::results::default_store();
    let mut held: Vec<String> = tracked_symbols()
        .into_iter()
        .filter(|s| portfolio.holdings.iter().any(|h| &h.symbol == s))
        .collect();
    held.sort();
    let (symbols, next) = rotate(&held, state.result_cursor, RESULT_CHECKS_PER_SYNC);
    state.result_cursor = next;
    for symbol in symbols {
        if !keep_going() {
            bail!("cancelled");
        }
        if let Ok(c) = crate::results::collect(client, &symbol, market, store, false, false).await
            && let Some(p) = result_check(&state.last_results, &c)
        {
            pending.push(p);
        }
    }

    // Exchange filings: symbols never looked at come first, so a new holding
    // is recorded promptly even when more than the per-sync cap are held.
    let mut held: Vec<String> = portfolio
        .holdings
        .iter()
        .filter(|h| h.symbol.ends_with(".NS"))
        .map(|h| h.symbol.clone())
        .collect();
    held.sort();
    held.dedup();
    state.filings_seen.retain(|s, _| held.contains(s));
    let (to_check, next) = pick_filing_checks(
        &held,
        &state.filings_seen,
        state.filing_cursor,
        FILING_CHECKS_PER_SYNC,
    );
    state.filing_cursor = next;
    for symbol in to_check {
        if !keep_going() {
            bail!("cancelled");
        }
        let base = symbol.trim_end_matches(".NS");
        // A failed fetch records nothing, so the next sync tries again.
        let fetched = tokio::time::timeout(
            Duration::from_secs(10),
            crate::research::fetch_nse_filings(client, base, FILING_LOOKBACK_DAYS),
        )
        .await;
        if let Ok(Ok(list)) = fetched {
            let found = filing_pending(&symbol, &list, &state.filings_seen);
            pending.extend(found);
        }
    }

    if !keep_going() {
        bail!("cancelled");
    }
    let mut sent = 0;
    let mut filings = 0;
    for p in pending {
        // Silent records (first sight, a lot sold before it turned) carry no
        // message: remember them straight away.
        let silent = p.msg.title.is_empty();
        let delivered = silent || notifier.send_logged(&p.msg).await;
        if !silent {
            sent += 1;
            filings += usize::from(p.filings.is_some());
        }
        if delivered {
            state.ltcg_notified.extend(p.ltcg_keys);
            if let Some((symbol, end)) = p.result {
                state.last_results.insert(symbol, end);
            }
            if let Some((symbol, ids)) = p.filings {
                record_filings(&mut state.filings_seen, symbol, ids);
            }
        }
    }
    trim_notified(&mut state.ltcg_notified);
    // Merge into the file's current state: the worker loop saves its own
    // fields too, and neither write should erase the other.
    let mut latest: WorkerState = crate::store::load(STATE_FILE).unwrap_or_default();
    latest.ltcg_notified = state.ltcg_notified;
    latest.last_results = state.last_results;
    latest.result_cursor = state.result_cursor;
    latest.filings_seen = state.filings_seen;
    latest.filing_cursor = state.filing_cursor;
    crate::store::save(STATE_FILE, &latest, false)?;
    Ok(NoticeCounts {
        total: sent,
        filings,
    })
}

/// The session date whose post-close sync is due, if any: the market closed
/// today and the sync has not run for this date yet.
fn eod_due(market: Market, now: chrono::DateTime<Utc>, state: &WorkerState) -> Option<String> {
    let date = clock::session_date(market, now);
    let close = clock::close_at(market, date)?;
    let key = date.to_string();
    (now >= close && state.last_eod.as_deref() != Some(&key)).then_some(key)
}

// ── Service: ownership lock, pid file, run loop, status ──

/// Path of the worker pid file.
fn pid_path() -> Result<std::path::PathBuf> {
    crate::store::path(PID_FILE)
}

/// The pid the worker wrote, if the file is readable.
fn read_pid() -> Option<u32> {
    std::fs::read_to_string(pid_path().ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Try to take the worker's ownership lock: an exclusive OS lock on
/// `worker.lock` that lives as long as the returned file. `None` means a
/// worker holds it. The OS drops the lock when the holder dies, so a crash
/// never leaves a stale owner, and start, stop and the worker itself all
/// coordinate on this one lock.
fn try_own() -> Result<Option<std::fs::File>> {
    let path = crate::store::path(LOCK_FILE)?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("Cannot open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => {
            Err(e).with_context(|| format!("Cannot lock {}", path.display()))
        }
    }
}

/// The running worker's pid, if one holds the lock and has written its pid.
pub fn running_pid() -> Option<u32> {
    match try_own() {
        Ok(None) => read_pid(),
        _ => None,
    }
}

/// Start the worker detached and wait until it owns the lock and has
/// written its pid. Returns that pid, or the reason it did not start.
pub fn start(market: Market) -> Result<u32> {
    if try_own()?.is_none() {
        match read_pid() {
            Some(pid) => bail!("The worker is already running (pid {pid})"),
            None => bail!("A worker is starting up; check `stockwise worker status`"),
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let exe = std::env::current_exe().context("Cannot find the stockwise executable")?;
        let log_path = crate::store::path(LOG_FILE)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Cannot open {}", log_path.display()))?;
        let mut child = std::process::Command::new(exe)
            .args(["-m", &crate::value_name(market), "worker", "run"])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()
            .context("Cannot start the worker")?;
        // The child acknowledges by taking the lock and writing its pid; a
        // child that exits first (lost the lock, unwritable data dir) is a
        // failed start, not a success.
        wait_ready(&mut child, &log_path)
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP: no console, and the
        // terminal's Ctrl+C does not reach it.
        const DETACHED: u32 = 0x0000_0008 | 0x0000_0200;
        let exe = std::env::current_exe().context("Cannot find the stockwise executable")?;
        let log_path = crate::store::path(LOG_FILE)?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .with_context(|| format!("Cannot open {}", log_path.display()))?;
        let mut child = std::process::Command::new(exe)
            .args(["-m", &crate::value_name(market), "worker", "run"])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .creation_flags(DETACHED)
            .spawn()
            .context("Cannot start the worker")?;
        wait_ready(&mut child, &log_path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = market;
        bail!("`worker start` is not supported here; run `stockwise worker run` instead")
    }
}

/// Wait for a freshly spawned worker to own the lock and write its pid, or
/// report that it exited first / never became ready.
fn wait_ready(child: &mut std::process::Child, log_path: &std::path::Path) -> Result<u32> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = child.try_wait()? {
            bail!(
                "The worker exited while starting ({status}); see {}",
                log_path.display()
            );
        }
        if running_pid() == Some(child.id()) {
            return Ok(child.id());
        }
        if std::time::Instant::now() >= deadline {
            // A failed start must not leave a worker that comes up later:
            // end the child and reap it before reporting.
            let _ = child.kill();
            let _ = child.wait();
            bail!(
                "The worker did not report ready within 10 s and was stopped; see {}",
                log_path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Stop the worker and wait until it has released the lock; false when none
/// was running. The worker removes its own pid file while it still holds
/// the lock, so `stop` never deletes a pid file a newer worker wrote.
pub async fn stop() -> Result<bool> {
    let Some(guard) = try_own()? else {
        // A worker holds the lock. It may not have written its pid yet.
        let mut pid = None;
        for _ in 0..40 {
            pid = read_pid();
            if pid.is_some() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let pid = pid.context("The worker has not reported its pid; try again")?;
        #[cfg(unix)]
        {
            let _ = std::process::Command::new("kill")
                .arg(pid.to_string())
                .status();
        }
        #[cfg(windows)]
        {
            // No console to signal: ask through the stop file, so the worker
            // releases its task and cleans up like on SIGTERM. Only if it
            // does not react within 10 s is the whole process tree ended.
            let _ = std::fs::write(crate::store::path(STOP_FILE)?, b"stop");
            let mut graceful = false;
            for _ in 0..100 {
                if let Some(free) = try_own()? {
                    drop(free);
                    graceful = true;
                    break;
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            if !graceful {
                let _ = std::process::Command::new("taskkill")
                    .args(["/PID", &pid.to_string(), "/T", "/F"])
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        }
        for _ in 0..50 {
            if let Some(free) = try_own()? {
                // Killed workers cannot clean up after themselves: remove
                // the pid file while we hold the lock.
                let _ = std::fs::remove_file(pid_path()?);
                let _ = std::fs::remove_file(crate::store::path(STOP_FILE)?);
                // A force-killed worker could not release its claimed task:
                // requeue (warm/eod) or fail (run) whatever it left running.
                // A failure is reported, not hidden behind "stopped".
                let recovered = reconcile_tasks(db::l2());
                drop(free);
                recovered?;
                return Ok(true);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        bail!("The worker (pid {pid}) did not stop; stop it manually");
    };
    // Nobody owns it: a pid file left by a crash is removed under the lock,
    // so a worker starting right now cannot have its file deleted.
    let _ = std::fs::remove_file(pid_path()?);
    drop(guard);
    Ok(false)
}

/// Requeue or fail the tasks a force-killed worker left `running` (`warm` and
/// `eod` go back in the queue; a `run` command is failed, never repeated).
/// Returns how many were requeued. An error means a task may still say
/// `running`; a disabled `cache.db` has no tasks to recover.
fn reconcile_tasks(l2: Option<db::L2>) -> Result<u64> {
    let Some(l2) = l2 else { return Ok(0) };
    let db = l2
        .lock()
        .map_err(|_| anyhow::anyhow!("cache.db is locked by a panicked thread"))?;
    db.requeue_stale_tasks(0)
        .context("The worker stopped, but a task it was running could not be recovered")
}

/// What `worker status` prints.
pub struct Status {
    pub pid: Option<u32>,
    pub last_eod: Option<String>,
    pub queued: usize,
    pub running: usize,
}

/// Worker state for `worker status`.
pub fn status() -> Status {
    let state: WorkerState = crate::store::load(STATE_FILE).unwrap_or_default();
    let tasks = db::with_db(|d| d.list_tasks(500)).unwrap_or_default();
    Status {
        pid: running_pid(),
        last_eod: state.last_eod,
        queued: tasks.iter().filter(|t| t.status == "queued").count(),
        running: tasks.iter().filter(|t| t.status == "running").count(),
    }
}

/// Load the alert watcher; a read or parse error is reported once per
/// distinct message and retried by the caller, never silently dropped.
fn load_alerts(last_error: &mut Option<String>) -> Option<crate::commands::AlertWatcher> {
    match crate::commands::AlertWatcher::load() {
        Ok(w) => {
            *last_error = None;
            Some(w)
        }
        Err(e) => {
            let msg = format!("{e:#}");
            if last_error.as_deref() != Some(&msg) {
                eprintln!("alerts disabled until alert_watch.json loads: {msg}");
                *last_error = Some(msg);
            }
            None
        }
    }
}

/// Resolves once `worker stop` has left the stop file (polled twice a second).
async fn stop_requested() {
    let Ok(path) = crate::store::path(STOP_FILE) else {
        return std::future::pending().await;
    };
    while !path.exists() {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// The foreground worker: ticker, task executor, alert checks, post-close
/// sync. Runs until Ctrl+C or SIGTERM.
pub async fn run_worker(market: Market) -> Result<()> {
    // Exclusive ownership first, held through shutdown and pid-file cleanup.
    let Some(_owner) = try_own()? else {
        bail!("Another worker is already running");
    };
    // A stop request left from before this start must not stop the new worker.
    let _ = std::fs::remove_file(crate::store::path(STOP_FILE)?);
    std::fs::write(pid_path()?, std::process::id().to_string())
        .context("Cannot write the worker pid file")?;
    let mut shutdown = crate::daemon::Shutdown::install("State is saved after every step.");
    // We own the lock, so no executor can still own a `running` task: recover
    // them all, whatever their age (warm/eod requeue, run fails).
    db::with_db(|d| d.requeue_stale_tasks(0));

    let bus = bus();
    let ticker = spawn_ticker(market, bus.clone());
    // The task the executor is running, so shutdown can release it.
    let current = std::sync::Arc::new(std::sync::atomic::AtomicI64::new(0));
    let in_flight = current.clone();
    let executor = tokio::spawn(async move {
        loop {
            drain_tasks(&in_flight).await;
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    });

    let mut alerts: Option<crate::commands::AlertWatcher> = None;
    let mut alerts_error: Option<String> = None;
    let mut alert_pass_error: Option<String> = None;
    let mut state: WorkerState = crate::store::load(STATE_FILE).unwrap_or_default();
    println!(
        "worker started (pid {}), market {}",
        std::process::id(),
        market.label()
    );
    loop {
        let now = Utc::now();
        // `check_alerts` skips each alert whose own market is closed, so this
        // is only rate-limited: a worker started with `-m in` still serves
        // US alerts during the US session.
        let due = state
            .last_alert_pass
            .is_none_or(|t| now.timestamp() - t >= OPEN_TICK.as_secs() as i64);
        if due {
            if alerts.is_none() {
                alerts = load_alerts(&mut alerts_error);
            }
            if let Some(w) = alerts.as_mut() {
                match w.pass(market).await {
                    Ok(_) => alert_pass_error = None,
                    Err(e) => {
                        let msg = format!("{e:#}");
                        if alert_pass_error.as_deref() != Some(&msg) {
                            eprintln!("alert check failed: {msg}");
                            alert_pass_error = Some(msg);
                        }
                    }
                }
            }
            state.last_alert_pass = Some(now.timestamp());
        }
        if let Some(date) = eod_due(market, now, &state) {
            let has_work = Portfolio::load().is_ok_and(|p| !p.holdings.is_empty())
                || !tracked_symbols().is_empty();
            if has_work && let Ok(client) = YahooClient::new().await {
                let _ = eod_sync(&client, market, |_| true).await;
            }
            // The sync saved its notification state: reload it, then record
            // the date, so neither write erases the other.
            let alert_pass = state.last_alert_pass;
            state = crate::store::load(STATE_FILE).unwrap_or_default();
            state.last_alert_pass = alert_pass;
            state.last_eod = Some(date);
            let _ = crate::store::save(STATE_FILE, &state, false);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = shutdown.wait() => break,
            _ = stop_requested() => break,
        }
    }
    ticker.abort();
    executor.abort();
    // Wait for the cancellation to finish: only then is `current` final and
    // the child of a `run` task killed (kill_on_drop).
    let _ = executor.await;
    // Do not leave the interrupted task `running` forever: idempotent kinds
    // go back in the queue; a `run` command may have changed state, so it is
    // marked failed instead of being run twice.
    let id = current.load(std::sync::atomic::Ordering::SeqCst);
    if id > 0 {
        db::with_db(|d| d.release_task(id));
    }
    // Still holding the lock: no newer worker can have written a pid file.
    let _ = std::fs::remove_file(pid_path()?);
    let _ = std::fs::remove_file(crate::store::path(STOP_FILE)?);
    println!("worker stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quote(symbol: &str, price: f64) -> Quote {
        Quote {
            symbol: Some(symbol.into()),
            regular_market_price: Some(price),
            regular_market_change_percent: Some(1.5),
            regular_market_volume: Some(2_000),
            average_daily_volume_3_month: Some(1_000),
            market_cap: Some(6.0e11),
            trailing_pe: Some(22.0),
            return_on_equity: Some(0.18),
            sector: Some("Technology".into()),
            short_name: Some("Tata Consultancy".into()),
            ..Default::default()
        }
    }

    #[test]
    fn snapshot_maps_units_and_buckets() {
        let s = snapshot_from_quote("us", &quote("TCS.NS", 3500.0)).unwrap();
        assert_eq!(s.market, "in");
        // ₹60,000 Cr: above the ₹33,000 Cr mid-cap cut-off, below ₹1,00,000 Cr.
        assert_eq!(s.cap_tier.as_deref(), Some("mid"));
        assert_eq!(s.rvol, Some(2.0));
        assert!((s.roe.unwrap() - 18.0).abs() < 1e-9);
        assert_eq!(s.name, "Tata Consultancy");
        let us = snapshot_from_quote("us", &quote("AAPL", 200.0)).unwrap();
        assert_eq!(us.market, "us");
        let mut none = quote("X", 1.0);
        none.regular_market_price = None;
        assert!(snapshot_from_quote("us", &none).is_none());
    }

    #[test]
    fn snapshot_market_and_tier_follow_the_quote_not_the_worker() {
        let with = |symbol: &str, currency: &str| {
            let mut q = quote(symbol, 10.0);
            q.currency = Some(currency.into());
            q
        };
        // A held US stock under an Indian worker is a US row with a US tier.
        let aapl = snapshot_from_quote("in", &with("AAPL", "USD")).unwrap();
        assert_eq!(
            (aapl.market.as_str(), aapl.cap_tier.as_deref()),
            ("us", Some("mega"))
        );
        // A global market keeps its label; the tier converts the JPY cap
        // (¥600 billion is about $4B).
        let toyota = snapshot_from_quote("jp", &with("7203.T", "JPY")).unwrap();
        assert_eq!(
            (toyota.market.as_str(), toyota.cap_tier.as_deref()),
            ("jp", Some("mid"))
        );
        // An unknown currency under the home market is not filed as `in`.
        let sap = snapshot_from_quote("in", &with("SAP.F", "EUR")).unwrap();
        assert_eq!(
            (sap.market.as_str(), sap.cap_tier.as_deref()),
            ("other", Some("mega"))
        );
        // A German holding is a German row even under a Japanese worker.
        let sap_jp = snapshot_from_quote("jp", &with("SAP.DE", "EUR")).unwrap();
        assert_eq!(sap_jp.market, "de");
        // London pence convert as pence: 600 billion pence is £6B, a mid cap.
        let lse = snapshot_from_quote("in", &with("VOD.L", "GBp")).unwrap();
        assert_eq!(lse.cap_tier.as_deref(), Some("mid"));
        // A `.NS` symbol is an Indian stock whatever the currency says.
        let odd = snapshot_from_quote("us", &with("X.NS", "USD")).unwrap();
        assert_eq!(
            (odd.market.as_str(), odd.cap_tier.as_deref()),
            ("in", Some("mid"))
        );
    }

    #[test]
    fn snapshot_round_trips_into_a_quote() {
        let row = snapshot_from_quote("in", &quote("TCS.NS", 3000.0)).unwrap();
        let q = quote_from_snapshot(&row);
        assert_eq!(q.symbol.as_deref(), Some("TCS.NS"));
        assert_eq!(q.currency.as_deref(), Some("INR"));
        assert_eq!(q.regular_market_price, Some(3000.0));
        assert_eq!(q.market_cap, Some(6.0e11));
        assert_eq!(q.trailing_pe, Some(22.0));
        assert!((q.return_on_equity.unwrap() - 0.18).abs() < 1e-9);
        // A global row has no known currency.
        let mut other = row;
        other.market = "uk".into();
        assert_eq!(quote_from_snapshot(&other).currency, None);
    }

    #[test]
    fn price_diff_reports_only_changes() {
        let mut last = HashMap::new();
        let first = price_diff(&mut last, &[quote("A", 1.0), quote("B", 2.0)]);
        assert_eq!(first.len(), 2);
        let second = price_diff(&mut last, &[quote("A", 1.0), quote("B", 2.5)]);
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].symbol, "B");
        assert!(price_diff(&mut last, &[quote("A", 1.0), quote("B", 2.5)]).is_empty());
    }

    #[test]
    fn events_serialize_with_a_type_tag() {
        let e = LiveEvent::Task {
            task: TaskUpdate {
                id: 3,
                title: "t".into(),
                status: "running".into(),
                progress_pct: 40.0,
            },
        };
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], "task");
        assert_eq!(v["task"]["progress_pct"], 40.0);
    }

    #[test]
    fn eod_runs_once_per_session_after_the_close() {
        use chrono::TimeZone;
        // A Wednesday: 2026-03-04, NSE closes 15:30 IST = 10:00 UTC.
        let before = Utc.with_ymd_and_hms(2026, 3, 4, 9, 0, 0).unwrap();
        let after = Utc.with_ymd_and_hms(2026, 3, 4, 10, 30, 0).unwrap();
        let mut state = WorkerState::default();
        assert_eq!(eod_due(Market::In, before, &state), None);
        let due = eod_due(Market::In, after, &state);
        assert_eq!(due.as_deref(), Some("2026-03-04"));
        state.last_eod = due;
        assert_eq!(eod_due(Market::In, after, &state), None);
    }

    #[test]
    fn tick_is_faster_while_open() {
        use chrono::TimeZone;
        let open = Utc.with_ymd_and_hms(2026, 3, 4, 6, 0, 0).unwrap();
        let closed = Utc.with_ymd_and_hms(2026, 3, 4, 14, 0, 0).unwrap();
        assert_eq!(tick_interval(Market::In, open), OPEN_TICK);
        assert_eq!(tick_interval(Market::In, closed), CLOSED_TICK);
    }

    #[test]
    fn task_recovery_reports_failures() {
        let healthy: db::L2 = Box::leak(Box::new(std::sync::Mutex::new(
            db::CacheDb::open_in_memory().unwrap(),
        )));
        {
            let d = healthy.lock().unwrap();
            let warm = d.enqueue_task("warm", "w", "{}").unwrap();
            let run = d.enqueue_task("run", "r", "{}").unwrap();
            d.claim_task().unwrap();
            d.claim_task().unwrap();
            assert_eq!(warm + 1, run);
        }
        // Idempotent work is requeued, a command is failed; the count is reported.
        assert_eq!(reconcile_tasks(Some(healthy)).unwrap(), 1);
        // A disabled store has nothing to recover.
        assert_eq!(reconcile_tasks(None).unwrap(), 0);
        // A broken store is an error, never a silent success.
        let broken: db::L2 = Box::leak(Box::new(std::sync::Mutex::new(
            db::CacheDb::open_in_memory().unwrap(),
        )));
        broken.lock().unwrap().drop_tasks_table_for_test();
        let err = reconcile_tasks(Some(broken)).unwrap_err();
        assert!(
            format!("{err:#}").contains("could not be recovered"),
            "{err:#}"
        );
    }

    fn held(symbol: &str, bought: &str) -> Portfolio {
        let mut p = Portfolio::default();
        p.add_lot(
            symbol,
            crate::portfolio::Lot::new(10.0, 100.0, bought, "manual"),
        );
        p
    }

    /// One sync that delivers everything: the real titled notices, with their
    /// keys recorded as the worker does after delivery.
    fn deliver(pending: Vec<Pending>, seen: &mut Vec<String>) -> Vec<crate::notify::Message> {
        let mut out = Vec::new();
        for p in pending {
            seen.extend(p.ltcg_keys);
            if !p.msg.title.is_empty() {
                out.push(p.msg);
            }
        }
        out
    }

    fn day(s: &str) -> chrono::NaiveDate {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn ltcg_notices_fire_once_per_lot_and_stage() {
        let india = crate::tax::Country::India;
        // Bought 2025-12-20: the first long-term day is 2026-12-21.
        let p = held("TCS.NS", "2025-12-20");
        let mut seen = Vec::new();
        // 20 days out: nothing yet.
        assert!(deliver(ltcg_notices(&p, india, day("2026-12-01"), &seen), &mut seen).is_empty());
        // Inside the week: one heads-up, then silence on later passes.
        let soon = deliver(ltcg_notices(&p, india, day("2026-12-15"), &seen), &mut seen);
        assert_eq!(soon.len(), 1);
        assert!(soon[0].title.contains("soon") && soon[0].body.contains("2026-12-21"));
        assert!(deliver(ltcg_notices(&p, india, day("2026-12-16"), &seen), &mut seen).is_empty());
        // Once it has turned: one "now long-term" notice, then silence.
        let now = deliver(ltcg_notices(&p, india, day("2026-12-21"), &seen), &mut seen);
        assert_eq!(now.len(), 1, "{now:?}");
        assert!(now[0].title.contains("now long-term"), "{}", now[0].title);
        assert!(deliver(ltcg_notices(&p, india, day("2026-12-22"), &seen), &mut seen).is_empty());
        // Notices state facts and name a command, not a recommendation (the
        // command's own name, `portfolio sell`, is not advice wording).
        for m in [&soon[0], &now[0]] {
            crate::verdict::testing::assert_neutral(&m.body.replace("portfolio sell", "portfolio"));
        }
    }

    #[test]
    fn an_undelivered_ltcg_notice_is_offered_again() {
        let india = crate::tax::Country::India;
        let p = held("TCS.NS", "2025-12-20");
        let seen: Vec<String> = Vec::new();
        // Delivery failed: nothing was recorded, so the next sync repeats it.
        let first = ltcg_notices(&p, india, day("2026-12-15"), &seen);
        let again = ltcg_notices(&p, india, day("2026-12-16"), &seen);
        assert_eq!(first.len(), 1);
        assert_eq!(again.len(), 1);
        assert_eq!(first[0].ltcg_keys, again[0].ltcg_keys);
    }

    #[test]
    fn a_sold_lot_gets_no_now_notice() {
        let india = crate::tax::Country::India;
        let mut seen = Vec::new();
        let p = held("TCS.NS", "2025-12-20");
        assert_eq!(
            deliver(ltcg_notices(&p, india, day("2026-12-15"), &seen), &mut seen).len(),
            1
        );
        // The lot is gone by the time it would have turned: silent, and
        // recorded so it is not looked at again.
        let empty = Portfolio::default();
        assert!(
            deliver(
                ltcg_notices(&empty, india, day("2026-12-22"), &seen),
                &mut seen
            )
            .is_empty()
        );
        assert!(seen.iter().any(|k| k.ends_with(":now")));
    }

    #[test]
    fn ltcg_notices_skip_funds_and_uk() {
        let today = day("2026-12-18");
        let fund = held("MF:120503", "2025-12-20");
        assert!(ltcg_notices(&fund, crate::tax::Country::India, today, &[]).is_empty());
        let uk = held("VOD.L", "2025-12-20");
        assert!(ltcg_notices(&uk, crate::tax::Country::Uk, today, &[]).is_empty());
    }

    fn collected(symbol: &str, ends: &[(&str, f64, f64)]) -> crate::results::Collected {
        let quarterly = ends
            .iter()
            .map(|(e, rev, pat)| {
                // Some fields are private to `results`: start from the default.
                let mut p = crate::results::ResultPeriod::default();
                p.end = day(e);
                p.revenue = Some(*rev);
                p.pat = Some(*pat);
                p
            })
            .collect();
        crate::results::Collected {
            symbol: symbol.into(),
            name: "Tata Consultancy".into(),
            currency: Some("INR".into()),
            is_india: true,
            is_financial: false,
            quarterly,
            annual: Vec::new(),
            sources: Vec::new(),
            warnings: Vec::new(),
            from_cache: false,
        }
    }

    /// Run a check and apply it the way a delivered sync does.
    fn check_and_commit(
        seen: &mut HashMap<String, String>,
        c: &crate::results::Collected,
    ) -> Option<Pending> {
        let p = result_check(seen, c)?;
        if let Some((s, e)) = p.result.clone() {
            seen.insert(s, e);
        }
        Some(p)
    }

    #[test]
    fn a_new_quarter_is_announced_once_and_the_first_sight_is_silent() {
        let mut seen = HashMap::new();
        let old = collected("TCS.NS", &[("2025-09-30", 1000.0, 100.0)]);
        // First sight: recorded, not announced.
        let first = check_and_commit(&mut seen, &old).expect("recorded");
        assert!(first.msg.title.is_empty());
        assert!(result_check(&seen, &old).is_none());
        let new = collected(
            "TCS.NS",
            &[("2025-09-30", 1000.0, 100.0), ("2026-09-30", 1100.0, 120.0)],
        );
        let msg = check_and_commit(&mut seen, &new)
            .expect("a new quarter is announced")
            .msg;
        assert!(msg.title.contains("TCS.NS"));
        assert!(msg.body.contains("2026-09-30"));
        assert!(
            msg.body.contains("+10.0% YoY") && msg.body.contains("+20.0% YoY"),
            "{}",
            msg.body
        );
        assert!(result_check(&seen, &new).is_none(), "announced once");
        // A later pass with fewer quarters (partial payload) is silent and
        // does not lower the record, so the next full pass is silent too.
        assert!(result_check(&seen, &old).is_none());
        assert_eq!(seen["TCS.NS"], "2026-09-30");
        assert!(result_check(&seen, &new).is_none());
    }

    #[test]
    fn an_undelivered_quarter_is_offered_again() {
        let mut seen = HashMap::new();
        let old = collected("TCS.NS", &[("2025-09-30", 1000.0, 100.0)]);
        check_and_commit(&mut seen, &old);
        let new = collected(
            "TCS.NS",
            &[("2025-09-30", 1000.0, 100.0), ("2026-09-30", 1100.0, 120.0)],
        );
        // Delivery failed: the record is not moved, so it is announced again.
        assert!(result_check(&seen, &new).is_some());
        assert!(result_check(&seen, &new).is_some());
        assert_eq!(seen["TCS.NS"], "2025-09-30");
    }

    #[test]
    fn result_checks_rotate_through_every_holding() {
        let symbols: Vec<String> = (0..5).map(|i| format!("S{i}")).collect();
        let (a, next) = rotate(&symbols, 0, 2);
        assert_eq!((a, next), (vec!["S0".to_string(), "S1".into()], 2));
        let (b, next) = rotate(&symbols, next, 2);
        assert_eq!((b, next), (vec!["S2".to_string(), "S3".into()], 4));
        // Wraps around, and reaches the last holding.
        let (c, next) = rotate(&symbols, next, 2);
        assert_eq!((c, next), (vec!["S4".to_string(), "S0".into()], 1));
        // Fewer holdings than the limit: all of them, cursor back at 0.
        assert_eq!(rotate(&symbols[..2], 1, 20).0.len(), 2);
        assert_eq!(rotate(&[], 3, 20), (Vec::new(), 0));
    }

    /// Run `filing_pending` and count every notice as delivered.
    fn deliver_filings(
        symbol: &str,
        filings: &[crate::research::Headline],
        seen: &mut HashMap<String, Vec<String>>,
    ) -> Vec<crate::notify::Message> {
        let mut out = Vec::new();
        for p in filing_pending(symbol, filings, seen) {
            if let Some((s, ids)) = p.filings {
                record_filings(seen, s, ids);
            }
            if !p.msg.title.is_empty() {
                out.push(p.msg);
            }
        }
        out
    }

    #[test]
    fn an_undelivered_filing_is_offered_again() {
        let mut seen = HashMap::new();
        deliver_filings("TCS.NS", &[], &mut seen);
        let f = filing(T0, "Updates", "Order win worth Rs 500 crore");
        let first = filing_pending("TCS.NS", std::slice::from_ref(&f), &seen);
        assert_eq!(first.len(), 1);
        // Not delivered, so nothing was recorded: the next sync offers it again.
        assert_eq!(
            filing_pending("TCS.NS", std::slice::from_ref(&f), &seen).len(),
            1
        );
        assert!(seen["TCS.NS"].is_empty());
    }

    #[test]
    fn first_sight_is_one_silent_record() {
        let seen = HashMap::new();
        let f = filing(T0, "Updates", "Order win worth Rs 500 crore");
        let p = filing_pending("TCS.NS", &[f], &seen);
        assert_eq!(p.len(), 1);
        assert!(p[0].msg.title.is_empty());
        assert_eq!(p[0].filings.as_ref().unwrap().1.len(), 1);
    }

    fn filing(ts: i64, category: &str, title: &str) -> crate::research::Headline {
        crate::research::Headline {
            ts,
            title: title.into(),
            summary: None,
            publisher: "NSE filing".into(),
            link: None,
            official: true,
            category: Some(category.into()),
        }
    }

    const DAY: i64 = 86_400;
    // 2026-11-02 00:00 UTC.
    const T0: i64 = 1_793_577_600;

    #[test]
    fn a_new_filing_is_announced_once_and_the_first_sight_is_silent() {
        let mut seen = HashMap::new();
        let old = filing(T0, "Updates", "Order win worth Rs 500 crore");
        assert!(deliver_filings("TCS.NS", std::slice::from_ref(&old), &mut seen).is_empty());
        assert!(deliver_filings("TCS.NS", std::slice::from_ref(&old), &mut seen).is_empty());
        let new = filing(T0 + DAY, "Outcome of Board Meeting", "Dividend declared");
        let both = [new.clone(), old];
        let msgs = deliver_filings("TCS.NS", &both, &mut seen);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].title.contains("TCS.NS"));
        assert!(
            msgs[0].body.contains("Dividend declared"),
            "{}",
            msgs[0].body
        );
        assert!(msgs[0].body.contains("03 Nov 2026"), "{}", msgs[0].body);
        assert!(msgs[0].body.contains("CORP ACTION"), "{}", msgs[0].body);
        assert!(msgs[0].body.contains("`stockwise events TCS.NS`"));
        crate::verdict::testing::assert_neutral(&msgs[0].body.replace("stockwise events", "x"));
        assert!(
            deliver_filings("TCS.NS", &both, &mut seen).is_empty(),
            "once"
        );
    }

    #[test]
    fn routine_filings_are_never_announced() {
        let mut seen = HashMap::new();
        deliver_filings("INFY.NS", &[], &mut seen);
        let routine = filing(T0, "Updates", "Trading Window closure intimation");
        assert!(deliver_filings("INFY.NS", &[routine], &mut seen).is_empty());
        assert!(
            seen["INFY.NS"].is_empty(),
            "routine ones are not remembered"
        );
    }

    #[test]
    fn filing_ids_are_stable_and_distinguish_titles_and_days() {
        let a = filing(T0, "x", "Acquisition of stake");
        assert_eq!(
            filing_id(&a),
            filing_id(&filing(T0 + 60, "y", "  ACQUISITION of stake "))
        );
        assert_ne!(
            filing_id(&a),
            filing_id(&filing(T0, "x", "Acquisition of land"))
        );
        assert_ne!(
            filing_id(&a),
            filing_id(&filing(T0 + DAY, "x", "Acquisition of stake"))
        );
        assert_eq!(
            filing_id(&a),
            "2026-11-02:".to_string() + &filing_id(&a)[11..]
        );
    }

    #[test]
    fn a_burst_of_filings_is_summarised_and_the_memory_is_capped() {
        let mut seen = HashMap::new();
        deliver_filings("LT.NS", &[], &mut seen);
        let burst: Vec<_> = (0..6)
            .map(|i| filing(T0 + i * 60, "Updates", &format!("Order win number {i}")))
            .collect();
        let msgs = deliver_filings("LT.NS", &burst, &mut seen);
        assert_eq!(msgs.len(), MAX_FILING_NOTICES + 1);
        assert!(
            msgs[0].body.contains("3 more new filings"),
            "{}",
            msgs[0].body
        );
        assert!(
            msgs[3].body.contains("Order win number 5"),
            "newest is last"
        );
        assert!(deliver_filings("LT.NS", &burst, &mut seen).is_empty());

        let many: Vec<_> = (0..300)
            .map(|i| filing(T0 + i * 60, "Updates", &format!("Order win number {i}")))
            .collect();
        deliver_filings("LT.NS", &many, &mut seen);
        assert_eq!(seen["LT.NS"].len(), MAX_FILINGS_SEEN);
    }

    #[test]
    fn long_titles_are_clipped() {
        let long = "word ".repeat(100);
        let c = clip(&long, 50);
        assert!(c.chars().count() <= 51 && c.ends_with('…'), "{c}");
        assert_eq!(clip("a  b\nc", 50), "a b c");
    }

    #[test]
    fn filing_checks_cover_every_holding_over_successive_syncs() {
        let held: Vec<String> = (0..25).map(|i| format!("S{i:02}.NS")).collect();
        let mut seen: HashMap<String, Vec<String>> = HashMap::new();
        let mut cursor = 0;
        let mut checked: std::collections::HashSet<String> = Default::default();
        // Every sync records what it checked, as the worker does on success.
        for _ in 0..6 {
            let (picks, next) = pick_filing_checks(&held, &seen, cursor, 20);
            assert!(picks.len() <= 20);
            cursor = next;
            for p in picks {
                checked.insert(p.clone());
                seen.insert(p, Vec::new());
            }
        }
        assert_eq!(checked.len(), 25, "every holding is eventually checked");
        // Unseen symbols always come first.
        let mut partial = HashMap::new();
        partial.insert("S00.NS".to_string(), Vec::new());
        let (picks, _) = pick_filing_checks(&held[..3], &partial, 0, 2);
        assert_eq!(picks, vec!["S01.NS".to_string(), "S02.NS".to_string()]);
        // Steady state alternates instead of repeating the first 20.
        let all_seen: HashMap<String, Vec<String>> =
            held.iter().map(|s| (s.clone(), Vec::new())).collect();
        let (first, next) = pick_filing_checks(&held, &all_seen, 0, 20);
        let (second, _) = pick_filing_checks(&held, &all_seen, next, 20);
        assert!(second.contains(&"S24.NS".to_string()) && !first.contains(&"S24.NS".to_string()));
    }

    #[test]
    fn tax_country_follows_the_market() {
        assert_eq!(tax_country(Market::In), Some(crate::tax::Country::India));
        assert_eq!(tax_country(Market::Us), Some(crate::tax::Country::Us));
        assert_eq!(tax_country(Market::Crypto), None);
    }

    #[test]
    fn run_task_needs_a_command() {
        assert!(enqueue(TaskKind::Run, Market::In, &[]).is_err());
    }
}
