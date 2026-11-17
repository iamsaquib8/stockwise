//! `stockwise update`: find out how this binary was installed, check GitHub
//! Releases for a newer version and, for a standalone install (install.sh or
//! a release archive), replace the binary in place after verifying its
//! SHA-256.
//!
//! Binaries owned by a package manager (Homebrew, Scoop, Nix, Cargo, npm,
//! pipx/uv/pip, winget, `.deb`/`.rpm`, Docker) are never touched: the command
//! prints that channel's upgrade command instead.
//!
//! Only `stockwise update` and `stockwise doctor` call [`latest_release`]. No
//! other command checks for updates (offline first, no telemetry).
//!
//! Everything that decides something is a pure function over its inputs, and
//! the network goes through [`Fetcher`], so the tests never reach GitHub.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::cmp::Ordering;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const REPO: &str = "iamsaquib8/stockwise";
pub const BIN: &str = "stockwise";
const LATEST_RELEASE_API: &str =
    "https://api.github.com/repos/iamsaquib8/stockwise/releases/latest";

/// How long a `doctor` update check stays fresh.
pub const CHECK_TTL_SECS: u64 = 7 * 24 * 60 * 60;
/// File in the data directory that holds the last update check.
const CHECK_CACHE_FILE: &str = "update-check.json";

// ── Install channel ────────────────────────────────────────────────────────

/// Who owns the running binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Homebrew,
    Scoop,
    Nix,
    Cargo,
    Npm,
    Pipx,
    Uv,
    Pip,
    Winget,
    /// `/usr/bin`: installed by the `.deb` or `.rpm` package.
    SystemPackage,
    Docker,
    /// install.sh or a release archive: `stockwise update` replaces it.
    Standalone,
}

impl Channel {
    pub fn name(self) -> &'static str {
        match self {
            Channel::Homebrew => "Homebrew",
            Channel::Scoop => "Scoop",
            Channel::Nix => "Nix",
            Channel::Cargo => "Cargo",
            Channel::Npm => "npm",
            Channel::Pipx => "pipx",
            Channel::Uv => "uv",
            Channel::Pip => "pip (Python environment)",
            Channel::Winget => "winget",
            Channel::SystemPackage => ".deb / .rpm package",
            Channel::Docker => "Docker",
            Channel::Standalone => "standalone (install.sh or release archive)",
        }
    }

    /// The command that upgrades this channel's install, or `None` for a
    /// standalone binary, which `stockwise update` replaces itself.
    pub fn upgrade_commands(self) -> Option<&'static [&'static str]> {
        Some(match self {
            Channel::Homebrew => &["brew upgrade stockwise"],
            Channel::Scoop => &["scoop update stockwise"],
            Channel::Nix => &["nix profile upgrade stockwise"],
            Channel::Cargo => &[
                "cargo binstall stockwise",
                "cargo install --locked stockwise   # to build from source",
            ],
            Channel::Npm => &["npm update -g stockwise"],
            Channel::Pipx => &["pipx upgrade stockwise"],
            Channel::Uv => &["uv tool upgrade stockwise"],
            Channel::Pip => &["python -m pip install --upgrade stockwise"],
            Channel::Winget => &["winget upgrade iamsaquib8.StockWise"],
            Channel::SystemPackage => &[
                "Debian/Ubuntu: curl -fsSLO https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise_amd64.deb && sudo apt install ./stockwise_amd64.deb",
                "Fedora/RHEL:   sudo dnf install https://github.com/iamsaquib8/stockwise/releases/latest/download/stockwise-x86_64.rpm",
                "(on ARM use stockwise_arm64.deb or stockwise-aarch64.rpm)",
            ],
            Channel::Docker => &["docker pull ghcr.io/iamsaquib8/stockwise"],
            Channel::Standalone => return None,
        })
    }
}

/// What [`detect_channel`] looks at besides the executable path.
#[derive(Clone, Debug, Default)]
pub struct ChannelHints {
    /// `$CARGO_HOME`, if set.
    pub cargo_home: Option<PathBuf>,
    /// The user's home directory (for `~/.cargo/bin`).
    pub home: Option<PathBuf>,
    /// `/.dockerenv` or `/run/.containerenv` exists.
    pub in_container: bool,
    /// The directory above the binary's directory holds `pyvenv.cfg`.
    pub in_python_venv: bool,
}

impl ChannelHints {
    /// Hints for the running process, read from the environment and disk.
    pub fn current(exe: &Path) -> Self {
        let in_python_venv = exe
            .parent()
            .and_then(Path::parent)
            .is_some_and(|venv| venv.join("pyvenv.cfg").is_file());
        Self {
            cargo_home: std::env::var_os("CARGO_HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from),
            home: dirs::home_dir(),
            in_container: Path::new("/.dockerenv").exists()
                || Path::new("/run/.containerenv").exists(),
            in_python_venv,
        }
    }
}

/// `path` as a forward-slash string, for marker matching on every OS.
fn slashed(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// Which channel owns the binary at `exe` (already canonicalised, so
/// symlinks such as Homebrew's or pipx's point into the real location).
pub fn detect_channel(exe: &Path, hints: &ChannelHints) -> Channel {
    let p = slashed(exe);
    let lower = p.to_lowercase();
    let dir = slashed(exe.parent().unwrap_or(Path::new("")));

    if hints.in_container {
        return Channel::Docker;
    }
    if p.starts_with("/nix/store/") {
        return Channel::Nix;
    }
    if p.starts_with("/opt/homebrew/")
        || p.starts_with("/usr/local/Cellar/")
        || p.starts_with("/home/linuxbrew/")
    {
        return Channel::Homebrew;
    }
    if lower.contains("/scoop/apps/stockwise/") {
        return Channel::Scoop;
    }
    if lower.contains("/winget/") {
        return Channel::Winget;
    }
    let cargo_bins = [
        hints.cargo_home.as_ref().map(|h| h.join("bin")),
        hints.home.as_ref().map(|h| h.join(".cargo").join("bin")),
    ];
    if cargo_bins.iter().flatten().any(|bin| slashed(bin) == dir) {
        return Channel::Cargo;
    }
    if p.contains("/node_modules/") {
        return Channel::Npm;
    }
    if p.contains("/pipx/venvs/") {
        return Channel::Pipx;
    }
    if p.contains("/uv/tools/") {
        return Channel::Uv;
    }
    if p.contains("/site-packages/") || hints.in_python_venv {
        return Channel::Pip;
    }
    if dir == "/usr/bin" {
        return Channel::SystemPackage;
    }
    Channel::Standalone
}

// ── Versions ───────────────────────────────────────────────────────────────

/// A release version: `v0.9.0`, `0.9.0` or `0.10.0-rc.1`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// Text after `-`, if any. A pre-release sorts before its release.
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        let s = s.split('+').next()?; // build metadata never orders
        let (core, pre) = match s.split_once('-') {
            Some((c, p)) if !p.is_empty() => (c, Some(p.to_string())),
            Some(_) => return None,
            None => (s, None),
        };
        let mut parts = core.split('.');
        let mut next = || -> Option<u64> { parts.next()?.parse().ok() };
        let (major, minor, patch) = (next()?, next()?, next()?);
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => compare_pre(a, b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Semver pre-release order: dot-separated fields, numbers numerically and
/// below words.
fn compare_pre(a: &str, b: &str) -> Ordering {
    let mut a = a.split('.');
    let mut b = b.split('.');
    loop {
        match (a.next(), b.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

/// Whether `latest` is newer than `current`. `None` if either does not parse.
pub fn is_newer(latest: &str, current: &str) -> Option<bool> {
    Some(Version::parse(latest)? > Version::parse(current)?)
}

// ── Release assets ─────────────────────────────────────────────────────────

/// The release archive target for a platform, as named in
/// `.github/workflows/release.yml` and `install.sh`. `None` where no
/// prebuilt archive exists.
pub fn release_target(arch: &str, os: &str, env: &str) -> Option<String> {
    let arch = match arch {
        "x86_64" | "aarch64" => arch,
        _ => return None,
    };
    Some(match (os, env) {
        ("linux", "gnu") => format!("{arch}-unknown-linux-gnu"),
        ("android", _) if arch == "aarch64" => "aarch64-linux-android".to_string(),
        ("macos", _) => format!("{arch}-apple-darwin"),
        ("windows", "msvc") if arch == "x86_64" => "x86_64-pc-windows-msvc".to_string(),
        _ => return None,
    })
}

/// The target this binary was built for.
pub fn current_target() -> Option<String> {
    release_target(
        std::env::consts::ARCH,
        std::env::consts::OS,
        if cfg!(target_env = "gnu") {
            "gnu"
        } else if cfg!(target_env = "msvc") {
            "msvc"
        } else if cfg!(target_env = "musl") {
            "musl"
        } else {
            ""
        },
    )
}

/// Archive name for `target`: `stockwise-<target>.tar.gz`, or `.zip` on Windows.
pub fn asset_name(target: &str) -> String {
    if target.contains("windows") {
        format!("{BIN}-{target}.zip")
    } else {
        format!("{BIN}-{target}.tar.gz")
    }
}

/// Path of the binary inside the archive: `stockwise-<target>/stockwise[.exe]`.
pub fn binary_path_in_archive(target: &str) -> String {
    let exe = if target.contains("windows") {
        ".exe"
    } else {
        ""
    };
    format!("{BIN}-{target}/{BIN}{exe}")
}

#[derive(Clone, Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
}

/// Download URLs of the archive and its checksum. A release without the
/// `.sha256` next to the archive is refused: the checksum is mandatory.
pub fn select_assets(release: &Release, asset: &str) -> Result<(String, String)> {
    let url_of = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| a.name == name)
            .map(|a| a.browser_download_url.clone())
    };
    let archive =
        url_of(asset).with_context(|| format!("release {} has no {asset}", release.tag_name))?;
    let checksum = url_of(&format!("{asset}.sha256")).with_context(|| {
        format!(
            "release {} has no {asset}.sha256; refusing to install an unverified binary",
            release.tag_name
        )
    })?;
    Ok((archive, checksum))
}

// ── Checksum ───────────────────────────────────────────────────────────────

/// Check `data` against a `.sha256` file (`<hex>  <name>`, as `sha256sum`
/// writes it). A missing, empty or malformed checksum is an error, never a
/// pass.
pub fn verify_checksum(data: &[u8], checksum_file: Option<&str>) -> Result<()> {
    let text = checksum_file.context("checksum file is missing; refusing to install")?;
    let expected = text
        .split_whitespace()
        .next()
        .context("checksum file is empty; refusing to install")?
        .to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("checksum file is not a SHA-256 digest; refusing to install");
    }
    let actual = hex(&Sha256::digest(data));
    if actual != expected {
        bail!("checksum mismatch: expected {expected}, got {actual}; nothing was changed");
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── Archive extraction ─────────────────────────────────────────────────────

/// The file at `inner` from a release archive (`.tar.gz` or `.zip`).
pub fn extract_binary(archive: &[u8], asset: &str, inner: &str) -> Result<Vec<u8>> {
    if asset.ends_with(".zip") {
        extract_from_zip(archive, inner)
    } else {
        let mut tar = Vec::new();
        flate2::read::GzDecoder::new(archive)
            .read_to_end(&mut tar)
            .context("archive is not valid gzip")?;
        extract_from_tar(&tar, inner)
    }
}

fn parse_octal(field: &[u8]) -> Option<usize> {
    let s = std::str::from_utf8(field).ok()?;
    let s = s.trim_matches(|c: char| c == '\0' || c == ' ');
    if s.is_empty() {
        return Some(0);
    }
    usize::from_str_radix(s, 8).ok()
}

fn cstr(field: &[u8]) -> &[u8] {
    let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
    &field[..end]
}

/// A regular file from an uncompressed tar stream (ustar, with or without
/// pax/GNU extension entries, which are skipped).
fn extract_from_tar(tar: &[u8], inner: &str) -> Result<Vec<u8>> {
    let mut pos = 0;
    while pos + 512 <= tar.len() {
        let header = &tar[pos..pos + 512];
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let size = parse_octal(&header[124..136]).context("corrupt tar header")?;
        let kind = header[156];
        let name = cstr(&header[0..100]);
        let prefix = if &header[257..262] == b"ustar" {
            cstr(&header[345..500])
        } else {
            &[]
        };
        let full = if prefix.is_empty() {
            String::from_utf8_lossy(name).into_owned()
        } else {
            format!(
                "{}/{}",
                String::from_utf8_lossy(prefix),
                String::from_utf8_lossy(name)
            )
        };
        let start = pos + 512;
        let end = start.checked_add(size).context("corrupt tar header")?;
        if end > tar.len() {
            bail!("archive is truncated");
        }
        let full = full.strip_prefix("./").unwrap_or(&full);
        if (kind == b'0' || kind == 0) && full == inner {
            return Ok(tar[start..end].to_vec());
        }
        pos = start + size.div_ceil(512) * 512;
    }
    bail!("{inner} not found in the archive")
}

fn u16_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?) as usize)
}

fn u32_at(b: &[u8], at: usize) -> Option<usize> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?) as usize)
}

/// A file from a zip archive (stored or deflate), found through the central
/// directory.
fn extract_from_zip(zip: &[u8], inner: &str) -> Result<Vec<u8>> {
    const EOCD: &[u8] = &[0x50, 0x4b, 0x05, 0x06];
    const CENTRAL: &[u8] = &[0x50, 0x4b, 0x01, 0x02];
    const LOCAL: &[u8] = &[0x50, 0x4b, 0x03, 0x04];
    let corrupt = || anyhow::anyhow!("archive is not a valid zip file");

    let search_from = zip.len().saturating_sub(22 + 65_535);
    let eocd = (search_from..zip.len().saturating_sub(21))
        .rev()
        .find(|&i| &zip[i..i + 4] == EOCD)
        .ok_or_else(corrupt)?;
    let entries = u16_at(zip, eocd + 10).ok_or_else(corrupt)?;
    let mut pos = u32_at(zip, eocd + 16).ok_or_else(corrupt)?;
    for _ in 0..entries {
        if zip.get(pos..pos + 4) != Some(CENTRAL) {
            return Err(corrupt());
        }
        let method = u16_at(zip, pos + 10).ok_or_else(corrupt)?;
        let compressed = u32_at(zip, pos + 20).ok_or_else(corrupt)?;
        let size = u32_at(zip, pos + 24).ok_or_else(corrupt)?;
        let name_len = u16_at(zip, pos + 28).ok_or_else(corrupt)?;
        let extra_len = u16_at(zip, pos + 30).ok_or_else(corrupt)?;
        let comment_len = u16_at(zip, pos + 32).ok_or_else(corrupt)?;
        let local = u32_at(zip, pos + 42).ok_or_else(corrupt)?;
        let name = zip.get(pos + 46..pos + 46 + name_len).ok_or_else(corrupt)?;
        let name = String::from_utf8_lossy(name).replace('\\', "/");
        pos += 46 + name_len + extra_len + comment_len;
        if name != inner {
            continue;
        }
        if zip.get(local..local + 4) != Some(LOCAL) {
            return Err(corrupt());
        }
        let data_at = local
            + 30
            + u16_at(zip, local + 26).ok_or_else(corrupt)?
            + u16_at(zip, local + 28).ok_or_else(corrupt)?;
        let data = zip.get(data_at..data_at + compressed).ok_or_else(corrupt)?;
        let out = match method {
            0 => data.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(data)
                    .read_to_end(&mut out)
                    .map_err(|_| corrupt())?;
                out
            }
            m => bail!("unsupported zip compression method {m}"),
        };
        if out.len() != size {
            return Err(corrupt());
        }
        return Ok(out);
    }
    bail!("{inner} not found in the archive")
}

// ── Atomic swap ────────────────────────────────────────────────────────────

/// A temporary file next to `target`, so the final rename stays on one
/// file system.
fn sibling(target: &Path, suffix: &str) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| BIN.to_string());
    target.with_file_name(format!(".{name}.{}.{suffix}", std::process::id()))
}

/// Write `bytes` to a new executable file next to `target`.
fn write_staged(target: &Path, bytes: &[u8]) -> Result<PathBuf> {
    use std::io::Write;
    let staged = sibling(target, "new");
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&staged)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(target)
                .map(|m| m.permissions().mode() & 0o777)
                .unwrap_or(0o755)
                | 0o111;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode))?;
        }
        Ok(())
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&staged);
        return Err(e).with_context(|| {
            format!(
                "cannot write to {} (re-run with the permissions that installed it)",
                target.parent().unwrap_or(Path::new(".")).display()
            )
        });
    }
    Ok(staged)
}

/// Unix: stage the new binary next to `target`, then `rename` it over the
/// old one. The rename is atomic, so `target` is always a whole binary, and
/// a running process keeps its old inode.
pub fn swap_by_rename(target: &Path, bytes: &[u8]) -> Result<()> {
    let staged = write_staged(target, bytes)?;
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::remove_file(&staged);
        return Err(e).with_context(|| format!("cannot replace {}", target.display()));
    }
    Ok(())
}

/// The `.old` path [`swap_with_backup`] moves the running binary to.
pub fn backup_path(target: &Path) -> PathBuf {
    let mut name = target.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    target.with_file_name(name)
}

/// Windows: a running `.exe` cannot be replaced but can be renamed. Move it
/// to `<name>.old`, then move the staged binary into its place; if that
/// fails, the old binary is moved back.
pub fn swap_with_backup(target: &Path, bytes: &[u8]) -> Result<()> {
    let staged = write_staged(target, bytes)?;
    let old = backup_path(target);
    let _ = std::fs::remove_file(&old); // left by an earlier update
    if let Err(e) = std::fs::rename(target, &old) {
        let _ = std::fs::remove_file(&staged);
        return Err(e).with_context(|| format!("cannot move {} aside", target.display()));
    }
    if let Err(e) = std::fs::rename(&staged, target) {
        let _ = std::fs::rename(&old, target);
        let _ = std::fs::remove_file(&staged);
        return Err(e).with_context(|| format!("cannot replace {}", target.display()));
    }
    Ok(())
}

/// Replace the binary at `target` with `bytes` the way this OS allows.
pub fn swap_binary(target: &Path, bytes: &[u8]) -> Result<()> {
    if cfg!(windows) {
        swap_with_backup(target, bytes)
    } else {
        swap_by_rename(target, bytes)
    }
}

// ── Network ────────────────────────────────────────────────────────────────

/// Where release data comes from: GitHub in the binary, fixtures in tests.
pub(crate) trait Fetcher {
    /// The body of `url`, or an error for a failed request or non-2xx status.
    async fn get(&self, url: &str) -> Result<Vec<u8>>;
}

/// The real fetcher: HTTPS to GitHub with a StockWise user agent.
pub(crate) struct GitHub {
    client: reqwest::Client,
}

impl GitHub {
    pub fn new(timeout: Duration) -> Result<Self> {
        Ok(Self {
            client: crate::pagecache::client(timeout)?,
        })
    }
}

impl Fetcher for GitHub {
    async fn get(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self
            .client
            .get(url)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .with_context(|| format!("cannot reach {url}"))?;
        let status = resp.status();
        if !status.is_success() {
            bail!("{url} answered {status}");
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

/// The latest stable release (GitHub's `releases/latest` skips drafts and
/// pre-releases).
pub(crate) async fn latest_release(fetcher: &impl Fetcher) -> Result<Release> {
    let body = fetcher.get(LATEST_RELEASE_API).await?;
    serde_json::from_slice(&body).context("unexpected answer from the GitHub Releases API")
}

/// What a standalone update did.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    UpToDate { latest: String },
    Updated { from: String, to: String },
}

/// Update the standalone binary at `target` from `release`: download the
/// archive and its `.sha256`, verify, extract, swap. Any failure before the
/// swap leaves `target` untouched.
pub(crate) async fn install_release(
    fetcher: &impl Fetcher,
    release: &Release,
    current: &str,
    target_triple: &str,
    target: &Path,
) -> Result<Outcome> {
    match is_newer(&release.tag_name, current) {
        Some(true) => {}
        Some(false) => {
            return Ok(Outcome::UpToDate {
                latest: release.tag_name.clone(),
            });
        }
        None => bail!("cannot compare versions {} and {current}", release.tag_name),
    }
    let asset = asset_name(target_triple);
    let (archive_url, checksum_url) = select_assets(release, &asset)?;
    let checksum = fetcher
        .get(&checksum_url)
        .await
        .with_context(|| format!("cannot download {asset}.sha256; refusing to install"))?;
    let checksum = String::from_utf8(checksum).ok();
    let archive = fetcher
        .get(&archive_url)
        .await
        .with_context(|| format!("cannot download {asset}"))?;
    verify_checksum(&archive, checksum.as_deref())?;
    let binary = extract_binary(&archive, &asset, &binary_path_in_archive(target_triple))?;
    if binary.is_empty() {
        bail!("the binary in {asset} is empty; nothing was changed");
    }
    swap_binary(target, &binary)?;
    Ok(Outcome::Updated {
        from: current.to_string(),
        to: release.tag_name.clone(),
    })
}

// ── Weekly check cache (doctor) ────────────────────────────────────────────

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CheckCache {
    /// Unix seconds of the last successful check.
    pub checked_at: u64,
    /// Latest release tag at that time.
    pub latest: String,
}

/// Whether a check made at `checked_at` is still fresh at `now`.
pub fn cache_is_fresh(checked_at: u64, now: u64) -> bool {
    now >= checked_at && now - checked_at < CHECK_TTL_SECS
}

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

pub fn read_cache(dir: &Path) -> Option<CheckCache> {
    let text = std::fs::read_to_string(dir.join(CHECK_CACHE_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn write_cache(dir: &Path, cache: &CheckCache) {
    if let Ok(text) = serde_json::to_string(cache) {
        let _ = std::fs::write(dir.join(CHECK_CACHE_FILE), text);
    }
}

/// The latest release tag for `doctor`: from the cache in `dir` when it is
/// less than a week old, otherwise from GitHub (and then cached). The
/// second value says whether it came from the cache.
pub(crate) async fn latest_tag_cached(
    fetcher: &impl Fetcher,
    dir: Option<&Path>,
    now: u64,
) -> Result<(String, bool)> {
    if let Some(cache) = dir.and_then(read_cache)
        && cache_is_fresh(cache.checked_at, now)
    {
        return Ok((cache.latest, true));
    }
    let release = latest_release(fetcher).await?;
    if let Some(dir) = dir {
        write_cache(
            dir,
            &CheckCache {
                checked_at: now,
                latest: release.tag_name.clone(),
            },
        );
    }
    Ok((release.tag_name, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::io::Write;
    use std::sync::Mutex;

    // ── fixtures ──

    struct TempDir(PathBuf);
    impl TempDir {
        fn new(tag: &str) -> Self {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = std::env::temp_dir().join(format!(
                "stockwise-selfupdate-{tag}-{}-{n}-{}",
                std::process::id(),
                now_secs()
            ));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// Serves canned bodies by URL; anything else is a 404. Records requests.
    struct FakeFetcher {
        bodies: HashMap<String, Vec<u8>>,
        seen: Mutex<Vec<String>>,
    }
    impl FakeFetcher {
        fn new(bodies: &[(&str, Vec<u8>)]) -> Self {
            Self {
                bodies: bodies
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.clone()))
                    .collect(),
                seen: Mutex::new(Vec::new()),
            }
        }
        fn requests(&self) -> usize {
            self.seen.lock().unwrap().len()
        }
    }
    impl Fetcher for FakeFetcher {
        async fn get(&self, url: &str) -> Result<Vec<u8>> {
            self.seen.lock().unwrap().push(url.to_string());
            self.bodies
                .get(url)
                .cloned()
                .with_context(|| format!("{url} answered 404 Not Found"))
        }
    }

    fn tar_entry(out: &mut Vec<u8>, name: &str, kind: u8, data: &[u8]) {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        h[100..107].copy_from_slice(b"0000755");
        h[108..115].copy_from_slice(b"0000000");
        h[116..123].copy_from_slice(b"0000000");
        h[124..135].copy_from_slice(format!("{:011o}", data.len()).as_bytes());
        h[136..147].copy_from_slice(b"00000000000");
        h[156] = kind;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| b as u32).sum();
        h[148..155].copy_from_slice(format!("{:06o}\0", sum).as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
    }

    /// A `.tar.gz` laid out like the release workflow's, with a pax header
    /// and a macOS `._` file in front of the binary.
    fn tar_gz(target: &str, binary: &[u8]) -> Vec<u8> {
        let mut tar = Vec::new();
        tar_entry(&mut tar, &format!("stockwise-{target}/"), b'5', b"");
        tar_entry(
            &mut tar,
            "PaxHeader/stockwise",
            b'x',
            b"30 mtime=1700000000.000000000\n",
        );
        tar_entry(
            &mut tar,
            &format!("stockwise-{target}/._stockwise"),
            b'0',
            b"junk",
        );
        tar_entry(
            &mut tar,
            &format!("stockwise-{target}/stockwise"),
            b'0',
            binary,
        );
        tar_entry(
            &mut tar,
            &format!("stockwise-{target}/README.md"),
            b'0',
            b"readme",
        );
        tar.extend_from_slice(&[0u8; 1024]);
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&tar).unwrap();
        gz.finish().unwrap()
    }

    /// A zip with one deflated and one stored entry, as 7z writes them.
    fn zip(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data, deflate) in entries {
            let body = if *deflate {
                let mut e =
                    flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::fast());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let method: u16 = if *deflate { 8 } else { 0 };
            let offset = out.len() as u32;
            out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
            out.extend_from_slice(&20u16.to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // time, date, crc (unchecked)
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);

            central.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&20u16.to_le_bytes());
            central.extend_from_slice(&0u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]); // extra, comment, disk, attrs
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let cd_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    fn sha_line(data: &[u8], name: &str) -> String {
        format!("{}  {name}\n", hex(&Sha256::digest(data)))
    }

    // ── channel detection ──

    fn detect(path: &str) -> Channel {
        detect_channel(Path::new(path), &ChannelHints::default())
    }

    #[test]
    fn detects_homebrew() {
        assert_eq!(
            detect("/opt/homebrew/Cellar/stockwise/0.9.0/bin/stockwise"),
            Channel::Homebrew
        );
        assert_eq!(
            detect("/usr/local/Cellar/stockwise/0.9.0/bin/stockwise"),
            Channel::Homebrew
        );
        assert_eq!(
            detect("/home/linuxbrew/.linuxbrew/Cellar/stockwise/0.9.0/bin/stockwise"),
            Channel::Homebrew
        );
    }

    #[test]
    fn detects_scoop_and_winget_with_windows_paths() {
        assert_eq!(
            detect(r"C:\Users\me\scoop\apps\stockwise\current\stockwise.exe"),
            Channel::Scoop
        );
        assert_eq!(
            detect(
                r"C:\Users\me\AppData\Local\Microsoft\WinGet\Packages\iamsaquib8.StockWise_Microsoft.Winget.Source_8wekyb3d8bbwe\stockwise.exe"
            ),
            Channel::Winget
        );
    }

    #[test]
    fn detects_nix() {
        assert_eq!(
            detect("/nix/store/abc123-stockwise-0.9.0/bin/stockwise"),
            Channel::Nix
        );
    }

    #[test]
    fn detects_cargo_from_home_and_cargo_home() {
        let hints = ChannelHints {
            home: Some(PathBuf::from("/home/me")),
            ..Default::default()
        };
        assert_eq!(
            detect_channel(Path::new("/home/me/.cargo/bin/stockwise"), &hints),
            Channel::Cargo
        );
        let hints = ChannelHints {
            home: Some(PathBuf::from("/home/me")),
            cargo_home: Some(PathBuf::from("/opt/rust/cargo")),
            ..Default::default()
        };
        assert_eq!(
            detect_channel(Path::new("/opt/rust/cargo/bin/stockwise"), &hints),
            Channel::Cargo
        );
        // A different user's .cargo is not this user's cargo install.
        assert_eq!(
            detect("/home/other/.cargo/bin/stockwise"),
            Channel::Standalone
        );
    }

    #[test]
    fn detects_npm() {
        assert_eq!(
            detect("/usr/lib/node_modules/stockwise/bin/stockwise"),
            Channel::Npm
        );
    }

    #[test]
    fn detects_python_channels() {
        assert_eq!(
            detect("/home/me/.local/pipx/venvs/stockwise/bin/stockwise"),
            Channel::Pipx
        );
        assert_eq!(
            detect("/home/me/.local/share/pipx/venvs/stockwise/bin/stockwise"),
            Channel::Pipx
        );
        assert_eq!(
            detect("/home/me/.local/share/uv/tools/stockwise/bin/stockwise"),
            Channel::Uv
        );
        assert_eq!(
            detect("/usr/lib/python3/site-packages/stockwise/stockwise"),
            Channel::Pip
        );
        let venv = ChannelHints {
            in_python_venv: true,
            ..Default::default()
        };
        assert_eq!(
            detect_channel(Path::new("/home/me/project/.venv/bin/stockwise"), &venv),
            Channel::Pip
        );
    }

    #[test]
    fn detects_deb_rpm() {
        assert_eq!(detect("/usr/bin/stockwise"), Channel::SystemPackage);
    }

    #[test]
    fn detects_docker_before_any_path() {
        let docker = ChannelHints {
            in_container: true,
            ..Default::default()
        };
        assert_eq!(
            detect_channel(Path::new("/usr/local/bin/stockwise"), &docker),
            Channel::Docker
        );
    }

    #[test]
    fn everything_else_is_standalone() {
        for p in [
            "/home/me/.local/bin/stockwise",
            "/usr/local/bin/stockwise",
            "/data/data/com.termux/files/usr/bin/stockwise",
            r"C:\tools\stockwise\stockwise.exe",
            "/home/me/Downloads/stockwise-x86_64-unknown-linux-gnu/stockwise",
        ] {
            assert_eq!(detect(p), Channel::Standalone, "{p}");
        }
    }

    #[test]
    fn only_standalone_has_no_upgrade_command() {
        use Channel::*;
        for c in [
            Homebrew,
            Scoop,
            Nix,
            Cargo,
            Npm,
            Pipx,
            Uv,
            Pip,
            Winget,
            SystemPackage,
            Docker,
        ] {
            assert!(c.upgrade_commands().is_some_and(|v| !v.is_empty()), "{c:?}");
        }
        assert!(Standalone.upgrade_commands().is_none());
    }

    // ── versions ──

    #[test]
    fn version_comparison() {
        assert_eq!(is_newer("v0.9.0", "0.8.1"), Some(true));
        assert_eq!(is_newer("v0.8.1", "0.8.1"), Some(false));
        assert_eq!(is_newer("v0.8.0", "0.8.1"), Some(false));
        assert_eq!(is_newer("v0.10.0", "0.9.9"), Some(true));
        assert_eq!(is_newer("v1.0.0", "0.99.99"), Some(true));
        assert_eq!(is_newer("0.8.2", "v0.8.1"), Some(true));
        // A pre-release sorts before its release.
        assert_eq!(is_newer("v0.9.0", "0.9.0-rc.1"), Some(true));
        assert_eq!(is_newer("v0.9.0-rc.1", "0.9.0"), Some(false));
        assert_eq!(is_newer("v0.9.0-rc.2", "0.9.0-rc.1"), Some(true));
        assert_eq!(is_newer("v0.9.0-rc.10", "0.9.0-rc.9"), Some(true));
        assert_eq!(is_newer("v0.9.0-beta", "0.9.0-alpha"), Some(true));
        // Build metadata does not order.
        assert_eq!(is_newer("v0.8.1+build.5", "0.8.1"), Some(false));
        // Not a version.
        assert_eq!(is_newer("nightly", "0.8.1"), None);
        assert_eq!(is_newer("v0.9", "0.8.1"), None);
        assert_eq!(is_newer("v0.9.0.1", "0.8.1"), None);
        assert_eq!(is_newer("v0.9.0-", "0.8.1"), None);
    }

    #[test]
    fn own_version_parses() {
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_some());
    }

    // ── assets ──

    #[test]
    fn release_targets_match_the_workflow() {
        assert_eq!(
            release_target("x86_64", "linux", "gnu").as_deref(),
            Some("x86_64-unknown-linux-gnu")
        );
        assert_eq!(
            release_target("aarch64", "linux", "gnu").as_deref(),
            Some("aarch64-unknown-linux-gnu")
        );
        assert_eq!(
            release_target("x86_64", "macos", "").as_deref(),
            Some("x86_64-apple-darwin")
        );
        assert_eq!(
            release_target("aarch64", "macos", "").as_deref(),
            Some("aarch64-apple-darwin")
        );
        assert_eq!(
            release_target("x86_64", "windows", "msvc").as_deref(),
            Some("x86_64-pc-windows-msvc")
        );
        assert_eq!(
            release_target("aarch64", "android", "").as_deref(),
            Some("aarch64-linux-android")
        );
        assert_eq!(release_target("x86_64", "linux", "musl"), None);
        assert_eq!(release_target("riscv64", "linux", "gnu"), None);
        assert_eq!(release_target("aarch64", "windows", "msvc"), None);

        assert_eq!(
            asset_name("x86_64-unknown-linux-gnu"),
            "stockwise-x86_64-unknown-linux-gnu.tar.gz"
        );
        assert_eq!(
            asset_name("x86_64-pc-windows-msvc"),
            "stockwise-x86_64-pc-windows-msvc.zip"
        );
        assert_eq!(
            binary_path_in_archive("x86_64-pc-windows-msvc"),
            "stockwise-x86_64-pc-windows-msvc/stockwise.exe"
        );
    }

    fn release(tag: &str, names: &[&str]) -> Release {
        Release {
            tag_name: tag.into(),
            html_url: String::new(),
            assets: names
                .iter()
                .map(|n| Asset {
                    name: n.to_string(),
                    browser_download_url: format!("https://dl/{n}"),
                })
                .collect(),
        }
    }

    #[test]
    fn missing_sha256_asset_is_refused() {
        let asset = "stockwise-x86_64-apple-darwin.tar.gz";
        let r = release("v9.0.0", &[asset]);
        let err = select_assets(&r, asset).unwrap_err().to_string();
        assert!(err.contains(".sha256"), "{err}");
        let r = release("v9.0.0", &[asset, &format!("{asset}.sha256")]);
        assert_eq!(
            select_assets(&r, asset).unwrap(),
            (
                format!("https://dl/{asset}"),
                format!("https://dl/{asset}.sha256")
            )
        );
    }

    // ── checksum ──

    #[test]
    fn checksum_matches() {
        let data = b"binary";
        verify_checksum(data, Some(&sha_line(data, "x.tar.gz"))).unwrap();
        // Upper-case hex and a bare digest pass too.
        let upper = hex(&Sha256::digest(data)).to_uppercase();
        verify_checksum(data, Some(&upper)).unwrap();
    }

    #[test]
    fn checksum_mismatch_aborts() {
        let err = verify_checksum(b"tampered", Some(&sha_line(b"binary", "x")))
            .unwrap_err()
            .to_string();
        assert!(err.contains("mismatch"), "{err}");
    }

    #[test]
    fn missing_or_malformed_checksum_aborts() {
        assert!(verify_checksum(b"binary", None).is_err());
        assert!(verify_checksum(b"binary", Some("")).is_err());
        assert!(verify_checksum(b"binary", Some("   \n")).is_err());
        assert!(verify_checksum(b"binary", Some("abc123  x.tar.gz")).is_err());
        assert!(verify_checksum(b"binary", Some(&"z".repeat(64))).is_err());
    }

    // ── archives ──

    #[test]
    fn extracts_binary_from_tar_gz() {
        let t = "aarch64-apple-darwin";
        let archive = tar_gz(t, b"new binary");
        let bin = extract_binary(&archive, &asset_name(t), &binary_path_in_archive(t)).unwrap();
        assert_eq!(bin, b"new binary");
        assert!(extract_binary(&archive, &asset_name(t), "stockwise-other/stockwise").is_err());
        assert!(extract_binary(b"not gzip", &asset_name(t), "x").is_err());
    }

    #[test]
    fn extracts_binary_from_zip() {
        let t = "x86_64-pc-windows-msvc";
        let inner = binary_path_in_archive(t);
        for deflate in [true, false] {
            let archive = zip(&[
                (
                    "stockwise-x86_64-pc-windows-msvc/README.md",
                    b"readme",
                    false,
                ),
                (&inner, b"new exe new exe new exe", deflate),
            ]);
            let bin = extract_binary(&archive, &asset_name(t), &inner).unwrap();
            assert_eq!(bin, b"new exe new exe new exe");
        }
        assert!(extract_binary(b"PK not a zip", &asset_name(t), &inner).is_err());
    }

    // ── swap ──

    #[test]
    fn swap_by_rename_replaces_atomically() {
        let dir = TempDir::new("rename");
        let exe = dir.0.join("stockwise");
        std::fs::write(&exe, b"old").unwrap();
        swap_by_rename(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "new binary is executable");
        }
        // Only the binary is left: the staged file was renamed, not copied.
        let names: Vec<_> = std::fs::read_dir(&dir.0).unwrap().collect();
        assert_eq!(names.len(), 1);
    }

    #[test]
    fn swap_with_backup_keeps_old_binary() {
        let dir = TempDir::new("backup");
        let exe = dir.0.join("stockwise.exe");
        std::fs::write(&exe, b"old").unwrap();
        std::fs::write(backup_path(&exe), b"older").unwrap();
        swap_with_backup(&exe, b"new").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert_eq!(std::fs::read(backup_path(&exe)).unwrap(), b"old");
        assert_eq!(backup_path(&exe).file_name().unwrap(), "stockwise.exe.old");
    }

    #[test]
    fn swap_into_missing_directory_fails_cleanly() {
        let dir = TempDir::new("missing");
        let exe = dir.0.join("nope").join("stockwise");
        assert!(swap_by_rename(&exe, b"new").is_err());
        assert!(swap_with_backup(&exe, b"new").is_err());
    }

    // ── end to end, with a fake GitHub ──

    const TARGET: &str = "x86_64-unknown-linux-gnu";

    fn fake_github(tag: &str, archive: &[u8], checksum: Option<String>) -> FakeFetcher {
        let asset = asset_name(TARGET);
        let mut assets = vec![serde_json::json!({
            "name": asset, "browser_download_url": format!("https://dl/{asset}")
        })];
        let mut bodies = vec![(format!("https://dl/{asset}"), archive.to_vec())];
        if let Some(sum) = checksum {
            assets.push(serde_json::json!({
                "name": format!("{asset}.sha256"),
                "browser_download_url": format!("https://dl/{asset}.sha256")
            }));
            bodies.push((format!("https://dl/{asset}.sha256"), sum.into_bytes()));
        }
        let json = serde_json::json!({ "tag_name": tag, "html_url": "", "assets": assets });
        bodies.push((
            LATEST_RELEASE_API.to_string(),
            json.to_string().into_bytes(),
        ));
        let refs: Vec<(&str, Vec<u8>)> = bodies
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        FakeFetcher::new(&refs)
    }

    async fn run(fetcher: &FakeFetcher, exe: &Path) -> Result<Outcome> {
        let rel = latest_release(fetcher).await?;
        install_release(fetcher, &rel, "0.8.1", TARGET, exe).await
    }

    #[tokio::test]
    async fn updates_standalone_binary() {
        let dir = TempDir::new("e2e");
        let exe = dir.0.join("stockwise");
        std::fs::write(&exe, b"old").unwrap();
        let archive = tar_gz(TARGET, b"new binary");
        let f = fake_github(
            "v0.9.0",
            &archive,
            Some(sha_line(&archive, &asset_name(TARGET))),
        );
        assert_eq!(
            run(&f, &exe).await.unwrap(),
            Outcome::Updated {
                from: "0.8.1".into(),
                to: "v0.9.0".into()
            }
        );
        assert_eq!(std::fs::read(&exe).unwrap(), b"new binary");
    }

    #[tokio::test]
    async fn up_to_date_downloads_nothing() {
        let dir = TempDir::new("current");
        let exe = dir.0.join("stockwise");
        std::fs::write(&exe, b"old").unwrap();
        let f = fake_github("v0.8.1", b"x", None);
        assert_eq!(
            run(&f, &exe).await.unwrap(),
            Outcome::UpToDate {
                latest: "v0.8.1".into()
            }
        );
        assert_eq!(f.requests(), 1, "only the release lookup");
        assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    }

    #[tokio::test]
    async fn bad_or_missing_checksum_leaves_binary_untouched() {
        let archive = tar_gz(TARGET, b"evil binary");
        for checksum in [
            None,
            Some(sha_line(b"something else", "x")),
            Some(String::new()),
        ] {
            let dir = TempDir::new("reject");
            let exe = dir.0.join("stockwise");
            std::fs::write(&exe, b"old").unwrap();
            let f = fake_github("v0.9.0", &archive, checksum);
            assert!(run(&f, &exe).await.is_err());
            assert_eq!(std::fs::read(&exe).unwrap(), b"old");
            let left: Vec<_> = std::fs::read_dir(&dir.0).unwrap().collect();
            assert_eq!(left.len(), 1, "no staged file left behind");
        }
    }

    // ── doctor cache ──

    #[test]
    fn cache_freshness() {
        assert!(cache_is_fresh(1_000, 1_000));
        assert!(cache_is_fresh(1_000, 1_000 + CHECK_TTL_SECS - 1));
        assert!(!cache_is_fresh(1_000, 1_000 + CHECK_TTL_SECS));
        // A clock that went backwards does not keep a cache forever.
        assert!(!cache_is_fresh(2_000, 1_000));
    }

    #[tokio::test]
    async fn doctor_uses_fresh_cache_without_network() {
        let dir = TempDir::new("cache");
        let f = fake_github("v0.9.0", b"x", None);
        let now = 10 * CHECK_TTL_SECS;

        // Nothing cached: one request, then cached.
        let (tag, cached) = latest_tag_cached(&f, Some(&dir.0), now).await.unwrap();
        assert_eq!((tag.as_str(), cached), ("v0.9.0", false));
        assert_eq!(f.requests(), 1);
        assert_eq!(read_cache(&dir.0).unwrap().checked_at, now);

        // Fresh: no request.
        let (tag, cached) = latest_tag_cached(&f, Some(&dir.0), now + 60).await.unwrap();
        assert_eq!((tag.as_str(), cached), ("v0.9.0", true));
        assert_eq!(f.requests(), 1);

        // A week later: asks again.
        let (_, cached) = latest_tag_cached(&f, Some(&dir.0), now + CHECK_TTL_SECS)
            .await
            .unwrap();
        assert!(!cached);
        assert_eq!(f.requests(), 2);
    }
}
