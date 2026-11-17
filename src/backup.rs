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
use std::io::{Read, Write};
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
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let file = e.file_name().to_string_lossy().to_string();
            let name = format!("portfolios/{}", file);
            if is_backed_up(&name) && e.path().is_file() {
                files.insert(name, read(&e.path())?);
            }
        }
    }
    if let Some(cfg) = &loc.config_file
        && cfg.is_file()
    {
        files.insert(CONFIG_ENTRY.to_string(), strip_secrets(&read(cfg)?));
    }
    Ok(files)
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("Cannot read {}", path.display()))
}

/// `config.toml` without lines that set a notification secret.
fn strip_secrets(config: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(config) else {
        return config.to_vec();
    };
    if !text.lines().any(is_secret_line) {
        return config.to_vec();
    }
    text.split_inclusive('\n')
        .filter(|l| !is_secret_line(l))
        .collect::<String>()
        .into_bytes()
}

fn is_secret_line(line: &str) -> bool {
    let line = line.trim_start();
    let Some((key, _)) = line.split_once('=') else {
        return false;
    };
    let key = key.trim().trim_matches('"');
    crate::config::is_secret_notify_key(key)
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

/// Write the planned files: each replaced file is copied to `<file>.bak`
/// first, then every file is written to a temp file and renamed into place.
pub fn apply(plan: &[Planned], files: &BTreeMap<String, Vec<u8>>) -> Result<()> {
    for p in plan.iter().filter(|p| p.change != Change::Same) {
        let data = &files[&p.name];
        let _lock = crate::store::lock(&p.target)?;
        if p.change == Change::Replace {
            let mut bak = p.target.as_os_str().to_owned();
            bak.push(".bak");
            std::fs::copy(&p.target, &bak)
                .with_context(|| format!("Cannot back up {}", p.target.display()))?;
        }
        write_atomic(&p.target, data)?;
    }
    Ok(())
}

/// Write `data` to a temp file next to `path`, flush it, rename it over `path`.
fn write_atomic(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".{}.restore.tmp", std::process::id()));
    let tmp = PathBuf::from(tmp);
    let _ = std::fs::remove_file(&tmp);
    let mut f =
        std::fs::File::create(&tmp).with_context(|| format!("Cannot write {}", tmp.display()))?;
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
    write_atomic(&path, &sealed)?;
    let _ = crate::store::restrict_permissions(&path);
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
        "\n  {} Restored {} file(s). Replaced files were kept as <file>.bak.",
        "✓".green(),
        changes
    );
    if plan
        .iter()
        .any(|p| p.name == CONFIG_ENTRY && p.change == Change::Replace)
    {
        println!(
            "  {}",
            "Notification secrets are never in a backup; copy them back from config.toml.bak if you need them."
                .dimmed()
        );
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
        let cfg = b"market = \"in\"\n[notify]\ntelegram_token = \"123:abc\"\nsmtp_password=\"pw\"\ntelegram_chat_id = \"42\"\n";
        let out = String::from_utf8(strip_secrets(cfg)).unwrap();
        assert_eq!(
            out,
            "market = \"in\"\n[notify]\ntelegram_chat_id = \"42\"\n"
        );
        let clean = b"market = \"in\"\n";
        assert_eq!(strip_secrets(clean), clean);
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
