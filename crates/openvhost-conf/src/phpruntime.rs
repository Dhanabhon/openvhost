// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtime config generation. Separate from WebServerAdapter because
//! php-fpm.conf has no Windows analog — `generate_pool_config` returns
//! `Option`, `None` on Windows (php-cgi pool membership is pure Rust state).

use std::path::Path;

use async_trait::async_trait;

use crate::ctx::{PhpUpstream, RenderCtx, to_config_path};
use crate::engine::render;
use crate::error::ConfError;
use crate::{GeneratedFile, ValidationReport};

#[async_trait]
pub trait PhpRuntimeAdapter: Send + Sync {
    fn generate_pool_config(
        &self,
        home: &Path,
        major: &str,
        upstream: &PhpUpstream,
    ) -> Result<Option<GeneratedFile>, ConfError>;
    async fn validate(
        &self,
        php_bin: &Path,
        ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError>;
}

pub struct PhpFpmRuntime;

#[async_trait]
impl PhpRuntimeAdapter for PhpFpmRuntime {
    fn generate_pool_config(
        &self,
        home: &Path,
        major: &str,
        upstream: &PhpUpstream,
    ) -> Result<Option<GeneratedFile>, ConfError> {
        let home_str = to_config_path(home)?;
        // php-fpm listens on the unix socket named by the upstream on macOS.
        let socket = match upstream {
            PhpUpstream::UnixSocket(p) => to_config_path(p)?,
            PhpUpstream::TcpPorts(_) => {
                // No php-fpm pool file on the TCP (Windows) path.
                return Ok(None);
            }
        };
        let mut tc = tera::Context::new();
        tc.insert(
            "custom_pool_dir",
            &format!("{home_str}/config/custom/php/{major}/pool.d"),
        );
        // Per-major, not shared: every major used to point at one
        // `logs/php-fpm.log`, so a line in it could never be attributed to a
        // pool (P1 live-log-viewer design, spec D1). This crate cannot
        // depend on `openvhost-core` (core depends on conf), so this value
        // is derived independently here rather than via
        // `openvhost_core::logs::LogPaths::php_fpm_error` — the two are kept
        // in sync by that module's own
        // `php_fpm_error_matches_the_confs_independent_render` test, which
        // renders through this exact function and compares.
        tc.insert(
            "error_log",
            &format!("{home_str}/logs/services/php-fpm-{major}/error.log"),
        );
        tc.insert("socket", &socket);
        tc.insert(
            "custom_pool_glob",
            &format!("{home_str}/config/custom/php/{major}/pool.d/*.conf"),
        );
        let contents = render("php-fpm/pool.conf", &tc)?;
        Ok(Some(GeneratedFile {
            path: home
                .join("config/generated/php")
                .join(major)
                .join("php-fpm.conf"),
            contents,
        }))
    }

    /// `ctx.home` MUST be a throwaway validation home — `validate`
    /// materializes generated files into it NON-ATOMICALLY (plain writes
    /// into `config/generated/...`). It must never be pointed at a live
    /// home; the apply/swap pipeline (deferred) owns atomic installation.
    async fn validate(
        &self,
        php_bin: &Path,
        ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError> {
        let Some(pool) = self.generate_pool_config(&ctx.home, &ctx.php_major, &ctx.php_upstream)?
        else {
            // No pool file on the TCP/Windows path — nothing to validate here.
            return Ok(ValidationReport {
                ok: true,
                stderr: String::new(),
            });
        };
        crate::validate::materialize(std::slice::from_ref(&pool))?;
        // php-fpm -t only needs the per-major log directory to pre-exist
        // (for error_log) — mirrors `generate_pool_config`'s own comment on
        // why this crate derives the path independently rather than via
        // `openvhost_core::logs::LogPaths::php_fpm_error`.
        let logs = ctx
            .home
            .join(format!("logs/services/php-fpm-{}", ctx.php_major));
        std::fs::create_dir_all(&logs).map_err(|e| ConfError::Io {
            op: "create_dir",
            path: logs,
            source: e,
        })?;
        let out = tokio::process::Command::new(php_bin)
            .arg("-t")
            .arg("-n") // hermetic: skip brew php.ini
            .arg("-y")
            .arg(&pool.path)
            .output()
            .await
            .map_err(|e| ConfError::ValidatorSpawn {
                bin: php_bin.display().to_string(),
                source: e,
            })?;
        Ok(ValidationReport {
            ok: out.status.success(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::PhpUpstream;
    use std::path::PathBuf;

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
    fn pool_config_is_semicolon_banner_and_per_major() {
        let ctx = unix_ctx();
        let f = PhpFpmRuntime
            .generate_pool_config(&ctx.home, &ctx.php_major, &ctx.php_upstream)
            .unwrap()
            .unwrap();
        assert_eq!(
            f.path,
            PathBuf::from("/tmp/ovh/config/generated/php/8.4/php-fpm.conf")
        );
        let c = &f.contents;
        assert!(c.starts_with("; "), "php-fpm banner MUST be ;-style, not #");
        assert!(c.contains("DO NOT EDIT"));
        assert!(c.contains("error_log = /tmp/ovh/logs/services/php-fpm-8.4/error.log"));
        assert!(c.contains("listen = /tmp/ovh/run/php-fpm.sock"));
        assert!(c.contains("pm.max_children = 4"));
        // Stated explicitly rather than relied on as php-fpm's own default —
        // the same move this project already made for nginx's mandatory
        // `-e`. Without nginx's `try_files $uri =404`, this is what stops
        // php-fpm from executing a request for a file that is not actually a
        // `.php` script (site_apply_e2e.rs's `/style.css/x.php` case).
        assert!(c.contains("security.limit_extensions = .php"));
        // A6: stated explicitly rather than relied on as php-fpm's own
        // default — if that default ever flipped, the desktop app's
        // inherited environment (whatever a terminal-launched dev build's
        // shell exports) would be visible to every script the pool runs.
        assert!(c.contains("clear_env = yes"));
        assert!(c.contains("include=/tmp/ovh/config/custom/php/8.4/pool.d/*.conf"));
    }

    /// The bug this task fixes: every php-fpm major used to point at ONE
    /// shared `logs/php-fpm.log`, so a line in it could never be attributed
    /// to a pool. Rendering the same upstream for two different majors must
    /// produce two different `error_log` lines.
    #[test]
    fn pool_config_error_log_differs_per_major() {
        let upstream = PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock"));
        let render = |major: &str| {
            PhpFpmRuntime
                .generate_pool_config(&PathBuf::from("/tmp/ovh"), major, &upstream)
                .unwrap()
                .unwrap()
                .contents
        };
        let a = render("8.3");
        let b = render("8.4");
        let error_log_line = |c: &str| {
            c.lines()
                .find(|l| l.starts_with("error_log"))
                .unwrap_or_else(|| panic!("no error_log line in:\n{c}"))
                .to_string()
        };
        let (line_a, line_b) = (error_log_line(&a), error_log_line(&b));
        assert_ne!(
            line_a, line_b,
            "both majors rendered the same error_log line"
        );
        assert!(line_a.contains("php-fpm-8.3"), "got {line_a:?}");
        assert!(line_b.contains("php-fpm-8.4"), "got {line_b:?}");
    }

    /// P1 live-log-viewer bug fix: `validate()` used to only ensure the FLAT
    /// `logs/` directory existed, which sufficed while `error_log` lived
    /// directly under it. Now that it is per-major, a real `php-fpm -t`
    /// fails before it ever reaches whatever the pool config says: "failed
    /// to open error_log ... No such file or directory". A fake binary is
    /// enough to prove `validate()` creates the directory itself — its exit
    /// code is irrelevant to what this test checks.
    #[cfg(unix)]
    #[tokio::test]
    async fn validate_creates_the_per_major_log_directory_before_invoking_the_binary() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let home = root.path().to_path_buf();
        let ctx = RenderCtx::new(
            home.clone(),
            "myapp.localhost",
            home.join("www"),
            "127.0.0.1:8080".parse().unwrap(),
            "8.4",
            PhpUpstream::UnixSocket(home.join("run/php-fpm.sock")),
            "php_myapp",
        )
        .unwrap();

        let bin = home.join("fake-php-fpm");
        std::fs::write(&bin, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

        PhpFpmRuntime.validate(&bin, &ctx).await.unwrap();

        let dir = home.join("logs/services/php-fpm-8.4");
        assert!(
            dir.is_dir(),
            "{dir:?} must exist — php-fpm creates the error_log FILE but not its directory"
        );
    }
}
