// SPDX-License-Identifier: GPL-3.0-or-later
//! Typed errors for the package pipeline (thiserror — library crate).

use std::path::PathBuf;

/// Average transfer rate in KiB/s, guarding the degenerate `elapsed == 0`
/// case (a stall detected before the clock advanced a measurable amount)
/// rather than rendering `inf`/`NaN` into a user-facing error message.
fn kib_per_sec(bytes: u64, elapsed_secs: f64) -> f64 {
    if elapsed_secs <= 0.0 {
        return 0.0;
    }
    (bytes as f64 / 1024.0) / elapsed_secs
}

#[derive(Debug, thiserror::Error)]
pub enum PkgError {
    #[error("invalid path component {value:?}: {reason}")]
    InvalidComponent { value: String, reason: &'static str },
    #[error("invalid url: {0}")]
    InvalidUrl(&'static str),
    #[error("sha256 must be 64 lowercase hex characters")]
    InvalidSha256,
    #[error("invalid warm-up binary path {value:?}: {reason}")]
    InvalidWarmupPath { value: String, reason: &'static str },
    #[error("network error: {0}")]
    Network(String),
    /// The transfer stopped making progress. Deliberately NOT
    /// [`PkgError::Network`]: a stall and a slow-but-healthy connection used
    /// to be indistinguishable to the user (a fixed whole-request wall clock
    /// turned "your link is slower than 1.5 Mbit/s" into a generic network
    /// fault), so this variant carries what actually happened — how far the
    /// download got, how fast it was going, and how long it was silent.
    #[error(
        "download stalled after {received} of {} bytes: {:.0} KiB/s over {elapsed_secs:.1}s, \
         then no data for {stall_secs:.1}s",
        match expected { Some(n) => n.to_string(), None => "unknown".to_string() },
        kib_per_sec(*received, *elapsed_secs)
    )]
    DownloadStalled {
        received: u64,
        expected: Option<u64>,
        elapsed_secs: f64,
        stall_secs: f64,
    },
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
