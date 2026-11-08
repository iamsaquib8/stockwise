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

/// An open `cache.db`.
pub struct CacheDb {
    conn: Connection,
}

impl CacheDb {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)
            .with_context(|| format!("Cannot open cache database {}", path.display()))?;
        Self::init(conn)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

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
             CREATE INDEX IF NOT EXISTS http_cache_expires ON http_cache(expires_at);",
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

/// Persistent-cache read used by the HTTP layer.
pub fn http_get(key: &str) -> Option<String> {
    shared()?.lock().ok()?.get(key).ok().flatten()
}

/// Persistent-cache write used by the HTTP layer.
pub fn http_set(key: &str, body: &str, ttl_secs: u64, source: &str) {
    if let Some(db) = shared()
        && let Ok(db) = db.lock()
    {
        let _ = db.set(key, body, ttl_secs, source);
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
