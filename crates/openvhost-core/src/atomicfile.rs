// SPDX-License-Identifier: GPL-3.0-or-later
//! Hardened atomic file write shared by the apply pipeline and the site
//! scaffold. Moved verbatim from `site/apply/commit.rs`; callers map
//! `AtomicWriteError` into their own error types.

use std::path::{Path, PathBuf};

/// A failed atomic write: which operation failed, on which path.
#[derive(Debug)]
pub(crate) struct AtomicWriteError {
    pub op: &'static str,
    pub path: PathBuf,
    pub source: std::io::Error,
}

/// Write via a temp file in the SAME directory, then rename: a rename is
/// atomic only within one filesystem, which is the whole reason the temp file
/// cannot go in `/tmp` — a target under the user's home may live on a
/// different filesystem than the system temp directory. The temp name does
/// not end in `.conf` so `plan()`'s owned-file scan never mistakes a
/// leftover for a real site config.
///
/// A5: the actual suffix is `uuid::Uuid::new_v4().simple()` — see
/// `write_atomic`. It is injected here, rather than generated inline, so a
/// test can pin a known suffix and pre-plant a symlink at the exact temp
/// path `write_atomic` would otherwise pick unpredictably; production code
/// never calls this with anything but a fresh random suffix.
pub(crate) fn write_atomic_with_suffix(
    path: &Path,
    contents: &str,
    suffix: &str,
) -> Result<(), AtomicWriteError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).map_err(|source| AtomicWriteError {
        op: "create_dir_all",
        path: parent.to_path_buf(),
        source,
    })?;
    let tmp = parent.join(format!(
        ".{}.{suffix}.tmp",
        path.file_name().unwrap_or_default().to_string_lossy()
    ));
    // `create_new` opens (and fails on) the path ITSELF rather than whatever
    // it might point to: POSIX guarantees `O_CREAT|O_EXCL` fails with EEXIST
    // on a pre-existing symlink regardless of its target, so a pre-planted
    // `.<name>.<suffix>.tmp` symlink can never be written through — unlike
    // the old `std::fs::write`, which follows symlinks and would have made
    // that an arbitrary-file-overwrite primitive. The random suffix on top
    // means the name cannot be pre-planted in the first place, and keeps two
    // concurrent applies from colliding on the same temp path.
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|source| AtomicWriteError {
            op: "create",
            path: tmp.clone(),
            source,
        })?;
    f.write_all(contents.as_bytes())
        .map_err(|source| AtomicWriteError {
            op: "write",
            path: tmp.clone(),
            source,
        })?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|source| {
        // Best-effort: the rename error is the one worth propagating, and if
        // this cleanup also fails there is nothing useful left to report.
        let _ = std::fs::remove_file(&tmp);
        AtomicWriteError {
            op: "rename",
            path: path.to_path_buf(),
            source,
        }
    })
}

pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), AtomicWriteError> {
    write_atomic_with_suffix(path, contents, &uuid::Uuid::new_v4().simple().to_string())
}
