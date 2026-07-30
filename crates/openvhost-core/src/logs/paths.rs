// SPDX-License-Identifier: GPL-3.0-or-later
//! `LogPaths` — the single owner of every on-disk log path.
//!
//! # Confinement
//!
//! Every accessor here is built by joining fixed, hardcoded path SEGMENTS
//! onto `home` plus (for the per-major and per-site accessors) the inner
//! value of an already-validated newtype: [`PhpVersion`](crate::PhpVersion)
//! (`major.minor` digits only, parsed at ingress) or
//! [`Domain`](crate::Domain) (lowercase dotted `[a-z0-9-]` labels, parsed at
//! ingress). Neither newtype's charset admits `/`, `..`, a NUL byte, or any
//! other byte `PathBuf::join` would need to treat specially, so every
//! accessor's return value is provably a fixed-depth descendant of
//! [`root()`](LogPaths::root) — never a path that climbs out of it — by
//! construction of the TYPES this module accepts, not by a runtime check
//! here.
//!
//! What this module does NOT guarantee (the Docroot lesson, carried
//! forward): a `Domain`/`PhpVersion` being charset-valid is not a claim that
//! the site or runtime it names still exists, and a derived path is not a
//! claim about what is actually on disk there — a symlink could have been
//! planted at it after the fact. Both of those are the IPC/reader layer's
//! job (spec D5: catalogue check before deriving, `symlink_metadata` refusal
//! of anything that is not a regular file after deriving), deliberately kept
//! out of this pure path-building module.
//!
//! # The `openvhost-conf` seam
//!
//! `openvhost-conf` renders the nginx/php-fpm config that actually points at
//! these paths, but it cannot depend on this crate (core depends on conf,
//! so the reverse would cycle). Its `webserver.rs`/`phpruntime.rs` therefore
//! keep their own independent derivation of the identical values, documented
//! at each site. This module's own tests
//! (`nginx_log_values_match_the_confs_independent_render`,
//! `php_fpm_error_matches_the_confs_independent_render`) render through
//! those real conf-crate functions and assert the two agree, so a
//! divergence fails a test instead of silently drifting.

use std::path::{Path, PathBuf};

use crate::site::{Domain, PhpVersion};

/// The single owner of every on-disk log path OpenVHost derives. See the
/// module doc for the confinement argument and the `openvhost-conf` seam.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogPaths {
    root: PathBuf,
}

impl LogPaths {
    /// Build the log paths rooted at `<home>/logs`.
    pub fn new(home: &Path) -> Self {
        Self {
            root: home.join("logs"),
        }
    }

    /// `<home>/logs` — the confinement anchor every other accessor's output
    /// is guaranteed to fall under.
    pub fn root(&self) -> PathBuf {
        self.root.clone()
    }

    /// `<home>/logs/nginx.error.log`.
    ///
    /// UNCHANGED value (P1 live-log-viewer design, spec D1: nginx's globals
    /// are deliberately NOT relocated by this refactor — see the module doc
    /// for why, and for the `openvhost-conf` seam this value's formula is
    /// duplicated across).
    pub fn nginx_error(&self) -> PathBuf {
        self.root.join("nginx.error.log")
    }

    /// `<home>/logs/nginx.access.log`. UNCHANGED value — see [`Self::nginx_error`].
    pub fn nginx_access(&self) -> PathBuf {
        self.root.join("nginx.access.log")
    }

    /// `<home>/logs/services/php-fpm-<major>/error.log` — one file per PHP
    /// major, so a line in it can be attributed to a pool.
    ///
    /// This is the bug fix folded into this refactor: every major used to
    /// share one `logs/php-fpm.log` file (`openvhost-conf`'s
    /// `phpruntime.rs`), so a line could never be traced back to the pool
    /// that wrote it.
    pub fn php_fpm_error(&self, major: &PhpVersion) -> PathBuf {
        self.root
            .join("services")
            .join(format!("php-fpm-{}", major.as_str()))
            .join("error.log")
    }

    /// `<home>/logs/sites/<domain>/` — the per-site log directory. Nothing
    /// consumes this yet (Task 3 wires the nginx templates to it); defined
    /// now so every log path has exactly one place it is derived.
    pub fn site_dir(&self, domain: &Domain) -> PathBuf {
        self.root.join("sites").join(domain.as_str())
    }

    /// `<home>/logs/sites/<domain>/access.log`.
    pub fn site_access(&self, domain: &Domain) -> PathBuf {
        self.site_dir(domain).join("access.log")
    }

    /// `<home>/logs/sites/<domain>/error.log`.
    pub fn site_error(&self, domain: &Domain) -> PathBuf {
        self.site_dir(domain).join("error.log")
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::{Domain, PhpVersion};
    use std::path::Path;

    /// A fixture home matching this crate's existing test convention
    /// (`openvhost_conf`'s own tests render against the same literal home).
    fn fixture() -> LogPaths {
        LogPaths::new(Path::new("/tmp/ovh"))
    }

    #[test]
    fn root_is_home_joined_with_logs() {
        assert_eq!(fixture().root(), PathBuf::from("/tmp/ovh/logs"));
    }

    /// Spec D1: nginx's globals are NOT relocated by this refactor. These two
    /// values must stay byte-identical to what every call site already
    /// hardcoded — see `nginx_log_values_match_the_confs_independent_render`
    /// below for the proof that `openvhost-conf`'s own independent
    /// derivation (it cannot depend on this crate) agrees.
    #[test]
    fn nginx_error_and_access_are_the_unchanged_historical_values() {
        let p = fixture();
        assert_eq!(
            p.nginx_error(),
            PathBuf::from("/tmp/ovh/logs/nginx.error.log")
        );
        assert_eq!(
            p.nginx_access(),
            PathBuf::from("/tmp/ovh/logs/nginx.access.log")
        );
    }

    /// The bug this task fixes: every php-fpm major used to share ONE
    /// `logs/php-fpm.log` file, so a line in it could never be attributed to
    /// a pool.
    #[test]
    fn php_fpm_error_differs_per_major() {
        let p = fixture();
        let v84 = PhpVersion::parse("8.4").unwrap();
        let v83 = PhpVersion::parse("8.3").unwrap();
        let e84 = p.php_fpm_error(&v84);
        let e83 = p.php_fpm_error(&v83);
        assert_ne!(e84, e83, "two majors must not share one log file");
        assert_eq!(
            e84,
            PathBuf::from("/tmp/ovh/logs/services/php-fpm-8.4/error.log")
        );
        assert_eq!(
            e83,
            PathBuf::from("/tmp/ovh/logs/services/php-fpm-8.3/error.log")
        );
    }

    #[test]
    fn site_log_paths_never_collide_across_two_domains() {
        let p = fixture();
        let a = Domain::parse("shop.localhost").unwrap();
        let b = Domain::parse("blog.localhost").unwrap();
        let paths = [
            p.site_dir(&a),
            p.site_access(&a),
            p.site_error(&a),
            p.site_dir(&b),
            p.site_access(&b),
            p.site_error(&b),
        ];
        for i in 0..paths.len() {
            for j in (i + 1)..paths.len() {
                assert_ne!(
                    paths[i], paths[j],
                    "path {i} collided with path {j}: {:?}",
                    paths[i]
                );
            }
        }
    }

    #[test]
    fn site_access_and_error_live_directly_inside_site_dir() {
        let p = fixture();
        let d = Domain::parse("shop.localhost").unwrap();
        assert_eq!(
            p.site_dir(&d),
            PathBuf::from("/tmp/ovh/logs/sites/shop.localhost")
        );
        assert_eq!(
            p.site_access(&d),
            PathBuf::from("/tmp/ovh/logs/sites/shop.localhost/access.log")
        );
        assert_eq!(
            p.site_error(&d),
            PathBuf::from("/tmp/ovh/logs/sites/shop.localhost/error.log")
        );
    }

    /// Every accessor's output is confined under `root()` — the confinement
    /// argument this module's doc comment states, exercised rather than
    /// merely asserted in prose.
    #[test]
    fn every_accessors_output_starts_with_root() {
        let p = fixture();
        let major = PhpVersion::parse("8.4").unwrap();
        let domain = Domain::parse("shop.localhost").unwrap();
        let root = p.root();
        for path in [
            p.nginx_error(),
            p.nginx_access(),
            p.php_fpm_error(&major),
            p.site_dir(&domain),
            p.site_access(&domain),
            p.site_error(&domain),
        ] {
            assert!(path.starts_with(&root), "{path:?} escaped {root:?}");
        }
    }

    /// A mutation that hardcoded `/tmp/ovh` instead of using `home` would
    /// still pass every test above; this one changes `home` and requires the
    /// output to follow.
    #[test]
    fn new_is_rooted_at_the_given_home_not_a_fixed_path() {
        let other = LogPaths::new(Path::new("/elsewhere"));
        assert_eq!(other.root(), PathBuf::from("/elsewhere/logs"));
        assert_eq!(
            other.nginx_error(),
            PathBuf::from("/elsewhere/logs/nginx.error.log")
        );
    }

    // --- Seam tests -----------------------------------------------------
    //
    // `openvhost-conf` cannot depend on this crate (core depends on conf),
    // so `webserver.rs`/`phpruntime.rs` independently derive these same
    // values inline rather than calling `LogPaths`. These two tests render
    // through the REAL conf-crate functions and compare their output against
    // `LogPaths`, so the two derivations drifting apart fails a test instead
    // of silently diverging.

    #[test]
    fn nginx_log_values_match_the_confs_independent_render() {
        use openvhost_conf::WebServerAdapter;

        let home = Path::new("/tmp/ovh");
        let p = LogPaths::new(home);
        let rendered = openvhost_conf::NginxAdapter
            .generate_main_config(home, &openvhost_conf::WebServerSettings::default())
            .unwrap()
            .contents;
        assert!(
            rendered.contains(&format!(
                r#"error_log "{}" warn;"#,
                p.nginx_error().display()
            )),
            "conf's rendered error_log no longer matches LogPaths::nginx_error:\n{rendered}"
        );
        // A PREFIX check, not the full directive: the P1 live-log-viewer
        // slice appended a named `log_format` reference after the quoted
        // path (spec D5's privacy format), and that format's NAME is
        // `webserver.rs`'s own concern (see its
        // `main_config_declares_an_explicit_log_format_and_uses_it_for_the_global_access_log`
        // test) — this seam test's only job is pinning the PATH against
        // `LogPaths::nginx_access`.
        assert!(
            rendered.contains(&format!(r#"access_log "{}""#, p.nginx_access().display())),
            "conf's rendered access_log no longer matches LogPaths::nginx_access:\n{rendered}"
        );
    }

    /// Same seam as `nginx_log_values_match_the_confs_independent_render`,
    /// for the NEW per-site paths (P1 live-log-viewer design, spec D1): a
    /// site's `access_log`/`error_log` are rendered by
    /// `openvhost-conf`'s `generate_site_config` independently of
    /// `LogPaths::site_access`/`site_error` (this crate cannot depend on
    /// `openvhost-conf` in the other direction), so this test renders
    /// through that REAL function and compares, exactly mirroring how the
    /// nginx-globals and php-fpm seam tests above already guard their own
    /// values.
    #[test]
    fn site_log_values_match_the_confs_independent_render() {
        use openvhost_conf::{PhpUpstream, RenderCtx, WebServerAdapter};

        let home = Path::new("/tmp/ovh");
        let domain = Domain::parse("myapp.localhost").unwrap();
        let p = LogPaths::new(home);
        let ctx = RenderCtx::new(
            home.to_path_buf(),
            domain.as_str(),
            home.join("www"),
            "127.0.0.1:8080".parse().unwrap(),
            "8.4",
            PhpUpstream::UnixSocket(home.join("run/php-fpm.sock")),
            "php_myapp",
        )
        .unwrap();
        let rendered = openvhost_conf::NginxAdapter
            .generate_site_config(&ctx)
            .unwrap()
            .contents;
        assert!(
            rendered.contains(&format!(
                r#"access_log "{}""#,
                p.site_access(&domain).display()
            )),
            "conf's rendered access_log no longer matches LogPaths::site_access:\n{rendered}"
        );
        assert!(
            rendered.contains(&format!(
                r#"error_log "{}""#,
                p.site_error(&domain).display()
            )),
            "conf's rendered error_log no longer matches LogPaths::site_error:\n{rendered}"
        );
    }

    #[test]
    fn php_fpm_error_matches_the_confs_independent_render() {
        use openvhost_conf::PhpRuntimeAdapter;

        let home = Path::new("/tmp/ovh");
        let p = LogPaths::new(home);
        let major = PhpVersion::parse("8.4").unwrap();
        let upstream =
            openvhost_conf::PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock"));
        let rendered = openvhost_conf::PhpFpmRuntime
            .generate_pool_config(home, major.as_str(), &upstream)
            .unwrap()
            .unwrap()
            .contents;
        assert!(
            rendered.contains(&format!(
                "error_log = {}",
                p.php_fpm_error(&major).display()
            )),
            "conf's rendered error_log no longer matches LogPaths::php_fpm_error:\n{rendered}"
        );
    }
}
