//! `worker install`, `worker uninstall` and `worker service-status`: run the
//! background worker (`stockwise worker run`) at login as a per-user
//! launchd agent on macOS or a systemd user service on Linux.
//!
//! Only user-level locations are touched: `~/Library/LaunchAgents` and
//! `~/.config/systemd/user` (`$XDG_CONFIG_HOME`). The unit text is built by
//! pure functions so `--dry-run` and the tests see exactly what is written.

use super::*;
use std::path::{Path, PathBuf};

/// launchd job label, also the plist's file name.
pub const LAUNCHD_LABEL: &str = "com.stockwise.worker";
/// systemd user unit name.
pub const SYSTEMD_UNIT: &str = "stockwise-worker.service";
/// Where the worker's output goes on macOS, in the data directory (the same
/// file `worker start` appends to).
const LOG_FILE: &str = "worker.log";
/// Settings carried into the service when set here. Secrets (SMTP passwords,
/// bot tokens) are not: they would sit in plain text in the unit file; put
/// them in the config file instead.
const PASS_ENV: &[&str] = &["STOCKWISE_HOME", "STOCKWISE_CONFIG", "STOCKWISE_PORTFOLIO"];

/// The service manager to write for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Launchd,
    Systemd,
}

impl Platform {
    /// This machine's, or why there is none.
    pub fn current() -> Result<Self> {
        if cfg!(target_os = "macos") {
            Ok(Platform::Launchd)
        } else if cfg!(target_os = "linux") {
            Ok(Platform::Systemd)
        } else {
            bail!(
                "`worker install` supports macOS (launchd) and Linux (systemd user services). \
                 Here, start the worker yourself at login with `stockwise worker start`."
            )
        }
    }
}

/// What the service runs.
#[derive(Clone, Debug)]
pub struct ServiceSpec {
    pub exe: PathBuf,
    /// Arguments after the executable, e.g. `-m in worker run`.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// stdout/stderr file (launchd; systemd uses the journal).
    pub log: PathBuf,
    /// Wait before restarting a worker that exited with an error.
    pub restart_secs: u64,
}

/// Escape text for an XML element.
pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

/// The launchd agent: runs the worker at login, restarts it after a crash
/// (not after `worker stop`, which exits cleanly), logs to the data directory.
pub fn launchd_plist(spec: &ServiceSpec) -> String {
    let s = |v: &str| format!("<string>{}</string>", xml_escape(v));
    let mut out = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n",
    );
    out.push_str(&format!("\t<key>Label</key>\n\t{}\n", s(LAUNCHD_LABEL)));
    out.push_str("\t<key>ProgramArguments</key>\n\t<array>\n");
    out.push_str(&format!("\t\t{}\n", s(&spec.exe.to_string_lossy())));
    for a in &spec.args {
        out.push_str(&format!("\t\t{}\n", s(a)));
    }
    out.push_str("\t</array>\n");
    if !spec.env.is_empty() {
        out.push_str("\t<key>EnvironmentVariables</key>\n\t<dict>\n");
        for (k, v) in &spec.env {
            out.push_str(&format!("\t\t<key>{}</key>\n\t\t{}\n", xml_escape(k), s(v)));
        }
        out.push_str("\t</dict>\n");
    }
    let log = spec.log.to_string_lossy();
    out.push_str(&format!(
        "\t<key>RunAtLoad</key>\n\t<true/>\n\
         \t<key>KeepAlive</key>\n\t<dict>\n\t\t<key>SuccessfulExit</key>\n\t\t<false/>\n\t</dict>\n\
         \t<key>ThrottleInterval</key>\n\t<integer>{}</integer>\n\
         \t<key>StandardOutPath</key>\n\t{}\n\
         \t<key>StandardErrorPath</key>\n\t{}\n",
        spec.restart_secs,
        s(&log),
        s(&log)
    ));
    out.push_str("</dict>\n</plist>\n");
    out
}

/// Quote one systemd argument or assignment: double quotes, `\` and `"`
/// backslash-escaped, `%` doubled (specifiers), and in `ExecStart=` `$`
/// doubled (variable expansion).
pub fn systemd_quote(s: &str, exec: bool) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '%' => out.push_str("%%"),
            '$' if exec => out.push_str("$$"),
            '\n' => out.push_str("\\n"),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The systemd user service: started at login (`default.target`), restarted
/// after a failure, output in the journal.
pub fn systemd_unit(spec: &ServiceSpec) -> String {
    let exec: Vec<String> = std::iter::once(spec.exe.to_string_lossy().into_owned())
        .chain(spec.args.iter().cloned())
        .map(|a| systemd_quote(&a, true))
        .collect();
    let mut out = String::from(
        "[Unit]\n\
         Description=StockWise background worker (quotes, tasks, alerts, post-close sync)\n\
         After=network-online.target\n\n\
         [Service]\n\
         Type=simple\n",
    );
    out.push_str(&format!("ExecStart={}\n", exec.join(" ")));
    for (k, v) in &spec.env {
        out.push_str(&format!(
            "Environment={}\n",
            systemd_quote(&format!("{k}={v}"), false)
        ));
    }
    out.push_str(&format!(
        "Restart=on-failure\nRestartSec={}\n\n[Install]\nWantedBy=default.target\n",
        spec.restart_secs
    ));
    out
}

/// One command of a plan.
#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub cmd: Vec<String>,
    /// Failure is expected sometimes (unloading a job that is not loaded).
    pub ignore_failure: bool,
    /// Tried when `cmd` fails (older `launchctl` without `bootstrap`).
    pub fallback: Option<Vec<String>>,
}

impl Step {
    fn new(cmd: &[&str]) -> Self {
        Step {
            cmd: cmd.iter().map(|s| s.to_string()).collect(),
            ignore_failure: false,
            fallback: None,
        }
    }

    fn optional(mut self) -> Self {
        self.ignore_failure = true;
        self
    }

    fn or(mut self, fallback: &[&str]) -> Self {
        self.fallback = Some(fallback.iter().map(|s| s.to_string()).collect());
        self
    }
}

/// The unit file's path for `platform` under `home` (macOS) or the
/// config directory (Linux).
pub fn unit_path(platform: Platform, home: &Path, config_dir: &Path) -> PathBuf {
    match platform {
        Platform::Launchd => home
            .join("Library/LaunchAgents")
            .join(format!("{LAUNCHD_LABEL}.plist")),
        Platform::Systemd => config_dir.join("systemd/user").join(SYSTEMD_UNIT),
    }
}

/// The unit text for `platform`.
pub fn unit_text(platform: Platform, spec: &ServiceSpec) -> String {
    match platform {
        Platform::Launchd => launchd_plist(spec),
        Platform::Systemd => systemd_unit(spec),
    }
}

/// Commands that load the unit at `path` after it is written. `reinstall`:
/// a unit was there before, so the running service is replaced.
pub fn install_steps(platform: Platform, path: &Path, uid: &str, reinstall: bool) -> Vec<Step> {
    let p = path.to_string_lossy();
    match platform {
        Platform::Launchd => {
            let domain = format!("gui/{uid}");
            let target = format!("{domain}/{LAUNCHD_LABEL}");
            vec![
                // A loaded job must be unloaded before bootstrap can load the new plist.
                Step::new(&["launchctl", "bootout", &target]).optional(),
                Step::new(&["launchctl", "bootstrap", &domain, &p]).or(&[
                    "launchctl",
                    "load",
                    "-w",
                    &p,
                ]),
            ]
        }
        Platform::Systemd => {
            let mut steps = vec![
                Step::new(&["systemctl", "--user", "daemon-reload"]),
                Step::new(&["systemctl", "--user", "enable", "--now", SYSTEMD_UNIT]),
            ];
            if reinstall {
                steps.push(Step::new(&["systemctl", "--user", "restart", SYSTEMD_UNIT]));
            }
            steps
        }
    }
}

/// Commands that stop and unload the service before its unit file is removed.
pub fn uninstall_steps(platform: Platform, path: &Path, uid: &str) -> Vec<Step> {
    let p = path.to_string_lossy();
    match platform {
        Platform::Launchd => vec![
            Step::new(&[
                "launchctl",
                "bootout",
                &format!("gui/{uid}/{LAUNCHD_LABEL}"),
            ])
            .or(&["launchctl", "unload", "-w", &p])
            .optional(),
        ],
        Platform::Systemd => {
            vec![Step::new(&["systemctl", "--user", "disable", "--now", SYSTEMD_UNIT]).optional()]
        }
    }
}

/// Commands to run after the unit file is removed.
fn after_uninstall_steps(platform: Platform) -> Vec<Step> {
    match platform {
        Platform::Launchd => Vec::new(),
        Platform::Systemd => vec![Step::new(&["systemctl", "--user", "daemon-reload"]).optional()],
    }
}

/// A command as it would be typed in a shell.
pub fn shell_line(cmd: &[String]) -> String {
    cmd.iter()
        .map(|a| {
            let plain = !a.is_empty()
                && a.chars()
                    .all(|c| c.is_ascii_alphanumeric() || "-_./=:@%+,".contains(c));
            if plain {
                a.clone()
            } else {
                format!("'{}'", a.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The settings in [`PASS_ENV`] that are set, as (name, value).
pub fn passed_env(get: impl Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    PASS_ENV
        .iter()
        .filter_map(|k| {
            get(k)
                .filter(|v| !v.trim().is_empty())
                .map(|v| (k.to_string(), v))
        })
        .collect()
}

/// The numeric user id, for launchd's `gui/<uid>` domain.
fn uid() -> Result<String> {
    let out = std::process::Command::new("id")
        .arg("-u")
        .output()
        .context("Cannot run `id -u` to find your user id")?;
    let uid = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if !out.status.success() || uid.is_empty() || !uid.chars().all(|c| c.is_ascii_digit()) {
        bail!("`id -u` did not print a user id");
    }
    Ok(uid)
}

pub(crate) fn locations(platform: Platform) -> Result<PathBuf> {
    let home = dirs::home_dir().context("Cannot find your home directory")?;
    let config = dirs::config_dir().unwrap_or_else(|| home.join(".config"));
    Ok(unit_path(platform, &home, &config))
}

/// Run one step; `Ok(false)` when an optional step failed.
fn run_step(step: &Step) -> Result<bool> {
    let run = |cmd: &[String]| -> Result<std::process::Output> {
        std::process::Command::new(&cmd[0])
            .args(&cmd[1..])
            .stdin(std::process::Stdio::null())
            .output()
            .with_context(|| format!("Cannot run `{}`", shell_line(cmd)))
    };
    let mut out = run(&step.cmd)?;
    let mut line = shell_line(&step.cmd);
    if !out.status.success()
        && let Some(fb) = &step.fallback
    {
        out = run(fb)?;
        line = shell_line(fb);
    }
    if out.status.success() {
        println!("  {} {}", "✓".green(), line.dimmed());
        return Ok(true);
    }
    if step.ignore_failure {
        return Ok(false);
    }
    let err = String::from_utf8_lossy(&out.stderr);
    bail!("`{line}` failed ({}): {}", out.status, err.trim())
}

fn print_steps(steps: &[Step]) {
    for s in steps {
        let mut line = shell_line(&s.cmd);
        if let Some(fb) = &s.fallback {
            line.push_str(&format!(" || {}", shell_line(fb)));
        }
        if s.ignore_failure && s.fallback.is_none() {
            line.push_str(" || true");
        }
        println!("  {line}");
    }
}

/// `worker install`: write the unit and load it.
pub fn cmd_worker_install(market: Market, dry_run: bool, interval_min: u64) -> Result<()> {
    let platform = Platform::current()?;
    if !(1..=1440).contains(&interval_min) {
        bail!("--interval must be between 1 and 1440 minutes");
    }
    let exe = std::env::current_exe().context("Cannot find the stockwise executable")?;
    // The data directory is created only for a real install.
    let data_dir = if dry_run {
        crate::store::data_dir_location()?
    } else {
        crate::store::data_dir()?
    };
    let spec = ServiceSpec {
        exe,
        args: vec![
            "-m".into(),
            market.code().into(),
            "worker".into(),
            "run".into(),
        ],
        env: passed_env(|k| std::env::var(k).ok()),
        log: data_dir.join(LOG_FILE),
        restart_secs: interval_min * 60,
    };
    let path = locations(platform)?;
    let text = unit_text(platform, &spec);
    let uid = match platform {
        Platform::Launchd => uid()?,
        Platform::Systemd => String::new(),
    };
    let reinstall = path.exists();
    let steps = install_steps(platform, &path, &uid, reinstall);

    if dry_run {
        print_header("Worker service (dry run: nothing written or run)");
        println!("  Would write {}:\n", path.display());
        for line in text.lines() {
            println!("    {line}");
        }
        println!("\n  Then run:");
        print_steps(&steps);
        println!();
        return Ok(());
    }

    if let Some(pid) = crate::worker::running_pid() {
        println!(
            "  {} A worker is already running (pid {pid}). The service's worker waits for it: \
             stop that one with `stockwise worker stop`.",
            "!".yellow()
        );
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("Cannot create {}", dir.display()))?;
    }
    std::fs::write(&path, &text).with_context(|| format!("Cannot write {}", path.display()))?;
    println!("\n  {} Wrote {}", "✓".green(), path.display());
    for step in &steps {
        run_step(step)?;
    }
    println!(
        "\n  {} The worker now starts at login and restarts {} after a failure.",
        "✓".green(),
        if interval_min == 1 {
            "a minute".to_string()
        } else {
            format!("{interval_min} minutes")
        }
    );
    match platform {
        Platform::Launchd => println!("  {}", format!("Log: {}", spec.log.display()).dimmed()),
        Platform::Systemd => {
            println!(
                "  {}",
                format!("Log: journalctl --user -u {SYSTEMD_UNIT}").dimmed()
            );
            println!(
                "  {}",
                "To keep it running while you are logged out: loginctl enable-linger".dimmed()
            );
        }
    }
    println!(
        "  {}\n",
        "`stockwise worker service-status` checks it; `stockwise worker uninstall` removes it."
            .dimmed()
    );
    Ok(())
}

/// `worker uninstall`: stop the service and remove its unit file.
pub fn cmd_worker_uninstall(dry_run: bool) -> Result<()> {
    let platform = Platform::current()?;
    let path = locations(platform)?;
    let uid = match platform {
        Platform::Launchd => uid()?,
        Platform::Systemd => String::new(),
    };
    let before = uninstall_steps(platform, &path, &uid);
    let after = after_uninstall_steps(platform);
    if dry_run {
        print_header("Worker service (dry run: nothing removed or run)");
        print_steps(&before);
        println!(
            "  rm {}",
            shell_line(&[path.to_string_lossy().into_owned()])
        );
        print_steps(&after);
        println!();
        return Ok(());
    }
    if !path.exists() {
        println!(
            "\n  {}\n",
            format!("No worker service is installed ({}).", path.display()).dimmed()
        );
        return Ok(());
    }
    println!();
    for step in &before {
        run_step(step)?;
    }
    std::fs::remove_file(&path).with_context(|| format!("Cannot remove {}", path.display()))?;
    println!("  {} Removed {}", "✓".green(), path.display());
    for step in &after {
        run_step(step)?;
    }
    println!();
    Ok(())
}

/// The `key = value` lines of `launchctl print` that matter.
pub fn launchctl_fields(print: &str) -> Vec<(String, String)> {
    print
        .lines()
        .filter_map(|l| l.trim().split_once(" = "))
        .filter(|(k, _)| matches!(*k, "state" | "pid" | "last exit code" | "runs"))
        .map(|(k, v)| (k.to_string(), v.trim().to_string()))
        .fold(Vec::new(), |mut acc, (k, v)| {
            // Nested sections repeat keys; the job's own come first.
            if !acc.iter().any(|(seen, _)| *seen == k) {
                acc.push((k, v));
            }
            acc
        })
}

/// `worker service-status`: is the service installed and loaded, and is its
/// worker running.
pub fn cmd_worker_service_status() -> Result<()> {
    let platform = Platform::current()?;
    let path = locations(platform)?;
    print_header("Worker service");
    print_kv(
        "Unit file",
        &if path.exists() {
            path.display().to_string()
        } else {
            format!("{} (not installed)", path.display())
                .dimmed()
                .to_string()
        },
    );
    let output = |cmd: &[&str]| {
        std::process::Command::new(cmd[0])
            .args(&cmd[1..])
            .stdin(std::process::Stdio::null())
            .output()
            .ok()
    };
    match platform {
        Platform::Launchd => {
            let target = format!("gui/{}/{LAUNCHD_LABEL}", uid()?);
            match output(&["launchctl", "print", &target]) {
                Some(o) if o.status.success() => {
                    print_kv("Loaded", &"yes".green().to_string());
                    for (k, v) in launchctl_fields(&String::from_utf8_lossy(&o.stdout)) {
                        print_kv(&k, &v);
                    }
                }
                _ => print_kv("Loaded", &"no".dimmed().to_string()),
            }
        }
        Platform::Systemd => {
            for (label, verb) in [("Enabled", "is-enabled"), ("Active", "is-active")] {
                let v = output(&["systemctl", "--user", verb, SYSTEMD_UNIT])
                    .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "unknown".into());
                print_kv(label, &v);
            }
        }
    }
    match crate::worker::running_pid() {
        Some(pid) => print_kv("Worker", &format!("{} (pid {pid})", "running".green())),
        None => print_kv("Worker", &"not running".dimmed().to_string()),
    }
    println!();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ServiceSpec {
        ServiceSpec {
            exe: PathBuf::from("/Users/a b/My Apps/stockwise"),
            args: vec!["-m".into(), "in".into(), "worker".into(), "run".into()],
            env: vec![
                ("STOCKWISE_HOME".into(), "/data/R&D <x>".into()),
                ("STOCKWISE_CONFIG".into(), "/c/100% \"sure\" $HOME".into()),
            ],
            log: PathBuf::from("/data/R&D <x>/worker.log"),
            restart_secs: 120,
        }
    }

    #[test]
    fn plist_escapes_xml_and_keeps_spaces_in_one_argument() {
        let p = launchd_plist(&spec());
        assert!(p.contains("<string>com.stockwise.worker</string>"));
        assert!(p.contains(
            "\t\t<string>/Users/a b/My Apps/stockwise</string>\n\t\t<string>-m</string>"
        ));
        assert!(p.contains("<string>worker</string>\n\t\t<string>run</string>"));
        assert!(
            p.contains("<key>STOCKWISE_HOME</key>\n\t\t<string>/data/R&amp;D &lt;x&gt;</string>")
        );
        assert!(p.contains("<string>/c/100% &quot;sure&quot; $HOME</string>"));
        assert!(p.contains(
            "<key>StandardOutPath</key>\n\t<string>/data/R&amp;D &lt;x&gt;/worker.log</string>"
        ));
        assert!(p.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(p.contains("<key>SuccessfulExit</key>\n\t\t<false/>"));
        assert!(p.contains("<integer>120</integer>"));
        assert!(!p.contains("R&D"), "unescaped ampersand");
    }

    #[test]
    fn plist_without_env_has_no_env_dict() {
        let mut s = spec();
        s.env.clear();
        assert!(!launchd_plist(&s).contains("EnvironmentVariables"));
    }

    #[test]
    fn systemd_quotes_paths_and_escapes_specials() {
        let u = systemd_unit(&spec());
        assert!(
            u.contains(
                "ExecStart=\"/Users/a b/My Apps/stockwise\" \"-m\" \"in\" \"worker\" \"run\"\n"
            ),
            "{u}"
        );
        assert!(u.contains("Environment=\"STOCKWISE_HOME=/data/R&D <x>\"\n"));
        // `%` is a specifier and `"` ends the string; `$` is literal outside ExecStart.
        assert!(u.contains("Environment=\"STOCKWISE_CONFIG=/c/100%% \\\"sure\\\" $HOME\"\n"));
        assert!(u.contains("Restart=on-failure\nRestartSec=120\n"));
        assert!(u.contains("[Install]\nWantedBy=default.target\n"));
    }

    #[test]
    fn systemd_quote_rules() {
        assert_eq!(systemd_quote("a b", true), "\"a b\"");
        assert_eq!(systemd_quote("$x", true), "\"$$x\"");
        assert_eq!(systemd_quote("$x", false), "\"$x\"");
        assert_eq!(systemd_quote("a\\b", false), "\"a\\\\b\"");
        assert_eq!(systemd_quote("50%", false), "\"50%%\"");
    }

    #[test]
    fn unit_paths_are_user_level() {
        let home = Path::new("/home/u");
        let cfg = Path::new("/home/u/.config");
        assert_eq!(
            unit_path(Platform::Launchd, home, cfg),
            PathBuf::from("/home/u/Library/LaunchAgents/com.stockwise.worker.plist")
        );
        assert_eq!(
            unit_path(Platform::Systemd, home, cfg),
            PathBuf::from("/home/u/.config/systemd/user/stockwise-worker.service")
        );
    }

    #[test]
    fn launchd_steps_bootstrap_with_a_load_fallback() {
        let path = Path::new("/Users/a b/Library/LaunchAgents/com.stockwise.worker.plist");
        let steps = install_steps(Platform::Launchd, path, "501", false);
        assert_eq!(steps.len(), 2);
        assert!(steps[0].ignore_failure);
        assert_eq!(
            shell_line(&steps[0].cmd),
            "launchctl bootout gui/501/com.stockwise.worker"
        );
        assert_eq!(
            shell_line(&steps[1].cmd),
            "launchctl bootstrap gui/501 '/Users/a b/Library/LaunchAgents/com.stockwise.worker.plist'"
        );
        assert_eq!(
            steps[1].fallback.as_ref().unwrap()[..3],
            ["launchctl", "load", "-w"]
        );
        let un = uninstall_steps(Platform::Launchd, path, "501");
        assert_eq!(un[0].cmd[1], "bootout");
        assert_eq!(un[0].fallback.as_ref().unwrap()[1], "unload");
    }

    #[test]
    fn systemd_steps_reload_enable_and_restart_on_reinstall() {
        let path = Path::new("/home/u/.config/systemd/user/stockwise-worker.service");
        let lines = |steps: Vec<Step>| steps.iter().map(|s| shell_line(&s.cmd)).collect::<Vec<_>>();
        assert_eq!(
            lines(install_steps(Platform::Systemd, path, "", false)),
            [
                "systemctl --user daemon-reload",
                "systemctl --user enable --now stockwise-worker.service"
            ]
        );
        assert_eq!(
            lines(install_steps(Platform::Systemd, path, "", true))
                .last()
                .unwrap(),
            "systemctl --user restart stockwise-worker.service"
        );
        assert_eq!(
            lines(uninstall_steps(Platform::Systemd, path, "")),
            ["systemctl --user disable --now stockwise-worker.service"]
        );
    }

    #[test]
    fn shell_line_quotes_only_when_needed() {
        let cmd: Vec<String> = ["echo", "plain-arg", "with space", "it's"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(shell_line(&cmd), "echo plain-arg 'with space' 'it'\\''s'");
    }

    #[test]
    fn only_the_listed_settings_pass_through() {
        let env = passed_env(|k| match k {
            "STOCKWISE_HOME" => Some("/data".into()),
            "STOCKWISE_CONFIG" => Some("  ".into()),
            "STOCKWISE_SMTP_PASSWORD" => Some("secret".into()),
            _ => None,
        });
        assert_eq!(env, [("STOCKWISE_HOME".to_string(), "/data".to_string())]);
    }

    #[test]
    fn launchctl_print_fields() {
        let print = "gui/501/com.stockwise.worker = {\n\tactive count = 1\n\tstate = running\n\
                     \truns = 3\n\tpid = 4242\n\tlast exit code = 0\n\tendpoints = {\n\t\tstate = idle\n\t}\n}";
        assert_eq!(
            launchctl_fields(print),
            [
                ("state".to_string(), "running".to_string()),
                ("runs".to_string(), "3".to_string()),
                ("pid".to_string(), "4242".to_string()),
                ("last exit code".to_string(), "0".to_string()),
            ]
        );
    }
}
