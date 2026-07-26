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
        tc.insert("error_log", &format!("{home_str}/logs/php-fpm.log"));
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
        // php-fpm -t only needs logs/ to pre-exist (for error_log).
        let logs = ctx.home.join("logs");
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
        assert!(c.contains("error_log = /tmp/ovh/logs/php-fpm.log"));
        assert!(c.contains("listen = /tmp/ovh/run/php-fpm.sock"));
        assert!(c.contains("pm.max_children = 4"));
        assert!(c.contains("include=/tmp/ovh/config/custom/php/8.4/pool.d/*.conf"));
    }
}
