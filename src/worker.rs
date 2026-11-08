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
    let market = if symbol.ends_with(".NS") || symbol.ends_with(".BO") {
        "in".to_string()
    } else {
        default_market.to_string()
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
        cap_tier: q
            .market_cap
            .map(|m| CapTier::classify(&market, m).as_str().to_string()),
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
}

/// After the close: refresh tracked snapshots, append today's NAV for held
/// funds, and add a wealth-history snapshot. Returns a result summary.
pub async fn eod_sync(
    client: &YahooClient,
    market: Market,
    mut progress: impl FnMut(f64) -> bool,
) -> Result<String> {
    let symbols = tracked_symbols();
    let snapshots = refresh_snapshots(client, market, &symbols, |p| progress(p * 0.5)).await?;

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
        for s in schemes.iter().filter(|s| codes.contains(&s.code)) {
            if let (Some(nav), Some(date)) = (s.nav, s.date) {
                let row = vec![(date.format("%Y-%m-%d").to_string(), nav)];
                db::with_db(|d| d.put_navs(&s.code.to_string(), &row));
                navs += 1;
            }
        }
    }
    progress(75.0);

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
    progress(100.0);
    Ok(serde_json::json!({
        "snapshots": snapshots,
        "fund_navs": navs,
        "wealth_snapshot": wealth,
    })
    .to_string())
}

/// The session date whose post-close sync is due, if any: the market closed
/// today and the sync has not run for this date yet.
fn eod_due(market: Market, now: chrono::DateTime<Utc>, state: &WorkerState) -> Option<String> {
    let date = clock::session_date(market, now);
    let close = clock::close_at(market, date)?;
    let key = date.to_string();
    (now >= close && state.last_eod.as_deref() != Some(&key)).then_some(key)
}

// ── Service: pid file, run loop, status ──

fn pid_path() -> Result<std::path::PathBuf> {
    crate::store::path(PID_FILE)
}

fn read_pid() -> Option<u32> {
    std::fs::read_to_string(pid_path().ok()?)
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Whether a process with this id is running (Unix: `kill -0`).
fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

/// The running worker's pid, if any.
pub fn running_pid() -> Option<u32> {
    read_pid().filter(|p| alive(*p))
}

/// Start the worker detached; returns its pid.
pub fn start(market: Market) -> Result<u32> {
    if let Some(pid) = running_pid() {
        bail!("The worker is already running (pid {pid})");
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
        let child = std::process::Command::new(exe)
            .args(["-m", &crate::value_name(market), "worker", "run"])
            .stdin(std::process::Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .process_group(0)
            .spawn()
            .context("Cannot start the worker")?;
        Ok(child.id())
    }
    #[cfg(not(unix))]
    {
        let _ = market;
        bail!("`worker start` needs Unix; run `stockwise worker run` in a terminal instead")
    }
}

/// Stop the worker; false when none was running.
pub async fn stop() -> Result<bool> {
    let Some(pid) = running_pid() else {
        let _ = std::fs::remove_file(pid_path()?);
        return Ok(false);
    };
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .arg(pid.to_string())
            .status();
    }
    for _ in 0..50 {
        if !alive(pid) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    if alive(pid) {
        bail!("The worker (pid {pid}) did not stop; stop it manually");
    }
    let _ = std::fs::remove_file(pid_path()?);
    Ok(true)
}

/// What `worker status` prints.
pub struct Status {
    pub pid: Option<u32>,
    pub last_eod: Option<String>,
    pub queued: usize,
    pub running: usize,
}

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

/// The foreground worker: ticker, task executor, alert checks, post-close
/// sync. Runs until Ctrl+C or SIGTERM.
pub async fn run_worker(market: Market) -> Result<()> {
    if let Some(pid) = running_pid()
        && pid != std::process::id()
    {
        bail!("Another worker is already running (pid {pid})");
    }
    std::fs::write(pid_path()?, std::process::id().to_string())?;
    let mut shutdown = crate::daemon::Shutdown::install("State is saved after every step.");
    // A task left running by a worker that died goes back in the queue.
    db::with_db(|d| d.requeue_stale_tasks(300));

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

    let mut alerts = crate::commands::AlertWatcher::load().ok();
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
        if due && let Some(w) = alerts.as_mut() {
            let _ = w.pass(market).await;
            state.last_alert_pass = Some(now.timestamp());
        }
        if let Some(date) = eod_due(market, now, &state) {
            let has_work = Portfolio::load().is_ok_and(|p| !p.holdings.is_empty())
                || !tracked_symbols().is_empty();
            if has_work && let Ok(client) = YahooClient::new().await {
                let _ = eod_sync(&client, market, |_| true).await;
            }
            state.last_eod = Some(date);
            let _ = crate::store::save(STATE_FILE, &state, false);
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5)) => {}
            _ = shutdown.wait() => break,
        }
    }
    ticker.abort();
    executor.abort();
    // Do not leave the interrupted task `running` forever: idempotent kinds
    // go back in the queue; a `run` command may have changed state, so it is
    // marked failed instead of being run twice.
    let id = current.load(std::sync::atomic::Ordering::SeqCst);
    if id > 0 {
        db::with_db(|d| d.release_task(id));
    }
    let _ = std::fs::remove_file(pid_path()?);
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
        assert_eq!(s.cap_tier.as_deref(), Some("large"));
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
    fn run_task_needs_a_command() {
        assert!(enqueue(TaskKind::Run, Market::In, &[]).is_err());
    }
}
