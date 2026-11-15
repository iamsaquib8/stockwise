//! Changes made to the local workspace by an assistant (MCP or `ask`), with
//! a way back. Before a change, every file it may touch is copied to
//! `undo/` in the data directory; after it, the change is listed in
//! `audit.json` with each file's new fingerprint. `stockwise undo` puts the
//! copies back, and refuses (unless forced) when a file has been edited
//! since, so a later change is never silently lost.
//!
//! No database: undo works offline and survives `cache clear`.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The audit log's file name in the data directory.
pub const LOG_FILE: &str = "audit.json";
/// The folder of file copies in the data directory.
/// Lock file name (`<name>.lock`) held while a change is copied, made and logged.
const WRITER_LOCK: &str = "audit.writer";
pub const UNDO_DIR: &str = "undo";
/// Changes kept; older ones (and their copies) are dropped.
const KEEP: usize = 200;

/// One file a change touched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    pub path: PathBuf,
    /// Copy of the file before the change, in `undo/`; `None` when the file
    /// did not exist (undo then deletes it).
    pub before: Option<String>,
    /// Fingerprint after the change (`None`: the file is absent).
    pub after: Option<String>,
}

/// One recorded change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Change {
    pub id: u64,
    /// RFC 3339 time of the change.
    pub at: String,
    /// `mcp`, `ask`, ...
    pub source: String,
    pub tool: String,
    pub summary: String,
    pub files: Vec<FileChange>,
    #[serde(default)]
    pub undone: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Log {
    pub changes: Vec<Change>,
}

/// FNV-1a of `bytes`, as hex: stable across builds, enough to tell whether
/// a file changed.
fn fingerprint(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{:016x}", h)
}

fn fingerprint_of(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(fingerprint(&b))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("Cannot read {}", path.display())),
    }
}

/// Where the log and the copies live.
#[derive(Debug, Clone)]
pub struct Journal {
    pub dir: PathBuf,
}

impl Journal {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    /// The journal of the data directory.
    pub fn default_location() -> Result<Self> {
        Ok(Self::new(crate::store::data_dir()?))
    }

    fn log_path(&self) -> PathBuf {
        self.dir.join(LOG_FILE)
    }

    fn undo_dir(&self) -> PathBuf {
        self.dir.join(UNDO_DIR)
    }

    /// The lock every journalled change and undo holds, so two StockWise
    /// processes never copy the same file before either has changed it.
    /// (Each data file's own lock cannot be held here: `change` takes it.)
    fn writer_lock(&self) -> Result<crate::store::Lock> {
        crate::store::lock(&self.dir.join(WRITER_LOCK))
    }

    /// Run `change`, which may write any of `paths`, and record it. Each
    /// existing file is copied first; when `change` fails, or the change
    /// cannot be recorded, the copies are put back and nothing is recorded.
    /// Returns `change`'s result and the change's id.
    pub fn record<R>(
        &self,
        source: &str,
        tool: &str,
        summary: &str,
        paths: &[PathBuf],
        change: impl FnOnce() -> Result<R>,
    ) -> Result<(R, u64)> {
        std::fs::create_dir_all(&self.dir)
            .with_context(|| format!("Cannot create {}", self.dir.display()))?;
        let _writer = self.writer_lock()?;
        let stamp = chrono::Utc::now();
        let token = format!("{}", stamp.timestamp_nanos_opt().unwrap_or_default());
        std::fs::create_dir_all(self.undo_dir())
            .with_context(|| format!("Cannot create {}", self.undo_dir().display()))?;
        let mut files = Vec::new();
        for (i, path) in paths.iter().enumerate() {
            let before = if path.exists() {
                let name = format!(
                    "{}-{}-{}",
                    token,
                    i,
                    path.file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "file".into())
                );
                std::fs::copy(path, self.undo_dir().join(&name))
                    .with_context(|| format!("Cannot keep a copy of {}", path.display()))?;
                Some(name)
            } else {
                None
            };
            files.push(FileChange {
                path: path.clone(),
                before,
                after: None,
            });
        }
        let out = match change() {
            Ok(out) => out,
            Err(e) => return Err(self.roll_back(&files, e)),
        };
        let logged = (|| -> Result<u64> {
            for f in &mut files {
                f.after = fingerprint_of(&f.path)?;
            }
            let undo_dir = self.undo_dir();
            crate::store::update_at(&self.log_path(), |log: &mut Log| {
                let id = log.changes.last().map_or(1, |c| c.id + 1);
                log.changes.push(Change {
                    id,
                    at: stamp.to_rfc3339(),
                    source: source.to_string(),
                    tool: tool.to_string(),
                    summary: summary.to_string(),
                    files: files.clone(),
                    undone: false,
                });
                if log.changes.len() > KEEP {
                    let old: Vec<Change> = log.changes.drain(..log.changes.len() - KEEP).collect();
                    for c in old {
                        for f in c.files {
                            if let Some(b) = f.before {
                                let _ = std::fs::remove_file(undo_dir.join(b));
                            }
                        }
                    }
                }
                Ok(id)
            })
        })();
        match logged {
            Ok(id) => Ok((out, id)),
            // A change that cannot be undone is not kept.
            Err(e) => Err(self.roll_back(
                &files,
                e.context("The change could not be recorded for undo, so it was reversed"),
            )),
        }
    }

    /// Put every file of a failed change back. Returns `cause`, with the
    /// files that could not be restored and where their copies are kept.
    fn roll_back(&self, files: &[FileChange], cause: anyhow::Error) -> anyhow::Error {
        let failed: Vec<String> = files
            .iter()
            .filter_map(|f| {
                self.restore(f).err().map(|e| match &f.before {
                    Some(copy) => format!(
                        "{} ({:#}); its earlier copy is {}",
                        f.path.display(),
                        e,
                        self.undo_dir().join(copy).display()
                    ),
                    None => format!("{} ({:#}); it did not exist before", f.path.display(), e),
                })
            })
            .collect();
        if failed.is_empty() {
            cause
        } else {
            cause.context(format!(
                "The change failed and these files could not be put back; restore them by hand: {}",
                failed.join("; ")
            ))
        }
    }

    fn restore(&self, f: &FileChange) -> Result<()> {
        match &f.before {
            Some(copy) => {
                if let Some(parent) = f.path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(self.undo_dir().join(copy), &f.path)
                    .with_context(|| format!("Cannot restore {}", f.path.display()))?;
            }
            None => match std::fs::remove_file(&f.path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(e).with_context(|| format!("Cannot remove {}", f.path.display()));
                }
            },
        }
        Ok(())
    }

    /// Recorded changes, newest first.
    pub fn list(&self) -> Result<Vec<Change>> {
        let log: Log = crate::store::load_from(&self.log_path())?;
        Ok(log.changes.into_iter().rev().collect())
    }

    /// Undo change `id` (the newest change not yet undone when `None`). A
    /// file edited since the change is not overwritten unless `force`.
    /// Changes are undone newest first: undoing an older one while a newer
    /// one on the same file stands would lose the newer one, so that is
    /// refused too.
    pub fn undo(&self, id: Option<u64>, force: bool) -> Result<Change> {
        let _writer = self.writer_lock()?;
        crate::store::update_at(&self.log_path(), |log: &mut Log| {
            let pos = match id {
                Some(id) => log
                    .changes
                    .iter()
                    .position(|c| c.id == id)
                    .with_context(|| {
                        format!("No change #{} in the log (see `stockwise undo --list`)", id)
                    })?,
                None => log
                    .changes
                    .iter()
                    .rposition(|c| !c.undone)
                    .context("Nothing to undo")?,
            };
            let change = log.changes[pos].clone();
            if change.undone {
                bail!("Change #{} was already undone", change.id);
            }
            if !force {
                for later in log.changes[pos + 1..].iter().filter(|c| !c.undone) {
                    if later
                        .files
                        .iter()
                        .any(|f| change.files.iter().any(|g| g.path == f.path))
                    {
                        bail!(
                            "Change #{} edited the same file later; undo it first (or use --force)",
                            later.id
                        );
                    }
                }
                for f in &change.files {
                    if fingerprint_of(&f.path)? != f.after {
                        bail!(
                            "{} was edited after change #{}; undoing would lose that edit (use --force to undo anyway)",
                            f.path.display(),
                            change.id
                        );
                    }
                }
            }
            for f in &change.files {
                self.restore(f)?;
            }
            log.changes[pos].undone = true;
            Ok(change)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "stockwise-audit-{}-{}-{}",
            name,
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_change_is_recorded_and_undone() {
        let dir = scratch("basic");
        let j = Journal::new(&dir);
        let file = dir.join("watchlist.json");
        std::fs::write(&file, "old").unwrap();
        let ((), id) = j
            .record(
                "mcp",
                "watchlist_add",
                "Added TCS",
                std::slice::from_ref(&file),
                || {
                    std::fs::write(&file, "new")?;
                    Ok(())
                },
            )
            .unwrap();
        assert_eq!(id, 1);
        assert_eq!(j.list().unwrap()[0].summary, "Added TCS");
        let undone = j.undo(None, false).unwrap();
        assert_eq!(undone.id, 1);
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "old");
        assert!(j.list().unwrap()[0].undone);
        assert!(j.undo(None, false).is_err(), "nothing left to undo");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_created_file_is_removed_and_a_failed_change_is_not_recorded() {
        let dir = scratch("create");
        let j = Journal::new(&dir);
        let file = dir.join("goals.json");
        j.record(
            "ask",
            "goal_save",
            "Saved goal",
            std::slice::from_ref(&file),
            || {
                std::fs::write(&file, "{}")?;
                Ok(())
            },
        )
        .unwrap();
        // A change that fails half-way leaves the file as it was.
        let err = j.record::<()>("ask", "goal_save", "x", std::slice::from_ref(&file), || {
            std::fs::write(&file, "broken")?;
            anyhow::bail!("validation failed")
        });
        assert!(err.is_err());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{}");
        assert_eq!(j.list().unwrap().len(), 1);
        j.undo(None, false).unwrap();
        assert!(!file.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_change_that_cannot_be_logged_is_reversed() {
        let dir = scratch("unlogged");
        let j = Journal::new(&dir);
        let file = dir.join("watchlist.json");
        std::fs::write(&file, "[\"TCS.NS\"]").unwrap();
        // An unreadable log: the change runs, then cannot be recorded.
        std::fs::write(dir.join(LOG_FILE), "not json").unwrap();
        let err = j
            .record::<()>(
                "mcp",
                "watchlist_add",
                "Added INFY",
                std::slice::from_ref(&file),
                || {
                    std::fs::write(&file, "[\"TCS.NS\",\"INFY.NS\"]")?;
                    Ok(())
                },
            )
            .unwrap_err();
        assert!(format!("{err:#}").contains("reversed"), "{err:#}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[\"TCS.NS\"]");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn edits_made_since_are_not_lost() {
        let dir = scratch("guard");
        let j = Journal::new(&dir);
        let file = dir.join("alerts.json");
        std::fs::write(&file, "a").unwrap();
        j.record(
            "mcp",
            "alert_add",
            "first",
            std::slice::from_ref(&file),
            || {
                std::fs::write(&file, "ab")?;
                Ok(())
            },
        )
        .unwrap();
        j.record(
            "mcp",
            "alert_add",
            "second",
            std::slice::from_ref(&file),
            || {
                std::fs::write(&file, "abc")?;
                Ok(())
            },
        )
        .unwrap();
        // The older change is behind a newer one on the same file.
        let e = j.undo(Some(1), false).unwrap_err().to_string();
        assert!(e.contains("undo it first"), "{e}");
        // An edit by hand after the newest change.
        std::fs::write(&file, "abcd").unwrap();
        let e = j.undo(None, false).unwrap_err().to_string();
        assert!(e.contains("was edited after change #2"), "{e}");
        // Forced: back to before the newest change.
        j.undo(None, true).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ab");
        j.undo(None, false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "a");
        std::fs::remove_dir_all(&dir).ok();
    }
}
