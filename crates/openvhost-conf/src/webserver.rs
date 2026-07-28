// SPDX-License-Identifier: GPL-3.0-or-later
//! Web-server config generation. NginxAdapter renders the main + site
//! configs; the PHP-upstream OS branch is a Rust `match`, never a Tera
//! conditional (keeps the platform seam type-checked).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::ctx::{PhpUpstream, RenderCtx, to_config_path};
use crate::engine::render;
use crate::error::ConfError;
use crate::settings::WebServerSettings;
use crate::{GeneratedFile, ValidationReport};

#[async_trait]
pub trait WebServerAdapter: Send + Sync {
    fn id(&self) -> &'static str;
    /// The main config. `settings` supplies every tunable the Web server page
    /// edits; each one is written explicitly, even at its default value, so
    /// the generated file states what it means rather than leaving the reader
    /// to know nginx's own fallbacks (the same call already made for
    /// `clear_env` and `security.limit_extensions` in the php-fpm pool).
    fn generate_main_config(
        &self,
        home: &Path,
        settings: &WebServerSettings,
    ) -> Result<GeneratedFile, ConfError>;
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

    /// The gzip lines that only mean anything once gzip is on, composed in
    /// Rust rather than with a Tera `{% if %}` — the same rule the platform
    /// branch follows: decisions live in Rust, the template only interpolates.
    ///
    /// An empty `gzip_types` list yields NO `gzip_types` directive at all,
    /// rather than a bare `gzip_types;` (which nginx rejects outright). An
    /// empty list is a legitimate setting meaning "compress nothing beyond
    /// nginx's built-in `text/html`", and omitting the directive is exactly
    /// how nginx itself expresses that.
    ///
    /// Each line carries its own LEADING newline and the template appends this
    /// directly after `gzip on|off;` — so the empty case adds no blank line at
    /// all. Toggling gzip then shows up in the apply diff as exactly the lines
    /// that changed, instead of dragging a phantom blank line along with them.
    fn gzip_extra(settings: &WebServerSettings) -> String {
        if !settings.gzip.is_on() {
            return String::new();
        }
        let mut out = format!("\n    gzip_comp_level {};", settings.gzip_comp_level.get());
        let types = settings.gzip_types.as_directive();
        if !types.is_empty() {
            out.push_str(&format!("\n    gzip_types {types};"));
        }
        out
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

    fn generate_main_config(
        &self,
        home: &Path,
        settings: &WebServerSettings,
    ) -> Result<GeneratedFile, ConfError> {
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
        // Scope is load-bearing and is fixed by the template, not here:
        // `worker_connections` is only legal inside `events`, and the
        // `fastcgi_*` timeouts sit at `http` scope deliberately so they apply
        // to every site rather than being repeated per server block.
        tc.insert("worker_connections", &settings.worker_connections.get());
        tc.insert(
            "client_max_body_size",
            settings.client_max_body_size.as_str(),
        );
        tc.insert("keepalive_timeout", &settings.keepalive_timeout.get());
        tc.insert("tcp_nodelay", settings.tcp_nodelay.as_str());
        tc.insert(
            "fastcgi_connect_timeout",
            &settings.fastcgi_connect_timeout.get(),
        );
        tc.insert("fastcgi_send_timeout", &settings.fastcgi_send_timeout.get());
        tc.insert("fastcgi_read_timeout", &settings.fastcgi_read_timeout.get());
        tc.insert("gzip", settings.gzip.as_str());
        tc.insert("gzip_extra", &Self::gzip_extra(settings));
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
            path: Self::gen_dir(home)
                .join("sites")
                .join("00-default_server.conf"),
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
        // Defaults, not the user's stored settings: this call answers "is the
        // generated SHAPE valid?", and the user's values reach a real
        // `nginx -t` through the apply pipeline's `validate_live` on the
        // installed file.
        let main = self.generate_main_config(&ctx.home, &WebServerSettings::default())?;
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
    use crate::settings::{BodySize, GzipLevel, GzipTypes, OnOff, Seconds, WorkerConnections};
    use std::path::PathBuf;

    /// Render the main config for the default settings — the shape most tests
    /// want.
    fn main_conf() -> String {
        NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &WebServerSettings::default())
            .unwrap()
            .contents
    }

    /// The block path a directive sits inside, e.g. `["http"]`, walking the
    /// generated file line by line.
    ///
    /// Scope, not mere presence, is what this file gets wrong in the way that
    /// matters: `worker_connections` is legal only inside `events`, and a test
    /// that greps the whole string would pass just as happily with it sitting
    /// in `http`, where nginx refuses to start. Returns `None` when the
    /// directive is absent.
    fn scope_of(contents: &str, directive: &str) -> Option<Vec<String>> {
        let mut stack: Vec<String> = Vec::new();
        for line in contents.lines() {
            let t = line.trim();
            if t.starts_with('#') {
                continue;
            }
            if t.split_whitespace().next() == Some(directive) && t.ends_with(';') {
                return Some(stack);
            }
            if let Some(name) = t.strip_suffix('{') {
                stack.push(name.trim().to_string());
            } else if t == "}" {
                stack.pop();
            }
        }
        None
    }

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
        let f = NginxAdapter
            .generate_main_config(&unix_ctx().home, &WebServerSettings::default())
            .unwrap();
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
        let a = NginxAdapter
            .generate_main_config(&unix_ctx().home, &WebServerSettings::default())
            .unwrap();
        let b = NginxAdapter
            .generate_main_config(&unix_ctx().home, &WebServerSettings::default())
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn main_config_declares_mime_types() {
        let c = &main_conf();
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

    /// nginx matches regex `location` blocks in FILE ORDER and stops at the
    /// first match, so the dotfile deny must appear before the PHP location —
    /// otherwise a request like `/.env.php` or `/.git/x.php` hits the PHP
    /// location first and gets executed instead of denied. Comparing byte
    /// offsets (rather than just checking both are present) is what actually
    /// pins the ORDER, not merely their presence.
    #[test]
    fn the_dotfile_deny_is_ordered_before_the_php_location() {
        let c = NginxAdapter
            .generate_site_config(&unix_ctx())
            .unwrap()
            .contents;
        let deny_pos = c
            .find("location ~ /\\. {")
            .unwrap_or_else(|| panic!("dotfile deny block not found in:\n{c}"));
        let php_pos = c
            .find("location ~ \\.php$ {")
            .unwrap_or_else(|| panic!("PHP location not found in:\n{c}"));
        assert!(
            deny_pos < php_pos,
            "the dotfile deny must be listed before the PHP location, since nginx takes the \
             first matching regex location: deny at {deny_pos}, php at {php_pos} in:\n{c}"
        );
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

    /// Pins the approved spec's (§6.2) `\.php$` location, not the wider
    /// `[^/]\.php(/|$)` this used to render. The wider regex claims PATH_INFO
    /// URLs like `/index.php/admin` and 404s them via `try_files`, because
    /// `/index.php/admin` is not a file; under `\.php$` that URL does not match
    /// the PHP location at all and instead falls through to `location /`'s
    /// front-controller `try_files`, which rewrites it to `/index.php` and
    /// serves it. So a PATH_INFO URL must never be able to reach PHP-FPM's
    /// PATH_INFO machinery here — it has to reach the front controller
    /// instead — which is why no `PATH_INFO` fastcgi_param may be emitted
    /// either: `fastcgi_split_path_info`/`$fastcgi_path_info` are unreachable
    /// by construction once the location can only ever match a literal
    /// `.php` suffix.
    #[test]
    fn php_location_matches_only_a_literal_php_suffix_and_emits_no_path_info() {
        let c = NginxAdapter
            .generate_site_config(&unix_ctx())
            .unwrap()
            .contents;
        assert!(
            c.contains("location ~ \\.php$ {"),
            "expected the spec's exact `\\.php$` location, got:\n{c}"
        );
        assert!(
            !c.contains("PATH_INFO"),
            "PATH_INFO must not be emitted: a PATH_INFO URL has to fall through to the \
             front controller, not reach php-fpm's PATH_INFO handling"
        );
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
            PathBuf::from("/tmp/ovh/config/generated/nginx/sites/00-default_server.conf")
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

    #[test]
    fn no_valid_site_can_claim_the_catch_alls_filename() {
        // The catch-all's name contains `_`, which is outside the hostname charset
        // RenderCtx enforces for server_name — so a site cannot be named into a
        // collision with it. This is what makes the catch-all safe without a
        // duplicate-path check anywhere in the pipeline.
        let f = NginxAdapter
            .generate_default_site_config(
                std::path::Path::new("/tmp/ovh"),
                "127.0.0.1:8080".parse().unwrap(),
                None,
            )
            .unwrap();
        let name = f.path.file_name().unwrap().to_string_lossy().into_owned();
        let stem = name.strip_suffix(".conf").unwrap();
        assert!(
            RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                stem,
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                "8.4",
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm-8.4.sock")),
                "php_x",
            )
            .is_err(),
            "a site whose server_name is {stem:?} would collide with the catch-all"
        );
    }

    #[test]
    fn worker_connections_lands_inside_the_events_block() {
        // Scope matters: nginx rejects worker_connections anywhere else.
        let c = main_conf();
        assert_eq!(
            scope_of(&c, "worker_connections").as_deref(),
            Some(["events".to_string()].as_slice()),
            "worker_connections must sit inside `events`, got:\n{c}"
        );
    }

    #[test]
    fn every_http_scoped_directive_sits_directly_inside_http() {
        // Not "appears somewhere in the file": `fastcgi_read_timeout` nested
        // one block deeper (inside `types`, say) or hoisted to the top level
        // is a different config, and only its SCOPE distinguishes the two.
        let c = main_conf();
        for directive in [
            "client_max_body_size",
            "keepalive_timeout",
            "tcp_nodelay",
            "fastcgi_connect_timeout",
            "fastcgi_send_timeout",
            "fastcgi_read_timeout",
            "gzip",
        ] {
            assert_eq!(
                scope_of(&c, directive).as_deref(),
                Some(["http".to_string()].as_slice()),
                "{directive} must sit directly inside `http`, got:\n{c}"
            );
        }
    }

    #[test]
    fn the_http_level_settings_are_all_rendered() {
        let c = main_conf();
        assert!(c.contains("client_max_body_size 256m;"));
        assert!(c.contains("keepalive_timeout 65;"));
        assert!(c.contains("tcp_nodelay on;"));
        assert!(c.contains("fastcgi_connect_timeout 60;"));
        assert!(c.contains("fastcgi_send_timeout 300;"));
        assert!(c.contains("fastcgi_read_timeout 300;"));
        assert!(c.contains("gzip off;"));
        assert!(c.contains("worker_connections 1024;"));
    }

    #[test]
    fn a_changed_setting_changes_the_output() {
        // Guards the whole point of the slice: if the template ignored the
        // struct and kept its literals, every other test here would still pass.
        let s = WebServerSettings {
            fastcgi_read_timeout: Seconds::parse(900).unwrap(),
            ..WebServerSettings::default()
        };
        let c = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap()
            .contents;
        assert!(c.contains("fastcgi_read_timeout 900;"));
        assert!(!c.contains("fastcgi_read_timeout 300;"));
    }

    #[test]
    fn every_setting_is_reachable_from_the_struct() {
        // `a_changed_setting_changes_the_output` proves ONE field is wired.
        // A field the template forgot would still render its default here and
        // pass every literal assertion above, so change all of them at once
        // and require the output to differ field by field.
        let s = WebServerSettings {
            worker_connections: WorkerConnections::parse(2048).unwrap(),
            client_max_body_size: BodySize::parse("7m").unwrap(),
            keepalive_timeout: Seconds::parse(11).unwrap(),
            tcp_nodelay: OnOff::new(false),
            fastcgi_connect_timeout: Seconds::parse(22).unwrap(),
            fastcgi_send_timeout: Seconds::parse(33).unwrap(),
            fastcgi_read_timeout: Seconds::parse(44).unwrap(),
            gzip: OnOff::new(true),
            gzip_comp_level: GzipLevel::parse(6).unwrap(),
            gzip_types: GzipTypes::parse("text/x-component").unwrap(),
        };
        let c = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap()
            .contents;
        for expected in [
            "worker_connections 2048;",
            "client_max_body_size 7m;",
            "keepalive_timeout 11;",
            "tcp_nodelay off;",
            "fastcgi_connect_timeout 22;",
            "fastcgi_send_timeout 33;",
            "fastcgi_read_timeout 44;",
            "gzip on;",
            "gzip_comp_level 6;",
            "gzip_types text/x-component;",
        ] {
            assert!(c.contains(expected), "missing {expected:?} in:\n{c}");
        }
    }

    #[test]
    fn gzip_directives_appear_only_when_gzip_is_on() {
        let off = main_conf();
        assert!(off.contains("gzip off;"));
        assert!(
            !off.contains("gzip_types"),
            "no point listing types with gzip off"
        );
        assert!(!off.contains("gzip_comp_level"));
        // No phantom blank line where the gzip extras would have gone: toggling
        // gzip must diff as exactly the lines that changed.
        assert!(
            off.contains("    gzip off;\n    types {"),
            "gzip off left a stray blank line behind:\n{off}"
        );

        let s = WebServerSettings {
            gzip: OnOff::new(true),
            ..WebServerSettings::default()
        };
        let on = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap()
            .contents;
        assert!(on.contains("gzip on;"));
        assert!(on.contains("gzip_comp_level 1;"));
        assert!(on.contains("gzip_types text/plain"));
        assert_eq!(
            scope_of(&on, "gzip_comp_level").as_deref(),
            Some(["http".to_string()].as_slice())
        );
        assert_eq!(
            scope_of(&on, "gzip_types").as_deref(),
            Some(["http".to_string()].as_slice())
        );
    }

    #[test]
    fn an_empty_gzip_types_list_emits_no_gzip_types_directive_at_all() {
        // A blank input parses to an EMPTY list on purpose — "compress nothing
        // beyond nginx's built-in text/html". The honest rendering of that is
        // no directive; a bare `gzip_types;` is a syntax error nginx refuses
        // to start on, which would turn a legitimate setting into a config the
        // user cannot apply.
        let s = WebServerSettings {
            gzip: OnOff::new(true),
            gzip_types: GzipTypes::parse("   ").unwrap(),
            ..WebServerSettings::default()
        };
        let c = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap()
            .contents;
        assert!(c.contains("gzip on;"));
        assert!(c.contains("gzip_comp_level 1;"));
        assert!(
            !c.contains("gzip_types"),
            "an empty list must omit the directive, not emit a bare one:\n{c}"
        );
    }

    #[test]
    fn generation_stays_deterministic_for_non_default_settings() {
        let s = WebServerSettings {
            gzip: OnOff::new(true),
            gzip_types: GzipTypes::parse("text/plain application/json").unwrap(),
            ..WebServerSettings::default()
        };
        let a = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap();
        let b = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn every_rendered_setting_line_is_terminated() {
        // A directive missing its `;` swallows the following line into itself:
        // nginx either rejects the file or, worse, reads something other than
        // what the template meant.
        let s = WebServerSettings {
            gzip: OnOff::new(true),
            ..WebServerSettings::default()
        };
        let c = NginxAdapter
            .generate_main_config(Path::new("/tmp/ovh"), &s)
            .unwrap()
            .contents;
        for line in c.lines() {
            let t = line.trim();
            if t.is_empty() || t.starts_with('#') || t.ends_with('{') || t == "}" {
                continue;
            }
            assert!(t.ends_with(';'), "unterminated directive line: {line:?}");
        }
    }
}
