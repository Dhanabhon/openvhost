// SPDX-License-Identifier: GPL-3.0-or-later
//! End-to-end: apply a site, start the real stack, and prove the three things
//! the generated config must get right — PHP runs, static files keep their MIME
//! type, and a non-file path is never executed — plus the coverage the deleted
//! P0-4 test (`macos_stack.rs`, retired in Task 6) used to carry: real nginx
//! and real php-fpm actually complete the FastCGI handshake over the unix
//! socket, and both processes shut down cleanly with no leftover holding the
//! port.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use openvhost_conf::{WebServerSettings, probe_php_fpm_version};
use openvhost_core::platform::macos::demo_stack::{find_brew_binaries, provision_home};
use openvhost_core::site::apply::LISTEN_PORT;
use openvhost_core::{
    ApplyInput, Docroot, Domain, InstalledRuntimes, LogPaths, NginxValidator, PhpRuntime,
    PhpRuntimeSource, PhpVersion, Site, SiteId, SiteName, WebServer, apply, nginx_prefix_dir,
    nginx_spawn_argv, plan,
};

/// Both nginx (`worker_processes 1`) and php-fpm (`pm = ondemand`) fork
/// children of their own. A plain `Child::kill()` only signals the master we
/// spawned directly — SIGKILL does not propagate to its children, so the
/// worker/pool process survives as an orphan, still bound to the port. The
/// fix is process-group semantics on both ends: [`spawn_in_new_group`] makes
/// the spawned master the leader of its own process group (pgid == its pid),
/// and `Killed::drop` signals the WHOLE group (`kill -9 -<pid>`) rather than
/// just the one process.
fn spawn_in_new_group(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.process_group(0).spawn()
}

/// Kills the child's entire process group on drop, so a failed assertion
/// cannot leave a stray nginx worker or php-fpm pool process holding the
/// port.
struct Killed(Child);
impl Drop for Killed {
    fn drop(&mut self) {
        let pid = self.0.id();
        let _ = Command::new("kill")
            .args(["-9", &format!("-{pid}")])
            .status();
        let _ = self.0.wait();
    }
}

/// Minimal HTTP/1.0 GET. Raw TCP keeps this test dependency-free.
///
/// `deadline` used to be checked only BETWEEN connection attempts: once
/// `connect` succeeded, `read_to_string` had no timeout at all, so a wedged
/// FastCGI backend — exactly the failure this test exists to catch — would
/// hang the read forever instead of failing the test. A deadline that cannot
/// fire is as useless as an assertion that cannot fail, so `set_read_timeout`
/// bounds the read to whatever time is left before `deadline`, and a stalled
/// backend now surfaces as a normal timed-out `get()` (returning `None`)
/// rather than a CI job timeout with no diagnostic.
fn get(port: u16, host: &str, path: &str, deadline: Instant) -> Option<String> {
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            // A zero duration means "no timeout at all" to `set_read_timeout`,
            // which is the exact hang this exists to prevent — floor it to a
            // small positive value instead of passing `remaining` through
            // unchecked.
            let read_timeout = remaining.max(Duration::from_millis(100));
            let _ = s.set_read_timeout(Some(read_timeout));
            let req = format!("GET {path} HTTP/1.0\r\nHost: {host}\r\n\r\n");
            if s.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                if s.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    None
}

/// `kill -0 <pid>`: succeeds only if the process still exists. Used to prove
/// the `Killed` drop guard actually reaped its child rather than merely
/// sending a signal into the void.
fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tokio::test]
async fn site_apply_serves_a_real_site_end_to_end() {
    // This test binds the real LISTEN_PORT, so it cannot run alongside the
    // OpenVHost app (or anything else) already listening there. Check rather
    // than assume, and fail with a message that says so plainly — a bare
    // "no response" from a doomed nginx spawn is an hour of someone else's
    // life.
    match TcpListener::bind(("127.0.0.1", LISTEN_PORT)) {
        Ok(probe) => drop(probe),
        Err(e) => panic!(
            "cannot bind 127.0.0.1:{LISTEN_PORT} for this test ({e}). Something else is \
             already listening on port {LISTEN_PORT} — most likely the OpenVHost app itself, \
             or a leftover nginx from a previous failed run. Quit it and re-run \
             `cargo test -p openvhost-core --test site_apply_e2e`."
        ),
    }

    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP site_apply_e2e: Homebrew nginx/php-fpm not found (brew install nginx php)");
        return;
    };

    let Some(major) = probe_php_fpm_version(&brew.php_fpm).await else {
        eprintln!(
            "SKIP site_apply_e2e: could not probe a php-fpm version from {}",
            brew.php_fpm.display()
        );
        return;
    };

    // Short /tmp base — NOT the default TMPDIR (/var/folders/.../T/...), which
    // on macOS can push `<home>/run/php-fpm-<major>.sock` past the 103-byte
    // `sun_path` ceiling. `render_set` checks this for us on every `plan()`
    // call and fails clearly if it's ever wrong, but starting short avoids
    // failing this whole test over an environment accident.
    let home = tempfile::Builder::new()
        .prefix("ovh")
        .tempdir_in("/tmp")
        .unwrap_or_else(|e| panic!("failed to create a short-path home under /tmp: {e}"));
    provision_home(home.path()).unwrap_or_else(|e| panic!("provision_home failed: {e}"));

    // The docroot lives in its OWN temp dir, not inside the OpenVHost home —
    // a real user's project folder lives elsewhere, and nesting it under home
    // would let a path bug (docroot accidentally resolving relative to home)
    // pass unnoticed.
    let docroot = tempfile::tempdir().unwrap_or_else(|e| panic!("failed to create docroot: {e}"));
    std::fs::write(
        docroot.path().join("index.php"),
        "<?php echo \"PHP-OK \" . PHP_VERSION;",
    )
    .unwrap();
    std::fs::write(docroot.path().join("style.css"), "body { color: red; }").unwrap();

    let site = Site {
        id: SiteId::new(),
        name: SiteName::parse("e2e").unwrap(),
        domain: Domain::parse("e2e.localhost").unwrap(),
        docroot: Docroot::parse(docroot.path().to_str().expect("docroot path must be UTF-8"))
            .unwrap(),
        web_server: WebServer::parse("nginx").unwrap(),
        php_version: PhpVersion::parse(&major).unwrap(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };

    let input = ApplyInput {
        home: home.path().to_path_buf(),
        sites: vec![site],
        runtimes: InstalledRuntimes {
            nginx_bin: Some(brew.nginx.clone()),
            php: vec![PhpRuntime {
                major: major.clone(),
                fpm_bin: brew.php_fpm.clone(),
                // `find_brew_binaries` is where `brew.php_fpm` came from, so
                // this states the truth about this fixture rather than filling
                // a field: the test proves exactly what it proved before.
                source: PhpRuntimeSource::Homebrew,
            }],
        },
        // The defaults, deliberately: this test proves the pipeline serves a
        // real request end to end, not that any particular setting renders.
        settings: WebServerSettings::default(),
        // No preference, for the same reason: this fixture describes the
        // machine it always described, so what this test proves is unchanged.
        default_php: None,
    };

    let site_plan = plan(&input).unwrap_or_else(|e| panic!("plan() failed: {e}"));
    let main_conf = site_plan.main_conf.clone();
    let err_log = LogPaths::new(home.path()).nginx_error();
    let validator = NginxValidator {
        bin: brew.nginx.clone(),
        err_log: err_log.clone(),
        // `-p`'s target — see `NginxValidator::home`'s own doc comment (4B
        // fix-wave, item 1). Never `home.path()` itself, so this validator
        // proves the SAME invocation shape production uses.
        home: nginx_prefix_dir(home.path()),
    };
    let outcome = apply(&site_plan, &validator).await;
    assert!(outcome.is_ok(), "apply() was rejected: {:?}", outcome.err());

    let fpm_conf = home
        .path()
        .join(format!("config/generated/php/{major}/php-fpm.conf"));

    let mut php_fpm_cmd = Command::new(&brew.php_fpm);
    php_fpm_cmd.args(["-F", "-O", "-n", "-y"]).arg(&fpm_conf);
    let php_fpm_child = spawn_in_new_group(&mut php_fpm_cmd)
        .unwrap_or_else(|e| panic!("failed to spawn php-fpm ({}): {e}", brew.php_fpm.display()));
    let php_fpm_pid = php_fpm_child.id();
    let php_fpm_guard = Killed(php_fpm_child);

    // The socket file is the FastCGI handshake's precondition: its absence is
    // the single most common cause of an endless 502. Prove it exists before
    // ever asking nginx to talk to it.
    let sock_path = home.path().join(format!("run/php-fpm-{major}.sock"));
    let sock_deadline = Instant::now() + Duration::from_secs(10);
    while !sock_path.exists() {
        assert!(
            Instant::now() < sock_deadline,
            "php-fpm never created its socket at {} — the FastCGI handshake cannot happen \
             without it",
            sock_path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let mut nginx_cmd = Command::new(&brew.nginx);
    // THE production argv (4B fix-wave, item 3): this used to be a
    // hand-written copy of `stack.rs::nginx_spec`'s args that had silently
    // dropped `-p`, with nothing in the regression net able to notice.
    // Building it through `nginx_spawn_argv` — the SAME function
    // `nginx_spec` itself calls — makes that drift impossible now.
    nginx_cmd.args(nginx_spawn_argv(home.path(), &main_conf));
    let nginx_child = spawn_in_new_group(&mut nginx_cmd)
        .unwrap_or_else(|e| panic!("failed to spawn nginx ({}): {e}", brew.nginx.display()));
    let nginx_pid = nginx_child.id();
    let nginx_guard = Killed(nginx_child);

    let deadline = Instant::now() + Duration::from_secs(10);

    let php = get(LISTEN_PORT, "e2e.localhost", "/index.php", deadline).expect(
        "no response from nginx on the FastCGI-backed site request — is port 8080 already in \
         use by something else, or did nginx fail to bind it?",
    );
    assert!(php.contains("PHP-OK"), "PHP did not execute:\n{php}");

    let css = get(LISTEN_PORT, "e2e.localhost", "/style.css", deadline).unwrap();
    // Regression: without the types{} block this is application/octet-stream
    // and the browser refuses to apply the stylesheet.
    assert!(
        css.contains("Content-Type: text/css"),
        "wrong MIME type:\n{css}"
    );
    assert!(css.contains("color: red"));

    // Regression: `try_files $uri =404` in the PHP location is what stops this
    // at the nginx layer. Without it, nginx would hand the request to php-fpm
    // at all, and what happens next is NOT "style.css gets executed as PHP" —
    // php-fpm's own `security.limit_extensions` default (which OpenVHost does
    // not otherwise set) refuses to execute a script that isn't named `.php`,
    // so the observed outcome one layer further down is a 403, not execution.
    // This project does not want to rely on an inherited php-fpm default for
    // that refusal, so `security.limit_extensions = .php` is now stated
    // explicitly in the generated pool config (see pool.conf.tera and its
    // adapter test) — this assertion is what proves nginx itself never lets
    // the request reach that far in the first place.
    let exploit = get(LISTEN_PORT, "e2e.localhost", "/style.css/x.php", deadline).unwrap();
    assert!(
        exploit.starts_with("HTTP/1.1 404"),
        "path-info guard failed:\n{exploit}"
    );

    // The catch-all answers a host that matches no site, and PHP really ran:
    // the landing page echoes PHP_VERSION, so a literal "PHP_VERSION" in the
    // response would mean the file was served as text instead of executed.
    let fallback = get(LISTEN_PORT, "nothing.localhost", "/index.php", deadline).unwrap();
    assert!(
        fallback.contains("OpenVHost is running"),
        "catch-all did not serve the landing page:\n{fallback}"
    );
    assert!(
        !fallback.contains("PHP_VERSION"),
        "the landing page was served as text, not executed:\n{fallback}"
    );
    // A version PHP would actually print, e.g. "PHP 8.4.23 answered".
    assert!(
        fallback.contains(&format!("PHP {major}.")),
        "the catch-all did not report the installed PHP version:\n{fallback}"
    );
    // Whatever else changes about this page, it must not become phpinfo() again
    // (security audit A1): the catch-all answers any unmatched Host.
    assert!(
        !fallback.contains("phpinfo()") && !fallback.contains("Configuration File (php.ini)"),
        "the catch-all is disclosing phpinfo:\n{fallback}"
    );

    // Teardown proof: drop both guards explicitly (rather than letting scope
    // exit do it implicitly), then verify the two things a failed teardown
    // would leave behind — a zombie/live process, or a still-bound port.
    drop(nginx_guard);
    drop(php_fpm_guard);

    assert!(
        !process_alive(nginx_pid),
        "nginx (pid {nginx_pid}) was not reaped when the Killed guard dropped"
    );
    assert!(
        !process_alive(php_fpm_pid),
        "php-fpm (pid {php_fpm_pid}) was not reaped when the Killed guard dropped"
    );

    let port_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match TcpListener::bind(("127.0.0.1", LISTEN_PORT)) {
            Ok(listener) => {
                drop(listener);
                break;
            }
            Err(e) => {
                assert!(
                    Instant::now() < port_deadline,
                    "port {LISTEN_PORT} is still held after both processes were killed: {e}"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}
