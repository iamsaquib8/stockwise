//! `stockwise backup export|restore`: a passphrase-encrypted, portable copy
//! of the user's own data (portfolios, goals, watchlist, alerts, notes,
//! practice history and `config.toml`).
//!
//! ## File format (`.swbak`, version 1)
//!
//! ```text
//! header (64 bytes, authenticated as associated data)
//!   magic        "SWBAK\0\r\n"       8 bytes
//!   version      u16 little-endian   = 1
//!   kdf          u8                  = 1 (Argon2id v0x13)
//!   cipher       u8                  = 1 (XChaCha20-Poly1305)
//!   m_cost_kib   u32 LE
//!   t_cost       u32 LE
//!   parallelism  u32 LE
//!   salt         16 bytes
//!   nonce        24 bytes
//! ciphertext + 16-byte Poly1305 tag
//! ```
//!
//! The plaintext is a deterministic archive: entries sorted by path, no
//! timestamps or owners, so the same files always give the same plaintext.
//!
//! ```text
//!   magic "SWARCHV1"
//!   u32 LE entry count
//!   per entry: u32 LE path length, path (UTF-8, `/`-separated),
//!              u64 LE data length, data
//! ```
//!
//! Paths are relative to the data directory, except `config/config.toml`,
//! which is the config file (it lives in the platform config directory).
//!
//! Left out on purpose: caches (`cache.db*`, `cache/`, `*_cache.json`),
//! lock and temp files, rotating backups (`portfolio.json.N`, `*.bak`),
//! daemon and worker state, and every broker file (`*_config.json`,
//! `gtt_seen_*.json`), because those hold session tokens. Notification
//! secrets (`telegram_token`, `smtp_password`, ...) are removed from the
//! copy of `config.toml`.

use anyhow::{Context, Result, bail};
use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use colored::Colorize;
use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Component, Path, PathBuf};

const MAGIC: &[u8; 8] = b"SWBAK\0\r\n";
const FORMAT_VERSION: u16 = 1;
const KDF_ARGON2ID: u8 = 1;
const CIPHER_XCHACHA20POLY1305: u8 = 1;
const SALT_LEN: usize = 16;
const NONCE_LEN: usize = 24;
const HEADER_LEN: usize = 8 + 2 + 1 + 1 + 4 + 4 + 4 + SALT_LEN + NONCE_LEN;
const TAG_LEN: usize = 16;

const ARCHIVE_MAGIC: &[u8; 8] = b"SWARCHV1";

/// Archive name of the config file.
pub const CONFIG_ENTRY: &str = "config/config.toml";

/// Environment variable read for the passphrase in scripts and cron jobs.
pub const PASSPHRASE_ENV: &str = "STOCKWISE_BACKUP_PASSPHRASE";

/// Shortest passphrase `backup export` accepts.
const MIN_PASSPHRASE: usize = 8;

/// Largest Argon2 memory cost a backup file may ask for (1 GiB), so a crafted
/// file cannot make `restore` allocate without bound.
const MAX_M_COST_KIB: u32 = 1024 * 1024;
const MAX_T_COST: u32 = 64;
const MAX_PARALLELISM: u32 = 16;

/// Top-level data files that hold the user's own state.
const DATA_FILES: &[&str] = &[
    "portfolio.json",
    "wealth_history.json",
    "goals.json",
    "watchlist.json",
    "alerts.json",
    "notes.json",
    "audit.json",
    "sim_history.json",
    "sim_profile.json",
    "sim_challenge.json",
    "sim_replay.json",
    "sim_replay_tape.json",
];

/// Argon2id cost parameters, stored in the header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub parallelism: u32,
}

impl Default for KdfParams {
    /// 64 MiB, 3 passes, one lane: about a second on a laptop.
    fn default() -> Self {
        Self {
            m_cost_kib: 64 * 1024,
            t_cost: 3,
            parallelism: 1,
        }
    }
}

/// Where the files of one installation live.
#[derive(Debug, Clone)]
pub struct Locations {
    pub data_dir: PathBuf,
    pub config_file: Option<PathBuf>,
}

impl Locations {
    /// The current installation: `STOCKWISE_HOME` (or the default data
    /// directory) and `STOCKWISE_CONFIG` (or the default config path).
    pub fn current() -> Result<Self> {
        Ok(Self {
            data_dir: crate::store::data_dir_location()?,
            config_file: crate::config::path(),
        })
    }

    /// Local path of an archive entry, or `None` if the name is not one a
    /// backup may contain.
    fn target(&self, name: &str) -> Option<PathBuf> {
        if name == CONFIG_ENTRY {
            return self.config_file.clone();
        }
        if !is_backed_up(name) {
            return None;
        }
        let rel = Path::new(name);
        if !rel.components().all(|c| matches!(c, Component::Normal(_))) {
            return None;
        }
        Some(self.data_dir.join(rel))
    }
}

/// Whether a data-directory path (relative, `/`-separated) belongs in a backup.
fn is_backed_up(name: &str) -> bool {
    if DATA_FILES.contains(&name) {
        return true;
    }
    // Named portfolios and their wealth history: `portfolios/NAME.json`,
    // `portfolios/NAME.wealth_history.json`. Rotating copies end in `.N`,
    // `.bak`, `.lock` or `.tmp`, so they never match.
    match name.strip_prefix("portfolios/") {
        Some(file) => {
            !file.is_empty()
                && !file.contains('/')
                && !file.starts_with('.')
                && file.ends_with(".json")
        }
        None => false,
    }
}

/// The files a backup holds, by archive name, read from disk.
pub fn collect(loc: &Locations) -> Result<BTreeMap<String, Vec<u8>>> {
    let mut files = BTreeMap::new();
    for name in DATA_FILES {
        let path = loc.data_dir.join(name);
        if path.is_file() {
            files.insert(name.to_string(), read(&path)?);
        }
    }
    let dir = loc.data_dir.join("portfolios");
    // No `portfolios/` directory means no named portfolios; any other error
    // stops the export rather than leave them out of the backup unnoticed.
    match std::fs::read_dir(&dir) {
        Ok(entries) => {
            for e in entries {
                let e = e.with_context(|| format!("Cannot list {}", dir.display()))?;
                let file = e.file_name().to_string_lossy().to_string();
                let name = format!("portfolios/{}", file);
                if !is_backed_up(&name) {
                    continue;
                }
                // Follow symlinks, as the portfolio loader does; a broken
                // link stops the export instead of being skipped.
                let meta = std::fs::metadata(e.path())
                    .with_context(|| format!("Cannot read {}", e.path().display()))?;
                if meta.is_file() {
                    files.insert(name, read(&e.path())?);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e).with_context(|| format!("Cannot list {}", dir.display())),
    }
    if let Some(cfg) = &loc.config_file
        && cfg.is_file()
    {
        let text = strip_secrets(&read(cfg)?)
            .with_context(|| format!("Cannot back up {}", cfg.display()))?;
        files.insert(CONFIG_ENTRY.to_string(), text);
    }
    Ok(files)
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))
}

/// `config.toml` without the lines that set a notification secret. The
/// result is parsed again. This fails, instead of copying a secret into the
/// backup, when a secret is still there (an inline table, say), when
/// removing the lines broke the file (a multi-line value), or when a file
/// that is not valid TOML names a secret key.
fn strip_secrets(config: &[u8]) -> Result<Vec<u8>> {
    let text = std::str::from_utf8(config).context("config.toml is not UTF-8 text")?;
    // A secret whose value does not end on its own line (a multi-line string
    // or array) cannot be removed line by line: the rest of the value could
    // survive as valid TOML, such as comment lines.
    let multi_line = text
        .split_inclusive('\n')
        .filter(|l| is_secret_line(l))
        .any(|l| {
            let value = l.split_once('=').map_or("", |(_, v)| v);
            toml::from_str::<toml::Table>(&format!("v = {}", value.trim())).is_err()
        });
    if multi_line {
        bail!(SECRET_FORM_ERROR);
    }
    let stripped: String = text
        .split_inclusive('\n')
        .filter(|l| !is_secret_line(l))
        .collect();
    let leaks = match toml::from_str::<toml::Table>(&stripped) {
        Ok(table) => has_secret(&table),
        // Removing a line broke the file (it was part of a multi-line value),
        // or the file was never valid TOML and names a secret key somewhere.
        Err(_) => stripped != text || SECRET_KEY_NAMES.iter().any(|k| text.contains(k)),
    };
    if leaks {
        bail!(SECRET_FORM_ERROR);
    }
    Ok(if stripped == text {
        config.to_vec()
    } else {
        stripped.into_bytes()
    })
}

const SECRET_FORM_ERROR: &str = "config.toml sets a notification secret in a form the backup \
     cannot leave out. Write it on one line as `key = \"value\"` under [notify], or move it \
     to its STOCKWISE_* environment variable, then export again.";

/// Secret `[notify]` keys, for the check of a file that is not valid TOML.
const SECRET_KEY_NAMES: &[&str] = &[
    "telegram_token",
    "slack_webhook",
    "discord_webhook",
    "smtp_password",
];

/// Whether a parsed config still sets a secret `[notify]` key.
fn has_secret(table: &toml::Table) -> bool {
    table
        .get("notify")
        .and_then(|n| n.as_table())
        .is_some_and(|n| n.keys().any(|k| crate::config::is_secret_notify_key(k)))
}

/// A line that sets a secret key: `telegram_token = ...`, `'telegram_token' = ...`
/// or the dotted `notify.telegram_token = ...`.
fn is_secret_line(line: &str) -> bool {
    let Some((key, _)) = line.trim_start().split_once('=') else {
        return false;
    };
    let parts: Vec<String> = key
        .split('.')
        .map(|p| p.trim().trim_matches(|c| c == '"' || c == '\'').to_string())
        .collect();
    match parts.as_slice() {
        [k] => crate::config::is_secret_notify_key(k),
        [n, k] if n == "notify" => crate::config::is_secret_notify_key(k),
        _ => false,
    }
}

/// The deterministic plaintext archive of `files`.
pub fn pack(files: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(ARCHIVE_MAGIC);
    out.extend_from_slice(&(files.len() as u32).to_le_bytes());
    for (name, data) in files {
        out.extend_from_slice(&(name.len() as u32).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&(data.len() as u64).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

/// Read back an archive written by [`pack`].
pub fn unpack(mut bytes: &[u8]) -> Result<BTreeMap<String, Vec<u8>>> {
    fn take<'a>(b: &mut &'a [u8], n: usize) -> Result<&'a [u8]> {
        if b.len() < n {
            bail!("The backup archive is truncated.");
        }
        let (head, rest) = b.split_at(n);
        *b = rest;
        Ok(head)
    }
    if take(&mut bytes, 8)? != ARCHIVE_MAGIC {
        bail!("The backup archive has an unknown layout.");
    }
    let count = u32::from_le_bytes(take(&mut bytes, 4)?.try_into()?);
    let mut files = BTreeMap::new();
    for _ in 0..count {
        let len = u32::from_le_bytes(take(&mut bytes, 4)?.try_into()?) as usize;
        let name = std::str::from_utf8(take(&mut bytes, len)?)
            .context("The backup archive has a file name that is not UTF-8.")?
            .to_string();
        let len = u64::from_le_bytes(take(&mut bytes, 8)?.try_into()?);
        let len = usize::try_from(len).context("The backup archive is too large.")?;
        let data = take(&mut bytes, len)?.to_vec();
        if files.insert(name.clone(), data).is_some() {
            bail!("The backup archive lists {} twice.", name);
        }
    }
    if !bytes.is_empty() {
        bail!("The backup archive has extra bytes at the end.");
    }
    Ok(files)
}

fn derive_key(passphrase: &str, salt: &[u8], p: KdfParams) -> Result<[u8; 32]> {
    let params = Params::new(p.m_cost_kib, p.t_cost, p.parallelism, Some(32))
        .map_err(|e| anyhow::anyhow!("Invalid Argon2 parameters: {}", e))?;
    let mut key = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(passphrase.as_bytes(), salt, &mut key)
        .map_err(|e| anyhow::anyhow!("Key derivation failed: {}", e))?;
    Ok(key)
}

fn header(p: KdfParams, salt: &[u8; SALT_LEN], nonce: &[u8; NONCE_LEN]) -> Vec<u8> {
    let mut h = Vec::with_capacity(HEADER_LEN);
    h.extend_from_slice(MAGIC);
    h.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    h.push(KDF_ARGON2ID);
    h.push(CIPHER_XCHACHA20POLY1305);
    h.extend_from_slice(&p.m_cost_kib.to_le_bytes());
    h.extend_from_slice(&p.t_cost.to_le_bytes());
    h.extend_from_slice(&p.parallelism.to_le_bytes());
    h.extend_from_slice(salt);
    h.extend_from_slice(nonce);
    h
}

/// Encrypt `plaintext` into a `.swbak` file body with a fresh salt and nonce.
pub fn seal(plaintext: &[u8], passphrase: &str, p: KdfParams) -> Result<Vec<u8>> {
    let mut salt = [0u8; SALT_LEN];
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut salt).map_err(|e| anyhow::anyhow!("No system randomness: {}", e))?;
    getrandom::fill(&mut nonce).map_err(|e| anyhow::anyhow!("No system randomness: {}", e))?;
    let key = derive_key(passphrase, &salt, p)?;
    let head = header(p, &salt, &nonce);
    let cipher = XChaCha20Poly1305::new((&key).into());
    let body = cipher
        .encrypt(
            XNonce::from_slice(&nonce),
            Payload {
                msg: plaintext,
                aad: &head,
            },
        )
        .map_err(|_| anyhow::anyhow!("Encryption failed"))?;
    let mut out = head;
    out.extend_from_slice(&body);
    Ok(out)
}

/// Check and decrypt a `.swbak` file body. Fails without returning any data
/// on a wrong passphrase, a changed byte anywhere, or a truncated file.
pub fn open(bytes: &[u8], passphrase: &str) -> Result<Vec<u8>> {
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        bail!("This is not a StockWise backup (.swbak) file.");
    }
    if bytes.len() < HEADER_LEN + TAG_LEN {
        bail!("The backup file is truncated.");
    }
    let u32_at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().expect("4 bytes"));
    let version = u16::from_le_bytes([bytes[8], bytes[9]]);
    if version != FORMAT_VERSION {
        bail!(
            "This backup uses format version {}; this StockWise reads version {}. Update StockWise.",
            version,
            FORMAT_VERSION
        );
    }
    if bytes[10] != KDF_ARGON2ID || bytes[11] != CIPHER_XCHACHA20POLY1305 {
        bail!("This backup uses an unknown key derivation or cipher.");
    }
    let p = KdfParams {
        m_cost_kib: u32_at(12),
        t_cost: u32_at(16),
        parallelism: u32_at(20),
    };
    if p.m_cost_kib > MAX_M_COST_KIB || p.t_cost > MAX_T_COST || p.parallelism > MAX_PARALLELISM {
        bail!("The backup header asks for unreasonable key-derivation costs; the file is damaged.");
    }
    let salt = &bytes[24..24 + SALT_LEN];
    let nonce = &bytes[24 + SALT_LEN..HEADER_LEN];
    let key = derive_key(passphrase, salt, p)
        .context("The backup header is damaged (invalid key-derivation parameters).")?;
    let cipher = XChaCha20Poly1305::new((&key).into());
    cipher
        .decrypt(
            XNonce::from_slice(nonce),
            Payload {
                msg: &bytes[HEADER_LEN..],
                aad: &bytes[..HEADER_LEN],
            },
        )
        .map_err(|_| {
            anyhow::anyhow!(
                "Cannot decrypt the backup: the passphrase is wrong, or the file was changed or damaged."
            )
        })
}

/// What restoring one file would do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Add,
    Replace,
    Same,
}

/// One file of a restore plan.
#[derive(Debug, Clone)]
pub struct Planned {
    pub name: String,
    pub target: PathBuf,
    pub change: Change,
}

/// Compare the backup's files with the local ones. Fails if the backup
/// holds a path outside the allowed set (or a config file with no place to go).
pub fn plan(loc: &Locations, files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<Planned>> {
    let mut out = Vec::new();
    for (name, data) in files {
        let Some(target) = loc.target(name) else {
            if name == CONFIG_ENTRY {
                bail!(
                    "The backup has a config.toml but no config path is known here (set STOCKWISE_CONFIG)."
                );
            }
            bail!(
                "The backup holds an unexpected path {:?}; refusing to restore it.",
                name
            );
        };
        let change = match std::fs::read(&target) {
            Ok(cur) if cur == *data => Change::Same,
            Ok(_) => Change::Replace,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Change::Add,
            Err(e) => return Err(e).with_context(|| format!("Cannot read {}", target.display())),
        };
        out.push(Planned {
            name: name.clone(),
            target,
            change,
        });
    }
    Ok(out)
}

/// Restore the planned files, all or nothing.
///
/// 1. Lock every target and read it again under the lock (the plan was made
///    without locks, so a file may have appeared or changed since).
/// 2. Copy each file that exists and differs from the backup to a free
///    `<file>.bak` name (`.bak`, then `.bak.1`, `.bak.2`, ... so an earlier
///    copy is never overwritten).
/// 3. Write each file to a temp file and rename it into place.
///
/// If a write fails, the files already written get their previous content
/// back (files the restore added are removed), so the data directory ends
/// as it started. The error keeps its cause.
pub fn apply(plan: &[Planned], files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    apply_with(plan, files, write_atomic)
}

/// [`apply`] with the function that writes one file (tests make it fail).
fn apply_with(
    plan: &[Planned],
    files: &BTreeMap<String, Vec<u8>>,
    write: impl Fn(&Path, &[u8]) -> Result<()>,
) -> Result<()> {
    let todo: Vec<&Planned> = plan.iter().filter(|p| p.change != Change::Same).collect();
    let mut _locks = Vec::with_capacity(todo.len());
    for p in &todo {
        _locks.push(crate::store::lock(&p.target)?);
    }
    // (target, new content, content before the restore)
    let mut writes: Vec<(&Path, &[u8], Option<Vec<u8>>)> = Vec::new();
    for p in &todo {
        let data = files[&p.name].as_slice();
        let current = match std::fs::read(&p.target) {
            Ok(cur) => Some(cur),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(e).with_context(|| format!("Cannot read {}", p.target.display()));
            }
        };
        if current.as_deref() == Some(data) {
            continue;
        }
        if current.is_some() {
            let bak = free_bak_path(&p.target);
            std::fs::copy(&p.target, &bak)
                .with_context(|| format!("Cannot back up {}", p.target.display()))?;
        }
        writes.push((&p.target, data, current));
    }
    for (i, (target, data, _)) in writes.iter().enumerate() {
        if let Err(e) = write(target, data) {
            let mut not_undone = Vec::new();
            for (t, _, before) in writes[..i].iter().rev() {
                let undo = match before {
                    Some(old) => write_atomic(t, old),
                    None => std::fs::remove_file(t).map_err(Into::into),
                };
                if undo.is_err() {
                    not_undone.push(t.display().to_string());
                }
            }
            let state = if not_undone.is_empty() {
                "No file was changed: the files written before it were put back as they were."
                    .to_string()
            } else {
                format!(
                    "These files could not be put back and hold the backup's version: {}. \
                     Their previous content is in the newest <file>.bak* copy.",
                    not_undone.join(", ")
                )
            };
            return Err(e).context(format!("Restore failed at {}. {}", target.display(), state));
        }
    }
    Ok(())
}

/// The `<file>.bak`, `<file>.bak.1`, ... copies of a config file that still
/// set a notification secret (copies of a config from before any restore).
fn bak_copies_with_secrets(config: &Path) -> Vec<PathBuf> {
    (0u32..)
        .map(|n| {
            let mut s = config.as_os_str().to_owned();
            s.push(if n == 0 {
                ".bak".to_string()
            } else {
                format!(".bak.{}", n)
            });
            PathBuf::from(s)
        })
        .take(1000)
        .filter(|p| p.exists())
        .filter(|p| std::fs::read_to_string(p).is_ok_and(|t| t.lines().any(is_secret_line)))
        .collect()
}

/// `<file>.bak`, or `<file>.bak.N` with the first N not in use.
fn free_bak_path(target: &Path) -> PathBuf {
    (0u32..)
        .map(|n| {
            let mut s = target.as_os_str().to_owned();
            s.push(if n == 0 {
                ".bak".to_string()
            } else {
                format!(".bak.{}", n)
            });
            PathBuf::from(s)
        })
        .find(|p| !p.exists())
        .expect("a free .bak name")
}

/// Write `data` to a temp file next to `path`, flush it, rename it over `path`.
///
/// The temp file is created owner-only (0600 on Unix), so nothing is
/// readable by others while it is written. When `path` exists, the temp file
/// gets its permissions before the rename, so a private file stays private;
/// a new file stays owner-only.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    write_file(path, data, true)
}

/// [`write_atomic`]; with `keep_mode` false the file is always owner-only.
fn write_file(path: &Path, data: &[u8], keep_mode: bool) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.restore.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(&tmp)
        .with_context(|| format!("Cannot write {}", tmp.display()))?;
    #[cfg(not(unix))]
    let _ = keep_mode;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Some(m) = std::fs::metadata(path).ok().filter(|_| keep_mode) {
            let mode = m.permissions().mode() & 0o7777;
            f.set_permissions(std::fs::Permissions::from_mode(mode))
                .with_context(|| format!("Cannot set permissions on {}", tmp.display()))?;
        }
    }
    f.write_all(data)?;
    f.sync_all()?;
    drop(f);
    std::fs::rename(&tmp, path).with_context(|| format!("Cannot replace {}", path.display()))?;
    Ok(())
}

/// Where the passphrase comes from.
#[derive(Debug, Clone, Copy)]
pub enum PassphraseSource {
    /// `--passphrase-fd N`: the first line read from file descriptor N.
    Fd(u32),
    /// `STOCKWISE_BACKUP_PASSPHRASE`, else a hidden prompt.
    EnvOrPrompt,
}

fn get_passphrase(src: PassphraseSource, confirm: bool) -> Result<String> {
    let pass = match src {
        PassphraseSource::Fd(fd) => read_fd(fd)?,
        PassphraseSource::EnvOrPrompt => match std::env::var(PASSPHRASE_ENV) {
            Ok(p) if !p.is_empty() => p,
            _ => {
                use std::io::IsTerminal;
                if !std::io::stdin().is_terminal() {
                    bail!(
                        "No passphrase: set {} or pass --passphrase-fd N when not running in a terminal.",
                        PASSPHRASE_ENV
                    );
                }
                let p = rpassword::prompt_password("  Backup passphrase: ")?;
                if confirm {
                    let again = rpassword::prompt_password("  Repeat passphrase: ")?;
                    if again != p {
                        bail!("The passphrases do not match.");
                    }
                }
                p
            }
        },
    };
    if pass.is_empty() {
        bail!("The passphrase is empty.");
    }
    Ok(pass)
}

#[cfg(unix)]
fn read_fd(fd: u32) -> Result<String> {
    let path = format!("/dev/fd/{}", fd);
    use std::io::Read;
    let mut s = String::new();
    std::fs::File::open(&path)
        .with_context(|| format!("Cannot read the passphrase from file descriptor {}", fd))?
        .read_to_string(&mut s)
        .with_context(|| format!("Cannot read the passphrase from file descriptor {}", fd))?;
    Ok(s.lines().next().unwrap_or("").to_string())
}

#[cfg(not(unix))]
fn read_fd(_fd: u32) -> Result<String> {
    bail!(
        "--passphrase-fd works on Unix only; use {} instead.",
        PASSPHRASE_ENV
    )
}

/// `stockwise backup export [PATH]`.
pub fn cmd_export(path: Option<PathBuf>, src: PassphraseSource) -> Result<()> {
    let loc = Locations::current()?;
    println!("\n  Data directory: {}", loc.data_dir.display());
    let files = collect(&loc)?;
    if files.is_empty() {
        bail!(
            "Nothing to back up: no StockWise data in {}.",
            loc.data_dir.display()
        );
    }
    let path = path.unwrap_or_else(|| {
        PathBuf::from(format!(
            "stockwise-{}.swbak",
            chrono::Local::now().format("%Y-%m-%d")
        ))
    });
    let pass = get_passphrase(src, true)?;
    if pass.chars().count() < MIN_PASSPHRASE {
        bail!(
            "Use a passphrase of at least {} characters.",
            MIN_PASSPHRASE
        );
    }
    let sealed = seal(&pack(&files), &pass, KdfParams::default())?;
    write_file(&path, &sealed, false)?;
    println!(
        "\n  {} Backed up {} file(s) to {}",
        "✓".green(),
        files.len(),
        path.display().to_string().cyan()
    );
    for name in files.keys() {
        println!("    {}", name);
    }
    println!(
        "\n  {}",
        "Keep the passphrase safe: without it the backup cannot be opened.".dimmed()
    );
    Ok(())
}

/// `stockwise backup restore PATH [--dry-run]`.
pub fn cmd_restore(path: &Path, dry_run: bool, src: PassphraseSource) -> Result<()> {
    let bytes = read(path)?;
    if bytes.len() < MAGIC.len() || &bytes[..MAGIC.len()] != MAGIC {
        bail!(
            "{} is not a StockWise backup (.swbak) file.",
            path.display()
        );
    }
    let pass = get_passphrase(src, false)?;
    let files = unpack(&open(&bytes, &pass)?)?;
    let loc = Locations::current()?;
    println!("\n  Data directory: {}", loc.data_dir.display());
    let plan = plan(&loc, &files)?;
    println!(
        "\n  Backup {} holds {} file(s):",
        path.display().to_string().cyan(),
        files.len()
    );
    for p in &plan {
        let tag = match p.change {
            Change::Add => "add    ".green(),
            Change::Replace => "replace".yellow(),
            Change::Same => "same   ".dimmed(),
        };
        println!("    {} {}", tag, p.target.display());
    }
    let changes = plan.iter().filter(|p| p.change != Change::Same).count();
    if dry_run {
        println!(
            "\n  {} file(s) would change. Dry run: nothing written.",
            changes
        );
        return Ok(());
    }
    if changes == 0 {
        println!("\n  {} Everything already matches the backup.", "✓".green());
        return Ok(());
    }
    apply(&plan, &files)?;
    println!(
        "\n  {} Restored {} file(s). Replaced files were copied to <file>.bak (or .bak.1, .bak.2, ... when an earlier copy exists).",
        "✓".green(),
        changes
    );
    if plan
        .iter()
        .any(|p| p.name == CONFIG_ENTRY && p.change != Change::Same)
    {
        let with_secrets = loc
            .config_file
            .as_deref()
            .map(bak_copies_with_secrets)
            .unwrap_or_default();
        let hint = if with_secrets.is_empty() {
            "Notification secrets are never in a backup, and no config.toml.bak copy holds any: \
             take them from the source computer's config.toml, or set them as STOCKWISE_* \
             environment variables."
                .to_string()
        } else {
            format!(
                "Notification secrets are never in a backup. They are in: {}",
                with_secrets
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        println!("  {}", hint.dimmed());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap KDF settings so tests run fast; real exports use the defaults.
    const FAST: KdfParams = KdfParams {
        m_cost_kib: 64,
        t_cost: 1,
        parallelism: 1,
    };

    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "stockwise-backup-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn locations(root: &Path) -> Locations {
        Locations {
            data_dir: root.join("home"),
            config_file: Some(root.join("cfg").join("config.toml")),
        }
    }

    fn put(path: &Path, data: &[u8]) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, data).unwrap();
    }

    /// A data directory with two named portfolios plus files that must stay out.
    fn populate(loc: &Locations) {
        let d = &loc.data_dir;
        put(&d.join("portfolio.json"), br#"{"version":2,"holdings":[]}"#);
        put(&d.join("portfolios/us.json"), b"{\"holdings\":[1]}\n");
        put(
            &d.join("portfolios/kids.json"),
            "{\"note\":\"₹\"}".as_bytes(),
        );
        put(&d.join("portfolios/kids.wealth_history.json"), b"[]");
        put(&d.join("goals.json"), b"{\"goals\":[]}");
        put(&d.join("watchlist.json"), b"{\"symbols\":[\"TCS.NS\"]}");
        put(&d.join("alerts.json"), b"{}");
        put(&d.join("notes.json"), b"{\"notes\":{}}");
        // Binary content survives too.
        put(&d.join("sim_history.json"), &[0u8, 255, 10, 13, 0]);
        // Must not be backed up.
        put(&d.join("portfolio.json.1"), b"old");
        put(&d.join("portfolio.json.bak"), b"old");
        put(&d.join("portfolio.json.lock"), b"");
        put(&d.join("portfolios/us.json.2"), b"old");
        put(&d.join("portfolios/us.json.lock"), b"");
        put(&d.join("cache.db"), b"sqlite");
        put(&d.join("cache.db-wal"), b"wal");
        put(&d.join("cache/x.json"), b"{}");
        put(
            &d.join("zerodha_config.json"),
            b"{\"access_token\":\"secret\"}",
        );
        put(&d.join("gtt_seen_zerodha.json"), b"{}");
        put(&d.join("worker_state.json"), b"{}");
        put(&d.join("calendar_cache.json"), b"{}");
        put(
            loc.config_file.as_ref().unwrap(),
            b"market = \"in\"\n\n[notify]\ntelegram_chat_id = \"42\"\n",
        );
    }

    #[test]
    fn collect_takes_user_state_only() {
        let root = tempdir("collect");
        let loc = locations(&root);
        populate(&loc);
        let names: Vec<_> = collect(&loc).unwrap().into_keys().collect();
        assert_eq!(
            names,
            [
                "alerts.json",
                "config/config.toml",
                "goals.json",
                "notes.json",
                "portfolio.json",
                "portfolios/kids.json",
                "portfolios/kids.wealth_history.json",
                "portfolios/us.json",
                "sim_history.json",
                "watchlist.json",
            ]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn archive_is_deterministic() {
        let root = tempdir("determ");
        let loc = locations(&root);
        populate(&loc);
        let a = pack(&collect(&loc).unwrap());
        let b = pack(&collect(&loc).unwrap());
        assert_eq!(a, b);
        assert_eq!(unpack(&a).unwrap(), collect(&loc).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn export_then_restore_round_trips_bit_identically() {
        let src_root = tempdir("src");
        let src = locations(&src_root);
        populate(&src);
        let sealed = seal(&pack(&collect(&src).unwrap()), "correct horse", FAST).unwrap();

        let dst_root = tempdir("dst");
        let dst = locations(&dst_root);
        let files = unpack(&open(&sealed, "correct horse").unwrap()).unwrap();
        let planned = plan(&dst, &files).unwrap();
        assert!(planned.iter().all(|p| p.change == Change::Add));
        apply(&planned, &files).unwrap();

        for name in files.keys() {
            let rel = if name == CONFIG_ENTRY {
                "cfg/config.toml".to_string()
            } else {
                format!("home/{}", name)
            };
            assert_eq!(
                std::fs::read(src_root.join(&rel)).unwrap(),
                std::fs::read(dst_root.join(&rel)).unwrap(),
                "{rel}"
            );
        }
        // Restoring again changes nothing.
        assert!(
            plan(&dst, &files)
                .unwrap()
                .iter()
                .all(|p| p.change == Change::Same)
        );
        assert!(!dst.data_dir.join("zerodha_config.json").exists());
        assert!(!dst.data_dir.join("cache.db").exists());
        std::fs::remove_dir_all(src_root).unwrap();
        std::fs::remove_dir_all(dst_root).unwrap();
    }

    #[test]
    fn restore_keeps_a_bak_of_replaced_files() {
        let root = tempdir("bak");
        let loc = locations(&root);
        populate(&loc);
        let files = collect(&loc).unwrap();
        put(&loc.data_dir.join("goals.json"), b"changed");
        let planned = plan(&loc, &files).unwrap();
        let goals = planned.iter().find(|p| p.name == "goals.json").unwrap();
        assert_eq!(goals.change, Change::Replace);
        apply(&planned, &files).unwrap();
        assert_eq!(
            std::fs::read(loc.data_dir.join("goals.json")).unwrap(),
            b"{\"goals\":[]}"
        );
        assert_eq!(
            std::fs::read(loc.data_dir.join("goals.json.bak")).unwrap(),
            b"changed"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wrong_passphrase_fails_cleanly() {
        let sealed = seal(b"secret data", "right passphrase", FAST).unwrap();
        let e = open(&sealed, "wrong passphrase").unwrap_err().to_string();
        assert!(e.contains("passphrase is wrong"), "{e}");
    }

    #[test]
    fn any_flipped_byte_fails_cleanly() {
        let sealed = seal(b"secret data", "pass phrase", FAST).unwrap();
        assert_eq!(open(&sealed, "pass phrase").unwrap(), b"secret data");
        for i in 0..sealed.len() {
            let mut bad = sealed.clone();
            bad[i] ^= 0x01;
            assert!(open(&bad, "pass phrase").is_err(), "byte {i} accepted");
        }
        assert!(open(&sealed[..sealed.len() - 1], "pass phrase").is_err());
        assert!(open(b"not a backup", "pass phrase").is_err());
    }

    #[test]
    fn salt_and_nonce_are_fresh_per_export() {
        let a = seal(b"x", "pass phrase", FAST).unwrap();
        let b = seal(b"x", "pass phrase", FAST).unwrap();
        assert_ne!(a, b);
        assert_eq!(a[..24], b[..24], "same header fields before the salt");
    }

    #[test]
    fn restore_rejects_paths_outside_the_allowed_set() {
        let root = tempdir("evil");
        let loc = locations(&root);
        for evil in [
            "../escape.json",
            "/etc/passwd",
            "portfolios/../../x.json",
            "zerodha_config.json",
            "cache.db",
        ] {
            let mut files = BTreeMap::new();
            files.insert(evil.to_string(), b"x".to_vec());
            assert!(plan(&loc, &files).is_err(), "{evil}");
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn config_secrets_are_left_out() {
        let cfg = b"market = \"in\"\n[notify]\ntelegram_token = \"123:abc\"\nsmtp_password=\"pw\"\n'slack_webhook' = \"x\"\ntelegram_chat_id = \"42\"\n";
        let out = String::from_utf8(strip_secrets(cfg).unwrap()).unwrap();
        assert_eq!(
            out,
            "market = \"in\"\n[notify]\ntelegram_chat_id = \"42\"\n"
        );
        let dotted = b"market = \"in\"\nnotify.telegram_token = \"t\"\n";
        assert_eq!(strip_secrets(dotted).unwrap(), b"market = \"in\"\n");
        let clean = b"market = \"in\"\n";
        assert_eq!(strip_secrets(clean).unwrap(), clean);
        // A one-line triple-quoted value is still removed.
        let one_line = b"[notify]\nsmtp_password = \"\"\"pw\"\"\" # comment\n";
        assert_eq!(strip_secrets(one_line).unwrap(), b"[notify]\n");
    }

    #[test]
    fn config_secrets_that_cannot_be_stripped_stop_the_export() {
        for cfg in [
            "notify = { telegram_token = \"t\", telegram_chat_id = \"1\" }\n",
            "[notify]\nsmtp_password = \"\"\"\nsecret\n\"\"\"\n",
            "[notify\ntelegram_token = \"t\"\n",
            "[notify]\nsmtp_password = \"\"\"\n# sensitive-password\n# \"\"\"\n",
            "[notify]\nsmtp_password = '''\npw\n'''\n",
        ] {
            let e = strip_secrets(cfg.as_bytes()).unwrap_err().to_string();
            assert!(e.contains("cannot leave out"), "{cfg}: {e}");
        }
    }

    #[test]
    fn a_second_restore_keeps_the_first_bak() {
        let root = tempdir("bak2");
        let loc = locations(&root);
        populate(&loc);
        let files = collect(&loc).unwrap();
        let goals = loc.data_dir.join("goals.json");
        for (i, local) in [b"first".as_slice(), b"second"].into_iter().enumerate() {
            put(&goals, local);
            apply(&plan(&loc, &files).unwrap(), &files).unwrap();
            let bak = if i == 0 {
                "goals.json.bak"
            } else {
                "goals.json.bak.1"
            };
            assert_eq!(std::fs::read(loc.data_dir.join(bak)).unwrap(), local);
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failed_restore_puts_everything_back() {
        let root = tempdir("partial");
        let loc = locations(&root);
        put(&loc.data_dir.join("alerts.json"), b"old alerts");
        let mut files = BTreeMap::new();
        files.insert("alerts.json".to_string(), b"{}".to_vec());
        files.insert("goals.json".to_string(), b"{}".to_vec());
        files.insert("notes.json".to_string(), b"{}".to_vec());
        files.insert("watchlist.json".to_string(), b"{}".to_vec());
        let planned = plan(&loc, &files).unwrap();
        // Writing watchlist.json fails after alerts.json was replaced and
        // goals.json and notes.json were added.
        let failing = |path: &Path, data: &[u8]| {
            if path.ends_with("watchlist.json") {
                anyhow::bail!("disk full");
            }
            write_atomic(path, data)
        };
        let e = format!("{:#}", apply_with(&planned, &files, failing).unwrap_err());
        assert!(e.contains("disk full"), "{e}");
        assert!(e.contains("Restore failed at"), "{e}");
        assert!(e.contains("No file was changed"), "{e}");
        assert_eq!(
            std::fs::read(loc.data_dir.join("alerts.json")).unwrap(),
            b"old alerts"
        );
        assert!(!loc.data_dir.join("goals.json").exists());
        assert!(!loc.data_dir.join("notes.json").exists());
        assert!(!loc.data_dir.join("watchlist.json").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_named_portfolios_are_backed_up() {
        let root = tempdir("symlink");
        let loc = locations(&root);
        populate(&loc);
        let real = root.join("elsewhere.json");
        put(&real, b"{\"linked\":true}");
        std::os::unix::fs::symlink(&real, loc.data_dir.join("portfolios/linked.json")).unwrap();
        let files = collect(&loc).unwrap();
        assert_eq!(files["portfolios/linked.json"], b"{\"linked\":true}");
        // A broken link stops the export.
        std::os::unix::fs::symlink(
            root.join("gone.json"),
            loc.data_dir.join("portfolios/gone.json"),
        )
        .unwrap();
        assert!(collect(&loc).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn restore_keeps_a_private_file_private() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempdir("mode");
        let loc = locations(&root);
        let cfg = loc.config_file.clone().unwrap();
        put(&cfg, b"market = \"us\"\n");
        std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600)).unwrap();
        let goals = loc.data_dir.join("goals.json");
        put(&goals, b"old");
        std::fs::set_permissions(&goals, std::fs::Permissions::from_mode(0o644)).unwrap();
        let mut files = BTreeMap::new();
        files.insert(CONFIG_ENTRY.to_string(), b"market = \"in\"\n".to_vec());
        files.insert("goals.json".to_string(), b"{}".to_vec());
        files.insert("notes.json".to_string(), b"{}".to_vec());
        apply(&plan(&loc, &files).unwrap(), &files).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&cfg), 0o600);
        assert_eq!(mode(&goals), 0o644);
        assert_eq!(mode(&loc.data_dir.join("notes.json")), 0o600);
        let archive = root.join("b.swbak");
        put(&archive, b"old");
        std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_file(&archive, b"new", false).unwrap();
        assert_eq!(mode(&archive), 0o600);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_secrets_hint_names_the_copies_that_hold_secrets() {
        let root = tempdir("hint");
        let cfg = root.join("config.toml");
        put(
            &root.join("config.toml.bak"),
            b"[notify]\nsmtp_password = \"pw\"\n",
        );
        put(&root.join("config.toml.bak.1"), b"[notify]\n");
        assert_eq!(
            bak_copies_with_secrets(&cfg),
            [root.join("config.toml.bak")]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_file_that_appears_after_the_plan_is_kept_as_bak() {
        let root = tempdir("late");
        let loc = locations(&root);
        let mut files = BTreeMap::new();
        files.insert("goals.json".to_string(), b"{}".to_vec());
        let planned = plan(&loc, &files).unwrap();
        assert_eq!(planned[0].change, Change::Add);
        put(&loc.data_dir.join("goals.json"), b"saved meanwhile");
        apply(&planned, &files).unwrap();
        assert_eq!(
            std::fs::read(loc.data_dir.join("goals.json.bak")).unwrap(),
            b"saved meanwhile"
        );
        assert_eq!(
            std::fs::read(loc.data_dir.join("goals.json")).unwrap(),
            b"{}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_portfolios_directory_stops_the_export() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempdir("unreadable");
        let loc = locations(&root);
        populate(&loc);
        let dir = loc.data_dir.join("portfolios");
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root can read it anyway; then there is nothing to check.
        let readable = std::fs::read_dir(&dir).is_ok();
        let result = collect(&loc);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        if !readable {
            assert!(result.unwrap_err().to_string().contains("Cannot list"));
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn unpack_rejects_damaged_archives() {
        let mut files = BTreeMap::new();
        files.insert("goals.json".to_string(), b"{}".to_vec());
        let packed = pack(&files);
        assert!(unpack(&packed[..packed.len() - 1]).is_err());
        let mut extra = packed.clone();
        extra.push(0);
        assert!(unpack(&extra).is_err());
    }
}
