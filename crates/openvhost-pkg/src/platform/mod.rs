// SPDX-License-Identifier: GPL-3.0-or-later
//! Per-OS `current`-link maintenance (spec S22). Unix: atomic symlink swap
//! with a bare relative sibling target. Windows: NTFS junction (no admin
//! required) with a verified remove-then-create — NEVER a recursive delete
//! against `current`, since its blast radius is the real package payload.
//!
//! **macOS-first v1 (owner scope decision 2026-07-22):** the Windows half is
//! an explicit-error stub (see [`windows::update_current`]); the junction
//! implementation is preserved in spec §6.2 for a future Windows-enablement
//! phase and is deliberately NOT written here.
#![cfg_attr(not(test), allow(dead_code))]

use std::path::Path;

use crate::error::PkgError;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// Point `link` (…/current) at the sibling version directory `version`.
pub(crate) fn update_current(link: &Path, version: &str) -> Result<(), PkgError> {
    #[cfg(unix)]
    {
        unix::update_current(link, version)
    }
    #[cfg(windows)]
    {
        windows::update_current(link, version)
    }
}
