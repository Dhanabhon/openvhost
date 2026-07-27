// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated newtypes for the nginx settings the Web server page edits. Every
//! value here ends up inside a generated config file, so each one is private
//! behind a `parse` that is the *only* public constructor — the same
//! parse-don't-validate shape as `openvhost_core::site::model` (see that
//! module's docs for the rationale).
//!
//! `gzip_types` is the dangerous one: free text, arbitrarily long, rendered
//! straight into an nginx directive. Passed through unchecked, a value like
//! `text/html; } server { listen 9999; root /; } http {` becomes real
//! configuration that `nginx -t` accepts. `GzipTypes::parse` tokenises on
//! whitespace and validates each token as a MIME type shape, one at a time,
//! so a single hostile token is rejected by name rather than the whole field
//! being trusted as an opaque string.

use crate::error::ConfError;

/// Longest a single `gzip_types` token may be, in bytes. Real MIME types
/// (`application/vnd.openxmlformats-officedocument.wordprocessingml.document`
/// is 68 bytes) fit comfortably; this just keeps a single token from being
/// used to smuggle an arbitrarily long payload.
const GZIP_TYPE_TOKEN_MAX_LEN: usize = 128;

/// Most tokens `gzip_types` accepts in one setting. nginx's own default list
/// has a handful of entries; 64 is generous headroom without being
/// unbounded.
const GZIP_TYPES_MAX_TOKENS: usize = 64;

fn invalid(field: &'static str, value: impl Into<String>, reason: &'static str) -> ConfError {
    ConfError::InvalidField {
        field,
        value: value.into(),
        reason,
    }
}

/// Nginx `worker_connections`: the events-block cap on simultaneous
/// connections per worker process. `1..=65535` mirrors the range nginx's own
/// docs treat as meaningful — 0 connections is not a server, and nothing
/// past a `u16`-shaped count describes an intended limit rather than a typo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerConnections(u32);

impl WorkerConnections {
    /// Parse a worker-connections count, `1..=65535`.
    pub fn parse(v: u32) -> Result<Self, ConfError> {
        if (1..=65535).contains(&v) {
            Ok(Self(v))
        } else {
            Err(invalid(
                "worker_connections",
                v.to_string(),
                "must be between 1 and 65535",
            ))
        }
    }
    /// The validated count.
    pub fn get(&self) -> u32 {
        self.0
    }
    /// `Default`-only escape hatch: `Default` cannot fail, so it cannot go
    /// through `parse`. `pub(crate)` so nothing outside this module can use
    /// it to bypass `parse` — that is the actual validation boundary.
    pub(crate) fn new_unchecked(v: u32) -> Self {
        Self(v)
    }
}

/// A duration in seconds used by nginx's `keepalive_timeout` and the
/// `fastcgi_*_timeout` directives. `1..=86400` (one day) — 0 means "no
/// timeout" in some nginx directives and "immediate timeout" in others,
/// which is exactly the kind of directive-specific surprise this type
/// exists to avoid by simply not allowing it; a day is far beyond anything
/// a working site needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seconds(u32);

impl Seconds {
    /// Parse a timeout in seconds, `1..=86400`.
    pub fn parse(v: u32) -> Result<Self, ConfError> {
        if (1..=86400).contains(&v) {
            Ok(Self(v))
        } else {
            Err(invalid(
                "seconds",
                v.to_string(),
                "must be between 1 and 86400",
            ))
        }
    }
    /// The validated number of seconds.
    pub fn get(&self) -> u32 {
        self.0
    }
    /// `Default`-only escape hatch — see [`WorkerConnections::new_unchecked`].
    pub(crate) fn new_unchecked(v: u32) -> Self {
        Self(v)
    }
}

/// Nginx `gzip_comp_level`: the zlib compression level, `1..=9` (nginx's own
/// documented range — 1 is fastest/least compression, 9 is slowest/most).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GzipLevel(u32);

impl GzipLevel {
    /// Parse a gzip compression level, `1..=9`.
    pub fn parse(v: u32) -> Result<Self, ConfError> {
        if (1..=9).contains(&v) {
            Ok(Self(v))
        } else {
            Err(invalid(
                "gzip_comp_level",
                v.to_string(),
                "must be between 1 and 9",
            ))
        }
    }
    /// The validated compression level.
    pub fn get(&self) -> u32 {
        self.0
    }
    /// `Default`-only escape hatch — see [`WorkerConnections::new_unchecked`].
    pub(crate) fn new_unchecked(v: u32) -> Self {
        Self(v)
    }
}

/// Nginx `client_max_body_size`: a byte count with an optional `k`/`m`/`g`
/// (case-insensitive) unit suffix, matching `^\d+[kKmMgG]?$` — the exact
/// shape nginx's own size directives accept. No sign, no decimal point, no
/// trailing content: this is rendered directly as `client_max_body_size
/// {value};`, so anything past that shape is either meaningless to nginx or
/// a way to inject extra directive text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySize(String);

impl BodySize {
    /// Parse a body-size value, `^\d+[kKmMgG]?$`.
    pub fn parse(s: &str) -> Result<Self, ConfError> {
        let digits = match s.bytes().last() {
            Some(b) if b.is_ascii_alphabetic() => &s[..s.len() - 1],
            _ => s,
        };
        let suffix_ok = match s.bytes().last() {
            Some(b) if b.is_ascii_alphabetic() => {
                matches!(b, b'k' | b'K' | b'm' | b'M' | b'g' | b'G')
            }
            _ => true,
        };
        let digits_ok = !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit());
        if !digits_ok || !suffix_ok {
            return Err(invalid(
                "client_max_body_size",
                s.to_string(),
                "must match ^\\d+[kKmMgG]?$",
            ));
        }
        Ok(Self(s.to_string()))
    }
    /// The validated value as a `&str`, exactly as nginx expects it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// `Default`-only escape hatch — see [`WorkerConnections::new_unchecked`].
    pub(crate) fn new_unchecked(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Whether a token is a plausible MIME type shape: `type/subtype`, each side
/// non-empty and built only from characters MIME type/subtype names actually
/// use (`RFC 6838`-ish: alphanumerics, `-`, `.`, `+`, `_`). No whitespace, no
/// `;`, no `{`/`}`/`"`/`$` — the characters that would let a token close the
/// directive and open new configuration.
fn is_mime_shaped(token: &str) -> bool {
    let Some((ty, subtype)) = token.split_once('/') else {
        return false;
    };
    let is_token_chars = |s: &str| {
        !s.is_empty()
            && s.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'+' | b'_'))
    };
    is_token_chars(ty) && is_token_chars(subtype)
}

/// Nginx `gzip_types`: a whitespace-separated list of MIME types. This is
/// the field this whole layer exists to guard — see the module docs. Each
/// token is checked individually against [`is_mime_shaped`], so a malformed
/// entry is rejected by its own content rather than the field being trusted
/// wholesale.
///
/// `GzipTypes::parse("   ")` (whitespace only) succeeds with an empty list.
/// That is deliberate: it means "compress nothing beyond nginx's own
/// built-in `text/html`", which is nginx's own behaviour with no
/// `gzip_types` directive at all — the honest way to express "no extra
/// types", not an oversight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GzipTypes(Vec<String>);

impl GzipTypes {
    /// Parse a whitespace-separated `gzip_types` list. At most
    /// [`GZIP_TYPES_MAX_TOKENS`] tokens, each at most
    /// [`GZIP_TYPE_TOKEN_MAX_LEN`] bytes and MIME-shaped.
    pub fn parse(s: &str) -> Result<Self, ConfError> {
        let mut types = Vec::new();
        for token in s.split_whitespace() {
            if token.len() > GZIP_TYPE_TOKEN_MAX_LEN {
                return Err(invalid(
                    "gzip_types",
                    token.to_string(),
                    "token exceeds 128 bytes",
                ));
            }
            if !is_mime_shaped(token) {
                return Err(invalid(
                    "gzip_types",
                    token.to_string(),
                    "each token must look like a MIME type (type/subtype)",
                ));
            }
            types.push(token.to_string());
        }
        if types.len() > GZIP_TYPES_MAX_TOKENS {
            return Err(invalid(
                "gzip_types",
                s.to_string(),
                "at most 64 types are allowed",
            ));
        }
        Ok(Self(types))
    }
    /// The validated list, space-joined exactly as it belongs on the
    /// `gzip_types` directive line (without the directive keyword or the
    /// trailing `;` — the caller composes those).
    pub fn as_directive(&self) -> String {
        self.0.join(" ")
    }
    /// `Default`-only escape hatch — see [`WorkerConnections::new_unchecked`].
    /// Splits on whitespace the same way `parse` does, so `as_directive`
    /// behaves identically whichever constructor built the value.
    pub(crate) fn new_unchecked(s: &str) -> Self {
        Self(s.split_whitespace().map(str::to_string).collect())
    }
}

/// A boolean nginx directive value (`on` / `off`), e.g. `tcp_nodelay` and
/// `gzip`. Unlike the other newtypes this one cannot fail to parse — a
/// `bool` is already a valid value — so it has no `parse`, only `new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OnOff(bool);

impl OnOff {
    /// Build a directive value from a plain bool.
    pub fn new(on: bool) -> Self {
        Self(on)
    }
    /// `"on"` or `"off"`, exactly as nginx expects.
    pub fn as_str(&self) -> &'static str {
        if self.0 { "on" } else { "off" }
    }
    /// Whether this is `on`.
    pub fn is_on(&self) -> bool {
        self.0
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::settings::WebServerSettings;

    #[test]
    fn worker_connections_accepts_bounds_rejects_outside() {
        assert!(WorkerConnections::parse(1).is_ok());
        assert!(WorkerConnections::parse(65535).is_ok());
        assert!(WorkerConnections::parse(0).is_err());
        assert!(WorkerConnections::parse(65536).is_err());
    }

    #[test]
    fn seconds_accepts_bounds_rejects_outside() {
        assert!(Seconds::parse(1).is_ok());
        assert!(Seconds::parse(86400).is_ok());
        assert!(Seconds::parse(0).is_err());
        assert!(Seconds::parse(86401).is_err());
    }

    #[test]
    fn gzip_level_accepts_bounds_rejects_outside() {
        assert!(GzipLevel::parse(1).is_ok());
        assert!(GzipLevel::parse(9).is_ok());
        assert!(GzipLevel::parse(0).is_err());
        assert!(GzipLevel::parse(10).is_err());
        assert!(GzipLevel::parse(99).is_err());
    }

    #[test]
    fn body_size_accepts_digits_with_optional_unit_rejects_the_rest() {
        for good in ["0", "256", "256m", "256M", "1g", "1G", "10k", "10K"] {
            assert!(BodySize::parse(good).is_ok(), "should accept {good:?}");
        }
        for bad in ["", "m", "256mb", "-1", "1.5m", "256 m", "256;m", "256m;"] {
            assert!(BodySize::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn gzip_types_accepts_a_mime_shaped_list_and_blank_means_empty() {
        let t = GzipTypes::parse("text/plain text/css application/json").unwrap();
        assert_eq!(t.as_directive(), "text/plain text/css application/json");

        // Deliberate: whitespace-only means "no extra types", not an error.
        let empty = GzipTypes::parse("   ").unwrap();
        assert_eq!(empty.as_directive(), "");
    }

    #[test]
    fn gzip_types_refuses_a_value_that_would_become_a_directive() {
        // The injection scenario this whole layer exists to stop: passed
        // through unchecked, this string becomes real nginx configuration
        // that `nginx -t` accepts.
        let e =
            GzipTypes::parse("text/html; } server { listen 9999; root /; } http {").unwrap_err();
        match e {
            ConfError::InvalidField { field, value, .. } => {
                assert_eq!(field, "gzip_types");
                assert!(value.contains("text/html;"), "got {value:?}");
            }
            other => panic!("expected InvalidField, got {other:?}"),
        }
    }

    #[test]
    fn gzip_types_enforces_token_and_count_limits() {
        let overlong_token = format!("text/{}", "a".repeat(GZIP_TYPE_TOKEN_MAX_LEN));
        assert!(GzipTypes::parse(&overlong_token).is_err());

        let too_many = (0..GZIP_TYPES_MAX_TOKENS + 1)
            .map(|i| format!("text/t{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(GzipTypes::parse(&too_many).is_err());

        let exactly_max = (0..GZIP_TYPES_MAX_TOKENS)
            .map(|i| format!("text/t{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        assert!(GzipTypes::parse(&exactly_max).is_ok());
    }

    #[test]
    fn on_off_roundtrips() {
        assert_eq!(OnOff::new(true).as_str(), "on");
        assert!(OnOff::new(true).is_on());
        assert_eq!(OnOff::new(false).as_str(), "off");
        assert!(!OnOff::new(false).is_on());
    }

    #[test]
    fn every_default_would_survive_its_own_parser() {
        // Default bypasses `parse` (it cannot fail), so this is what stops a
        // default drifting outside the bounds the UI enforces — a value the user
        // could never type but the app ships with.
        let d = WebServerSettings::default();
        assert!(WorkerConnections::parse(d.worker_connections.get()).is_ok());
        assert!(Seconds::parse(d.keepalive_timeout.get()).is_ok());
        assert!(Seconds::parse(d.fastcgi_connect_timeout.get()).is_ok());
        assert!(Seconds::parse(d.fastcgi_send_timeout.get()).is_ok());
        assert!(Seconds::parse(d.fastcgi_read_timeout.get()).is_ok());
        assert!(GzipLevel::parse(d.gzip_comp_level.get()).is_ok());
        assert!(BodySize::parse(d.client_max_body_size.as_str()).is_ok());
        assert!(GzipTypes::parse(&d.gzip_types.as_directive()).is_ok());
    }
}
