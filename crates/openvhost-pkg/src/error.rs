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
}
