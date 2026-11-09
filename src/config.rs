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
use std::sync::{OnceLock, RwLock};

/// Env var that points at another config file.
pub const ENV_VAR: &str = "STOCKWISE_CONFIG";

/// Env var that selects an investor profile (`passive`, `active:low`, `active:med`, `active:high`, `ultra`).
pub const PROFILE_ENV_VAR: &str = "STOCKWISE_PROFILE";

/// Investor profile mode (`passive`, `active`, or `ultra`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ProfileMode {
    /// Index, SIP, ETF & mutual fund savers (auto `--explain`, conservative glide paths).
    Passive,
    /// Core-and-satellite / fundamental / GARP or compounder stock & fund pickers.
    #[default]
    Active,
    /// Pro / quant / trader full-telemetry mode.
    Ultra,
}

impl ProfileMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passive => "passive",
            Self::Active => "active",
            Self::Ultra => "ultra",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "passive" => Ok(Self::Passive),
            "active" => Ok(Self::Active),
            "ultra" => Ok(Self::Ultra),
            other => Err(format!(
                "`{other}` is not a profile mode; use passive, active or ultra"
            )),
        }
    }

    pub const fn default_risk(self) -> RiskLevel {
        match self {
            Self::Passive => RiskLevel::Low,
            Self::Active => RiskLevel::Med,
            Self::Ultra => RiskLevel::High,
        }
    }
}

/// Risk tolerance tier (`low`, `med`, or `high`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum RiskLevel {
    Low,
    #[default]
    #[value(alias = "medium")]
    Med,
    High,
}

impl RiskLevel {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Med => "med",
            Self::High => "high",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" => Ok(Self::Low),
            "med" | "medium" => Ok(Self::Med),
            "high" => Ok(Self::High),
            other => Err(format!(
                "`{other}` is not a risk level; use low, med or high"
            )),
        }
    }
}

/// Indian income-tax regime (`new` or `old`).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum TaxRegime {
    #[default]
    New,
    Old,
}

impl TaxRegime {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::New => "new",
            Self::Old => "old",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "new" => Ok(Self::New),
            "old" => Ok(Self::Old),
            other => Err(format!("`{other}` is not a tax regime; use new or old")),
        }
    }
}

/// Parsed `<MODE[:RISK]>` profile specification (e.g. `passive`, `active:low`, `ultra`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileSpec {
    pub mode: ProfileMode,
    pub risk: Option<RiskLevel>,
}

impl ProfileSpec {
    pub const fn effective_risk(self) -> RiskLevel {
        match self.risk {
            Some(r) => r,
            None => self.mode.default_risk(),
        }
    }

    pub fn canonical(self) -> String {
        let risk = self.effective_risk();
        match self.mode {
            ProfileMode::Ultra if self.risk.is_none() || risk == RiskLevel::High => {
                "ultra".to_string()
            }
            _ => format!("{}:{}", self.mode.as_str(), risk.as_str()),
        }
    }
}

impl std::str::FromStr for ProfileSpec {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_profile_spec(s)
    }
}

/// Parse a `<MODE[:RISK]>` string such as `passive`, `active:low`, `active:med`, `active:high`, or `ultra`.
pub fn parse_profile_spec(s: &str) -> Result<ProfileSpec, String> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(
            "profile cannot be empty; use passive, active:low, active:med, active:high or ultra"
                .into(),
        );
    }
    if let Some((m, r)) = trimmed.split_once(':') {
        let mode = ProfileMode::parse(m)?;
        let risk = RiskLevel::parse(r)?;
        Ok(ProfileSpec {
            mode,
            risk: Some(risk),
        })
    } else {
        let mode = ProfileMode::parse(trimmed)?;
        Ok(ProfileSpec { mode, risk: None })
    }
}

/// Optional `[profile]` section in `config.toml`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileConfig {
    /// `"passive"`, `"active"` or `"ultra"` (or `"active:low"`, etc.).
    pub mode: Option<String>,
    /// `"low"`, `"med"` or `"high"`.
    pub risk: Option<String>,
    /// Marginal income-tax slab in percent (`0..=50`, e.g. `30`).
    pub tax_slab: Option<f64>,
    /// `"new"` or `"old"` (India).
    pub tax_regime: Option<String>,
    /// Default investment horizon in years (`1..=60`, default `10`).
    pub horizon_years: Option<u32>,
    /// Single-stock look-through weight cap in percent (`1..=100`).
    pub max_stock_weight: Option<f64>,
    /// Combined Small + Micro cap weight guardrail in percent (`0..=100`).
    pub max_smallcap_weight: Option<f64>,
    /// Portfolio capital risked per trade in `stoploss`, in percent (`0.05..=20`).
    pub risk_per_trade: Option<f64>,
}

impl ProfileConfig {
    pub const fn is_empty(&self) -> bool {
        self.mode.is_none()
            && self.risk.is_none()
            && self.tax_slab.is_none()
            && self.tax_regime.is_none()
            && self.horizon_years.is_none()
            && self.max_stock_weight.is_none()
            && self.max_smallcap_weight.is_none()
            && self.risk_per_trade.is_none()
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(m) = &self.mode {
            parse_profile_spec(m).map_err(|e| anyhow::anyhow!("`profile.mode`: {e}"))?;
        }
        if let Some(r) = &self.risk {
            RiskLevel::parse(r).map_err(|e| anyhow::anyhow!("`profile.risk`: {e}"))?;
        }
        if let Some(s) = self.tax_slab
            && (!s.is_finite() || !(0.0..=50.0).contains(&s))
        {
            bail!("`profile.tax_slab` = {s} must be between 0 and 50");
        }
        if let Some(tr) = &self.tax_regime {
            TaxRegime::parse(tr).map_err(|e| anyhow::anyhow!("`profile.tax_regime`: {e}"))?;
        }
        if let Some(y) = self.horizon_years
            && !(1..=60).contains(&y)
        {
            bail!("`profile.horizon_years` = {y} must be between 1 and 60");
        }
        if let Some(w) = self.max_stock_weight
            && (!w.is_finite() || !(1.0..=100.0).contains(&w))
        {
            bail!("`profile.max_stock_weight` = {w} must be between 1 and 100");
        }
        if let Some(w) = self.max_smallcap_weight
            && (!w.is_finite() || !(0.0..=100.0).contains(&w))
        {
            bail!("`profile.max_smallcap_weight` = {w} must be between 0 and 100");
        }
        if let Some(r) = self.risk_per_trade
            && (!r.is_finite() || !(0.05..=20.0).contains(&r))
        {
            bail!("`profile.risk_per_trade` = {r} must be between 0.05 and 20");
        }
        Ok(())
    }
}

/// Archetype defaults for `(mode, risk)`:
/// `(label, max_stock_weight, max_smallcap_weight, risk_per_trade, default_drift_pct, screen_presets)`.
pub fn archetype_defaults(
    mode: ProfileMode,
    risk: RiskLevel,
) -> (&'static str, f64, f64, f64, f64, &'static [&'static str]) {
    match (mode, risk) {
        (ProfileMode::Passive, RiskLevel::Low) => (
            "Index, SIP, ETF & Mutual Fund Saver",
            10.0,
            10.0,
            0.5,
            5.0,
            &["bluechip", "dividend", "undervalued"],
        ),
        (ProfileMode::Passive, RiskLevel::Med | RiskLevel::High) => (
            "Index, SIP, ETF & Mutual Fund Saver",
            10.0,
            15.0,
            0.5,
            5.0,
            &["bluechip", "dividend", "undervalued"],
        ),
        (ProfileMode::Active, RiskLevel::Low) => (
            "Conservative Stock & Fund Picker",
            8.0,
            10.0,
            0.5,
            0.0,
            &[
                "quality-moat",
                "dividend-leaders",
                "bluechip",
                "piotroski-strong",
            ],
        ),
        (ProfileMode::Active, RiskLevel::Med) => (
            "Balanced Core-and-Satellite / GARP Investor",
            12.0,
            25.0,
            1.0,
            0.0,
            &["garp", "coffee-can", "undervalued", "momentum"],
        ),
        (ProfileMode::Active, RiskLevel::High) => (
            "Aggressive Growth / Small-Mid Compounder & Swing Trader",
            20.0,
            50.0,
            2.0,
            0.0,
            &["compounders", "stage2-uptrend", "canslim", "52w-breakout"],
        ),
        (ProfileMode::Ultra, _) => (
            "Pro / Quant / Trader Full-Telemetry Mode",
            20.0,
            50.0,
            2.0,
            0.0,
            &["momentum", "stage2-uptrend", "canslim", "52w-breakout"],
        ),
    }
}

/// Effective investor profile and personal guardrails after applying
/// `--profile` flag > `STOCKWISE_PROFILE` env > `[profile]` in `config.toml` > archetype defaults.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EffectiveProfile {
    pub mode: ProfileMode,
    pub risk: RiskLevel,
    pub spec: String,
    pub label: &'static str,
    pub mode_source: Source,
    pub risk_source: Source,
    pub tax_slab: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tax_slab_configured: Option<f64>,
    pub tax_slab_source: Source,
    pub tax_regime: TaxRegime,
    pub tax_regime_source: Source,
    pub horizon_years: u32,
    pub horizon_years_source: Source,
    pub max_stock_weight: f64,
    pub max_stock_weight_source: Source,
    pub max_smallcap_weight: f64,
    pub max_smallcap_weight_source: Source,
    pub risk_per_trade: f64,
    pub risk_per_trade_source: Source,
    pub default_drift_pct: f64,
    pub auto_explain: bool,
    pub screen_presets: &'static [&'static str],
}

impl Default for EffectiveProfile {
    fn default() -> Self {
        Self::from_parts(None, None, &ProfileConfig::default())
    }
}

impl EffectiveProfile {
    /// Resolve effective profile settings from explicit inputs.
    pub fn from_parts(
        cli_spec: Option<(ProfileSpec, Source)>,
        env_val: Option<&str>,
        file_cfg: &ProfileConfig,
    ) -> Self {
        let file_spec = file_cfg
            .mode
            .as_deref()
            .and_then(|m| parse_profile_spec(m).ok());
        let file_risk = file_cfg
            .risk
            .as_deref()
            .and_then(|r| RiskLevel::parse(r).ok())
            .or_else(|| file_spec.and_then(|s| s.risk));

        let env_spec = env_val
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .and_then(|s| parse_profile_spec(s).ok());

        let (mode, mode_source, risk, risk_source) =
            if let Some((spec, _)) = cli_spec.filter(|(_, s)| *s == Source::Flag) {
                let r = spec.effective_risk();
                (spec.mode, Source::Flag, r, Source::Flag)
            } else if let Some(spec) = env_spec {
                let r = spec.effective_risk();
                (spec.mode, Source::Env, r, Source::Env)
            } else if let Some(spec) = file_spec {
                let (r, r_src) = match file_risk {
                    Some(r) => (r, Source::File),
                    None => (spec.mode.default_risk(), Source::File),
                };
                (spec.mode, Source::File, r, r_src)
            } else if let Some(r) = file_risk {
                (ProfileMode::Active, Source::Default, r, Source::File)
            } else {
                (
                    ProfileMode::Active,
                    Source::Default,
                    RiskLevel::Med,
                    Source::Default,
                )
            };

        let (
            label,
            def_stock_wt,
            def_smallcap_wt,
            def_risk_trade,
            default_drift_pct,
            screen_presets,
        ) = archetype_defaults(mode, risk);

        // When `--profile` or `STOCKWISE_PROFILE` overrides the archetype on the
        // command line / env, use that archetype's guardrails unless the file's
        // mode/risk matches the override.
        let file_matches_archetype = match (mode_source, file_spec) {
            (Source::Flag | Source::Env, Some(fs)) => {
                fs.mode == mode && file_risk.unwrap_or_else(|| fs.mode.default_risk()) == risk
            }
            (Source::Flag | Source::Env, None) => false,
            _ => true,
        };
        let fallback_src = match mode_source {
            Source::Flag => Source::Flag,
            Source::Env => Source::Env,
            Source::File => Source::Default,
            Source::Default => Source::Default,
        };

        let (max_stock_weight, max_stock_weight_source) = match file_cfg.max_stock_weight {
            Some(w) if file_matches_archetype => (w, Source::File),
            _ => (def_stock_wt, fallback_src),
        };
        let (max_smallcap_weight, max_smallcap_weight_source) = match file_cfg.max_smallcap_weight {
            Some(w) if file_matches_archetype => (w, Source::File),
            _ => (def_smallcap_wt, fallback_src),
        };
        let (risk_per_trade, risk_per_trade_source) = match file_cfg.risk_per_trade {
            Some(r) if file_matches_archetype => (r, Source::File),
            _ => (def_risk_trade, fallback_src),
        };

        let (tax_slab, tax_slab_configured, tax_slab_source) = match file_cfg.tax_slab {
            Some(s) => (s, Some(s), Source::File),
            None => (30.0, None, Source::Default),
        };
        let (tax_regime, tax_regime_source) = match file_cfg
            .tax_regime
            .as_deref()
            .and_then(|t| TaxRegime::parse(t).ok())
        {
            Some(tr) => (tr, Source::File),
            None => (TaxRegime::New, Source::Default),
        };
        let (horizon_years, horizon_years_source) = match file_cfg.horizon_years {
            Some(y) => (y, Source::File),
            None => (10, Source::Default),
        };

        let spec = ProfileSpec {
            mode,
            risk: Some(risk),
        }
        .canonical();

        Self {
            mode,
            risk,
            spec,
            label,
            mode_source,
            risk_source,
            tax_slab,
            tax_slab_configured,
            tax_slab_source,
            tax_regime,
            tax_regime_source,
            horizon_years,
            horizon_years_source,
            max_stock_weight,
            max_stock_weight_source,
            max_smallcap_weight,
            max_smallcap_weight_source,
            risk_per_trade,
            risk_per_trade_source,
            default_drift_pct,
            auto_explain: mode == ProfileMode::Passive,
            screen_presets,
        }
    }
}

static CLI_PROFILE: RwLock<Option<(ProfileSpec, Source)>> = RwLock::new(None);

/// Set or clear the process-wide CLI `--profile` override.
pub fn set_cli_profile(spec: Option<ProfileSpec>, source: Source) {
    if let Ok(mut guard) = CLI_PROFILE.write() {
        *guard = spec.map(|s| (s, source));
    }
}

/// Read the current process-wide CLI `--profile` override.
pub fn cli_profile() -> Option<(ProfileSpec, Source)> {
    CLI_PROFILE.read().ok().and_then(|g| *g)
}

/// Resolve the active [`EffectiveProfile`] for this process.
pub fn effective_profile() -> EffectiveProfile {
    let cli = cli_profile();
    let env_val = std::env::var(PROFILE_ENV_VAR).ok();
    EffectiveProfile::from_parts(cli, env_val.as_deref(), &get().profile)
}

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
    /// Investor profile and personal guardrails (`[profile]`).
    #[serde(default)]
    pub profile: ProfileConfig,
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
    profile: ProfileConfig {
        mode: None,
        risk: None,
        tax_slab: None,
        tax_regime: None,
        horizon_years: None,
        max_stock_weight: None,
        max_smallcap_weight: None,
        risk_per_trade: None,
    },
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
        self.profile.validate()?;
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

# Investor profile and personal guardrails (`stockwise profile`).
# Env: STOCKWISE_PROFILE (e.g. "passive", "active:low", "active:med", "active:high", "ultra").
# Flag: --profile <MODE[:RISK]>.
[profile]
# mode = "active"
# risk = "med"
# tax_slab = 30
# tax_regime = "new"
# horizon_years = 10
# max_stock_weight = 12.0
# max_smallcap_weight = 25.0
# risk_per_trade = 1.0
"#;

/// Updates to apply to the `[profile]` section of `config.toml`.
#[derive(Debug, Default, Clone)]
pub struct ProfileUpdate {
    pub mode: Option<ProfileMode>,
    pub risk: Option<RiskLevel>,
    pub tax_slab: Option<f64>,
    pub tax_regime: Option<TaxRegime>,
    pub horizon_years: Option<u32>,
    pub max_stock_weight: Option<f64>,
    pub max_smallcap_weight: Option<f64>,
    pub risk_per_trade: Option<f64>,
}

/// Update `[profile]` settings in the config file at `path`, creating it from
/// [`TEMPLATE`] when missing. Preserves comments and other sections.
pub fn set_profile(path: &Path, update: &ProfileUpdate) -> Result<()> {
    let probe = ProfileConfig {
        mode: update.mode.map(|m| m.as_str().to_string()),
        risk: update.risk.map(|r| r.as_str().to_string()),
        tax_slab: update.tax_slab,
        tax_regime: update.tax_regime.map(|t| t.as_str().to_string()),
        horizon_years: update.horizon_years,
        max_stock_weight: update.max_stock_weight,
        max_smallcap_weight: update.max_smallcap_weight,
        risk_per_trade: update.risk_per_trade,
    };
    probe.validate()?;

    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => TEMPLATE.to_string(),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
    };

    let mut pairs: Vec<(&str, String)> = Vec::new();
    if let Some(m) = update.mode {
        pairs.push(("mode", format!("\"{}\"", m.as_str())));
    }
    if let Some(r) = update.risk {
        pairs.push(("risk", format!("\"{}\"", r.as_str())));
    }
    if let Some(s) = update.tax_slab {
        let formatted = if (s.fract()).abs() < 1e-9 {
            format!("{}", s as i64)
        } else {
            format!("{s}")
        };
        pairs.push(("tax_slab", formatted));
    }
    if let Some(tr) = update.tax_regime {
        pairs.push(("tax_regime", format!("\"{}\"", tr.as_str())));
    }
    if let Some(y) = update.horizon_years {
        pairs.push(("horizon_years", y.to_string()));
    }
    if let Some(w) = update.max_stock_weight {
        pairs.push(("max_stock_weight", format!("{w:.1}")));
    }
    if let Some(w) = update.max_smallcap_weight {
        pairs.push(("max_smallcap_weight", format!("{w:.1}")));
    }
    if let Some(r) = update.risk_per_trade {
        pairs.push(("risk_per_trade", format!("{r}")));
    }

    let is_key = |l: &str, prefix: &str| {
        l.strip_prefix(prefix)
            .is_some_and(|r| r.trim_start().starts_with('='))
    };

    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    let header_idx = match lines.iter().position(|l| l.trim() == "[profile]") {
        Some(idx) => idx,
        None => {
            if !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            lines.push("[profile]".to_string());
            lines.len() - 1
        }
    };

    for (key, val_str) in pairs {
        let new_line = format!("{key} = {val_str}");
        let end_idx = lines[header_idx + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map(|rel| header_idx + 1 + rel)
            .unwrap_or(lines.len());
        let commented = format!("# {key}");
        let found = lines[header_idx + 1..end_idx]
            .iter()
            .position(|l| is_key(l.trim_start(), key))
            .or_else(|| {
                lines[header_idx + 1..end_idx]
                    .iter()
                    .position(|l| is_key(l.trim_start(), &commented))
            })
            .map(|rel| header_idx + 1 + rel);
        match found {
            Some(i) => lines[i] = new_line,
            None => lines.insert(end_idx, new_line),
        }
    }

    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("Cannot create directory {}", dir.display()))?;
    }
    let mut out = lines.join("\n");
    out.push('\n');
    std::fs::write(path, out).with_context(|| format!("Cannot write {}", path.display()))
}

/// Comment out all active keys inside `[profile]` at `path`, restoring built-in defaults.
pub fn reset_profile(path: &Path) -> Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("Cannot read {}", path.display())),
    };
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    if let Some(header_idx) = lines.iter().position(|l| l.trim() == "[profile]") {
        let end_idx = lines[header_idx + 1..]
            .iter()
            .position(|l| l.trim_start().starts_with('['))
            .map(|rel| header_idx + 1 + rel)
            .unwrap_or(lines.len());
        for line in &mut lines[header_idx + 1..end_idx] {
            let trimmed = line.trim_start();
            if !trimmed.is_empty() && !trimmed.starts_with('#') {
                *line = format!("# {trimmed}");
            }
        }
        let mut out = lines.join("\n");
        out.push('\n');
        std::fs::write(path, out).with_context(|| format!("Cannot write {}", path.display()))?;
    }
    Ok(())
}

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

    #[test]
    fn profile_spec_and_archetype_precedence_resolve_correctly() {
        let passive = parse_profile_spec("passive").unwrap();
        assert_eq!(passive.mode, ProfileMode::Passive);
        assert_eq!(passive.effective_risk(), RiskLevel::Low);
        assert_eq!(passive.canonical(), "passive:low");

        let active_low = parse_profile_spec("active:low").unwrap();
        assert_eq!(active_low.mode, ProfileMode::Active);
        assert_eq!(active_low.effective_risk(), RiskLevel::Low);

        let ultra = parse_profile_spec("ultra").unwrap();
        assert_eq!(ultra.mode, ProfileMode::Ultra);
        assert_eq!(ultra.effective_risk(), RiskLevel::High);
        assert_eq!(ultra.canonical(), "ultra");

        assert!(parse_profile_spec("yolo").is_err());

        // Built-in default is active:med.
        let def = EffectiveProfile::from_parts(None, None, &ProfileConfig::default());
        assert_eq!(def.mode, ProfileMode::Active);
        assert_eq!(def.risk, RiskLevel::Med);
        assert_eq!(def.max_stock_weight, 12.0);
        assert_eq!(def.max_smallcap_weight, 25.0);
        assert_eq!(def.risk_per_trade, 1.0);
        assert!(!def.auto_explain);

        // File config sets active:low with custom tax_slab.
        let file_cfg = ProfileConfig {
            mode: Some("active".into()),
            risk: Some("low".into()),
            tax_slab: Some(20.0),
            tax_regime: Some("old".into()),
            horizon_years: Some(15),
            max_stock_weight: None,
            max_smallcap_weight: None,
            risk_per_trade: None,
        };
        let eff_file = EffectiveProfile::from_parts(None, None, &file_cfg);
        assert_eq!(eff_file.mode, ProfileMode::Active);
        assert_eq!(eff_file.risk, RiskLevel::Low);
        assert_eq!(eff_file.max_stock_weight, 8.0);
        assert_eq!(eff_file.max_smallcap_weight, 10.0);
        assert_eq!(eff_file.risk_per_trade, 0.5);
        assert_eq!(eff_file.tax_slab, 20.0);
        assert_eq!(eff_file.tax_regime, TaxRegime::Old);
        assert_eq!(eff_file.horizon_years, 15);

        // Env beats file.
        let eff_env = EffectiveProfile::from_parts(None, Some("passive"), &file_cfg);
        assert_eq!(eff_env.mode, ProfileMode::Passive);
        assert_eq!(eff_env.mode_source, Source::Env);
        assert_eq!(eff_env.max_stock_weight, 10.0);
        assert!(eff_env.auto_explain);

        // CLI flag beats env and file.
        let eff_flag =
            EffectiveProfile::from_parts(Some((ultra, Source::Flag)), Some("passive"), &file_cfg);
        assert_eq!(eff_flag.mode, ProfileMode::Ultra);
        assert_eq!(eff_flag.mode_source, Source::Flag);
        assert_eq!(eff_flag.max_stock_weight, 20.0);
        assert_eq!(eff_flag.max_smallcap_weight, 50.0);
        assert_eq!(eff_flag.risk_per_trade, 2.0);
    }

    #[test]
    fn set_profile_and_reset_profile_preserve_comments_and_validate() {
        let dir = std::env::temp_dir().join(format!("sw-setprofile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let file = dir.join("config.toml");

        set_profile(
            &file,
            &ProfileUpdate {
                mode: Some(ProfileMode::Active),
                risk: Some(RiskLevel::Low),
                tax_slab: Some(30.0),
                ..Default::default()
            },
        )
        .unwrap();

        let text = std::fs::read_to_string(&file).unwrap();
        let cfg = parse(&text, CMDS).unwrap();
        assert_eq!(cfg.profile.mode.as_deref(), Some("active"));
        assert_eq!(cfg.profile.risk.as_deref(), Some("low"));
        assert_eq!(cfg.profile.tax_slab, Some(30.0));

        reset_profile(&file).unwrap();
        let reset_text = std::fs::read_to_string(&file).unwrap();
        let reset_cfg = parse(&reset_text, CMDS).unwrap();
        assert!(reset_cfg.profile.is_empty());

        assert!(err("[profile]\nmode = \" invalid \"\n").contains("profile.mode"));
        assert!(err("[profile]\ntax_slab = 90\n").contains("profile.tax_slab"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
