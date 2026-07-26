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
    fn generate_main_config(&self, home: &Path) -> Result<GeneratedFile, ConfError>;
    fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;
    fn generate_default_site_config(
        &self,
        home: &Path,
        listen: std::net::SocketAddr,
        php_upstream: Option<&PhpUpstream>,
    ) -> Result<GeneratedFile, ConfError>;
    async fn validate(&self, bin: &Path, ctx: &RenderCtx) -> Result<ValidationReport, ConfError>;
    fn supports_hot_reload(&self) -> bool;
}

pub struct NginxAdapter;

impl NginxAdapter {
    fn gen_dir(home: &Path) -> PathBuf {
        home.join("config/generated/nginx")
    }

    /// The OS branch, in Rust rather than in a Tera conditional: the platform
    /// seam stays type-checked (spec 2026-07-23 §4).
    fn upstream_parts(
        upstream: &PhpUpstream,
        upstream_name: &str,
    ) -> Result<(String, String), ConfError> {
        Ok(match upstream {
            PhpUpstream::UnixSocket(p) => {
                let sock = to_config_path(p)?;
                (String::new(), format!("fastcgi_pass \"unix:{sock}\";"))
            }
            PhpUpstream::TcpPorts(ports) => {
                let mut block = format!("upstream {upstream_name} {{\n");
                for addr in ports {
                    block.push_str(&format!("    server {addr} max_fails=1 fail_timeout=1s;\n"));
                }
                block.push_str("}\n\n");
                let pass = format!(
                    "fastcgi_pass {upstream_name};\n        fastcgi_next_upstream error timeout invalid_header http_500;"
                );
                (block, pass)
            }
        })
    }

    /// The PHP `location` block, or an empty string when no PHP runtime is
    /// installed — a `fastcgi_pass` with no pool behind it only produces 502s.
    fn php_location(
        home_str: &str,
        server_name: &str,
        php_pass: &str,
    ) -> Result<String, ConfError> {
        let mut tc = tera::Context::new();
        tc.insert("php_pass", php_pass);
        tc.insert(
            "custom_site_glob",
            &format!("{home_str}/config/custom/sites/{server_name}.d/*.conf"),
        );
        render("nginx/php-location.conf", &tc)
    }
}

#[async_trait]
impl WebServerAdapter for NginxAdapter {
    fn id(&self) -> &'static str {
        "nginx"
    }

    fn generate_main_config(&self, home: &Path) -> Result<GeneratedFile, ConfError> {
        let home_str = to_config_path(home)?;
        let mut tc = tera::Context::new();
        tc.insert(
            "custom_sites_dir",
            &format!("{home_str}/config/custom/sites"),
        );
        tc.insert("pid_path", &format!("{home_str}/run/nginx.pid"));
        tc.insert("error_log", &format!("{home_str}/logs/nginx.error.log"));
        tc.insert("access_log", &format!("{home_str}/logs/nginx.access.log"));
        tc.insert("temp_dir", &format!("{home_str}/run/nginx"));
        tc.insert(
            "generated_sites_glob",
            &format!("{home_str}/config/generated/nginx/sites/*.conf"),
        );
        tc.insert(
            "custom_sites_glob",
            &format!("{home_str}/config/custom/sites/*.conf"),
        );
        let contents = render("nginx/main.conf", &tc)?;
        Ok(GeneratedFile {
            path: Self::gen_dir(home).join("nginx.conf"),
            contents,
        })
    }

    fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError> {
        let home = to_config_path(&ctx.home)?;
        let docroot = to_config_path(&ctx.docroot)?;

        let (php_upstream_block, php_pass) =
            Self::upstream_parts(&ctx.php_upstream, &ctx.upstream_name)?;
        let php_location = Self::php_location(&home, &ctx.server_name, &php_pass)?;

        let mut tc = tera::Context::new();
        tc.insert(
            "custom_site_dir",
            &format!("{home}/config/custom/sites/{}.d", ctx.server_name),
        );
        tc.insert("php_upstream_block", &php_upstream_block);
        tc.insert("php_location", &php_location);
        tc.insert("listen_addr", &ctx.listen_addr.to_string());
        tc.insert("server_name", &ctx.server_name);
        tc.insert("docroot", &docroot);
        let contents = render("nginx/site.conf", &tc)?;
        Ok(GeneratedFile {
            path: Self::gen_dir(&ctx.home)
                .join("sites")
                .join(format!("{}.conf", ctx.server_name)),
            contents,
        })
    }

    fn generate_default_site_config(
        &self,
        home: &Path,
        listen: std::net::SocketAddr,
        php_upstream: Option<&PhpUpstream>,
    ) -> Result<GeneratedFile, ConfError> {
        let home_str = to_config_path(home)?;
        let docroot = to_config_path(&home.join("www"))?;
        let php_location = match php_upstream {
            // `default` is a fixed, safe token: it names the custom-config
            // directory for the catch-all and the Windows upstream block.
            Some(up) => {
                // The catch-all's `upstream{}` block is deliberately dropped
                // (`_`): on the unix path it is always empty, and the
                // Windows pool manager is a later phase that will revisit
                // the catch-all.
                let (_, pass) = Self::upstream_parts(up, "php_default")?;
                Self::php_location(&home_str, "default", &pass)?
            }
            None => String::new(),
        };
        let mut tc = tera::Context::new();
        tc.insert(
            "custom_sites_dir",
            &format!("{home_str}/config/custom/sites"),
        );
        tc.insert("listen_addr", &listen.to_string());
        tc.insert("docroot", &docroot);
        tc.insert("php_location", &php_location);
        let contents = render("nginx/default-site.conf", &tc)?;
        Ok(GeneratedFile {
            path: Self::gen_dir(home).join("sites").join("00-default.conf"),
            contents,
        })
    }

    /// `ctx.home` MUST be a throwaway validation home — `validate`
    /// materializes generated files into it NON-ATOMICALLY (plain writes
    /// into `config/generated/...`). It must never be pointed at a live
    /// home; the apply/swap pipeline (deferred) owns atomic installation.
    /// A clean run here also does NOT prove the REAL home's socket path
    /// fits `sun_path`: the caller's `MAX_SOCKET_PATH_BYTES <= 103` guard
    /// against the real home stays authoritative (spec §5).
    async fn validate(&self, bin: &Path, ctx: &RenderCtx) -> Result<ValidationReport, ConfError> {
        // Materialize main + site into ctx.home, pre-create the dirs `nginx -t`
        // needs (run/, run/nginx/, logs/ — NOT www/), then run the validator.
        let main = self.generate_main_config(&ctx.home)?;
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
        let out = tokio::process::Command::new(bin)
            .arg("-e")
            .arg(&err_log) // MANDATORY: without -e, nginx leaks to /opt/homebrew/var
            .arg("-t")
            .arg("-c")
            .arg(&main.path)
            .output()
            .await
            .map_err(|e| ConfError::ValidatorSpawn {
                bin: bin.display().to_string(),
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
        let f = NginxAdapter.generate_main_config(&unix_ctx().home).unwrap();
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
        assert!(c.contains("fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;"));
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
        let a = NginxAdapter.generate_main_config(&unix_ctx().home).unwrap();
        let b = NginxAdapter.generate_main_config(&unix_ctx().home).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn main_config_declares_mime_types() {
        let f = NginxAdapter
            .generate_main_config(std::path::Path::new("/tmp/ovh"))
            .unwrap();
        let c = &f.contents;
        // Without a types block nginx labels every response octet-stream and
        // browsers refuse to apply the stylesheet.
        assert!(c.contains("text/css                              css;"));
        assert!(c.contains("application/javascript                js mjs;"));
        assert!(c.contains("default_type application/octet-stream;"));
    }

    #[test]
    fn site_config_serves_static_files_without_php() {
        let c = NginxAdapter
            .generate_site_config(&unix_ctx())
            .unwrap()
            .contents;
        // The front controller handles unknown paths; real files are served as files.
        assert!(c.contains("try_files $uri $uri/ /index.php$is_args$args;"));
        assert!(c.contains("index index.php index.html;"));
        // SCRIPT_FILENAME must follow the request, not be pinned to index.php.
        assert!(c.contains("fastcgi_param SCRIPT_FILENAME $document_root$fastcgi_script_name;"));
        assert!(!c.contains("$document_root/index.php"));
    }

    #[test]
    fn php_location_refuses_to_execute_a_path_that_is_not_a_file() {
        let c = NginxAdapter
            .generate_site_config(&unix_ctx())
            .unwrap()
            .contents;
        // Without this, an uploaded avatar.jpg containing PHP executes via
        // /uploads/avatar.jpg/x.php. The guard is the whole defence.
        assert!(c.contains("try_files $uri =404;"));
        assert!(c.contains("fastcgi_param REDIRECT_STATUS 200;"));
        assert!(c.contains("location ~ /\\. {"));
    }

    #[test]
    fn default_site_is_the_catch_all_and_can_run_php() {
        let sock = PathBuf::from("/tmp/ovh/run/php-fpm-8.4.sock");
        let up = PhpUpstream::UnixSocket(sock);
        let f = NginxAdapter
            .generate_default_site_config(
                std::path::Path::new("/tmp/ovh"),
                "127.0.0.1:8080".parse().unwrap(),
                Some(&up),
            )
            .unwrap();
        assert_eq!(
            f.path,
            PathBuf::from("/tmp/ovh/config/generated/nginx/sites/00-default.conf")
        );
        let c = &f.contents;
        assert!(c.contains("listen 127.0.0.1:8080 default_server;"));
        assert!(c.contains("server_name _;"));
        assert!(c.contains(r#"root "/tmp/ovh/www";"#));
        assert!(c.contains(r#"fastcgi_pass "unix:/tmp/ovh/run/php-fpm-8.4.sock";"#));
    }

    #[test]
    fn default_site_without_php_has_no_fastcgi_at_all() {
        let f = NginxAdapter
            .generate_default_site_config(
                std::path::Path::new("/tmp/ovh"),
                "127.0.0.1:8080".parse().unwrap(),
                None,
            )
            .unwrap();
        let c = &f.contents;
        assert!(c.contains("default_server;"));
        // A fastcgi_pass with no pool behind it is a 502 generator.
        assert!(!c.contains("fastcgi_pass"));
        assert!(!c.contains("location ~ [^/]\\.php"));
    }
}
