// SPDX-License-Identifier: GPL-3.0-or-later
//! OPENVHOST_HOME resolution (master plan §3.2; spec §7.1).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Resolve the OpenVHost home directory: `OPENVHOST_HOME` env override wins,
/// otherwise `<user home>/.openvhost`. The override is what makes tests and
/// the future integration harness hermetic.
pub fn resolve_home() -> Result<PathBuf, CoreError> {
    resolve_home_from(
        std::env::var_os("OPENVHOST_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// Pure core of [`resolve_home`], testable without touching process env.
pub(crate) fn resolve_home_from(
    override_val: Option<&OsStr>,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CoreError> {
    if let Some(v) = override_val.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    home_dir
        .map(|h| h.join(".openvhost"))
        .ok_or(CoreError::HomeDirUnavailable)
}

/// Total bytes of regular files under `root`, NOT following symlinks.
///
/// **Symlinks contribute nothing and are never descended into.** The package
/// layout places a `current` link per major version at
/// `packages/<name>/<major>/current`, pointing at a sibling version directory,
/// so a link-following walk would count every installed version twice.
/// `symlink_metadata` is used rather than `entry.metadata()` — both avoid
/// traversal on unix, but the name states the intent at the call site, and that
/// intent is the entire hazard here.
///
/// A directory that cannot be read is SKIPPED rather than fatal: a partial
/// figure beats no figure in a status strip, and an unreadable subdirectory is
/// not something the user can act on from there. A `root` that does not exist
/// yields 0 for the same reason — a first run may not have provisioned it.
///
/// Iterative with an explicit stack, not recursive: a deep tree must not risk
/// the call stack.
pub(crate) fn dir_size_no_follow(root: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // unreadable or missing — skip, see the doc comment
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(md) = path.symlink_metadata() else {
                continue; // vanished mid-walk; nothing to count
            };
            let ft = md.file_type();
            if ft.is_symlink() {
                continue; // never counted, never descended
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    total
}

/// Total bytes under the resolved OpenVHost home. See [`dir_size_no_follow`] for
/// the symlink and unreadable-directory rules.
pub fn home_disk_usage() -> Result<u64, CoreError> {
    Ok(dir_size_no_follow(&resolve_home()?))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let p = resolve_home_from(
            Some(OsStr::new("/custom/openvhost-home")),
            Some(Path::new("/Users/x")),
        )
        .unwrap();
        assert_eq!(p, PathBuf::from("/custom/openvhost-home"));
    }

    #[test]
    fn defaults_to_dot_openvhost_under_home() {
        let p = resolve_home_from(None, Some(Path::new("/Users/x"))).unwrap();
        // Build expected via join so the separator is right on Windows too.
        assert_eq!(p, Path::new("/Users/x").join(".openvhost"));
    }

    #[test]
    fn empty_override_falls_back_to_default() {
        let p = resolve_home_from(Some(OsStr::new("")), Some(Path::new("/Users/x"))).unwrap();
        assert_eq!(p, Path::new("/Users/x").join(".openvhost"));
    }

    #[test]
    fn no_home_and_no_override_errors() {
        assert!(matches!(
            resolve_home_from(None, None),
            Err(CoreError::HomeDirUnavailable)
        ));
    }

    /// Files at several depths all count, and the total is exact.
    #[test]
    fn sums_regular_files_at_every_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.bin"), vec![0u8; 250]).unwrap();
        std::fs::create_dir(root.join("sub/deeper")).unwrap();
        std::fs::write(root.join("sub/deeper/c.bin"), vec![0u8; 5]).unwrap();
        assert_eq!(dir_size_no_follow(root), 355);
    }

    /// THE test this function exists for. The package layout places a `current`
    /// symlink beside each version directory
    /// (`packages/<name>/<major>/current` -> a sibling version dir), so a walk
    /// that follows links counts every installed version twice. Adding a link
    /// must not change the total by one byte.
    #[cfg(unix)]
    #[test]
    fn a_symlink_does_not_add_to_the_total() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("8.3.7")).unwrap();
        std::fs::write(root.join("8.3.7/php"), vec![0u8; 1000]).unwrap();
        let before = dir_size_no_follow(root);
        assert_eq!(before, 1000);

        // A link to the DIRECTORY — the `current` case. Following it would walk
        // 8.3.7 a second time and double the figure.
        std::os::unix::fs::symlink(root.join("8.3.7"), root.join("current")).unwrap();
        // A link to a FILE — following it would add that file's bytes again.
        std::os::unix::fs::symlink(root.join("8.3.7/php"), root.join("php-link")).unwrap();

        assert_eq!(dir_size_no_follow(root), before);
    }

    /// A directory we cannot read is skipped, not fatal: a partial figure beats
    /// no figure in a status strip. (Skipped when running as root, which can read
    /// a 0o000 directory regardless.)
    /// Our effective uid, via the owner of a file we just create: root bypasses
    /// the permission bits the next test relies on. Uses `std` only, so this crate
    /// needs no new dev-dependency for it. The probe lives in its own temp file so
    /// it cannot contribute bytes to any measured tree.
    #[cfg(unix)]
    fn running_as_root() -> bool {
        use std::os::unix::fs::MetadataExt;
        let probe = tempfile::NamedTempFile::new().unwrap();
        probe.as_file().metadata().unwrap().uid() == 0
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_skipped_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        if running_as_root() {
            return; // root can read a 0o000 directory regardless
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("readable.bin"), vec![0u8; 42]).unwrap();
        let locked = root.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("hidden.bin"), vec![0u8; 9999]).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let total = dir_size_no_follow(root);

        // Restore before the assert so the tempdir can always be cleaned up.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(total, 42, "the readable part must still be counted");
    }

    /// A path that does not exist is 0, not an error — `~/.openvhost` may not
    /// have been provisioned yet on a first run.
    #[test]
    fn a_missing_root_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(dir_size_no_follow(&tmp.path().join("nope")), 0);
    }
}
