//! `portfolio add`, `watch add` and `alert add` keep `MF:<AMFI code>` and
//! `MF:<ISIN>` as typed in every market — they once stored `MF:120503.NS`,
//! which nothing could price. Runs the real binary against a scratch data
//! directory; none of these commands touches the network.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "stockwise-namespaced-{}-{}",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // An empty config file: built-in defaults, not the developer's own.
    std::fs::write(dir.join("config.toml"), "").unwrap();
    dir
}

fn stockwise(home: &Path, args: &[&str]) {
    let out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(args)
        .env("STOCKWISE_HOME", home)
        .env("STOCKWISE_CONFIG", home.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .output()
        .expect("run stockwise");
    assert!(
        out.status.success(),
        "stockwise {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn read_json(home: &Path, file: &str) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(home.join(file)).unwrap()).unwrap()
}

const MARKETS: &[&str] = &["in", "us", "crypto", "fx", "commodity", "uk", "hk"];
const FUNDS: &[(&str, &str)] = &[
    ("MF:120503", "MF:120503"),
    ("mf:inf179k01ut0", "MF:INF179K01UT0"),
];

#[test]
fn portfolio_add_keeps_fund_symbols() {
    for market in MARKETS {
        let home = scratch(&format!("portfolio-{market}"));
        for (typed, _) in FUNDS {
            stockwise(
                &home,
                &["-m", market, "portfolio", "add", typed, "10", "50"],
            );
        }
        let p = read_json(&home, "portfolio.json");
        let symbols: Vec<&str> = p["holdings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["symbol"].as_str().unwrap())
            .collect();
        let want: Vec<&str> = FUNDS.iter().map(|(_, stored)| *stored).collect();
        assert_eq!(symbols, want, "-m {market}");
    }
}

#[test]
fn watch_add_keeps_fund_symbols() {
    for market in MARKETS {
        let home = scratch(&format!("watch-{market}"));
        let typed: Vec<&str> = FUNDS.iter().map(|(t, _)| *t).collect();
        let mut args = vec!["-m", market, "watch", "add"];
        args.extend(&typed);
        stockwise(&home, &args);
        let wl = read_json(&home, "watchlist.json");
        let want: Vec<&str> = FUNDS.iter().map(|(_, stored)| *stored).collect();
        assert_eq!(wl["symbols"], serde_json::json!(want), "-m {market}");
    }
}

#[test]
fn alert_add_keeps_fund_symbols() {
    for market in MARKETS {
        let home = scratch(&format!("alert-{market}"));
        for (typed, _) in FUNDS {
            stockwise(&home, &["-m", market, "alert", "add", typed, "above", "60"]);
        }
        let store = read_json(&home, "alerts.json");
        let symbols: Vec<&str> = store["alerts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["symbol"].as_str().unwrap())
            .collect();
        let want: Vec<&str> = FUNDS.iter().map(|(_, stored)| *stored).collect();
        assert_eq!(symbols, want, "-m {market}");
    }
}

#[test]
fn symbols_stored_by_older_versions_are_repaired() {
    let home = scratch("repair");
    std::fs::write(
        home.join("watchlist.json"),
        r#"{"symbols": ["MF:120503.NS", "TCS.NS"]}"#,
    )
    .unwrap();
    // Any change to the watchlist saves the repaired symbols.
    stockwise(&home, &["watch", "add", "INFY"]);
    let wl = read_json(&home, "watchlist.json");
    assert_eq!(
        wl["symbols"],
        serde_json::json!(["MF:120503", "TCS.NS", "INFY.NS"])
    );
    // And removing by the real code finds it.
    stockwise(&home, &["watch", "remove", "MF:120503"]);
    let wl = read_json(&home, "watchlist.json");
    assert_eq!(wl["symbols"], serde_json::json!(["TCS.NS", "INFY.NS"]));
}
