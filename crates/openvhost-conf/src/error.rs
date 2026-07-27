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

impl ConfError {
    /// Relabel an [`ConfError::InvalidField`] with the name of the field the
    /// CALLER was parsing. Every other variant passes through untouched.
    ///
    /// This exists for exactly one shape: a newtype that guards several
    /// different fields and therefore cannot know, from its arguments alone,
    /// which one it is validating. [`crate::Seconds`] is the case —
    /// `Seconds::parse(0)` can only report `field: "seconds"`, and four
    /// separate settings (`keepalive_timeout`, `fastcgi_connect_timeout`,
    /// `fastcgi_send_timeout`, `fastcgi_read_timeout`) are all `Seconds`. Left
    /// alone, a bad `fastcgi_read_timeout` reaches the UI naming a field called
    /// "seconds", which exists on no form, so nothing gets highlighted and the
    /// user is told only that something, somewhere, is wrong.
    ///
    /// Widening `Seconds::parse` to take the field name instead would be the
    /// stronger fix — the compiler would force every call site to name its
    /// field — but it is also the only newtype with the problem, and the change
    /// would touch every construction of a `Seconds` in tests and fixtures
    /// across three crates. This is deliberately narrow: use it where one
    /// `parse` guards several distinct fields, not to paper over an error that
    /// names the wrong thing for some other reason.
    #[must_use]
    pub fn with_field(self, field: &'static str) -> Self {
        match self {
            ConfError::InvalidField { value, reason, .. } => ConfError::InvalidField {
                field,
                value,
                reason,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn with_field_renames_an_invalid_field_and_keeps_the_rest() {
        let e = ConfError::InvalidField {
            field: "seconds",
            value: "0".into(),
            reason: "must be between 1 and 86400",
        }
        .with_field("fastcgi_read_timeout");
        match e {
            ConfError::InvalidField {
                field,
                value,
                reason,
            } => {
                assert_eq!(field, "fastcgi_read_timeout");
                assert_eq!(value, "0");
                assert_eq!(reason, "must be between 1 and 86400");
            }
            other => panic!("expected InvalidField, got {other:?}"),
        }
    }

    #[test]
    fn with_field_leaves_other_variants_alone() {
        // Relabelling is only meaningful for a field-shaped error; anything
        // else must not silently acquire a field name it never had.
        let e = ConfError::EmptyUpstream.with_field("gzip_types");
        assert!(matches!(e, ConfError::EmptyUpstream), "got {e:?}");
    }
}
