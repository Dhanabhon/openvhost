// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows `current`-link maintenance — **macOS-first v1 stub**.

use std::path::Path;

use crate::error::PkgError;

/// macOS-first v1: Windows `current`-link support (an NTFS junction — design
/// preserved in spec §6.2: verify existing reparse point → `fs::remove_dir`
/// [never `remove_dir_all`, S22] → `junction::create`) is deferred to a
/// future Windows-enablement phase. This returns an explicit error rather
/// than a silent no-op, so a Windows build fails loudly at the link step
/// instead of pretending a link was created.
pub(crate) fn update_current(_link: &Path, _version: &str) -> Result<(), PkgError> {
    Err(PkgError::Unsupported(
        "current-link on Windows is not implemented in v1 (macOS-first)".to_string(),
    ))
}

/// macOS-first v1: the removal half of the same deferred capability. The
/// Windows shape is `fs::remove_dir` on the junction — **never**
/// `remove_dir_all`, whose blast radius here is the real package payload
/// (S22) — after verifying the entry is a reparse point rather than a real
/// directory. Deliberately NOT written here; an explicit error so a Windows
/// build fails loudly at the link step instead of pretending a link was
/// removed and leaving discovery reporting a broken install.
pub(crate) fn remove_current(_link: &Path) -> Result<(), PkgError> {
    Err(PkgError::Unsupported(
        "current-link on Windows is not implemented in v1 (macOS-first)".to_string(),
    ))
}
