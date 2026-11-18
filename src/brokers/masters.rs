//! Daily cache for broker instrument masters (Kite, Upstox, Dhan, Fyers).
//!
//! Each broker publishes its instrument list once a day before the market
//! opens, and the files are 15–30 MB. Instead of downloading the list for
//! every symbol lookup, [`MasterCache::load`] keeps:
//!
//! - one file per broker, exchange and IST date under
//!   `cache/instruments/` in the data directory, written atomically and
//!   checked with a length and SHA-256 header, and
//! - the parsed list in memory for the life of the process.
//!
//! A copy from an earlier IST day is never used: the next lookup downloads
//! the list again and removes the old file. A cached file that is
//! truncated, damaged or no longer parses is downloaded again. A cache that
//! cannot be written (for example a read-only data directory) only costs
//! the download; it never blocks an order.

use anyhow::{Context, Result};
use chrono::{NaiveDate, Utc};
use std::any::Any;
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// First line of every cache file: format tag, body length, body SHA-256.
const MAGIC: &str = "stockwise-instrument-master 1";

type Parsed = Arc<dyn Any + Send + Sync>;
type Today = Arc<dyn Fn() -> NaiveDate + Send + Sync>;

/// Where instrument masters are kept between lookups.
pub struct MasterCache {
    /// `None` turns the cache off: every lookup downloads (unit tests of a
    /// broker's own parsing and request shapes).
    dir: Option<PathBuf>,
    today: Today,
    memory: Mutex<HashMap<String, Parsed>>,
}

/// The process-wide cache in the data directory, shared by every broker
/// client so a second client in the same process reuses the parsed list.
static SHARED: LazyLock<Arc<MasterCache>> = LazyLock::new(|| {
    let dir = crate::store::data_dir()
        .ok()
        .map(|d| d.join("cache").join("instruments"));
    Arc::new(MasterCache {
        dir,
        today: Arc::new(ist_today),
        memory: Mutex::new(HashMap::new()),
    })
});

/// Today's date in India, which is when NSE masters roll over.
pub fn ist_today() -> NaiveDate {
    Utc::now().with_timezone(&crate::clock::ist()).date_naive()
}

impl MasterCache {
    /// The shared cache in the StockWise data directory.
    pub fn shared() -> Arc<MasterCache> {
        SHARED.clone()
    }

    /// No cache: every lookup downloads.
    pub fn off() -> Arc<MasterCache> {
        Arc::new(MasterCache {
            dir: None,
            today: Arc::new(ist_today),
            memory: Mutex::new(HashMap::new()),
        })
    }

    /// A cache in `dir` whose idea of "today" comes from `today` (tests).
    #[cfg(test)]
    pub fn at(dir: &Path, today: impl Fn() -> NaiveDate + Send + Sync + 'static) -> MasterCache {
        MasterCache {
            dir: Some(dir.to_path_buf()),
            today: Arc::new(today),
            memory: Mutex::new(HashMap::new()),
        }
    }

    /// The parsed master for `broker` / `exchange` for today (IST).
    ///
    /// Order of lookup: memory, then today's file, then `fetch`. `parse`
    /// turns the downloaded bytes into the lookup structure; bytes that do
    /// not parse are never cached. A cached file that fails its checksum or
    /// `parse` is treated as missing.
    pub async fn load<T, F, Fut>(
        &self,
        broker: &str,
        exchange: &str,
        fetch: F,
        parse: fn(&[u8]) -> Result<T>,
    ) -> Result<Arc<T>>
    where
        T: Send + Sync + 'static,
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Vec<u8>>>,
    {
        let Some(dir) = &self.dir else {
            return Ok(Arc::new(parse(&fetch().await?)?));
        };
        let date = (self.today)();
        let stem = format!("{broker}-{exchange}-");
        let key = format!("{stem}{date}");
        if let Some(hit) = self.remembered::<T>(&key) {
            return Ok(hit);
        }
        let path = dir.join(format!("{key}.master"));
        if let Some(parsed) = read_file(&path).and_then(|b| parse(&b).ok()) {
            return Ok(self.remember(&stem, key, parsed));
        }
        let body = fetch().await?;
        let parsed = parse(&body)?;
        // Best effort: a cache that cannot be written only costs a download.
        if std::fs::create_dir_all(dir).is_ok()
            && crate::pagecache::write_atomic(&path, envelope(&body)).is_ok()
        {
            remove_older(dir, &stem, &path);
        }
        Ok(self.remember(&stem, key, parsed))
    }

    fn remembered<T: Send + Sync + 'static>(&self, key: &str) -> Option<Arc<T>> {
        let memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        memory.get(key)?.clone().downcast::<T>().ok()
    }

    fn remember<T: Send + Sync + 'static>(&self, stem: &str, key: String, parsed: T) -> Arc<T> {
        let parsed = Arc::new(parsed);
        let mut memory = self.memory.lock().unwrap_or_else(|e| e.into_inner());
        // Earlier days of the same master are no longer needed.
        memory.retain(|k, _| !k.starts_with(stem));
        memory.insert(key, parsed.clone() as Parsed);
        parsed
    }
}

fn sha256(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Header line plus body, so a short or damaged file is caught on read.
fn envelope(body: &[u8]) -> Vec<u8> {
    let mut out = format!("{MAGIC} {} {}\n", body.len(), sha256(body)).into_bytes();
    out.extend_from_slice(body);
    out
}

/// The body of a cache file whose header matches its contents.
fn read_file(path: &Path) -> Option<Vec<u8>> {
    let raw = std::fs::read(path).ok()?;
    let nl = raw.iter().position(|&b| b == b'\n')?;
    let header = std::str::from_utf8(&raw[..nl]).ok()?;
    let rest = header.strip_prefix(MAGIC)?.trim();
    let (len, sum) = rest.split_once(' ')?;
    let body = &raw[nl + 1..];
    (len.parse::<usize>().ok()? == body.len() && sum == sha256(body)).then(|| body.to_vec())
}

/// Delete earlier days' files for the same broker and exchange.
fn remove_older(dir: &Path, stem: &str, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let name = e.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(stem) && name.ends_with(".master") && e.path() != keep {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

/// Bytes as text after gunzipping them if needed. A master with no header
/// line is refused so an empty or damaged download is never cached.
pub fn csv_text(bytes: &[u8]) -> Result<String> {
    let raw = super::http::maybe_gunzip(bytes)?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    text.lines()
        .next()
        .filter(|h| h.contains(','))
        .context("The instrument list is empty or not CSV")?;
    Ok(text)
}

/// [`csv_text`], refused unless `ok` accepts the first line's fields. Each
/// broker passes the check for the columns its lookup reads, so an error
/// page or a changed layout is never cached for the rest of the day.
pub fn csv_checked(bytes: &[u8], what: &str, ok: fn(&[String]) -> bool) -> Result<String> {
    let text = csv_text(bytes)?;
    let first: Vec<String> = super::http::csv_fields(text.lines().next().unwrap_or_default())
        .into_iter()
        .map(|f| f.trim().to_string())
        .collect();
    if !ok(&first) {
        anyhow::bail!("The {what} does not have the expected columns");
    }
    Ok(text)
}

/// Whether `fields` holds every name in `names`.
pub fn has_columns(fields: &[String], names: &[&str]) -> bool {
    names.iter().all(|n| fields.iter().any(|f| f == n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sw-masters-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 11, d).unwrap()
    }

    /// A cache whose date can be moved, plus a download counter.
    struct Rig {
        dir: PathBuf,
        date: Arc<Mutex<NaiveDate>>,
        downloads: Arc<AtomicUsize>,
    }

    impl Rig {
        fn new(name: &str) -> Self {
            Rig {
                dir: temp_dir(name),
                date: Arc::new(Mutex::new(day(17))),
                downloads: Arc::new(AtomicUsize::new(0)),
            }
        }

        /// A fresh cache over the same directory, like a new process.
        fn cache(&self) -> MasterCache {
            let date = self.date.clone();
            MasterCache::at(&self.dir, move || *date.lock().unwrap())
        }

        async fn load(&self, cache: &MasterCache, body: &str) -> Result<Arc<String>> {
            let n = self.downloads.clone();
            let body = body.to_string();
            cache
                .load(
                    "kite",
                    "NSE",
                    || async move {
                        n.fetch_add(1, Ordering::SeqCst);
                        Ok(body.into_bytes())
                    },
                    csv_text,
                )
                .await
        }

        fn files(&self) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(&self.dir)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }

        fn downloads(&self) -> usize {
            self.downloads.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn same_day_lookups_download_once_in_and_across_processes() {
        let rig = Rig::new("same-day");
        let cache = rig.cache();
        let first = rig.load(&cache, "a,b\n1,2\n").await.unwrap();
        assert_eq!(rig.downloads(), 1);
        // Same process: served from memory.
        let again = rig.load(&cache, "never,used\n").await.unwrap();
        assert!(Arc::ptr_eq(&first, &again));
        // New process, same day: served from the file.
        let fresh = rig.load(&rig.cache(), "never,used\n").await.unwrap();
        assert_eq!(*fresh, "a,b\n1,2\n");
        assert_eq!(rig.downloads(), 1);
        assert_eq!(rig.files(), ["kite-NSE-2026-11-17.master"]);
    }

    #[tokio::test]
    async fn a_new_day_downloads_again_and_drops_the_old_file() {
        let rig = Rig::new("new-day");
        let cache = rig.cache();
        rig.load(&cache, "old,list\n").await.unwrap();
        *rig.date.lock().unwrap() = day(18);
        // The same long-running process sees the new day too.
        let got = rig.load(&cache, "new,list\n").await.unwrap();
        assert_eq!(*got, "new,list\n");
        assert_eq!(rig.downloads(), 2);
        assert_eq!(rig.files(), ["kite-NSE-2026-11-18.master"]);
    }

    #[tokio::test]
    async fn a_damaged_file_is_downloaded_again() {
        let rig = Rig::new("corrupt");
        rig.load(&rig.cache(), "a,b\n1,2\n").await.unwrap();
        let path = rig.dir.join("kite-NSE-2026-11-17.master");

        // Truncated body: the checksum no longer matches.
        let raw = std::fs::read(&path).unwrap();
        std::fs::write(&path, &raw[..raw.len() - 2]).unwrap();
        let got = rig.load(&rig.cache(), "a,b\n3,4\n").await.unwrap();
        assert_eq!(*got, "a,b\n3,4\n");
        assert_eq!(rig.downloads(), 2);

        // Garbage with no header at all.
        std::fs::write(&path, b"\x00\x01not a master").unwrap();
        rig.load(&rig.cache(), "a,b\n5,6\n").await.unwrap();
        assert_eq!(rig.downloads(), 3);

        // A valid envelope around a body that no longer parses.
        std::fs::write(&path, envelope(b"no header line")).unwrap();
        let got = rig.load(&rig.cache(), "a,b\n7,8\n").await.unwrap();
        assert_eq!(*got, "a,b\n7,8\n");
        assert_eq!(rig.downloads(), 4);
        // The repaired file is used again.
        rig.load(&rig.cache(), "never,used\n").await.unwrap();
        assert_eq!(rig.downloads(), 4);
    }

    #[tokio::test]
    async fn a_bad_download_is_an_error_and_is_not_cached() {
        let rig = Rig::new("bad-download");
        assert!(rig.load(&rig.cache(), "").await.is_err());
        assert!(rig.files().is_empty());
        let failed = rig
            .cache()
            .load(
                "kite",
                "NSE",
                || async { anyhow::bail!("HTTP 503") },
                csv_text,
            )
            .await;
        assert!(failed.is_err());
        assert!(rig.files().is_empty());
    }

    #[tokio::test]
    async fn brokers_and_exchanges_are_kept_apart() {
        let rig = Rig::new("keys");
        let cache = rig.cache();
        rig.load(&cache, "kite,list\n").await.unwrap();
        let other: Arc<String> = cache
            .load(
                "dhan",
                "NSE",
                || async { Ok(b"dhan,list\n".to_vec()) },
                csv_text,
            )
            .await
            .unwrap();
        assert_eq!(*other, "dhan,list\n");
        assert_eq!(
            rig.files(),
            ["dhan-NSE-2026-11-17.master", "kite-NSE-2026-11-17.master"]
        );
    }

    #[tokio::test]
    async fn off_always_downloads() {
        let cache = MasterCache::off();
        let n = Arc::new(AtomicUsize::new(0));
        for _ in 0..2 {
            let n = n.clone();
            cache
                .load(
                    "kite",
                    "NSE",
                    || async move {
                        n.fetch_add(1, Ordering::SeqCst);
                        Ok(b"a,b\n".to_vec())
                    },
                    csv_text,
                )
                .await
                .unwrap();
        }
        assert_eq!(n.load(Ordering::SeqCst), 2);
    }
}
