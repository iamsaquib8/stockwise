//! Network diagnostics on stderr: `--verbose` or `STOCKWISE_LOG`.
//!
//! One line per HTTP request (source, host and path, status, time) and, at
//! `debug`, per cache hit. Query strings are never printed: they can carry
//! API keys, tokens and Yahoo's crumb.

use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

/// How much to print. Higher levels include the lower ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Off = 0,
    /// Failed requests only (errors, refusals, rate limits).
    Warn = 1,
    /// Every request.
    Info = 2,
    /// Every request and every cache hit.
    Debug = 3,
}

static LEVEL: AtomicU8 = AtomicU8::new(Level::Off as u8);
static VERBOSE_FLAG: AtomicBool = AtomicBool::new(false);

/// `STOCKWISE_LOG` value → level. Unknown values mean `info`, so a typo
/// still shows something.
pub fn parse_level(value: &str) -> Level {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "0" | "off" | "none" | "false" => Level::Off,
        "warn" | "warning" | "error" => Level::Warn,
        "debug" | "trace" | "all" => Level::Debug,
        _ => Level::Info,
    }
}

/// Set the level from `--verbose` and `STOCKWISE_LOG` (the higher wins).
pub fn init(verbose: bool) {
    let from_env = std::env::var("STOCKWISE_LOG")
        .map(|v| parse_level(&v))
        .unwrap_or(Level::Off);
    let level = if verbose {
        from_env.max(Level::Info)
    } else {
        from_env
    };
    VERBOSE_FLAG.store(verbose, Ordering::Relaxed);
    LEVEL.store(level as u8, Ordering::Relaxed);
}

/// Whether `--verbose` was given (commands with their own detail, such as
/// `ask`'s tool calls, show it too).
pub fn verbose_flag() -> bool {
    VERBOSE_FLAG.load(Ordering::Relaxed)
}

pub fn enabled(level: Level) -> bool {
    level != Level::Off && LEVEL.load(Ordering::Relaxed) >= level as u8
}

/// `https://host/path?query` → `host/path`.
pub fn redact(url: &str) -> String {
    match reqwest::Url::parse(url) {
        Ok(u) => format!("{}{}", u.host_str().unwrap_or("?"), u.path()),
        Err(_) => url.split(['?', '#']).next().unwrap_or("").to_string(),
    }
}

fn line(text: &str) {
    eprintln!("stockwise[net] {text}");
}

/// One finished request. `status` is the HTTP status, or `None` when no
/// answer came (connection error, timeout).
pub fn request(source: &str, url: &str, status: Option<u16>, elapsed: Duration) {
    let failed = status.is_none_or(|s| !(200..400).contains(&s));
    let level = if failed { Level::Warn } else { Level::Info };
    if !enabled(level) {
        return;
    }
    let status = status.map_or("no answer".to_string(), |s| s.to_string());
    line(&format!(
        "{source} GET {} -> {status} ({} ms)",
        redact(url),
        elapsed.as_millis()
    ));
}

/// An answer served from a cache instead of the network.
pub fn cache_hit(source: &str, url: &str) {
    if enabled(Level::Debug) {
        line(&format!("{source} cache hit {}", redact(url)));
    }
}

/// A free-form diagnostic (retries, fallbacks) at `level`.
pub fn note(level: Level, text: &str) {
    if enabled(level) {
        line(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_parse_and_typos_still_log() {
        assert_eq!(parse_level("off"), Level::Off);
        assert_eq!(parse_level(""), Level::Off);
        assert_eq!(parse_level("WARN"), Level::Warn);
        assert_eq!(parse_level("info"), Level::Info);
        assert_eq!(parse_level("debug"), Level::Debug);
        assert_eq!(parse_level("verbose"), Level::Info);
    }

    #[test]
    fn urls_lose_their_query_strings() {
        assert_eq!(
            redact("https://query1.finance.yahoo.com/v7/finance/quote?symbols=TCS.NS&crumb=abc"),
            "query1.finance.yahoo.com/v7/finance/quote"
        );
        assert_eq!(
            redact("https://financialmodelingprep.com/api/v3/quote/AAPL?apikey=SECRET"),
            "financialmodelingprep.com/api/v3/quote/AAPL"
        );
        assert_eq!(redact("not a url?token=x"), "not a url");
    }
}
