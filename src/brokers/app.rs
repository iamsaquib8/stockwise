//! Config shared by the brokers that use a developer app plus a daily
//! access token (Zerodha Kite, Upstox, Fyers) or a pasted token (Dhan).

use super::secrets::{self, Secret, Storage};
use super::{BrokerKind, ConfigStatus};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

pub const SECRET_KEYS: &[&str] = &["api_secret", "access_token"];

/// `<broker>_config.json`. Field names are the same for every broker; the
/// setup steps say what each broker calls them (Kite "API key", Fyers
/// "App ID", ...).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AppConfig {
    /// Public app id: Kite api_key, Upstox API key, Fyers App ID, Dhan client id.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub api_secret: Secret,
    /// OAuth redirect URL registered with the app (Upstox, Fyers).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub redirect_uri: String,
    /// Today's session, saved by `trade login`.
    #[serde(default, skip_serializing_if = "secret_is_empty")]
    pub access_token: Secret,
    /// When the session was created (IST, RFC 3339).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_created: Option<String>,
    /// Account id reported at login, for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Refuse any single order whose estimated value exceeds this (₹).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_order_value: Option<f64>,
    /// Allow live orders through this experimental integration.
    #[serde(default)]
    pub enable_orders: bool,
}

fn secret_is_empty(s: &Secret) -> bool {
    s.expose().is_empty()
}

/// What a broker needs filled in before `trade login` can work.
#[derive(Debug, Clone, Copy)]
pub struct Needs {
    pub api_secret: bool,
    pub redirect_uri: bool,
}

pub fn load(kind: BrokerKind, migrate: bool) -> Result<(AppConfig, Storage)> {
    let loaded = secrets::load(kind.config_file(), SECRET_KEYS, secrets::backend(), migrate)?;
    Ok((loaded.config, loaded.storage))
}

pub fn save(kind: BrokerKind, config: &AppConfig, storage: &Storage) -> Result<()> {
    secrets::save(
        kind.config_file(),
        config,
        SECRET_KEYS,
        storage,
        secrets::backend(),
    )
}

pub fn is_configured(config: &AppConfig, needs: Needs) -> bool {
    secrets::is_real(&config.api_key)
        && (!needs.api_secret || config.api_secret.is_real())
        && (!needs.redirect_uri || secrets::is_real(&config.redirect_uri))
}

pub fn template(needs: Needs, redirect_default: &str) -> AppConfig {
    let p = secrets::PLACEHOLDER_PREFIX;
    AppConfig {
        api_key: format!("{p}API_KEY"),
        api_secret: if needs.api_secret {
            Secret::new(format!("{p}API_SECRET"))
        } else {
            Secret::default()
        },
        redirect_uri: if needs.redirect_uri {
            redirect_default.to_string()
        } else {
            String::new()
        },
        ..Default::default()
    }
}

pub fn status(kind: BrokerKind, needs: Needs, migrate: bool) -> Result<ConfigStatus> {
    let (config, storage) = load(kind, migrate)?;
    let session = config.access_token.is_real().then(|| {
        let when = config
            .session_created
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|t| t.format("%Y-%m-%d %H:%M IST").to_string())
            .unwrap_or_else(|| "unknown time".into());
        match &config.user_id {
            Some(u) => login_summary(u, &when),
            None => format!("token saved {when}"),
        }
    });
    let mut status = super::status_of(
        kind,
        is_configured(&config, needs),
        app_summary(&config.api_key),
        storage,
        session,
    )?;
    status.session_date = config
        .access_token
        .is_real()
        .then_some(config.session_created.as_deref())
        .flatten()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.date_naive());
    status.identifiers = std::iter::once(config.api_key.clone())
        .chain(config.user_id.clone())
        .filter(|s| secrets::is_real(s))
        .collect();
    Ok(status)
}

fn login_summary(user_id: &str, when: &str) -> String {
    format!("logged in as {} ({when})", super::redact_id(user_id))
}

fn app_summary(id: &str) -> String {
    format!("app {}", super::redact_id(id))
}

/// Load the config for an API call: credentials and a saved session.
pub fn session(kind: BrokerKind, needs: Needs) -> Result<AppConfig> {
    let (config, _) = load(kind, secrets::auto_migrate())?;
    if !is_configured(&config, needs) {
        return Err(super::not_configured(kind));
    }
    if !config.access_token.is_real() {
        return Err(super::not_logged_in(kind));
    }
    Ok(config)
}

/// Load the config for `trade login`: credentials must be filled in.
pub fn for_login(kind: BrokerKind, needs: Needs) -> Result<(AppConfig, Storage)> {
    let (config, storage) = load(kind, secrets::auto_migrate())?;
    if !is_configured(&config, needs) {
        return Err(super::not_configured(kind));
    }
    Ok((config, storage))
}

/// Save a new session token from `trade login`.
pub fn save_session(
    kind: BrokerKind,
    mut config: AppConfig,
    storage: &Storage,
    token: Secret,
    user_id: Option<String>,
) -> Result<()> {
    if !token.is_real() {
        bail!("{} returned an empty access token", kind.label());
    }
    config.access_token = token;
    config.user_id = user_id;
    config.session_created = Some(super::ist_now().to_rfc3339());
    save(kind, &config, storage)?;
    let where_ = match storage {
        Storage::Keyring { .. } => secrets::backend().name(),
        Storage::File => "owner-only config file",
    };
    println!(
        "  {} Session saved in the {} for {}.",
        colored::Colorize::green("✓"),
        where_,
        kind.label()
    );
    Ok(())
}

/// Print the login URL and read back the redirect URL. Returns `param`
/// from it (or the bare value, if only that was pasted). When the pasted URL
/// carries a `state`, it must match the one we sent.
pub fn ask_for_redirect(
    kind: BrokerKind,
    url: &str,
    param: &str,
    state: Option<&str>,
) -> Result<String> {
    use colored::Colorize;
    println!();
    println!(
        "  1. Open this URL in your browser and log in to {}:",
        kind.label()
    );
    println!("     {}", url.cyan());
    println!("  2. After login the browser goes to your redirect URL (the page may not load —");
    println!("     that's fine). Copy the full address from the address bar.");
    println!();
    let pasted = super::http::prompt_line("  Paste the redirect URL here: ")?;
    redirect_value(&pasted, param, state)
}

/// See [`ask_for_redirect`].
pub fn redirect_value(pasted: &str, param: &str, state: Option<&str>) -> Result<String> {
    use super::http::query_param;
    if let (Some(expected), true) = (state, pasted.contains('?'))
        && let Some(got) = query_param(pasted, "state")
        && got != expected
    {
        bail!(
            "The pasted URL is from a different login attempt (state mismatch). Run `trade login` again."
        );
    }
    if pasted.contains('?') && query_param(pasted, "status").is_some_and(|s| s != "success") {
        bail!(
            "{} reported a failed login. Run `trade login` again.",
            "The broker"
        );
    }
    match query_param(pasted, param) {
        Some(v) => Ok(v),
        None => bail!("No `{param}` found in what was pasted. Run `trade login` again."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirect_value_checks_state_and_status() {
        let url = "http://127.0.0.1/?code=abc&state=s1";
        assert_eq!(redirect_value(url, "code", Some("s1")).unwrap(), "abc");
        assert!(redirect_value(url, "code", Some("other")).is_err());
        assert_eq!(redirect_value("abc", "code", Some("s1")).unwrap(), "abc");
        let failed = "http://127.0.0.1/?status=error&request_token=x";
        assert!(redirect_value(failed, "request_token", None).is_err());
        assert!(redirect_value("http://127.0.0.1/?foo=1", "code", None).is_err());
    }

    #[test]
    fn status_session_masks_logged_in_user_id() {
        let session = login_summary("AB1234", "2026-11-08 12:00 IST");
        assert_eq!(session, "logged in as ••34 (2026-11-08 12:00 IST)");
        assert!(!session.contains("AB1234"));
    }

    #[test]
    fn status_summary_masks_app_id() {
        let summary = app_summary("kite-secret-app-1234");
        assert_eq!(summary, "app ••34");
        assert!(!summary.contains("kite-secret-app-1234"));
    }

    #[test]
    fn template_has_placeholders_and_is_not_configured() {
        let needs = Needs {
            api_secret: true,
            redirect_uri: true,
        };
        let t = template(needs, "http://127.0.0.1/");
        assert!(!is_configured(&t, needs));
        let raw = serde_json::to_value(&t).unwrap();
        assert!(raw.get("access_token").is_none());
        assert_eq!(raw["enable_orders"], false);
    }
}
