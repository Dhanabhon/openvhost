// SPDX-License-Identifier: GPL-3.0-or-later
//! Web-server config generation. NginxAdapter renders the main + site
//! configs; the PHP-upstream OS branch is a Rust `match`, never a Tera
//! conditional (keeps the platform seam type-checked).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::ctx::{PhpUpstream, RenderCtx, to_config_path};
use crate::engine::render;
use crate::error::ConfError;
use crate::{GeneratedFile, ValidationReport};

#[async_trait]
pub trait WebServerAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    fn generate_main_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;
    fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;
    async fn validate(
        &self,
        nginx_bin: &Path,
        ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError>;
    fn supports_hot_reload(&self) -> bool;
}

pub struct NginxAdapter;

impl NginxAdapter {
    fn gen_dir(home: &Path) -> PathBuf {
        home.join("config/generated/nginx")
    }
}

#[async_trait]
impl WebServerAdapter for NginxAdapter {
    fn id(&self) -> &'static str {
        "nginx"
    }

    fn generate_main_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError> {
        let home = to_config_path(&ctx.home)?;
        let mut tc = tera::Context::new();
        tc.insert("custom_sites_dir", &format!("{home}/config/custom/sites"));
        tc.insert("pid_path", &format!("{home}/run/nginx.pid"));
        tc.insert("error_log", &format!("{home}/logs/nginx.error.log"));
        tc.insert("access_log", &format!("{home}/logs/nginx.access.log"));
        tc.insert("temp_dir", &format!("{home}/run/nginx"));
        tc.insert(
            "generated_sites_glob",
            &format!("{home}/config/generated/nginx/sites/*.conf"),
        );
        tc.insert(
            "custom_sites_glob",
            &format!("{home}/config/custom/sites/*.conf"),
        );
        let contents = render("nginx/main.conf", &tc)?;
        Ok(GeneratedFile {
            path: Self::gen_dir(&ctx.home).join("nginx.conf"),
            contents,
        })
    }

    fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError> {
        let home = to_config_path(&ctx.home)?;
        let docroot = to_config_path(&ctx.docroot)?;

        // OS branch in Rust: build the upstream block + fastcgi_pass directive.
        let (php_upstream_block, php_pass) = match &ctx.php_upstream {
            PhpUpstream::UnixSocket(p) => {
                let sock = to_config_path(p)?;
                (String::new(), format!("fastcgi_pass \"unix:{sock}\";"))
            }
            PhpUpstream::TcpPorts(ports) => {
                let mut block = format!("upstream {} {{\n", ctx.upstream_name);
                for addr in ports {
                    block.push_str(&format!("    server {addr} max_fails=1 fail_timeout=1s;\n"));
                }
                block.push_str("}\n\n");
                let pass = format!(
                    "fastcgi_pass {};\n        fastcgi_next_upstream error timeout invalid_header http_500;",
                    ctx.upstream_name
                );
                (block, pass)
            }
        };

        let mut tc = tera::Context::new();
        tc.insert(
            "custom_site_dir",
            &format!("{home}/config/custom/sites/{}.d", ctx.server_name),
        );
        tc.insert("php_upstream_block", &php_upstream_block);
        tc.insert("php_pass", &php_pass);
        tc.insert("listen_addr", &ctx.listen_addr.to_string());
        tc.insert("server_name", &ctx.server_name);
        tc.insert("docroot", &docroot);
        tc.insert(
            "custom_site_glob",
            &format!("{home}/config/custom/sites/{}.d/*.conf", ctx.server_name),
        );
        let contents = render("nginx/site.conf", &tc)?;
        Ok(GeneratedFile {
            path: Self::gen_dir(&ctx.home)
                .join("sites")
                .join(format!("{}.conf", ctx.server_name)),
            contents,
        })
    }

    async fn validate(
        &self,
        nginx_bin: &Path,
        ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError> {
        // Materialize main + site into ctx.home, pre-create the dirs `nginx -t`
        // needs (run/, run/nginx/, logs/ — NOT www/), then run the validator.
        let main = self.generate_main_config(ctx)?;
        let site = self.generate_site_config(ctx)?;
        crate::validate::materialize(&[main.clone(), site])?;
        for d in ["run", "run/nginx", "logs"] {
            let p = ctx.home.join(d);
            std::fs::create_dir_all(&p).map_err(|e| ConfError::Io {
                op: "create_dir",
                path: p,
                source: e,
            })?;
        }
        let err_log = ctx.home.join("logs/nginx.error.log");
        let out = tokio::process::Command::new(nginx_bin)
            .arg("-e")
            .arg(&err_log) // MANDATORY: without -e, nginx leaks to /opt/homebrew/var
            .arg("-t")
            .arg("-c")
            .arg(&main.path)
            .output()
            .await
            .map_err(|e| ConfError::ValidatorSpawn {
                bin: nginx_bin.display().to_string(),
                source: e,
            })?;
        Ok(ValidationReport {
            ok: out.status.success(), // exit code ONLY
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }

    fn supports_hot_reload(&self) -> bool {
        true
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
    fn main_config_is_banner_quoted_and_includes() {
        let f = NginxAdapter.generate_main_config(&unix_ctx()).unwrap();
        assert_eq!(
            f.path,
            PathBuf::from("/tmp/ovh/config/generated/nginx/nginx.conf")
        );
        let c = &f.contents;
        assert!(c.starts_with("# "), "nginx banner is #-style");
        assert!(c.contains("DO NOT EDIT"));
        assert!(c.contains("/tmp/ovh/config/custom/sites")); // banner names custom path
        assert!(c.contains("daemon off;"));
        assert!(c.contains(r#"pid "/tmp/ovh/run/nginx.pid";"#)); // quoted
        assert!(c.contains(r#"error_log "/tmp/ovh/logs/nginx.error.log" warn;"#));
        assert!(c.contains(r#"fastcgi_temp_path "/tmp/ovh/run/nginx/fastcgi";"#));
        assert!(c.contains(r#"include "/tmp/ovh/config/generated/nginx/sites/*.conf";"#));
        assert!(c.contains(r#"include "/tmp/ovh/config/custom/sites/*.conf";"#));
    }

    #[test]
    fn site_config_unix_upstream() {
        let f = NginxAdapter.generate_site_config(&unix_ctx()).unwrap();
        assert_eq!(
            f.path,
            PathBuf::from("/tmp/ovh/config/generated/nginx/sites/myapp.localhost.conf")
        );
        let c = &f.contents;
        assert!(c.starts_with("# "));
        assert!(c.contains("listen 127.0.0.1:8080;"));
        assert!(c.contains("server_name myapp.localhost;"));
        assert!(c.contains(r#"root "/tmp/ovh/www";"#));
        assert!(c.contains(r#"fastcgi_pass "unix:/tmp/ovh/run/php-fpm.sock";"#));
        assert!(c.contains("fastcgi_param SCRIPT_FILENAME $document_root/index.php;"));
        // no upstream block for the unix path:
        assert!(!c.contains("upstream "));
    }

    #[test]
    fn site_config_tcp_upstream_seam() {
        // Windows-shaped path — defined + unit-proven, not runtime-tested.
        let ctx = RenderCtx::new(
            PathBuf::from("/tmp/ovh"),
            "win.localhost",
            PathBuf::from("/tmp/ovh/www"),
            "127.0.0.1:8080".parse().unwrap(),
            "8.4",
            PhpUpstream::TcpPorts(vec![
                "127.0.0.1:9001".parse().unwrap(),
                "127.0.0.1:9002".parse().unwrap(),
            ]),
            "php_win",
        )
        .unwrap();
        let c = NginxAdapter.generate_site_config(&ctx).unwrap().contents;
        assert!(c.contains("upstream php_win {"));
        assert!(c.contains("server 127.0.0.1:9001 max_fails=1 fail_timeout=1s;"));
        assert!(c.contains("server 127.0.0.1:9002 max_fails=1 fail_timeout=1s;"));
        assert!(c.contains("fastcgi_pass php_win;"));
        assert!(c.contains("fastcgi_next_upstream error timeout invalid_header http_500;"));
    }

    #[test]
    fn generation_is_deterministic() {
        let a = NginxAdapter.generate_main_config(&unix_ctx()).unwrap();
        let b = NginxAdapter.generate_main_config(&unix_ctx()).unwrap();
        assert_eq!(a, b);
    }
}
