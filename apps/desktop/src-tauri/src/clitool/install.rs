// SPDX-License-Identifier: GPL-3.0-or-later
//! The only code in this slice that writes anything (D2, D3).
//!
//! ## What it may touch
//!
//! Exactly one path: `<candidate>/openvhost`, plus a staging link beside it,
//! where `<candidate>` is one of the two directories
//! [`super::detect::candidates`] returns. Nothing else, ever. There is no
//! user-supplied path anywhere in this feature — the symlink's target comes
//! from `current_exe()` and its name is a constant.
//!
//! ## The two rules
//!
//! 1. **Never unlink something we did not create.** [`super::detect::classify`]
//!    decides that, and anything it calls `Foreign` ends the attempt with
//!    [`super::InstallOutcome::Refused`] — no unlink, no rename, no
//!    truncation. A user with their own `openvhost` on PATH must not lose it
//!    silently.
//! 2. **A half-installed PATH entry is worse than none.** The symlink is
//!    created under a temporary name in the same directory and `rename`d over
//!    the target, so the path is either the old node or the new link and
//!    never something in between. [`StagingLink`] removes the temporary name
//!    on **every** exit path, including `?`, so a failure anywhere between
//!    the two steps leaves no residue.
//!
//! ## "Writable" is decided by trying
//!
//! There is no `access(2)` pre-flight. Creating the staging link *is* the
//! writability test: it answers the question the way the kernel would, with
//! no gap between asking and acting, and the artefact it produces is the one
//! we were going to need anyway. A permission failure moves to the next
//! candidate having written nothing.

use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};

use super::detect::{Candidate, Occupant, candidates, classify, source_binary};
use super::shell::{login_shell_path, path_status};
use super::{CLI_BINARY_NAME, CliToolError, InstallOutcome, PathStatus};

/// Directory mode for a `~/.local/bin` we create (D2).
///
/// Requested, not forced: the process umask still applies, so a user running
/// with a tighter umask gets a tighter directory. We deliberately do **not**
/// `set_permissions` afterwards — widening the mode of a directory in
/// someone's home because our spec named a number would be a worse bug than
/// the one it fixes. An **existing** directory's mode is never touched at
/// all.
const NEW_DIR_MODE: u32 = 0o755;

/// A symlink that exists under a temporary name and will be removed unless it
/// is renamed into place.
///
/// The `Drop` impl is the "no residue" guarantee, and it holds for every exit
/// path — an early `return`, a `?`, or a panic — which no explicit cleanup
/// call could claim.
struct StagingLink(Option<PathBuf>);

impl StagingLink {
    /// Create `source`-pointing symlink at a unique hidden name inside `dir`.
    ///
    /// Same directory as the eventual target, which is what makes the
    /// [`Self::rename_over`] below atomic: `rename` cannot cross filesystems,
    /// and there is no filesystem boundary within one directory.
    fn create(dir: &Path, source: &Path) -> Result<StagingLink, std::io::Error> {
        let name = format!(
            ".{CLI_BINARY_NAME}-install-{}.tmp",
            uuid::Uuid::new_v4().simple()
        );
        let path = dir.join(name);
        std::os::unix::fs::symlink(source, &path)?;
        Ok(StagingLink(Some(path)))
    }

    /// Move the staged link onto `dest`, replacing whatever is there.
    ///
    /// `rename(2)` replaces the destination atomically, so a concurrent
    /// `execve` of `dest` sees either the old node or the new link — never a
    /// missing one. Callers must have classified `dest` as ours first.
    fn rename_over(&mut self, dest: &Path) -> Result<(), CliToolError> {
        let Some(staged) = self.0.take() else {
            return Err(CliToolError::Io {
                op: "install",
                path: dest.to_path_buf(),
                source: std::io::Error::other("the staged link was already consumed"),
            });
        };
        std::fs::rename(&staged, dest).map_err(|e| {
            // `take()` disarmed the guard, so clean up here instead. Ignored
            // on failure: we are already returning an error, and a leftover
            // dotfile is not worth masking it.
            let _ = std::fs::remove_file(&staged);
            CliToolError::Io {
                op: "install",
                path: dest.to_path_buf(),
                source: e,
            }
        })
    }
}

impl Drop for StagingLink {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            // `remove_file` is `unlink(2)`: it removes the LINK, never what
            // it points at.
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// What [`place`] did, before the PATH probe turns it into an
/// [`InstallOutcome`].
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Placed {
    /// Nothing was there; the link was created.
    Created { dir: PathBuf },
    /// Our link already pointed at this binary. Nothing was written.
    Unchanged { dir: PathBuf },
    /// Our link was stale or dangling and now points at this binary.
    Repointed { dir: PathBuf },
    /// Something not ours occupies the path. Nothing was written or removed.
    Refused { dir: PathBuf, what: String },
}

/// Walk the candidates in order and act on the first **writable** one (D2/D3).
///
/// Order of operations inside a candidate is deliberate: create the staging
/// link first (that is the writability test), *then* classify the target.
/// Doing it the other way round would mean classifying a directory we cannot
/// write to and reporting a refusal we would never have acted on anyway.
pub(super) fn place(candidates: &[Candidate], source: &Path) -> Result<Placed, CliToolError> {
    let mut tried: Vec<String> = Vec::new();
    for candidate in candidates {
        tried.push(candidate.dir.display().to_string());
        if !candidate.dir.is_dir() {
            // `/usr/local/bin` is a system directory; observing it is fine,
            // conjuring it is not.
            if !candidate.create_if_absent {
                continue;
            }
            if std::fs::DirBuilder::new()
                .recursive(true)
                .mode(NEW_DIR_MODE)
                .create(&candidate.dir)
                .is_err()
            {
                continue;
            }
        }
        let mut staged = match StagingLink::create(&candidate.dir, source) {
            Ok(staged) => staged,
            Err(e) if is_permission_denied(&e) => continue,
            Err(e) => {
                return Err(CliToolError::Io {
                    op: "create a staging symlink in",
                    path: candidate.dir.clone(),
                    source: e,
                });
            }
        };
        // From here every exit drops `staged`, which unlinks it.
        let link = candidate.dir.join(CLI_BINARY_NAME);
        return match classify(&link, Some(source))? {
            Occupant::OursCurrent => Ok(Placed::Unchanged {
                dir: candidate.dir.clone(),
            }),
            Occupant::Foreign { what } => Ok(Placed::Refused {
                dir: candidate.dir.clone(),
                what,
            }),
            Occupant::Absent => {
                staged.rename_over(&link)?;
                Ok(Placed::Created {
                    dir: candidate.dir.clone(),
                })
            }
            Occupant::OursStale { .. } => {
                staged.rename_over(&link)?;
                Ok(Placed::Repointed {
                    dir: candidate.dir.clone(),
                })
            }
        };
    }
    Err(CliToolError::NoWritableDir(tried.join(", ")))
}

/// Is this the kernel telling us the directory is not ours to write?
///
/// Checked against the raw errno rather than [`std::io::ErrorKind`] so the
/// set is unambiguous and does not depend on which kinds are stable.
fn is_permission_denied(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EACCES) | Some(libc::EPERM) | Some(libc::EROFS)
    )
}

/// Put `openvhost` on the user's PATH, and report exactly what happened.
///
/// Never escalates privileges, never edits a shell profile, never writes
/// outside the two candidate directories.
pub async fn install() -> Result<InstallOutcome, CliToolError> {
    let source = source_binary()?;
    // One exhaustive match over what happened, carrying the constructor for
    // the outcome rather than repeating the match after the probe. A refusal
    // returns here and never pays for a probe it has no use for; every other
    // outcome is reported with a real one, and the probe therefore delays the
    // *report* only — never the install.
    type Build = fn(PathBuf, PathStatus) -> InstallOutcome;
    let (dir, build): (PathBuf, Build) = match place(&candidates(), &source)? {
        Placed::Refused { dir, what } => {
            return Ok(InstallOutcome::Refused {
                dir,
                what_is_there: what,
            });
        }
        Placed::Created { dir } => (dir, |dir, status| InstallOutcome::Installed {
            dir,
            path_status: status,
        }),
        Placed::Unchanged { dir } => (dir, |dir, status| InstallOutcome::AlreadyInstalled {
            dir,
            path_status: status,
        }),
        Placed::Repointed { dir } => (dir, |dir, status| InstallOutcome::Repaired {
            dir,
            path_status: status,
        }),
    };
    let status = path_status(
        &dir,
        &login_shell_path().await,
        std::env::var_os("SHELL").as_deref(),
        dirs::home_dir().as_deref(),
    );
    Ok(build(dir, status))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

    struct Bed {
        tmp: tempfile::TempDir,
    }

    impl Bed {
        fn new() -> Bed {
            let bed = Bed {
                tmp: tempfile::tempdir().unwrap(),
            };
            let source = bed.source();
            fs::create_dir_all(source.parent().unwrap()).unwrap();
            fs::write(&source, b"the cli binary").unwrap();
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
        fn candidates(&self) -> Vec<Candidate> {
            vec![Candidate {
                dir: self.bin(),
                create_if_absent: false,
            }]
        }
        fn place(&self) -> Result<Placed, CliToolError> {
            place(&self.candidates(), &self.source())
        }
        /// Everything in the candidate directory, sorted. Used to prove no
        /// staging residue survives.
        fn entries(&self) -> Vec<String> {
            let mut names: Vec<String> = fs::read_dir(self.bin())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            names.sort();
            names
        }
    }

    // --- group: installing --------------------------------------------

    #[test]
    fn installing_into_an_empty_directory_creates_the_symlink() {
        let bed = Bed::new();
        assert_eq!(bed.place().unwrap(), Placed::Created { dir: bed.bin() });
        assert_eq!(fs::read_link(bed.link()).unwrap(), bed.source());
        assert_eq!(bed.entries(), vec!["openvhost".to_string()]);
    }

    /// Idempotent, and specifically **inode-stable**: a second run must not
    /// re-create the link, because a re-`rename` would break any process
    /// holding it and churn the node for no reason.
    #[test]
    fn installing_twice_changes_nothing_and_does_not_churn_the_inode() {
        let bed = Bed::new();
        bed.place().unwrap();
        let before = fs::symlink_metadata(bed.link()).unwrap();

        assert_eq!(bed.place().unwrap(), Placed::Unchanged { dir: bed.bin() });

        let after = fs::symlink_metadata(bed.link()).unwrap();
        assert_eq!(
            (before.dev(), before.ino()),
            (after.dev(), after.ino()),
            "the link was re-created when it should have been left alone"
        );
        assert_eq!(bed.entries(), vec!["openvhost".to_string()]);
    }

    /// D5's `Broken` case, repaired: the app moved, the link dangles.
    #[test]
    fn a_dangling_link_of_ours_is_repointed_at_the_current_binary() {
        let bed = Bed::new();
        let moved = bed.tmp.path().join("Old.app/Contents/MacOS/openvhost");
        symlink(&moved, bed.link()).unwrap();

        assert_eq!(bed.place().unwrap(), Placed::Repointed { dir: bed.bin() });
        assert_eq!(fs::read_link(bed.link()).unwrap(), bed.source());
        assert_eq!(bed.entries(), vec!["openvhost".to_string()]);
    }

    #[test]
    fn a_stale_link_into_another_bundle_is_repointed() {
        let bed = Bed::new();
        let older = bed.tmp.path().join("Older.app/Contents/MacOS/openvhost");
        fs::create_dir_all(older.parent().unwrap()).unwrap();
        fs::write(&older, b"old").unwrap();
        symlink(&older, bed.link()).unwrap();

        assert_eq!(bed.place().unwrap(), Placed::Repointed { dir: bed.bin() });
        assert_eq!(fs::read_link(bed.link()).unwrap(), bed.source());
    }

    #[test]
    fn a_missing_home_candidate_directory_is_created_and_used() {
        let bed = Bed::new();
        let fresh = bed.tmp.path().join("home/.local/bin");
        let placed = place(
            &[Candidate {
                dir: fresh.clone(),
                create_if_absent: true,
            }],
            &bed.source(),
        )
        .unwrap();

        assert_eq!(placed, Placed::Created { dir: fresh.clone() });
        assert!(fresh.is_dir());
        assert_eq!(
            fs::read_link(fresh.join("openvhost")).unwrap(),
            bed.source()
        );
        // The ceiling is written as a LITERAL, not as `NEW_DIR_MODE`: an
        // assertion phrased against the constant it guards moves whenever the
        // constant moves, so it cannot fail — which is exactly what the
        // vacuity pass caught in an earlier draft of this test.
        assert_eq!(NEW_DIR_MODE, 0o755, "the requested mode must stay 0755");
        let mode = fs::symlink_metadata(&fresh).unwrap().mode() & 0o777;
        assert_eq!(mode & !0o755, 0, "created wider than 0755: {mode:o}");
    }

    /// An **existing** directory's permissions are never touched — not even
    /// to "correct" them to [`NEW_DIR_MODE`]. A user who deliberately keeps
    /// `~/.local/bin` private must not have it opened up because our spec
    /// happened to name a number. (The target machine's really is 0700.)
    #[test]
    fn an_existing_candidate_directorys_permissions_are_left_alone() {
        let bed = Bed::new();
        let private = bed.tmp.path().join("private-bin");
        fs::create_dir(&private).unwrap();
        fs::set_permissions(&private, fs::Permissions::from_mode(0o700)).unwrap();

        place(
            &[Candidate {
                dir: private.clone(),
                create_if_absent: true,
            }],
            &bed.source(),
        )
        .unwrap();

        assert_eq!(
            fs::symlink_metadata(&private).unwrap().mode() & 0o777,
            0o700,
            "an existing directory's mode was widened"
        );
    }

    /// A system candidate we may not create is skipped, not conjured.
    #[test]
    fn an_absent_system_candidate_is_skipped_rather_than_created() {
        let bed = Bed::new();
        let absent = bed.tmp.path().join("usr/local/bin");
        let placed = place(
            &[
                Candidate {
                    dir: absent.clone(),
                    create_if_absent: false,
                },
                Candidate {
                    dir: bed.bin(),
                    create_if_absent: false,
                },
            ],
            &bed.source(),
        )
        .unwrap();

        assert_eq!(placed, Placed::Created { dir: bed.bin() });
        assert!(!absent.exists(), "a system directory must never be created");
    }

    /// D2's ordering, and the reason "writable" is decided by trying: the
    /// first candidate exists but cannot be written, so the second wins and
    /// the first is left exactly as it was.
    #[test]
    fn an_unwritable_earlier_candidate_is_skipped_and_left_untouched() {
        let bed = Bed::new();
        let locked = bed.tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        let placed = place(
            &[
                Candidate {
                    dir: locked.clone(),
                    create_if_absent: false,
                },
                Candidate {
                    dir: bed.bin(),
                    create_if_absent: false,
                },
            ],
            &bed.source(),
        )
        .unwrap();

        assert_eq!(placed, Placed::Created { dir: bed.bin() });
        assert_eq!(
            fs::read_dir(&locked).unwrap().count(),
            0,
            "nothing may be left in a directory we skipped"
        );
        // restore so the tempdir can be cleaned up
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn no_writable_candidate_at_all_is_an_error_that_names_what_was_tried() {
        let bed = Bed::new();
        let locked = bed.tmp.path().join("locked");
        fs::create_dir(&locked).unwrap();
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o500)).unwrap();

        let err = place(
            &[Candidate {
                dir: locked.clone(),
                create_if_absent: false,
            }],
            &bed.source(),
        )
        .unwrap_err();

        match err {
            CliToolError::NoWritableDir(tried) => {
                assert!(tried.contains(&locked.display().to_string()))
            }
            other => panic!("expected NoWritableDir, got {other:?}"),
        }
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    }

    // --- group: refusals (D3) ------------------------------------------
    //
    // These assert the occupying node is **byte-identical and inode-identical
    // afterwards**, not merely that a `Result` said no. A test that only
    // checked the return value would pass against an implementation that
    // unlinks the user's file and then fails.

    #[test]
    fn a_regular_file_is_refused_and_left_byte_identical() {
        let bed = Bed::new();
        let precious = b"#!/bin/sh\necho this is the user's own openvhost\n";
        fs::write(bed.link(), precious).unwrap();
        fs::set_permissions(bed.link(), fs::Permissions::from_mode(0o755)).unwrap();
        let before = fs::symlink_metadata(bed.link()).unwrap();

        let placed = bed.place().unwrap();

        assert_eq!(
            placed,
            Placed::Refused {
                dir: bed.bin(),
                what: "a regular file".to_string()
            }
        );
        let after = fs::symlink_metadata(bed.link()).unwrap();
        assert_eq!(fs::read(bed.link()).unwrap(), precious, "contents changed");
        assert_eq!(
            (before.dev(), before.ino()),
            (after.dev(), after.ino()),
            "the file was replaced, not preserved"
        );
        assert_eq!(before.mode(), after.mode(), "permissions changed");
        assert_eq!(bed.entries(), vec!["openvhost".to_string()], "residue left");
    }

    #[test]
    fn a_foreign_symlink_is_refused_and_still_points_where_it_did() {
        let bed = Bed::new();
        let theirs = bed.tmp.path().join("their-tool");
        fs::write(&theirs, b"theirs").unwrap();
        symlink(&theirs, bed.link()).unwrap();
        let before = fs::symlink_metadata(bed.link()).unwrap();

        let placed = bed.place().unwrap();

        match placed {
            Placed::Refused { dir, what } => {
                assert_eq!(dir, bed.bin());
                assert!(what.contains(&theirs.display().to_string()), "{what}");
            }
            other => panic!("expected Refused, got {other:?}"),
        }
        let after = fs::symlink_metadata(bed.link()).unwrap();
        assert_eq!(fs::read_link(bed.link()).unwrap(), theirs, "target changed");
        assert_eq!(
            (before.dev(), before.ino()),
            (after.dev(), after.ino()),
            "the symlink was replaced, not preserved"
        );
        assert_eq!(
            fs::read(&theirs).unwrap(),
            b"theirs",
            "the TARGET was touched"
        );
        assert_eq!(bed.entries(), vec!["openvhost".to_string()], "residue left");
    }

    #[test]
    fn a_directory_is_refused_and_its_contents_survive() {
        let bed = Bed::new();
        fs::create_dir(bed.link()).unwrap();
        fs::write(bed.link().join("keepme"), b"still here").unwrap();

        let placed = bed.place().unwrap();

        assert_eq!(
            placed,
            Placed::Refused {
                dir: bed.bin(),
                what: "a directory".to_string()
            }
        );
        assert_eq!(fs::read(bed.link().join("keepme")).unwrap(), b"still here");
        assert_eq!(bed.entries(), vec!["openvhost".to_string()], "residue left");
    }

    // --- group: no residue --------------------------------------------

    /// The staging link is removed even when the step after it fails. The
    /// failure is injected the only way the real code can hit it: `rename`
    /// onto a non-empty directory returns `ENOTDIR`.
    #[test]
    fn a_failure_between_staging_and_rename_leaves_no_residue() {
        let bed = Bed::new();
        let occupied = bed.bin().join("occupied");
        fs::create_dir(&occupied).unwrap();
        fs::write(occupied.join("child"), b"x").unwrap();

        let mut staged = StagingLink::create(&bed.bin(), &bed.source()).unwrap();
        let staged_path = staged.0.clone().unwrap();
        assert!(fs::symlink_metadata(&staged_path).is_ok(), "staging failed");

        let err = staged.rename_over(&occupied).unwrap_err();

        assert!(matches!(err, CliToolError::Io { .. }));
        assert!(
            fs::symlink_metadata(&staged_path).is_err(),
            "the staging link survived a failed rename"
        );
        assert!(occupied.is_dir(), "the destination was damaged");
        assert_eq!(bed.entries(), vec!["occupied".to_string()]);
    }

    #[test]
    fn a_staging_link_that_is_merely_dropped_removes_itself() {
        let bed = Bed::new();
        let path = {
            let staged = StagingLink::create(&bed.bin(), &bed.source()).unwrap();
            let path = staged.0.clone().unwrap();
            assert!(fs::symlink_metadata(&path).is_ok());
            path
        };
        assert!(
            fs::symlink_metadata(&path).is_err(),
            "dropping the guard must unlink the staged symlink"
        );
        assert!(bed.entries().is_empty());
    }

    /// Reentrancy: two placements racing in the same directory must both
    /// succeed and leave exactly one link and no staging files. Unique
    /// staging names are what make that true.
    #[test]
    fn two_concurrent_placements_converge_on_one_link_and_no_residue() {
        let bed = Bed::new();
        let source = bed.source();
        let dir = bed.bin();
        let results: Vec<_> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    let dir = dir.clone();
                    let source = source.clone();
                    scope.spawn(move || {
                        place(
                            &[Candidate {
                                dir,
                                create_if_absent: false,
                            }],
                            &source,
                        )
                    })
                })
                .collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });

        for result in &results {
            assert!(result.is_ok(), "a concurrent placement failed: {result:?}");
        }
        assert_eq!(bed.entries(), vec!["openvhost".to_string()]);
        assert_eq!(fs::read_link(bed.link()).unwrap(), bed.source());
    }
}
