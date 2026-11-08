//! `cache.db`: a zero-setup SQLite (WAL) store in the data directory.
//!
//! Today it holds `http_cache`, the persistent L2 under the in-memory cache,
//! so consecutive commands reuse fresh provider responses without Redis. User
//! data (`portfolio.json`, `config.toml`) stays in readable JSON/TOML; this
//! file only holds data that can be fetched again, so it is always safe to
//! delete.
//!
//! Every function is best-effort: when the file cannot be opened (read-only
//! data directory, corrupt file) callers get `None`/`Err` and fall back to
//! the network.

use crate::store;
use anyhow::{Context, Result};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const FILE_NAME: &str = "cache.db";
/// Entries are dropped once this many rows exist, oldest first.
const MAX_ROWS: i64 = 5_000;

/// Counts and size of the persistent cache, for `cache status`.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CacheStats {
    pub entries: u64,
    pub fresh: u64,
    pub expired: u64,
    pub body_bytes: u64,
    pub file_bytes: u64,
}

/// Current Unix time in seconds.
fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Path of `cache.db` (the data directory is created if missing).
pub fn path() -> Result<PathBuf> {
    store::path(FILE_NAME)
}

pub use crate::market::CapTier;

/// One row of `market_snapshots`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Snapshot {
    pub symbol: String,
    pub market: String,
    pub name: String,
    pub sector: Option<String>,
    pub industry: Option<String>,
    pub cap_tier: Option<String>,
    pub mcap: Option<f64>,
    pub price: Option<f64>,
    pub change_1d: Option<f64>,
    pub rvol: Option<f64>,
    pub pe: Option<f64>,
    pub roe: Option<f64>,
    pub div_yield: Option<f64>,
    pub updated_at: i64,
}

/// Columns a snapshot query may sort by (a whitelist: never user text in SQL).
const SNAPSHOT_SORTS: &[&str] = &[
    "symbol",
    "mcap",
    "price",
    "change_1d",
    "rvol",
    "pe",
    "roe",
    "div_yield",
];

/// Filter for [`CacheDb::query_snapshots`].
#[derive(Debug, Clone, Default)]
pub struct SnapshotFilter {
    pub market: Option<String>,
    pub cap_tier: Option<CapTier>,
    pub sector: Option<String>,
    pub min_pe: Option<f64>,
    pub max_pe: Option<f64>,
    pub min_roe: Option<f64>,
    pub min_div_yield: Option<f64>,
    pub min_rvol: Option<f64>,
    /// Ignore rows not updated since this Unix time.
    pub updated_since: Option<i64>,
    pub sort_by: Option<String>,
    pub descending: bool,
    pub limit: Option<usize>,
}

/// One OHLCV bar; `ts` is Unix seconds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub ts: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

/// A background task row.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub id: i64,
    pub kind: String,
    pub status: String,
    pub progress_pct: f64,
    pub title: String,
    pub payload_json: String,
    pub result_json: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Map a `tasks` row (in `TASK_COLS` order) to a [`Task`].
fn task_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<Task> {
    Ok(Task {
        id: r.get(0)?,
        kind: r.get(1)?,
        status: r.get(2)?,
        progress_pct: r.get(3)?,
        title: r.get(4)?,
        payload_json: r.get(5)?,
        result_json: r.get(6)?,
        error: r.get(7)?,
        created_at: r.get(8)?,
        updated_at: r.get(9)?,
    })
}

const TASK_COLS: &str = "id, kind, status, progress_pct, title, payload_json, result_json, error, created_at, updated_at";

/// An open `cache.db`.
pub struct CacheDb {
    conn: Connection,
}

impl CacheDb {
    /// Open (creating if needed) the database at `path` in WAL mode.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Cannot open cache database {}", path.display()))?;
        Self::init(conn)
    }

    /// An in-memory database for tests.
    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    /// Set pragmas and create every table and index.
    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(std::time::Duration::from_millis(5000))?;
        // `journal_mode` returns a row, so it cannot go through execute_batch
        // on every SQLite build; read it and ignore the answer.
        let _: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;
        conn.execute_batch(
            "PRAGMA synchronous = NORMAL;
             CREATE TABLE IF NOT EXISTS http_cache (
                 key        TEXT PRIMARY KEY,
                 body       BLOB NOT NULL,
                 fetched_at INTEGER NOT NULL,
                 expires_at INTEGER NOT NULL,
                 source     TEXT NOT NULL DEFAULT ''
             );
             CREATE INDEX IF NOT EXISTS http_cache_expires ON http_cache(expires_at);
             CREATE TABLE IF NOT EXISTS market_snapshots (
                 symbol TEXT NOT NULL, market TEXT NOT NULL,
                 name TEXT NOT NULL DEFAULT '', sector TEXT, industry TEXT,
                 cap_tier TEXT, mcap REAL, price REAL, change_1d REAL,
                 rvol REAL, pe REAL, roe REAL, div_yield REAL,
                 updated_at INTEGER NOT NULL,
                 PRIMARY KEY (symbol, market)
             );
             CREATE INDEX IF NOT EXISTS snap_market_cap ON market_snapshots(market, cap_tier);
             CREATE INDEX IF NOT EXISTS snap_sector ON market_snapshots(market, sector);
             CREATE TABLE IF NOT EXISTS ohlcv_bars (
                 symbol TEXT NOT NULL, interval TEXT NOT NULL, ts INTEGER NOT NULL,
                 open REAL, high REAL, low REAL, close REAL, volume REAL,
                 PRIMARY KEY (symbol, interval, ts)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS nav_history (
                 code TEXT NOT NULL, date TEXT NOT NULL, nav REAL NOT NULL,
                 PRIMARY KEY (code, date)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS financial_results (
                 symbol TEXT NOT NULL, period_end TEXT NOT NULL,
                 is_annual INTEGER NOT NULL, source TEXT NOT NULL DEFAULT '',
                 payload_json TEXT NOT NULL, updated_at INTEGER NOT NULL,
                 PRIMARY KEY (symbol, period_end, is_annual)
             ) WITHOUT ROWID;
             CREATE TABLE IF NOT EXISTS tasks (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 kind TEXT NOT NULL, status TEXT NOT NULL DEFAULT 'queued',
                 progress_pct REAL NOT NULL DEFAULT 0, title TEXT NOT NULL DEFAULT '',
                 payload_json TEXT NOT NULL DEFAULT '{}', result_json TEXT, error TEXT,
                 created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS tasks_status ON tasks(status, id);",
        )?;
        Ok(Self { conn })
    }

    /// The body stored under `key`, if it has not expired.
    pub fn get(&self, key: &str) -> Result<Option<String>> {
        let body: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT body FROM http_cache WHERE key = ?1 AND expires_at > ?2",
                params![key, now_secs()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(body.and_then(|b| String::from_utf8(b).ok()))
    }

    /// Store `body` under `key` for `ttl_secs` (at least one second).
    pub fn set(&self, key: &str, body: &str, ttl_secs: u64, source: &str) -> Result<()> {
        let now = now_secs();
        let expires = now.saturating_add(ttl_secs.max(1).min(i64::MAX as u64 / 2) as i64);
        self.conn.execute(
            "INSERT INTO http_cache (key, body, fetched_at, expires_at, source)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(key) DO UPDATE SET body = excluded.body,
                 fetched_at = excluded.fetched_at,
                 expires_at = excluded.expires_at,
                 source = excluded.source",
            params![key, body.as_bytes(), now, expires, source],
        )?;
        self.trim()?;
        Ok(())
    }

    /// Drop expired rows, then the oldest ones above [`MAX_ROWS`].
    fn trim(&self) -> Result<()> {
        let rows: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM http_cache", [], |r| r.get(0))?;
        if rows <= MAX_ROWS {
            return Ok(());
        }
        self.conn.execute(
            "DELETE FROM http_cache WHERE expires_at <= ?1",
            params![now_secs()],
        )?;
        self.conn.execute(
            "DELETE FROM http_cache WHERE key IN (
                 SELECT key FROM http_cache ORDER BY fetched_at ASC
                 LIMIT MAX(0, (SELECT COUNT(*) FROM http_cache) - ?1))",
            params![MAX_ROWS],
        )?;
        Ok(())
    }

    /// Delete every cached response; returns how many rows went.
    pub fn clear(&self) -> Result<u64> {
        let n = self.conn.execute("DELETE FROM http_cache", [])?;
        Ok(n as u64)
    }

    // ── market_snapshots ──

    /// Insert or update snapshots by `(symbol, market)` in one transaction. A
    /// field the new row lacks keeps its stored value (a bulk index feed has
    /// prices but no P/E), so fundamentals are never blanked by a price scan.
    pub fn upsert_snapshots(&mut self, rows: &[Snapshot]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT INTO market_snapshots (symbol, market, name, sector, industry, cap_tier,
                     mcap, price, change_1d, rvol, pe, roe, div_yield, updated_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)
                 ON CONFLICT(symbol, market) DO UPDATE SET
                     name=CASE WHEN excluded.name = '' THEN name ELSE excluded.name END,
                     sector=COALESCE(excluded.sector, sector),
                     industry=COALESCE(excluded.industry, industry),
                     cap_tier=COALESCE(excluded.cap_tier, cap_tier),
                     mcap=COALESCE(excluded.mcap, mcap), price=excluded.price,
                     change_1d=excluded.change_1d, rvol=COALESCE(excluded.rvol, rvol),
                     pe=COALESCE(excluded.pe, pe), roe=COALESCE(excluded.roe, roe),
                     div_yield=COALESCE(excluded.div_yield, div_yield),
                     updated_at=excluded.updated_at",
            )?;
            for r in rows {
                st.execute(params![
                    r.symbol,
                    r.market,
                    r.name,
                    r.sector,
                    r.industry,
                    r.cap_tier,
                    r.mcap,
                    r.price,
                    r.change_1d,
                    r.rvol,
                    r.pe,
                    r.roe,
                    r.div_yield,
                    r.updated_at
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Snapshots matching `f`, sorted by a whitelisted column.
    pub fn query_snapshots(&self, f: &SnapshotFilter) -> Result<Vec<Snapshot>> {
        let mut sql = String::from(
            "SELECT symbol, market, name, sector, industry, cap_tier, mcap, price, change_1d,
                    rvol, pe, roe, div_yield, updated_at FROM market_snapshots WHERE 1=1",
        );
        let mut args: Vec<rusqlite::types::Value> = Vec::new();
        let mut cond = |clause: &str, v: rusqlite::types::Value| {
            args.push(v);
            sql.push_str(&format!(" AND {clause} ?{}", args.len()));
        };
        if let Some(v) = &f.market {
            cond("market =", v.clone().into());
        }
        if let Some(v) = f.cap_tier {
            cond("cap_tier =", v.as_str().to_string().into());
        }
        if let Some(v) = &f.sector {
            cond("sector =", v.clone().into());
        }
        if let Some(v) = f.min_pe {
            cond("pe >=", v.into());
        }
        if let Some(v) = f.max_pe {
            cond("pe <=", v.into());
        }
        if let Some(v) = f.min_roe {
            cond("roe >=", v.into());
        }
        if let Some(v) = f.min_div_yield {
            cond("div_yield >=", v.into());
        }
        if let Some(v) = f.min_rvol {
            cond("rvol >=", v.into());
        }
        if let Some(v) = f.updated_since {
            cond("updated_at >=", v.into());
        }
        let sort = f
            .sort_by
            .as_deref()
            .filter(|c| SNAPSHOT_SORTS.contains(c))
            .unwrap_or("symbol");
        sql.push_str(&format!(
            " ORDER BY {sort} {}, symbol ASC",
            if f.descending { "DESC" } else { "ASC" }
        ));
        if let Some(n) = f.limit {
            sql.push_str(&format!(" LIMIT {n}"));
        }
        let mut st = self.conn.prepare(&sql)?;
        let rows = st.query_map(rusqlite::params_from_iter(args), |r| {
            Ok(Snapshot {
                symbol: r.get(0)?,
                market: r.get(1)?,
                name: r.get(2)?,
                sector: r.get(3)?,
                industry: r.get(4)?,
                cap_tier: r.get(5)?,
                mcap: r.get(6)?,
                price: r.get(7)?,
                change_1d: r.get(8)?,
                rvol: r.get(9)?,
                pe: r.get(10)?,
                roe: r.get(11)?,
                div_yield: r.get(12)?,
                updated_at: r.get(13)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ── ohlcv_bars ──

    /// Insert or replace bars for `symbol` and `interval` in one transaction.
    pub fn put_bars(&mut self, symbol: &str, interval: &str, bars: &[Bar]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st = tx.prepare_cached(
                "INSERT OR REPLACE INTO ohlcv_bars VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
            )?;
            for b in bars {
                st.execute(params![
                    symbol, interval, b.ts, b.open, b.high, b.low, b.close, b.volume
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Bars with `from <= ts <= to`, oldest first.
    pub fn get_bars(&self, symbol: &str, interval: &str, from: i64, to: i64) -> Result<Vec<Bar>> {
        let mut st = self.conn.prepare_cached(
            "SELECT ts, open, high, low, close, volume FROM ohlcv_bars
             WHERE symbol=?1 AND interval=?2 AND ts BETWEEN ?3 AND ?4 ORDER BY ts",
        )?;
        let rows = st.query_map(params![symbol, interval, from, to], |r| {
            Ok(Bar {
                ts: r.get(0)?,
                open: r.get(1)?,
                high: r.get(2)?,
                low: r.get(3)?,
                close: r.get(4)?,
                volume: r.get(5)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ── nav_history ──

    /// `navs` are `(YYYY-MM-DD, nav)` pairs.
    pub fn put_navs(&mut self, code: &str, navs: &[(String, f64)]) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut st =
                tx.prepare_cached("INSERT OR REPLACE INTO nav_history VALUES (?1,?2,?3)")?;
            for (d, n) in navs {
                st.execute(params![code, d, n])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// `(date, nav)` pairs for a fund, oldest first.
    pub fn get_navs(&self, code: &str) -> Result<Vec<(String, f64)>> {
        let mut st = self
            .conn
            .prepare_cached("SELECT date, nav FROM nav_history WHERE code=?1 ORDER BY date")?;
        let rows = st.query_map(params![code], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ── financial_results ──

    /// Insert or replace one period of financial results.
    pub fn put_result(
        &self,
        symbol: &str,
        period_end: &str,
        is_annual: bool,
        source: &str,
        payload_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO financial_results VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                symbol,
                period_end,
                is_annual,
                source,
                payload_json,
                now_secs()
            ],
        )?;
        Ok(())
    }

    /// `(period_end, is_annual, payload_json)`, newest period first.
    pub fn get_results(&self, symbol: &str) -> Result<Vec<(String, bool, String)>> {
        let mut st = self.conn.prepare_cached(
            "SELECT period_end, is_annual, payload_json FROM financial_results
             WHERE symbol=?1 ORDER BY period_end DESC",
        )?;
        let rows = st.query_map(params![symbol], |r| {
            Ok((r.get(0)?, r.get::<_, i64>(1)? != 0, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    // ── tasks ──

    /// Queue a task; returns its id.
    pub fn enqueue_task(&self, kind: &str, title: &str, payload_json: &str) -> Result<i64> {
        let now = now_secs();
        self.conn.execute(
            "INSERT INTO tasks (kind, title, payload_json, created_at, updated_at)
             VALUES (?1,?2,?3,?4,?4)",
            params![kind, title, payload_json, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    /// One task by id.
    pub fn get_task(&self, id: i64) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {TASK_COLS} FROM tasks WHERE id=?1"),
                params![id],
                task_from_row,
            )
            .optional()?)
    }

    /// Newest first.
    pub fn list_tasks(&self, limit: usize) -> Result<Vec<Task>> {
        let mut st = self.conn.prepare(&format!(
            "SELECT {TASK_COLS} FROM tasks ORDER BY id DESC LIMIT ?1"
        ))?;
        let rows = st.query_map(params![limit as i64], task_from_row)?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Atomically move the oldest queued task to `running` and return it, so
    /// two workers never take the same one.
    pub fn claim_task(&self) -> Result<Option<Task>> {
        Ok(self
            .conn
            .query_row(
                &format!(
                    "UPDATE tasks SET status='running', updated_at=?1
                     WHERE id = (SELECT id FROM tasks WHERE status='queued' ORDER BY id LIMIT 1)
                     RETURNING {TASK_COLS}"
                ),
                params![now_secs()],
                task_from_row,
            )
            .optional()?)
    }

    /// Record progress of a running task; returns false if it is no longer
    /// running (cancelled), so the executor can stop.
    pub fn set_task_progress(&self, id: i64, pct: f64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE tasks SET progress_pct=?2, updated_at=?3 WHERE id=?1 AND status='running'",
            params![id, pct.clamp(0.0, 100.0), now_secs()],
        )?;
        Ok(n > 0)
    }

    /// Record a running task as `done` (with a result) or `failed` (with an error).
    pub fn finish_task(&self, id: i64, outcome: std::result::Result<&str, &str>) -> Result<()> {
        let (status, result, error) = match outcome {
            Ok(r) => ("done", Some(r), None),
            Err(e) => ("failed", None, Some(e)),
        };
        self.conn.execute(
            "UPDATE tasks SET status=?2, result_json=?3, error=?4,
                 progress_pct = CASE WHEN ?2='done' THEN 100 ELSE progress_pct END,
                 updated_at=?5
             WHERE id=?1 AND status='running'",
            params![id, status, result, error, now_secs()],
        )?;
        Ok(())
    }

    /// Cancel a queued or running task; false when it already finished.
    pub fn cancel_task(&self, id: i64) -> Result<bool> {
        let n = self.conn.execute(
            "UPDATE tasks SET status='cancelled', updated_at=?2
             WHERE id=?1 AND status IN ('queued','running')",
            params![id, now_secs()],
        )?;
        Ok(n > 0)
    }

    /// Tasks left `running` by a worker that died. Idempotent kinds (`warm`,
    /// `eod`) go back to `queued`; others (a `run` command may already have
    /// changed state) are marked `failed` rather than run twice. Returns how
    /// many were requeued.
    pub fn requeue_stale_tasks(&self, older_than_secs: i64) -> Result<u64> {
        let (cutoff, now) = (now_secs() - older_than_secs, now_secs());
        self.conn.execute(
            "UPDATE tasks SET status='failed', error='worker stopped while this was running',
                 updated_at=?2
             WHERE status='running' AND updated_at <= ?1 AND kind NOT IN ('warm','eod')",
            params![cutoff, now],
        )?;
        let n = self.conn.execute(
            "UPDATE tasks SET status='queued', updated_at=?2
             WHERE status='running' AND updated_at <= ?1 AND kind IN ('warm','eod')",
            params![cutoff, now],
        )?;
        Ok(n as u64)
    }

    /// Release a task whose worker is shutting down: requeue it when it is
    /// safe to run again, else mark it failed.
    pub fn release_task(&self, id: i64) -> Result<()> {
        let now = now_secs();
        self.conn.execute(
            "UPDATE tasks SET status = CASE WHEN kind IN ('warm','eod') THEN 'queued' ELSE 'failed' END,
                 error = CASE WHEN kind IN ('warm','eod') THEN NULL
                              ELSE 'interrupted by worker shutdown' END,
                 progress_pct = CASE WHEN kind IN ('warm','eod') THEN 0 ELSE progress_pct END,
                 updated_at=?2
             WHERE id=?1 AND status='running'",
            params![id, now],
        )?;
        Ok(())
    }

    /// Entry counts and sizes for `cache status`.
    pub fn stats(&self, file_bytes: u64) -> Result<CacheStats> {
        let (entries, fresh, bytes): (i64, i64, i64) = self.conn.query_row(
            "SELECT COUNT(*),
                    COALESCE(SUM(expires_at > ?1), 0),
                    COALESCE(SUM(LENGTH(body)), 0)
             FROM http_cache",
            params![now_secs()],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok(CacheStats {
            entries: entries as u64,
            fresh: fresh as u64,
            expired: (entries - fresh) as u64,
            body_bytes: bytes as u64,
            file_bytes,
        })
    }
}

/// The shared connection for this process. `None` when `cache.db` cannot be
/// opened; that is remembered so a broken file costs one failed open, not one
/// per request.
fn shared() -> Option<&'static Mutex<CacheDb>> {
    static DB: OnceLock<Option<Mutex<CacheDb>>> = OnceLock::new();
    DB.get_or_init(|| {
        if persistent_cache_disabled() {
            return None;
        }
        let path = path().ok()?;
        CacheDb::open(&path).ok().map(Mutex::new)
    })
    .as_ref()
}

/// `STOCKWISE_CACHE_DB=off` turns the persistent cache off.
fn persistent_cache_disabled() -> bool {
    std::env::var("STOCKWISE_CACHE_DB")
        .map(|v| {
            matches!(
                v.trim().to_ascii_lowercase().as_str(),
                "off" | "0" | "false"
            )
        })
        .unwrap_or(false)
}

/// Run `f` on the shared database; `None` when it is off or `f` fails.
pub fn with_db<T>(f: impl FnOnce(&mut CacheDb) -> Result<T>) -> Option<T> {
    let mut db = shared()?.lock().ok()?;
    f(&mut db).ok()
}

/// A shared handle to a [`CacheDb`] for the HTTP cache layer.
pub type L2 = &'static Mutex<CacheDb>;

/// The process-wide persistent cache, `None` when it is off or unusable.
pub fn l2() -> Option<L2> {
    shared()
}

impl CacheDb {
    /// Cached body for `key` through a shared handle, ignoring errors.
    pub fn l2_get(l2: L2, key: &str) -> Option<String> {
        l2.lock().ok()?.get(key).ok().flatten()
    }

    /// Store a body through a shared handle, ignoring errors.
    pub fn l2_set(l2: L2, key: &str, body: &str, ttl_secs: u64) {
        if let Ok(db) = l2.lock() {
            let _ = db.set(key, body, ttl_secs, "http");
        }
    }
}

/// Stats for `cache status`, `None` when the cache is off or unreadable.
pub fn http_stats() -> Option<CacheStats> {
    let file_bytes = path()
        .ok()
        .and_then(|p| std::fs::metadata(p).ok())
        .map(|m| m.len())
        .unwrap_or(0);
    shared()?.lock().ok()?.stats(file_bytes).ok()
}

/// Delete all cached responses; `None` when the cache is off or unreadable.
pub fn http_clear() -> Option<u64> {
    shared()?.lock().ok()?.clear().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_then_get_round_trips() {
        let db = CacheDb::open_in_memory().unwrap();
        db.set("k", "{\"a\":1}", 60, "yahoo").unwrap();
        assert_eq!(db.get("k").unwrap().as_deref(), Some("{\"a\":1}"));
        assert_eq!(db.get("missing").unwrap(), None);
    }

    #[test]
    fn expired_entries_are_not_returned() {
        let db = CacheDb::open_in_memory().unwrap();
        db.set("k", "v", 60, "").unwrap();
        db.conn
            .execute(
                "UPDATE http_cache SET expires_at = ?1",
                params![now_secs() - 1],
            )
            .unwrap();
        assert_eq!(db.get("k").unwrap(), None);
        let stats = db.stats(0).unwrap();
        assert_eq!((stats.entries, stats.fresh, stats.expired), (1, 0, 1));
    }

    #[test]
    fn set_overwrites_and_clear_empties() {
        let db = CacheDb::open_in_memory().unwrap();
        db.set("k", "one", 60, "").unwrap();
        db.set("k", "two", 60, "").unwrap();
        assert_eq!(db.get("k").unwrap().as_deref(), Some("two"));
        db.set("j", "x", 60, "").unwrap();
        assert_eq!(db.clear().unwrap(), 2);
        assert_eq!(db.stats(0).unwrap().entries, 0);
    }

    #[test]
    fn rows_are_capped_oldest_first() {
        let db = CacheDb::open_in_memory().unwrap();
        for i in 0..(MAX_ROWS + 10) {
            db.conn
                .execute(
                    "INSERT INTO http_cache VALUES (?1, x'00', ?2, ?3, '')",
                    params![format!("k{i}"), i, now_secs() + 3600],
                )
                .unwrap();
        }
        db.set("new", "v", 60, "").unwrap();
        let stats = db.stats(0).unwrap();
        assert_eq!(stats.entries, MAX_ROWS as u64);
        assert_eq!(db.get("k0").unwrap(), None);
        assert_eq!(db.get("new").unwrap().as_deref(), Some("v"));
    }

    fn snap(i: usize, market: &str, mcap: f64) -> Snapshot {
        Snapshot {
            symbol: format!("S{i}"),
            market: market.into(),
            name: format!("Stock {i}"),
            sector: Some(if i.is_multiple_of(2) { "Tech" } else { "Banks" }.into()),
            cap_tier: Some(
                CapTier::classify(
                    "",
                    Some(mcap),
                    Some(if market == "in" { "INR" } else { "USD" }),
                    None,
                )
                .as_str()
                .into(),
            ),
            mcap: Some(mcap),
            price: Some(100.0 + i as f64),
            change_1d: Some((i % 7) as f64 - 3.0),
            rvol: Some(0.5 + (i % 5) as f64),
            pe: Some(10.0 + (i % 40) as f64),
            updated_at: now_secs(),
            ..Default::default()
        }
    }

    #[test]
    fn snapshots_filter_and_sort_by_cap_and_columns() {
        let mut db = CacheDb::open_in_memory().unwrap();
        // 600 stocks across all tiers; market caps spread from micro to mega.
        let rows: Vec<Snapshot> = (0..600)
            .map(|i| snap(i, "in", 1.0e9 * 1.03f64.powi(i as i32 * 2)))
            .collect();
        db.upsert_snapshots(&rows).unwrap();
        let all = db
            .query_snapshots(&SnapshotFilter {
                market: Some("in".into()),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(all.len(), 600);
        let tiers: std::collections::HashSet<_> =
            all.iter().filter_map(|r| r.cap_tier.clone()).collect();
        assert!(tiers.len() >= 4, "{tiers:?}");
        let large = db
            .query_snapshots(&SnapshotFilter {
                cap_tier: Some(CapTier::Large),
                min_pe: Some(20.0),
                sector: Some("Tech".into()),
                sort_by: Some("pe".into()),
                descending: true,
                limit: Some(5),
                ..Default::default()
            })
            .unwrap();
        assert!(large.len() <= 5);
        assert!(large.iter().all(|r| r.cap_tier.as_deref() == Some("large")
            && r.pe.unwrap() >= 20.0
            && r.sector.as_deref() == Some("Tech")));
        assert!(large.windows(2).all(|w| w[0].pe >= w[1].pe));
        // A bogus sort column falls back to symbol instead of reaching SQL.
        let rows = db
            .query_snapshots(&SnapshotFilter {
                sort_by: Some("symbol; DROP TABLE tasks".into()),
                limit: Some(1),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(rows.len(), 1);
        // Upsert replaces by (symbol, market).
        let mut again = snap(0, "in", 5.0e12);
        again.price = Some(1.0);
        db.upsert_snapshots(&[again]).unwrap();
        let n = db
            .query_snapshots(&SnapshotFilter::default())
            .unwrap()
            .len();
        assert_eq!(n, 600);
    }

    #[test]
    fn a_price_only_upsert_keeps_stored_fundamentals() {
        let mut db = CacheDb::open_in_memory().unwrap();
        let full = Snapshot {
            symbol: "TCS.NS".into(),
            market: "in".into(),
            name: "TCS".into(),
            sector: Some("Technology".into()),
            cap_tier: Some("large".into()),
            mcap: Some(1.4e13),
            price: Some(3000.0),
            pe: Some(28.0),
            updated_at: 1,
            ..Default::default()
        };
        db.upsert_snapshots(&[full]).unwrap();
        // An NSE bulk row: price and change only.
        let bulk = Snapshot {
            symbol: "TCS.NS".into(),
            market: "in".into(),
            price: Some(3100.0),
            change_1d: Some(1.5),
            updated_at: 2,
            ..Default::default()
        };
        db.upsert_snapshots(&[bulk]).unwrap();
        let rows = db.query_snapshots(&SnapshotFilter::default()).unwrap();
        assert_eq!(rows.len(), 1);
        let r = &rows[0];
        assert_eq!(r.price, Some(3100.0));
        assert_eq!(r.change_1d, Some(1.5));
        assert_eq!(r.name, "TCS");
        assert_eq!(r.sector.as_deref(), Some("Technology"));
        assert_eq!(r.cap_tier.as_deref(), Some("large"));
        assert_eq!(r.pe, Some(28.0));
        assert_eq!(r.mcap, Some(1.4e13));
        assert_eq!(r.updated_at, 2);
    }

    #[test]
    fn bars_navs_and_results_round_trip() {
        let mut db = CacheDb::open_in_memory().unwrap();
        let bars: Vec<Bar> = (0..10)
            .map(|i| Bar {
                ts: 1000 + i,
                open: 1.0,
                high: 2.0,
                low: 0.5,
                close: 1.5,
                volume: 10.0,
            })
            .collect();
        db.put_bars("TCS.NS", "1d", &bars).unwrap();
        db.put_bars("TCS.NS", "1d", &bars[..3]).unwrap();
        assert_eq!(db.get_bars("TCS.NS", "1d", 1002, 1005).unwrap().len(), 4);
        assert!(db.get_bars("TCS.NS", "1h", 0, i64::MAX).unwrap().is_empty());
        db.put_navs(
            "120503",
            &[("2026-01-02".into(), 10.5), ("2026-01-01".into(), 10.0)],
        )
        .unwrap();
        assert_eq!(
            db.get_navs("120503").unwrap()[0],
            ("2026-01-01".to_string(), 10.0)
        );
        db.put_result("INFY.NS", "2026-03-31", false, "nse", "{\"revenue\":1}")
            .unwrap();
        db.put_result("INFY.NS", "2026-03-31", false, "nse", "{\"revenue\":2}")
            .unwrap();
        db.put_result("INFY.NS", "2025-03-31", true, "nse", "{}")
            .unwrap();
        let r = db.get_results("INFY.NS").unwrap();
        assert_eq!(r.len(), 2);
        assert_eq!(
            r[0],
            (
                "2026-03-31".to_string(),
                false,
                "{\"revenue\":2}".to_string()
            )
        );
    }

    #[test]
    fn task_lifecycle_claim_progress_cancel_requeue() {
        let db = CacheDb::open_in_memory().unwrap();
        let a = db.enqueue_task("warm", "Warm quotes", "{}").unwrap();
        let b = db
            .enqueue_task("backtest", "Backtest", "{\"s\":1}")
            .unwrap();
        let first = db.claim_task().unwrap().unwrap();
        assert_eq!((first.id, first.status.as_str()), (a, "running"));
        assert!(db.set_task_progress(a, 40.0).unwrap());
        assert_eq!(db.get_task(a).unwrap().unwrap().progress_pct, 40.0);
        // Cancelling a running task makes the executor see it.
        assert!(db.cancel_task(a).unwrap());
        assert!(!db.set_task_progress(a, 50.0).unwrap());
        assert!(!db.cancel_task(a).unwrap());
        let second = db.claim_task().unwrap().unwrap();
        assert_eq!(second.id, b);
        assert!(db.claim_task().unwrap().is_none());
        db.finish_task(b, Ok("{\"ok\":true}")).unwrap();
        let done = db.get_task(b).unwrap().unwrap();
        assert_eq!((done.status.as_str(), done.progress_pct), ("done", 100.0));
        let c = db.enqueue_task("x", "", "{}").unwrap();
        db.claim_task().unwrap();
        db.finish_task(c, Err("boom")).unwrap();
        assert_eq!(
            db.get_task(c).unwrap().unwrap().error.as_deref(),
            Some("boom")
        );
        // A stuck idempotent task is requeued once it is old enough.
        let d = db.enqueue_task("warm", "", "{}").unwrap();
        let r = db.enqueue_task("run", "", "{}").unwrap();
        db.claim_task().unwrap();
        db.claim_task().unwrap();
        db.conn
            .execute(
                "UPDATE tasks SET updated_at = updated_at - 1000 WHERE id IN (?1, ?2)",
                params![d, r],
            )
            .unwrap();
        assert_eq!(db.requeue_stale_tasks(600).unwrap(), 1);
        assert_eq!(db.get_task(d).unwrap().unwrap().status, "queued");
        // A `run` command is never run twice: failed, not requeued.
        let run = db.get_task(r).unwrap().unwrap();
        assert_eq!(run.status, "failed");
        assert!(run.error.unwrap().contains("worker stopped"));
        // Shutdown release follows the same rule.
        db.claim_task().unwrap();
        db.release_task(d).unwrap();
        assert_eq!(db.get_task(d).unwrap().unwrap().status, "queued");
        // Take it out of the queue so the next claim is the `run` task.
        assert!(db.cancel_task(d).unwrap());
        let r2 = db.enqueue_task("run", "", "{}").unwrap();
        db.claim_task().unwrap();
        db.release_task(r2).unwrap();
        assert_eq!(db.get_task(r2).unwrap().unwrap().status, "failed");
        assert_eq!(db.list_tasks(2).unwrap().len(), 2);
    }

    #[test]
    fn file_database_persists_across_connections() {
        let dir = std::env::temp_dir().join(format!("stockwise-db-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("cache.db");
        CacheDb::open(&file).unwrap().set("k", "v", 60, "").unwrap();
        assert_eq!(
            CacheDb::open(&file).unwrap().get("k").unwrap().as_deref(),
            Some("v")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
