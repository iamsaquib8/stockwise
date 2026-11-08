//! `stockwise results` argument checks that fail before any request. Runs the
//! real binary against a scratch data directory; nothing here reaches the
//! network or the real data directory.

use std::process::{Command, Stdio};

fn run(args: &[&str]) -> (bool, String) {
    let dir = std::env::temp_dir().join(format!("stockwise-results-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .arg("results")
        .args(args)
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", dir.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run stockwise");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).to_string(),
    )
}

#[test]
fn without_a_symbol_or_scope_it_says_what_to_pass() {
    let (ok, err) = run(&[]);
    assert!(!ok);
    assert!(err.contains("Name a symbol"), "{err}");
}

#[test]
fn annual_needs_a_symbol() {
    let (ok, err) = run(&["--holdings", "--annual"]);
    assert!(!ok);
    assert!(err.contains("--annual"), "{err}");
}

#[test]
fn a_symbol_and_a_scope_conflict() {
    let (ok, err) = run(&["TCS", "--holdings"]);
    assert!(!ok);
    assert!(err.contains("cannot be used with"), "{err}");
}

#[test]
fn the_universe_scoreboard_is_indian_only() {
    let (ok, err) = run(&["--universe", "nifty50", "-m", "us"]);
    assert!(!ok);
    assert!(err.contains("-m in"), "{err}");
}

#[test]
fn quarters_are_bounded() {
    let (ok, err) = run(&["TCS", "--quarters", "99"]);
    assert!(!ok);
    assert!(err.contains("1 to 40"), "{err}");
}
