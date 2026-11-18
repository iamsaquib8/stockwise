//! `stockwise use` end to end: it runs on an invalid config file and leaves
//! a valid one behind.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

struct TestHome(PathBuf);

impl TestHome {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let seq = NEXT.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "stockwise-use-command-{}-{seq}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn config(&self) -> PathBuf {
        self.0.join("config.toml")
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_stockwise"))
            .args(args)
            .env("STOCKWISE_HOME", &self.0)
            .env("STOCKWISE_CONFIG", self.config())
            .env("STOCKWISE_KEYRING", "off")
            .env_remove("STOCKWISE_MARKET")
            .env_remove("STOCKWISE_PORTFOLIO")
            .output()
            .unwrap()
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const BROKEN: &str = "[portfolio_markets]\nDEFAULT = \"us\"\ndefault = \"uk\"\n";

/// The `[portfolio_markets]` lines that bind the default portfolio.
fn default_bindings(config: &Path) -> Vec<String> {
    fs::read_to_string(config)
        .unwrap()
        .lines()
        .filter(|l| {
            l.split('=')
                .next()
                .is_some_and(|k| k.trim().trim_matches('"').eq_ignore_ascii_case("default"))
        })
        .map(str::to_string)
        .collect()
}

fn context_json(home: &TestHome) -> serde_json::Value {
    let out = home.run(&["use", "--json"]);
    assert!(
        out.status.success(),
        "`use --json` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

#[test]
fn use_bind_repairs_case_equivalent_bindings() {
    let home = TestHome::new();
    fs::write(home.config(), BROKEN).unwrap();
    // Other commands refuse the broken file.
    let out = home.run(&["quote", "TCS"]);
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("twice"));

    let out = home.run(&["use", "default", "ca", "--bind"]);
    assert!(
        out.status.success(),
        "`use default ca --bind` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(default_bindings(&home.config()), ["default = \"ca\""]);

    // The file is valid again, and the context reads it back.
    let ctx = context_json(&home);
    assert_eq!(ctx["market"], "ca");
    assert_eq!(ctx["portfolio_market"], "ca");
    assert_eq!(ctx["bindings"], serde_json::json!({ "default": "ca" }));
}

#[test]
fn use_unbind_removes_every_spelling() {
    let home = TestHome::new();
    fs::write(home.config(), BROKEN).unwrap();
    let out = home.run(&["use", "default", "--unbind"]);
    assert!(
        out.status.success(),
        "`use default --unbind` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(default_bindings(&home.config()).is_empty());
    let ctx = context_json(&home);
    assert_eq!(ctx["bindings"], serde_json::json!({}));
    assert!(ctx["portfolio_market"].is_null());
}
