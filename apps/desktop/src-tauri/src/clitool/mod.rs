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
// What the user is told (D5/D6). Every user-visible string this feature has
// lives in this section — one place to extract from when the Phase 2 i18n
// layer lands, and one place to read when reviewing the copy.
//
// Pure by construction: these take enum values and return text. Nothing here
// touches Tauri, so the DECISIONS are unit-testable even though the dialog
// that shows them (`quit::show_report_dialog`, no `NSAlert` in a test
// process) is not — the same split `tray::service_control::failure_dialog_text`
// already uses.
// ---------------------------------------------------------------------------

/// How a [`Report`] should be presented.
///
/// This module's OWN enum rather than `tauri_plugin_dialog::MessageDialogKind`
/// because nothing here talks to Tauri (see the module docs). The caller maps
/// it in one exhaustive match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    /// The tool is where it should be.
    Info,
    /// Nothing was damaged, but the tool is not installed and the user has to
    /// act before it can be.
    Warning,
    /// We could not act at all.
    Error,
}

/// A dialog's worth of text: what happened, and what to do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub kind: ReportKind,
    pub title: String,
    pub body: String,
}

/// The app-menu row's label for the state [`detect`](detect()) found (D5/D6).
///
/// Exhaustive — a new [`InstallState`] variant fails to compile here rather
/// than silently inheriting whichever label a wildcard arm happened to name.
///
/// **`Installed` reads "Install…", not "Installed"**: the row is an action,
/// and running it while already installed is a legitimate, documented no-op
/// (it reports [`InstallOutcome::AlreadyInstalled`] and changes nothing).
/// Only `Broken` gets its own wording, because that is the one state where
/// the user is being told something they did not know — the link is there and
/// does not work.
pub fn menu_label(state: &InstallState) -> &'static str {
    match state {
        InstallState::Broken { .. } => "Reinstall Command Line Tool…",
        InstallState::NotInstalled
        | InstallState::Installed { .. }
        | InstallState::Blocked { .. } => "Install Command Line Tool…",
    }
}

/// What to tell the user about what [`install`] just did.
///
/// Exhaustive over [`InstallOutcome`], and the PATH paragraph is exhaustive
/// over [`PathStatus`] — so every one of the ten reachable combinations
/// produces its own text. That is asserted, not merely intended: collapsing
/// two states into one sentence is the exact failure this codebase has paid
/// for four times.
pub fn report_for_outcome(outcome: &InstallOutcome) -> Report {
    match outcome {
        InstallOutcome::Installed { dir, path_status } => Report {
            kind: ReportKind::Info,
            title: "Command line tool installed".to_string(),
            body: format!(
                "openvhost is now linked at {}.\n\n{}",
                installed_path(dir).display(),
                path_paragraph(dir, path_status)
            ),
        },
        InstallOutcome::AlreadyInstalled { dir, path_status } => Report {
            kind: ReportKind::Info,
            title: "Command line tool already installed".to_string(),
            body: format!(
                "openvhost was already linked at {}. Nothing was changed.\n\n{}",
                installed_path(dir).display(),
                path_paragraph(dir, path_status)
            ),
        },
        InstallOutcome::Repaired { dir, path_status } => Report {
            kind: ReportKind::Info,
            title: "Command line tool repaired".to_string(),
            body: format!(
                "The link at {} pointed somewhere else. It now points at this copy of \
                 OpenVHost.\n\n{}",
                installed_path(dir).display(),
                path_paragraph(dir, path_status)
            ),
        },
        // Names the exact PATH, not just the directory: the user has to be
        // able to go and look at the thing we refused to touch. Printing it
        // is also what makes D8's "no uninstall action" honest — `rm` on a
        // path we printed is the whole answer.
        InstallOutcome::Refused { dir, what_is_there } => Report {
            kind: ReportKind::Warning,
            title: "Command line tool not installed".to_string(),
            body: format!(
                "{} is {}, which OpenVHost did not create — so it was left exactly as it \
                 was.\n\nMove or remove it yourself, then run this again.",
                installed_path(dir).display(),
                what_is_there
            ),
        },
    }
}

/// The PATH verdict paragraph (D4), exhaustive over [`PathStatus`].
///
/// `NotOnPath` **and** `Unknown` both carry the `export` line, and `Unknown`
/// says plainly that the check did not succeed. There is deliberately no
/// wording anywhere in this function that claims the tool is reachable
/// unless the probe actually said so.
fn path_paragraph(dir: &Path, status: &PathStatus) -> String {
    match status {
        PathStatus::OnPath => format!(
            "{} is on your PATH. Open a new terminal window and run: openvhost list",
            dir.display()
        ),
        PathStatus::NotOnPath {
            export_line,
            profile,
        } => format!(
            "{} is not on your PATH, so a new terminal window will not find openvhost \
             yet.\n\nAdd this line to {}, then open a new terminal window:\n\n{}",
            dir.display(),
            profile.display(),
            export_line
        ),
        // The caveat comes FIRST, before any advice, so a user who reads one
        // sentence reads the one that says we do not know.
        PathStatus::Unknown {
            reason,
            export_line,
            profile,
        } => format!(
            "OpenVHost could not check whether {} is on your PATH: {}\n\nIf a new terminal \
             window cannot find openvhost, add this line to {}:\n\n{}",
            dir.display(),
            reason,
            profile.display(),
            export_line
        ),
    }
}

/// What to tell the user when the install could not happen at all.
///
/// The error's own `Display` is the evidence; [`next_step`] is the way
/// forward. Brand guidelines §6.2: state what happened, show the evidence,
/// offer the next action — never just "something went wrong".
pub fn report_for_error(error: &CliToolError) -> Report {
    Report {
        kind: ReportKind::Error,
        title: "Could not install the command line tool".to_string(),
        body: format!("{error}\n\n{}", next_step(error)),
    }
}

/// The one actionable sentence per failure. Exhaustive over [`CliToolError`]
/// — a new variant fails to compile here rather than silently rendering a
/// dead end.
fn next_step(error: &CliToolError) -> &'static str {
    match error {
        // `current_exe` is recorded at exec time; a relaunch re-establishes
        // it. See `detect::source_binary`'s "moved while running" note.
        CliToolError::CurrentExe(_) => "Relaunch OpenVHost and try again.",
        CliToolError::SourceMissing(_) => {
            "Reinstall OpenVHost — this copy is missing its command line binary. \
             In a development build, run: cargo build -p openvhost"
        }
        // D2: this cannot happen while $HOME is writable, so the home
        // directory is the thing worth checking.
        CliToolError::NoWritableDir(_) => {
            "Check that your home directory is writable, then try again."
        }
        CliToolError::Io { .. } => "Check the permissions on that path, then try again.",
        CliToolError::Unsupported => {
            "Run the openvhost binary directly from where it is installed instead."
        }
    }
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

    // -----------------------------------------------------------------------
    // group: the menu label follows `detect()` (D5/D6).
    //
    // VACUITY (neuter-and-watch-it-fail): folded `Broken` into the shared arm
    // of `menu_label` so every state returned "Install Command Line Tool…" —
    // `a_broken_install_offers_to_reinstall` failed on the "Reinstall"
    // assertion while every other test in this group kept passing, which is
    // the point: only the `Broken` case distinguishes the two labels.
    // -----------------------------------------------------------------------

    const DIR: &str = "/Users/tester/.local/bin";
    const EXPORT_LINE: &str = "export PATH=\"/Users/tester/.local/bin:$PATH\"";
    const PROFILE: &str = "/Users/tester/.zprofile";

    #[test]
    fn a_broken_install_offers_to_reinstall() {
        assert_eq!(
            menu_label(&InstallState::Broken {
                dir: PathBuf::from(DIR),
                reason: "it points at /Gone.app/Contents/MacOS/openvhost".to_string(),
            }),
            "Reinstall Command Line Tool…"
        );
    }

    #[test]
    fn every_other_state_offers_to_install() {
        let states = [
            InstallState::NotInstalled,
            InstallState::Installed {
                dir: PathBuf::from(DIR),
                path_status: PathStatus::OnPath,
            },
            InstallState::Blocked {
                dir: PathBuf::from(DIR),
                what_is_there: "a regular file".to_string(),
            },
        ];
        for state in &states {
            assert_eq!(menu_label(state), "Install Command Line Tool…", "{state:?}");
        }
    }

    /// The row is an action, and macOS convention is that a trailing ellipsis
    /// means "this opens something". Pinned because the verification
    /// click-list looks for this exact string (D6).
    #[test]
    fn both_labels_end_in_an_ellipsis() {
        for state in [
            InstallState::NotInstalled,
            InstallState::Broken {
                dir: PathBuf::from(DIR),
                reason: "gone".to_string(),
            },
        ] {
            assert!(menu_label(&state).ends_with('…'), "{state:?}");
        }
    }

    // -----------------------------------------------------------------------
    // group: every outcome x every path status renders a DISTINCT message.
    //
    // This is the group guarding the failure this codebase has paid for four
    // times — a rendering that collapses two states into one text. Asserting
    // "not empty" would not catch it; asserting pairwise inequality does.
    //
    // VACUITY (neuter-and-watch-it-fail), twice, once per axis:
    //
    // - STATUS AXIS: made `path_paragraph`'s `Unknown` arm render `NotOnPath`'s
    //   paragraph verbatim (`reason: _`, i.e. "we could not check" silently
    //   becomes "it is not on PATH") — the exact collapse D4 exists to
    //   prevent. `every_outcome_and_path_status_combination_renders_a_distinct_message`
    //   FAILED with "Installed/NotOnPath and Installed/Unknown render the same
    //   dialog", and `an_unknown_verdict_renders_the_caveat_and_the_export_line`
    //   failed alongside it.
    // - OUTCOME AXIS: gave `Repaired` `Installed`'s title and first line —
    //   only the distinctness test FAILED ("Installed/OnPath and
    //   Repaired/OnPath render the same dialog"); every other test in this
    //   file still passed, which is precisely why pairwise inequality is
    //   asserted rather than a per-variant substring check.
    //
    // Restoring each arm made them pass again.
    // -----------------------------------------------------------------------

    /// The three PATH verdicts, with `NotOnPath` and `Unknown` deliberately
    /// carrying the **same** `export_line` and `profile`. The only difference
    /// between those two inputs is which variant they are — so a renderer
    /// that printed the advice and ignored the state it came from produces
    /// identical text and fails the distinctness assertion below.
    fn statuses() -> Vec<(&'static str, PathStatus)> {
        vec![
            ("OnPath", PathStatus::OnPath),
            (
                "NotOnPath",
                PathStatus::NotOnPath {
                    export_line: EXPORT_LINE.to_string(),
                    profile: PathBuf::from(PROFILE),
                },
            ),
            (
                "Unknown",
                PathStatus::Unknown {
                    reason: "/bin/zsh did not answer within 2 seconds".to_string(),
                    export_line: EXPORT_LINE.to_string(),
                    profile: PathBuf::from(PROFILE),
                },
            ),
        ]
    }

    /// Every reachable outcome, labelled: the three that carry a
    /// [`PathStatus`] crossed with all three verdicts, plus `Refused`, which
    /// carries none (an install that never happened has no PATH verdict to
    /// report, and inventing one would be the guess D4 forbids).
    fn every_report() -> Vec<(String, Report)> {
        let dir = PathBuf::from(DIR);
        let mut out = Vec::new();
        for (status_name, status) in statuses() {
            let built: [(&str, InstallOutcome); 3] = [
                (
                    "Installed",
                    InstallOutcome::Installed {
                        dir: dir.clone(),
                        path_status: status.clone(),
                    },
                ),
                (
                    "AlreadyInstalled",
                    InstallOutcome::AlreadyInstalled {
                        dir: dir.clone(),
                        path_status: status.clone(),
                    },
                ),
                (
                    "Repaired",
                    InstallOutcome::Repaired {
                        dir: dir.clone(),
                        path_status: status.clone(),
                    },
                ),
            ];
            for (outcome_name, outcome) in built {
                out.push((
                    format!("{outcome_name}/{status_name}"),
                    report_for_outcome(&outcome),
                ));
            }
        }
        out.push((
            "Refused".to_string(),
            report_for_outcome(&InstallOutcome::Refused {
                dir,
                what_is_there: "a regular file".to_string(),
            }),
        ));
        out
    }

    #[test]
    fn every_outcome_and_path_status_combination_renders_a_distinct_message() {
        let reports = every_report();
        assert_eq!(
            reports.len(),
            10,
            "3 outcomes x 3 verdicts, plus Refused — update this if a variant lands"
        );
        for (i, (a_name, a)) in reports.iter().enumerate() {
            for (b_name, b) in reports.iter().skip(i + 1) {
                assert_ne!(
                    a, b,
                    "{a_name} and {b_name} render the same dialog — two states collapsed into one"
                );
            }
        }
    }

    #[test]
    fn every_report_names_the_directory_it_acted_on() {
        for (name, report) in every_report() {
            assert!(!report.title.is_empty(), "{name} has no title");
            assert!(
                report.body.contains(DIR),
                "{name} never names the directory: {}",
                report.body
            );
        }
    }

    /// The one outcome that is not a success must not look like one.
    #[test]
    fn a_refusal_is_a_warning_and_every_completed_install_is_not() {
        for (name, report) in every_report() {
            let expected = if name == "Refused" {
                ReportKind::Warning
            } else {
                ReportKind::Info
            };
            assert_eq!(report.kind, expected, "{name}");
        }
    }

    // -----------------------------------------------------------------------
    // group: the PATH verdict paragraph, state by state (D4).
    // -----------------------------------------------------------------------

    fn body_for(status: PathStatus) -> String {
        report_for_outcome(&InstallOutcome::Installed {
            dir: PathBuf::from(DIR),
            path_status: status,
        })
        .body
    }

    /// THE ONE THAT MATTERS MOST: a probe that did not succeed must say so
    /// **and** still offer the advice, and must never read as "you're all
    /// set".
    #[test]
    fn an_unknown_verdict_renders_the_caveat_and_the_export_line() {
        let body = body_for(PathStatus::Unknown {
            reason: "/bin/zsh did not answer within 2 seconds".to_string(),
            export_line: EXPORT_LINE.to_string(),
            profile: PathBuf::from(PROFILE),
        });
        assert!(
            body.contains("could not check"),
            "the caveat is missing: {body}"
        );
        assert!(
            body.contains("/bin/zsh did not answer within 2 seconds"),
            "the reason is missing: {body}"
        );
        assert!(body.contains(EXPORT_LINE), "the export line is missing");
        assert!(body.contains(PROFILE), "the profile file is missing");
        assert!(
            !body.contains("is on your PATH."),
            "an unchecked directory must never be reported as being on PATH: {body}"
        );
    }

    #[test]
    fn a_not_on_path_verdict_renders_the_export_line_and_the_profile_to_add_it_to() {
        let body = body_for(PathStatus::NotOnPath {
            export_line: EXPORT_LINE.to_string(),
            profile: PathBuf::from(PROFILE),
        });
        assert!(body.contains("not on your PATH"), "{body}");
        assert!(body.contains(EXPORT_LINE), "the export line is missing");
        assert!(body.contains(PROFILE), "the profile file is missing");
    }

    /// The profile file is rendered VERBATIM from the enum. `.zprofile` is
    /// the one the shell layer computes for zsh, and it is not `.zshrc` for a
    /// hard-won reason (a login zsh never reads `.zshrc`, and the probe is
    /// `zsh -l -c`) — this layer must not second-guess it by naming a file of
    /// its own.
    #[test]
    fn the_profile_is_whatever_the_enum_carries_and_is_never_named_here() {
        for profile in ["/Users/tester/.bash_profile", "/Users/tester/.profile"] {
            let body = body_for(PathStatus::NotOnPath {
                export_line: EXPORT_LINE.to_string(),
                profile: PathBuf::from(profile),
            });
            assert!(body.contains(profile), "{body}");
            assert!(
                !body.contains(".zprofile") && !body.contains(".zshrc"),
                "this layer invented a profile filename: {body}"
            );
        }
    }

    #[test]
    fn an_on_path_verdict_tells_the_user_what_to_run_and_offers_no_export_line() {
        let body = body_for(PathStatus::OnPath);
        assert!(body.contains("is on your PATH"), "{body}");
        assert!(body.contains("openvhost list"), "{body}");
        assert!(
            !body.contains("export PATH"),
            "no advice is owed when it already works: {body}"
        );
    }

    // -----------------------------------------------------------------------
    // group: a refusal names the exact occupied path (D3).
    //
    // VACUITY (neuter-and-watch-it-fail): swapped `installed_path(dir)` for
    // `dir` in the `Refused` arm — the dialog then named the DIRECTORY, which
    // is what the enum carries and the easy thing to reach for.
    // `a_refusal_names_the_occupying_path_and_what_is_there` FAILED ("the
    // refusal must name /Users/tester/.local/bin/openvhost"), while
    // `every_report_names_the_directory_it_acted_on` kept passing — a
    // directory-only refusal satisfies that weaker check completely.
    // -----------------------------------------------------------------------

    /// The DIRECTORY is not enough — the user has to be told which file to go
    /// and look at. `installed_path`, not a second hardcoded join.
    #[test]
    fn a_refusal_names_the_occupying_path_and_what_is_there() {
        let dir = PathBuf::from(DIR);
        let report = report_for_outcome(&InstallOutcome::Refused {
            dir: dir.clone(),
            what_is_there: "a symlink to /opt/homebrew/Cellar/openvhost/1.0/bin/openvhost"
                .to_string(),
        });
        let expected = installed_path(&dir).display().to_string();
        assert!(
            report.body.contains(&expected),
            "the refusal must name {expected}, got {}",
            report.body
        );
        assert!(
            report
                .body
                .contains("/opt/homebrew/Cellar/openvhost/1.0/bin/openvhost"),
            "the refusal must say what is there: {}",
            report.body
        );
        assert!(
            report.body.contains("did not create"),
            "the refusal must say we left it alone: {}",
            report.body
        );
    }

    // -----------------------------------------------------------------------
    // group: every error variant renders something actionable.
    //
    // VACUITY (neuter-and-watch-it-fail): dropped `next_step` from
    // `report_for_error`, leaving the body as the error's own `Display` —
    // evidence with no way forward, which reads plausible and is the whole
    // thing brand guidelines §6.2 forbids.
    // `every_error_variant_states_what_happened_and_what_to_do_next` FAILED
    // ("offers no next step") for the first variant, while
    // `every_error_variant_renders_a_distinct_message` kept passing, since
    // the five `Display` strings differ on their own. Restoring it passed.
    // -----------------------------------------------------------------------

    fn every_error() -> Vec<CliToolError> {
        vec![
            CliToolError::CurrentExe("no such process".to_string()),
            CliToolError::SourceMissing(PathBuf::from(
                "/Applications/OpenVHost.app/Contents/MacOS/openvhost",
            )),
            CliToolError::NoWritableDir("/usr/local/bin, /Users/tester/.local/bin".to_string()),
            CliToolError::Io {
                op: "install",
                path: PathBuf::from(DIR).join(CLI_BINARY_NAME),
                source: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
            },
            CliToolError::Unsupported,
        ]
    }

    #[test]
    fn every_error_variant_states_what_happened_and_what_to_do_next() {
        for error in every_error() {
            let report = report_for_error(&error);
            assert_eq!(report.kind, ReportKind::Error, "{error:?}");
            assert!(!report.title.is_empty(), "{error:?}");
            // The evidence: the error's own message, verbatim.
            assert!(
                report.body.contains(&error.to_string()),
                "{error:?} lost its own message: {}",
                report.body
            );
            // The way forward: a sentence past the evidence, not a dead end.
            let advice = report
                .body
                .strip_prefix(&error.to_string())
                .unwrap_or_default();
            assert!(
                advice.trim().len() > 10,
                "{error:?} offers no next step: {}",
                report.body
            );
        }
    }

    #[test]
    fn every_error_variant_renders_a_distinct_message() {
        let reports: Vec<Report> = every_error().iter().map(report_for_error).collect();
        for (i, a) in reports.iter().enumerate() {
            for b in reports.iter().skip(i + 1) {
                assert_ne!(a, b, "two error variants render the same dialog");
            }
        }
    }

    /// A refusal is not an error — it is a normal outcome the user has to act
    /// on — so the two must never render alike either.
    #[test]
    fn a_refusal_never_renders_like_a_failure_to_act() {
        let refused = report_for_outcome(&InstallOutcome::Refused {
            dir: PathBuf::from(DIR),
            what_is_there: "a regular file".to_string(),
        });
        for error in every_error() {
            assert_ne!(refused, report_for_error(&error));
        }
    }
}
