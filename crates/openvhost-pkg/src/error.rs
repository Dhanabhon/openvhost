// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed errors for the package pipeline (thiserror — library crate).

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("invalid path component {value:?}: {reason}")]
    InvalidComponent { value: String, reason: &'static str },
    #[error("invalid url: {0}")]
    InvalidUrl(&'static str),
    #[error("sha256 must be 64 lowercase hex characters")]
    InvalidSha256,
    #[error("network error: {0}")]
    Network(String),
    #[error("download exceeded the {cap}-byte size cap")]
    TooLarge { cap: u64 },
    #[error("sha256 mismatch: expected {expected}, got {actual}")]
    HashMismatch { expected: String, actual: String },
    #[error("archive rejected: {0}")]
    UnsafeArchive(String),
    #[error("package {name} {version} is already installed")]
    AlreadyInstalled { name: String, version: String },
    #[error("io error {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("internal error: {0}")]
    Internal(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

impl PkgError {
    /// Build an [`PkgError::Io`] variant. The ONE shared constructor for
    /// this variant, used by every call site across the crate (download,
    /// layout, install, extract/targz, extract/zip, platform/unix) instead
    /// of each module keeping its own byte-identical private `io_err`
    /// helper — a future change to how I/O errors are reported only has to
    /// land here.
    pub(crate) fn io(op: &'static str, path: &std::path::Path, source: std::io::Error) -> PkgError {
        PkgError::Io {
            op,
            path: path.to_path_buf(),
            source,
        }
    }
}
