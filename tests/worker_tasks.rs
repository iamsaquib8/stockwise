//! `worker` and `tasks` against the real binary and a scratch data directory:
//! a queued `run` task is picked up by the background worker, finishes with
//! its output stored, and a queued task can be cancelled. No network.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("stockwise-worker-{name}-{}", std::process::id()));
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
        .env_remove("STOCKWISE_REDIS_URL")
        .env("NO_COLOR", "1")
        .output()
        .expect("run stockwise")
}

fn ok(home: &Path, args: &[&str]) -> String {
    let out = stockwise(home, args);
    assert!(
        out.status.success(),
        "stockwise {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Stops the worker even when an assertion fails.
struct Worker<'a>(&'a Path);
impl Drop for Worker<'_> {
    fn drop(&mut self) {
        let _ = stockwise(self.0, &["worker", "stop"]);
    }
}

#[test]
fn queued_task_runs_in_the_worker_and_keeps_its_output() {
    let home = scratch("run");
    ok(&home, &["tasks", "add", "run", "--", "--version"]);
    let list = ok(&home, &["tasks"]);
    assert!(list.contains("queued"), "{list}");

    let _worker = Worker(&home);
    ok(&home, &["worker", "start"]);
    // `start` returns before the child writes its pid file; "not running"
    // also contains "running", so wait for the pid itself.
    let started = Instant::now() + Duration::from_secs(5);
    loop {
        if ok(&home, &["worker", "status"]).contains("(pid ") {
            break;
        }
        assert!(Instant::now() < started, "worker never reported a pid");
        std::thread::sleep(Duration::from_millis(100));
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    let task = loop {
        let json = ok(&home, &["tasks", "show", "1", "--json"]);
        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        if v["status"] == "done" || v["status"] == "failed" || Instant::now() > deadline {
            break v;
        }
        std::thread::sleep(Duration::from_millis(300));
    };
    assert_eq!(task["status"], "done", "{task}");
    assert_eq!(task["progress_pct"], 100.0);
    assert_eq!(task["result"]["exit_code"], 0);
    assert!(
        task["result"]["stdout"]
            .as_str()
            .unwrap()
            .contains("stockwise"),
        "{task}"
    );

    assert!(ok(&home, &["worker", "stop"]).contains("stopped"));
    assert!(ok(&home, &["worker", "status"]).contains("not running"));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn a_queued_task_can_be_cancelled() {
    let home = scratch("cancel");
    ok(&home, &["tasks", "add", "run", "--", "--version"]);
    ok(&home, &["tasks", "cancel", "1"]);
    let v: serde_json::Value =
        serde_json::from_str(&ok(&home, &["tasks", "show", "1", "--json"])).unwrap();
    assert_eq!(v["status"], "cancelled");
    // Already finished: cancelling again is an error, not a silent success.
    assert!(!stockwise(&home, &["tasks", "cancel", "1"]).status.success());
    assert!(!stockwise(&home, &["tasks", "show", "99"]).status.success());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn run_task_without_a_command_is_refused() {
    let home = scratch("norun");
    assert!(!stockwise(&home, &["tasks", "add", "run"]).status.success());
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn only_one_worker_owns_the_data_directory() {
    let home = scratch("owner");
    let _worker = Worker(&home);
    ok(&home, &["worker", "start"]);
    // A second start, and a second foreground worker, are refused.
    let again = stockwise(&home, &["worker", "start"]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("already running"));
    assert!(!stockwise(&home, &["worker", "run"]).status.success());
    // The first one is untouched and still stoppable.
    assert!(ok(&home, &["worker", "status"]).contains("(pid "));
    assert!(ok(&home, &["worker", "stop"]).contains("stopped"));
    // After a clean stop the pid file is gone and a new worker can start.
    assert!(!home.join("worker.pid").exists());
    ok(&home, &["worker", "start"]);
    assert!(ok(&home, &["worker", "stop"]).contains("stopped"));
    let _ = std::fs::remove_dir_all(&home);
}

/// The stop file is how `worker stop` reaches a Windows worker; the worker
/// honours it on every platform, so it is testable here.
#[test]
fn a_stop_file_shuts_the_worker_down_cleanly() {
    let home = scratch("stopfile");
    let _worker = Worker(&home);
    ok(&home, &["worker", "start"]);
    std::fs::write(home.join("worker.stop"), b"stop").unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !ok(&home, &["worker", "status"]).contains("not running") {
        assert!(Instant::now() < deadline, "worker ignored the stop file");
        std::thread::sleep(Duration::from_millis(200));
    }
    // It cleaned up after itself: no pid file, no leftover stop request.
    assert!(!home.join("worker.pid").exists());
    assert!(!home.join("worker.stop").exists());
    let _ = std::fs::remove_dir_all(&home);
}
