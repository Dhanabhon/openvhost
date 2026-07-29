// SPDX-License-Identifier: GPL-3.0-or-later
//! Datadir lifecycle: where MySQL state lives on disk ([`mysql_paths`]),
//! what state a datadir is actually in — read from disk, never a stored
//! boolean (see [`classify_datadir`]) — and cleanup of abandoned staged-init
//! directories ([`sweep_stale_staging`]). See spec D2:
//! `docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md`.

use std::io;
use std::path::{Path, PathBuf};

use crate::error::CoreError;
use crate::site::apply::MAX_SOCKET_PATH_BYTES;

use super::MysqlMajor;

/// Every generated/state path for one MySQL major, all derived from the
/// resolved OpenVHost home + a [`MysqlMajor`].
///
/// CONFINEMENT ARGUMENT (spec D2, the Docroot lesson: a newtype's shape
/// guard is confinement, not policy — state it explicitly rather than
/// re-learn it): every field below is `home.join(...)...join(major.as_str())`
/// — never a path supplied by an untrusted caller. `home` comes only from
/// [`crate::resolve_home`] (an env override or the OS user-home lookup —
/// never IPC input), and `major.as_str()` can only ever be ASCII digits and
/// a single `.` (enforced identically by both of [`MysqlMajor`]'s
/// constructors — see its doc comment), so it can never contain a path
/// separator or `..`. Nothing about *where* these paths point is steerable
/// from outside this process, regardless of whether `major` came from the
/// strict, catalogue-checked `parse` or the discovery-only `from_probe`.
/// Whether `major` is allowed to be installed/initialized is a separate,
/// orthogonal policy question (`MysqlMajor::is_cataloged`), decided by the
/// caller — never by path derivation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MysqlPaths {
    /// `<home>/data/mysql/<major>/` — the live datadir once initialized.
    pub datadir: PathBuf,
    /// `<home>/config/generated/mysql/<major>/my.cnf`.
    pub my_cnf: PathBuf,
    /// `<home>/run/mysql-<major>.sock` — the running server's socket.
    pub socket: PathBuf,
    /// `<home>/run/mysql-<major>-init.sock` — the network-less temp server
    /// used only during staged init (spec D2 step 2).
    pub init_socket: PathBuf,
    /// `<home>/config/custom/mysql/<major>/conf.d` — the user's own
    /// `!includedir`, never written by this app.
    pub custom_confd: PathBuf,
    /// `<home>/data/mysql/` — parent of both `datadir` and every staging
    /// directory for every major, so the finishing `rename` at the end of
    /// init is atomic (same filesystem, same parent). Also the argument
    /// [`sweep_stale_staging`] expects.
    pub staging_parent: PathBuf,
}

impl MysqlPaths {
    /// Guards [`Self::socket`] and [`Self::init_socket`] against Darwin's
    /// `sun_path` ceiling. Reuses the exact constant
    /// ([`crate::site::apply::MAX_SOCKET_PATH_BYTES`]) and error variant
    /// (`CoreError::SocketPathTooLong`) `site::apply::socket_path` uses for
    /// php-fpm's socket — the same 104-byte `sun_path` limit applies to
    /// every unix socket this app binds, mysqld's included, so this reuses
    /// rather than reinvents that guard. [`mysql_paths`] itself never fails
    /// (pure path joining); this is the explicit, separate check callers
    /// run before acting on either socket path.
    pub fn check_socket_lengths(&self) -> Result<(), CoreError> {
        guard_socket_path(&self.socket)?;
        guard_socket_path(&self.init_socket)
    }
}

fn guard_socket_path(path: &Path) -> Result<(), CoreError> {
    let len = path.as_os_str().as_encoded_bytes().len();
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(CoreError::SocketPathTooLong {
            path: path.to_path_buf(),
            len,
        });
    }
    Ok(())
}

/// Derive every path this major needs from `home` + `major`. Pure and
/// infallible (see the CONFINEMENT ARGUMENT on [`MysqlPaths`]) — see
/// [`MysqlPaths::check_socket_lengths`] for the guard callers must run
/// before using either socket path.
pub fn mysql_paths(home: &Path, major: &MysqlMajor) -> MysqlPaths {
    let data_root = home.join("data").join("mysql");
    let major_str = major.as_str();
    MysqlPaths {
        datadir: data_root.join(major_str),
        my_cnf: home
            .join("config")
            .join("generated")
            .join("mysql")
            .join(major_str)
            .join("my.cnf"),
        socket: home.join("run").join(format!("mysql-{major_str}.sock")),
        init_socket: home
            .join("run")
            .join(format!("mysql-{major_str}-init.sock")),
        custom_confd: home
            .join("config")
            .join("custom")
            .join("mysql")
            .join(major_str)
            .join("conf.d"),
        staging_parent: data_root,
    }
}

/// Directory entries that mark an already-`--initialize`d MySQL datadir: the
/// system schema directory and MySQL's own bootstrap marker file. Both must
/// be present — either alone is not enough evidence (a half-copied backup
/// could easily have just one).
const SENTINEL_DIR: &str = "mysql";
const SENTINEL_FILE: &str = "auto.cnf";

/// What a datadir directory actually contains, established the ONE way spec
/// D2 allows: by reading the filesystem. Never a state.db boolean — a
/// restored or hand-copied datadir must classify correctly even though
/// state.db has never heard of it, which is exactly the "genuinely ready"
/// property a boolean cannot provide (this codebase has already hit that
/// bug class once, for service status).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatadirState {
    /// Missing, or present but empty: safe to `--initialize` into.
    NotInitialized,
    /// Both sentinels are present: a real, already-initialized datadir.
    Initialized,
    /// Present, non-empty, and NOT recognizably a MySQL datadir. Rendered
    /// honestly to the user (spec D2) — never adopted, never deleted, never
    /// initialized into.
    Foreign { detail: String },
}

/// Classify `dir` by reading it — see [`DatadirState`]. A missing directory
/// is [`DatadirState::NotInitialized`], not an error: the datadir may not
/// have been provisioned yet (mirrors `home::dir_size_no_follow`'s identical
/// "missing root is not fatal" convention).
pub fn classify_datadir(dir: &Path) -> io::Result<DatadirState> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(DatadirState::NotInitialized),
        Err(e) => return Err(e),
    };

    let mut names: Vec<String> = Vec::new();
    let mut has_sentinel_dir = false;
    let mut has_sentinel_file = false;
    for entry in entries {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let file_type = entry.file_type()?;
        if name == SENTINEL_DIR && file_type.is_dir() {
            has_sentinel_dir = true;
        } else if name == SENTINEL_FILE && file_type.is_file() {
            has_sentinel_file = true;
        }
        names.push(name);
    }

    if names.is_empty() {
        return Ok(DatadirState::NotInitialized);
    }
    if has_sentinel_dir && has_sentinel_file {
        return Ok(DatadirState::Initialized);
    }

    names.sort();
    Ok(DatadirState::Foreign {
        detail: format!(
            "{} does not look like a MySQL datadir (missing {SENTINEL_DIR}/ and/or \
             {SENTINEL_FILE}; found: {})",
            dir.display(),
            names.join(", ")
        ),
    })
}

/// Staging directories look like `.<major>.init-<uuid>` (spec D2) — a
/// leading dot (hidden), a major-shaped prefix, then the literal `.init-`,
/// then a non-empty suffix. Reuses [`MysqlMajor::from_probe`] for the
/// "major-shaped" half of that check rather than re-implementing the shape
/// regex a second time — and deliberately does NOT go through
/// [`MysqlMajor::parse`]: a stale leftover for a major this build no longer
/// offers must still be recognized and swept.
fn is_stale_staging_name(name: &str) -> bool {
    let Some(rest) = name.strip_prefix('.') else {
        return false;
    };
    let Some(marker) = rest.find(".init-") else {
        return false;
    };
    let (major_part, suffix) = (&rest[..marker], &rest[marker + ".init-".len()..]);
    !suffix.is_empty() && MysqlMajor::from_probe(major_part.to_string()).is_some()
}

/// Remove abandoned staging directories directly under `parent` (spec D2:
/// `.<major>.init-<uuid>`), returning what was removed. Meant to run on
/// rescan — a crash or force-quit mid-init leaves one of these behind. The
/// FINAL datadir (bare `<major>`, no leading dot) is a completely different
/// name shape and is never touched here.
///
/// Only removes entries that are BOTH (a) directories and (b) name-shaped
/// like a staging directory (see `is_stale_staging_name`); anything else
/// in `parent` — an unrelated hidden directory, a final datadir, or even a
/// symlink that happens to share the name shape — is left exactly alone.
/// `entry.file_type()` does not follow symlinks (unlike `entry.metadata()`
/// on some platforms), so a symlink squatting on the pattern is reported as
/// a symlink, not a directory, and is skipped — the same "never follow a
/// link into an unintended traversal" discipline `home::dir_size_no_follow`
/// documents for the `packages/.../current` link.
///
/// A missing `parent` is not an error: nothing has been provisioned yet.
pub fn sweep_stale_staging(parent: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match std::fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    let mut removed = Vec::new();
    for entry in entries {
        let entry = entry?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue; // non-UTF-8 name: can never match our ASCII pattern
        };
        if !is_stale_staging_name(&name) {
            continue;
        }
        if !entry.file_type()?.is_dir() {
            continue; // never a symlink, never a stray same-named file
        }
        let path = entry.path();
        std::fs::remove_dir_all(&path)?;
        removed.push(path);
    }
    removed.sort();
    Ok(removed)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- classify_datadir ----

    #[test]
    fn a_missing_directory_is_not_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("does-not-exist");
        assert_eq!(
            classify_datadir(&dir).unwrap(),
            DatadirState::NotInitialized
        );
    }

    #[test]
    fn an_empty_directory_is_not_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(
            classify_datadir(tmp.path()).unwrap(),
            DatadirState::NotInitialized
        );
    }

    #[test]
    fn both_sentinels_present_means_initialized() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("mysql")).unwrap();
        std::fs::write(tmp.path().join("auto.cnf"), b"[auto]\n").unwrap();
        // A real datadir has many more files; classification must not need
        // an exhaustive match, only both sentinels present.
        std::fs::write(tmp.path().join("ibdata1"), b"").unwrap();
        assert_eq!(
            classify_datadir(tmp.path()).unwrap(),
            DatadirState::Initialized
        );
    }

    #[test]
    fn only_the_sentinel_dir_without_auto_cnf_is_foreign() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("mysql")).unwrap();
        match classify_datadir(tmp.path()).unwrap() {
            DatadirState::Foreign { .. } => {}
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn only_auto_cnf_without_the_sentinel_dir_is_foreign() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("auto.cnf"), b"[auto]\n").unwrap();
        match classify_datadir(tmp.path()).unwrap() {
            DatadirState::Foreign { .. } => {}
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    #[test]
    fn a_stray_file_is_foreign_and_names_the_offender() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("some-note.txt"), b"hi").unwrap();
        match classify_datadir(tmp.path()).unwrap() {
            DatadirState::Foreign { detail } => {
                assert!(detail.contains("some-note.txt"), "detail: {detail}");
            }
            other => panic!("expected Foreign, got {other:?}"),
        }
    }

    // ---- sweep_stale_staging ----

    #[test]
    fn removes_a_stale_staging_directory_and_reports_it() {
        let tmp = tempfile::tempdir().unwrap();
        let staging = tmp.path().join(".8.4.init-a1b2c3d4e5f6");
        std::fs::create_dir(&staging).unwrap();
        std::fs::write(staging.join("leftover"), b"x").unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert_eq!(removed, vec![staging.clone()]);
        assert!(
            !staging.exists(),
            "staging directory should have been removed"
        );
    }

    #[test]
    fn a_non_staging_name_is_never_removed() {
        // Vacuity check named in the brief: point sweep at a directory whose
        // name is NOT staging-shaped and confirm it survives untouched.
        let tmp = tempfile::tempdir().unwrap();
        let innocent = tmp.path().join("not-a-staging-dir");
        std::fs::create_dir(&innocent).unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert!(removed.is_empty(), "got {removed:?}");
        assert!(
            innocent.exists(),
            "an unrelated directory must never be removed"
        );
    }

    #[test]
    fn a_finished_datadir_without_the_leading_dot_is_never_removed() {
        // The FINAL datadir is named bare `<major>` (no leading dot) — must
        // never be mistaken for a staging leftover.
        let tmp = tempfile::tempdir().unwrap();
        let finished = tmp.path().join("8.4");
        std::fs::create_dir(&finished).unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert!(removed.is_empty(), "got {removed:?}");
        assert!(finished.exists());
    }

    #[test]
    fn a_missing_parent_is_not_an_error() {
        let tmp = tempfile::tempdir().unwrap();
        let missing = tmp.path().join("nope");
        assert_eq!(
            sweep_stale_staging(&missing).unwrap(),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn several_stale_dirs_are_all_removed_and_returned_sorted() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join(".8.4.init-aaaa");
        let b = tmp.path().join(".8.4.init-bbbb");
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert_eq!(removed, vec![a, b]); // lexicographic: "aaaa" < "bbbb"
    }

    #[cfg(unix)]
    #[test]
    fn a_symlink_named_like_a_staging_directory_is_never_removed() {
        // file_type() does not follow symlinks (the home.rs `current`-link
        // lesson) — a symlink squatting on the staging name pattern must
        // survive, even if it points at a real directory.
        let tmp = tempfile::tempdir().unwrap();
        let real_dir = tmp.path().join("real-target");
        std::fs::create_dir(&real_dir).unwrap();
        let squatter = tmp.path().join(".8.4.init-deadbeef");
        std::os::unix::fs::symlink(&real_dir, &squatter).unwrap();

        let removed = sweep_stale_staging(tmp.path()).unwrap();

        assert!(removed.is_empty(), "got {removed:?}");
        assert!(squatter.exists(), "the symlink itself must survive");
        assert!(real_dir.exists(), "and its target must survive too");
    }

    // ---- mysql_paths + socket length guard ----

    #[test]
    fn mysql_paths_derives_every_path_under_home() {
        let home = PathBuf::from("/tmp/ovh");
        let major = MysqlMajor::parse("8.4").unwrap();
        let paths = mysql_paths(&home, &major);

        assert_eq!(paths.datadir, PathBuf::from("/tmp/ovh/data/mysql/8.4"));
        assert_eq!(
            paths.my_cnf,
            PathBuf::from("/tmp/ovh/config/generated/mysql/8.4/my.cnf")
        );
        assert_eq!(paths.socket, PathBuf::from("/tmp/ovh/run/mysql-8.4.sock"));
        assert_eq!(
            paths.init_socket,
            PathBuf::from("/tmp/ovh/run/mysql-8.4-init.sock")
        );
        assert_eq!(
            paths.custom_confd,
            PathBuf::from("/tmp/ovh/config/custom/mysql/8.4/conf.d")
        );
        assert_eq!(paths.staging_parent, PathBuf::from("/tmp/ovh/data/mysql"));
    }

    #[test]
    fn staging_parent_is_the_direct_parent_of_datadir() {
        // D2: staging lives in the SAME parent as the final datadir so the
        // finishing rename is atomic (same filesystem, same directory).
        let home = PathBuf::from("/tmp/ovh");
        let major = MysqlMajor::parse("8.4").unwrap();
        let paths = mysql_paths(&home, &major);
        assert_eq!(paths.datadir.parent(), Some(paths.staging_parent.as_path()));
    }

    #[test]
    fn short_home_passes_the_socket_length_guard() {
        let major = MysqlMajor::parse("8.4").unwrap();
        let paths = mysql_paths(&PathBuf::from("/tmp/ovh"), &major);
        assert!(paths.check_socket_lengths().is_ok());
    }

    #[test]
    fn a_home_too_deep_for_the_socket_is_refused() {
        // Mirrors site::apply's identical guard test for php-fpm's socket —
        // same constant, same error, reused rather than reinvented.
        let deep_home = PathBuf::from(format!("/tmp/{}", "d".repeat(120)));
        let major = MysqlMajor::parse("8.4").unwrap();
        let paths = mysql_paths(&deep_home, &major);
        let err = paths.check_socket_lengths().unwrap_err();
        assert!(matches!(err, CoreError::SocketPathTooLong { .. }));
    }
}
