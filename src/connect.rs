//! `stockwise connect`: add StockWise's MCP server (and, where the client
//! has them, its agent skill) to AI apps and coding agents in one step.
//!
//! Each client is a config file and a format. The file is read, the
//! `stockwise` entry added or replaced (nothing else changes), a backup
//! kept next to it (`<file>.stockwise.bak`), and the file written back.
//! A file that cannot be parsed (comments in JSON, say) is left alone and
//! the snippet is printed to paste by hand.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

/// The skill installed into clients that read `SKILL.md` folders.
pub const SKILL: &str = include_str!("../skills/stockwise/SKILL.md");

/// Every client id, in the order `clients` lists them.
pub const IDS: &[&str] = &[
    "claude-desktop",
    "claude-code",
    "cursor",
    "windsurf",
    "vscode",
    "cline",
    "gemini-cli",
    "antigravity",
    "codex",
    "zed",
    "lm-studio",
    "opencode",
    "goose",
    "continue",
];

/// How a client stores its MCP servers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `{"mcpServers": {"stockwise": {"command", "args"}}}` (Claude Desktop,
    /// Cursor, Windsurf, Gemini CLI, Antigravity, LM Studio, Cline).
    McpServers,
    /// VS Code: `{"servers": {"stockwise": {"type": "stdio", ...}}}`.
    VsCode,
    /// Zed: `{"context_servers": {"stockwise": {"command", "args"}}}`.
    Zed,
    /// opencode: `{"mcp": {"stockwise": {"type": "local", "command": [..]}}}`.
    OpenCode,
    /// Codex: `[mcp_servers.stockwise]` in TOML.
    CodexToml,
    /// Claude Code: `claude mcp add` (its own CLI keeps the config).
    ClaudeCli,
    /// No file format StockWise edits: print the snippet.
    Manual,
}

/// One client.
#[derive(Debug, Clone)]
pub struct Client {
    pub id: &'static str,
    pub name: &'static str,
    pub format: Format,
    /// Candidate config files, first existing one wins (the first is
    /// created when none exists and the client is named).
    pub paths: Vec<PathBuf>,
    /// Folder whose presence means the client is installed.
    pub marker: Option<PathBuf>,
    /// Where its skills live (`<dir>/stockwise/SKILL.md`), if it has them.
    pub skills: Option<PathBuf>,
    /// Shown for `Manual` clients.
    pub hint: &'static str,
}

/// Every client, for a home directory and OS-specific app-data folders.
pub fn clients(home: &Path) -> Vec<Client> {
    let mac = cfg!(target_os = "macos");
    let win = cfg!(windows);
    let app_support = home.join("Library").join("Application Support");
    let appdata = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("AppData").join("Roaming"));
    let xdg = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    // Per-user app folder: macOS Application Support, Windows %APPDATA%,
    // Linux ~/.config.
    let app = |name: &str| {
        if mac {
            app_support.join(name)
        } else if win {
            appdata.join(name)
        } else {
            xdg.join(name)
        }
    };
    let code_user = app("Code").join("User");
    vec![
        Client {
            id: "claude-desktop",
            name: "Claude Desktop",
            format: Format::McpServers,
            paths: vec![app("Claude").join("claude_desktop_config.json")],
            marker: Some(app("Claude")),
            skills: None,
            hint: "",
        },
        Client {
            id: "claude-code",
            name: "Claude Code",
            format: Format::ClaudeCli,
            paths: vec![],
            marker: Some(home.join(".claude")),
            skills: Some(home.join(".claude").join("skills")),
            hint: "",
        },
        Client {
            id: "cursor",
            name: "Cursor",
            format: Format::McpServers,
            paths: vec![home.join(".cursor").join("mcp.json")],
            marker: Some(home.join(".cursor")),
            skills: None,
            hint: "",
        },
        Client {
            id: "windsurf",
            name: "Windsurf",
            format: Format::McpServers,
            paths: vec![
                home.join(".codeium")
                    .join("windsurf")
                    .join("mcp_config.json"),
            ],
            marker: Some(home.join(".codeium").join("windsurf")),
            skills: None,
            hint: "",
        },
        Client {
            id: "vscode",
            name: "VS Code (Copilot agent mode)",
            format: Format::VsCode,
            paths: vec![code_user.join("mcp.json")],
            marker: Some(code_user.clone()),
            skills: None,
            hint: "",
        },
        Client {
            id: "cline",
            name: "Cline (VS Code)",
            format: Format::McpServers,
            paths: vec![
                code_user
                    .join("globalStorage")
                    .join("saoudrizwan.claude-dev")
                    .join("settings")
                    .join("cline_mcp_settings.json"),
            ],
            marker: Some(
                code_user
                    .join("globalStorage")
                    .join("saoudrizwan.claude-dev"),
            ),
            skills: None,
            hint: "",
        },
        Client {
            id: "gemini-cli",
            name: "Gemini CLI",
            format: Format::McpServers,
            paths: vec![home.join(".gemini").join("settings.json")],
            marker: Some(home.join(".gemini")),
            skills: None,
            hint: "",
        },
        Client {
            id: "antigravity",
            name: "Google Antigravity",
            format: Format::McpServers,
            // Releases have used both locations.
            paths: vec![
                home.join(".gemini")
                    .join("antigravity")
                    .join("mcp_config.json"),
                home.join(".gemini").join("config").join("mcp_config.json"),
            ],
            marker: Some(home.join(".gemini").join("antigravity")),
            skills: Some(home.join(".gemini").join("antigravity").join("skills")),
            hint: "",
        },
        Client {
            id: "codex",
            name: "OpenAI Codex CLI",
            format: Format::CodexToml,
            paths: vec![
                std::env::var_os("CODEX_HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".codex"))
                    .join("config.toml"),
            ],
            marker: Some(home.join(".codex")),
            skills: None,
            hint: "",
        },
        Client {
            id: "zed",
            name: "Zed",
            format: Format::Zed,
            paths: vec![if win {
                appdata.join("Zed").join("settings.json")
            } else {
                xdg.join("zed").join("settings.json")
            }],
            marker: Some(if win {
                appdata.join("Zed")
            } else {
                xdg.join("zed")
            }),
            skills: None,
            hint: "",
        },
        Client {
            id: "lm-studio",
            name: "LM Studio",
            format: Format::McpServers,
            paths: vec![home.join(".lmstudio").join("mcp.json")],
            marker: Some(home.join(".lmstudio")),
            skills: None,
            hint: "",
        },
        Client {
            id: "opencode",
            name: "opencode",
            format: Format::OpenCode,
            paths: vec![xdg.join("opencode").join("opencode.json")],
            marker: Some(xdg.join("opencode")),
            skills: None,
            hint: "",
        },
        Client {
            id: "goose",
            name: "Goose",
            format: Format::Manual,
            paths: vec![],
            marker: None,
            skills: None,
            hint: "run `goose configure`, Add Extension, Command-line Extension, with the command",
        },
        Client {
            id: "continue",
            name: "Continue",
            format: Format::Manual,
            paths: vec![],
            marker: None,
            skills: None,
            hint: "add an entry named stockwise under mcpServers in ~/.continue/config.yaml, with the command",
        },
    ]
}

/// The command a client runs.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    pub command: String,
    pub args: Vec<String>,
}

impl Launch {
    /// This executable, `mcp`, and the access flags.
    pub fn this(writes: bool, portfolio: bool) -> Result<Self> {
        let exe = std::env::current_exe().context("Cannot find the stockwise executable")?;
        let exe = exe.canonicalize().unwrap_or(exe);
        let mut args = vec!["mcp".to_string()];
        if portfolio {
            args.push("--allow-portfolio".into());
        } else if writes {
            args.push("--allow-writes".into());
        }
        Ok(Self {
            command: exe.to_string_lossy().into_owned(),
            args,
        })
    }

    pub fn line(&self) -> String {
        let quote = |s: &str| {
            if s.contains(' ') {
                format!("\"{}\"", s)
            } else {
                s.to_string()
            }
        };
        std::iter::once(quote(&self.command))
            .chain(self.args.iter().map(|a| quote(a)))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// The JSON entry for `format`.
pub fn entry(format: Format, l: &Launch) -> Value {
    match format {
        Format::VsCode => json!({"type": "stdio", "command": l.command, "args": l.args}),
        Format::OpenCode => {
            let mut cmd = vec![l.command.clone()];
            cmd.extend(l.args.iter().cloned());
            json!({"type": "local", "command": cmd, "enabled": true})
        }
        _ => json!({"command": l.command, "args": l.args}),
    }
}

/// The key the servers live under.
fn servers_key(format: Format) -> &'static str {
    match format {
        Format::VsCode => "servers",
        Format::Zed => "context_servers",
        Format::OpenCode => "mcp",
        _ => "mcpServers",
    }
}

/// `text` (a JSON config, or empty) with the `stockwise` entry set (or
/// removed with `remove`). Errors when the text is not a JSON object.
pub fn merge_json(text: &str, format: Format, l: &Launch, remove: bool) -> Result<String> {
    let mut root: Value = if text.trim().is_empty() {
        json!({})
    } else {
        serde_json::from_str(text)
            .context("not plain JSON (comments or trailing commas?); paste the snippet by hand")?
    };
    let obj = root
        .as_object_mut()
        .context("the file is not a JSON object")?;
    let key = servers_key(format);
    let servers = obj
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let servers = servers
        .as_object_mut()
        .with_context(|| format!("`{}` is not an object", key))?;
    if remove {
        servers.remove("stockwise");
    } else {
        servers.insert("stockwise".into(), entry(format, l));
    }
    Ok(serde_json::to_string_pretty(&root)? + "\n")
}

/// `text` (Codex's config.toml) with the `[mcp_servers.stockwise]` table
/// set (or removed). Other tables and comments are kept as they are.
pub fn merge_toml(text: &str, l: &Launch, remove: bool) -> String {
    let header = "[mcp_servers.stockwise]";
    let mut out = String::new();
    let mut skipping = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            skipping = t == header || t.starts_with("[mcp_servers.stockwise.");
        }
        if !skipping {
            out.push_str(line);
            out.push('\n');
        }
    }
    if !remove {
        let esc = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push('\n');
        }
        out.push_str(&format!(
            "{}\ncommand = \"{}\"\nargs = [{}]\n",
            header,
            esc(&l.command),
            l.args
                .iter()
                .map(|a| format!("\"{}\"", esc(a)))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    out
}

/// What happened for one client.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct Outcome {
    pub client: String,
    pub status: &'static str,
    pub detail: String,
}

/// Connect (or disconnect) one client.
pub fn apply(c: &Client, l: &Launch, remove: bool, skill: bool, dry_run: bool) -> Outcome {
    let done = |status, detail: String| Outcome {
        client: c.id.to_string(),
        status,
        detail,
    };
    let result = (|| -> Result<(String, Option<String>)> {
        let mut detail = match c.format {
            Format::Manual => bail!("{}: {}", c.hint, l.line()),
            Format::ClaudeCli => {
                let mut cmd = std::process::Command::new("claude");
                if remove {
                    cmd.args(["mcp", "remove", "--scope", "user", "stockwise"]);
                } else {
                    cmd.args(["mcp", "add", "--scope", "user", "stockwise", "--"])
                        .arg(&l.command)
                        .args(&l.args);
                }
                if dry_run {
                    format!(
                        "would run: claude mcp {} ...",
                        if remove { "remove" } else { "add" }
                    )
                } else {
                    // Replace an older entry first; a missing one is fine.
                    if !remove {
                        let _ = std::process::Command::new("claude")
                            .args(["mcp", "remove", "--scope", "user", "stockwise"])
                            .output();
                    }
                    let out = cmd.output().with_context(|| {
                        format!(
                            "`claude` is not on PATH; run: claude mcp add --scope user stockwise -- {}",
                            l.line()
                        )
                    })?;
                    if !out.status.success() {
                        bail!(
                            "claude mcp failed: {}",
                            String::from_utf8_lossy(&out.stderr).trim()
                        );
                    }
                    "via `claude mcp` (user scope)".to_string()
                }
            }
            _ => {
                let path = c
                    .paths
                    .iter()
                    .find(|p| p.exists())
                    .or(c.paths.first())
                    .context("no config path")?
                    .clone();
                // Write through a symlink (Antigravity links its config
                // into ~/.gemini/config), not over it.
                let path = std::fs::canonicalize(&path).unwrap_or(path);
                let old = std::fs::read_to_string(&path).unwrap_or_default();
                let new = match c.format {
                    Format::CodexToml => merge_toml(&old, l, remove),
                    f => merge_json(&old, f, l, remove)
                        .with_context(|| format!("{}", path.display()))?,
                };
                if !dry_run && new != old {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    if path.exists() {
                        let bak = path.with_extension(format!(
                            "{}.stockwise.bak",
                            path.extension().and_then(|e| e.to_str()).unwrap_or("cfg")
                        ));
                        std::fs::copy(&path, &bak)
                            .with_context(|| format!("Cannot back up {}", path.display()))?;
                    }
                    std::fs::write(&path, &new)
                        .with_context(|| format!("Cannot write {}", path.display()))?;
                }
                format!(
                    "{}{}",
                    path.display(),
                    if dry_run { " (dry run)" } else { "" }
                )
            }
        };
        let mut skill_note = None;
        if skill && let Some(dir) = &c.skills {
            let file = dir.join("stockwise").join("SKILL.md");
            if !dry_run {
                if remove {
                    let _ = std::fs::remove_dir_all(dir.join("stockwise"));
                } else {
                    std::fs::create_dir_all(dir.join("stockwise"))?;
                    std::fs::write(&file, SKILL)
                        .with_context(|| format!("Cannot write {}", file.display()))?;
                }
            }
            skill_note = Some(file.display().to_string());
        }
        if let Some(s) = &skill_note {
            detail.push_str(&format!("; skill {}", s));
        }
        Ok((detail, skill_note))
    })();
    match result {
        Ok((detail, _)) => done(if remove { "removed" } else { "connected" }, detail),
        Err(e) if c.format == Format::Manual => done("manual", format!("{:#}", e)),
        Err(e) => done("failed", format!("{:#}", e)),
    }
}

/// Whether a client looks installed on this machine.
pub fn detected(c: &Client) -> bool {
    match c.format {
        Format::Manual => false,
        Format::ClaudeCli => {
            c.marker.as_ref().is_some_and(|m| m.exists())
                || std::process::Command::new("claude")
                    .arg("--version")
                    .output()
                    .is_ok()
        }
        _ => c.paths.iter().any(|p| p.exists()) || c.marker.as_ref().is_some_and(|m| m.exists()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launch() -> Launch {
        Launch {
            command: "/usr/local/bin/stockwise".into(),
            args: vec!["mcp".into(), "--allow-writes".into()],
        }
    }

    #[test]
    fn ids_match_the_client_list() {
        let ids: Vec<_> = clients(Path::new("/h")).iter().map(|c| c.id).collect();
        assert_eq!(ids, IDS);
    }

    #[test]
    fn json_configs_keep_other_servers_and_settings() {
        let old = r#"{"theme": "dark", "mcpServers": {"github": {"command": "gh"}}}"#;
        let new = merge_json(old, Format::McpServers, &launch(), false).unwrap();
        let v: Value = serde_json::from_str(&new).unwrap();
        assert_eq!(v["theme"], "dark");
        assert_eq!(v["mcpServers"]["github"]["command"], "gh");
        assert_eq!(
            v["mcpServers"]["stockwise"]["args"],
            json!(["mcp", "--allow-writes"])
        );
        // Again: the same file (idempotent), then removed.
        assert_eq!(
            merge_json(&new, Format::McpServers, &launch(), false).unwrap(),
            new
        );
        let gone = merge_json(&new, Format::McpServers, &launch(), true).unwrap();
        assert!(serde_json::from_str::<Value>(&gone).unwrap()["mcpServers"]["stockwise"].is_null());
        // Each client's key and shape.
        let vs: Value =
            serde_json::from_str(&merge_json("", Format::VsCode, &launch(), false).unwrap())
                .unwrap();
        assert_eq!(vs["servers"]["stockwise"]["type"], "stdio");
        let oc: Value =
            serde_json::from_str(&merge_json("", Format::OpenCode, &launch(), false).unwrap())
                .unwrap();
        assert_eq!(oc["mcp"]["stockwise"]["command"][1], "mcp");
        let zed: Value =
            serde_json::from_str(&merge_json("{}", Format::Zed, &launch(), false).unwrap())
                .unwrap();
        assert!(zed["context_servers"]["stockwise"].is_object());
        // JSON with comments is left alone.
        assert!(merge_json("{ // hi\n }", Format::Zed, &launch(), false).is_err());
    }

    #[test]
    fn codex_toml_replaces_only_the_stockwise_table() {
        let old = "model = \"o4\"\n\n[mcp_servers.github]\ncommand = \"gh\"\n\n[mcp_servers.stockwise]\ncommand = \"old\"\nargs = [\"mcp\"]\n\n[profiles.x]\na = 1\n";
        let new = merge_toml(old, &launch(), false);
        assert!(new.contains("[mcp_servers.github]\ncommand = \"gh\""));
        assert!(new.contains("[profiles.x]\na = 1"));
        assert_eq!(new.matches("[mcp_servers.stockwise]").count(), 1);
        assert!(new.contains("args = [\"mcp\", \"--allow-writes\"]"));
        assert!(!new.contains("command = \"old\""));
        let parsed: toml::Table = toml::from_str(&new).unwrap();
        assert_eq!(
            parsed["mcp_servers"]["stockwise"]["command"].as_str(),
            Some("/usr/local/bin/stockwise")
        );
        assert_eq!(merge_toml(&new, &launch(), false), new, "idempotent");
        assert!(!merge_toml(&new, &launch(), true).contains("stockwise"));
    }

    #[test]
    fn connecting_writes_a_backup_and_the_skill() {
        let home = std::env::temp_dir().join(format!(
            "stockwise-connect-{}-{}",
            std::process::id(),
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let cs = clients(&home);
        let ag = cs.iter().find(|c| c.id == "antigravity").unwrap();
        assert!(!detected(ag));
        std::fs::create_dir_all(home.join(".gemini").join("antigravity")).unwrap();
        assert!(detected(ag));
        let o = apply(ag, &launch(), false, true, false);
        assert_eq!(o.status, "connected", "{o:?}");
        let cfg = home.join(".gemini/antigravity/mcp_config.json");
        let v: Value = serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(
            v["mcpServers"]["stockwise"]["command"],
            "/usr/local/bin/stockwise"
        );
        let skill = home.join(".gemini/antigravity/skills/stockwise/SKILL.md");
        assert!(
            std::fs::read_to_string(&skill)
                .unwrap()
                .contains("name: stockwise")
        );
        // A second run backs up the file it changes.
        let l2 = Launch {
            args: vec!["mcp".into()],
            ..launch()
        };
        apply(ag, &l2, false, true, false);
        assert!(
            home.join(".gemini/antigravity/mcp_config.json.stockwise.bak")
                .exists()
        );
        // Remove takes the entry and the skill out.
        let o = apply(ag, &l2, true, true, false);
        assert_eq!(o.status, "removed");
        assert!(!skill.exists());
        // A symlinked config is written through, and stays a link.
        #[cfg(unix)]
        {
            let real = home.join(".gemini/config/mcp_config.json");
            std::fs::create_dir_all(real.parent().unwrap()).unwrap();
            std::fs::write(&real, "{\"mcpServers\": {}}").unwrap();
            std::fs::remove_file(&cfg).unwrap();
            std::os::unix::fs::symlink(&real, &cfg).unwrap();
            assert_eq!(apply(ag, &l2, false, false, false).status, "connected");
            assert!(
                std::fs::symlink_metadata(&cfg)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
            let v: Value = serde_json::from_str(&std::fs::read_to_string(&real).unwrap()).unwrap();
            assert!(v["mcpServers"]["stockwise"].is_object());
        }
        // Manual clients print what to do.
        let goose = cs.iter().find(|c| c.id == "goose").unwrap();
        let o = apply(goose, &launch(), false, false, false);
        assert_eq!(o.status, "manual");
        assert!(o.detail.contains("--allow-writes"));
        std::fs::remove_dir_all(&home).ok();
    }
}
