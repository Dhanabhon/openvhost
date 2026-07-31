// SPDX-License-Identifier: GPL-3.0-or-later
//! Render context and the single path-rendering chokepoint.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::ConfError;

/// The PHP upstream — the #1 cross-platform seam (master plan §3.4). Rendered
/// by a Rust `match` in the adapter, never by a Tera conditional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhpUpstream {
    /// macOS: `fastcgi_pass unix:<path>`.
    UnixSocket(PathBuf),
    /// Windows php-cgi pool — defined now, runtime deferred. Invariant:
    /// never empty (an empty nginx `upstream{}` fails `nginx -t`).
    TcpPorts(Vec<SocketAddr>),
}

#[derive(Debug, Clone)]
pub struct RenderCtx {
    pub home: PathBuf,
    pub server_name: String,
    pub docroot: PathBuf,
    pub listen_addr: SocketAddr,
    pub php_major: String,
    pub php_upstream: PhpUpstream,
    /// Stable, pre-sanitized, unique-per-site token that names the Windows
    /// nginx `upstream{}` block. Unused-but-present for `UnixSocket`.
    pub upstream_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    /// True iff the native validator exited 0. Never derived from stderr
    /// emptiness — php-fpm prints a harmless empty-glob WARNING every time.
    pub ok: bool,
    pub stderr: String,
}

fn valid_hostname_char(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-')
}

fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-')
        })
}

/// nginx `upstream{}` block name token: `[a-z0-9_]`, non-empty.
fn valid_upstream_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

impl RenderCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        home: PathBuf,
        server_name: impl Into<String>,
        docroot: PathBuf,
        listen_addr: SocketAddr,
        php_major: impl Into<String>,
        php_upstream: PhpUpstream,
        upstream_name: impl Into<String>,
    ) -> Result<Self, ConfError> {
        let server_name = server_name.into();
        let php_major = php_major.into();
        let upstream_name = upstream_name.into();

        // The empty-label check (rejects `.`, `..`, `a..b`, `.a`, `a.`) is
        // defense-in-depth, not reachable in production (the sole caller
        // feeds an already-`Domain::parse`d string, which already excludes
        // it) — see `rejects_a_server_name_with_an_empty_dot_separated_label`'s
        // doc comment for why it matters here anyway: `server_name` is
        // spliced into a WHOLE PATH COMPONENT (`webserver.rs`'s
        // `site_log_dir`), where `..` climbs a directory level, not merely
        // a filename suffix as it always used to be.
        if server_name.is_empty()
            || !server_name.bytes().all(valid_hostname_char)
            || server_name.split('.').any(str::is_empty)
        {
            return Err(ConfError::InvalidField {
                field: "server_name",
                value: server_name,
                reason: "must be a non-empty [a-z0-9.-] hostname with no empty dot-separated \
                         label",
            });
        }
        if !valid_component(&php_major) {
            return Err(ConfError::InvalidField {
                field: "php_major",
                value: php_major,
                reason: "must be a safe [a-z0-9._-] path component",
            });
        }
        if !valid_upstream_name(&upstream_name) {
            return Err(ConfError::InvalidField {
                field: "upstream_name",
                value: upstream_name,
                reason: "must be a non-empty [a-z0-9_] token",
            });
        }
        #[allow(clippy::collapsible_if)]
        if let PhpUpstream::TcpPorts(ports) = &php_upstream {
            if ports.is_empty() {
                return Err(ConfError::EmptyUpstream);
            }
        }
        Ok(Self {
            home,
            server_name,
            docroot,
            listen_addr,
            php_major,
            php_upstream,
            upstream_name,
        })
    }
}

/// The single chokepoint for embedding a path into a config template: reject
/// non-UTF-8 (Tera cannot render it), normalize `\` to `/`, strip a `\\?\` /
/// `\\?\UNC\` verbatim prefix (nginx's parser understands neither), and
/// reject an embedded `"`, `$`, `#`, `;`, or ASCII control character — the
/// crate's central quoting invariant. Every generated directive
/// double-quotes its path value, so a path containing `"` (e.g. a docroot of
/// `/tmp/x" ; daemon on; #`) would splice an injected directive that still
/// passes `nginx -t`. `$` matters for a different reason: nginx's `root` is a
/// *complex value* that expands variables even inside double quotes, so a
/// docroot containing e.g. `$http_x_root` renders as
/// `root "/tmp/projects/app$http_x_root";` — a directive that still passes
/// `nginx -t` but resolves the document root (and, via
/// `SCRIPT_FILENAME $document_root$fastcgi_script_name`, the PHP script path)
/// from an attacker-controlled request header at request time. `#` and `;`
/// guard a THIRD dialect this chokepoint also feeds: `openvhost_conf::mysql`
/// renders `my.cnf`, an INI file, whose parser treats `#` and `;` as
/// comment leaders with NO quoting escape at all — unlike nginx, where a
/// bare `#`/`;` inside a double-quoted value is inert, my.cnf has nothing
/// that could neutralize either character. A `datadir`/`socket`/`pid-file`
/// path containing one would silently truncate the value `mysqld` actually
/// reads from that point to end-of-line — not a syntax error `--validate-config`
/// would catch, just a quietly wrong path. The same chokepoint also feeds the
/// php-fpm pool template's unquoted INI `listen =`, `error_log =`, and
/// `include=` lines, where `$` is expanded and the identical comment-leader
/// hazard applies. A no-op on ordinary, clean unix paths.
pub(crate) fn to_config_path(p: &Path) -> Result<String, ConfError> {
    let s = p
        .to_str()
        .ok_or_else(|| ConfError::PathNotUtf8(p.to_path_buf()))?;
    let s = s
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(s).to_string());
    let s = s.replace('\\', "/");
    if s.contains('"')
        || s.contains('$')
        || s.contains('#')
        || s.contains(';')
        || s.bytes().any(|b| b.is_ascii_control())
    {
        return Err(ConfError::InvalidField {
            field: "path",
            value: s,
            reason: "must not contain a double-quote, dollar sign, hash, semicolon, \
                     or control character",
        });
    }
    Ok(s)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn unix_ctx() -> RenderCtx {
        RenderCtx::new(
            PathBuf::from("/tmp/ovh"),
            "myapp.localhost",
            PathBuf::from("/tmp/ovh/www"),
            "127.0.0.1:8080".parse().unwrap(),
            "8.4",
            PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
            "php_myapp",
        )
        .unwrap()
    }

    #[test]
    fn accepts_clean_ctx() {
        let c = unix_ctx();
        assert_eq!(c.server_name, "myapp.localhost");
        assert_eq!(c.php_major, "8.4");
    }

    #[test]
    fn rejects_bad_server_name() {
        for bad in ["", "a b", "UPPER", "has_underscore", "sl/ash", "semi;colon"] {
            let r = RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                bad,
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                "8.4",
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
                "php_x",
            );
            assert!(r.is_err(), "should reject server_name {bad:?}");
        }
    }

    /// Defense-in-depth (review finding, P1 live-log-viewer): before that
    /// slice `server_name` was only ever spliced in as a FILENAME SUFFIX
    /// (`<server_name>.conf`, `<server_name>.d`); `webserver.rs`'s
    /// `site_log_dir` now also makes it a WHOLE PATH COMPONENT for the
    /// first time (`<home>/logs/sites/<server_name>/`), so an empty
    /// dot-separated label — `.` or `..` — means something it never did
    /// before: `..` climbs one directory level. Unreachable in production
    /// today (the sole caller feeds an already-`Domain::parse`d string,
    /// which already rejects empty labels — see `site::model::Domain`), but
    /// this project's standing rule is that each layer confines on its own
    /// rather than trusting its caller (the Site-newtype-permissiveness
    /// carry-forward): `valid_hostname_char` admits `.` and this
    /// constructor had no empty-label check, so `.`/`..` passed straight
    /// through the charset check.
    #[test]
    fn rejects_a_server_name_with_an_empty_dot_separated_label() {
        for bad in [".", "..", "...", "a..b", ".a", "a.", "..a.."] {
            let r = RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                bad,
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                "8.4",
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
                "php_x",
            );
            assert!(
                r.is_err(),
                "should reject server_name {bad:?} (empty dot-separated label)"
            );
        }
    }

    #[test]
    fn rejects_bad_php_major() {
        for bad in ["", "..", "8/4", "8 4", "../etc"] {
            let r = RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                "a.localhost",
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                bad,
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
                "php_x",
            );
            assert!(r.is_err(), "should reject php_major {bad:?}");
        }
    }

    #[test]
    fn rejects_bad_upstream_name() {
        for bad in ["php myapp", "php-x", ""] {
            let r = RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                "a.localhost",
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                "8.4",
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
                bad,
            );
            assert!(r.is_err(), "should reject upstream_name {bad:?}");
        }
    }

    #[test]
    fn rejects_empty_tcp_upstream() {
        let r = RenderCtx::new(
            PathBuf::from("/tmp/ovh"),
            "a.localhost",
            PathBuf::from("/tmp/ovh/www"),
            "127.0.0.1:8080".parse().unwrap(),
            "8.4",
            PhpUpstream::TcpPorts(vec![]),
            "php_x",
        );
        assert!(matches!(r, Err(ConfError::EmptyUpstream)));
    }

    #[test]
    fn to_config_path_forward_slashes_and_checks_utf8() {
        // On unix the path is already forward-slash; the fn is identity there.
        let s = to_config_path(&PathBuf::from("/tmp/ovh/run/php-fpm.sock")).unwrap();
        assert_eq!(s, "/tmp/ovh/run/php-fpm.sock");
        // Backslashes (a Windows-style path, exercised even on unix) become '/'.
        let s2 = to_config_path(std::path::Path::new(r"C:\Users\a\www")).unwrap();
        assert_eq!(s2, "C:/Users/a/www");
        // A verbatim prefix is stripped.
        let s3 = to_config_path(std::path::Path::new(r"\\?\C:\x")).unwrap();
        assert_eq!(s3, "C:/x");
    }

    #[test]
    fn to_config_path_rejects_quote_and_control_chars() {
        let quoted = to_config_path(Path::new("/tmp/x\" ; daemon on; #"));
        assert!(matches!(
            quoted,
            Err(ConfError::InvalidField { field: "path", .. })
        ));
        let newline = to_config_path(Path::new("/tmp/x\ny"));
        assert!(matches!(
            newline,
            Err(ConfError::InvalidField { field: "path", .. })
        ));
    }

    /// Audit finding M1: `#` and `;` are the my.cnf/INI dialect's comment
    /// leaders (`openvhost_conf::mysql`'s templates render through this same
    /// chokepoint) — everything from the leader to end-of-line is silently
    /// DROPPED by that parser, not rejected. Unlike nginx (where a bare `#`
    /// or `;` inside a double-quoted directive value is inert), my.cnf has
    /// no quoting at all for these two characters: a datadir/socket path
    /// containing either would silently truncate the value MySQL actually
    /// uses, which is a distinct hazard from the quote/`$`/control-char class
    /// above and gets its own independent assertion per character (RED
    /// against pre-fix code: both currently pass straight through).
    #[test]
    fn to_config_path_rejects_ini_comment_leaders() {
        let hash = to_config_path(Path::new("/tmp/x#comment"));
        assert!(
            matches!(hash, Err(ConfError::InvalidField { field: "path", .. })),
            "got {hash:?}"
        );
        let semicolon = to_config_path(Path::new("/tmp/x;comment"));
        assert!(
            matches!(
                semicolon,
                Err(ConfError::InvalidField { field: "path", .. })
            ),
            "got {semicolon:?}"
        );
    }

    #[test]
    fn to_config_path_rejects_dollar_sign() {
        // Header-controlled-root vector (B1): nginx's `root` is a complex
        // value and expands `$`-variables even inside double quotes, so a
        // docroot containing `$http_evil` would let a request header choose
        // the document root while `nginx -t` stays green.
        let hostile = to_config_path(Path::new("/tmp/x$http_evil"));
        assert!(matches!(
            hostile,
            Err(ConfError::InvalidField { field: "path", .. })
        ));
    }
}
