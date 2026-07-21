// SPDX-License-Identifier: GPL-3.0-or-later
//! Crate error type (thiserror in lib crates — master plan §5).

/// Errors produced by openvhost-proc.
#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    /// No service registered under this id.
    #[error("unknown service '{0}'")]
    NotFound(String),
    /// Underlying process/system operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
