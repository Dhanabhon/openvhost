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
    /// The OpenVHost home is not valid UTF-8, so it cannot be written into
    /// text configs faithfully.
    #[error("openvhost home {} is not valid UTF-8", path.display())]
    HomeNotUtf8 { path: PathBuf },
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
}
