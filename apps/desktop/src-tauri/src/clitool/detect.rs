// SPDX-License-Identifier: GPL-3.0-or-later
//! Read-only half of the CLI-install slice: where our binary is, where it may
//! go, and what is already sitting at `<dir>/openvhost` (D1–D3, D5).
//!
//! Nothing in this file writes to the filesystem. [`super::install`] is the
//! only module that does.

use std::ffi::OsStr;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use super::{CLI_BINARY_NAME, CliToolError, InstallState};

/// The traditional location: on the default macOS PATH and, on Apple Silicon,
/// *not* managed by Homebrew.
const SYSTEM_LOCAL_BIN: &str = "/usr/local/bin";

/// `/opt/homebrew/bin` is **deliberately not a candidate**, even though it is
/// writable and on PATH on every Apple Silicon machine with Homebrew.
///
/// That directory belongs to Homebrew, and `brew doctor` reports unbrewed
/// symlinks there as a warning. Shipping a feature that makes a widely-used
/// diagnostic complain is not a good trade for one saved fallback. This
/// constant exists so the exclusion reads as a decision rather than an
/// omission, and so the test that pins it names the thing it is excluding
/// (D2). **Do not "fix" this by adding it to [`candidates_from`].**
#[cfg(test)]
const DELIBERATELY_EXCLUDED: &str = "/opt/homebrew/bin";

/// A directory we may install into, plus whether we are allowed to create it.
///
/// The flag is not cosmetic: `~/.local/bin` is ours to make (0755, D2), while
/// `/usr/local/bin` is a system directory we will observe and use but never
/// bring into existence.
pub(super) struct Candidate {
    pub dir: PathBuf,
    pub create_if_absent: bool,
}

/// Candidates in D2 order. The first **writable** one wins; see
/// [`super::install::place`], which is where "writable" is decided (by trying,
/// not by asking).
pub(super) fn candidates() -> Vec<Candidate> {
    candidates_from(dirs::home_dir().as_deref())
}

/// Pure core of [`candidates`], testable without touching the real home.
pub(super) fn candidates_from(home: Option<&Path>) -> Vec<Candidate> {
    let mut out = vec![Candidate {
        dir: PathBuf::from(SYSTEM_LOCAL_BIN),
        create_if_absent: false,
    }];
    if let Some(home) = home {
        out.push(Candidate {
            dir: home.join(".local").join("bin"),
            create_if_absent: true,
        });
    }
    out
}

/// The directories [`super::install`] will consider, in order. Never contains
/// `/opt/homebrew/bin` (D2), never contains `/bin`, `/usr/bin` or `/sbin`.
pub fn candidate_dirs() -> Vec<PathBuf> {
    candidates().into_iter().map(|c| c.dir).collect()
}

/// The `openvhost` binary that rides beside this application (D1).
///
/// Resolved from `std::env::current_exe()`'s **parent joined with
/// `openvhost`** — never a hardcoded `/Applications`, never a PATH search. Two
/// payoffs: the app works wherever the user drags it, and in a **dev build**
/// `target/debug/openvhost` sits beside `openvhost-desktop`, so the same code
/// path works unbundled.
///
/// If the sibling is missing this fails with that as the reason. It never
/// falls back to some other `openvhost` found elsewhere.
///
/// ## Moved while running
///
/// `current_exe()` on macOS returns the path recorded at `exec` time and does
/// **not** track a later move — measured, not assumed: a binary moved out from
/// under a running process still reports its original path, and does not start
/// returning an error. So if the user drags the app somewhere else and then
/// uses the menu item without relaunching, this fails with
/// [`CliToolError::SourceMissing`] naming the path the app *used* to occupy.
/// That is unhelpful prose but the right behaviour: it is a refusal, and the
/// alternative — installing a symlink to a binary that is no longer there — is
/// exactly the `Broken` state D5 exists to detect. A relaunch fixes it, and the
/// verification click-list moves the app across a relaunch for that reason.
pub fn source_binary() -> Result<PathBuf, CliToolError> {
    let exe = std::env::current_exe().map_err(|e| CliToolError::CurrentExe(e.to_string()))?;
    source_binary_from(&exe)
}

/// Pure core of [`source_binary`], testable without being the executable.
pub(super) fn source_binary_from(exe: &Path) -> Result<PathBuf, CliToolError> {
    // Absolute is required, not merely tidy: the result becomes a **symlink
    // target**, and a relative target would be resolved against the candidate
    // directory the link lives in — pointing at `/usr/local/bin/openvhost`
    // itself, i.e. a loop. `current_exe` is absolute on every unix we build
    // for; this turns that assumption into a checked one.
    if !exe.is_absolute() {
        return Err(CliToolError::CurrentExe(format!(
            "{} is not an absolute path",
            exe.display()
        )));
    }
    let dir = exe.parent().ok_or_else(|| {
        CliToolError::CurrentExe(format!("{} has no parent directory", exe.display()))
    })?;
    let source = dir.join(CLI_BINARY_NAME);
    match std::fs::metadata(&source) {
        Ok(md) if md.is_file() => Ok(source),
        // A directory (or anything else) named `openvhost` beside us is not a
        // binary we can point a symlink at, and treating it as one would
        // install a link to something unrunnable.
        Ok(_) | Err(_) => Err(CliToolError::SourceMissing(source)),
    }
}

/// What is sitting at `<dir>/openvhost` right now (D3's decision table).
///
/// The whole security surface of this slice is the boundary between
/// `Ours*` and [`Occupant::Foreign`]: everything `Ours*` may be `rename`d
/// over, and nothing `Foreign` is ever touched.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Occupant {
    /// Nothing there.
    Absent,
    /// Our symlink, already resolving to exactly the binary we would install.
    /// Nothing to do.
    OursCurrent,
    /// Our symlink, pointing somewhere else that is recognisably a build of
    /// this tool — an older install, or the app moved. `target` is the link
    /// text; `resolves` says whether it currently leads to a file.
    OursStale { target: PathBuf, resolves: bool },
    /// Not ours. Never unlinked, never renamed over. `what` describes it for
    /// the user.
    Foreign { what: String },
}

/// Classify `link` against the binary we would install.
///
/// `source` is `None` when the caller could not resolve its own sibling
/// binary — [`detect`] still has to answer, and it can: without a source we
/// simply cannot distinguish [`Occupant::OursCurrent`] from
/// [`Occupant::OursStale`], and both mean "installed" to `detect`.
///
/// An I/O error other than "not found" is **not** flattened into
/// [`Occupant::Absent`]. That distinction matters: "absent" leads to a
/// create, and a permission error read as absence would send us at a path we
/// cannot see.
pub(super) fn classify(link: &Path, source: Option<&Path>) -> Result<Occupant, CliToolError> {
    // `symlink_metadata`, not `metadata`: we must see the LINK, not what it
    // points at. `metadata` on a dangling symlink returns NotFound, which
    // would classify the exact case D5 calls `Broken` as "nothing there" and
    // silently create over it — right outcome, wrong reason, and it would
    // also silently create over a dangling link that was never ours.
    let md = match std::fs::symlink_metadata(link) {
        Ok(md) => md,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Occupant::Absent),
        Err(e) => {
            return Err(CliToolError::Io {
                op: "inspect",
                path: link.to_path_buf(),
                source: e,
            });
        }
    };
    if !md.file_type().is_symlink() {
        return Ok(Occupant::Foreign {
            what: describe(&md.file_type()),
        });
    }
    let target = std::fs::read_link(link).map_err(|e| CliToolError::Io {
        op: "read the symlink at",
        path: link.to_path_buf(),
        source: e,
    })?;
    // Identity, not spelling. `same_file` follows `link` all the way, so a
    // RELATIVE link target resolves against the link's own directory (as the
    // kernel would), a `/private/var` vs `/var` alias compares equal, and a
    // case-different spelling on a case-insensitive volume compares equal —
    // all three would defeat a string comparison and cause a needless rename
    // of a link that is already correct.
    if let Some(source) = source
        && same_file(link, source)
    {
        return Ok(Occupant::OursCurrent);
    }
    if looks_like_ours(&target) {
        return Ok(Occupant::OursStale {
            target,
            // Follows the link: `Ok` means it leads to something real.
            resolves: std::fs::metadata(link).is_ok(),
        });
    }
    Ok(Occupant::Foreign {
        what: format!("a symlink to {}", target.display()),
    })
}

/// Could this application have written a symlink with this target?
///
/// **Deliberately narrow.** This predicate is the only thing standing between
/// a user's own `openvhost` on PATH and a `rename` over it, so it recognises
/// exactly the two shapes D3 names and nothing else:
///
/// - `…/<name>.app/Contents/MacOS/openvhost` — a macOS application bundle,
///   which is where D1 puts the binary.
/// - `…/target/{debug,release}/openvhost` — a Cargo build directory, which is
///   where D1's dev-build path puts it.
///
/// A build with `CARGO_TARGET_DIR` pointed somewhere unusual, or a
/// `--target`-suffixed directory (`target/aarch64-apple-darwin/debug`), is
/// **not** recognised, so a stale link from one of those is refused rather
/// than replaced. That is the correct bias: refusing costs a developer one
/// `rm`; guessing wrong costs a user their own binary.
///
/// Note this never runs for a link that already resolves to our source —
/// [`classify`] checks identity first — so an unusual build directory still
/// re-installs cleanly as long as the app has not moved.
fn looks_like_ours(target: &Path) -> bool {
    if target.file_name() != Some(OsStr::new(CLI_BINARY_NAME)) {
        return false;
    }
    match target.parent() {
        Some(parent) => in_app_bundle(parent) || in_cargo_target(parent),
        None => false,
    }
}

/// Is `dir` the `Contents/MacOS` of something ending in `.app`?
fn in_app_bundle(dir: &Path) -> bool {
    if dir.file_name() != Some(OsStr::new("MacOS")) {
        return false;
    }
    let Some(contents) = dir.parent() else {
        return false;
    };
    if contents.file_name() != Some(OsStr::new("Contents")) {
        return false;
    }
    contents
        .parent()
        .and_then(|bundle| bundle.extension())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("app"))
}

/// Is `dir` a Cargo `target/debug` or `target/release`?
fn in_cargo_target(dir: &Path) -> bool {
    let Some(profile) = dir.file_name() else {
        return false;
    };
    if profile != OsStr::new("debug") && profile != OsStr::new("release") {
        return false;
    }
    dir.parent().and_then(|t| t.file_name()) == Some(OsStr::new("target"))
}

/// Do `a` and `b` name the same file? Follows symlinks on both sides.
///
/// `(st_dev, st_ino)` rather than path equality — see [`classify`] for the
/// three ways path equality gets this wrong on macOS.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(a), Ok(b)) => (a.dev(), a.ino()) == (b.dev(), b.ino()),
        // One of them does not exist: they are not the same file. A dangling
        // link is handled by `looks_like_ours` instead.
        _ => false,
    }
}

/// A short human description of a node we refuse to touch, for the dialog.
fn describe(ft: &std::fs::FileType) -> String {
    use std::os::unix::fs::FileTypeExt;
    if ft.is_file() {
        "a regular file".to_string()
    } else if ft.is_dir() {
        "a directory".to_string()
    } else if ft.is_fifo() {
        "a named pipe".to_string()
    } else if ft.is_socket() {
        "a socket".to_string()
    } else if ft.is_char_device() {
        "a character device".to_string()
    } else if ft.is_block_device() {
        "a block device".to_string()
    } else {
        "an unrecognised filesystem node".to_string()
    }
}

/// What state the command line tool is in right now (D5).
///
/// Synchronous, and therefore **does not run the D4 login-shell probe** — see
/// [`super::InstallState`] for why, and for what the `path_status` inside
/// `Installed` says instead.
pub fn detect() -> InstallState {
    detect_in(
        &candidates(),
        source_binary().ok().as_deref(),
        std::env::var_os("SHELL").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// Pure-ish core of [`detect`]: same logic, injected inputs.
///
/// Scans **every** candidate rather than stopping at the first, and reports
/// the strongest finding: `Installed` beats `Broken` beats `Blocked` beats
/// `NotInstalled`, ties going to the earlier directory. The alternative —
/// answering about one chosen directory — would report `Blocked` for a user
/// who has a foreign file in `/usr/local/bin` and a perfectly good install in
/// `~/.local/bin`, i.e. tell someone who can run `openvhost` that they
/// cannot.
pub(super) fn detect_in(
    candidates: &[Candidate],
    source: Option<&Path>,
    shell: Option<&OsStr>,
    home: Option<&Path>,
) -> InstallState {
    let mut best = InstallState::NotInstalled;
    for candidate in candidates {
        let link = candidate.dir.join(CLI_BINARY_NAME);
        let found = match classify(&link, source) {
            Ok(Occupant::Absent) => continue,
            Ok(Occupant::OursCurrent) => InstallState::Installed {
                dir: candidate.dir.clone(),
                path_status: super::shell::unprobed_status(&candidate.dir, shell, home),
            },
            Ok(Occupant::OursStale { target, resolves }) => {
                if resolves {
                    InstallState::Installed {
                        dir: candidate.dir.clone(),
                        path_status: super::shell::unprobed_status(&candidate.dir, shell, home),
                    }
                } else {
                    InstallState::Broken {
                        dir: candidate.dir.clone(),
                        reason: format!(
                            "it points at {}, which no longer exists",
                            target.display()
                        ),
                    }
                }
            }
            Ok(Occupant::Foreign { what }) => InstallState::Blocked {
                dir: candidate.dir.clone(),
                what_is_there: what,
            },
            // "Could not tell" blocks us just as effectively as "occupied",
            // and saying so beats silently reporting `NotInstalled`.
            Err(e) => InstallState::Blocked {
                dir: candidate.dir.clone(),
                what_is_there: format!("something that could not be inspected ({e})"),
            },
        };
        if rank(&found) > rank(&best) {
            best = found;
        }
    }
    best
}

/// Ordering for [`detect_in`]'s "strongest finding wins". Exhaustive by
/// construction — a new [`InstallState`] variant fails to compile here.
fn rank(state: &InstallState) -> u8 {
    match state {
        InstallState::NotInstalled => 0,
        InstallState::Blocked { .. } => 1,
        InstallState::Broken { .. } => 2,
        InstallState::Installed { .. } => 3,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::clitool::PathStatus;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn touch(path: &Path) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, b"binary").unwrap();
    }

    // --- group: source_binary (D1) -------------------------------------

    #[test]
    fn source_binary_is_the_sibling_of_the_running_executable() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("openvhost-desktop");
        touch(&exe);
        touch(&tmp.path().join("openvhost"));

        let source = source_binary_from(&exe).unwrap();

        assert_eq!(source, tmp.path().join("openvhost"));
        assert_eq!(source.parent(), exe.parent());
    }

    #[test]
    fn source_binary_fails_naming_the_expected_path_when_the_sibling_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("openvhost-desktop");
        touch(&exe);

        let err = source_binary_from(&exe).unwrap_err();

        match err {
            CliToolError::SourceMissing(p) => assert_eq!(p, tmp.path().join("openvhost")),
            other => panic!("expected SourceMissing, got {other:?}"),
        }
    }

    /// D1: never a PATH search. With the sibling absent, the answer is an
    /// error even though a real `openvhost` may well exist elsewhere on this
    /// machine — the failure above is the proof that no fallback exists.
    #[test]
    fn source_binary_rejects_a_directory_named_like_the_binary() {
        let tmp = tempfile::tempdir().unwrap();
        let exe = tmp.path().join("openvhost-desktop");
        touch(&exe);
        fs::create_dir(tmp.path().join("openvhost")).unwrap();

        assert!(matches!(
            source_binary_from(&exe),
            Err(CliToolError::SourceMissing(_))
        ));
    }

    #[test]
    fn source_binary_rejects_a_relative_executable_path() {
        assert!(matches!(
            source_binary_from(Path::new("target/debug/openvhost-desktop")),
            Err(CliToolError::CurrentExe(_))
        ));
    }

    // --- group: candidate ordering (D2) --------------------------------

    #[test]
    fn candidates_are_usr_local_bin_then_home_local_bin() {
        let home = PathBuf::from("/Users/tester");
        let dirs: Vec<PathBuf> = candidates_from(Some(&home))
            .into_iter()
            .map(|c| c.dir)
            .collect();
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/usr/local/bin"),
                PathBuf::from("/Users/tester/.local/bin"),
            ]
        );
    }

    /// D2, stated as a test so a later "helpful" addition trips it.
    #[test]
    fn homebrew_bin_is_never_a_candidate() {
        let home = PathBuf::from("/Users/tester");
        let dirs = candidates_from(Some(&home));
        assert!(
            !dirs
                .iter()
                .any(|c| c.dir.as_path() == Path::new(DELIBERATELY_EXCLUDED)),
            "{DELIBERATELY_EXCLUDED} must stay excluded — brew doctor warns on unbrewed symlinks"
        );
    }

    #[test]
    fn sip_protected_directories_are_never_candidates() {
        let home = PathBuf::from("/Users/tester");
        let dirs = candidates_from(Some(&home));
        for forbidden in ["/bin", "/usr/bin", "/sbin", "/usr/sbin"] {
            assert!(!dirs.iter().any(|c| c.dir.as_path() == Path::new(forbidden)));
        }
    }

    #[test]
    fn only_the_home_candidate_may_be_created() {
        let home = PathBuf::from("/Users/tester");
        let dirs = candidates_from(Some(&home));
        assert!(!dirs[0].create_if_absent, "/usr/local/bin is not ours");
        assert!(dirs[1].create_if_absent, "~/.local/bin is ours to make");
    }

    #[test]
    fn without_a_home_directory_only_the_system_candidate_remains() {
        let dirs: Vec<PathBuf> = candidates_from(None).into_iter().map(|c| c.dir).collect();
        assert_eq!(dirs, vec![PathBuf::from("/usr/local/bin")]);
    }

    // --- group: the clobber decision table (D3) ------------------------
    //
    // Every node type that can occupy `<dir>/openvhost`, asserted
    // exhaustively: absent, our current link, our link into another app
    // bundle, our link into a Cargo build dir, our dangling link, a foreign
    // link, a regular file, a directory, a fifo.

    struct Bed {
        tmp: tempfile::TempDir,
    }

    impl Bed {
        fn new() -> Self {
            let bed = Bed {
                tmp: tempfile::tempdir().unwrap(),
            };
            touch(&bed.source());
            fs::create_dir_all(bed.bin()).unwrap();
            bed
        }
        fn source(&self) -> PathBuf {
            self.tmp
                .path()
                .join("OpenVHost.app/Contents/MacOS/openvhost")
        }
        fn bin(&self) -> PathBuf {
            self.tmp.path().join("bin")
        }
        fn link(&self) -> PathBuf {
            self.bin().join("openvhost")
        }
        fn classify(&self) -> Occupant {
            classify(&self.link(), Some(&self.source())).unwrap()
        }
    }

    #[test]
    fn nothing_there_is_absent() {
        let bed = Bed::new();
        assert_eq!(bed.classify(), Occupant::Absent);
    }

    #[test]
    fn our_link_pointing_at_the_binary_we_would_install_is_current() {
        let bed = Bed::new();
        symlink(bed.source(), bed.link()).unwrap();
        assert_eq!(bed.classify(), Occupant::OursCurrent);
    }

    #[test]
    fn our_link_into_a_different_app_bundle_is_stale_not_foreign() {
        let bed = Bed::new();
        let older = bed
            .tmp
            .path()
            .join("Old/OpenVHost.app/Contents/MacOS/openvhost");
        touch(&older);
        symlink(&older, bed.link()).unwrap();
        assert_eq!(
            bed.classify(),
            Occupant::OursStale {
                target: older,
                resolves: true
            }
        );
    }

    #[test]
    fn our_link_into_a_cargo_build_directory_is_stale_not_foreign() {
        let bed = Bed::new();
        let dev = bed.tmp.path().join("checkout/target/debug/openvhost");
        touch(&dev);
        symlink(&dev, bed.link()).unwrap();
        assert_eq!(
            bed.classify(),
            Occupant::OursStale {
                target: dev,
                resolves: true
            }
        );
    }

    #[test]
    fn our_link_whose_target_is_gone_is_stale_and_does_not_resolve() {
        let bed = Bed::new();
        let gone = bed.tmp.path().join("Moved.app/Contents/MacOS/openvhost");
        symlink(&gone, bed.link()).unwrap();
        assert_eq!(
            bed.classify(),
            Occupant::OursStale {
                target: gone,
                resolves: false
            }
        );
    }

    #[test]
    fn a_symlink_pointing_anywhere_else_is_foreign() {
        let bed = Bed::new();
        let theirs = bed.tmp.path().join("their-openvhost");
        touch(&theirs);
        symlink(&theirs, bed.link()).unwrap();
        match bed.classify() {
            Occupant::Foreign { what } => assert!(
                what.contains(&theirs.display().to_string()),
                "the refusal must name what it found, got {what:?}"
            ),
            other => panic!("a foreign symlink must be Foreign, got {other:?}"),
        }
    }

    /// A Homebrew-style link, the realistic collision: same *name*, wrong
    /// place. Recognising it as ours would delete a user's brew-managed tool.
    #[test]
    fn a_symlink_to_a_homebrew_cellar_binary_is_foreign() {
        let bed = Bed::new();
        let brewed = bed.tmp.path().join("Cellar/openvhost/1.0/bin/openvhost");
        touch(&brewed);
        symlink(&brewed, bed.link()).unwrap();
        assert!(matches!(bed.classify(), Occupant::Foreign { .. }));
    }

    #[test]
    fn a_regular_file_is_foreign() {
        let bed = Bed::new();
        touch(&bed.link());
        assert_eq!(
            bed.classify(),
            Occupant::Foreign {
                what: "a regular file".to_string()
            }
        );
    }

    #[test]
    fn a_directory_is_foreign() {
        let bed = Bed::new();
        fs::create_dir(bed.link()).unwrap();
        assert_eq!(
            bed.classify(),
            Occupant::Foreign {
                what: "a directory".to_string()
            }
        );
    }

    #[test]
    fn a_fifo_is_foreign() {
        let bed = Bed::new();
        let status = std::process::Command::new("/usr/bin/mkfifo")
            .arg(bed.link())
            .status()
            .unwrap();
        assert!(status.success(), "mkfifo failed");
        assert_eq!(
            bed.classify(),
            Occupant::Foreign {
                what: "a named pipe".to_string()
            }
        );
    }

    /// `Contents/MacOS/openvhost` under something that is NOT a `.app` is not
    /// an app bundle — the `.app` extension is what makes the shape ours.
    #[test]
    fn a_contents_macos_path_outside_a_dot_app_bundle_is_foreign() {
        let bed = Bed::new();
        let decoy = bed.tmp.path().join("NotABundle/Contents/MacOS/openvhost");
        touch(&decoy);
        symlink(&decoy, bed.link()).unwrap();
        assert!(matches!(bed.classify(), Occupant::Foreign { .. }));
    }

    /// A link to a *differently named* binary inside our own bundle is not
    /// ours either: the file name is part of the shape.
    #[test]
    fn a_link_to_another_binary_in_our_own_bundle_is_foreign() {
        let bed = Bed::new();
        let sibling = bed
            .tmp
            .path()
            .join("OpenVHost.app/Contents/MacOS/openvhost-desktop");
        touch(&sibling);
        symlink(&sibling, bed.link()).unwrap();
        assert!(matches!(bed.classify(), Occupant::Foreign { .. }));
    }

    /// Without a source, "current" is unknowable — but "ours" is not, and
    /// `detect` only needs the latter.
    #[test]
    fn without_a_source_our_own_link_still_classifies_as_ours() {
        let bed = Bed::new();
        symlink(bed.source(), bed.link()).unwrap();
        assert!(matches!(
            classify(&bed.link(), None).unwrap(),
            Occupant::OursStale { resolves: true, .. }
        ));
    }

    // --- group: InstallState classification (D5) -----------------------

    fn bed_candidates(dirs: &[&Path]) -> Vec<Candidate> {
        dirs.iter()
            .map(|d| Candidate {
                dir: d.to_path_buf(),
                create_if_absent: false,
            })
            .collect()
    }

    #[test]
    fn an_empty_candidate_directory_is_not_installed() {
        let bed = Bed::new();
        let state = detect_in(
            &bed_candidates(&[&bed.bin()]),
            Some(&bed.source()),
            None,
            None,
        );
        assert_eq!(state, InstallState::NotInstalled);
    }

    #[test]
    fn our_resolving_link_is_installed_and_names_the_directory() {
        let bed = Bed::new();
        symlink(bed.source(), bed.link()).unwrap();
        match detect_in(
            &bed_candidates(&[&bed.bin()]),
            Some(&bed.source()),
            None,
            None,
        ) {
            InstallState::Installed { dir, .. } => assert_eq!(dir, bed.bin()),
            other => panic!("expected Installed, got {other:?}"),
        }
    }

    /// D5's whole point: the app was moved, and this must not read as
    /// "installed" nor as "nothing there".
    #[test]
    fn our_dangling_link_is_broken_and_says_what_is_missing() {
        let bed = Bed::new();
        let gone = bed.tmp.path().join("Moved.app/Contents/MacOS/openvhost");
        symlink(&gone, bed.link()).unwrap();
        match detect_in(
            &bed_candidates(&[&bed.bin()]),
            Some(&bed.source()),
            None,
            None,
        ) {
            InstallState::Broken { dir, reason } => {
                assert_eq!(dir, bed.bin());
                assert!(
                    reason.contains(&gone.display().to_string()),
                    "the reason must name the missing target, got {reason:?}"
                );
            }
            other => panic!("expected Broken, got {other:?}"),
        }
    }

    #[test]
    fn a_foreign_file_is_blocked_and_says_what_is_there() {
        let bed = Bed::new();
        touch(&bed.link());
        match detect_in(
            &bed_candidates(&[&bed.bin()]),
            Some(&bed.source()),
            None,
            None,
        ) {
            InstallState::Blocked { dir, what_is_there } => {
                assert_eq!(dir, bed.bin());
                assert_eq!(what_is_there, "a regular file");
            }
            other => panic!("expected Blocked, got {other:?}"),
        }
    }

    /// A real install in the second candidate must not be hidden by junk in
    /// the first — the user CAN run `openvhost`, and saying otherwise is the
    /// same class of lie as a boolean.
    #[test]
    fn a_working_install_in_a_later_directory_outranks_a_blocked_earlier_one() {
        let bed = Bed::new();
        let second = bed.tmp.path().join("bin2");
        fs::create_dir_all(&second).unwrap();
        touch(&bed.link()); // junk in the first candidate
        symlink(bed.source(), second.join("openvhost")).unwrap();

        match detect_in(
            &bed_candidates(&[&bed.bin(), &second]),
            Some(&bed.source()),
            None,
            None,
        ) {
            InstallState::Installed { dir, .. } => assert_eq!(dir, second),
            other => panic!("expected Installed from the second dir, got {other:?}"),
        }
    }

    #[test]
    fn a_broken_link_outranks_a_blocked_directory_but_loses_to_an_install() {
        let bed = Bed::new();
        let second = bed.tmp.path().join("bin2");
        fs::create_dir_all(&second).unwrap();
        touch(&bed.link()); // Blocked
        symlink(
            bed.tmp.path().join("Gone.app/Contents/MacOS/openvhost"),
            second.join("openvhost"),
        )
        .unwrap(); // Broken

        assert!(matches!(
            detect_in(
                &bed_candidates(&[&bed.bin(), &second]),
                Some(&bed.source()),
                None,
                None
            ),
            InstallState::Broken { .. }
        ));
    }

    /// `detect` is synchronous and must not claim a PATH verdict it never
    /// checked (D4's "never render 'you're all set' on a guess").
    #[test]
    fn detect_never_reports_on_path_because_it_never_probes() {
        let bed = Bed::new();
        symlink(bed.source(), bed.link()).unwrap();
        match detect_in(
            &bed_candidates(&[&bed.bin()]),
            Some(&bed.source()),
            Some(OsStr::new("/bin/zsh")),
            Some(Path::new("/Users/tester")),
        ) {
            InstallState::Installed {
                path_status: PathStatus::Unknown { reason, .. },
                ..
            } => assert!(!reason.is_empty(), "the caveat must say something"),
            other => panic!("expected Installed with Unknown path status, got {other:?}"),
        }
    }
}
