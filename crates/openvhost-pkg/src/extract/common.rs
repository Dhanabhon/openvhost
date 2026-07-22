// SPDX-License-Identifier: GPL-3.0-or-later
//! Extraction helpers shared by both format walks (`targz`, `zip`): the
//! reject-and-log chokepoint (security audit A1), permission-mode clamping
//! (S16), and the running-total-capped copy loop (S17). Consolidated here
//! so a future fix to any of these lands in exactly one place, instead of
//! (as before) two byte-identical copies that could silently drift apart.

use std::fs;
use std::io::{self, Read};
use std::path::Path;

use super::validate::MAX_TOTAL_BYTES;
use crate::error::PkgError;

/// Build a [`PkgError::UnsafeArchive`] and log the rejection reason (S27
/// audit logging): every archive rejection across both format walks passes
/// through here, so there is exactly one place that must emit the event.
pub(crate) fn reject(msg: impl Into<String>) -> PkgError {
    let msg = msg.into();
    tracing::warn!(reason = %msg, "archive rejected");
    PkgError::UnsafeArchive(msg)
}

/// Clamp a raw archive mode to a fixed, safe pair (S16): `0o755` if any exec
/// bit is set, else `0o644`. Strips setuid/setgid/sticky and world-write
/// regardless of what the archive itself declared.
pub(crate) fn clamp_mode(mode: u32) -> u32 {
    if mode & 0o111 != 0 { 0o755 } else { 0o644 }
}

/// Copy with a running total cap over REAL decompressed bytes (S17); never
/// `read_to_end` (which would let a hostile entry allocate unbounded memory
/// before the cap could ever reject it).
pub(crate) fn copy_capped(
    reader: &mut impl Read,
    writer: &mut impl io::Write,
    mut total: u64,
) -> Result<u64, PkgError> {
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| reject(format!("read: {e}")))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > MAX_TOTAL_BYTES {
            return Err(reject("decompressed size exceeds cap"));
        }
        writer
            .write_all(&buf[..n])
            .map_err(|e| reject(format!("write: {e}")))?;
    }
    Ok(total)
}

#[cfg(unix)]
pub(crate) fn set_file_mode(p: &Path, mode: u32) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(mode))
        .map_err(|e| PkgError::io("chmod", p, e))
}
#[cfg(not(unix))]
pub(crate) fn set_file_mode(_p: &Path, _mode: u32) -> Result<(), PkgError> {
    Ok(())
}

#[cfg(unix)]
pub(crate) fn set_dir_mode(p: &Path) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o755))
        .map_err(|e| PkgError::io("chmod", p, e))
}
#[cfg(not(unix))]
pub(crate) fn set_dir_mode(_p: &Path) -> Result<(), PkgError> {
    Ok(())
}
