// SPDX-License-Identifier: GPL-3.0-or-later
//! Core error type (thiserror in library crates — master plan §5).

use std::path::PathBuf;

/// Errors produced by openvhost-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The user home directory could not be determined and no
    /// `OPENVHOST_HOME` override was provided.
    #[error("could not determine the user home directory (set OPENVHOST_HOME to override)")]
    HomeDirUnavailable,
    /// A filesystem operation failed while provisioning.
    #[error("provision: {op} {}: {source}", path.display())]
    ProvisionIo {
        op: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// The php-fpm unix socket path would exceed Darwin's 104-byte
    /// `sun_path`. php-fpm does NOT reject longer paths — it warns, silently
    /// truncates, and binds the wrong path while nginx 502s forever
    /// (specialist-proven). Refuse early instead.
    #[error("socket path {} is {len} bytes (max 103); use a shorter OPENVHOST_HOME", path.display())]
    SocketPathTooLong { path: PathBuf, len: usize },
    /// A database operation failed.
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),
    /// A domain value failed validation at the boundary (parse-don't-validate).
    #[error("invalid {field}: {reason}")]
    Validation { field: &'static str, reason: String },
    /// A filesystem write outside the provisioning path (which already has
    /// its own [`CoreError::ProvisionIo`]) and outside the site-apply plan
    /// (which has its own `site::apply::ApplyError::Io`). Added for the
    /// MySQL init sequence's `my.cnf` write (P1 MySQL lifecycle design, spec
    /// D5: "written with `atomicfile::write_atomic` as a `GeneratedFile`"),
    /// which needs the SAME hardened atomic write `site::apply::commit`
    /// already uses but has no `ApplyPlan` of its own to go through — see
    /// `crate::mysql::write_generated_config`.
    #[error("{op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
}

/// Maps the crate-shared hardened atomic write's error (`crate::atomicfile`)
/// into [`CoreError::Io`] — mirrors `site::apply::error`'s identical
/// `From<AtomicWriteError> for ApplyError` (a manual impl, not `#[from]`,
/// because the fields are remapped, not wrapped as-is).
impl From<crate::atomicfile::AtomicWriteError> for CoreError {
    fn from(e: crate::atomicfile::AtomicWriteError) -> Self {
        CoreError::Io {
            op: e.op,
            path: e.path,
            source: e.source,
        }
    }
}
