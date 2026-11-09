//! Tests for `stockwise profile` CLI commands, flags, environment variables,
//! and profile overrides.

use std::process::{Command, Stdio};

#[test]
fn profile_show_default_and_json() {
    let dir = std::env::temp_dir().join(format!("sw-test-profile-show-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let empty_cfg = dir.join("empty.toml");
    std::fs::write(&empty_cfg, "").unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["profile", "--json"])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &empty_cfg)
        .env_remove("STOCKWISE_PROFILE")
        .stdin(Stdio::null())
        .output()
        .expect("run profile --json");

    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&text).expect("valid json");
    assert_eq!(v["mode"], "active");
    assert_eq!(v["risk"], "med");
    assert_eq!(v["spec"], "active:med");
    assert_eq!(v["max_stock_weight"], 12.0);
    assert_eq!(v["max_smallcap_weight"], 25.0);
    assert_eq!(v["risk_per_trade"], 1.0);
    assert_eq!(v["mode_source"], "default");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profile_flag_and_env_precedence() {
    let dir =
        std::env::temp_dir().join(format!("sw-test-profile-precedence-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.toml");
    std::fs::write(
        &cfg,
        r#"[profile]
mode = "active"
risk = "low"
tax_slab = 20.0
"#,
    )
    .unwrap();

    // 1. Config file alone
    let out1 = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["profile", "--json"])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &cfg)
        .env_remove("STOCKWISE_PROFILE")
        .stdin(Stdio::null())
        .output()
        .expect("run profile --json");
    assert!(out1.status.success());
    let v1: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out1.stdout)).unwrap();
    assert_eq!(v1["mode"], "active");
    assert_eq!(v1["risk"], "low");
    assert_eq!(v1["tax_slab"], 20.0);
    assert_eq!(v1["mode_source"], "file");

    // 2. Env var overrides config file
    let out2 = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["profile", "--json"])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &cfg)
        .env("STOCKWISE_PROFILE", "passive")
        .stdin(Stdio::null())
        .output()
        .expect("run profile --json with env");
    assert!(out2.status.success());
    let v2: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out2.stdout)).unwrap();
    assert_eq!(v2["mode"], "passive");
    assert_eq!(v2["risk"], "low");
    assert_eq!(v2["mode_source"], "env");

    // 3. CLI flag overrides both env var and config file
    let out3 = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["--profile", "ultra", "profile", "--json"])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &cfg)
        .env("STOCKWISE_PROFILE", "passive")
        .stdin(Stdio::null())
        .output()
        .expect("run profile --json with flag");
    assert!(out3.status.success());
    let v3: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out3.stdout)).unwrap();
    assert_eq!(v3["mode"], "ultra");
    assert_eq!(v3["risk"], "high");
    assert_eq!(v3["mode_source"], "flag");
    assert_eq!(v3["max_stock_weight"], 20.0);
    assert_eq!(v3["max_smallcap_weight"], 50.0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn profile_set_and_reset_flow() {
    let dir = std::env::temp_dir().join(format!("sw-test-profile-set-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let cfg = dir.join("config.toml");

    // Set passive profile with custom guardrails
    let set_out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args([
            "profile",
            "set",
            "passive",
            "--tax-slab",
            "15",
            "--horizon-years",
            "12",
            "--max-stock-weight",
            "8.5",
            "--json",
        ])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &cfg)
        .env_remove("STOCKWISE_PROFILE")
        .stdin(Stdio::null())
        .output()
        .expect("run profile set");
    assert!(
        set_out.status.success(),
        "profile set failed: {}",
        String::from_utf8_lossy(&set_out.stderr)
    );
    let v_set: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&set_out.stdout)).unwrap();
    assert_eq!(v_set["mode"], "passive");
    assert_eq!(v_set["tax_slab"], 15.0);
    assert_eq!(v_set["horizon_years"], 12);
    assert_eq!(v_set["max_stock_weight"], 8.5);

    // Verify config file was written
    let file_text = std::fs::read_to_string(&cfg).unwrap();
    assert!(file_text.contains("mode = \"passive\""), "{file_text}");
    assert!(file_text.contains("tax_slab = 15"), "{file_text}");

    // Reset profile
    let reset_out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["profile", "reset"])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &cfg)
        .stdin(Stdio::null())
        .output()
        .expect("run profile reset");
    assert!(reset_out.status.success());

    // Show profile after reset returns defaults
    let show_out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["profile", "--json"])
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &cfg)
        .env_remove("STOCKWISE_PROFILE")
        .stdin(Stdio::null())
        .output()
        .expect("run profile show after reset");
    assert!(show_out.status.success());
    let v_reset: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&show_out.stdout)).unwrap();
    assert_eq!(v_reset["mode"], "active");
    assert_eq!(v_reset["mode_source"], "default");

    let _ = std::fs::remove_dir_all(&dir);
}
