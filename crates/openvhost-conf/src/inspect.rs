// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only inspection of an installed web server: what version is it, and is
//! a given config file valid? Both shell out, so both are bounded by
//! `PROBE_TIMEOUT` and kill the child on expiry — these run behind a UI action,
//! where an unbounded wait is a hung spinner.
//!
//! Deliberately NOT `WebServerAdapter::validate`: that materializes generated
//! files into `ctx.home` first, and `validate::materialize`'s contract forbids
//! pointing it at a live home. `validate_live` validates a config file that
//! already exists, in place, writing nothing.

use std::path::Path;
use std::time::Duration;

use crate::ValidationReport;
use crate::error::ConfError;

/// Both probes are short-lived local process launches; 5s is far beyond a
/// healthy `nginx -v`/`-t` and short enough that a wedged binary surfaces as an
/// error instead of a spinner that never resolves.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// Version as the binary reports it (`1.27.3` from `nginx version: nginx/1.27.3`),
/// or `None` for any failure — missing binary, non-zero exit, unparseable banner,
/// timeout. Deliberately not a `Result`: a page that lists servers should still
/// list them when one version is unknowable, and the caller has nothing
/// actionable to do with the distinction.
pub async fn probe_version(bin: &Path) -> Option<String> {
    let mut cmd = tokio::process::Command::new(bin);
    // Without this, a wedged `-v` leaks its process forever: `Command::output`
    // is spawn + wait_with_output under the hood (same shape as
    // `validate_live`), and dropping that future on timeout is a no-op unless
    // kill_on_drop is set — see the doc comment above ("kill the child on
    // expiry" is a promise about THIS call too, not just `validate_live`).
    cmd.arg("-v").kill_on_drop(true);
    let run = cmd.output();
    // nginx writes its banner to STDERR, not stdout.
    let out = tokio::time::timeout(PROBE_TIMEOUT, run).await.ok()?.ok()?;
    let text = String::from_utf8_lossy(&out.stderr);
    parse_version(&text)
}

/// Pull `1.27.3` out of `nginx version: nginx/1.27.3 (extra build detail)`.
/// Split out so the parsing is testable without spawning anything.
fn parse_version(stderr: &str) -> Option<String> {
    let after_slash = stderr.split_once('/')?.1;
    let token = after_slash
        .split_whitespace()
        .next()?
        .trim_end_matches(|c: char| !c.is_ascii_digit() && c != '.');
    if token.is_empty() || !token.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    Some(token.to_string())
}

/// Validate a config file that ALREADY EXISTS, in place. Writes nothing to
/// `conf` and never calls `materialize`.
///
/// `-e <err_log>` is MANDATORY: without it nginx writes into its compiled-in
/// prefix (`/opt/homebrew/var`) rather than our home.
pub async fn validate_live(
    bin: &Path,
    conf: &Path,
    err_log: &Path,
) -> Result<ValidationReport, ConfError> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg("-e")
        .arg(err_log)
        .arg("-t")
        .arg("-c")
        .arg(conf)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    // Own process group: on timeout we must reclaim any grandchild the
    // validator spawned (e.g. a shell-wrapped binary), not just this one pid.
    // `kill_on_drop`/`Child::kill()` only ever signal the single tracked pid —
    // see openvhost-proc's UnixDriver, which hits this exact constraint for
    // managed services and solves it the same way.
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd.spawn().map_err(|e| ConfError::ValidatorSpawn {
        bin: bin.display().to_string(),
        source: e,
    })?;
    // Snapshotted BEFORE `wait_with_output` consumes `child` below.
    // `process_group(0)` makes this pid double as the pgid.
    #[cfg(unix)]
    let pgid = child.id();

    match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(res) => {
            let out = res.map_err(|e| ConfError::ValidatorSpawn {
                bin: bin.display().to_string(),
                source: e,
            })?;
            Ok(ValidationReport {
                // Exit code ONLY — nginx writes to stderr even on success.
                ok: out.status.success(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            })
        }
        Err(_) => {
            #[cfg(unix)]
            if let Some(pgid) = pgid {
                // SAFETY: plain syscall, no memory handed over. Negating the pid
                // signals the whole process GROUP (pgid == pid, from
                // `process_group(0)` above), reaching any grandchild the
                // timed-out validator spawned. The return value is intentionally
                // ignored: ESRCH here just means it already exited on its own,
                // which is the common case, not a failure worth surfacing.
                unsafe {
                    libc::kill(-(pgid as libc::pid_t), libc::SIGKILL);
                }
            }
            Err(ConfError::ValidatorTimeout {
                bin: bin.display().to_string(),
                secs: PROBE_TIMEOUT.as_secs(),
            })
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A fake "binary": a shell script that writes fixed text and exits with a
    /// fixed code. Lets these tests assert real spawn behaviour without needing
    /// nginx installed, which CI and most dev machines cannot assume.
    fn fake_bin(dir: &Path, name: &str, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[tokio::test]
    async fn version_is_read_from_stderr_not_stdout() {
        // nginx prints its banner to STDERR. A stdout-only reader returns None
        // against a real nginx, which is the bug this pins.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "echo 'nginx version: nginx/1.27.3' 1>&2");
        assert_eq!(probe_version(&bin).await.as_deref(), Some("1.27.3"));
    }

    #[tokio::test]
    async fn version_tolerates_a_banner_with_extra_build_detail() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(
            d.path(),
            "nginx",
            "echo 'nginx version: nginx/1.25.1 (Ubuntu)' 1>&2",
        );
        assert_eq!(probe_version(&bin).await.as_deref(), Some("1.25.1"));
    }

    #[tokio::test]
    async fn version_is_none_when_the_output_has_no_version() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "echo 'totally unrelated' 1>&2");
        assert_eq!(probe_version(&bin).await, None);
    }

    #[tokio::test]
    async fn version_is_none_when_the_binary_does_not_exist() {
        // A missing binary must not be an error that fails a whole page load.
        assert_eq!(probe_version(Path::new("/nonexistent/nginx")).await, None);
    }

    #[tokio::test]
    async fn validate_reports_ok_from_the_exit_code_alone() {
        // Success still writes to stderr (nginx says "syntax is ok" there), so
        // deriving `ok` from stderr emptiness would report every success as a
        // failure.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "echo 'syntax is ok' 1>&2; exit 0");
        let r = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap();
        assert!(r.ok);
        assert!(r.stderr.contains("syntax is ok"));
    }

    #[tokio::test]
    async fn validate_reports_failure_and_keeps_stderr_verbatim() {
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(
            d.path(),
            "nginx",
            "echo 'unknown directive \"bogus\"' 1>&2; exit 1",
        );
        let r = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap();
        assert!(!r.ok);
        assert!(r.stderr.contains("unknown directive"));
    }

    #[tokio::test]
    async fn validate_passes_the_mandatory_error_log_flag_and_the_config_path() {
        // Pins the argv shape. Without `-e`, nginx writes into the Homebrew
        // prefix; without `-c`, it validates its own compiled-in config instead
        // of ours. The fake echoes its args so the test can read them back.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", r#"echo "$@" 1>&2"#);
        let r = validate_live(&bin, Path::new("/tmp/live.conf"), Path::new("/tmp/err.log"))
            .await
            .unwrap();
        assert!(
            r.stderr.contains("-e /tmp/err.log"),
            "argv was: {}",
            r.stderr
        );
        assert!(r.stderr.contains("-t"), "argv was: {}", r.stderr);
        assert!(
            r.stderr.contains("-c /tmp/live.conf"),
            "argv was: {}",
            r.stderr
        );
    }

    #[tokio::test]
    async fn validate_errors_when_the_binary_cannot_be_launched() {
        let e = validate_live(
            Path::new("/nonexistent/nginx"),
            Path::new("/tmp/x.conf"),
            Path::new("/tmp/e.log"),
        )
        .await
        .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorSpawn { .. }));
    }

    #[tokio::test]
    async fn validate_times_out_instead_of_hanging_forever() {
        // The P0-7 validator uses a bare `.output().await` with no timeout. This
        // pins that the UI-facing path does not inherit that.
        let d = tempfile::tempdir().unwrap();
        let bin = fake_bin(d.path(), "nginx", "sleep 30");
        let e = validate_live(&bin, Path::new("/tmp/x.conf"), Path::new("/tmp/e.log"))
            .await
            .unwrap_err();
        assert!(matches!(e, ConfError::ValidatorTimeout { .. }), "got {e:?}");
    }

    #[test]
    fn parse_version_pulls_the_token_after_the_slash() {
        assert_eq!(
            parse_version("nginx version: nginx/1.27.3").as_deref(),
            Some("1.27.3")
        );
        assert_eq!(parse_version("nginx/1.2.3 (x)").as_deref(), Some("1.2.3"));
        assert_eq!(parse_version("no slash here"), None);
        assert_eq!(parse_version("trailing/"), None);
    }
}
