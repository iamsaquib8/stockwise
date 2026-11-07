//! Disk cache for pages read from public websites (CCIL, RBI, NSE): one
//! file per page under `cache/` in the data directory.
//!
//! A fresh copy is used without a request; an old one is refreshed; and
//! when the site cannot be reached or refuses, a stale copy is still shown
//! with a warning and its date — never invented data.

use crate::store;
use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// A cached page and when it was downloaded.
#[derive(Debug, Clone)]
pub struct Page {
    pub text: String,
    pub fetched: DateTime<Utc>,
    /// Older than the caller's maximum age (the site could not be reached).
    pub stale: bool,
}

fn path(name: &str) -> Result<PathBuf> {
    let dir = store::data_dir()?.join("cache");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Cannot create cache directory {}", dir.display()))?;
    Ok(dir.join(name))
}

/// The cached copy of `name`, however old, if it passes `validate`.
pub fn read(name: &str, max_age: Duration, validate: impl Fn(&str) -> bool) -> Option<Page> {
    let path = path(name).ok()?;
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok()?;
    let text = std::fs::read_to_string(&path).ok()?;
    if !validate(&text) {
        return None;
    }
    let age = SystemTime::now()
        .duration_since(modified)
        .unwrap_or_default();
    Some(Page {
        text,
        fetched: modified.into(),
        stale: age >= max_age,
    })
}

/// Save `text` as `name` (temp file + rename). Best-effort: a read-only data
/// directory still works, just without a cache.
pub fn write(name: &str, text: &str) {
    let Ok(path) = path(name) else { return };
    let _ = write_atomic(&path, text);
}

/// Write `text` to `path` through a temp file next to it, then rename it over
/// the target, so readers never see half a file. The temp name holds the
/// process id and a counter: two tasks (or threads) writing the same file at
/// once each get their own temp file, and the last rename wins whole. A temp
/// file is removed again when the write or the rename fails.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tmp = path.with_extension(format!(
        "{}.{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let result = std::fs::write(&tmp, text).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// HTTP client for public data pages: a short timeout, a StockWise user agent.
pub fn client(timeout: Duration) -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!(
            "stockwise/",
            env!("CARGO_PKG_VERSION"),
            " (+https://github.com/iamsaquib8/stockwise)"
        ))
        .connect_timeout(Duration::from_secs(10).min(timeout))
        .timeout(timeout)
        .build()
        .context("Failed to build HTTP client")
}

/// GET `url` as text: one retry for network errors and 5xx, none for
/// refusals (403/429), so a blocked site is not hammered.
async fn get(client: &reqwest::Client, url: &str, source: &str) -> Result<String> {
    let mut last = None;
    for attempt in 0..2u64 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(800)).await;
        }
        match client.get(url).send().await {
            Ok(resp) if resp.status().is_success() => {
                return resp
                    .text()
                    .await
                    .with_context(|| format!("Failed to read the {source} response"));
            }
            Ok(resp) if resp.status().is_server_error() => {
                last = Some(anyhow::anyhow!("{source} returned HTTP {}", resp.status()));
            }
            Ok(resp) => bail!("{source} refused the request (HTTP {})", resp.status()),
            Err(e) => last = Some(anyhow::anyhow!("Cannot reach {source} ({e})")),
        }
    }
    Err(last.unwrap_or_else(|| anyhow::anyhow!("Cannot reach {source}")))
}

/// `url` from the cache when younger than `max_age`, else downloaded (and
/// cached). If the download fails, a stale copy is returned with a warning
/// on stderr; with no copy at all the error is returned.
pub async fn fetch(
    client: &reqwest::Client,
    name: &str,
    url: &str,
    source: &str,
    max_age: Duration,
    validate: impl Fn(&str) -> bool,
) -> Result<Page> {
    let cached = read(name, max_age, &validate);
    if let Some(page) = &cached
        && !page.stale
    {
        return Ok(page.clone());
    }
    match get(client, url, source).await {
        Ok(text) if validate(&text) => {
            write(name, &text);
            Ok(Page {
                text,
                fetched: Utc::now(),
                stale: false,
            })
        }
        Ok(_) => fall_back(
            cached,
            anyhow::anyhow!("{source} returned an unexpected page"),
        ),
        Err(e) => fall_back(cached, e),
    }
}

/// Use a stale copy (saying so on stderr), or return the error.
pub fn fall_back(cached: Option<Page>, err: anyhow::Error) -> Result<Page> {
    match cached {
        Some(page) => {
            eprintln!(
                "  Warning: {err:#}; showing the copy saved {}",
                page.fetched.format("%d %b %Y %H:%M UTC")
            );
            Ok(Page {
                stale: true,
                ..page
            })
        }
        None => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("sw-pagecache-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn leftovers(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".tmp"))
            .collect()
    }

    #[test]
    fn write_atomic_replaces_the_file_and_leaves_no_temp_file() {
        let dir = temp_dir("replace");
        let path = dir.join("page.txt");
        write_atomic(&path, "old").unwrap();
        write_atomic(&path, "new").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "new");
        assert!(leftovers(&dir).is_empty());
    }

    #[test]
    fn write_atomic_removes_its_temp_file_when_the_rename_fails() {
        let dir = temp_dir("fails");
        // A directory cannot be replaced by a file, so the rename fails.
        let target = dir.join("page.txt");
        std::fs::create_dir(&target).unwrap();
        assert!(write_atomic(&target, "text").is_err());
        assert!(leftovers(&dir).is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_writes_to_one_file_never_collide() {
        let dir = temp_dir("concurrent");
        let path = dir.join("page.txt");
        let texts: Vec<String> = (0..16).map(|i| format!("{i}:").repeat(20_000)).collect();
        let tasks: Vec<_> = texts
            .iter()
            .cloned()
            .map(|text| {
                let path = path.clone();
                tokio::task::spawn_blocking(move || {
                    for _ in 0..10 {
                        write_atomic(&path, &text).expect("write succeeds");
                    }
                })
            })
            .collect();
        for t in tasks {
            t.await.unwrap();
        }
        // The file holds one writer's text, whole.
        let got = std::fs::read_to_string(&path).unwrap();
        assert!(texts.contains(&got), "file is a mix of writes");
        assert!(leftovers(&dir).is_empty());
    }
}
