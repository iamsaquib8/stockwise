//! `--cap` scans served from fresh `cache.db` snapshots, against the real
//! binary and a scratch data directory. No network: the scan finds the
//! stored rows and never asks a data source.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// A scratch data directory, removed when the test ends.
struct Scratch(PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("stockwise-cap-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "").unwrap();
    Scratch(dir)
}

fn stockwise(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(args)
        .env("STOCKWISE_HOME", home)
        .env("STOCKWISE_CONFIG", home.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .env_remove("STOCKWISE_REDIS_URL")
        // Anything that does reach for the network fails at once.
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("NO_COLOR", "1")
        .output()
        .expect("run stockwise")
}

/// The market's built-in list (the symbols a plain scan covers).
const NSE_LIST: [&str; 30] = [
    "RELIANCE",
    "TCS",
    "HDFCBANK",
    "INFY",
    "ICICIBANK",
    "HINDUNILVR",
    "SBIN",
    "BHARTIARTL",
    "ITC",
    "KOTAKBANK",
    "LT",
    "AXISBANK",
    "ASIANPAINT",
    "MARUTI",
    "TITAN",
    "SUNPHARMA",
    "TMPV",
    "BAJFINANCE",
    "WIPRO",
    "HCLTECH",
    "ONGC",
    "NTPC",
    "POWERGRID",
    "TATASTEEL",
    "ADANIENT",
    "ULTRACEMCO",
    "JSWSTEEL",
    "TECHM",
    "INDUSINDBK",
    "DRREDDY",
];

/// 30 Indian rows: ten large, twenty mid, changes alternating in sign.
fn seed(home: &Path) {
    // The first run creates cache.db with its tables.
    let out = stockwise(home, &["cache", "snapshots"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let conn = rusqlite::Connection::open(home.join("cache.db")).unwrap();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    for (i, name) in NSE_LIST.iter().enumerate() {
        let (tier, mcap) = if i < 10 {
            ("large", 2.0e12)
        } else {
            ("mid", 5.0e11)
        };
        let change = if i % 2 == 0 {
            1.0 + i as f64
        } else {
            -1.0 - i as f64
        };
        conn.execute(
            "INSERT INTO market_snapshots (symbol, market, name, cap_tier, mcap, price, change_1d, updated_at)
             VALUES (?1, 'in', ?2, ?3, ?4, 100.0, ?5, ?6)",
            rusqlite::params![format!("{name}.NS"), format!("Stock {i}"), tier, mcap, change, now],
        )
        .unwrap();
    }
}

#[test]
fn movers_cap_reads_fresh_snapshots_without_network() {
    let home = scratch("movers");
    seed(&home);
    let out = stockwise(&home, &["-m", "in", "movers", "--cap", "mid", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows: Vec<&serde_json::Value> = v["gainers"]
        .as_array()
        .unwrap()
        .iter()
        .chain(v["losers"].as_array().unwrap())
        .collect();
    assert!(!rows.is_empty());
    for r in &rows {
        assert_eq!(r["cap_tier"], "mid", "{r}");
        let sym = r["symbol"].as_str().unwrap().trim_end_matches(".NS");
        let n = NSE_LIST.iter().position(|s| *s == sym).unwrap();
        assert!(n >= 10, "large stock in a mid scan: {r}");
    }
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("cache.db"), "{err}");
}

#[test]
fn screen_cap_filters_and_tags_rows() {
    let home = scratch("screen");
    seed(&home);
    let out = stockwise(
        &home,
        &["-m", "in", "screen", "momentum", "--cap", "large", "--json"],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v.as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r["cap_tier"] == "large"), "{v}");
}

#[test]
fn unknown_cap_value_is_rejected() {
    let home = scratch("bad");
    let out = stockwise(&home, &["movers", "--cap", "huge"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--cap"));
}
