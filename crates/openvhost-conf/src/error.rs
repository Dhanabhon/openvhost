// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors for config generation and validation.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfError {
    #[error("path {0} is not valid UTF-8 (cannot render into a config template)")]
    PathNotUtf8(PathBuf),
    #[error("invalid {field}: {value:?} ({reason})")]
    InvalidField {
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("php upstream TcpPorts list must not be empty")]
    EmptyUpstream,
    #[error("template render failed: {0}")]
    Render(String),
    #[error("io error {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("validator {bin} could not be launched: {source}")]
    ValidatorSpawn {
        bin: String,
        #[source]
        source: std::io::Error,
    },
    #[error("validator {bin} did not finish within {secs}s and was killed")]
    ValidatorTimeout { bin: String, secs: u64 },
}
