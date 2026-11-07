//! `daemon intraday --once` on a day that is already settled must exit at
//! once. It used to wait for the next trading day and then run that session.
//! Runs the real binary on a scratch data directory; nothing here touches the
//! network.

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

#[test]
fn once_on_a_settled_day_exits_immediately() {
    let home: PathBuf =
        std::env::temp_dir().join(format!("stockwise-once-settled-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"), "").unwrap();

    // Today's session date in India (UTC+5:30), a settled day with no trades.
    let today = (chrono::Utc::now() + chrono::Duration::minutes(330))
        .date_naive()
        .to_string();
    let state = format!(
        r#"{{"date":"{today}","market":"IN","capital":25000.0,"target_pct":2.0,
        "entered":false,"positions":[],
        "risk":{{"capital":25000.0,"daily_loss_limit_pct":3.0,"max_positions":5,
                 "max_position_pct":30.0,"realized_pnl":0.0,"killed":false}},
        "tick_count":0,"settled":true,"updated_at":"{today}T10:00:00Z"}}"#
    );
    std::fs::write(home.join("daemon_state.json"), state).unwrap();

    let mut child = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(["daemon", "intraday", "--once"])
        .env("STOCKWISE_HOME", &home)
        .env("STOCKWISE_CONFIG", home.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start stockwise");

    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s;
        }
        if start.elapsed() > Duration::from_secs(20) {
            let _ = child.kill();
            panic!("`daemon intraday --once` kept waiting on a settled day");
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let out = child.wait_with_output().unwrap();
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(
        status.success(),
        "{text}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        text.contains("already settled") && text.contains("--once"),
        "{text}"
    );
    let _ = std::fs::remove_dir_all(&home);
}
