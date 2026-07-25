// SPDX-License-Identifier: GPL-3.0-or-later
//! The `Site` domain entity + parse-don't-validate newtypes. Every field that
//! reaches generated config or a filesystem path is charset-checked here, at
//! the boundary (the P0-7 config-injection lesson pushed to ingress).

use std::path::Path;

use crate::error::CoreError;

fn invalid(field: &'static str, reason: impl Into<String>) -> CoreError {
    CoreError::Validation {
        field,
        reason: reason.into(),
    }
}

macro_rules! newtype_str {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq)]
        pub struct $name(String);
        impl $name {
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
    };
}

newtype_str!(SiteId);
newtype_str!(SiteName);
newtype_str!(Domain);
newtype_str!(PhpVersion);

impl SiteId {
    /// A fresh v4 UUID.
    pub fn new() -> Self {
        SiteId(uuid::Uuid::new_v4().to_string())
    }
    /// Parse a UUID string into a validated `SiteId`.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        uuid::Uuid::parse_str(s).map_err(|_| invalid("id", "not a UUID"))?;
        Ok(SiteId(s.to_string()))
    }
}
impl Default for SiteId {
    fn default() -> Self {
        Self::new()
    }
}

impl SiteName {
    /// Slug: `[a-z0-9]` first char, then `[a-z0-9-]`, length 1..=63.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let ok = (1..=63).contains(&s.len())
            && s.bytes()
                .enumerate()
                .all(|(i, b)| b.is_ascii_lowercase() || b.is_ascii_digit() || (i > 0 && b == b'-'));
        if !ok {
            return Err(invalid(
                "name",
                "must be a 1-63 char [a-z0-9-] slug starting alphanumeric",
            ));
        }
        Ok(SiteName(s.to_string()))
    }
}

impl Domain {
    /// Hostname: labels of `[a-z0-9-]` (no leading/trailing `-`), dot-joined,
    /// each label 1..=63, total ≤253, lowercase only.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let total_ok = (1..=253).contains(&s.len());
        // An empty `s` already fails here: `"".split('.')` yields one
        // zero-length label, which fails the per-label `1..=63` bound below.
        let labels_ok = s.split('.').all(|label| {
            (1..=63).contains(&label.len())
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        });
        if !(total_ok && labels_ok) {
            return Err(invalid(
                "domain",
                "must be a lowercase dotted hostname (labels [a-z0-9-])",
            ));
        }
        Ok(Domain(s.to_string()))
    }
}

impl PhpVersion {
    /// `major.minor`, digits only (e.g. `8.3`).
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        let ok = match s.split_once('.') {
            Some((maj, min)) => {
                !maj.is_empty()
                    && !min.is_empty()
                    && maj.bytes().all(|b| b.is_ascii_digit())
                    && min.bytes().all(|b| b.is_ascii_digit())
            }
            None => false,
        };
        if !ok {
            return Err(invalid(
                "php_version",
                "must be major.minor digits, e.g. 8.3",
            ));
        }
        Ok(PhpVersion(s.to_string()))
    }
}

/// A validated site document root: an absolute path with no NUL, `"`, or
/// control byte (the exact class `openvhost-conf`'s `to_config_path`
/// rejects — config-injection stopped at ingress). Constructed only via
/// `parse`; the inner value is guaranteed valid UTF-8.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Docroot(String);

impl Docroot {
    /// Validate a docroot string: absolute, valid UTF-8 (it's `&str`), no
    /// quote or control character (the exact class P0-7's `to_config_path`
    /// rejects). NUL is already covered by `is_ascii_control`.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        if !Path::new(s).is_absolute() {
            return Err(invalid("docroot", "must be an absolute path"));
        }
        if s.bytes().any(|b| b == b'"' || b.is_ascii_control()) {
            return Err(invalid("docroot", "contains a quote or control character"));
        }
        Ok(Docroot(s.to_string()))
    }
    /// The validated docroot as a `&str`.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The validated docroot as a `&Path`.
    pub fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebServer {
    Nginx,
    Apache,
}
impl WebServer {
    /// Parse `"nginx"` or `"apache"` into a `WebServer`.
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        match s {
            "nginx" => Ok(WebServer::Nginx),
            "apache" => Ok(WebServer::Apache),
            other => Err(invalid(
                "web_server",
                format!("unknown web server {other:?}"),
            )),
        }
    }
    /// The canonical lowercase name (`"nginx"` or `"apache"`).
    pub fn as_str(&self) -> &'static str {
        match self {
            WebServer::Nginx => "nginx",
            WebServer::Apache => "apache",
        }
    }
}

/// A persisted site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Site {
    pub id: SiteId,
    pub name: SiteName,
    pub domain: Domain,
    pub docroot: Docroot,
    pub web_server: WebServer,
    pub php_version: PhpVersion,
    pub enabled: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Un-persisted input (no id/timestamps) — all fields already validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSite {
    pub name: SiteName,
    pub domain: Domain,
    pub docroot: Docroot,
    pub web_server: WebServer,
    pub php_version: PhpVersion,
    pub enabled: bool,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn sitename_accepts_slug_rejects_hostile() {
        assert!(SiteName::parse("my-shop").is_ok());
        assert!(SiteName::parse("blog1").is_ok());
        for bad in [
            "",
            "-lead",
            "UPPER",
            "has space",
            "quote\"",
            "semi;colon",
            "a/b",
        ] {
            assert!(SiteName::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn domain_accepts_hostname_rejects_hostile() {
        assert!(Domain::parse("myshop.localhost").is_ok());
        for bad in [
            "",
            "bad domain",
            "a..b",
            ".lead",
            "trail.",
            "quote\".x",
            "x\n.y",
            "under_score.x",
        ] {
            assert!(Domain::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn phpversion_major_minor_only() {
        assert!(PhpVersion::parse("8.3").is_ok());
        for bad in ["8", "8.3.1", "8.x", "v8.3", ""] {
            assert!(PhpVersion::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn docroot_absolute_utf8_no_control_or_quote() {
        assert!(Docroot::parse("/srv/www/shop").is_ok());
        for bad in ["relative/path", "/has\"quote", "/has\0nul", "/has\ncontrol"] {
            assert!(Docroot::parse(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn docroot_roundtrips_as_path_and_as_str() {
        let docroot = Docroot::parse("/srv/www").unwrap();
        assert_eq!(docroot.as_path(), Path::new("/srv/www"));
        assert_eq!(docroot.as_str(), "/srv/www");
    }

    #[test]
    fn webserver_roundtrip() {
        assert_eq!(WebServer::parse("nginx").unwrap().as_str(), "nginx");
        assert_eq!(WebServer::parse("apache").unwrap().as_str(), "apache");
        assert!(WebServer::parse("caddy").is_err());
    }

    #[test]
    fn siteid_new_is_a_uuid_and_parses_back() {
        let id = SiteId::new();
        assert!(SiteId::parse(id.as_str()).is_ok());
        assert!(SiteId::parse("not-a-uuid").is_err());
    }
}
