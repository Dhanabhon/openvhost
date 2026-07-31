// SPDX-License-Identifier: GPL-3.0-or-later
//! The user's **login shell**: what PATH it really produces, whether our
//! directory is in it, and what to tell them if it is not (D4).
//!
//! ## Why we ask the shell instead of reading `PATH`
//!
//! A GUI app launched from Finder inherits launchd's minimal environment, not
//! the shell's. `std::env::var("PATH")` would therefore make the app
//! confidently wrong about the one thing this feature exists to report. So we
//! run the login shell once and read the PATH it builds — the same PATH a
//! fresh Terminal window gets.
//!
//! ## Bounded, contained, and never interpreted
//!
//! The command is `$SHELL -l -c 'printf %s "$PATH"'`, a **constant** script
//! with nothing interpolated into it, run as a one-shot
//! `tokio::process::Command` — the practice already established for
//! `nginx -t` and `php -i` (golden rule 4 governs *supervised* processes;
//! this is a query). It is bounded at [`PROBE_TIMEOUT`] because a slow or
//! interactive-hostile profile is a well-known hang in tools that do this,
//! and hanging a menu action is unacceptable. Containment mirrors
//! `openvhost_conf::inspect::run_bounded`: `stdin` is `/dev/null` so no
//! profile can block reading a terminal, the child gets its **own process
//! group** so the timeout arm can reclaim any grandchild the profile forked,
//! and `kill_on_drop` is a secondary net for a dropped future.
//!
//! `openvhost_conf::inspect::run_bounded` is deliberately *not* reused: its
//! timeout is 5 s and not configurable, and D4 pins 2 s.
//!
//! ## The environment is inherited, on purpose
//!
//! `run_bounded` clears the environment down to four variables. Doing that
//! here would change the very thing being measured: `ZDOTDIR` decides which
//! files zsh reads, and dropping it would make the probe consult profiles the
//! user's real shell never reads. Inheriting is the faithful simulation of
//! "what does a fresh Terminal get". The environment in question is this
//! process's own, established by launchd from the user's login session — this
//! feature accepts no input that can reach it.

use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use super::{PathStatus, PathStatusProbe};

/// D4's bound. Two seconds is long enough for any sane profile (the measured
/// cost on the target machine is ~30 ms) and short enough that a hostile one
/// does not feel like a hang.
pub(super) const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// The script handed to the login shell. A **constant**: nothing is ever
/// interpolated into it, and the shell's answer is never executed or fed back
/// into a command.
const PATH_SCRIPT: &str = "printf %s \"$PATH\"";

/// Above this, the answer is not a PATH. A real one is a few hundred bytes;
/// macOS caps the whole environment at 1 MiB. Refusing rather than truncating
/// keeps us from reporting `NotOnPath` off a half-read string.
const MAX_PATH_BYTES: usize = 128 * 1024;

/// What [`super::detect`] puts in a [`PathStatus::Unknown`] it produced
/// without probing at all.
const NOT_PROBED: &str =
    "the login shell's PATH was not checked here; that check runs when you install";

/// Ask the user's login shell what its PATH is (D4).
///
/// Never panics, never returns a `bool`: a failure is
/// [`PathStatusProbe::Failed`] with a reason the dialog can show.
pub async fn login_shell_path() -> PathStatusProbe {
    match std::env::var_os("SHELL").filter(|s| !s.is_empty()) {
        Some(shell) => probe_shell_path(Path::new(&shell)).await,
        None => PathStatusProbe::Failed {
            reason: "the SHELL environment variable is not set".to_string(),
        },
    }
}

/// [`login_shell_path`] against an explicit shell, so tests can point it at a
/// fake one.
pub(super) async fn probe_shell_path(shell: &Path) -> PathStatusProbe {
    // Absolute only. A relative `$SHELL` would be resolved against *our*
    // PATH — the minimal launchd one — which is both unpredictable and the
    // one lookup this whole module exists to avoid trusting.
    if !shell.is_absolute() {
        return PathStatusProbe::Failed {
            reason: format!("SHELL ({}) is not an absolute path", shell.display()),
        };
    }
    let mut cmd = tokio::process::Command::new(shell);
    cmd.arg("-l")
        .arg("-c")
        .arg(PATH_SCRIPT)
        // A profile that reads stdin sees EOF immediately instead of blocking
        // on a terminal we inherited.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Secondary net for "the caller dropped this future". Not sufficient
        // alone: like `Child::kill`, it signals only the tracked pid, so a
        // profile's forked grandchild survives it. The timeout arm below does
        // the real reclaiming.
        .kill_on_drop(true);
    // Own process group, set atomically at spawn, so the timeout arm can
    // reach grandchildren too.
    cmd.process_group(0);

    let child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return PathStatusProbe::Failed {
                reason: format!("could not run {}: {e}", shell.display()),
            };
        }
    };
    // Snapshotted BEFORE `wait_with_output` consumes `child`.
    // `process_group(0)` makes this pid double as the pgid.
    let pgid = child.id();

    // DROP ORDERING IS LOAD-BEARING, exactly as in
    // `openvhost_conf::inspect::run_bounded`: `child` lives inside the
    // `Timeout` temporary in this `match`'s scrutinee, and a scrutinee
    // temporary lives to the END of the `match`. So in the timeout arm the
    // group LEADER IS STILL ALIVE and `kill_on_drop` has not fired, which is
    // what makes `-pgid` provably still our own group with no pid-reuse
    // window. Hoisting this into `let res = timeout(..).await;` breaks that.
    match tokio::time::timeout(PROBE_TIMEOUT, child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() => {
            if out.stdout.len() > MAX_PATH_BYTES {
                return PathStatusProbe::Failed {
                    reason: format!(
                        "{} printed {} bytes, which is not a PATH",
                        shell.display(),
                        out.stdout.len()
                    ),
                };
            }
            PathStatusProbe::Resolved {
                path: String::from_utf8_lossy(&out.stdout).into_owned(),
            }
        }
        Ok(Ok(out)) => PathStatusProbe::Failed {
            reason: format!(
                "{} -l exited with {}{}",
                shell.display(),
                out.status,
                stderr_tail(&out.stderr)
            ),
        },
        Ok(Err(e)) => PathStatusProbe::Failed {
            reason: format!("could not read the output of {}: {e}", shell.display()),
        },
        Err(_) => {
            kill_process_group(pgid);
            PathStatusProbe::Failed {
                reason: format!(
                    "{} did not answer within {} seconds",
                    shell.display(),
                    PROBE_TIMEOUT.as_secs()
                ),
            }
        }
    }
}

/// At most a couple of lines of a failed shell's complaint, for the reason
/// string. Truncated because a broken profile can be very chatty and this
/// ends up in a modal dialog.
fn stderr_tail(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    let cut: String = trimmed.chars().take(200).collect();
    format!(": {cut}")
}

/// Kill the probe's whole process group. Mirrors
/// `openvhost_conf::inspect::kill_process_group`, including the reason the
/// checks below are written out rather than asserted in prose.
fn kill_process_group(pgid: Option<u32>) {
    // No pid means the child was already reaped; nothing to signal.
    let Some(pgid) = pgid else { return };
    let Ok(pgid) = i32::try_from(pgid) else {
        return;
    };
    if pgid > 1 {
        // SAFETY: a plain `kill` syscall — no memory is handed over. The two
        // checks above are the negation's preconditions made visible: `pgid`
        // is in `2..=i32::MAX`, so `-pgid` is in `-i32::MAX..=-2`. It cannot
        // be `0` (our OWN group), cannot be `-1` (every process we may
        // signal), and cannot evaluate `-(i32::MIN)`. A negative pid targets
        // the process GROUP, which is the point: it reaches any grandchild a
        // profile forked. `pgid` came from `Child::id()` on a child spawned
        // ten statements above with `process_group(0)`; nothing outside this
        // module can choose it. The result is ignored — ESRCH here only means
        // the group already exited, the common case.
        unsafe {
            libc::kill(-pgid, libc::SIGKILL);
        }
    }
}

// ---------------------------------------------------------------------------
// PATH membership
// ---------------------------------------------------------------------------

/// The non-empty elements of a PATH value.
///
/// An **empty element** means "the current directory" to every POSIX shell,
/// so it can never name one of our absolute candidates; dropping it is exact,
/// not a simplification. A trailing colon produces exactly one of those.
pub(super) fn path_elements(path: &str) -> Vec<&str> {
    path.split(':').filter(|e| !e.is_empty()).collect()
}

/// Does `path` (a PATH value) contain an element naming the **same
/// directory** as `dir`?
///
/// Decided by the filesystem — `(st_dev, st_ino)` — because the filesystem is
/// what decides it for the shell too. That single choice settles three
/// questions that a string comparison gets wrong, each verified on the target
/// machine:
///
/// - **Case.** macOS's default APFS volume is case-insensitive, so
///   `/usr/local/BIN` on PATH really does find binaries in `/usr/local/bin`
///   (`stat` on both returns the same dev+ino), while on a case-sensitive
///   volume it really does not (`stat` fails). Neither a case-sensitive nor a
///   case-insensitive *string* comparison is right on both; asking the
///   filesystem is right on both. Note `realpath` does **not** normalise
///   case on macOS, so canonicalising the strings would not have worked.
/// - **A literal `~`.** `PATH="~/.local/bin:…"` finds nothing — verified in
///   both `zsh` and `sh`, because the kernel does not expand `~` and neither
///   shell re-expands PATH at lookup time. `stat("~/.local/bin")` fails for
///   the same reason, so this returns `false`, which is not a conservative
///   guess but the literal truth.
/// - **Symlinked entries.** A PATH entry that is a symlink to our directory
///   does work for command lookup, and `metadata` follows it, so it matches.
///
/// `dir` must itself be an existing directory for anything to match: nothing
/// on PATH can name the same directory as a path that is not one. The
/// exact-string comparison inside the loop is then belt-and-braces for an
/// element we could not `stat` but that is spelled identically — it is
/// deliberately **not** reachable when `dir` is a regular file, which an
/// unguarded string comparison would have let through.
pub(super) fn path_contains_dir(path: &str, dir: &Path) -> bool {
    let Some(wanted) = dir_identity(dir) else {
        return false;
    };
    path_elements(path).into_iter().any(|element| {
        let element = Path::new(element);
        element == dir || dir_identity(element) == Some(wanted)
    })
}

/// `(st_dev, st_ino)` of `p`, or `None` if it is not an existing directory.
/// Follows symlinks — deliberately, see [`path_contains_dir`].
fn dir_identity(p: &Path) -> Option<(u64, u64)> {
    let md = std::fs::metadata(p).ok()?;
    md.is_dir().then(|| (md.dev(), md.ino()))
}

// ---------------------------------------------------------------------------
// What to tell the user, and where to put it
// ---------------------------------------------------------------------------

/// The shells we can give file-specific advice for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShellKind {
    Zsh,
    Bash,
    Unknown,
}

fn shell_kind(shell: Option<&OsStr>) -> ShellKind {
    let name = shell
        .map(Path::new)
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    match name {
        "zsh" => ShellKind::Zsh,
        "bash" => ShellKind::Bash,
        _ => ShellKind::Unknown,
    }
}

/// Which file to add the `export` line to.
///
/// **zsh gets `.zprofile`, not `.zshrc`,** and that is not a style
/// preference. A login zsh reads `.zshenv`, `.zprofile` and `.zlogin`; it
/// reads `.zshrc` only when interactive. Our own probe runs `zsh -l -c`,
/// which is non-interactive — so had we told the user to edit `.zshrc`, they
/// would follow the advice, their Terminal would work, and this app would go
/// on reporting `NotOnPath` forever. `.zprofile` is read by both, and is what
/// Homebrew's own instructions use.
///
/// The reverse mismatch is possible and harmless: a user whose PATH edit
/// already lives in `.zshrc` is reported `NotOnPath` when in fact they are
/// fine. That direction only ever shows an unnecessary `export` line; it can
/// never render "you're all set" on a guess, which is the direction D4 cares
/// about.
fn profile_for(kind: ShellKind, home: Option<&Path>) -> PathBuf {
    let name = match kind {
        ShellKind::Zsh => ".zprofile",
        ShellKind::Bash => ".bash_profile",
        // POSIX-ish fallback. Note a `fish` user would be sent here and to an
        // `export` line fish does not understand — a known gap, recorded
        // rather than guessed at.
        ShellKind::Unknown => ".profile",
    };
    match home {
        Some(home) => home.join(name),
        None => PathBuf::from(name),
    }
}

/// The line to add. **We never write it ourselves** (D4): silently appending
/// to a shell profile is how tools break people's shells, and nobody who does
/// it offers an undo.
fn export_line(dir: &Path) -> String {
    format!("export PATH=\"{}:$PATH\"", sh_double_quote_escape(dir))
}

/// Escape the four characters that are still special inside POSIX double
/// quotes, so the line we print is one the user can paste verbatim. `$PATH`
/// itself is deliberately left outside this — it must still expand.
///
/// Lossy on a non-UTF-8 path, which is acceptable for a string we only ever
/// display; macOS paths are UTF-8 in practice.
fn sh_double_quote_escape(dir: &Path) -> String {
    let mut out = String::new();
    for ch in dir.to_string_lossy().chars() {
        if matches!(ch, '"' | '\\' | '$' | '`') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

/// Turn a raw [`PathStatusProbe`] into the three-state verdict for `dir`.
///
/// Exhaustive over the probe: a failure becomes [`PathStatus::Unknown`],
/// never `NotOnPath` and never `OnPath`.
pub(super) fn path_status(
    dir: &Path,
    probe: &PathStatusProbe,
    shell: Option<&OsStr>,
    home: Option<&Path>,
) -> PathStatus {
    match probe {
        PathStatusProbe::Resolved { path } if path_contains_dir(path, dir) => PathStatus::OnPath,
        PathStatusProbe::Resolved { .. } => PathStatus::NotOnPath {
            export_line: export_line(dir),
            profile: profile_for(shell_kind(shell), home),
        },
        PathStatusProbe::Failed { reason } => PathStatus::Unknown {
            reason: reason.clone(),
            export_line: export_line(dir),
            profile: profile_for(shell_kind(shell), home),
        },
    }
}

/// The verdict [`super::detect`] reports without probing at all — see
/// [`super::InstallState`] for why a synchronous `detect` must not probe.
pub(super) fn unprobed_status(
    dir: &Path,
    shell: Option<&OsStr>,
    home: Option<&Path>,
) -> PathStatus {
    PathStatus::Unknown {
        reason: NOT_PROBED.to_string(),
        export_line: export_line(dir),
        profile: profile_for(shell_kind(shell), home),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    // --- group: PATH parsing -------------------------------------------

    #[test]
    fn a_trailing_colon_does_not_produce_an_element() {
        assert_eq!(path_elements("/a:/b:"), vec!["/a", "/b"]);
    }

    #[test]
    fn an_empty_element_is_dropped_rather_than_treated_as_a_directory() {
        assert_eq!(path_elements("/a::/b"), vec!["/a", "/b"]);
        assert_eq!(path_elements(":/a"), vec!["/a"]);
        assert!(path_elements("").is_empty());
        assert!(path_elements(":::").is_empty());
    }

    #[test]
    fn a_duplicate_element_is_kept_as_written() {
        assert_eq!(path_elements("/a:/b:/a"), vec!["/a", "/b", "/a"]);
    }

    #[test]
    fn elements_containing_spaces_survive_intact() {
        assert_eq!(
            path_elements("/Applications/Visual Studio Code.app/bin:/usr/bin"),
            vec!["/Applications/Visual Studio Code.app/bin", "/usr/bin"]
        );
    }

    // --- group: PATH membership ----------------------------------------

    #[test]
    fn a_directory_listed_verbatim_is_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        fs::create_dir(&dir).unwrap();
        let path = format!("/usr/bin:{}:/bin", dir.display());
        assert!(path_contains_dir(&path, &dir));
    }

    #[test]
    fn a_directory_absent_from_path_is_not_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        fs::create_dir(&dir).unwrap();
        assert!(!path_contains_dir("/usr/bin:/bin", &dir));
    }

    #[test]
    fn a_duplicate_entry_still_counts_once_and_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        fs::create_dir(&dir).unwrap();
        let path = format!("{d}:/usr/bin:{d}:", d = dir.display());
        assert!(path_contains_dir(&path, &dir));
    }

    /// A literal `~` in PATH finds nothing — verified against `zsh` and `sh`
    /// on the target machine — so it must not count as being on PATH.
    #[test]
    fn a_tilde_relative_entry_does_not_count_as_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".local").join("bin");
        fs::create_dir_all(&dir).unwrap();
        assert!(!path_contains_dir("~/.local/bin:/usr/bin", &dir));
    }

    /// A PATH entry that is a symlink to our directory DOES work for command
    /// lookup, so it must count.
    #[test]
    fn a_symlinked_path_entry_pointing_at_the_directory_counts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("real-bin");
        fs::create_dir(&dir).unwrap();
        let alias = tmp.path().join("alias-bin");
        std::os::unix::fs::symlink(&dir, &alias).unwrap();
        assert!(path_contains_dir(
            &format!("{}:/usr/bin", alias.display()),
            &dir
        ));
    }

    /// The case question, decided by the volume rather than by us. On the
    /// default (case-insensitive) macOS volume the differently-cased spelling
    /// really does work for command lookup and must match; on a
    /// case-sensitive volume it really does not and must not. The assertion
    /// is therefore against the filesystem's own answer, so this test is
    /// correct on both — and it fails loudly if the matcher ever stops
    /// agreeing with the kernel.
    #[test]
    fn case_differing_entries_match_exactly_when_the_volume_says_they_do() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("casebin");
        fs::create_dir(&dir).unwrap();
        let shouty = tmp.path().join("CASEBIN");
        let volume_is_case_insensitive = shouty.is_dir();

        assert_eq!(
            path_contains_dir(&format!("{}:/usr/bin", shouty.display()), &dir),
            volume_is_case_insensitive,
            "matcher disagreed with the volume (case-insensitive: {volume_is_case_insensitive})"
        );
    }

    #[test]
    fn a_nonexistent_path_entry_never_matches() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        fs::create_dir(&dir).unwrap();
        assert!(!path_contains_dir("/no/such/dir/anywhere:/usr/bin", &dir));
    }

    /// A regular *file* on PATH is not a directory and must never satisfy the
    /// membership test, even if some element resolves to it.
    #[test]
    fn a_file_listed_on_path_is_not_a_matching_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("notadir");
        fs::write(&file, b"x").unwrap();
        assert!(!path_contains_dir(&format!("{}", file.display()), &file));
    }

    // --- group: export line and profile file ---------------------------

    #[test]
    fn zsh_is_pointed_at_zprofile_because_a_login_zsh_never_reads_zshrc() {
        assert_eq!(
            profile_for(
                shell_kind(Some(OsStr::new("/bin/zsh"))),
                Some(Path::new("/Users/t"))
            ),
            PathBuf::from("/Users/t/.zprofile")
        );
    }

    #[test]
    fn bash_is_pointed_at_bash_profile() {
        assert_eq!(
            profile_for(
                shell_kind(Some(OsStr::new("/bin/bash"))),
                Some(Path::new("/Users/t"))
            ),
            PathBuf::from("/Users/t/.bash_profile")
        );
    }

    #[test]
    fn an_unknown_shell_falls_back_to_dot_profile() {
        for shell in ["/opt/homebrew/bin/fish", "/usr/local/bin/nu", "/bin/sh"] {
            assert_eq!(
                profile_for(
                    shell_kind(Some(OsStr::new(shell))),
                    Some(Path::new("/Users/t"))
                ),
                PathBuf::from("/Users/t/.profile"),
                "{shell}"
            );
        }
    }

    #[test]
    fn an_unset_shell_falls_back_to_dot_profile() {
        assert_eq!(
            profile_for(shell_kind(None), Some(Path::new("/Users/t"))),
            PathBuf::from("/Users/t/.profile")
        );
    }

    #[test]
    fn the_export_line_prepends_the_directory_and_keeps_path_expanding() {
        assert_eq!(
            export_line(Path::new("/Users/t/.local/bin")),
            "export PATH=\"/Users/t/.local/bin:$PATH\""
        );
    }

    /// The line is displayed for the user to paste, so a home directory with
    /// a shell metacharacter in it must not produce a line that does
    /// something else when pasted.
    #[test]
    fn the_export_line_escapes_characters_that_stay_special_inside_double_quotes() {
        let line = export_line(Path::new("/Users/a\"b$c`d\\e/.local/bin"));
        assert_eq!(
            line,
            "export PATH=\"/Users/a\\\"b\\$c\\`d\\\\e/.local/bin:$PATH\""
        );
        // and `$PATH` itself is untouched, or the line would be useless
        assert!(line.ends_with(":$PATH\""));
    }

    // --- group: the three-state verdict (D4) ---------------------------

    #[test]
    fn a_resolved_probe_listing_the_directory_is_on_path() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        fs::create_dir(&dir).unwrap();
        let probe = PathStatusProbe::Resolved {
            path: format!("/usr/bin:{}", dir.display()),
        };
        assert_eq!(
            path_status(&dir, &probe, Some(OsStr::new("/bin/zsh")), None),
            PathStatus::OnPath
        );
    }

    #[test]
    fn a_resolved_probe_without_the_directory_yields_advice_not_a_verdict() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bin");
        fs::create_dir(&dir).unwrap();
        let probe = PathStatusProbe::Resolved {
            path: "/usr/bin:/bin".to_string(),
        };
        match path_status(
            &dir,
            &probe,
            Some(OsStr::new("/bin/zsh")),
            Some(Path::new("/Users/t")),
        ) {
            PathStatus::NotOnPath {
                export_line,
                profile,
            } => {
                assert!(export_line.contains(&dir.display().to_string()));
                assert_eq!(profile, PathBuf::from("/Users/t/.zprofile"));
            }
            other => panic!("expected NotOnPath, got {other:?}"),
        }
    }

    /// The single most important assertion in this file: a failed probe must
    /// never collapse into either confident answer.
    #[test]
    fn a_failed_probe_is_unknown_and_still_carries_the_advice() {
        let probe = PathStatusProbe::Failed {
            reason: "it timed out".to_string(),
        };
        match path_status(
            Path::new("/Users/t/.local/bin"),
            &probe,
            Some(OsStr::new("/bin/zsh")),
            Some(Path::new("/Users/t")),
        ) {
            PathStatus::Unknown {
                reason,
                export_line,
                profile,
            } => {
                assert_eq!(reason, "it timed out");
                assert!(export_line.contains("/Users/t/.local/bin"));
                assert_eq!(profile, PathBuf::from("/Users/t/.zprofile"));
            }
            other => panic!("a failed probe must be Unknown, got {other:?}"),
        }
    }

    #[test]
    fn the_unprobed_status_is_unknown_and_says_so() {
        match unprobed_status(Path::new("/x/bin"), Some(OsStr::new("/bin/bash")), None) {
            PathStatus::Unknown { reason, .. } => assert_eq!(reason, NOT_PROBED),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    // --- group: the login-shell probe itself ---------------------------

    /// Write an executable stand-in for a login shell. It ignores `-l -c` and
    /// does whatever `body` says, which is what lets us drive every branch.
    fn fake_shell(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[tokio::test]
    async fn a_shell_that_prints_its_path_is_resolved_verbatim() {
        let tmp = tempfile::tempdir().unwrap();
        let shell = fake_shell(tmp.path(), "goodsh", "printf %s '/opt/x/bin:/usr/bin'");
        assert_eq!(
            probe_shell_path(&shell).await,
            PathStatusProbe::Resolved {
                path: "/opt/x/bin:/usr/bin".to_string()
            }
        );
    }

    #[tokio::test]
    async fn a_shell_that_exits_non_zero_fails_and_quotes_its_complaint() {
        let tmp = tempfile::tempdir().unwrap();
        let shell = fake_shell(tmp.path(), "badsh", "echo 'profile is broken' >&2\nexit 3");
        match probe_shell_path(&shell).await {
            PathStatusProbe::Failed { reason } => {
                assert!(reason.contains("profile is broken"), "got {reason:?}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_shell_that_cannot_be_run_fails_rather_than_hanging() {
        let tmp = tempfile::tempdir().unwrap();
        match probe_shell_path(&tmp.path().join("no-such-shell")).await {
            PathStatusProbe::Failed { reason } => assert!(reason.contains("could not run")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_relative_shell_is_refused_without_being_spawned() {
        match probe_shell_path(Path::new("zsh")).await {
            PathStatusProbe::Failed { reason } => assert!(reason.contains("absolute")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_implausibly_large_answer_is_refused_rather_than_parsed() {
        let tmp = tempfile::tempdir().unwrap();
        let shell = fake_shell(
            tmp.path(),
            "floodsh",
            "i=0; while [ $i -lt 300 ]; do printf '%01000d' 0; i=$((i+1)); done",
        );
        match probe_shell_path(&shell).await {
            PathStatusProbe::Failed { reason } => assert!(reason.contains("not a PATH")),
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    /// The hang D4 exists to bound — and the grandchild the profile forked
    /// must die with it, which `kill_on_drop` alone would not achieve.
    #[tokio::test]
    async fn a_hanging_shell_times_out_and_takes_its_background_children_with_it() {
        let tmp = tempfile::tempdir().unwrap();
        let pidfile = tmp.path().join("grandchild.pid");
        let shell = fake_shell(
            tmp.path(),
            "hangsh",
            &format!("sleep 30 & echo $! > '{}'\nsleep 30", pidfile.display()),
        );

        let started = std::time::Instant::now();
        let result = probe_shell_path(&shell).await;
        let elapsed = started.elapsed();

        match result {
            PathStatusProbe::Failed { reason } => {
                assert!(reason.contains("did not answer"), "got {reason:?}")
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert!(
            elapsed >= PROBE_TIMEOUT && elapsed < PROBE_TIMEOUT + Duration::from_secs(3),
            "expected a bounded wait, took {elapsed:?}"
        );

        let pid: i32 = fs::read_to_string(&pidfile)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // Poll: SIGKILL delivery and reaping are not instantaneous.
        let mut alive = true;
        for _ in 0..100 {
            // SAFETY: signal 0 performs the permission/existence check only
            // and delivers nothing. `pid` was written by our own fake shell.
            if unsafe { libc::kill(pid, 0) } != 0 {
                alive = false;
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(
            !alive,
            "the forked grandchild (pid {pid}) outlived the probe"
        );
    }
}
