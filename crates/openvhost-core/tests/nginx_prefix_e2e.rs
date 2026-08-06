// SPDX-License-Identifier: GPL-3.0-or-later
//! Live, real-binary proof for the nginx discovery 4B fix-wave's item 1 (the
//! HIGHEST-severity audit finding): `-p <home>` let a RELATIVE path in a
//! config — including one in a file the USER authored, since
//! `main.conf.tera` explicitly invites custom nginx files via
//! `include "{{ custom_sites_glob }}"` — resolve under `home` itself, which
//! holds `state.db` (MySQL/MariaDB root credentials at rest; mode `0600`
//! does not help, because nginx runs as the same user). [`nginx_prefix_dir`]
//! gives `-p` a dedicated, empty, provisioned directory instead.
//!
//! Reproduces the auditor's own live finding exactly: same shape of config
//! (`root .;`), same request (`GET /state.db`), both sides (before/after) in
//! ONE test so a future change to how `home`/the prefix relate cannot make
//! only one half compile or skip. Skip gate and helper style mirror
//! `site_apply_e2e.rs` (binary-presence-only, no opt-in env var — this is
//! one more short-lived nginx spawn, the same weight class); the helpers
//! themselves are re-derived rather than shared, matching that file's own
//! documented convention for `tests/`.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use openvhost_core::nginx_prefix_dir;
use openvhost_core::platform::macos::demo_stack::find_brew_binaries;

/// Mirrors `site_apply_e2e.rs::spawn_in_new_group` exactly — see that file's
/// module doc for why this is re-derived here rather than shared.
fn spawn_in_new_group(cmd: &mut Command) -> std::io::Result<Child> {
    cmd.process_group(0).spawn()
}

/// Mirrors `site_apply_e2e.rs::Killed` exactly.
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

/// Minimal HTTP/1.0 GET, dependency-free — mirrors `site_apply_e2e.rs::get`,
/// trimmed to a single deadline parameter since this file spawns exactly one
/// nginx per case rather than a shared stack.
fn get(port: u16, path: &str, deadline: Instant) -> Option<String> {
    while Instant::now() < deadline {
        if let Ok(mut s) = TcpStream::connect(("127.0.0.1", port)) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let read_timeout = remaining.max(Duration::from_millis(100));
            let _ = s.set_read_timeout(Some(read_timeout));
            let req = format!("GET {path} HTTP/1.0\r\nHost: x\r\n\r\n");
            if s.write_all(req.as_bytes()).is_ok() {
                let mut buf = String::new();
                if s.read_to_string(&mut buf).is_ok() && !buf.is_empty() {
                    return Some(buf);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    None
}

/// A free ephemeral port, so two nginx spawns in the same test (or a
/// parallel test binary) never race for one fixed literal.
fn free_port() -> u16 {
    TcpListener::bind(("127.0.0.1", 0))
        .expect("failed to bind an ephemeral port")
        .local_addr()
        .expect("bound listener must have a local address")
        .port()
}

/// Serve `home` with `root .;` — RELATIVE, the whole point, exactly the
/// shape a user-authored custom nginx file can carry — under `-p prefix`,
/// `GET /state.db`, and return nginx's response.
///
/// `pid`/`access_log` are pinned to absolute, ALREADY-CREATABLE paths inside
/// `home` (not under `prefix`) so this proves ONLY the `root` resolution
/// under test — a missing `logs/` directory under whichever `prefix` is
/// passed must never be why either case passes or fails.
fn serve_and_get_state_db(nginx_bin: &Path, home: &Path, prefix: &Path) -> String {
    let port = free_port();
    let conf = home.join(format!("prefix-test-{port}.conf"));
    let pid_path = home.join(format!("prefix-test-{port}.pid"));
    std::fs::write(
        &conf,
        format!(
            "worker_processes 1;\ndaemon off;\npid \"{}\";\nevents {{}}\nhttp {{\n  \
             access_log off;\n  server {{\n    listen 127.0.0.1:{port};\n    server_name _;\n    \
             root .;\n    location / {{}}\n  }}\n}}\n",
            pid_path.display()
        ),
    )
    .expect("write nginx config");
    let err_log = home.join(format!("prefix-test-{port}.error.log"));

    let mut cmd = Command::new(nginx_bin);
    cmd.arg("-e")
        .arg(&err_log)
        .arg("-p")
        .arg(prefix)
        .arg("-c")
        .arg(&conf);
    let child = spawn_in_new_group(&mut cmd)
        .unwrap_or_else(|e| panic!("failed to spawn nginx ({}): {e}", nginx_bin.display()));
    let _guard = Killed(child);

    let deadline = Instant::now() + Duration::from_secs(10);
    get(port, "/state.db", deadline).unwrap_or_else(|| {
        panic!(
            "no response from nginx on 127.0.0.1:{port} — check {}",
            err_log.display()
        )
    })
}

/// THE regression proof (audit finding, item 1, HIGHEST severity): with
/// `-p home`, a relative `root .;` resolves under `home` and serves
/// `state.db` verbatim; with `-p nginx_prefix_dir(home)`, the identical
/// relative root must resolve under the dedicated, empty prefix instead and
/// the file must not be servable at all.
///
/// VACUITY, proven by construction rather than by mutation: both halves of
/// this test drive the SAME `serve_and_get_state_db` helper, differing only
/// in which `prefix` is passed. The BEFORE half is the auditor's own live
/// finding, reproduced here as the test's own precondition — if the BEFORE
/// assertion ever stopped holding (e.g. nginx behaviour changed, or the test
/// fixture broke), this test would fail there and say so, rather than the
/// AFTER assertion passing for a reason that has nothing to do with the fix.
#[test]
fn a_relative_root_reaches_state_db_through_home_but_not_through_the_dedicated_prefix() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!(
            "SKIP a_relative_root_reaches_state_db_through_home_but_not_through_the_dedicated_prefix: \
             Homebrew nginx not found (brew install nginx)"
        );
        return;
    };

    let home = tempfile::Builder::new()
        .prefix("ovh prefix ")
        .tempdir_in("/tmp")
        .unwrap_or_else(|e| panic!("failed to create a short-path home under /tmp: {e}"));
    std::fs::write(home.path().join("state.db"), b"SECRET-ROOT-CREDENTIAL")
        .expect("write the state.db stand-in");

    // BEFORE (today's un-hardened shape, and the auditor's own reproduction):
    // `-p home` serves it.
    let before = serve_and_get_state_db(&brew.nginx, home.path(), home.path());
    assert!(
        before.contains("SECRET-ROOT-CREDENTIAL"),
        "precondition: a relative `root .;` must resolve under home when -p is home itself, \
         or this test proves nothing about the fix — got:\n{before}"
    );

    // AFTER (the fix): `-p nginx_prefix_dir(home)` must not.
    let prefix = nginx_prefix_dir(home.path());
    std::fs::create_dir_all(&prefix).expect("provision the dedicated prefix directory");
    let after = serve_and_get_state_db(&brew.nginx, home.path(), &prefix);
    assert!(
        after.starts_with("HTTP/1.1 404"),
        "state.db must NOT be servable once -p points at the dedicated prefix directory \
         instead of home itself — got:\n{after}"
    );
    assert!(
        !after.contains("SECRET-ROOT-CREDENTIAL"),
        "the credential marker leaked even though the status line was not 200:\n{after}"
    );
}
