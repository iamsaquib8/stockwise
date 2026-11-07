//! Local JSON persistence shared by the portfolio, watchlist, alerts,
//! simulator history, wealth history, and broker config.

use anyhow::{Context, Result};
use serde::{Serialize, de::DeserializeOwned};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where the data directory is, without creating it: `STOCKWISE_HOME`, then
/// `home` in the config file, then `dirs::data_local_dir()/stockwise`.
pub fn data_dir_location() -> Result<PathBuf> {
    Ok(
        match crate::config::setting("STOCKWISE_HOME", |c| c.home.as_ref()) {
            Some(p) => crate::config::expand_home(&p),
            None => dirs::data_local_dir()
                .context("Cannot determine local data directory (set STOCKWISE_HOME)")?
                .join("stockwise"),
        },
    )
}

/// Directory that holds every StockWise data file (see
/// [`data_dir_location`]), created if missing.
pub fn data_dir() -> Result<PathBuf> {
    let dir = data_dir_location()?;
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("Cannot create data directory {}", dir.display()))?;
    Ok(dir)
}

/// Full path of `file` inside the data directory.
pub fn path(file: &str) -> Result<PathBuf> {
    Ok(data_dir()?.join(file))
}

/// Load `file` from the data directory, or `T::default()` if it does not exist.
pub fn load<T: DeserializeOwned + Default>(file: &str) -> Result<T> {
    load_from(&path(file)?)
}

/// Save `value` to `file` in the data directory. See [`save_to`].
pub fn save<T: Serialize>(file: &str, value: &T, private: bool) -> Result<()> {
    save_to(&path(file)?, value, private)
}

/// How long [`lock`] waits for another StockWise process before giving up.
const LOCK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// An exclusive lock on one data file, held until it is dropped.
pub struct Lock {
    _file: std::fs::File,
}

/// Take an exclusive lock for read-modify-write of `path`, on `<path>.lock`
/// (the data file itself is replaced by rename, so it can't carry the lock).
/// Waits for another process that holds it; after 30 seconds it fails with a
/// message instead of overwriting that process's change.
pub fn lock(path: &Path) -> Result<Lock> {
    lock_for(path, LOCK_TIMEOUT)
}

fn lock_for(path: &Path, timeout: std::time::Duration) -> Result<Lock> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    let mut name = path.as_os_str().to_owned();
    name.push(".lock");
    let lock_path = PathBuf::from(name);
    let file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)
        .with_context(|| format!("Cannot open {}", lock_path.display()))?;
    let start = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(Lock { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if start.elapsed() >= timeout {
                    anyhow::bail!(
                        "{} is being updated by another StockWise command (waited {} s). Try again in a moment.",
                        path.display(),
                        timeout.as_secs()
                    );
                }
                std::thread::sleep(std::time::Duration::from_millis(25));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(e).with_context(|| format!("Cannot lock {}", lock_path.display()));
            }
        }
    }
}

/// Read-modify-write `file` safely: take its lock, load the current copy,
/// apply `change`, save, unlock. Do slow work (network calls) before calling
/// this and apply only the result inside `change`, so the lock is short and
/// a change another command saved in the meantime is kept.
pub fn update<T, R>(file: &str, change: impl FnOnce(&mut T) -> Result<R>) -> Result<R>
where
    T: DeserializeOwned + Serialize + Default,
{
    update_at(&path(file)?, change)
}

/// [`update`] for a file given by path.
pub fn update_at<T, R>(path: &Path, change: impl FnOnce(&mut T) -> Result<R>) -> Result<R>
where
    T: DeserializeOwned + Serialize + Default,
{
    let _lock = lock(path)?;
    let mut value: T = load_from(path)?;
    let out = change(&mut value)?;
    save_to(path, &value, false)?;
    Ok(out)
}

pub fn load_from<T: DeserializeOwned + Default>(path: &Path) -> Result<T> {
    if !path.exists() {
        return Ok(T::default());
    }
    let data =
        std::fs::read_to_string(path).with_context(|| format!("Cannot read {}", path.display()))?;
    serde_json::from_str(&data).with_context(|| {
        format!(
            "{} is not valid JSON. Fix it, or move it aside to start fresh.",
            path.display()
        )
    })
}

/// Write `value` as pretty JSON without ever leaving a half-written file:
/// write a temp file next to the target, flush it to disk, then rename it over
/// the target. With `private`, the file is readable by the owner only (0600 on
/// Unix) — used for broker credentials.
pub fn save_to<T: Serialize>(path: &Path, value: &T, private: bool) -> Result<()> {
    let data = serde_json::to_string_pretty(value)?;
    // Named portfolios live in a subdirectory (`portfolios/NAME.json`).
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    // Per-process temp name so a concurrent writer (e.g. the daemon) can't
    // rename our half-written file or delete it from under us.
    let tmp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    // A leftover temp file would keep its old permissions on reopen.
    let _ = std::fs::remove_file(&tmp);

    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    if private {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = private;

    let mut f = opts
        .open(&tmp)
        .with_context(|| format!("Cannot write {}", tmp.display()))?;
    f.write_all(data.as_bytes())?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}

/// On Unix, tighten `path` to owner-only (0600) if group or others can read
/// it. Returns true if the permissions were changed.
#[cfg(unix)]
pub fn restrict_permissions(path: &Path) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = std::fs::metadata(path) else {
        return Ok(false);
    };
    let mode = meta.permissions().mode();
    if mode & 0o077 == 0 {
        return Ok(false);
    }
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("Cannot restrict permissions on {}", path.display()))?;
    Ok(true)
}

#[cfg(not(unix))]
pub fn restrict_permissions(_path: &Path) -> Result<bool> {
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
    struct Doc {
        items: Vec<String>,
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("stockwise-store-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_loads_default() {
        let dir = temp_dir("missing");
        let doc: Doc = load_from(&dir.join("nope.json")).unwrap();
        assert_eq!(doc, Doc::default());
    }

    #[test]
    fn save_then_load_round_trips_and_leaves_no_temp_file() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("doc.json");
        let doc = Doc {
            items: vec!["AAPL".into(), "M&M.NS".into()],
        };
        save_to(&path, &doc, false).unwrap();
        assert_eq!(load_from::<Doc>(&path).unwrap(), doc);
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn updates_from_threads_keep_every_change() {
        let dir = temp_dir("update");
        let path = dir.join("doc.json");
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        update_at(&path, |doc: &mut Doc| {
                            doc.items.push(format!("{t}-{i}"));
                            // Hold the lock a moment so the writers really overlap.
                            std::thread::sleep(std::time::Duration::from_millis(1));
                            Ok(())
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().unwrap();
        }
        let doc: Doc = load_from(&path).unwrap();
        let mut seen = doc.items.clone();
        seen.sort();
        seen.dedup();
        assert_eq!((doc.items.len(), seen.len()), (80, 80));
    }

    #[test]
    fn a_second_writer_waits_then_fails_with_a_clear_message() {
        let dir = temp_dir("lock-timeout");
        let path = dir.join("doc.json");
        let held = lock(&path).unwrap();
        let err = lock_for(&path, std::time::Duration::from_millis(100))
            .err()
            .expect("the lock is taken")
            .to_string();
        assert!(err.contains("another StockWise command"), "{err}");
        // Once the first writer is done the lock is free again.
        drop(held);
        assert!(lock_for(&path, std::time::Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn update_changes_the_file_as_it_is_now_and_keeps_it_on_error() {
        let dir = temp_dir("update-fn");
        let path = dir.join("doc.json");
        save_to(
            &path,
            &Doc {
                items: vec!["a".into()],
            },
            false,
        )
        .unwrap();
        // The slow part of a command ran on an older copy...
        let stale: Doc = load_from(&path).unwrap();
        // ...while another command saved a change.
        update_at(&path, |d: &mut Doc| {
            d.items.push("b".into());
            Ok(())
        })
        .unwrap();
        // Applying the result of the slow part to the current file keeps both.
        update_at(&path, |d: &mut Doc| {
            d.items.extend(stale.items.iter().map(|i| format!("{i}2")));
            Ok(())
        })
        .unwrap();
        assert_eq!(load_from::<Doc>(&path).unwrap().items, ["a", "b", "a2"]);
        // A failing change saves nothing and releases the lock.
        let err = update_at(&path, |d: &mut Doc| -> Result<()> {
            d.items.clear();
            anyhow::bail!("nope")
        });
        assert!(err.is_err());
        assert_eq!(load_from::<Doc>(&path).unwrap().items.len(), 3);
        assert!(lock_for(&path, std::time::Duration::from_millis(100)).is_ok());
    }

    #[test]
    fn corrupt_file_error_names_the_path() {
        let dir = temp_dir("corrupt");
        let path = dir.join("doc.json");
        std::fs::write(&path, "{ not json").unwrap();
        let err = load_from::<Doc>(&path).unwrap_err().to_string();
        assert!(err.contains("doc.json"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn private_save_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("private");
        let path = dir.join("secret.json");
        save_to(&path, &Doc::default(), true).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(restrict_permissions(&path).unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
