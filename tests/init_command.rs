//! `stockwise init` with every step given as a flag: no TTY, no prompts, no
//! network. Runs the real binary against a scratch data directory.

use std::process::{Command, Stdio};

#[test]
fn init_imports_a_groww_file_and_prints_holdings() {
    let dir = std::env::temp_dir().join(format!("stockwise-init-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let fixture = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/src/commands/fixtures/groww_holdings.csv"
    );
    let config = dir.join("config.toml");

    let out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["init", "--market", "in", "--import"])
        .arg(format!("groww:{fixture}"))
        .arg("--no-mcp")
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", &config)
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run stockwise");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "init failed: {}\n{stdout}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        stdout.contains("Symbol") && stdout.contains("Avg cost"),
        "{stdout}"
    );
    assert!(stdout.contains("stockwise harvest"), "{stdout}");
    assert!(
        std::fs::read_to_string(&config)
            .unwrap()
            .contains("market = \"in\"")
    );
    let portfolio = std::fs::read_to_string(dir.join("portfolio.json")).unwrap();
    assert!(portfolio.contains("\"holdings\""), "{portfolio}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn init_without_flags_or_tty_prompts_nothing() {
    let dir = std::env::temp_dir().join(format!("stockwise-init-bare-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .arg("init")
        .env("STOCKWISE_HOME", &dir)
        .env("STOCKWISE_CONFIG", dir.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .stdin(Stdio::null())
        .output()
        .expect("run stockwise");
    assert!(out.status.success());
    assert!(
        !dir.join("config.toml").exists(),
        "no flag, no config write"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
