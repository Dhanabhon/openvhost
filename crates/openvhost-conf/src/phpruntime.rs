// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtime config generation. Separate from WebServerAdapter because
//! php-fpm.conf has no Windows analog — `generate_pool_config` returns
//! `Option`, `None` on Windows (php-cgi pool membership is pure Rust state).

use std::path::Path;

use async_trait::async_trait;

use crate::ctx::{RenderCtx, to_config_path};
use crate::engine::render;
use crate::error::ConfError;
use crate::{GeneratedFile, ValidationReport};

#[async_trait]
pub trait PhpRuntimeAdapter: Send + Sync {
    fn generate_pool_config(&self, ctx: &RenderCtx) -> Result<Option<GeneratedFile>, ConfError>;
    async fn validate(
        &self,
        php_bin: &Path,
        ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError>;
}

pub struct PhpFpmRuntime;

#[async_trait]
impl PhpRuntimeAdapter for PhpFpmRuntime {
    fn generate_pool_config(&self, ctx: &RenderCtx) -> Result<Option<GeneratedFile>, ConfError> {
        let home = to_config_path(&ctx.home)?;
        // php-fpm listens on the unix socket named by the upstream on macOS.
        let socket = match &ctx.php_upstream {
            crate::PhpUpstream::UnixSocket(p) => to_config_path(p)?,
            crate::PhpUpstream::TcpPorts(_) => {
                // No php-fpm pool file on the TCP (Windows) path.
                return Ok(None);
            }
        };
        let mut tc = tera::Context::new();
        tc.insert(
            "custom_pool_dir",
            &format!("{home}/config/custom/php/{}/pool.d", ctx.php_major),
        );
        tc.insert("error_log", &format!("{home}/logs/php-fpm.log"));
        tc.insert("socket", &socket);
        tc.insert(
            "custom_pool_glob",
            &format!("{home}/config/custom/php/{}/pool.d/*.conf", ctx.php_major),
        );
        let contents = render("php-fpm/pool.conf", &tc)?;
        Ok(Some(GeneratedFile {
            path: ctx
                .home
                .join("config/generated/php")
                .join(&ctx.php_major)
                .join("php-fpm.conf"),
            contents,
        }))
    }

    async fn validate(
        &self,
        _php_bin: &Path,
        _ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError> {
        unimplemented!("php-fpm validate lands in Task 3")
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
        let f = PhpFpmRuntime
            .generate_pool_config(&unix_ctx())
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
