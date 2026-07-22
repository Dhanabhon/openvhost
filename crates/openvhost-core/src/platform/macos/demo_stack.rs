// SPDX-License-Identifier: GPL-3.0-or-later
//! P0 throwaway — superseded by openvhost-conf/Tera templates (P0-7).
//!
//! Provisions a self-contained nginx + php-fpm demo stack under the
//! OpenVHost home (spec: docs/superpowers/specs/2026-07-22-p04-macos-stack-design.md).
//! Every path written into the configs is ABSOLUTE — the stack must behave
//! identically from a terminal `dev.sh` run and a packaged .app under
//! launchd's bare GUI environment (no PATH-dependent behavior, ever).

use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Darwin `sockaddr_un.sun_path` is 104 bytes; keep one for the NUL.
pub const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Paths produced by [`provision_macos_demo_stack`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackPaths {
    pub nginx_conf: PathBuf,
    pub fpm_conf: PathBuf,
    pub docroot: PathBuf,
    pub socket: PathBuf,
    pub nginx_error_log: PathBuf,
    pub port: u16,
}

/// Homebrew binaries for the stack. Resolve at registration time: the
/// `opt/` symlinks silently retarget on major version bumps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewStack {
    pub nginx: PathBuf,
    pub php_fpm: PathBuf,
}

/// Probe the standard Homebrew prefixes (Apple Silicon first, then Intel).
pub fn find_brew_binaries() -> Option<BrewStack> {
    find_brew_binaries_in(&[Path::new("/opt/homebrew"), Path::new("/usr/local")])
}

/// Pure prober: first prefix holding BOTH binaries wins.
pub fn find_brew_binaries_in(prefixes: &[&Path]) -> Option<BrewStack> {
    prefixes.iter().find_map(|p| {
        let nginx = p.join("opt/nginx/bin/nginx");
        let php_fpm = p.join("opt/php/sbin/php-fpm");
        if nginx.is_file() && php_fpm.is_file() {
            Some(BrewStack { nginx, php_fpm })
        } else {
            None
        }
    })
}

/// nginx config. Deliberate shape (specialist-verified live, 2026-07-22):
/// - `-e` on the command line covers the pre-config-read window; this file
///   covers everything after. All five `*_temp_path` are pinned because the
///   brew build compiles ABSOLUTE `/opt/homebrew/var/...` defaults that
///   `-p` does not remap. No `mime.types` (not provisioned; FastCGI sets
///   Content-Type). fastcgi_params inlined: the proven-minimal set.
const NGINX_CONF: &str = "\
daemon off;
worker_processes 1;
pid {home}/run/nginx.pid;
error_log {home}/logs/nginx.error.log warn;
error_log stderr notice;

events {}

http {
    access_log {home}/logs/nginx.access.log;
    client_body_temp_path {home}/run/nginx/client_body;
    proxy_temp_path {home}/run/nginx/proxy;
    fastcgi_temp_path {home}/run/nginx/fastcgi;
    uwsgi_temp_path {home}/run/nginx/uwsgi;
    scgi_temp_path {home}/run/nginx/scgi;

    server {
        listen 127.0.0.1:{port};
        root {home}/www;
        location / {
            fastcgi_pass unix:{home}/run/php-fpm.sock;
            fastcgi_param SCRIPT_FILENAME $document_root/index.php;
            fastcgi_param QUERY_STRING $query_string;
            fastcgi_param REQUEST_METHOD $request_method;
            fastcgi_param CONTENT_TYPE $content_type;
            fastcgi_param CONTENT_LENGTH $content_length;
            fastcgi_param SERVER_PROTOCOL $server_protocol;
            fastcgi_param REMOTE_ADDR $remote_addr;
            fastcgi_param SERVER_NAME $server_name;
            fastcgi_param SERVER_PORT $server_port;
        }
    }
}
";

/// php-fpm config. No `user`/`group` (non-root + present = warning noise
/// that `php-fpm -t` does not surface); `pm.max_children` is REQUIRED
/// (omitting it is a startup fatal); no `pid` (no pid file needed — fpm
/// unlinks a stale socket before bind, crash recovery is self-healing).
const FPM_CONF: &str = "\
[global]
error_log = {home}/logs/php-fpm.log

[www]
listen = {home}/run/php-fpm.sock
pm = ondemand
pm.max_children = 4
catch_workers_output = yes
";

const INDEX_PHP: &str = "<?php phpinfo();\n";

/// Write the demo-stack config set under `home`, atomically and
/// overwrite-always (reruns are deterministic). Validation happens before
/// any filesystem mutation.
pub fn provision_macos_demo_stack(home: &Path, port: u16) -> Result<StackPaths, CoreError> {
    let socket = home.join("run/php-fpm.sock");
    let socket_len = socket.as_os_str().as_encoded_bytes().len();
    if socket_len > MAX_SOCKET_PATH_BYTES {
        return Err(CoreError::SocketPathTooLong {
            path: socket,
            len: socket_len,
        });
    }
    let home_str = home.to_str().ok_or_else(|| CoreError::HomeNotUtf8 {
        path: home.to_path_buf(),
    })?;

    for dir in ["conf", "www", "run", "run/nginx", "logs"] {
        let d = home.join(dir);
        std::fs::create_dir_all(&d).map_err(|source| CoreError::ProvisionIo {
            op: "create_dir_all",
            path: d.clone(),
            source,
        })?;
    }

    let nginx_conf = home.join("conf/nginx.conf");
    let fpm_conf = home.join("conf/php-fpm.conf");
    let docroot = home.join("www");

    let nginx_text = NGINX_CONF
        .replace("{home}", home_str)
        .replace("{port}", &port.to_string());
    let fpm_text = FPM_CONF.replace("{home}", home_str);

    atomic_write(&nginx_conf, &nginx_text)?;
    atomic_write(&fpm_conf, &fpm_text)?;
    atomic_write(&docroot.join("index.php"), INDEX_PHP)?;

    Ok(StackPaths {
        nginx_conf,
        fpm_conf,
        nginx_error_log: home.join("logs/nginx.error.log"),
        docroot,
        socket,
        port,
    })
}

/// Atomic write: temp file in the SAME directory as the target (same-volume
/// rename — never TMPDIR), then rename over the target.
fn atomic_write(path: &Path, contents: &str) -> Result<(), CoreError> {
    let file_name = path
        .file_name()
        .ok_or_else(|| CoreError::ProvisionIo {
            op: "file_name",
            path: path.to_path_buf(),
            source: std::io::Error::new(std::io::ErrorKind::InvalidInput, "no file name"),
        })?
        .to_string_lossy()
        .into_owned();
    let tmp = path.with_file_name(format!(".{file_name}.tmp"));
    std::fs::write(&tmp, contents).map_err(|source| CoreError::ProvisionIo {
        op: "write",
        path: tmp.clone(),
        source,
    })?;
    std::fs::rename(&tmp, path).map_err(|source| CoreError::ProvisionIo {
        op: "rename",
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Short-path tempdir: /tmp keeps socket paths far under the 104-byte
    /// Darwin limit (TMPDIR is /var/folders/... and brittle-long).
    fn short_home() -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix("ovh")
            .tempdir_in("/tmp")
            .unwrap()
    }

    #[test]
    fn provision_creates_files_and_temp_dir_parent() {
        let home = short_home();
        let paths = provision_macos_demo_stack(home.path(), 18080).unwrap();
        assert!(paths.nginx_conf.is_file());
        assert!(paths.fpm_conf.is_file());
        assert!(paths.docroot.join("index.php").is_file());
        // nginx only mkdirs the LEAF temp dir; the parent must pre-exist.
        assert!(home.path().join("run/nginx").is_dir());
        assert_eq!(paths.port, 18080);
    }

    #[test]
    fn nginx_conf_pins_all_state_inside_home() {
        let home = short_home();
        let paths = provision_macos_demo_stack(home.path(), 18081).unwrap();
        let conf = std::fs::read_to_string(&paths.nginx_conf).unwrap();
        let h = home.path().to_str().unwrap();
        assert!(conf.contains("daemon off;"));
        assert!(conf.contains("error_log stderr notice;"));
        assert!(conf.contains(&format!("pid {h}/run/nginx.pid;")));
        assert!(conf.contains("listen 127.0.0.1:18081;"));
        assert!(conf.contains(&format!("fastcgi_pass unix:{h}/run/php-fpm.sock;")));
        // All five compiled-in temp paths must be remapped (brew defaults
        // are absolute /opt/homebrew/var paths that -p does NOT fix).
        for tp in [
            "client_body_temp_path",
            "proxy_temp_path",
            "fastcgi_temp_path",
            "uwsgi_temp_path",
            "scgi_temp_path",
        ] {
            assert!(
                conf.contains(&format!("{tp} {h}/run/nginx/")),
                "missing {tp}"
            );
        }
        assert!(!conf.contains("mime.types"));
    }

    #[test]
    fn fpm_conf_is_nonroot_safe_and_complete() {
        let home = short_home();
        let paths = provision_macos_demo_stack(home.path(), 18082).unwrap();
        let conf = std::fs::read_to_string(&paths.fpm_conf).unwrap();
        let h = home.path().to_str().unwrap();
        assert!(conf.contains(&format!("listen = {h}/run/php-fpm.sock")));
        assert!(conf.contains("pm = ondemand"));
        assert!(conf.contains("pm.max_children = 4")); // omitting is a startup FATAL
        assert!(conf.contains("catch_workers_output = yes"));
        assert!(
            !conf.contains("user ="),
            "user/group cause non-root warnings"
        );
    }

    #[test]
    fn rerun_is_idempotent() {
        let home = short_home();
        let p1 = provision_macos_demo_stack(home.path(), 18083).unwrap();
        let first = std::fs::read_to_string(&p1.nginx_conf).unwrap();
        let p2 = provision_macos_demo_stack(home.path(), 18083).unwrap();
        let second = std::fs::read_to_string(&p2.nginx_conf).unwrap();
        assert_eq!(p1, p2);
        assert_eq!(first, second);
    }

    #[test]
    fn no_temp_files_left_behind() {
        let home = short_home();
        provision_macos_demo_stack(home.path(), 18084).unwrap();
        let leftovers: Vec<_> = std::fs::read_dir(home.path().join("conf"))
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "stale atomic-write temps: {leftovers:?}"
        );
    }

    #[test]
    fn socket_path_over_103_bytes_is_refused() {
        let long_home = PathBuf::from(format!("/tmp/{}", "x".repeat(120)));
        let err = provision_macos_demo_stack(&long_home, 18085).unwrap_err();
        assert!(matches!(err, CoreError::SocketPathTooLong { len, .. } if len > 103));
        // Validation precedes any filesystem writes:
        assert!(!long_home.exists());
    }

    #[test]
    fn brew_prober_requires_both_binaries_in_one_prefix() {
        let fake = short_home();
        let prefix = fake.path();
        std::fs::create_dir_all(prefix.join("opt/nginx/bin")).unwrap();
        std::fs::create_dir_all(prefix.join("opt/php/sbin")).unwrap();
        std::fs::write(prefix.join("opt/nginx/bin/nginx"), "").unwrap();
        // Only nginx present -> None.
        assert!(find_brew_binaries_in(&[prefix]).is_none());
        std::fs::write(prefix.join("opt/php/sbin/php-fpm"), "").unwrap();
        let stack = find_brew_binaries_in(&[prefix]).unwrap();
        assert_eq!(stack.nginx, prefix.join("opt/nginx/bin/nginx"));
        assert_eq!(stack.php_fpm, prefix.join("opt/php/sbin/php-fpm"));
    }
}
