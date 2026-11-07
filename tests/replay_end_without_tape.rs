//! `sim replay end` scores a replay from the marks and fills saved in
//! `sim_replay.json`. It once also demanded the price file
//! (`sim_replay_tape.json`), so a replay whose tape was deleted could be
//! neither ended nor restarted. Runs the real binary on a scratch data
//! directory; nothing here touches the network.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stockwise-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("config.toml"), "").unwrap();
    dir
}

fn stockwise(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(args)
        .env("STOCKWISE_HOME", home)
        .env("STOCKWISE_CONFIG", home.join("config.toml"))
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .output()
        .expect("run stockwise")
}

#[test]
fn replay_end_works_when_the_price_file_is_gone() {
    let home = scratch("replay-end");
    // A replay three days in, with no sim_replay_tape.json beside it.
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sim_replay_three_days.json"),
        home.join("sim_replay.json"),
    )
    .unwrap();
    assert!(!home.join("sim_replay_tape.json").exists());

    // Other commands say the file is missing and point at `end`.
    let status = stockwise(&home, &["sim", "replay", "status"]);
    assert!(!status.status.success());
    let err = String::from_utf8_lossy(&status.stderr);
    assert!(
        err.contains("price file") && err.contains("sim replay end"),
        "{err}"
    );

    let end = stockwise(&home, &["sim", "replay", "end", "--json"]);
    assert!(
        end.status.success(),
        "replay end failed: {}",
        String::from_utf8_lossy(&end.stderr)
    );
    assert!(
        !home.join("sim_replay.json").exists(),
        "the replay is cleared"
    );
}
