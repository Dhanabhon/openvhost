// SPDX-License-Identifier: GPL-3.0-or-later
//! Unix `current`-link maintenance: an atomic symlink swap.
#![cfg_attr(not(test), allow(dead_code))]

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::error::PkgError;

/// Monotonic per-process counter making each swap's temp symlink name unique,
/// so two concurrent `update_current` calls for the same major can never share
/// a temp path (and thus can't move each other's link into place). This is
/// defense-in-depth beneath the install-level serialization (S25); it holds
/// even if that serialization is ever bypassed.
static SWAP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Atomic swap: create a temp symlink with a BARE RELATIVE sibling target
/// (e.g. `"8.4.8"`, never `"../8.4.8"` or an absolute path — this survives
/// home relocation/Time Machine restores, and `rename` over an existing
/// symlink is atomic on APFS), then rename it over `current`. If `current`
/// already exists it MUST already be a symlink — refusing to replace a real
/// file/directory is what keeps this safe from ever wiping a live install
/// (S22; the caller must NEVER reach for `remove_dir_all` here).
pub(crate) fn update_current(link: &Path, version: &str) -> Result<(), PkgError> {
    let parent = link.parent().ok_or_else(|| bad("current has no parent"))?;
    // Proceed only if `current` is absent or is itself a symlink. A stat error
    // other than NotFound is not a safe "assume absent" — surface it.
    match fs::symlink_metadata(link) {
        Ok(meta) if !meta.file_type().is_symlink() => {
            return Err(bad(
                "existing 'current' is not a symlink; refusing to replace",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(io_err("stat", link, e)),
    }
    let tmp = parent.join(format!(
        ".current.{}.{}.tmp",
        std::process::id(),
        SWAP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::os::unix::fs::symlink(version, &tmp).map_err(|e| io_err("symlink", &tmp, e))?;
    if let Err(e) = fs::rename(&tmp, link) {
        // Never leave the unique temp symlink behind on the failure path.
        let _ = fs::remove_file(&tmp);
        return Err(io_err("rename", link, e));
    }
    Ok(())
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
