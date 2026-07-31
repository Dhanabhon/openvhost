// SPDX-License-Identifier: GPL-3.0-or-later
//! Live, real-binary proof for the P1 live-log-viewer slice (plan Task 7,
//! `docs/superpowers/specs/2026-07-30-p1-log-viewer-design.md`) — the four
//! behaviors two fix waves just changed, proven against real nginx + real
//! php-fpm rather than a rendered string or a synthetic fixture:
//!
//! 1. A front-controller request logs its ORIGINAL path in the site's
//!    access log, never the internal `/index.php` rewrite target, and never
//!    its query string (spec D5's `$request_path` fix + privacy guarantee).
//! 2. A real PHP fatal is findable in the SITE's own error log via nginx's
//!    `FastCGI sent in stderr: "PHP message: …"` capture (spec D1a's
//!    fallback path — the slice's core user story).
//! 3. [`openvhost_core::read_window`] finds a match OLDER than a plain tail
//!    on the REAL access log this test just produced (spec D4 — "the design
//!    decision the whole slice rests on").
//! 4. Two sites served by the same nginx write to their own
//!    `logs/sites/<domain>/` files — no cross-contamination (spec D1).
//!
//! Also asserts the per-site log directories are `0700` (spec D5) — the
//! apply pipeline is what creates them, so that is checked right after
//! `apply()`, before any process starts.
//!
//! # Item 2 required a real product fix, found by this exact live proof
//!
//! Writing this test against the code as it stood found that item 2 did NOT
//! hold: php-fpm is launched with `-n` (no php.ini — `stack.rs::php_fpm_spec`'s
//! real invocation, mirrored by this file's own `php_fpm_cmd`), which leaves
//! PHP's compiled-in default for `log_errors` in effect (`Off`), so an
//! uncaught fatal was captured in NO log at all — confirmed independently at
//! the raw FastCGI-protocol level, bypassing nginx entirely: the baseline
//! pool config sent zero STDERR bytes for an uncaught fatal, and its own
//! `error_log` file stayed empty; only the raw HTTP response body (via
//! `display_errors`, On by default) showed anything. `pool.conf.tera` now
//! sets `php_admin_value[log_errors] = On` and
//! `php_admin_value[display_errors] = Off` (see that file's own comment and
//! `phpruntime.rs`'s
//! `pool_config_captures_fatals_instead_of_only_disclosing_them_in_the_response_body`
//! for the fast unit-level regression) — the second directive also closes an
//! information-disclosure gap (full file-system paths and a stack trace,
//! visible to any client on 127.0.0.1:8080) of the same class already
//! treated as a security concern for the phpinfo() catch-all (security audit
//! A1). **This template change touches PHP error-disclosure behavior and has
//! not had a security-auditor look — flagged for that review before this
//! branch merges to main**, per this project's established pattern for
//! exactly this class of change.
//!
//! # Sibling of `site_apply_e2e.rs`, not an extension of it
//!
//! That file proves the generated config SERVES correctly (PHP runs, MIME
//! types are right, a non-file path is never executed) plus the P0-4
//! process-group teardown contract. This file proves a materially different
//! thing — what actually lands in the LOG FILES that config produces, plus
//! the bounded reader that reads them back — against a *second*,
//! purpose-built stack (two sites, a front-controller router, a deliberate
//! fatal). Folding this in would make one file cover two independent
//! concerns; `mysql_live.rs` sets the same precedent for this crate (a
//! dedicated live file per slice's live proof, re-deriving small helpers
//! rather than sharing a test-utils module — see its own module doc).
//! [`spawn_in_new_group`], [`Killed`], [`get`] and [`process_alive`] below
//! are therefore mirrored byte-for-byte from `site_apply_e2e.rs`, not
//! imported from it (integration test files are separate crates; there is
//! no clean import path between them without a shared `tests/common/`
//! module neither file currently uses).
//!
//! # Skip gate — mirrors `site_apply_e2e.rs` exactly, not `mysql_live.rs`
//!
//! Binary-presence-only: no opt-in env var. Serving a handful of HTTP
//! requests against nginx + php-fpm is the same weight class as
//! `site_apply_e2e.rs` (which already runs on every plain
//! `cargo test --workspace`), unlike `mysql_live.rs`'s real `mysqld`
//! lifecycle (init + start + stop), which is why THAT file gates behind
//! `OPENVHOST_MYSQL_LIVE_TESTS=1`.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use openvhost_conf::{WebServerSettings, probe_php_fpm_version};
use openvhost_core::platform::macos::demo_stack::{find_brew_binaries, provision_home};
use openvhost_core::site::apply::LISTEN_PORT;
use openvhost_core::{
    ApplyInput, Docroot, Domain, InstalledRuntimes, LogLimits, LogPaths, LogQuery, NginxValidator,
    PhpRuntime, PhpVersion, Site, SiteId, SiteName, WebServer, apply, plan, read_window,
};

// ---------------------------------------------------------------------------
// Helpers — mirrored from `site_apply_e2e.rs` (see module doc for why they
// are re-derived here rather than shared).
// ---------------------------------------------------------------------------

/// Both nginx (`worker_processes 1`) and php-fpm (`pm = ondemand`) fork
/// children of their own — mirrors `site_apply_e2e.rs::spawn_in_new_group`
/// exactly: makes the spawned master the leader of its own process group so
/// [`Killed`] can signal the WHOLE group, not just the process we spawned
/// directly.
fn spawn_in_new_group(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.process_group(0).spawn()
}

/// Kills the child's entire process group on drop — mirrors
/// `site_apply_e2e.rs::Killed` exactly, so a failed assertion here cannot
/// leave a stray nginx worker or php-fpm pool process holding the port
/// either.
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

/// Minimal HTTP/1.0 GET, dependency-free — mirrors `site_apply_e2e.rs::get`
/// exactly, including its bounded-read-timeout fix (once `connect` succeeds,
/// `read_to_string` still needs its OWN timeout, or a wedged FastCGI backend
/// hangs the read forever instead of failing the test).
fn get(port: u16, host: &str, path: &str, deadline: Instant) -> Option<String> {
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let remaining = deadline.saturating_duration_since(Instant::now());
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

/// `kill -0 <pid>` — mirrors `site_apply_e2e.rs::process_alive` exactly.
fn process_alive(pid: u32) -> bool {
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Poll `path`'s full contents until `pred` holds or `deadline` elapses.
/// Neither `access_log` nor `error_log` this pipeline renders sets
/// `buffer=`, so nginx writes each line unbuffered — but the write()
/// syscall and this read can still race by a few milliseconds right after
/// `get()` returns. This is the "never sleep-and-hope" poll for that race
/// (bounded, with a deadline), not a fixed sleep. Panics with the
/// last-seen content on timeout so a genuine miss is diagnosable rather
/// than a bare assertion failure.
fn wait_for_content(
    path: &Path,
    what: &str,
    deadline: Instant,
    pred: impl Fn(&str) -> bool,
) -> String {
    loop {
        let last = std::fs::read_to_string(path).unwrap_or_default();
        if pred(&last) {
            return last;
        }
        if Instant::now() >= deadline {
            panic!(
                "timed out waiting for {what} in {}; last content:\n{last}",
                path.display()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Site A's docroot: a tiny front-controller router. Any path other than
/// `/boom` exercises item 1 (the original-path logging fix); `/boom`
/// exercises item 2 (a real, uncaught PHP fatal — `Call to undefined
/// function`, always fatal on every supported PHP 7/8 major, never merely a
/// warning).
const INDEX_PHP_A: &str = r#"<?php
$path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH);
if ($path === '/boom') {
    ovh_log_proof_undefined_fn();
}
echo "SITE-A-OK " . $path;
"#;

/// Site B's docroot: deliberately trivial — its only job is to exist as a
/// SECOND site so item 4 (per-site attribution) has two real log files to
/// compare.
const INDEX_PHP_B: &str = "<?php\necho \"SITE-B-OK\";\n";

#[tokio::test]
async fn per_site_logs_capture_the_real_request_and_the_fatal() {
    // Same port guard as `site_apply_e2e.rs`, same reason: this binds the
    // real LISTEN_PORT, so it cannot run alongside the OpenVHost app or a
    // leftover nginx from a previous failed run.
    match TcpListener::bind(("127.0.0.1", LISTEN_PORT)) {
        Ok(probe) => drop(probe),
        Err(e) => panic!(
            "cannot bind 127.0.0.1:{LISTEN_PORT} for this test ({e}). Something else is \
             already listening on port {LISTEN_PORT} — most likely the OpenVHost app itself, \
             or a leftover nginx from a previous run. Quit it and re-run \
             `cargo test -p openvhost-core --test site_logs_e2e`."
        ),
    }

    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP site_logs_e2e: Homebrew nginx/php-fpm not found (brew install nginx php)");
        return;
    };
    let Some(major) = probe_php_fpm_version(&brew.php_fpm).await else {
        eprintln!(
            "SKIP site_logs_e2e: could not probe a php-fpm version from {}",
            brew.php_fpm.display()
        );
        return;
    };

    // Short /tmp base, not TMPDIR (/var/folders/.../T/...) — same
    // `sun_path` headroom reason as `site_apply_e2e.rs`: a php-fpm unix
    // socket path under a long TMPDIR can exceed the 103-byte ceiling.
    let home = tempfile::Builder::new()
        .prefix("ovhlogs")
        .tempdir_in("/tmp")
        .unwrap_or_else(|e| panic!("failed to create a short-path home under /tmp: {e}"));
    provision_home(home.path()).unwrap_or_else(|e| panic!("provision_home failed: {e}"));

    // Each site's docroot lives in its own temp dir, outside the OpenVHost
    // home — mirrors `site_apply_e2e.rs`'s identical reasoning (a real
    // user's project folder lives elsewhere; nesting it under home would let
    // a docroot-resolves-relative-to-home path bug pass unnoticed).
    let docroot_a = tempfile::tempdir().expect("failed to create site A's docroot");
    std::fs::write(docroot_a.path().join("index.php"), INDEX_PHP_A).expect("write index.php (A)");
    let docroot_b = tempfile::tempdir().expect("failed to create site B's docroot");
    std::fs::write(docroot_b.path().join("index.php"), INDEX_PHP_B).expect("write index.php (B)");

    let site_a = Site {
        id: SiteId::new(),
        name: SiteName::parse("logs-a").unwrap(),
        domain: Domain::parse("logs-a.localhost").unwrap(),
        docroot: Docroot::parse(
            docroot_a
                .path()
                .to_str()
                .expect("docroot path must be UTF-8"),
        )
        .unwrap(),
        web_server: WebServer::parse("nginx").unwrap(),
        php_version: PhpVersion::parse(&major).unwrap(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };
    let site_b = Site {
        id: SiteId::new(),
        name: SiteName::parse("logs-b").unwrap(),
        domain: Domain::parse("logs-b.localhost").unwrap(),
        docroot: Docroot::parse(
            docroot_b
                .path()
                .to_str()
                .expect("docroot path must be UTF-8"),
        )
        .unwrap(),
        web_server: WebServer::parse("nginx").unwrap(),
        php_version: PhpVersion::parse(&major).unwrap(),
        enabled: true,
        created_at: 0,
        updated_at: 0,
    };

    let input = ApplyInput {
        home: home.path().to_path_buf(),
        sites: vec![site_a.clone(), site_b.clone()],
        runtimes: InstalledRuntimes {
            nginx_bin: brew.nginx.clone(),
            php: vec![PhpRuntime {
                major: major.clone(),
                fpm_bin: brew.php_fpm.clone(),
            }],
        },
        // The defaults, deliberately — same reasoning as `site_apply_e2e.rs`:
        // this test proves what the logs pipeline does for a normal apply,
        // not that any particular setting renders.
        settings: WebServerSettings::default(),
    };

    let site_plan = plan(&input).unwrap_or_else(|e| panic!("plan() failed: {e}"));
    let main_conf = site_plan.main_conf.clone();
    let log_paths = LogPaths::new(home.path());
    let err_log = log_paths.nginx_error();
    let validator = NginxValidator {
        bin: brew.nginx.clone(),
        err_log: err_log.clone(),
    };
    let outcome = apply(&site_plan, &validator).await;
    assert!(outcome.is_ok(), "apply() was rejected: {:?}", outcome.err());

    // Spec D5: log directories are 0700 explicitly. The apply pipeline
    // (`commit()`) is what creates them, so this is asserted right here —
    // before any process has even started — for BOTH sites, not merely one.
    {
        use std::os::unix::fs::PermissionsExt;
        for domain in [&site_a.domain, &site_b.domain] {
            let dir = log_paths.site_dir(domain);
            let mode = std::fs::metadata(&dir)
                .unwrap_or_else(|e| panic!("{dir:?} must exist after apply(): {e}"))
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o700, "{dir:?} must be 0700 (spec D5), got {mode:o}");
        }
    }

    let access_log_a = log_paths.site_access(&site_a.domain);
    let error_log_a = log_paths.site_error(&site_a.domain);
    let access_log_b = log_paths.site_access(&site_b.domain);

    // ---- Start the real stack (mirrors `site_apply_e2e.rs` exactly) ----
    let fpm_conf = home
        .path()
        .join(format!("config/generated/php/{major}/php-fpm.conf"));
    let mut php_fpm_cmd = Command::new(&brew.php_fpm);
    php_fpm_cmd.args(["-F", "-O", "-n", "-y"]).arg(&fpm_conf);
    let php_fpm_child = spawn_in_new_group(&mut php_fpm_cmd)
        .unwrap_or_else(|e| panic!("failed to spawn php-fpm ({}): {e}", brew.php_fpm.display()));
    let php_fpm_pid = php_fpm_child.id();
    let php_fpm_guard = Killed(php_fpm_child);

    let sock_path = home.path().join(format!("run/php-fpm-{major}.sock"));
    let sock_deadline = Instant::now() + Duration::from_secs(10);
    while !sock_path.exists() {
        assert!(
            Instant::now() < sock_deadline,
            "php-fpm never created its socket at {} within 10s — the FastCGI handshake cannot \
             happen without it",
            sock_path.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let mut nginx_cmd = Command::new(&brew.nginx);
    nginx_cmd.arg("-e").arg(&err_log).arg("-c").arg(&main_conf);
    let nginx_child = spawn_in_new_group(&mut nginx_cmd)
        .unwrap_or_else(|e| panic!("failed to spawn nginx ({}): {e}", brew.nginx.display()));
    let nginx_pid = nginx_child.id();
    let nginx_guard = Killed(nginx_child);

    // ==== Item 1: the ORIGINAL path is logged — never /index.php, never the
    // query string (spec D5's $request_path fix + privacy guarantee). ====
    let deadline = Instant::now() + Duration::from_secs(10);
    let first = get(
        LISTEN_PORT,
        "logs-a.localhost",
        "/hello/world?token=abc123",
        deadline,
    )
    .expect(
        "no response from nginx for /hello/world — is port 8080 already in use, or did nginx \
         fail to bind it?",
    );
    assert!(first.contains("SITE-A-OK"), "PHP did not execute:\n{first}");

    let content = wait_for_content(
        &access_log_a,
        "the /hello/world request",
        Instant::now() + Duration::from_secs(10),
        |c| c.contains("/hello/world"),
    );
    assert!(
        content.contains(r#""GET /hello/world HTTP/1.0""#),
        "expected the ORIGINAL, pre-rewrite request path in the access log, got:\n{content}"
    );
    assert!(
        !content.contains("/index.php"),
        "the access log must record the path actually requested, never the internal \
         front-controller rewrite target `try_files … /index.php$is_args$args` sent it to:\n\
         {content}"
    );
    assert!(
        !content.contains("token") && !content.contains("abc123"),
        "the access log must never carry a query string (spec D5 privacy guarantee) — neither \
         the param name nor its value may appear anywhere in the line:\n{content}"
    );

    // ==== Item 2: a real PHP fatal is findable in the SITE's own error log
    // (spec D1a's fallback path — the slice's core user story). ====
    get(LISTEN_PORT, "logs-a.localhost", "/boom", deadline)
        .expect("no response from nginx for /boom");

    let err_content = wait_for_content(
        &error_log_a,
        "the PHP fatal from /boom",
        Instant::now() + Duration::from_secs(10),
        |c| c.contains("ovh_log_proof_undefined_fn"),
    );
    assert!(
        err_content.contains("FastCGI sent in stderr"),
        "expected nginx's FastCGI stderr capture in the SITE's error log, got:\n{err_content}"
    );
    assert!(
        err_content.contains("PHP message:"),
        "expected a 'PHP message:' capture in the site error log, got:\n{err_content}"
    );
    assert!(
        err_content.contains("Fatal error"),
        "expected 'Fatal error' in the site error log, got:\n{err_content}"
    );

    // ==== Item 4: per-site attribution — a request to A never appears in
    // B's log, and vice versa (spec D1). ====
    let b_resp = get(LISTEN_PORT, "logs-b.localhost", "/", deadline)
        .expect("no response from nginx for site B");
    assert!(
        b_resp.contains("SITE-B-OK"),
        "site B did not execute PHP:\n{b_resp}"
    );

    let content_b = wait_for_content(
        &access_log_b,
        "site B's own request",
        Instant::now() + Duration::from_secs(10),
        |c| c.contains(r#""GET / HTTP/1.0""#),
    );
    assert!(
        !content_b.contains("/hello/world") && !content_b.contains("/boom"),
        "site B's access log must not contain ANY of site A's requests:\n{content_b}"
    );
    let content_a_now = std::fs::read_to_string(&access_log_a).unwrap_or_default();
    assert!(
        !content_a_now.contains(r#""GET / HTTP/1.0""#),
        "site A's access log must not contain site B's request:\n{content_a_now}"
    );

    // ==== Item 3: the reader finds a match OLDER than a plain tail (spec
    // D4 — the design decision the whole slice rests on), proven on the
    // REAL access log just produced above, not a synthetic fixture. ====
    //
    // Only `payload` is shrunk here, and explicitly so: the production
    // default (512 KiB, spec D3) would need several thousand real HTTP
    // round-trips to push the very first line ("/hello/world", item 1's own
    // subject) out of a plain tail inside a bounded test run. `scan` — the
    // ACTUAL scan-back budget spec D4 is about, and the thing this item
    // exists to prove — stays at `LogLimits::default()` (16 MiB), so this
    // still exercises the real mechanism at its real production size, not a
    // shrunk stand-in for it.
    const FILLER_REQUESTS: usize = 25;
    let filler_deadline = Instant::now() + Duration::from_secs(20);
    for i in 0..FILLER_REQUESTS {
        get(
            LISTEN_PORT,
            "logs-a.localhost",
            &format!("/filler-{i}"),
            filler_deadline,
        )
        .unwrap_or_else(|| panic!("filler request {i} to site A got no response"));
    }

    let limits = LogLimits {
        payload: 256,
        ..LogLimits::default()
    };

    let plain = read_window(&access_log_a, None, &LogQuery::default(), &limits)
        .unwrap_or_else(|e| panic!("read_window (plain tail) failed: {e}"));
    assert!(
        !plain.rows.is_empty(),
        "the plain tail returned no rows at all — the negative assertion below would be \
         vacuous; got: {plain:?}"
    );
    assert!(
        plain.rows.iter().all(|r| !r.text.contains("/hello/world")),
        "a plain tail (payload={} bytes) must NOT reach back to the first request after {} \
         filler lines were appended — got rows: {:?}",
        limits.payload,
        FILLER_REQUESTS,
        plain.rows
    );

    let query = LogQuery {
        needle: Some("/hello/world".to_string()),
        case_sensitive: true,
        min_level: None,
    };
    let filtered = read_window(&access_log_a, None, &query, &limits)
        .unwrap_or_else(|e| panic!("read_window (filtered) failed: {e}"));
    assert_eq!(
        filtered.rows.len(),
        1,
        "expected exactly one filtered match reaching back to the first request, got: {:?}",
        filtered.rows
    );
    assert!(
        filtered.rows[0]
            .text
            .contains(r#""GET /hello/world HTTP/1.0""#),
        "the filtered match did not carry the original path, got: {:?}",
        filtered.rows[0]
    );

    // ---- Teardown proof (mirrors `site_apply_e2e.rs` exactly) ----
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
