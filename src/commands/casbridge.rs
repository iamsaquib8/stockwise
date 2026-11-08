//! `import cas FILE.pdf`: CAMS / KFintech / MFCentral email a password
//! protected PDF. When `casparser` is on `PATH` it converts the PDF to JSON,
//! which then goes through the same importer as a hand-made `cas.json`.
//! `import cas -` reads casparser's JSON (or CSV) from standard input.

use super::*;
use std::io::{IsTerminal, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The command printed when `casparser` is missing: one line to copy.
pub(crate) fn install_hint(file: &str) -> String {
    format!(
        "casparser is not installed, so StockWise cannot read this PDF. Run: \
         pipx run casparser \"{file}\" -p <password> -o cas.json && stockwise import cas cas.json \
         (or `pip install casparser` first)"
    )
}

/// `name` on the `PATH` string `path_var`, if an executable file is there.
pub(crate) fn find_on_path(name: &str, path_var: &std::ffi::OsStr) -> Option<PathBuf> {
    std::env::split_paths(path_var)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

/// Whether the file is a PDF, by extension or by its first bytes.
pub(crate) fn is_pdf(path: &Path) -> bool {
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
    {
        return true;
    }
    let mut magic = [0u8; 4];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut magic))
        .is_ok()
        && &magic == b"%PDF"
}

/// The password: `--password`, else `STOCKWISE_CAS_PASSWORD`, else None.
pub(crate) fn password_from(flag: Option<&str>, env: Option<String>) -> Option<String> {
    flag.map(str::to_string)
        .or(env)
        .filter(|p| !p.trim().is_empty())
}

/// Run `bin` on `pdf` and return the JSON it wrote. The output file is a
/// fresh private directory that is removed afterwards.
pub(crate) fn run_casparser(bin: &Path, pdf: &Path, password: &str) -> Result<String> {
    let dir = std::env::temp_dir().join(format!(
        "stockwise-cas-{}-{}",
        std::process::id(),
        chrono::Local::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let out = dir.join("cas.json");
    let result = Command::new(bin)
        .arg(pdf)
        .arg("-p")
        .arg(password)
        .arg("-o")
        .arg(&out)
        .output()
        .with_context(|| format!("Could not run {}", bin.display()));
    let json = match result {
        Err(e) => Err(e),
        Ok(o) if !o.status.success() => {
            let msg = String::from_utf8_lossy(&o.stderr);
            let msg = msg.trim();
            Err(anyhow::anyhow!(
                "casparser failed ({}): {}. Check the password (usually your PAN in capitals, or the one you set when requesting the statement).",
                o.status,
                if msg.is_empty() { "no error text" } else { msg }
            ))
        }
        Ok(_) => std::fs::read_to_string(&out)
            .context("casparser finished but wrote no JSON; try `casparser --help`"),
    };
    let _ = std::fs::remove_dir_all(&dir);
    json
}

/// `stockwise import cas FILE`: a PDF goes through casparser, `-` reads
/// standard input, anything else takes the usual file path.
pub async fn cmd_import_cas(
    file: &str,
    password: Option<&str>,
    replace: bool,
    sheet: Option<&str>,
    market: Market,
) -> Result<()> {
    let path = Path::new(file);
    if file == "-" {
        let mut raw = Vec::new();
        std::io::stdin()
            .read_to_end(&mut raw)
            .context("Could not read standard input")?;
        if raw.starts_with(b"%PDF") {
            bail!("{}", install_hint("CAS.pdf"));
        }
        let content = String::from_utf8(raw).context("Standard input is not UTF-8 text")?;
        return super::imports::import_rows(
            "cas",
            "stdin",
            replace,
            super::imports::parse_cas(&content)?,
        )
        .await;
    }
    if !path.exists() || !is_pdf(path) {
        return cmd_import("cas", file, replace, sheet, market).await;
    }
    let Some(path_var) = std::env::var_os("PATH") else {
        bail!("{}", install_hint(file));
    };
    let Some(bin) = find_on_path("casparser", &path_var) else {
        bail!("{}", install_hint(file));
    };
    let password = match password_from(password, std::env::var("STOCKWISE_CAS_PASSWORD").ok()) {
        Some(p) => p,
        None if std::io::stdin().is_terminal() => {
            rpassword::prompt_password("CAS PDF password (hidden): ")?
        }
        None => {
            bail!("This PDF is password protected. Pass --password, or set STOCKWISE_CAS_PASSWORD.")
        }
    };
    println!("  {} Converting {} with casparser…", "·".dimmed(), file);
    let json = run_casparser(&bin, path, &password)?;
    super::imports::import_rows("cas", file, replace, super::imports::parse_cas(&json)?).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_is_one_line_and_names_the_file() {
        let h = install_hint("my cas.pdf");
        assert!(!h.contains('\n'));
        assert!(h.contains("pipx run casparser \"my cas.pdf\""));
        assert!(h.contains("stockwise import cas cas.json"));
    }

    #[test]
    fn password_prefers_flag_over_env() {
        assert_eq!(
            password_from(Some("A"), Some("B".into())).as_deref(),
            Some("A")
        );
        assert_eq!(password_from(None, Some("B".into())).as_deref(), Some("B"));
        assert_eq!(password_from(None, Some("  ".into())), None);
        assert_eq!(password_from(None, None), None);
    }

    #[test]
    fn detects_pdf_by_extension_and_magic() {
        let dir = std::env::temp_dir().join(format!("sw-pdf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let by_ext = dir.join("a.PDF");
        let by_magic = dir.join("noext");
        let csv = dir.join("b.csv");
        std::fs::write(&by_ext, b"x").unwrap();
        std::fs::write(&by_magic, b"%PDF-1.7").unwrap();
        std::fs::write(&csv, b"a,b\n").unwrap();
        assert!(is_pdf(&by_ext) && is_pdf(&by_magic) && !is_pdf(&csv));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_binary_on_path_or_not() {
        let dir = std::env::temp_dir().join(format!("sw-path-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("casparser"), b"").unwrap();
        let path = std::env::join_paths([dir.clone()]).unwrap();
        assert!(find_on_path("casparser", &path).is_some());
        assert!(find_on_path("nothere", &path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stand-in `casparser` that checks its arguments and writes the
    /// fixture JSON to the `-o` path.
    #[cfg(unix)]
    #[test]
    fn bridge_runs_casparser_and_output_parses() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("sw-fake-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let fixture = dir.join("fixture.json");
        std::fs::write(&fixture, include_str!("fixtures/cas_cams.json")).unwrap();
        let bin = dir.join("casparser");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\n[ \"$2\" = \"-p\" ] && [ \"$3\" = \"s3cret\" ] && [ \"$4\" = \"-o\" ] || {{ echo bad args >&2; exit 2; }}\ncp '{}' \"$5\"\n",
                fixture.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let json = run_casparser(&bin, Path::new("x.pdf"), "s3cret").unwrap();
        assert!(super::super::imports::parse_cas(&json).is_ok());
        let err = run_casparser(&bin, Path::new("x.pdf"), "wrong").unwrap_err();
        assert!(err.to_string().contains("casparser failed"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
