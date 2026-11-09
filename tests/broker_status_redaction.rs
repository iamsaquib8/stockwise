//! CLI-level guards against leaking configured broker identifiers in diagnostics.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

struct TestHome(PathBuf);

impl TestHome {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "stockwise-broker-status-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn trade_config(&self, broker: &str, filename: &str, config: &str) -> String {
        fs::write(self.0.join(filename), config).unwrap();
        let empty_config = self.0.join("no-config.toml");
        fs::write(&empty_config, "").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_stockwise"))
            .args(["trade", "--broker", broker, "config"])
            .env("STOCKWISE_HOME", &self.0)
            .env("STOCKWISE_CONFIG", &empty_config)
            .env("STOCKWISE_KEYRING", "off")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "trade config failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn angel_trade_config_never_prints_raw_client_id() {
    let home = TestHome::new();
    let output = home.trade_config(
        "angel",
        "angel_config.json",
        r#"{
            "api_key": "angel-test-key",
            "client_id": "A1234567",
            "password": "test-pin",
            "totp_secret": "test-totp"
        }"#,
    );
    assert!(output.contains("client ••67"), "{output}");
    assert!(!output.contains("A1234567"), "{output}");
}

#[test]
fn app_trade_config_never_prints_raw_app_or_login_id() {
    let home = TestHome::new();
    let output = home.trade_config(
        "zerodha",
        "zerodha_config.json",
        r#"{
            "api_key": "kite-secret-app-1234",
            "api_secret": "test-app-secret",
            "access_token": "test-access-token",
            "user_id": "AB5678",
            "session_created": "2026-11-08T12:00:00+05:30"
        }"#,
    );
    assert!(output.contains("app ••34"), "{output}");
    assert!(output.contains("logged in as ••78"), "{output}");
    assert!(!output.contains("kite-secret-app-1234"), "{output}");
    assert!(!output.contains("AB5678"), "{output}");
}
