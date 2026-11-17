//! `stockwise update [--check]`: see [`crate::selfupdate`].

use super::*;
use crate::selfupdate::{self, ChannelHints, GitHub, Outcome};
use std::time::Duration;

pub async fn cmd_update(check: bool) -> Result<()> {
    let current = env!("CARGO_PKG_VERSION");
    let exe = std::env::current_exe().context("cannot find the running binary")?;
    // Resolve symlinks (Homebrew, pipx and others link into their own trees).
    let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
    let channel = selfupdate::detect_channel(&exe, &ChannelHints::current(&exe));

    print_header("StockWise update");
    print_kv("Installed", &format!("v{current}"));
    print_kv("Binary", &exe.display().to_string());
    print_kv("Install channel", channel.name());

    let github = GitHub::new(Duration::from_secs(20))?;
    let release = selfupdate::latest_release(&github)
        .await
        .context("cannot check for a new release")?;
    if let Ok(dir) = crate::store::data_dir() {
        selfupdate::write_cache(
            &dir,
            &selfupdate::CheckCache {
                checked_at: selfupdate::now_secs(),
                latest: release.tag_name.clone(),
            },
        );
    }
    let newer = selfupdate::is_newer(&release.tag_name, current);
    print_kv(
        "Latest release",
        &match newer {
            Some(true) => format!("{} {}", release.tag_name, "(newer)".yellow()),
            Some(false) => format!("{} {}", release.tag_name, "(up to date)".green()),
            None => release.tag_name.clone(),
        },
    );
    println!();

    if newer != Some(true) {
        println!("  {} StockWise is up to date.\n", "✓".green());
        return Ok(());
    }
    if !release.html_url.is_empty() {
        println!("  Release notes: {}\n", release.html_url);
    }

    if let Some(commands) = channel.upgrade_commands() {
        // Never replace a binary that a package manager owns.
        println!(
            "  This copy is managed by {}. Upgrade it with:\n",
            channel.name()
        );
        for c in commands {
            println!("    {c}");
        }
        println!();
        return Ok(());
    }

    if check {
        println!("  Run `stockwise update` to install it.\n");
        return Ok(());
    }

    let Some(target) = selfupdate::current_target() else {
        bail!(
            "there is no prebuilt release archive for this platform; \
             upgrade with `cargo install --locked stockwise`"
        );
    };
    let download = GitHub::new(Duration::from_secs(300))?;
    println!(
        "  Downloading {} and its SHA-256 checksum...",
        selfupdate::asset_name(&target)
    );
    match selfupdate::install_release(&download, &release, current, &target, &exe).await? {
        Outcome::Updated { from, to } => {
            println!("  {} Checksum verified.", "✓".green());
            println!(
                "  {} Updated v{from} -> {to} at {}\n",
                "✓".green(),
                exe.display()
            );
            if cfg!(windows) {
                println!(
                    "  The previous binary is at {}; delete it once the new one works.\n",
                    selfupdate::backup_path(&exe).display()
                );
            }
        }
        Outcome::UpToDate { .. } => println!("  {} StockWise is up to date.\n", "✓".green()),
    }
    Ok(())
}

/// The `doctor` line: whether a newer release exists, from the weekly cache
/// when it is fresh.
pub(crate) async fn doctor_version_line() -> String {
    let current = env!("CARGO_PKG_VERSION");
    let dir = crate::store::data_dir().ok();
    let github = match GitHub::new(Duration::from_secs(5)) {
        Ok(g) => g,
        Err(_) => return "could not check".dimmed().to_string(),
    };
    match selfupdate::latest_tag_cached(&github, dir.as_deref(), selfupdate::now_secs()).await {
        Ok((tag, cached)) => {
            let when = if cached { " (checked this week)" } else { "" };
            match selfupdate::is_newer(&tag, current) {
                Some(true) => format!(
                    "{} {} is available{} — run `stockwise update`",
                    "!".yellow(),
                    tag,
                    when.dimmed()
                ),
                _ => format!("{} v{current} is the latest{}", "✓".green(), when.dimmed()),
            }
        }
        Err(_) => format!(
            "v{current} {}",
            "(could not check for a newer release)".dimmed()
        ),
    }
}
