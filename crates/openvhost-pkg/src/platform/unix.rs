// SPDX-License-Identifier: GPL-3.0-or-later
//! Unix `current`-link maintenance: an atomic symlink swap.
#![cfg_attr(not(test), allow(dead_code))]

use std::fs;
use std::path::Path;

use crate::error::PkgError;

/// Atomic swap: create a temp symlink with a BARE RELATIVE sibling target
/// (e.g. `"8.4.8"`, never `"../8.4.8"` or an absolute path — this survives
/// home relocation/Time Machine restores, and `rename` over an existing
/// symlink is atomic on APFS), then rename it over `current`. If `current`
/// already exists it MUST already be a symlink — refusing to replace a real
/// file/directory is what keeps this safe from ever wiping a live install
/// (S22; the caller must NEVER reach for `remove_dir_all` here).
pub(crate) fn update_current(link: &Path, version: &str) -> Result<(), PkgError> {
    let parent = link.parent().ok_or_else(|| bad("current has no parent"))?;
    if let Ok(meta) = fs::symlink_metadata(link)
        && !meta.file_type().is_symlink()
    {
        return Err(bad(
            "existing 'current' is not a symlink; refusing to replace",
        ));
    }
    let tmp = parent.join(".current.tmp");
    let _ = fs::remove_file(&tmp);
    std::os::unix::fs::symlink(version, &tmp).map_err(|e| io_err("symlink", &tmp, e))?;
    fs::rename(&tmp, link).map_err(|e| io_err("rename", link, e))
}

fn bad(m: &'static str) -> PkgError {
    PkgError::UnsafeArchive(m.to_string())
}

fn io_err(op: &'static str, p: &Path, e: std::io::Error) -> PkgError {
    PkgError::Io {
        op,
        path: p.to_path_buf(),
        source: e,
    }
}
