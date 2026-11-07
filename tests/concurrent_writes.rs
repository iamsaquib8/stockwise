//! Commands that change the portfolio at the same moment keep every change:
//! each one takes `portfolio.json.lock`, reloads the file and saves, instead
//! of overwriting what another process saved. Runs the real binary against a
//! scratch data directory; `portfolio add` does not touch the network.

use std::path::{Path, PathBuf};
use std::process::Command;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "stockwise-concurrent-{}-{}",
        name,
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "").unwrap();
    dir
}

fn spawn(home: &Path, args: &[&str]) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(args)
        .env("STOCKWISE_HOME", home)
        .env("STOCKWISE_CONFIG", home.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .stdout(std::process::Stdio::null())
        .spawn()
        .expect("run stockwise")
}

#[test]
fn parallel_portfolio_adds_all_land() {
    let home = scratch("adds");
    let symbols: Vec<String> = (0..12).map(|i| format!("TST{i}")).collect();
    let children: Vec<_> = symbols
        .iter()
        .map(|s| {
            spawn(
                &home,
                &[
                    "portfolio",
                    "add",
                    s,
                    "1",
                    "100",
                    "--date",
                    "2024-01-02",
                    // The symbols are made up; skip the ticker lookup.
                    "--force",
                ],
            )
        })
        .collect();
    for mut c in children {
        assert!(c.wait().unwrap().success());
    }
    let p: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("portfolio.json")).unwrap())
            .unwrap();
    let mut held: Vec<String> = p["holdings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["symbol"].as_str().unwrap().to_string())
        .collect();
    held.sort();
    let mut want: Vec<String> = symbols.iter().map(|s| format!("{s}.NS")).collect();
    want.sort();
    assert_eq!(held, want);
}
