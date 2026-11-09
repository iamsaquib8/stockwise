//! Optional config file for defaults: `config.toml` in the platform config
//! directory (`dirs::config_dir()/stockwise`), or the file named by
//! `STOCKWISE_CONFIG`.
//!
//! Precedence for every setting: command-line flag > environment variable >
//! config file > built-in default. `main` turns the file's `market`, `ai` and
//! `[periods]` values into clap defaults, so `--help` shows them and flags and
//! env vars still win. Other modules read the rest through [`setting`].

use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Env var that points at another config file.
pub const ENV_VAR: &str = "STOCKWISE_CONFIG";

#[derive(Debug, Default, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Market for bare tickers: `in` or `us`.
    pub market: Option<String>,
    /// Add the local-AI section to analysis commands.
    pub ai: Option<bool>,
    /// Ollama model name.
    pub model: Option<String>,
    /// Ollama server (`host:port` or a URL).
    pub ollama_host: Option<String>,
    /// Redis cache URL, or `off`.
    pub redis_url: Option<String>,
    /// Data directory; a leading `~` means the home directory.
    pub home: Option<String>,
    /// Output language: `en` or `hi`.
    pub lang: Option<String>,
    /// Currency bare crypto tickers are quoted in (`usd`, `inr`, ...).
    pub quote_currency: Option<String>,
    /// Named portfolio to use (see `--portfolio`).
    pub portfolio: Option<String>,
    /// AI provider: `ollama` (default), `openai`, `anthropic` or `gemini`.
    /// API keys stay in environment variables.
    pub ai_provider: Option<String>,
    /// Market-data provider: `yahoo` (default), `alphavantage`, `massive`
    /// or `twelvedata`. API keys stay in environment variables.
    pub data_provider: Option<String>,
    /// Default `--period` per command, e.g. `technical = "6mo"`.
    #[serde(default)]
    pub periods: BTreeMap<String, String>,
    /// Notification channels, keyed like `STOCKWISE_<KEY>` without the
    /// prefix: `telegram_token = "..."`. Environment variables win.
    #[serde(default)]
    pub notify: BTreeMap<String, String>,
}

static EMPTY: Config = Config {
    market: None,
    ai: None,
    model: None,
    ollama_host: None,
    redis_url: None,
    home: None,
    lang: None,
    quote_currency: None,
    portfolio: None,
    ai_provider: None,
    data_provider: None,
    periods: BTreeMap::new(),
    notify: BTreeMap::new(),
};

/// The `[notify]` key for a `STOCKWISE_*` notification variable.
pub fn notify_key(env: &str) -> Option<String> {
    env.strip_prefix("STOCKWISE_").map(str::to_ascii_lowercase)
}

/// A notification setting: the environment variable, else `[notify]` in the
/// config file.
pub fn notify_setting(env: &str) -> Option<String> {
    std::env::var(env)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| notify_key(env).and_then(|k| get().notify.get(&k).cloned()))
}

/// `[notify]` keys that hold secrets.
const SECRET_NOTIFY_KEYS: &[&str] = &[
    "telegram_token",
    "slack_webhook",
    "discord_webhook",
    "smtp_password",
];

/// Where the config file was looked for and what came of it.
#[derive(Debug)]
pub struct State {
    pub path: Option<PathBuf>,
    pub config: Config,
    pub status: Status,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// No file at the path: built-in defaults apply.
    Missing,
    Loaded,
    /// The file could not be read or is invalid. The message names the file
    /// and the offending key.
    Invalid(String),
}

/// The config file's path when it holds notification secrets and other
/// users can read it (Unix permissions).
pub fn exposed_secrets() -> Option<&'static Path> {
    let state = state()?;
    if state.status != Status::Loaded {
        return None;
    }
    let has_secret = SECRET_NOTIFY_KEYS.iter().any(|k| {
        state
            .config
            .notify
            .get(*k)
            .is_some_and(|v| !v.trim().is_empty())
    });
    let path = state.path.as_deref().filter(|_| has_secret)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(path).ok()?.permissions().mode();
        (mode & 0o077 != 0).then_some(path)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

/// Whether `key` (a `[notify]` key) holds a secret, so `config show` masks it.
pub fn is_secret_notify_key(key: &str) -> bool {
    SECRET_NOTIFY_KEYS.contains(&key)
}

/// Where an effective setting came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    Flag,
    Env,
    File,
    Default,
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Source::Flag => "flag",
            Source::Env => "env",
            Source::File => "config file",
            Source::Default => "default",
        })
    }
}

/// Where the global `--market` and `--ai` values came from (clap knows
/// whether a flag or env var was used; `main` fills this in).
#[derive(Debug, Clone, Copy)]
pub struct CliSources {
    pub market: Source,
    pub ai: Source,
    pub quote_currency: Source,
    pub portfolio: Source,
}

/// Config file path: `STOCKWISE_CONFIG`, else `<config dir>/stockwise/config.toml`.
pub fn path() -> Option<PathBuf> {
    match std::env::var_os(ENV_VAR) {
        Some(p) if !p.is_empty() => Some(PathBuf::from(p)),
        _ => dirs::config_dir().map(|d| d.join("stockwise").join("config.toml")),
    }
}

/// Find and parse the config file. Never fails: problems are reported in
/// [`State::status`] so `doctor` and `config` can still run.
/// `period_commands` are the commands that take `--period`.
pub fn load(period_commands: &[&str]) -> State {
    let Some(path) = path() else {
        return State {
            path: None,
            config: Config::default(),
            status: Status::Missing,
        };
    };
    let explicit = std::env::var_os(ENV_VAR).is_some_and(|p| !p.is_empty());
    let (config, status) = match read(&path, period_commands) {
        Ok(Some(config)) => (config, Status::Loaded),
        Ok(None) if explicit => (
            Config::default(),
            Status::Invalid(format!(
                "{} points to {}, which does not exist (create it with `stockwise config init`)",
                ENV_VAR,
                path.display()
            )),
        ),
        Ok(None) => (Config::default(), Status::Missing),
        Err(e) => (Config::default(), Status::Invalid(format!("{:#}", e))),
    };
    State {
        path: Some(path),
        config,
        status,
    }
}

/// Read and validate the file at `path`; `None` if it does not exist.
pub fn read(path: &Path, period_commands: &[&str]) -> Result<Option<Config>> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(e).with_context(|| format!("Cannot read config file {}", path.display()));
        }
    };
    parse(&text, period_commands)
        .map(Some)
        .with_context(|| format!("Invalid config file {}", path.display()))
}

/// Parse and validate config text.
pub fn parse(text: &str, period_commands: &[&str]) -> Result<Config> {
    let config: Config = toml::from_str(text)?;
    config.validate(period_commands)?;
    Ok(config)
}

impl Config {
    fn validate(&self, period_commands: &[&str]) -> Result<()> {
        if let Some(m) = &self.market {
            market_value(m)?;
        }
        for (key, value) in [
            ("model", &self.model),
            ("ollama_host", &self.ollama_host),
            ("redis_url", &self.redis_url),
            ("home", &self.home),
            ("lang", &self.lang),
            ("quote_currency", &self.quote_currency),
            ("portfolio", &self.portfolio),
            ("ai_provider", &self.ai_provider),
            ("data_provider", &self.data_provider),
        ] {
            if value.as_deref().is_some_and(|v| v.trim().is_empty()) {
                bail!("`{}` is empty — remove the line to use the default", key);
            }
        }
        if let Some(l) = &self.lang
            && !crate::i18n::is_lang(l)
        {
            bail!("`lang` = \"{}\" is not a language; use \"en\" or \"hi\"", l);
        }
        if let Some(c) = &self.quote_currency
            && !(c.trim().len() == 3 && c.trim().chars().all(|ch| ch.is_ascii_alphabetic()))
        {
            bail!(
                "`quote_currency` = \"{}\" is not a 3-letter currency code (e.g. \"usd\", \"inr\")",
                c
            );
        }
        if let Some(p) = &self.portfolio {
            crate::portfolio::normalize_name(Some(p)).context("`portfolio`")?;
        }
        if let Some(p) = &self.ai_provider
            && crate::ai::AiProvider::parse(p).is_none()
        {
            bail!(
                "`ai_provider` = \"{}\" is not a provider; use ollama, openai, anthropic or gemini",
                p
            );
        }
        if let Some(p) = &self.data_provider
            && !p.trim().eq_ignore_ascii_case("yahoo")
            && crate::providers::ProviderKind::parse(p).is_none()
        {
            bail!(
                "`data_provider` = \"{}\" is not a provider; use yahoo, alphavantage, massive, twelvedata, finnhub, fmp, nse, bse or exchange",
                p
            );
        }
        let known: Vec<String> = crate::notify::ENV_VARS
            .iter()
            .filter_map(|(env, _)| notify_key(env))
            .collect();
        for key in self.notify.keys() {
            if !known.contains(key) {
                bail!(
                    "`notify.{}` is not a setting; use one of {}",
                    key,
                    known.join(", ")
                );
            }
        }
        for (command, period) in &self.periods {
            if !period_commands.contains(&command.as_str()) {
                bail!(
                    "`periods.{}`: `{}` has no --period option. Commands with one: {}",
                    command,
                    command,
                    period_commands.join(", ")
                );
            }
            period_value(period).with_context(|| format!("`periods.{}`", command))?;
        }
        Ok(())
    }
}

/// The clap spelling (`in` / `us`) of a market name from the file.
pub fn market_value(s: &str) -> Result<&'static str> {
    match crate::market::Market::from_str(s, true) {
        Ok(market) => Ok(market.code()),
        Err(_) => bail!(
            "`market` = \"{}\" is not a market; use \"in\", \"us\", \"crypto\", \"fx\", \"commodity\" \
             or a global exchange (\"uk\", \"de\", \"jp\", \"hk\", \"sg\", \"ca\", \"au\")",
            s
        ),
    }
}

/// The matching entry of [`crate::PERIODS`], so it can be a clap default.
pub fn period_value(s: &str) -> Result<&'static str> {
    crate::PERIODS
        .iter()
        .copied()
        .find(|p| *p == s)
        .with_context(|| {
            format!(
                "\"{}\" is not a period; use one of {}",
                s,
                crate::PERIODS.join(", ")
            )
        })
}

static STATE: OnceLock<State> = OnceLock::new();

/// Make `state` the process-wide config. Later calls are ignored.
pub fn install(state: State) -> &'static State {
    let _ = STATE.set(state);
    STATE.get().expect("just set")
}

pub fn state() -> Option<&'static State> {
    STATE.get()
}

/// The loaded config, or an empty one (no file, invalid file, or tests).
pub fn get() -> &'static Config {
    STATE
        .get()
        .filter(|s| s.status == Status::Loaded)
        .map(|s| &s.config)
        .unwrap_or(&EMPTY)
}

/// Env var (if set and non-empty) over the config file's value.
pub fn resolve(env: Option<String>, file: Option<&str>) -> Option<(String, Source)> {
    match env {
        Some(v) if !v.trim().is_empty() => Some((v, Source::Env)),
        _ => file.map(|v| (v.to_string(), Source::File)),
    }
}

/// A setting from `env` or, failing that, the config file. `None` means
/// "use the built-in default".
pub fn setting(env: &str, pick: impl FnOnce(&Config) -> Option<&String>) -> Option<String> {
    resolve(std::env::var(env).ok(), pick(get()).map(String::as_str)).map(|(v, _)| v)
}

/// Where [`setting`] would take `env`'s value from.
pub fn setting_source(env: &str, pick: impl FnOnce(&Config) -> Option<&String>) -> Source {
    resolve(std::env::var(env).ok(), pick(get()).map(String::as_str))
        .map_or(Source::Default, |(_, s)| s)
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(p: &str) -> PathBuf {
    match p.strip_prefix('~') {
        Some(rest) if rest.is_empty() || rest.starts_with('/') || rest.starts_with('\\') => {
            match dirs::home_dir() {
                Some(home) => home.join(rest.trim_start_matches(['/', '\\'])),
                None => PathBuf::from(p),
            }
        }
        _ => PathBuf::from(p),
    }
}

/// Commented template written by `stockwise config init`. Everything is
/// commented out, so a fresh file changes nothing until a line is edited.
pub const TEMPLATE: &str = r#"# StockWise configuration.
#
# Precedence: command-line flag > environment variable > this file > built-in default.
# Uncomment a line to change a default; `stockwise config show` prints the
# effective values and where each one comes from.

# Market for bare tickers: "in" (India, NSE — TCS means TCS.NS) or "us".
# Env: STOCKWISE_MARKET. Flag: -m / --market.
# market = "in"

# Add a local-AI (Ollama) section to analysis commands (slow: 30-60s per call).
# Env: STOCKWISE_AI. Flags: --ai / --no-ai.
# ai = false

# Ollama model and server. Env: STOCKWISE_MODEL, OLLAMA_HOST.
# model = "qwen3:14b"
# ollama_host = "http://localhost:11434"

# Optional Redis cache server. Off by default: StockWise keeps a SQLite
# cache.db in the data directory instead (`stockwise cache status`).
# Env: STOCKWISE_REDIS_URL.
# redis_url = "redis://127.0.0.1:6379/"

# Data directory for portfolio, watchlist, alerts and history ("~" = your home).
# Env: STOCKWISE_HOME. Default: shown by `stockwise doctor`.
# home = "~/stockwise"

# Output language: "en" or "hi" (Hindi). Env: STOCKWISE_LANG. Flag: --lang.
# lang = "en"

# Currency bare crypto tickers are quoted in with `-m crypto` (BTC → BTC-USD).
# Env: STOCKWISE_QUOTE_CURRENCY. Flag: --quote-currency.
# quote_currency = "usd"

# Named portfolio to use ("default" is portfolio.json).
# Env: STOCKWISE_PORTFOLIO. Flag: --portfolio.
# portfolio = "default"

# AI provider for `report` and --ai: "ollama" (local, default), "openai",
# "anthropic" or "gemini". Cloud API keys stay in environment variables
# (OPENAI_API_KEY, ANTHROPIC_API_KEY, GEMINI_API_KEY). Env: STOCKWISE_AI_PROVIDER.
# ai_provider = "ollama"

# Market-data provider: "yahoo" (default), "alphavantage", "massive",
# "twelvedata", "finnhub", "fmp", or the exchanges' own websites: "nse",
# "bse" or "exchange" (NSE for .NS, BSE for .BO; no key). API keys stay in
# environment variables. Env: STOCKWISE_DATA_PROVIDER. FINNHUB_API_KEY and
# FMP_API_KEY also back Yahoo up without naming a provider here.
# data_provider = "yahoo"

# Default --period per command. Periods: 1d 5d 1w 1mo 3mo 6mo ytd 1y 2y 5y 10y max.
[periods]
# deep = "3mo"
# technical = "3mo"
# history = "1mo"
# risk = "1y"
# patterns = "1mo"
# support = "1mo"
# fibs = "1mo"
# volume = "1mo"
# gaps = "1mo"
# stoploss = "1mo"
# forecast = "6mo"
# options = "1mo"
# backtest = "2y"
# sip = "5y"

# Notification channels for `alert watch` and the daemon (`stockwise notify test`).
# Each key is the STOCKWISE_* variable without the prefix; the variable wins.
# If you put tokens or passwords here, keep this file private (chmod 600).
[notify]
# telegram_token = ""
# telegram_chat_id = ""
# slack_webhook = ""
# discord_webhook = ""
# smtp_host = "smtp.gmail.com"
# smtp_port = "587"
# smtp_tls = "starttls"
# smtp_user = ""
# smtp_password = ""
# email_from = ""
# email_to = ""
# desktop_notify = "1"
"#;

/// Write [`TEMPLATE`] to `path`, refusing to replace an existing file
/// unless `force`.
pub fn init(path: &Path, force: bool) -> Result<()> {
    if path.exists() && !force {
        bail!(
            "{} already exists — edit it, or pass --force to replace it with the template",
            path.display()
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    std::fs::write(path, TEMPLATE).with_context(|| format!("Cannot write {}", path.display()))
}

/// Set `market = "<value>"` in the config file at `path`, creating it from
/// [`TEMPLATE`] when missing. An active `market` line is replaced, else the
/// template's commented one, else a new line goes on top. Other lines and
/// comments are kept.
pub fn set_market(path: &Path, value: &str) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
    };
    let new_line = format!("market = \"{value}\"");
    let is_key = |l: &str, prefix: &str| {
        l.strip_prefix(prefix)
            .is_some_and(|r| r.trim_start().starts_with('='))
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    // Only top-level keys count: stop at the first table header.
    let top = lines
        .iter()
        .position(|l| l.trim_start().starts_with('['))
        .unwrap_or(lines.len());
    let at = lines[..top]
        .iter()
        .position(|l| is_key(l.trim_start(), "market"))
        .or_else(|| {
            lines[..top]
                .iter()
                .position(|l| is_key(l.trim_start(), "# market"))
        });
    match at {
        Some(i) => lines[i] = new_line,
        None => lines.insert(0, new_line),
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(path, out).with_context(|| format!("Cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_market_fills_the_template_line_and_parses() {
        let dir = std::env::temp_dir().join(format!("sw-setmarket-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("config.toml");
        set_market(&file, "us").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.lines().any(|l| l == "market = \"us\""), "{text}");
        assert_eq!(parse(&text, CMDS).unwrap().market.as_deref(), Some("us"));
        // Replaces an active line, keeps other settings.
        std::fs::write(
            &file,
            "market = \"in\"\nai = true\n[periods]\n# deep = \"3mo\"\n",
        )
        .unwrap();
        set_market(&file, "us").unwrap();
        let c = parse(&std::fs::read_to_string(&file).unwrap(), CMDS).unwrap();
        assert_eq!((c.market.as_deref(), c.ai), (Some("us"), Some(true)));
        // No market line at all: one is added.
        std::fs::write(&file, "ai = true\n").unwrap();
        set_market(&file, "in").unwrap();
        let c = parse(&std::fs::read_to_string(&file).unwrap(), CMDS).unwrap();
        assert_eq!((c.market.as_deref(), c.ai), (Some("in"), Some(true)));
        let _ = std::fs::remove_dir_all(&dir);
    }

    const CMDS: &[&str] = &[
        "deep",
        "technical",
        "history",
        "risk",
        "patterns",
        "support",
        "fibs",
        "volume",
        "gaps",
        "stoploss",
        "forecast",
        "options",
        "backtest",
        "sip",
    ];

    fn err(text: &str) -> String {
        format!("{:#}", parse(text, CMDS).unwrap_err())
    }

    #[test]
    fn template_parses_to_defaults() {
        assert_eq!(parse(TEMPLATE, CMDS).unwrap(), Config::default());
    }

    #[test]
    fn template_lists_every_period_command() {
        for cmd in CMDS {
            assert!(
                TEMPLATE.contains(&format!("# {} = ", cmd)),
                "template is missing periods.{cmd}"
            );
        }
    }

    #[test]
    fn full_config_parses() {
        let c = parse(
            r#"
            market = "US"
            ai = true
            model = "llama3.1:8b"
            ollama_host = "gpu-box:11434"
            redis_url = "off"
            home = "~/sw"
            [periods]
            technical = "6mo"
            backtest = "5y"
            "#,
            CMDS,
        )
        .unwrap();
        assert_eq!(c.market.as_deref(), Some("US"));
        assert_eq!(market_value(c.market.as_deref().unwrap()).unwrap(), "us");
        assert_eq!(c.ai, Some(true));
        assert_eq!(c.periods["technical"], "6mo");
        assert_eq!(c.periods["backtest"], "5y");
    }

    #[test]
    fn unknown_key_is_named() {
        let e = err("markt = \"us\"");
        assert!(e.contains("markt"), "{e}");
    }

    #[test]
    fn wrong_type_shows_the_line() {
        let e = err("ai = \"yes\"");
        assert!(e.contains("ai = \"yes\""), "{e}");
    }

    #[test]
    fn bad_market_is_named() {
        let e = err("market = \"mars\"");
        assert!(e.contains("`market`") && e.contains("mars"), "{e}");
        // Global exchanges and commodities are markets too.
        assert_eq!(market_value("UK").unwrap(), "uk");
        assert_eq!(market_value("japan").unwrap(), "jp");
        assert_eq!(market_value("commodity").unwrap(), "commodity");
    }

    #[test]
    fn bad_period_value_names_the_key() {
        let e = err("[periods]\ntechnical = \"7x\"");
        assert!(e.contains("periods.technical") && e.contains("7x"), "{e}");
    }

    #[test]
    fn period_for_command_without_period_is_rejected() {
        let e = err("[periods]\nquote = \"1y\"");
        assert!(e.contains("periods.quote"), "{e}");
    }

    #[test]
    fn empty_string_is_rejected() {
        let e = err("model = \"\"");
        assert!(e.contains("`model`"), "{e}");
    }

    #[test]
    fn read_names_the_file() {
        let dir = std::env::temp_dir().join(format!("stockwise-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        assert!(
            read(&path, CMDS).unwrap().is_none(),
            "missing file is no config"
        );
        std::fs::write(&path, "market = 1").unwrap();
        let e = format!("{:#}", read(&path, CMDS).unwrap_err());
        assert!(e.contains("config.toml"), "{e}");
    }

    #[test]
    fn init_refuses_to_overwrite_without_force() {
        let dir = std::env::temp_dir().join(format!("stockwise-init-{}", std::process::id()));
        let path = dir.join("nested").join("config.toml");
        let _ = std::fs::remove_file(&path);
        init(&path, false).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
        std::fs::write(&path, "market = \"us\"").unwrap();
        assert!(init(&path, false).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "market = \"us\"");
        init(&path, true).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), TEMPLATE);
    }

    #[test]
    fn env_beats_file_beats_default() {
        assert_eq!(
            resolve(Some("env".into()), Some("file")),
            Some(("env".into(), Source::Env))
        );
        assert_eq!(
            resolve(None, Some("file")),
            Some(("file".into(), Source::File))
        );
        assert_eq!(
            resolve(Some("  ".into()), Some("file")),
            Some(("file".into(), Source::File)),
            "an empty env var counts as unset"
        );
        assert_eq!(resolve(None, None), None);
    }

    #[test]
    fn newer_settings_parse_and_are_validated() {
        let cfg = parse(
            "lang = \"hi\"\nquote_currency = \"inr\"\nportfolio = \"long-term\"\n\
             ai_provider = \"anthropic\"\ndata_provider = \"twelvedata\"\n\
             [notify]\ntelegram_chat_id = \"42\"\nsmtp_host = \"smtp.example.com\"\n",
            CMDS,
        )
        .unwrap();
        assert_eq!(cfg.lang.as_deref(), Some("hi"));
        assert_eq!(
            cfg.notify.get("telegram_chat_id").map(String::as_str),
            Some("42")
        );
        assert!(parse("data_provider = \"yahoo\"\n", CMDS).is_ok());
        assert!(parse("data_provider = \"nse\"\n", CMDS).is_ok());
        assert!(parse("data_provider = \"exchange\"\n", CMDS).is_ok());

        assert!(err("lang = \"klingon\"\n").contains("`lang`"));
        assert!(err("quote_currency = \"dollars\"\n").contains("`quote_currency`"));
        assert!(err("portfolio = \"a/b\"\n").contains("`portfolio`"));
        assert!(err("ai_provider = \"skynet\"\n").contains("`ai_provider`"));
        assert!(err("data_provider = \"bloomberg\"\n").contains("`data_provider`"));
        let e = err("[notify]\ntelegram = \"x\"\n");
        assert!(
            e.contains("`notify.telegram`") && e.contains("telegram_token"),
            "{e}"
        );
    }

    #[test]
    fn notify_keys_mirror_the_env_vars() {
        assert_eq!(
            notify_key("STOCKWISE_SMTP_PASSWORD").as_deref(),
            Some("smtp_password")
        );
        assert!(is_secret_notify_key("smtp_password"));
        assert!(!is_secret_notify_key("smtp_host"));
        for (env, _) in crate::notify::ENV_VARS {
            let key = notify_key(env).unwrap();
            assert!(
                TEMPLATE.contains(&format!("# {} = ", key)),
                "template lacks {key}"
            );
        }
    }

    #[test]
    fn home_expands_tilde_only_at_the_start() {
        if let Some(home) = dirs::home_dir() {
            assert_eq!(expand_home("~"), home);
            assert_eq!(expand_home("~/sw"), home.join("sw"));
        }
        assert_eq!(expand_home("/data/sw"), PathBuf::from("/data/sw"));
        assert_eq!(expand_home("~bob/sw"), PathBuf::from("~bob/sw"));
    }
}
