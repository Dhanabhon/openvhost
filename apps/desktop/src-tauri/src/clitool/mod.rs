// SPDX-License-Identifier: GPL-3.0-or-later
//! Putting `openvhost` on the user's PATH — the resolve / detect / install
//! logic behind **OpenVHost → Install Command Line Tool…** (P1 CLI-install
//! design, `docs/superpowers/specs/2026-07-31-p1-cli-install-design.md`,
//! D1–D5).
//!
//! Nothing here talks to Tauri, and nothing here is a Tauri command: the menu
//! handler calls [`install`] directly, exactly as the tray's handlers call
//! `service_control`. The webview gets no new surface.
//!
//! ## The one fact the whole design rests on
//!
//! On the target machine `/usr/local/bin` exists but is **not** writable,
//! while `~/.local/bin` exists, is writable, and is already on PATH. A
//! writable directory on PATH already exists, so **this feature never
//! escalates privileges** — no `sudo`, no `osascript with administrator
//! privileges`, no authorization prompt. The privileged helper is Phase 3.
//!
//! ## Module split
//!
//! - [`detect`] — read-only filesystem questions: where our binary is
//!   ([`source_binary`]), where it may go ([`candidate_dirs`]), what is
//!   sitting at `<dir>/openvhost` right now, and [`detect`](detect()).
//! - [`shell`] — everything about the user's *login shell*: the bounded
//!   `$SHELL -l -c 'printf %s "$PATH"'` probe ([`login_shell_path`]), PATH
//!   membership, and the `export` line / profile filename we recommend.
//! - [`install`] — the only code in this module that writes anything: choose
//!   a candidate, apply the D3 clobber rules, stage a symlink under a
//!   temporary name and `rename` it over the target.
//!
//! ## Why `#[cfg(unix)]` and not `#[cfg(target_os = "macos")]`
//!
//! Every mechanism used here is POSIX — `symlink`, `rename`, `st_dev`/
//! `st_ino`, a login shell — and none of it is macOS-specific beyond the
//! `…​.app/Contents/MacOS` shape that [`detect`] merely *recognises*. Gating
//! on `unix` mirrors `openvhost_proc::orphan::lock::InstanceLock`, which
//! splits `#[cfg(unix)]` / `#[cfg(not(unix))]` for the same reason, and keeps
//! this usable as-is by a future Linux target. **Windows** is the platform
//! actually deferred project-wide, and it lands in the `not(unix)` arm below,
//! which reports [`CliToolError::Unsupported`] rather than pretending to
//! succeed.

use std::path::{Path, PathBuf};

#[cfg(unix)]
mod detect;
#[cfg(unix)]
mod install;
#[cfg(unix)]
mod shell;

#[cfg(unix)]
pub use detect::{candidate_dirs, detect, source_binary};
#[cfg(unix)]
pub use install::install;
#[cfg(unix)]
pub use shell::login_shell_path;

/// The file name of the CLI, both beside the app binary (the symlink source)
/// and inside the candidate directory (the symlink itself).
///
/// Public so the dialog layer can name the exact occupied path on a
/// [`InstallOutcome::Refused`] without hardcoding the string a second time —
/// see [`installed_path`].
pub const CLI_BINARY_NAME: &str = "openvhost";

/// The full path the tool occupies inside `dir` — `<dir>/openvhost`.
///
/// [`InstallOutcome::Refused`] and [`InstallState::Blocked`] carry the
/// *directory*, per the published interface; this is how a caller turns that
/// into the path it must name to the user.
pub fn installed_path(dir: &Path) -> PathBuf {
    dir.join(CLI_BINARY_NAME)
}

/// Whether the directory we installed into is on the PATH the user's **login
/// shell** actually produces (D4).
///
/// Three states, never a `bool`. A GUI app launched from Finder inherits a
/// minimal PATH, not the shell's, so `std::env::var("PATH")` would make the
/// app confidently wrong; and when the probe fails we must say so rather than
/// render "you're all set" on a guess. This codebase has collapsed a state
/// into a boolean four times and paid for it every time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStatus {
    /// The login shell's PATH contains the directory. `openvhost` runs from a
    /// fresh terminal with no path prefix.
    OnPath,
    /// The probe succeeded and the directory is genuinely absent from PATH.
    /// `export_line` is what to add; `profile` is the file to add it to.
    NotOnPath {
        export_line: String,
        profile: PathBuf,
    },
    /// The probe failed or timed out. `reason` says why; `export_line` and
    /// `profile` are offered **as a precaution**, and the caller must say
    /// plainly that the check did not succeed.
    Unknown {
        reason: String,
        export_line: String,
        profile: PathBuf,
    },
}

/// The raw answer from [`login_shell_path`], before it is interpreted against
/// any particular directory.
///
/// Separate from [`PathStatus`] because the probe genuinely does not know
/// which directory we installed into — it asks one question ("what is your
/// PATH?") and reports success or failure honestly. Turning that into a
/// [`PathStatus`] needs the directory as well, and that is [`shell`]'s
/// `path_status`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathStatusProbe {
    /// The login shell answered. `path` is its `PATH`, verbatim.
    Resolved { path: String },
    /// The shell could not be run, exited non-zero, or did not answer within
    /// the timeout. Never collapsed into "not on PATH".
    Failed { reason: String },
}

/// What [`detect`](detect()) found (D5) — a state, not a checkbox.
///
/// `Broken` is the case a boolean would hide, and it is not hypothetical: the
/// user drags the app to the Trash or renames it, and the symlink dangles.
///
/// **`path_status` inside `Installed` is always
/// [`PathStatus::Unknown`] here**, and deliberately so. [`detect`](detect())
/// is synchronous because it runs while the menu is being built, and the D4
/// probe spawns a login shell that may take up to two seconds — blocking menu
/// construction on that is unacceptable. So `detect()` answers the filesystem
/// question only and states, in `reason`, that the PATH check was not run.
/// The real verdict reaches the user through [`InstallOutcome`], which is
/// produced by the `async` [`install`] and always carries a genuine probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallState {
    /// Nothing of ours in any candidate directory.
    NotInstalled,
    /// Our symlink is there and its target resolves.
    Installed {
        dir: PathBuf,
        path_status: PathStatus,
    },
    /// Our symlink is there and its target is gone — the app moved or was
    /// deleted. Repairable: the menu item reads "Reinstall…".
    Broken { dir: PathBuf, reason: String },
    /// Something that is **not ours** occupies `<dir>/openvhost`. We never
    /// touch it.
    Blocked { dir: PathBuf, what_is_there: String },
}

/// What [`install`] actually did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallOutcome {
    /// Nothing was there; the symlink was created.
    Installed {
        dir: PathBuf,
        path_status: PathStatus,
    },
    /// Our symlink already pointed at exactly this binary. **Nothing was
    /// written** — not even a re-`rename`, so the inode is unchanged.
    AlreadyInstalled {
        dir: PathBuf,
        path_status: PathStatus,
    },
    /// Our symlink was there but stale or dangling (an older install, or the
    /// app moved); it now points at this binary.
    Repaired {
        dir: PathBuf,
        path_status: PathStatus,
    },
    /// Something that is not ours occupies the path. **Nothing was written,
    /// nothing was unlinked.** `what_is_there` describes the node; the
    /// directory is `dir` and the exact path is [`installed_path`].
    Refused { dir: PathBuf, what_is_there: String },
}

/// Everything that can go wrong before we get as far as an
/// [`InstallOutcome`]. A refusal is *not* an error — it is a normal outcome
/// the user must be told about, so it lives in [`InstallOutcome::Refused`].
#[derive(Debug, thiserror::Error)]
pub enum CliToolError {
    /// `std::env::current_exe()` failed, or returned something with no parent
    /// directory or no absolute path. Without it we cannot find our own
    /// sibling binary, and we will not go looking on PATH for some other
    /// `openvhost` (D1).
    #[error("could not determine this application's own location: {0}")]
    CurrentExe(String),
    /// The `openvhost` binary is not beside the app binary. In a packaged
    /// build that means the bundle was assembled without it; in a dev build
    /// it means `cargo build -p openvhost` has not been run.
    #[error(
        "the `openvhost` command line binary is not next to this application (expected it at {})",
        .0.display()
    )]
    SourceMissing(PathBuf),
    /// No candidate directory could be written to. Cannot happen while
    /// `$HOME` is writable — a defensive arm, not a user-facing path (D2).
    #[error("no writable directory to install into; tried {0}")]
    NoWritableDir(String),
    /// A filesystem call we cannot recover from. Distinct from
    /// [`InstallOutcome::Refused`]: this is "we could not tell / could not
    /// act", not "we decided not to".
    #[error("could not {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// Windows (project-wide macOS-first). Reported rather than silently
    /// succeeding, mirroring `InstanceLock::acquire`'s non-unix arm.
    #[error("installing the command line tool is not supported on this platform")]
    Unsupported,
}

// ---------------------------------------------------------------------------
// Non-unix: report unsupported. Mirrors `openvhost_proc::orphan::lock`'s
// `#[cfg(not(unix))]` arm — an explicit refusal, never a pretend success.
// ---------------------------------------------------------------------------

/// See the `#[cfg(unix)]` implementation in [`detect`].
#[cfg(not(unix))]
pub fn source_binary() -> Result<PathBuf, CliToolError> {
    Err(CliToolError::Unsupported)
}

/// See the `#[cfg(unix)]` implementation in [`detect`]. Empty, not a guess:
/// neither `/usr/local/bin` nor `~/.local/bin` means anything on Windows.
#[cfg(not(unix))]
pub fn candidate_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// See the `#[cfg(unix)]` implementation in [`shell`].
#[cfg(not(unix))]
pub async fn login_shell_path() -> PathStatusProbe {
    PathStatusProbe::Failed {
        reason: "the login-shell PATH probe is not implemented on this platform".to_string(),
    }
}

/// See the `#[cfg(unix)]` implementation in [`detect`].
///
/// `NotInstalled` is the truthful answer on a platform where we never install
/// anything, and it keeps the menu label sensible; the *action* is what
/// reports [`CliToolError::Unsupported`], from [`install`].
#[cfg(not(unix))]
pub fn detect() -> InstallState {
    InstallState::NotInstalled
}

/// See the `#[cfg(unix)]` implementation in [`install`].
#[cfg(not(unix))]
pub async fn install() -> Result<InstallOutcome, CliToolError> {
    Err(CliToolError::Unsupported)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn installed_path_appends_the_binary_name_to_the_directory() {
        assert_eq!(
            installed_path(Path::new("/usr/local/bin")),
            PathBuf::from("/usr/local/bin/openvhost")
        );
    }

    /// The four states are distinct values, not shades of one. A regression
    /// that made `Broken` compare equal to `Installed` (say, by dropping the
    /// discriminant in a refactor) would hide exactly the case D5 exists for.
    #[test]
    fn install_states_do_not_compare_equal_across_variants() {
        let dir = PathBuf::from("/usr/local/bin");
        let installed = InstallState::Installed {
            dir: dir.clone(),
            path_status: PathStatus::OnPath,
        };
        let broken = InstallState::Broken {
            dir: dir.clone(),
            reason: "gone".to_string(),
        };
        let blocked = InstallState::Blocked {
            dir,
            what_is_there: "a regular file".to_string(),
        };
        assert_ne!(installed, broken);
        assert_ne!(broken, blocked);
        assert_ne!(blocked, InstallState::NotInstalled);
    }

    /// `Unknown` must never be mistaken for `OnPath` by a caller that only
    /// checks "is it not `NotOnPath`".
    #[test]
    fn path_status_unknown_is_not_on_path() {
        let unknown = PathStatus::Unknown {
            reason: "timed out".to_string(),
            export_line: "export PATH=\"/x:$PATH\"".to_string(),
            profile: PathBuf::from("/home/u/.zprofile"),
        };
        assert_ne!(unknown, PathStatus::OnPath);
    }
}
