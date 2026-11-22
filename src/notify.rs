//! Notifications for `alert watch`, the worker and the intraday daemon:
//! Telegram, Slack and Discord (incoming webhooks), email over SMTP, ntfy,
//! Pushover, and desktop pop-ups (with the `desktop-notify` cargo feature).
//!
//! Each message has a [`Priority`]. `STOCKWISE_NOTIFY_ROUTES` sends a channel
//! only messages at or above a priority (`desktop=high,telegram=low`), and
//! `STOCKWISE_QUIET_HOURS` (`22:00-07:00`, this machine's local time) holds
//! everything below high priority until the quiet hours end, then sends the
//! held notices as one message.
//!
//! Every channel is opt-in and configured with environment variables (see
//! [`NotifyConfig::from_lookup`] for the names). Secrets — the Telegram bot
//! token, webhook URLs and the SMTP password — are only ever sent to their
//! own endpoint: redirects are not followed, error messages are stripped of
//! URLs, and nothing here prints them.

use anyhow::{Context, Result, anyhow, bail};
use std::time::Duration;

/// Per-request timeout, so a dead endpoint can't stall a worker loop.
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
/// Discord rejects messages longer than this.
const DISCORD_MAX: usize = 2000;
/// Telegram rejects messages longer than this.
const TELEGRAM_MAX: usize = 4096;

/// How urgent a notice is. Routing and quiet hours compare against it.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    serde::Serialize,
    serde::Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum Priority {
    /// Digest material: dates coming up, routine filings.
    Low,
    #[default]
    Normal,
    /// Large moves, alerts the user set, new lows: sent even in quiet hours.
    High,
}

impl Priority {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "low" | "all" => Some(Priority::Low),
            "normal" | "default" | "medium" => Some(Priority::Normal),
            "high" | "urgent" => Some(Priority::High),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
        }
    }
}

/// A notification: a short title, a plain-text body and a priority.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Message {
    pub title: String,
    pub body: String,
    #[serde(default)]
    pub priority: Priority,
}

impl Message {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            priority: Priority::Normal,
        }
    }

    pub fn with_priority(mut self, priority: Priority) -> Self {
        self.priority = priority;
        self
    }

    /// Title and body as one chat message.
    fn text(&self) -> String {
        if self.body.is_empty() {
            self.title.clone()
        } else {
            format!("{}\n{}", self.title, self.body)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Channel {
    Telegram,
    Slack,
    Discord,
    Email,
    Ntfy,
    Pushover,
    Desktop,
}

impl Channel {
    pub const ALL: [Channel; 7] = [
        Channel::Telegram,
        Channel::Slack,
        Channel::Discord,
        Channel::Email,
        Channel::Ntfy,
        Channel::Pushover,
        Channel::Desktop,
    ];

    fn parse(s: &str) -> Option<Self> {
        let s = s.trim().to_ascii_lowercase();
        Self::ALL
            .into_iter()
            .find(|c| c.to_string().to_ascii_lowercase() == s)
    }
}

impl std::fmt::Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.pad(match self {
            Channel::Telegram => "Telegram",
            Channel::Slack => "Slack",
            Channel::Discord => "Discord",
            Channel::Email => "Email",
            Channel::Ntfy => "ntfy",
            Channel::Pushover => "Pushover",
            Channel::Desktop => "Desktop",
        })
    }
}

#[derive(Clone)]
pub struct Telegram {
    /// `https://api.telegram.org` (tests point it at a local server).
    pub api_base: String,
    pub token: String,
    pub chat_id: String,
}

/// A Slack or Discord incoming-webhook URL. The URL itself is the secret.
#[derive(Clone)]
pub struct Webhook {
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTls {
    /// Plain connection upgraded with STARTTLS (port 587).
    StartTls,
    /// TLS from the first byte (port 465).
    Tls,
    /// No encryption — only allowed to a server on this machine.
    None,
}

#[derive(Clone)]
pub struct Email {
    pub host: String,
    pub port: u16,
    pub tls: SmtpTls,
    pub user: Option<String>,
    pub password: Option<String>,
    pub from: String,
    pub to: Vec<String>,
}

/// An ntfy topic (<https://ntfy.sh> or a self-hosted server). The topic
/// name is the secret unless the server needs a token.
#[derive(Clone)]
pub struct Ntfy {
    pub server: String,
    pub topic: String,
    pub token: Option<String>,
}

/// A Pushover application token and user key.
#[derive(Clone)]
pub struct Pushover {
    /// `https://api.pushover.net` (tests point it at a local server).
    pub api_base: String,
    pub token: String,
    pub user: String,
}

/// Quiet hours in this machine's local time; `start > end` wraps midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuietHours {
    pub start: chrono::NaiveTime,
    pub end: chrono::NaiveTime,
}

impl QuietHours {
    /// `22:00-07:00` (also `22-7`).
    pub fn parse(s: &str) -> Option<Self> {
        let (a, b) = s.split_once('-')?;
        let time = |t: &str| {
            let t = t.trim();
            chrono::NaiveTime::parse_from_str(t, "%H:%M")
                .ok()
                .or_else(|| {
                    t.parse::<u32>()
                        .ok()
                        .and_then(|h| chrono::NaiveTime::from_hms_opt(h, 0, 0))
                })
        };
        let (start, end) = (time(a)?, time(b)?);
        (start != end).then_some(Self { start, end })
    }

    pub fn contains(&self, t: chrono::NaiveTime) -> bool {
        if self.start < self.end {
            t >= self.start && t < self.end
        } else {
            t >= self.start || t < self.end
        }
    }
}

impl std::fmt::Display for QuietHours {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}",
            self.start.format("%H:%M"),
            self.end.format("%H:%M")
        )
    }
}

/// Which channels are configured. Built from environment variables; a
/// config file can supply the same keys through [`NotifyConfig::from_lookup`].
#[derive(Clone, Default)]
pub struct NotifyConfig {
    pub telegram: Option<Telegram>,
    pub slack: Option<Webhook>,
    pub discord: Option<Webhook>,
    pub email: Option<Email>,
    pub ntfy: Option<Ntfy>,
    pub pushover: Option<Pushover>,
    pub desktop: bool,
    /// Lowest priority each channel sends (absent: everything).
    pub routes: std::collections::HashMap<Channel, Priority>,
    /// Hold notices below high priority during these hours.
    pub quiet: Option<QuietHours>,
    /// Half-configured channels, explained (never containing secrets).
    pub problems: Vec<String>,
}

/// Environment variables that configure notifications, for help text.
pub const ENV_VARS: &[(&str, &str)] = &[
    (
        "STOCKWISE_TELEGRAM_TOKEN",
        "Telegram bot token (from @BotFather)",
    ),
    (
        "STOCKWISE_TELEGRAM_CHAT_ID",
        "Chat to message (your user id or a group id)",
    ),
    ("STOCKWISE_SLACK_WEBHOOK", "Slack incoming-webhook URL"),
    ("STOCKWISE_DISCORD_WEBHOOK", "Discord webhook URL"),
    (
        "STOCKWISE_SMTP_HOST",
        "SMTP server, e.g. smtp.gmail.com (enables email)",
    ),
    ("STOCKWISE_SMTP_PORT", "Default 587 (starttls) or 465 (tls)"),
    ("STOCKWISE_SMTP_TLS", "starttls (default), tls, or none"),
    ("STOCKWISE_SMTP_USER", "SMTP login"),
    ("STOCKWISE_SMTP_PASSWORD", "SMTP password or app password"),
    (
        "STOCKWISE_EMAIL_FROM",
        "Sender address (default: the SMTP user)",
    ),
    ("STOCKWISE_EMAIL_TO", "Recipients, comma-separated"),
    (
        "STOCKWISE_NTFY_TOPIC",
        "ntfy topic to publish to (enables ntfy)",
    ),
    (
        "STOCKWISE_NTFY_SERVER",
        "ntfy server (default https://ntfy.sh)",
    ),
    (
        "STOCKWISE_NTFY_TOKEN",
        "ntfy access token, if the topic needs one",
    ),
    ("STOCKWISE_PUSHOVER_TOKEN", "Pushover application token"),
    ("STOCKWISE_PUSHOVER_USER", "Pushover user key"),
    (
        "STOCKWISE_DESKTOP_NOTIFY",
        "1 for desktop pop-ups (needs the desktop-notify build feature)",
    ),
    (
        "STOCKWISE_NOTIFY_ROUTES",
        "Lowest priority per channel, e.g. desktop=high,telegram=low",
    ),
    (
        "STOCKWISE_QUIET_HOURS",
        "Hold all but high-priority notices, e.g. 22:00-07:00 (local time)",
    ),
];

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

impl NotifyConfig {
    /// From the environment, then `[notify]` in the config file.
    pub fn from_env() -> Self {
        Self::from_lookup(crate::config::notify_setting)
    }

    /// Build the config from a key lookup (blank values count as unset).
    /// `from_env` passes `std::env::var`; a config-file layer can pass a
    /// lookup that checks the environment first and then the file.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> Self {
        let get = |key: &str| {
            lookup(key)
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        let mut cfg = NotifyConfig::default();

        match (
            get("STOCKWISE_TELEGRAM_TOKEN"),
            get("STOCKWISE_TELEGRAM_CHAT_ID"),
        ) {
            (Some(token), Some(chat_id)) => {
                cfg.telegram = Some(Telegram {
                    api_base: "https://api.telegram.org".into(),
                    token,
                    chat_id,
                })
            }
            (Some(_), None) => cfg
                .problems
                .push("Telegram: STOCKWISE_TELEGRAM_CHAT_ID is not set".into()),
            (None, Some(_)) => cfg
                .problems
                .push("Telegram: STOCKWISE_TELEGRAM_TOKEN is not set".into()),
            (None, None) => {}
        }

        for (key, name, slot) in [
            ("STOCKWISE_SLACK_WEBHOOK", "Slack", &mut cfg.slack),
            ("STOCKWISE_DISCORD_WEBHOOK", "Discord", &mut cfg.discord),
        ] {
            if let Some(url) = get(key) {
                if url.starts_with("https://") {
                    *slot = Some(Webhook { url });
                } else {
                    cfg.problems
                        .push(format!("{}: {} must be an https:// URL", name, key));
                }
            }
        }

        if let Some(host) = get("STOCKWISE_SMTP_HOST") {
            match Self::email_from_lookup(host, &get) {
                Ok(email) => cfg.email = Some(email),
                Err(e) => cfg.problems.push(format!("Email: {}", e)),
            }
        }

        match (get("STOCKWISE_NTFY_TOPIC"), get("STOCKWISE_NTFY_SERVER")) {
            (Some(topic), server) => {
                let server = server.unwrap_or_else(|| "https://ntfy.sh".into());
                let parsed = reqwest::Url::parse(&server).ok();
                let valid = parsed.as_ref().is_some_and(|u| {
                    u.has_host()
                        && (u.scheme() == "https"
                            || (u.scheme() == "http" && u.host_str().is_some_and(is_loopback)))
                });
                if valid {
                    cfg.ntfy = Some(Ntfy {
                        server: server.trim_end_matches('/').to_string(),
                        topic,
                        token: get("STOCKWISE_NTFY_TOKEN"),
                    });
                } else {
                    cfg.problems
                        .push("ntfy: STOCKWISE_NTFY_SERVER must be an https:// URL".into());
                }
            }
            (None, Some(_)) => cfg
                .problems
                .push("ntfy: STOCKWISE_NTFY_TOPIC is not set".into()),
            (None, None) => {}
        }

        match (
            get("STOCKWISE_PUSHOVER_TOKEN"),
            get("STOCKWISE_PUSHOVER_USER"),
        ) {
            (Some(token), Some(user)) => {
                cfg.pushover = Some(Pushover {
                    api_base: "https://api.pushover.net".into(),
                    token,
                    user,
                })
            }
            (Some(_), None) => cfg
                .problems
                .push("Pushover: STOCKWISE_PUSHOVER_USER is not set".into()),
            (None, Some(_)) => cfg
                .problems
                .push("Pushover: STOCKWISE_PUSHOVER_TOKEN is not set".into()),
            (None, None) => {}
        }

        if let Some(routes) = get("STOCKWISE_NOTIFY_ROUTES") {
            for part in routes.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                let parsed = part
                    .split_once(['=', ':'])
                    .and_then(|(c, p)| Some((Channel::parse(c)?, Priority::parse(p)?)));
                match parsed {
                    Some((c, p)) => {
                        cfg.routes.insert(c, p);
                    }
                    None => cfg.problems.push(format!(
                        "Routes: `{part}` is not CHANNEL=low|normal|high (channels: telegram, slack, discord, email, ntfy, pushover, desktop)"
                    )),
                }
            }
        }

        if let Some(q) = get("STOCKWISE_QUIET_HOURS") {
            match QuietHours::parse(&q) {
                Some(q) => cfg.quiet = Some(q),
                None => cfg.problems.push(format!(
                    "Quiet hours: `{q}` is not HH:MM-HH:MM, e.g. 22:00-07:00"
                )),
            }
        }

        cfg.desktop = get("STOCKWISE_DESKTOP_NOTIFY")
            .is_some_and(|v| matches!(v.to_lowercase().as_str(), "1" | "true" | "yes" | "on"));
        if cfg.desktop && !cfg!(feature = "desktop-notify") {
            cfg.desktop = false;
            cfg.problems.push(
                "Desktop: this build has no desktop notifications \
                 (reinstall with `--features desktop-notify`)"
                    .into(),
            );
        }
        cfg
    }

    fn email_from_lookup(host: String, get: &impl Fn(&str) -> Option<String>) -> Result<Email> {
        let tls = match get("STOCKWISE_SMTP_TLS").as_deref().map(str::to_lowercase) {
            None => SmtpTls::StartTls,
            Some(v) if v == "starttls" => SmtpTls::StartTls,
            Some(v) if v == "tls" || v == "ssl" => SmtpTls::Tls,
            Some(v) if v == "none" => SmtpTls::None,
            Some(v) => bail!(
                "STOCKWISE_SMTP_TLS must be starttls, tls or none (got `{}`)",
                v
            ),
        };
        let port = match get("STOCKWISE_SMTP_PORT") {
            Some(p) => p
                .parse()
                .map_err(|_| anyhow!("STOCKWISE_SMTP_PORT `{}` is not a port number", p))?,
            None if tls == SmtpTls::Tls => 465,
            None if tls == SmtpTls::None => 25,
            None => 587,
        };
        let user = get("STOCKWISE_SMTP_USER");
        let password = get("STOCKWISE_SMTP_PASSWORD");
        if tls == SmtpTls::None && password.is_some() && !is_loopback(&host) {
            bail!(
                "refusing to send the SMTP password unencrypted; use STOCKWISE_SMTP_TLS=starttls or tls"
            );
        }
        let from = get("STOCKWISE_EMAIL_FROM")
            .or_else(|| user.clone().filter(|u| u.contains('@')))
            .context("STOCKWISE_EMAIL_FROM is not set")?;
        let to: Vec<String> = get("STOCKWISE_EMAIL_TO")
            .context("STOCKWISE_EMAIL_TO is not set")?
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if to.is_empty() {
            bail!("STOCKWISE_EMAIL_TO has no addresses");
        }
        Ok(Email {
            host,
            port,
            tls,
            user,
            password,
            from,
            to,
        })
    }

    /// Configured channels, in a fixed order.
    pub fn channels(&self) -> Vec<Channel> {
        let mut out = Vec::new();
        if self.telegram.is_some() {
            out.push(Channel::Telegram);
        }
        if self.slack.is_some() {
            out.push(Channel::Slack);
        }
        if self.discord.is_some() {
            out.push(Channel::Discord);
        }
        if self.email.is_some() {
            out.push(Channel::Email);
        }
        if self.ntfy.is_some() {
            out.push(Channel::Ntfy);
        }
        if self.pushover.is_some() {
            out.push(Channel::Pushover);
        }
        if self.desktop {
            out.push(Channel::Desktop);
        }
        out
    }

    /// Where a channel delivers, with secrets left out: the chat id, the
    /// webhook's host, the SMTP server and recipient count.
    pub fn destination(&self, channel: Channel) -> Option<String> {
        let host_of = |url: &str| {
            reqwest::Url::parse(url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_string))
                .unwrap_or_else(|| "webhook".into())
        };
        Some(match channel {
            Channel::Telegram => format!("chat {}", self.telegram.as_ref()?.chat_id),
            Channel::Slack => host_of(&self.slack.as_ref()?.url),
            Channel::Discord => host_of(&self.discord.as_ref()?.url),
            Channel::Email => {
                let e = self.email.as_ref()?;
                let tls = match e.tls {
                    SmtpTls::StartTls => "STARTTLS",
                    SmtpTls::Tls => "TLS",
                    SmtpTls::None => "unencrypted",
                };
                format!(
                    "{}:{} ({}) → {} recipient{}",
                    e.host,
                    e.port,
                    tls,
                    e.to.len(),
                    if e.to.len() == 1 { "" } else { "s" }
                )
            }
            Channel::Ntfy => {
                let n = self.ntfy.as_ref()?;
                format!("{} (topic hidden)", host_of(&n.server))
            }
            Channel::Pushover => "api.pushover.net".into(),
            Channel::Desktop => "this machine".into(),
        })
    }

    /// Whether `channel` sends a message of `priority`.
    pub fn routes_to(&self, channel: Channel, priority: Priority) -> bool {
        self.routes.get(&channel).is_none_or(|min| priority >= *min)
    }

    /// Whether a message of `priority` is held at local time `t`.
    pub fn holds(&self, priority: Priority, t: chrono::NaiveTime) -> bool {
        priority < Priority::High && self.quiet.is_some_and(|q| q.contains(t))
    }
}

/// Notices held during quiet hours, sent as one message afterwards.
pub const HELD_FILE: &str = "notify_held.json";

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Held {
    pub messages: Vec<Message>,
}

impl Held {
    /// The held notices as one message, newest last; `None` when empty.
    pub fn combined(&self) -> Option<Message> {
        match self.messages.as_slice() {
            [] => None,
            [one] => Some(one.clone()),
            many => Some(batch(
                &format!("StockWise: {} notices from quiet hours", many.len()),
                many,
            )),
        }
    }
}

/// Several notices as one message: each title, with its body indented.
/// The batch takes the highest priority among them.
pub fn batch(title: &str, messages: &[Message]) -> Message {
    let mut body = String::new();
    for m in messages {
        if !body.is_empty() {
            body.push('\n');
        }
        body.push_str("• ");
        body.push_str(&m.title);
        for line in m.body.lines().filter(|l| !l.trim().is_empty()) {
            body.push_str("\n  ");
            body.push_str(line);
        }
    }
    Message::new(title, body).with_priority(
        messages
            .iter()
            .map(|m| m.priority)
            .max()
            .unwrap_or_default(),
    )
}

/// Sends messages to every configured channel.
#[derive(Clone)]
pub struct Notifier {
    config: NotifyConfig,
    http: reqwest::Client,
}

impl Notifier {
    pub fn new(config: NotifyConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(SEND_TIMEOUT)
            .connect_timeout(SEND_TIMEOUT)
            // A redirect would carry the request (and a token in its URL)
            // somewhere the user didn't configure.
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_default();
        Self { config, http }
    }

    pub fn from_env() -> Self {
        Self::new(NotifyConfig::from_env())
    }

    pub fn config(&self) -> &NotifyConfig {
        &self.config
    }

    pub fn is_enabled(&self) -> bool {
        !self.config.channels().is_empty()
    }

    /// Send `msg` to every channel its priority is routed to, at once; one
    /// result per channel.
    pub async fn send(&self, msg: &Message) -> Vec<(Channel, Result<()>)> {
        let mut tasks = tokio::task::JoinSet::new();
        let channels = self
            .config
            .channels()
            .into_iter()
            .filter(|c| self.config.routes_to(*c, msg.priority));
        for (i, channel) in channels.enumerate() {
            let this = self.clone();
            let msg = msg.clone();
            tasks.spawn(async move { (i, channel, this.send_one(channel, &msg).await) });
        }
        let mut results = Vec::new();
        while let Some(joined) = tasks.join_next().await {
            if let Ok(r) = joined {
                results.push(r);
            }
        }
        results.sort_by_key(|(i, ..)| *i);
        results.into_iter().map(|(_, c, r)| (c, r)).collect()
    }

    /// Send, and print any failure as a warning. For worker loops, where a
    /// notification that can't be delivered must never stop the work.
    /// Returns false only when channels are configured and every one failed.
    ///
    /// In quiet hours a message below high priority is held (and counts as
    /// delivered); the first send after the quiet hours sends the held ones
    /// first, as one message.
    pub async fn send_logged(&self, msg: &Message) -> bool {
        if !self.is_enabled() {
            return true;
        }
        let now = chrono::Local::now().time();
        if self.config.holds(msg.priority, now) {
            return self.hold(msg);
        }
        self.flush_held().await;
        self.deliver_logged(msg).await
    }

    /// Keep `msg` for after the quiet hours. False if it couldn't be saved.
    fn hold(&self, msg: &Message) -> bool {
        crate::store::update::<Held, ()>(HELD_FILE, |h| {
            h.messages.push(msg.clone());
            // A week of quiet hours at most; the oldest go first.
            if h.messages.len() > 200 {
                let drop = h.messages.len() - 200;
                h.messages.drain(..drop);
            }
            Ok(())
        })
        .is_ok()
    }

    /// Send what quiet hours held, if they are over. Called before every
    /// send and by the worker loop; a failed send keeps the notices held.
    pub async fn flush_held(&self) {
        if !self.is_enabled()
            || self
                .config
                .holds(Priority::Normal, chrono::Local::now().time())
        {
            return;
        }
        let Ok(held) = crate::store::load::<Held>(HELD_FILE) else {
            return;
        };
        if held.messages.is_empty() {
            return;
        }
        // Apply each channel's priority threshold per message before combining
        // so low-priority held notices are not promoted by a higher-priority sibling.
        let mut any_eligible = false;
        let mut delivered = false;
        for channel in self.config.channels() {
            let eligible: Vec<Message> = held
                .messages
                .iter()
                .filter(|m| self.config.routes_to(channel, m.priority))
                .cloned()
                .collect();
            let batch_holder = Held { messages: eligible };
            let Some(msg) = batch_holder.combined() else {
                continue;
            };
            any_eligible = true;
            match self.send_one(channel, &msg).await {
                Ok(()) => delivered = true,
                Err(e) => eprintln!("  ! {} notification failed: {:#}", channel, e),
            }
        }
        if !any_eligible || delivered {
            let sent = held.messages.len();
            let _ = crate::store::update::<Held, ()>(HELD_FILE, |h| {
                h.messages.drain(..sent.min(h.messages.len()));
                Ok(())
            });
        }
    }

    async fn deliver_logged(&self, msg: &Message) -> bool {
        let results = self.send(msg).await;
        if results.is_empty() {
            return true;
        }
        let mut delivered = false;
        for (channel, result) in results {
            match result {
                Ok(()) => delivered = true,
                Err(e) => eprintln!("  ! {} notification failed: {:#}", channel, e),
            }
        }
        delivered
    }

    async fn send_one(&self, channel: Channel, msg: &Message) -> Result<()> {
        match channel {
            Channel::Telegram => {
                let t = self.config.telegram.as_ref().context("not configured")?;
                let url = format!(
                    "{}/bot{}/sendMessage",
                    t.api_base.trim_end_matches('/'),
                    t.token
                );
                let body = serde_json::json!({
                    "chat_id": t.chat_id,
                    "text": truncate(&msg.text(), TELEGRAM_MAX),
                    "disable_web_page_preview": true,
                });
                self.post_json(&url, &body).await
            }
            Channel::Slack => {
                let w = self.config.slack.as_ref().context("not configured")?;
                self.post_json(&w.url, &serde_json::json!({ "text": msg.text() }))
                    .await
            }
            Channel::Discord => {
                let w = self.config.discord.as_ref().context("not configured")?;
                let body = serde_json::json!({
                    "content": truncate(&msg.text(), DISCORD_MAX),
                    "allowed_mentions": { "parse": [] },
                });
                self.post_json(&w.url, &body).await
            }
            Channel::Email => {
                let e = self.config.email.as_ref().context("not configured")?;
                send_email(e, msg).await
            }
            Channel::Ntfy => {
                let n = self.config.ntfy.as_ref().context("not configured")?;
                // JSON publishing to the server root keeps a UTF-8 title
                // intact (headers would have to be ASCII).
                let body = serde_json::json!({
                    "topic": n.topic,
                    "title": msg.title,
                    "message": if msg.body.is_empty() { &msg.title } else { &msg.body },
                    "priority": match msg.priority {
                        Priority::Low => 2,
                        Priority::Normal => 3,
                        Priority::High => 5,
                    },
                });
                let mut req = self.http.post(&n.server).json(&body);
                if let Some(t) = &n.token {
                    req = req.bearer_auth(t);
                }
                self.finish(req).await
            }
            Channel::Pushover => {
                let p = self.config.pushover.as_ref().context("not configured")?;
                let url = format!("{}/1/messages.json", p.api_base.trim_end_matches('/'));
                let body = serde_json::json!({
                    "token": p.token,
                    "user": p.user,
                    "title": truncate(&msg.title, 250),
                    "message": truncate(if msg.body.is_empty() { &msg.title } else { &msg.body }, 1024),
                    "priority": match msg.priority {
                        Priority::Low => -1,
                        Priority::Normal => 0,
                        Priority::High => 1,
                    },
                });
                self.post_json(&url, &body).await
            }
            Channel::Desktop => send_desktop(msg).await,
        }
    }

    /// POST JSON; any non-2xx status is an error carrying the service's
    /// explanation. Transport errors are stripped of the URL.
    async fn post_json(&self, url: &str, body: &serde_json::Value) -> Result<()> {
        self.finish(self.http.post(url).json(body)).await
    }

    /// Send a prepared request; see [`Notifier::post_json`].
    async fn finish(&self, req: reqwest::RequestBuilder) -> Result<()> {
        let resp = req
            .send()
            .await
            .map_err(|e| anyhow!("{}", e.without_url()))?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let text = resp.text().await.unwrap_or_default();
        let detail = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|v| {
                v.get("description")
                    .or_else(|| v.get("message"))
                    .and_then(|d| d.as_str())
                    .map(str::to_string)
            })
            .unwrap_or(text);
        let detail = truncate(detail.trim(), 200);
        if detail.is_empty() {
            bail!("HTTP {}", status)
        } else {
            bail!("HTTP {}: {}", status, detail)
        }
    }
}

/// Shorten to at most `max` characters, marking the cut.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

async fn send_email(e: &Email, msg: &Message) -> Result<()> {
    use lettre::message::header::ContentType;
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let mut builder = lettre::Message::builder()
        .from(
            e.from
                .parse()
                .with_context(|| format!("bad sender address `{}`", e.from))?,
        )
        .subject(if msg.title.starts_with("StockWise") {
            msg.title.clone()
        } else {
            format!("StockWise: {}", msg.title)
        })
        .header(ContentType::TEXT_PLAIN);
    for to in &e.to {
        builder = builder.to(to
            .parse()
            .with_context(|| format!("bad recipient address `{}`", to))?);
    }
    let mail = builder.body(msg.body.clone())?;

    let transport = match e.tls {
        SmtpTls::StartTls => AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&e.host)?,
        SmtpTls::Tls => AsyncSmtpTransport::<Tokio1Executor>::relay(&e.host)?,
        SmtpTls::None => AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&e.host),
    };
    let mut transport = transport.port(e.port).timeout(Some(SEND_TIMEOUT));
    if let (Some(user), Some(password)) = (&e.user, &e.password) {
        transport = transport.credentials(Credentials::new(user.clone(), password.clone()));
    }
    transport
        .build()
        .send(mail)
        .await
        .map_err(|err| anyhow!("SMTP: {}", err))?;
    Ok(())
}

#[cfg(feature = "desktop-notify")]
async fn send_desktop(msg: &Message) -> Result<()> {
    let msg = msg.clone();
    tokio::task::spawn_blocking(move || {
        notify_rust::Notification::new()
            .appname("StockWise")
            .summary(&msg.title)
            .body(&msg.body)
            .show()
            .map(|_| ())
            .map_err(|e| anyhow!("{}", e))
    })
    .await?
}

#[cfg(not(feature = "desktop-notify"))]
async fn send_desktop(_msg: &Message) -> Result<()> {
    bail!("this build has no desktop notifications (reinstall with `--features desktop-notify`)")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    use tokio::net::TcpListener;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k| map.get(k).cloned()
    }

    /// A one-shot HTTP server: answers the first request with `status` and
    /// `body`, and hands back the request line and body it received.
    async fn mock_http(
        status: u16,
        body: &'static str,
    ) -> (String, tokio::task::JoinHandle<(String, String)>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let mut reader = BufReader::new(sock);
            let mut request_line = String::new();
            reader.read_line(&mut request_line).await.unwrap();
            let mut len = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).await.unwrap();
                if line == "\r\n" || line.is_empty() {
                    break;
                }
                if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut buf = vec![0u8; len];
            reader.read_exact(&mut buf).await.unwrap();
            let reply = format!(
                "HTTP/1.1 {} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                status,
                body.len(),
                body
            );
            reader.get_mut().write_all(reply.as_bytes()).await.unwrap();
            (
                request_line.trim().to_string(),
                String::from_utf8(buf).unwrap(),
            )
        });
        (base, handle)
    }

    fn only(config: NotifyConfig) -> Notifier {
        Notifier::new(config)
    }

    #[test]
    fn nothing_configured_means_no_channels() {
        let cfg = NotifyConfig::from_lookup(lookup(&[]));
        assert!(cfg.channels().is_empty());
        assert!(cfg.problems.is_empty());
    }

    #[test]
    fn half_configured_channels_are_reported_without_secrets() {
        let cfg = NotifyConfig::from_lookup(lookup(&[
            ("STOCKWISE_TELEGRAM_TOKEN", "123:SECRET"),
            (
                "STOCKWISE_SLACK_WEBHOOK",
                "http://hooks.slack.com/services/SECRET",
            ),
            ("STOCKWISE_SMTP_HOST", "smtp.example.com"),
            ("STOCKWISE_SMTP_TLS", "none"),
            ("STOCKWISE_SMTP_PASSWORD", "SECRET"),
            ("STOCKWISE_EMAIL_TO", "me@example.com"),
        ]));
        assert!(cfg.channels().is_empty());
        assert_eq!(cfg.problems.len(), 3, "{:?}", cfg.problems);
        assert!(cfg.problems.iter().all(|p| !p.contains("SECRET")));
        assert!(cfg.problems[2].contains("unencrypted"));
    }

    #[test]
    fn full_config_and_redacted_destinations() {
        let cfg = NotifyConfig::from_lookup(lookup(&[
            ("STOCKWISE_TELEGRAM_TOKEN", "123:SECRET"),
            ("STOCKWISE_TELEGRAM_CHAT_ID", "42"),
            (
                "STOCKWISE_SLACK_WEBHOOK",
                "https://hooks.slack.com/services/T/B/SECRET",
            ),
            (
                "STOCKWISE_DISCORD_WEBHOOK",
                "https://discord.com/api/webhooks/1/SECRET",
            ),
            ("STOCKWISE_SMTP_HOST", "smtp.example.com"),
            ("STOCKWISE_SMTP_USER", "me@example.com"),
            ("STOCKWISE_SMTP_PASSWORD", "SECRET"),
            ("STOCKWISE_EMAIL_TO", "a@example.com, b@example.com"),
        ]));
        assert_eq!(
            cfg.channels(),
            vec![
                Channel::Telegram,
                Channel::Slack,
                Channel::Discord,
                Channel::Email
            ]
        );
        let email = cfg.email.as_ref().unwrap();
        assert_eq!(email.port, 587);
        assert_eq!(email.from, "me@example.com");
        assert_eq!(email.to.len(), 2);
        for c in cfg.channels() {
            let d = cfg.destination(c).unwrap();
            assert!(!d.contains("SECRET"), "{c}: {d}");
        }
        assert_eq!(cfg.destination(Channel::Slack).unwrap(), "hooks.slack.com");
        assert_eq!(
            cfg.destination(Channel::Email).unwrap(),
            "smtp.example.com:587 (STARTTLS) → 2 recipients"
        );
    }

    #[cfg(not(feature = "desktop-notify"))]
    #[test]
    fn desktop_without_the_feature_is_a_problem_not_a_channel() {
        let cfg = NotifyConfig::from_lookup(lookup(&[("STOCKWISE_DESKTOP_NOTIFY", "1")]));
        assert!(!cfg.desktop);
        assert!(cfg.problems[0].contains("desktop-notify"));
    }

    #[tokio::test]
    async fn telegram_posts_chat_id_and_text_to_the_bot_url() {
        let (base, server) = mock_http(200, r#"{"ok":true}"#).await;
        let cfg = NotifyConfig {
            telegram: Some(Telegram {
                api_base: base,
                token: "123:ABC".into(),
                chat_id: "42".into(),
            }),
            ..Default::default()
        };
        let results = only(cfg)
            .send(&Message::new("Alert", "TCS above 4200"))
            .await;
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok(), "{:?}", results[0].1);
        let (line, body) = server.await.unwrap();
        assert_eq!(line, "POST /bot123:ABC/sendMessage HTTP/1.1");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["chat_id"], "42");
        assert_eq!(json["text"], "Alert\nTCS above 4200");
    }

    #[tokio::test]
    async fn slack_and_discord_post_their_payloads() {
        let (slack_base, slack) = mock_http(200, "ok").await;
        let (discord_base, discord) = mock_http(204, "").await;
        let cfg = NotifyConfig {
            slack: Some(Webhook {
                url: format!("{}/services/T/B/X", slack_base),
            }),
            discord: Some(Webhook {
                url: format!("{}/api/webhooks/1/X", discord_base),
            }),
            ..Default::default()
        };
        let results = only(cfg).send(&Message::new("Hi", "there")).await;
        assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
        let (_, body) = slack.await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["text"],
            "Hi\nthere"
        );
        let (_, body) = discord.await.unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&body).unwrap()["content"],
            "Hi\nthere"
        );
    }

    #[tokio::test]
    async fn errors_carry_the_service_reason_but_never_the_url() {
        let (base, _server) = mock_http(401, r#"{"ok":false,"description":"Unauthorized"}"#).await;
        let cfg = NotifyConfig {
            telegram: Some(Telegram {
                api_base: base,
                token: "123:SECRET".into(),
                chat_id: "42".into(),
            }),
            ..Default::default()
        };
        let results = only(cfg).send(&Message::new("x", "")).await;
        let err = format!("{:#}", results[0].1.as_ref().unwrap_err());
        assert!(err.contains("401") && err.contains("Unauthorized"), "{err}");
        assert!(!err.contains("SECRET"), "{err}");

        // A connection failure (nothing listening) must not leak the URL either.
        let cfg = NotifyConfig {
            slack: Some(Webhook {
                url: "http://127.0.0.1:9/services/SECRET".into(),
            }),
            ..Default::default()
        };
        let results = only(cfg).send(&Message::new("x", "")).await;
        let err = format!("{:#}", results[0].1.as_ref().unwrap_err());
        assert!(!err.contains("SECRET"), "{err}");
    }

    #[tokio::test]
    async fn redirects_are_not_followed() {
        let (base, _server) = mock_http(302, "").await;
        let cfg = NotifyConfig {
            slack: Some(Webhook {
                url: format!("{}/hook", base),
            }),
            ..Default::default()
        };
        let results = only(cfg).send(&Message::new("x", "")).await;
        assert!(results[0].1.is_err());
    }

    /// A minimal SMTP server that accepts one message and returns it.
    async fn mock_smtp() -> (u16, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = tokio::spawn(async move {
            let (sock, _) = listener.accept().await.unwrap();
            let (r, mut w) = sock.into_split();
            let mut reader = BufReader::new(r);
            w.write_all(b"220 localhost ESMTP\r\n").await.unwrap();
            let mut transcript = String::new();
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap() == 0 {
                    break;
                }
                let cmd = line.to_uppercase();
                if cmd.starts_with("EHLO") || cmd.starts_with("HELO") {
                    w.write_all(b"250 localhost\r\n").await.unwrap();
                } else if cmd.starts_with("DATA") {
                    w.write_all(b"354 go ahead\r\n").await.unwrap();
                    loop {
                        let mut l = String::new();
                        reader.read_line(&mut l).await.unwrap();
                        if l == ".\r\n" {
                            break;
                        }
                        transcript.push_str(&l);
                    }
                    w.write_all(b"250 queued\r\n").await.unwrap();
                } else if cmd.starts_with("QUIT") {
                    w.write_all(b"221 bye\r\n").await.unwrap();
                    break;
                } else {
                    transcript.push_str(&line);
                    w.write_all(b"250 OK\r\n").await.unwrap();
                }
            }
            transcript
        });
        (port, handle)
    }

    #[tokio::test]
    async fn email_is_delivered_over_smtp() {
        let (port, server) = mock_smtp().await;
        let cfg = NotifyConfig::from_lookup(lookup(&[
            ("STOCKWISE_SMTP_HOST", "127.0.0.1"),
            ("STOCKWISE_SMTP_PORT", &port.to_string()),
            ("STOCKWISE_SMTP_TLS", "none"),
            ("STOCKWISE_EMAIL_FROM", "bot@example.com"),
            ("STOCKWISE_EMAIL_TO", "me@example.com"),
        ]));
        assert_eq!(cfg.channels(), vec![Channel::Email]);
        let results = only(cfg).send(&Message::new("Daily P&L", "+₹120")).await;
        assert!(results[0].1.is_ok(), "{:?}", results[0].1);
        let transcript = server.await.unwrap();
        assert!(
            transcript.contains("MAIL FROM:<bot@example.com>"),
            "{transcript}"
        );
        assert!(
            transcript.contains("RCPT TO:<me@example.com>"),
            "{transcript}"
        );
        assert!(
            transcript.contains("Subject: StockWise: Daily P&L"),
            "{transcript}"
        );
    }

    #[test]
    fn quiet_hours_wrap_midnight_and_hold_only_below_high() {
        let t = |h, m| chrono::NaiveTime::from_hms_opt(h, m, 0).unwrap();
        let q = QuietHours::parse("22:00-07:00").unwrap();
        assert!(q.contains(t(23, 30)));
        assert!(q.contains(t(2, 0)));
        assert!(q.contains(t(22, 0)));
        assert!(!q.contains(t(7, 0)));
        assert!(!q.contains(t(12, 0)));
        let day = QuietHours::parse("13-14").unwrap();
        assert!(day.contains(t(13, 30)) && !day.contains(t(14, 0)));
        assert!(QuietHours::parse("22:00-22:00").is_none());
        assert!(QuietHours::parse("late").is_none());

        let cfg = NotifyConfig::from_lookup(lookup(&[("STOCKWISE_QUIET_HOURS", "22:00-07:00")]));
        assert_eq!(cfg.quiet, Some(q));
        assert!(cfg.holds(Priority::Normal, t(23, 0)));
        assert!(cfg.holds(Priority::Low, t(6, 59)));
        assert!(!cfg.holds(Priority::High, t(23, 0)));
        assert!(!cfg.holds(Priority::Normal, t(9, 0)));
        let bad = NotifyConfig::from_lookup(lookup(&[("STOCKWISE_QUIET_HOURS", "night")]));
        assert!(bad.quiet.is_none() && bad.problems[0].contains("Quiet hours"));
    }

    #[test]
    fn routes_send_each_channel_its_priorities() {
        let cfg = NotifyConfig::from_lookup(lookup(&[
            (
                "STOCKWISE_NOTIFY_ROUTES",
                "desktop=high, telegram:low, fax=high",
            ),
            ("STOCKWISE_DESKTOP_NOTIFY", "1"),
        ]));
        assert!(!cfg.routes_to(Channel::Desktop, Priority::Normal));
        assert!(cfg.routes_to(Channel::Desktop, Priority::High));
        assert!(cfg.routes_to(Channel::Telegram, Priority::Low));
        // Unrouted channels send everything.
        assert!(cfg.routes_to(Channel::Slack, Priority::Low));
        assert!(cfg.problems.iter().any(|p| p.contains("fax=high")));
    }

    #[test]
    fn a_batch_lists_every_notice_and_takes_the_highest_priority() {
        let held = Held {
            messages: vec![
                Message::new("TCS: results tomorrow", "").with_priority(Priority::Low),
                Message::new("INFY fell 6.1%", "Below 2σ of its usual day")
                    .with_priority(Priority::High),
            ],
        };
        let m = held.combined().unwrap();
        assert!(m.title.contains("2 notices"));
        assert_eq!(
            m.body,
            "• TCS: results tomorrow\n• INFY fell 6.1%\n  Below 2σ of its usual day"
        );
        assert_eq!(m.priority, Priority::High);
        assert!(Held::default().combined().is_none());
    }

    #[tokio::test]
    async fn ntfy_and_pushover_post_their_payloads() {
        let (ntfy_base, ntfy) = mock_http(200, "{}").await;
        let (po_base, po) = mock_http(200, r#"{"status":1}"#).await;
        let cfg = NotifyConfig {
            ntfy: Some(Ntfy {
                server: ntfy_base,
                topic: "my-topic".into(),
                token: Some("tk_SECRET".into()),
            }),
            pushover: Some(Pushover {
                api_base: po_base,
                token: "app".into(),
                user: "usr".into(),
            }),
            ..Default::default()
        };
        let msg = Message::new("₹ alert", "TCS −5%").with_priority(Priority::High);
        let results = only(cfg).send(&msg).await;
        assert!(results.iter().all(|(_, r)| r.is_ok()), "{results:?}");
        let (line, body) = ntfy.await.unwrap();
        assert!(line.starts_with("POST / "), "{line}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["topic"], "my-topic");
        assert_eq!(json["title"], "₹ alert");
        assert_eq!(json["priority"], 5);
        let (line, body) = po.await.unwrap();
        assert!(line.starts_with("POST /1/messages.json"), "{line}");
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["user"], "usr");
        assert_eq!(json["priority"], 1);
    }

    #[test]
    fn ntfy_needs_https_except_on_this_machine() {
        let cfg = NotifyConfig::from_lookup(lookup(&[
            ("STOCKWISE_NTFY_TOPIC", "t"),
            ("STOCKWISE_NTFY_SERVER", "http://ntfy.example.com"),
        ]));
        assert!(cfg.ntfy.is_none() && cfg.problems[0].contains("https"));
        let cfg = NotifyConfig::from_lookup(lookup(&[
            ("STOCKWISE_NTFY_TOPIC", "t"),
            ("STOCKWISE_NTFY_SERVER", "http://127.0.0.1:8080/"),
        ]));
        assert_eq!(cfg.ntfy.unwrap().server, "http://127.0.0.1:8080");
        let cfg = NotifyConfig::from_lookup(lookup(&[("STOCKWISE_NTFY_TOPIC", "t")]));
        assert_eq!(cfg.channels(), vec![Channel::Ntfy]);
    }

    #[test]
    fn truncate_marks_the_cut() {
        assert_eq!(truncate("abc", 5), "abc");
        assert_eq!(truncate("abcdef", 4), "abc…");
    }

    #[tokio::test]
    async fn deliver_logged_returns_true_when_no_channel_routes_priority() {
        let cfg = NotifyConfig::from_lookup(lookup(&[
            ("STOCKWISE_NTFY_TOPIC", "t"),
            ("STOCKWISE_NOTIFY_ROUTES", "ntfy=high"),
        ]));
        let n = only(cfg);
        assert!(n.is_enabled());
        let normal = Message::new("digest", "body").with_priority(Priority::Normal);
        assert!(n.deliver_logged(&normal).await);
    }
}
