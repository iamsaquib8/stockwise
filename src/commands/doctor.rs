//! `stockwise doctor`: one screen (or one JSON document) that answers "is
//! my setup working?", with a fix for each problem it finds.

use super::*;
use serde::Serialize;

/// How one check came out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    /// Neither good nor bad: optional and not set up, or plain facts.
    Info,
}

/// One line of the report.
#[derive(Clone, Debug, Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Remove ANSI colour codes.
fn plain(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC [ ... final byte in @..~
            if chars.peek() == Some(&'[') {
                chars.next();
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// A line's status from its leading mark (✓, !, ✗); unmarked lines are info.
fn status_of(line: &str) -> (CheckStatus, String) {
    let text = plain(line);
    let text = text.trim();
    for (mark, status) in [
        ("✓", CheckStatus::Ok),
        ("!", CheckStatus::Warn),
        ("✗", CheckStatus::Fail),
    ] {
        if let Some(rest) = text.strip_prefix(mark) {
            return (status, rest.trim().to_string());
        }
    }
    (CheckStatus::Info, text.to_string())
}

/// Prints each check as it comes (table) or collects them (JSON).
struct Report {
    json: bool,
    checks: Vec<Check>,
    last_name: String,
}

impl Report {
    fn new(json: bool) -> Self {
        Self {
            json,
            checks: Vec::new(),
            last_name: String::new(),
        }
    }

    /// One labelled line; an empty label continues the previous check.
    fn kv(&mut self, label: &str, value: &str) {
        if !self.json {
            print_kv(label, value);
        }
        let (status, detail) = status_of(value);
        if !label.is_empty() {
            self.last_name = plain(label);
        }
        self.checks.push(Check {
            name: self.last_name.clone(),
            status,
            detail,
            fix: None,
        });
    }

    /// The fix for the last check.
    fn fix(&mut self, hint: &str) {
        if !self.json {
            println!("  {:<28} {}", "", format!("fix: {hint}").dimmed());
        }
        if let Some(last) = self.checks.last_mut() {
            last.fix = Some(hint.to_string());
        }
    }

    /// End the table, or print the JSON document.
    fn finish(&self) -> Result<()> {
        if self.json {
            let problems = self
                .checks
                .iter()
                .filter(|c| matches!(c.status, CheckStatus::Warn | CheckStatus::Fail))
                .count();
            output::print_json(&serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "ok": !self.checks.iter().any(|c| c.status == CheckStatus::Fail),
                "problems": problems,
                "checks": self.checks,
            }))?;
        } else {
            println!();
        }
        Ok(())
    }
}

/// Whether one of the provider's key variables is set (never its value).
fn provider_key_set(kind: crate::providers::ProviderKind) -> bool {
    kind.key_envs()
        .iter()
        .any(|k| std::env::var(k).is_ok_and(|v| !v.trim().is_empty()))
}

pub async fn cmd_doctor(
    market: Market,
    sources: crate::config::CliSources,
    json: bool,
) -> Result<()> {
    let ok = |s: &str| format!("{} {}", "✓".green(), s);
    let warn = |s: &str| format!("{} {}", "!".yellow(), s);
    let bad = |s: &str| format!("{} {}", "✗".red(), s);

    let mut d = Report::new(json);
    if !json {
        print_header(&tf(Msg::DoctorTitle, &[&env!("CARGO_PKG_VERSION")]));
    }

    // Config file and defaults
    let cfg = crate::config::state();
    let cfg_path = cfg
        .and_then(|s| s.path.as_ref())
        .map_or("unknown (no config directory)".to_string(), |p| {
            p.display().to_string()
        });
    let cfg_invalid = match cfg.map(|s| &s.status) {
        Some(crate::config::Status::Loaded) => {
            d.kv("Config file", &ok(&format!("{} (loaded)", cfg_path)));
            if let Some(p) = crate::config::exposed_secrets() {
                d.kv(
                    "",
                    &warn("holds notification secrets and other users can read it"),
                );
                d.fix(&format!("chmod 600 {}", p.display()));
            }
            None
        }
        Some(crate::config::Status::Invalid(msg)) => {
            d.kv("Config file", &bad(&format!("{} (invalid)", cfg_path)));
            for line in msg.lines() {
                d.kv("", &line.red().to_string());
            }
            d.fix("fix the file, or replace it with `stockwise config init --force`");
            Some(cfg_path)
        }
        _ => {
            d.kv(
                "Config file",
                &format!(
                    "{} {}",
                    cfg_path,
                    "(none; optional — `stockwise config init` creates one)".dimmed()
                ),
            );
            None
        }
    };
    let market_src = match sources.market {
        crate::config::Source::Flag => "from -m/--market",
        crate::config::Source::Env => t(Msg::DoctorMarketFromEnv),
        crate::config::Source::File => "from the config file",
        crate::config::Source::Default => t(Msg::DoctorMarketDefault),
    };
    let market_name = match market {
        Market::In => "in (India, NSE)".to_string(),
        Market::Crypto => format!("crypto (vs {})", market::quote_currency()),
        Market::Commodity => "commodity (COMEX gold/silver, NSE ETFs and SGBs)".to_string(),
        Market::Global(x) => {
            let info = x.info();
            format!("{} ({}, {})", info.code, info.country, info.exchange_name)
        }
        Market::Us | Market::Fx => market.code().to_string(),
    };
    d.kv(
        t(Msg::DoctorMarket),
        &format!("{} — {}", market_name, market_src),
    );
    let lang_src = t(match crate::i18n::lang_source() {
        crate::i18n::LangSource::Flag => Msg::DoctorLangFlag,
        crate::i18n::LangSource::StockwiseLang => Msg::DoctorLangEnv,
        crate::i18n::LangSource::Config => Msg::DoctorLangConfig,
        crate::i18n::LangSource::Locale => Msg::DoctorLangLocale,
        crate::i18n::LangSource::Default => Msg::DoctorLangDefault,
    });
    d.kv(
        t(Msg::DoctorLanguage),
        &format!("{} — {}", crate::i18n::lang().code(), lang_src),
    );

    // Data directory
    match crate::store::data_dir() {
        Ok(dir) => {
            d.kv(t(Msg::DoctorDataDir), &ok(&dir.display().to_string()));
            let files = [
                "portfolio.json",
                "watchlist.json",
                "alerts.json",
                "sim_history.json",
                "wealth_history.json",
            ];
            let present: Vec<&str> = files
                .iter()
                .copied()
                .filter(|f| dir.join(f).exists())
                .collect();
            d.kv(
                t(Msg::DoctorDataFiles),
                &if present.is_empty() {
                    t(Msg::DoctorNoFilesYet).to_string()
                } else {
                    present.join(", ")
                },
            );
        }
        Err(e) => d.kv(t(Msg::DoctorDataDir), &bad(&format!("{:#}", e))),
    }

    // Market data: Yahoo is always needed (fundamentals, news, fallback).
    let started = std::time::Instant::now();
    let yahoo = match YahooClient::new().await {
        Ok(client) => client
            .yahoo_quote(&["^GSPC"])
            .await
            .map(|q| !q.is_empty())
            .unwrap_or(false),
        Err(_) => false,
    };
    d.kv(
        "Yahoo Finance",
        &if yahoo {
            ok(&tf(Msg::DoctorReachable, &[&started.elapsed().as_millis()]))
        } else {
            bad(t(Msg::DoctorYahooUnreachable))
        },
    );
    if !yahoo {
        d.fix(
            "check your internet connection or proxy (HTTPS_PROXY), then run \
             `stockwise quote TCS --verbose` to see each request",
        );
    }

    // New release: checked at most once a week (cached in the data directory).
    // Only `doctor` and `update` look; no other command checks for updates.
    d.kv("StockWise release", &super::doctor_version_line().await);

    // Optional data provider
    use crate::providers::{MarketData, Selection};
    let provider_lines = match Selection::from_env() {
        Selection::Yahoo => vec![t(Msg::DoctorProviderDefault).dimmed().to_string()],
        Selection::Unknown(value) => vec![warn(&tf(Msg::DoctorProviderUnknown, &[&value]))],
        Selection::MissingKey(kind) => vec![warn(&tf(
            Msg::DoctorProviderNoKey,
            &[&kind.name(), &kind.key_env()],
        ))],
        Selection::Ready(provider) => {
            // One probe per source: NSE and BSE separately in mixed mode.
            let mut lines = Vec::new();
            for (source, probe) in provider.probes() {
                let started = std::time::Instant::now();
                lines.push(match provider.quotes(&[probe]).await {
                    Ok(q) if q.iter().any(|q| q.regular_market_price.is_some()) => ok(&tf(
                        Msg::DoctorProviderAnswered,
                        &[&source, &probe, &started.elapsed().as_millis()],
                    )),
                    Ok(_) => warn(&tf(
                        Msg::DoctorProviderFellBack,
                        &[&source, &"no price returned"],
                    )),
                    Err(e) => warn(&tf(
                        Msg::DoctorProviderFellBack,
                        &[&source, &format!("{:#}", e)],
                    )),
                });
            }
            if !provider.kind().needs_key() {
                lines.push(t(Msg::DoctorExchangeNote).dimmed().to_string());
            }
            lines
        }
    };
    for (i, line) in provider_lines.iter().enumerate() {
        d.kv(
            if i == 0 {
                t(Msg::DoctorDataProvider)
            } else {
                ""
            },
            line,
        );
    }
    match Selection::from_env() {
        Selection::Unknown(_) => {
            d.fix("set STOCKWISE_DATA_PROVIDER to a provider `stockwise doctor` lists, or unset it")
        }
        Selection::MissingKey(kind) => d.fix(&format!("set {}", kind.key_env())),
        _ => {}
    }

    // Providers that back Yahoo up: the keyed ones (Finnhub, FMP) and the
    // keyless backups (Frankfurter for FX, CoinGecko for crypto). One probe
    // each, the key never shown.
    {
        use crate::providers::router::Router;
        let primary = match Selection::from_env() {
            Selection::Ready(p) => Some(p.kind()),
            _ => None,
        };
        let router = Router::from_env(primary, None);
        let mut first = true;
        for kind in [
            crate::providers::ProviderKind::Finnhub,
            crate::providers::ProviderKind::Fmp,
            crate::providers::ProviderKind::Frankfurter,
            crate::providers::ProviderKind::CoinGecko,
        ] {
            if Some(kind) == primary {
                continue;
            }
            let line = match router.members().iter().find(|p| p.kind() == kind) {
                None if kind.is_keyless_backup() => format!(
                    "{} {}",
                    kind.name(),
                    "turned off (STOCKWISE_KEYLESS_BACKUPS)".dimmed()
                ),
                None => format!(
                    "{} {}",
                    kind.name(),
                    format!(
                        "not configured (set {} to use it as a backup)",
                        kind.key_env()
                    )
                    .dimmed()
                ),
                Some(provider) => {
                    let probe = provider.probes().first().map_or("AAPL", |p| p.1);
                    let started = std::time::Instant::now();
                    let line = match provider.quotes(&[probe]).await {
                        Ok(q) if q.iter().any(|q| q.regular_market_price.is_some()) => ok(&tf(
                            Msg::DoctorProviderAnswered,
                            &[&kind.name(), &probe, &started.elapsed().as_millis()],
                        )),
                        Ok(_) => warn(&tf(
                            Msg::DoctorProviderFellBack,
                            &[&kind.name(), &"no price returned"],
                        )),
                        Err(e) => warn(&tf(
                            Msg::DoctorProviderFellBack,
                            &[&kind.name(), &format!("{:#}", e)],
                        )),
                    };
                    // CoinGecko's key is optional: say which way it runs.
                    if kind == crate::providers::ProviderKind::CoinGecko {
                        let how = if provider_key_set(kind) {
                            "Demo key from COINGECKO_API_KEY"
                        } else {
                            "keyless; COINGECKO_API_KEY takes an optional free Demo key"
                        };
                        format!("{line} {}", format!("({how})").dimmed())
                    } else {
                        line
                    }
                }
            };
            d.kv(if first { "Backup providers" } else { "" }, &line);
            first = false;
        }
    }

    // Cache
    let redis = crate::api::connect_redis().await.is_some();
    d.kv(
        t(Msg::DoctorRedis),
        &if redis {
            ok(t(Msg::DoctorConnected))
        } else {
            t(Msg::DoctorRedisNotUsed).dimmed().to_string()
        },
    );

    match crate::db::http_stats() {
        Some(st) => d.kv(
            "Cache database",
            &ok(&format!(
                "cache.db ({} entries, {} KiB)",
                st.entries,
                st.file_bytes / 1024
            )),
        ),
        None => d.kv(
            "Cache database",
            &"off or unavailable (in-memory cache only)"
                .dimmed()
                .to_string(),
        ),
    }

    // AI — provider and model, never the key itself.
    let ai = crate::ai::AiClient::connect().await;
    let ai_line = if ai.config_error().is_some() {
        let value = std::env::var("STOCKWISE_AI_PROVIDER").unwrap_or_default();
        bad(&tf(Msg::DoctorAiUnknown, &[&value.trim()]))
    } else {
        let provider = ai.provider();
        match provider.key_env() {
            Some(key_env) if ai.has_api_key() => ok(&tf(
                Msg::DoctorCloudReady,
                &[&provider.name(), &ai.model(), &key_env],
            )),
            Some(key_env) => warn(&tf(Msg::DoctorCloudNoKey, &[&provider.name(), &key_env])),
            None => match ai.has_model().await {
                Some(true) => ok(&tf(Msg::DoctorOllamaReady, &[&ai.model()])),
                Some(false) => warn(&tf(Msg::DoctorOllamaNoModel, &[&ai.model()])),
                None => t(Msg::DoctorOllamaDown).dimmed().to_string(),
            },
        }
    };
    d.kv(t(Msg::DoctorAi), &ai_line);
    if ai.config_error().is_some() {
        d.fix("set STOCKWISE_AI_PROVIDER to ollama, anthropic, openai or gemini");
    } else if let Some(key_env) = ai.provider().key_env() {
        if !ai.has_api_key() {
            d.fix(&format!("set {key_env}"));
        }
    } else if ai.has_model().await == Some(false) {
        d.fix(&format!("ollama pull {}", ai.model()));
    }

    // Brokers: Angel One always; the others once they have a config file.
    // Read-only: doctor never moves secrets into the keyring.
    let backend = crate::brokers::secrets::backend();
    d.kv(
        t(Msg::DoctorKeyring),
        &match backend.available() {
            Ok(()) => ok(&tf(Msg::DoctorKeyringAvailable, &[&backend.name()])),
            Err(reason) => tf(Msg::DoctorKeyringNotUsed, &[&reason])
                .dimmed()
                .to_string(),
        },
    );
    for kind in crate::brokers::BrokerKind::ALL {
        let line = match crate::brokers::config_status(kind, false) {
            Ok(st) if !st.exists && kind != crate::brokers::BrokerKind::Angel => continue,
            Ok(st) if st.configured => {
                let limits = crate::brokers::order_limits(kind).unwrap_or_default();
                let mut extra = limits
                    .max_order_value
                    .map(|v| tf(Msg::DoctorAngelMaxOrder, &[&format!("{:.0}", v)]))
                    .unwrap_or_default();
                if let Some(s) = &st.session {
                    extra.push_str(&format!(", {s}"));
                }
                match (kind.experimental(), limits.enable_orders) {
                    (true, true) => extra.push_str(&format!(", {}", t(Msg::DoctorBrokerOrdersOn))),
                    (true, false) => {
                        extra.push_str(&format!(", {}", t(Msg::DoctorBrokerOrdersOff)))
                    }
                    (false, true) => extra.push_str(&format!(", {}", t(Msg::DoctorAngelStopGttOn))),
                    (false, false) => {}
                }
                extra.push_str(&format!(
                    ", {}",
                    crate::brokers::secrets::describe(&st.storage, backend)
                ));
                ok(&tf(Msg::DoctorAngelConfigured, &[&st.summary, &extra]))
            }
            Ok(st) if st.exists => warn(&tf(
                Msg::DoctorBrokerPlaceholder,
                &[&st.path.display(), &kind.id()],
            )),
            Ok(_) => t(Msg::DoctorAngelNotConfigured).dimmed().to_string(),
            Err(e) => warn(&format!("{:#}", e)),
        };
        let label = if kind == crate::brokers::BrokerKind::Angel {
            t(Msg::DoctorAngel).to_string()
        } else {
            kind.display()
        };
        d.kv(&label, &line);
    }
    profile_checks(&mut d, market);
    notifier_checks(&mut d);
    worker_checks(&mut d);
    mcp_checks(&mut d);

    d.finish()?;

    if let Some(path) = cfg_invalid {
        bail!(
            "Config file {} is invalid — fix it, or replace it with `stockwise config init --force`",
            path
        );
    }
    if !yahoo {
        bail!("{}", t(Msg::DoctorMarketDataDown));
    }
    Ok(())
}

/// Investor profile and tax residence: they decide holding periods, tax
/// rules and how actions are sized.
fn profile_checks(d: &mut Report, market: Market) {
    let p = crate::config::effective_profile();
    d.kv(
        "Investor profile",
        &format!("{} ({}, from {})", p.spec, p.label, p.mode_source),
    );
    let slab = format!(
        "{:.0}% slab, {} regime",
        p.tax_slab,
        format!("{:?}", p.tax_regime).to_lowercase()
    );
    match crate::config::get().profile.residence.as_deref() {
        Some(r) => d.kv(
            "Tax residence",
            &format!("✓ {} (from the config file); {slab}", r),
        ),
        None => {
            let (guess, _) = super::portfolio_analyze::residence_of(market);
            let guess = guess.map_or("unknown".to_string(), |c| format!("{c:?}").to_lowercase());
            d.kv(
                "Tax residence",
                &format!("not set; assuming {guess} from the market; {slab}")
                    .dimmed()
                    .to_string(),
            );
            d.fix(
                "set `residence = \"in\"` (or us, uk) under [profile] in the config file \
                 (`stockwise config path` shows where it is)",
            );
        }
    }
}

/// Notification channels: which are set up, which are half set up.
fn notifier_checks(d: &mut Report) {
    let cfg = crate::notify::NotifyConfig::from_env();
    let channels = cfg.channels();
    if channels.is_empty() && cfg.problems.is_empty() {
        d.kv(
            "Notifications",
            &"none set up; alerts show only in the terminal"
                .dimmed()
                .to_string(),
        );
        d.fix("`stockwise notify --help` lists the channels (Telegram, ntfy, email, ...)");
        return;
    }
    if !channels.is_empty() {
        let names: Vec<String> = channels.iter().map(|c| c.to_string()).collect();
        d.kv("Notifications", &format!("✓ {}", names.join(", ")));
        if let Some(q) = &cfg.quiet {
            d.kv("", &format!("quiet hours {q}").dimmed().to_string());
        }
    }
    for problem in &cfg.problems {
        d.kv(
            if channels.is_empty() {
                "Notifications"
            } else {
                ""
            },
            &format!("! {problem}"),
        );
    }
    if !cfg.problems.is_empty() {
        d.fix("finish the channel's settings, then run `stockwise notify test`");
    }
}

/// The background worker (alerts, digests, the monitor) and its service.
fn worker_checks(d: &mut Report) {
    let running = crate::worker::running_pid();
    let alerts = crate::alerts::AlertStore::load()
        .map(|s| s.alerts.len())
        .unwrap_or(0);
    let installed = super::service::Platform::current()
        .ok()
        .and_then(|p| super::service::locations(p).ok())
        .map(|path| path.exists());
    let line = match (running, installed) {
        (Some(pid), Some(true)) => format!("✓ running (pid {pid}), started by the login service"),
        (Some(pid), _) => format!("✓ running (pid {pid})"),
        (None, Some(true)) => "! installed as a login service but not running".to_string(),
        (None, _) if alerts > 0 => {
            format!("! not running; your {alerts} alert(s) are checked only while it runs")
        }
        (None, _) => "not running (optional)".dimmed().to_string(),
    };
    d.kv("Worker", &line);
    match (running, installed) {
        (None, Some(true)) => {
            d.fix("`stockwise worker service-status`, then `stockwise worker install` to reload it")
        }
        (None, Some(false)) if alerts > 0 => d.fix(
            "`stockwise worker install` runs it at login (launchd on macOS, systemd on Linux)",
        ),
        (None, None) if alerts > 0 => d.fix("`stockwise worker start`"),
        _ => {}
    }
}

/// Which AI apps on this computer have the StockWise MCP server.
fn mcp_checks(d: &mut Report) {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    let mut connected = Vec::new();
    let mut missing = Vec::new();
    for client in crate::connect::clients(&home) {
        if client.paths.is_empty() || !crate::connect::detected(&client) {
            continue;
        }
        let has = client
            .paths
            .iter()
            .any(|p| std::fs::read_to_string(p).is_ok_and(|text| text.contains("stockwise")));
        if has {
            connected.push(client.name);
        } else {
            missing.push((client.name, client.id));
        }
    }
    if connected.is_empty() && missing.is_empty() {
        d.kv(
            "MCP (AI apps)",
            &"no supported AI app found".dimmed().to_string(),
        );
        return;
    }
    if !connected.is_empty() {
        d.kv(
            "MCP (AI apps)",
            &format!("✓ connected: {}", connected.join(", ")),
        );
    }
    if !missing.is_empty() {
        let names: Vec<&str> = missing.iter().map(|(n, _)| *n).collect();
        let ids: Vec<&str> = missing.iter().map(|(_, id)| *id).collect();
        d.kv(
            if connected.is_empty() {
                "MCP (AI apps)"
            } else {
                ""
            },
            &format!("not connected: {}", names.join(", "))
                .dimmed()
                .to_string(),
        );
        d.fix(&format!("stockwise connect {}", ids.join(" ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_marks_become_statuses_without_colour_codes() {
        let (s, t) = status_of(&format!("{} reachable", "✓".green()));
        assert_eq!((s, t.as_str()), (CheckStatus::Ok, "reachable"));
        assert_eq!(status_of("! no key").0, CheckStatus::Warn);
        assert_eq!(status_of("✗ down").0, CheckStatus::Fail);
        let (s, t) = status_of(&"not configured".dimmed().to_string());
        assert_eq!((s, t.as_str()), (CheckStatus::Info, "not configured"));
    }

    #[test]
    fn json_report_records_fixes_and_continuations() {
        let mut d = Report::new(true);
        d.kv("Worker", "! not running");
        d.fix("stockwise worker start");
        d.kv("", "✓ fine");
        assert_eq!(d.checks.len(), 2);
        assert_eq!(d.checks[0].fix.as_deref(), Some("stockwise worker start"));
        assert_eq!(d.checks[1].name, "Worker");
        let v = serde_json::to_value(&d.checks[0]).unwrap();
        assert_eq!(v["status"], "warn");
    }
}
