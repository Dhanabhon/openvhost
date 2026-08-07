// SPDX-License-Identifier: GPL-3.0-or-later
//! Turn the enabled sites into the complete generated config set, then plan,
//! commit and validate it. See
//! docs/superpowers/specs/2026-07-27-p1-site-apply-design.md.

mod commit;
mod error;
mod plan;
#[cfg(test)]
pub(crate) mod tests_support;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::path::{Path, PathBuf};

use openvhost_conf::{
    GeneratedFile, NginxAdapter, PhpFpmRuntime, PhpRuntimeAdapter, PhpUpstream, RenderCtx,
    WebServerAdapter, WebServerSettings,
};

pub use commit::{ApplyOutcome, ConfigValidator, NginxValidator, apply, commit, rollback};
pub use error::{ApplyError, RollbackReport};
pub use plan::{ApplyPlan, ChangeKind, FileChange, plan};

use crate::CoreError;
use crate::php::PhpRuntimeSource;
use crate::site::model::{Site, SiteId, WebServer};

/// Darwin's `sun_path` is 104 bytes including the NUL. php-fpm does not reject
/// a longer path — it warns, truncates, binds the wrong path, and nginx 502s
/// forever. Refuse early instead.
pub const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Every site is name-based virtual hosting on one port. Port 80 needs the
/// privileged helper (Phase 3).
pub const LISTEN_PORT: u16 = 8080;

pub fn listen_addr() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, LISTEN_PORT))
}

/// One discovered PHP installation: the `major.minor` it provides, the
/// `php-fpm` this app supervises, and where those binaries came from.
///
/// **The path is always concrete** (PHP-discovery design D3, mirroring
/// [`crate::nginx::NginxRuntime`] and [`crate::mysql::MysqlRuntime`]). For a
/// packaged runtime it names `packages/php/<major>/<version>/bin/php-fpm`,
/// never `packages/php/<major>/current/bin/php-fpm`: a supervised child is
/// spawned from whatever is recorded here, and spawning *through* the link
/// would mean a later `current` swap silently changed which binary a restart
/// brings up, with the running process and the one the UI describes diverging
/// and nothing in between to notice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpRuntime {
    pub major: String,
    pub fpm_bin: PathBuf,
    /// Which install produced [`Self::fpm_bin`] — see [`PhpRuntimeSource`].
    pub source: PhpRuntimeSource,
}

/// What is installed on this machine. Passed in as data rather than probed
/// here, so every test constructs it by hand and no test depends on what the
/// machine running it happens to have. `php` is ordered: the first entry is
/// the catch-all's runtime.
///
/// `nginx_bin` is `None` when discovery finds neither a packaged nor a
/// Homebrew nginx (nginx discovery design D3) — an honest absence rather than
/// a path to a binary that does not exist. Nothing in this module currently
/// reads it (`render_set` only ever consults `php`); it is carried here so
/// the desktop app's seam has one shape to fill in for both `InstalledRuntimes`
/// and `crate::platform` callers, mirroring `StackPaths.nginx_bin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledRuntimes {
    pub nginx_bin: Option<PathBuf>,
    pub php: Vec<PhpRuntime>,
}

#[derive(Debug, Clone)]
pub struct ApplyInput {
    pub home: PathBuf,
    /// ALL sites; `render_set` filters on `is_servable` itself.
    pub sites: Vec<Site>,
    pub runtimes: InstalledRuntimes,
    /// The editable nginx settings, as stored (`settings_repo`). Data in,
    /// like `sites` and `runtimes` — this module never reads them from the
    /// database itself, so the CLI and the desktop app supply the same
    /// struct and every test constructs it by hand.
    ///
    /// These reach exactly one generated file, the main config. That is what
    /// keeps the Web server page a second ENTRY POINT to this pipeline
    /// rather than a second pipeline: `plan` sees `nginx.conf` as Modified
    /// and the existing diff, `nginx -t`, rollback and restart cover it
    /// unchanged.
    pub settings: WebServerSettings,
}

/// Whether a site is one `render_set` may render as nginx: enabled AND its
/// chosen web server is nginx.
///
/// The site editor's own hint text promises that an Apache site "will save,
/// but it won't be served" (`SiteDrawer.svelte`, `#f-server-hint`, mirrored in
/// `WebServerRow.svelte`'s unavailable-brand copy). Filtering on `enabled`
/// alone would silently render an Apache site's docroot into an nginx
/// `server{}` block — serving it as nginx, which is not what the user chose
/// and not what the product told them would happen. This is the ONE place
/// that decides "is this site actually served", so both loops in
/// `render_set` (site configs and the `MissingRuntime` pre-check) go through
/// it rather than repeating the predicate and risking the two drifting apart.
fn is_servable(s: &Site) -> bool {
    s.enabled && matches!(s.web_server, WebServer::Nginx)
}

/// The php-fpm socket for one major, guarded against the `sun_path` ceiling.
pub fn socket_path(home: &Path, major: &str) -> Result<PathBuf, ApplyError> {
    let p = home.join("run").join(format!("php-fpm-{major}.sock"));
    let len = p.as_os_str().as_encoded_bytes().len();
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(ApplyError::Core(CoreError::SocketPathTooLong {
            path: p,
            len,
        }));
    }
    Ok(p)
}

/// nginx `upstream{}` block name: `[a-z0-9_]`, and genuinely unique per site.
///
/// Derived from the site's UUID rather than its domain because a
/// charset substitution on the domain is not injective — `a-b.example` and
/// `a.b-example` would both reduce to `php_a_b_example`, and on the Windows
/// path that means one nginx context defining the same upstream block twice
/// with different backends. The id is the table's primary key, so uniqueness
/// is structural.
fn upstream_name(id: &SiteId) -> String {
    format!("php_{}", id.as_str().replace('-', ""))
}

/// The complete desired config set, sorted by path so the output is stable.
/// Pure: no filesystem access at all.
pub fn render_set(input: &ApplyInput) -> Result<Vec<GeneratedFile>, ApplyError> {
    let nginx = NginxAdapter;
    let fpm = PhpFpmRuntime;
    let listen = listen_addr();

    // Every check that can fail without touching the disk runs first (spec §4.1).
    // Filtered by `is_servable`, not `enabled` alone: an Apache site is never
    // rendered, so it must never block the apply on a PHP version it will
    // never actually need installed.
    let available: Vec<String> = input.runtimes.php.iter().map(|r| r.major.clone()).collect();
    for site in input.sites.iter().filter(|s| is_servable(s)) {
        if !available.iter().any(|m| m == site.php_version.as_str()) {
            return Err(ApplyError::MissingRuntime {
                site: site.name.as_str().to_string(),
                requested: site.php_version.as_str().to_string(),
                available,
            });
        }
    }

    // The stored settings, threaded in through `ApplyInput` (the caller reads
    // them from `settings_repo`). The main config is the ONLY file they touch;
    // if that ever stops being true, the test below fails loudly.
    let mut out = vec![nginx.generate_main_config(&input.home, &input.settings)?];

    let default_upstream = match input.runtimes.php.first() {
        Some(rt) => Some(PhpUpstream::UnixSocket(socket_path(
            &input.home,
            &rt.major,
        )?)),
        None => None,
    };
    out.push(nginx.generate_default_site_config(&input.home, listen, default_upstream.as_ref())?);

    for site in input.sites.iter().filter(|s| is_servable(s)) {
        let major = site.php_version.as_str();
        let ctx = RenderCtx::new(
            input.home.clone(),
            site.domain.as_str(),
            PathBuf::from(site.docroot.as_str()),
            listen,
            major,
            PhpUpstream::UnixSocket(socket_path(&input.home, major)?),
            upstream_name(&site.id),
        )?;
        out.push(nginx.generate_site_config(&ctx)?);
    }

    for rt in &input.runtimes.php {
        let upstream = PhpUpstream::UnixSocket(socket_path(&input.home, &rt.major)?);
        if let Some(f) = fpm.generate_pool_config(&input.home, &rt.major, &upstream)? {
            out.push(f);
        }
    }

    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::site::apply::tests_support::{input, runtimes, site};

    #[test]
    fn changing_a_setting_changes_exactly_the_main_config() {
        // The whole architecture in one assertion: settings feed the generator, so
        // the existing plan/diff/validate/rollback pipeline covers them with no
        // second path. If this ever needs more than one file, something has leaked.
        let base = input(vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        let mut changed = base.clone();
        changed.settings.fastcgi_read_timeout = openvhost_conf::Seconds::parse(900).unwrap();

        let before = render_set(&base).unwrap();
        let after = render_set(&changed).unwrap();

        let differing: Vec<String> = before
            .iter()
            .zip(after.iter())
            .filter(|(a, b)| a.contents != b.contents)
            .map(|(a, _)| a.path.display().to_string())
            .collect();
        assert_eq!(differing.len(), 1, "got {differing:?}");
        assert!(differing[0].ends_with("nginx/nginx.conf"));
    }

    #[test]
    fn renders_main_catch_all_site_and_pool() {
        let set = render_set(&input(
            vec![site("app", "app.localhost", "8.4", true)],
            &["8.4"],
        ))
        .unwrap();
        let paths: Vec<String> = set.iter().map(|f| f.path.display().to_string()).collect();
        assert_eq!(
            paths,
            vec![
                "/tmp/ovh/config/generated/nginx/nginx.conf",
                "/tmp/ovh/config/generated/nginx/sites/00-default_server.conf",
                "/tmp/ovh/config/generated/nginx/sites/app.localhost.conf",
                "/tmp/ovh/config/generated/php/8.4/php-fpm.conf",
            ]
        );
    }

    #[test]
    fn is_deterministic() {
        let i = input(vec![site("app", "app.localhost", "8.4", true)], &["8.4"]);
        assert_eq!(render_set(&i).unwrap(), render_set(&i).unwrap());
    }

    #[test]
    fn a_disabled_site_is_not_rendered() {
        let set = render_set(&input(
            vec![
                site("app", "app.localhost", "8.4", true),
                site("old", "old.localhost", "8.4", false),
            ],
            &["8.4"],
        ))
        .unwrap();
        assert!(set.iter().any(|f| f.path.ends_with("app.localhost.conf")));
        assert!(!set.iter().any(|f| f.path.ends_with("old.localhost.conf")));
    }

    #[test]
    fn one_pool_per_installed_major_regardless_of_site_count() {
        let set = render_set(&input(
            vec![
                site("a", "a.localhost", "8.4", true),
                site("b", "b.localhost", "8.4", true),
                site("c", "c.localhost", "8.4", true),
            ],
            &["8.4"],
        ))
        .unwrap();
        let pools = set
            .iter()
            .filter(|f| f.path.ends_with("php-fpm.conf"))
            .count();
        assert_eq!(pools, 1);
    }

    #[test]
    fn pools_are_rendered_for_installed_majors_nobody_uses() {
        // The service set follows what is installed, not what sites ask for.
        let set = render_set(&input(
            vec![site("a", "a.localhost", "8.4", true)],
            &["8.3", "8.4"],
        ))
        .unwrap();
        assert!(set.iter().any(|f| f.path.ends_with("php/8.3/php-fpm.conf")));
        assert!(set.iter().any(|f| f.path.ends_with("php/8.4/php-fpm.conf")));
    }

    #[test]
    fn a_site_wanting_an_uninstalled_version_blocks_the_whole_apply() {
        let err = render_set(&input(
            vec![
                site("app", "app.localhost", "8.4", true),
                site("legacy", "legacy.localhost", "7.4", true),
            ],
            &["8.4"],
        ))
        .unwrap_err();
        match err {
            ApplyError::MissingRuntime {
                site,
                requested,
                available,
            } => {
                assert_eq!(site, "legacy");
                assert_eq!(requested, "7.4");
                assert_eq!(available, vec!["8.4".to_string()]);
            }
            other => panic!("expected MissingRuntime, got {other:?}"),
        }
    }

    #[test]
    fn a_disabled_site_never_blocks_on_a_missing_version() {
        let set = render_set(&input(
            vec![site("legacy", "legacy.localhost", "7.4", false)],
            &["8.4"],
        ));
        assert!(set.is_ok());
    }

    #[test]
    fn an_apache_site_is_saved_but_not_rendered() {
        // The site editor promises an Apache site "will save, but it won't be
        // served". Rendering it as nginx would silently substitute a different
        // web server for the one the user chose.
        //
        // Also requests a PHP version ("7.4") that is not in the installed set
        // (only "8.4" is) — because an Apache site is never rendered at all, that
        // must not block the whole apply. render_set returning Ok here (rather
        // than MissingRuntime) is what proves the MissingRuntime scan uses the
        // same is_servable filter as the render loop, not `enabled` alone.
        let mut apache = site("legacy", "legacy.localhost", "7.4", true);
        apache.web_server = WebServer::parse("apache").unwrap();
        let set = render_set(&input(
            vec![site("app", "app.localhost", "8.4", true), apache],
            &["8.4"],
        ))
        .unwrap();
        assert!(set.iter().any(|f| f.path.ends_with("app.localhost.conf")));
        assert!(
            !set.iter()
                .any(|f| f.path.ends_with("legacy.localhost.conf"))
        );
    }

    #[test]
    fn each_site_points_at_the_pool_socket_for_its_own_version() {
        let set = render_set(&input(
            vec![
                site("a", "a.localhost", "8.3", true),
                site("b", "b.localhost", "8.4", true),
            ],
            &["8.3", "8.4"],
        ))
        .unwrap();
        let a = set
            .iter()
            .find(|f| f.path.ends_with("a.localhost.conf"))
            .unwrap();
        let b = set
            .iter()
            .find(|f| f.path.ends_with("b.localhost.conf"))
            .unwrap();
        assert!(a.contents.contains("unix:/tmp/ovh/run/php-fpm-8.3.sock"));
        assert!(b.contents.contains("unix:/tmp/ovh/run/php-fpm-8.4.sock"));
    }

    #[test]
    fn a_home_too_deep_for_the_socket_is_refused_before_anything_renders() {
        let deep = PathBuf::from(format!("/tmp/{}", "d".repeat(120)));
        let err = render_set(&ApplyInput {
            home: deep,
            sites: vec![site("app", "app.localhost", "8.4", true)],
            runtimes: runtimes(&["8.4"]),
            settings: WebServerSettings::default(),
        })
        .unwrap_err();
        assert!(matches!(
            err,
            ApplyError::Core(CoreError::SocketPathTooLong { .. })
        ));
    }

    #[test]
    fn upstream_names_stay_distinct_for_domains_that_flatten_to_one_token() {
        // `a-b.example` and `a.b-example` both become `a_b_example` under a naive
        // charset substitution. They must not share an upstream block name.
        let set = render_set(&input(
            vec![
                site("one", "a-b.example", "8.4", true),
                site("two", "a.b-example", "8.4", true),
            ],
            &["8.4"],
        ))
        .unwrap();
        let names: Vec<String> = set
            .iter()
            .filter(|f| f.path.extension().is_some_and(|e| e == "conf"))
            .flat_map(|f| {
                f.contents
                    .lines()
                    .filter(|l| l.trim_start().starts_with("upstream "))
                    .map(|l| l.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .collect();
        // The unix path emits no upstream block, so this asserts on the derivation
        // directly rather than on rendered output.
        assert!(names.is_empty(), "unix path should emit no upstream block");

        let a = upstream_name(&SiteId::parse("11111111-1111-4111-8111-111111111111").unwrap());
        let b = upstream_name(&SiteId::parse("22222222-2222-4222-8222-222222222222").unwrap());
        assert_ne!(a, b);
        assert!(
            a.bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_')
        );
    }
}
