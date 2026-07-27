// SPDX-License-Identifier: GPL-3.0-or-later
//! Validated newtypes for the nginx settings the Web server page edits. Every
//! value here ends up inside a generated config file, so each one is `pub`
//! behind a `parse` that is the *only public* constructor — the same
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
//!
//! There is exactly one way to build a value here without going through
//! `parse`: [`unchecked_defaults`], which builds a whole
//! [`super::WebServerSettings`] from literals in one place, for
//! `Default::default` alone. It is `pub(super)` — visible only to the
//! `settings` module that declares `mod value;` — not `pub(crate)`. A
//! sibling module elsewhere in this crate (such as the template renderer in
//! `crate::webserver`) has no path to it and cannot construct an
//! out-of-range value.

use crate::error::ConfError;

/// Development-appropriate default `gzip_types` list — a handful of common
/// compressible text formats, not nginx's own (empty) default. See
/// [`unchecked_defaults`] and [`super::WebServerSettings::default`] for why a
/// development-appropriate default is safe to choose here.
const DEFAULT_GZIP_TYPES: &str = "text/plain text/css application/json application/javascript application/xml image/svg+xml font/woff2";

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
}

/// Whether a token (already lowercased by the caller) is a plausible MIME
/// type shape: `type/subtype`, each side matching spec §6's
/// `^[a-z0-9][a-z0-9.+-]*/[a-z0-9][a-z0-9.+-]*$` — a leading letter or digit,
/// then any run of lowercase alphanumerics plus `-`, `.`, `+`. Deliberately
/// narrower than RFC 6838's full token charset: no `_`, no leading `-`/`.`/
/// `+`, no uppercase. Lowercase-only matches what nginx configs
/// conventionally contain and what every real compressible type looks like
/// (`application/vnd.ms-fontobject`, `text/x-component`,
/// `image/svg+xml`), and a narrower guard is the right default for a value
/// that lands in a config file. No whitespace, no `;`, no `{`/`}`/`"`/`$` —
/// the characters that would let a token close the directive and open new
/// configuration.
fn is_mime_shaped(token: &str) -> bool {
    let Some((ty, subtype)) = token.split_once('/') else {
        return false;
    };
    let is_token_chars = |s: &str| {
        let mut bytes = s.bytes();
        let Some(first) = bytes.next() else {
            return false;
        };
        (first.is_ascii_lowercase() || first.is_ascii_digit())
            && bytes.all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'+')
            })
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
///
/// **Case:** MIME type/subtype tokens are case-insensitive by spec, and the
/// charset this type enforces is lowercase-only (see [`is_mime_shaped`]), so
/// `parse` lowercases each token *before* checking its shape. `TEXT/HTML`
/// therefore parses successfully and is stored, and later rendered by
/// [`GzipTypes::as_directive`], as `text/html` — rejecting a valid-but-
/// uppercase MIME type with a bare "invalid" message would be a bad
/// experience for input that is not actually wrong. A token that is
/// malformed for reasons other than case (stray punctuation, an
/// underscore, a leading `-`) is still rejected, naming the original token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GzipTypes(Vec<String>);

impl GzipTypes {
    /// Parse a whitespace-separated `gzip_types` list. At most
    /// [`GZIP_TYPES_MAX_TOKENS`] tokens, each at most
    /// [`GZIP_TYPE_TOKEN_MAX_LEN`] bytes and MIME-shaped (case-insensitively
    /// — see the type docs).
    ///
    /// The token-count cap is enforced *inside* the loop, not after
    /// collecting every token into a `Vec` first: `gzip_types` is free text
    /// with no upper length limit before this function sees it, so an input
    /// with millions of whitespace-separated tokens must be rejected after
    /// looking at [`GZIP_TYPES_MAX_TOKENS`] of them, not after allocating a
    /// `String` for every single one.
    pub fn parse(s: &str) -> Result<Self, ConfError> {
        let mut types = Vec::new();
        for token in s.split_whitespace() {
            if types.len() >= GZIP_TYPES_MAX_TOKENS {
                return Err(invalid(
                    "gzip_types",
                    s.to_string(),
                    "at most 64 types are allowed",
                ));
            }
            if token.len() > GZIP_TYPE_TOKEN_MAX_LEN {
                return Err(invalid(
                    "gzip_types",
                    token.to_string(),
                    "token exceeds 128 bytes",
                ));
            }
            let lowered = token.to_ascii_lowercase();
            if !is_mime_shaped(&lowered) {
                return Err(invalid(
                    "gzip_types",
                    token.to_string(),
                    "each token must look like a MIME type (type/subtype), using only a-z, 0-9, '.', '+', '-'",
                ));
            }
            types.push(lowered);
        }
        Ok(Self(types))
    }
    /// The validated list, space-joined exactly as it belongs on the
    /// `gzip_types` directive line (without the directive keyword or the
    /// trailing `;` — the caller composes those).
    pub fn as_directive(&self) -> String {
        self.0.join(" ")
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

/// The one bypass of `parse` in this crate. `Default` cannot fail, so
/// [`super::WebServerSettings::default`] cannot go through `parse` for any
/// of its fields — it calls this instead of performing one type-level
/// bypass per field. Every constant below is a raw literal from spec §5,
/// built directly (each newtype's inner field is private to this file, so
/// only code in this module can construct one without going through
/// `parse`); [`super::WebServerSettings::default`] is the only call site.
///
/// `pub(super)`: visible to `settings` — the module that defines
/// `WebServerSettings` and needs this to implement `Default` — and to
/// nothing else. That is the boundary the compiler actually enforces: a
/// sibling module in this crate (`crate::webserver`, which is where the
/// template renderer lives) cannot see this function, cannot call it, and
/// has no way to build an out-of-range value. `every_default_would_survive_
/// its_own_parser` is what keeps these constants honest.
pub(super) fn unchecked_defaults() -> super::WebServerSettings {
    super::WebServerSettings {
        worker_connections: WorkerConnections(1024),
        client_max_body_size: BodySize("256m".to_string()),
        keepalive_timeout: Seconds(65),
        tcp_nodelay: OnOff(true),
        fastcgi_connect_timeout: Seconds(60),
        fastcgi_send_timeout: Seconds(300),
        fastcgi_read_timeout: Seconds(300),
        gzip: OnOff(false),
        gzip_comp_level: GzipLevel(1),
        gzip_types: GzipTypes(
            DEFAULT_GZIP_TYPES
                .split_whitespace()
                .map(str::to_string)
                .collect(),
        ),
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
    fn body_size_handles_the_edge_cases_traced_by_review() {
        // A leading sign is not part of `\d+` — nginx's own directive has no
        // notion of a signed size.
        assert!(
            BodySize::parse("+256m").is_err(),
            "a leading '+' is not part of \\d+"
        );
        // Leading zeros are still a run of digits — `\d+` does not forbid them.
        assert!(
            BodySize::parse("007m").is_ok(),
            "leading zeros are still \\d+"
        );
        // `٣` is Arabic-Indic digit three (U+0663) — a Unicode decimal digit,
        // but not one of the ASCII bytes '0'..='9' the parser checks for.
        assert!(
            BodySize::parse("٣m").is_err(),
            "a non-ASCII Unicode digit is not ASCII \\d"
        );
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
    fn gzip_types_lowercases_a_valid_but_uppercase_token_instead_of_rejecting_it() {
        // TEXT/HTML is a perfectly valid MIME type spelled in the wrong case
        // for our charset. Rejecting it with a bare "invalid" message would
        // be a bad experience for input that isn't actually wrong, so `parse`
        // lowercases before validating and stores the lowercased form.
        let t = GzipTypes::parse("TEXT/HTML").unwrap();
        assert_eq!(t.as_directive(), "text/html");
    }

    #[test]
    fn is_mime_shaped_rejects_what_the_old_alphanumeric_charset_wrongly_accepted() {
        // Spec §6 fixes the per-token rule as
        // `^[a-z0-9][a-z0-9.+-]*/[a-z0-9][a-z0-9.+-]*$` — lowercase only, no
        // underscore, no leading punctuation. These three are exactly the
        // cases a `char::is_ascii_alphanumeric() || '-'|'.'|'+'|'_'` charset
        // would have let through and the documented regex forbids.
        assert!(
            !is_mime_shaped("TEXT/HTML"),
            "uppercase is outside the documented lowercase-only charset"
        );
        assert!(
            !is_mime_shaped("_/_"),
            "underscore is not in the documented charset at all"
        );
        assert!(
            !is_mime_shaped("-a/-a"),
            "a token may not start with punctuation"
        );
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

    #[test]
    fn defaults_match_the_documented_table_in_spec_section_5() {
        // `every_default_would_survive_its_own_parser` only proves internal
        // self-consistency (every default is inside the range its own parser
        // enforces) — it says nothing about whether those defaults are the
        // ones spec §5 actually documents. This pins each one to its literal
        // value from that table, so a future silent drift (e.g. someone
        // "tidies up" `unchecked_defaults` and changes a constant) fails a
        // test instead of shipping a config the spec does not describe.
        let d = WebServerSettings::default();
        assert_eq!(d.worker_connections.get(), 1024);
        assert_eq!(d.client_max_body_size.as_str(), "256m");
        assert_eq!(d.keepalive_timeout.get(), 65);
        assert!(d.tcp_nodelay.is_on());
        assert_eq!(d.fastcgi_connect_timeout.get(), 60);
        assert_eq!(d.fastcgi_send_timeout.get(), 300);
        assert_eq!(d.fastcgi_read_timeout.get(), 300);
        assert!(!d.gzip.is_on());
        assert_eq!(d.gzip_comp_level.get(), 1);
        assert_eq!(
            d.gzip_types.as_directive(),
            "text/plain text/css application/json application/javascript application/xml \
             image/svg+xml font/woff2"
        );
    }
}
