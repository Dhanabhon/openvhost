// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows `current`-link maintenance — **macOS-first v1 stub**.
#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;

use crate::error::PkgError;

/// macOS-first v1: Windows `current`-link support (an NTFS junction — design
/// preserved in spec §6.2: verify existing reparse point → `fs::remove_dir`
/// [never `remove_dir_all`, S22] → `junction::create`) is deferred to a
/// future Windows-enablement phase. This returns an explicit error rather
/// than a silent no-op, so a Windows build fails loudly at the link step
/// instead of pretending a link was created.
pub(crate) fn update_current(_link: &Path, _version: &str) -> Result<(), PkgError> {
    Err(PkgError::UnsafeArchive(
        "current-link on Windows is not implemented in v1 (macOS-first)".to_string(),
    ))
}
