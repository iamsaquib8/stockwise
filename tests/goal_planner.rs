//! `stockwise goal` offline: horizon-matched defaults, post-tax numbers, the
//! forced-equity warning and saved goals in `goals.json`. Runs the real binary
//! against a scratch data directory.

use serde_json::Value;
use std::path::Path;
use std::process::{Command, Stdio};

fn run(home: &Path, args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_stockwise"))
        .args(args)
        .env("STOCKWISE_HOME", home)
        .env_remove("STOCKWISE_CONFIG")
        .env_remove("STOCKWISE_MARKET")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("run stockwise");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// A scratch data directory, removed when the test ends.
struct Scratch(std::path::PathBuf);

impl std::ops::Deref for Scratch {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.0
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn scratch(name: &str) -> Scratch {
    let dir = std::env::temp_dir().join(format!("stockwise-goal-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    Scratch(dir)
}

fn json(home: &Path, args: &[&str]) -> Value {
    let (ok, out, err) = run(home, args);
    assert!(ok, "{args:?} failed: {err}\n{out}");
    serde_json::from_str(&out).unwrap_or_else(|e| panic!("{args:?}: {e}\n{out}"))
}

#[test]
fn a_one_year_goal_is_capital_preservation_with_tax_and_a_month_table() {
    let home = scratch("one-year");
    let v = json(
        &home,
        &["goal", "sip", "--target", "10L", "--years", "1", "--json"],
    );
    assert_eq!(v["goal"], "sip");
    assert_eq!(v["months"], 12);
    assert_eq!(v["annual_return_pct"], 6.75);
    assert_eq!(v["regime"]["kind"], "short");
    assert_eq!(v["monthly_schedule"].as_array().unwrap().len(), 12);
    // Redeeming on day 365: 11 of the 12 monthly lots are under 12 months.
    assert_eq!(v["tax"]["short_term_lots"], 11);
    assert_eq!(v["tax"]["long_term_lots"], 1);
    let gross_sip = v["monthly_sip"].as_f64().unwrap();
    assert!(v["monthly_sip_for_net_target"].as_f64().unwrap() > gross_sip);
    assert!(v["net_after_tax"].as_f64().unwrap() < 1_000_000.0);
    let p = &v["percentiles"];
    assert!(p["p10"].as_f64().unwrap() <= p["p50"].as_f64().unwrap());
    assert!(p["p50"].as_f64().unwrap() <= p["p90"].as_f64().unwrap());
    assert_eq!(v["warnings"].as_array().unwrap().len(), 0);
    // The documented keys of the old plan are still there.
    for k in [
        "target",
        "years",
        "total_invested",
        "gains",
        "lump_sum_today",
        "schedule",
        "currency",
    ] {
        assert!(v.get(k).is_some(), "missing {k}");
    }

    let (ok, text, _) = run(&home, &["goal", "sip", "--target", "10L", "--years", "1"]);
    assert!(ok);
    assert!(text.contains("in 1 year"), "{text}");
    assert!(!text.contains("1 years"), "{text}");
    assert!(text.contains("Month by month"), "{text}");
    assert!(!text.contains("Year by year"), "{text}");
}

#[test]
fn forcing_an_equity_return_on_a_short_goal_warns() {
    let home = scratch("warn");
    let v = json(
        &home,
        &[
            "goal", "sip", "--target", "10L", "--months", "12", "--return", "12", "--json",
        ],
    );
    let warnings = v["warnings"].as_array().unwrap();
    assert_eq!(warnings.len(), 1, "{v}");
    assert!(warnings[0].as_str().unwrap().contains("10th-percentile"));
    assert_eq!(v["mismatch"]["safe_return_pct"], 6.75);
    assert!(v["mismatch"]["shortfall_prob_pct"].as_f64().unwrap() > 0.0);
}

#[test]
fn sub_year_horizons_and_target_dates_work() {
    let home = scratch("horizons");
    let v = json(
        &home,
        &[
            "goal",
            "sip",
            "--target",
            "5L",
            "--horizon",
            "18mo",
            "--json",
        ],
    );
    assert_eq!(v["months"], 18);
    assert_eq!(v["years"], 2);
    let v = json(
        &home,
        &["goal", "sip", "--target", "5L", "--months", "6", "--json"],
    );
    assert_eq!(v["months"], 6);
    let (ok, _, err) = run(&home, &["goal", "sip", "--target", "5L", "--by", "2001-01"]);
    assert!(!ok);
    assert!(err.contains("not in the future"), "{err}");
    let (ok, _, _) = run(&home, &["goal", "sip", "--target", "5L"]);
    assert!(!ok, "a horizon is required");
}

#[test]
fn inflation_and_net_targets_change_the_sip() {
    let home = scratch("net");
    let plain = json(
        &home,
        &["goal", "sip", "--target", "1cr", "--years", "10", "--json"],
    );
    let infl = json(
        &home,
        &[
            "goal",
            "sip",
            "--target",
            "1cr",
            "--years",
            "10",
            "--inflation",
            "6",
            "--json",
        ],
    );
    assert!(infl["monthly_sip"].as_f64() > plain["monthly_sip"].as_f64());
    assert_eq!(infl["target_today"], 10_000_000.0);
    let net = json(
        &home,
        &[
            "goal", "sip", "--target", "1cr", "--years", "10", "--net", "--json",
        ],
    );
    assert!(net["monthly_sip"].as_f64() > plain["monthly_sip"].as_f64());
    assert!((net["net_after_tax"].as_f64().unwrap() - 10_000_000.0).abs() < 1.0);
}

#[test]
fn saved_goals_persist_in_goals_json() {
    let home = scratch("saved");
    let (ok, out, err) = run(
        &home,
        &[
            "goal",
            "add",
            "house",
            "--target",
            "50L",
            "--by",
            "2060-06",
            "--inflation",
            "6",
        ],
    );
    assert!(ok, "{err}\n{out}");
    assert!(home.join("goals.json").exists());
    let (ok, _, err) = run(
        &home,
        &["goal", "add", "House", "--target", "1L", "--by", "2060-06"],
    );
    assert!(!ok && err.contains("exists"), "{err}");

    let v = json(&home, &["goal", "list", "--json"]);
    let goals = v["goals"].as_array().unwrap();
    assert_eq!(goals.len(), 1);
    assert_eq!(goals[0]["name"], "house");
    assert_eq!(goals[0]["status"], "needs_sip");
    assert!(goals[0]["target_future"].as_f64().unwrap() > 5_000_000.0);
    assert_eq!(goals[0]["funded_pct"], 0.0);

    let (_, text, _) = run(&home, &["goal"]);
    assert!(text.contains("house"), "{text}");
    let shown = json(&home, &["goal", "show", "house", "--json"]);
    assert_eq!(shown["name"], "house");
    assert!(shown["plan"]["monthly_sip"].as_f64().unwrap() > 0.0);

    let (ok, _, _) = run(&home, &["goal", "remove", "house"]);
    assert!(ok);
    assert_eq!(
        json(&home, &["goal", "list", "--json"])["goals"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    let (ok, _, err) = run(&home, &["goal", "remove", "house"]);
    assert!(!ok && err.contains("No saved goal"), "{err}");
}

#[test]
fn retire_swp_emergency_and_prepay_report_their_numbers() {
    let home = scratch("calculators");
    let r = json(
        &home,
        &[
            "goal",
            "retire",
            "--monthly-expense",
            "80k",
            "--years-to-retire",
            "20",
            "--years-in-retirement",
            "30",
            "--json",
        ],
    );
    assert!(r["initial_withdrawal_rate_pct"].as_f64().unwrap() > 0.0);
    assert!(r["crash_survival_years"].as_f64().unwrap() < r["survival_years"].as_f64().unwrap());

    let s = json(
        &home,
        &[
            "goal",
            "swp",
            "--corpus",
            "1cr",
            "--withdrawal",
            "50k",
            "--json",
        ],
    );
    assert_eq!(s["goal"], "swp");
    assert!(
        s["fd"]["runway_months"].as_u64().unwrap() < s["swp"]["runway_months"].as_u64().unwrap()
    );

    let e = json(
        &home,
        &[
            "goal",
            "emergency",
            "--monthly-expense",
            "60k",
            "--savings",
            "1L",
            "--json",
        ],
    );
    assert_eq!(e["target"], 360_000.0);
    assert!(e["build"]["monthly_sip"].as_f64().unwrap() > 0.0);

    let p = json(
        &home,
        &[
            "goal",
            "prepay",
            "--principal",
            "50L",
            "--loan-rate",
            "9",
            "--remaining-months",
            "240",
            "--extra",
            "10k",
            "--json",
        ],
    );
    assert!(p["interest_saved"].as_f64().unwrap() > 0.0);
    assert!(p["tenure_saved_months"].as_u64().unwrap() > 0);
    assert!(p["breakeven_return_pct"].as_f64().unwrap() > 9.0);
}
