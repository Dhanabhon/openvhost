// SPDX-License-Identifier: GPL-3.0-or-later
//! Core error type (thiserror in library crates — master plan §5).

/// Errors produced by openvhost-core.
#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    /// The user home directory could not be determined and no
    /// `OPENVHOST_HOME` override was provided.
    #[error("could not determine the user home directory (set OPENVHOST_HOME to override)")]
    HomeDirUnavailable,
}
