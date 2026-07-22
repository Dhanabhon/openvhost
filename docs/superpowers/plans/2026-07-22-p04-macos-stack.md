# P0-4 — macOS nginx + php-fpm Stack Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Real nginx + php-fpm run under the shipped P0-3 supervisor on macOS, serving phpinfo over a unix socket, visible and controllable in the existing Services panel.

**Architecture:** A cfg-gated provisioning module in `openvhost-core` (`src/platform/macos/` — the §6.2 ownership glob) writes a fully-absolute-path config set under the OpenVHost home; the desktop app registers two data-only `ServiceSpec`s built from Homebrew binary paths; an integration test in core (dev-dep on `openvhost-proc`) proves the whole loop headlessly with `/usr/bin/curl`. No supervisor/trait changes anywhere.

**Tech Stack:** Rust (std fs only — no new runtime deps), tokio + tempfile as dev-deps, Homebrew nginx 1.31.x + PHP 8.5.x-fpm, existing tauri-specta bindings (untouched).

**Spec:** `docs/superpowers/specs/2026-07-22-p04-macos-stack-design.md` — every config directive below is specialist-verified (live on the dev Mac, 2026-07-22); do not "simplify" the nginx directive set, each line exists because its absence was proven to leak state or fail.

## Global Constraints

- Branch: `feat/p04-macos-stack` off current `main`.
- Every new `.rs` file starts with `// SPDX-License-Identifier: GPL-3.0-or-later`.
- No `unwrap()`/`expect()` outside `#[cfg(test)]` (workspace lints warn; tests use `#![allow(clippy::unwrap_used)]` / module-level allow, matching existing files).
- `openvhost-core` must never depend on tauri. The new `openvhost-proc` dependency is **dev-only** (tests) — proc is tauri-free, the CI guard stays satisfied.
- Conventional Commits, DCO-signed: always `git commit -s`.
- Gates per task: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.
- All provisioned paths are absolute; no PATH-dependent behavior anywhere (spec §5 invariant).

---

### Task 1: Core provisioning module (TDD)

**Files:**
- Create: `crates/openvhost-core/src/platform/mod.rs`
- Create: `crates/openvhost-core/src/platform/macos/mod.rs`
- Create: `crates/openvhost-core/src/platform/macos/demo_stack.rs` (impl + inline unit tests)
- Modify: `crates/openvhost-core/src/error.rs` (three new variants)
- Modify: `crates/openvhost-core/src/lib.rs` (add `pub mod platform;`)
- Modify: `crates/openvhost-core/Cargo.toml` (dev-dep `tempfile`)

**Interfaces:**
- Consumes: `crate::error::CoreError` (existing).
- Produces (Tasks 2–3 rely on these exact names):
  - `openvhost_core::platform::macos::demo_stack::provision_macos_demo_stack(home: &Path, port: u16) -> Result<StackPaths, CoreError>`
  - `pub struct StackPaths { pub nginx_conf: PathBuf, pub fpm_conf: PathBuf, pub docroot: PathBuf, pub socket: PathBuf, pub nginx_error_log: PathBuf, pub port: u16 }`
  - `pub struct BrewStack { pub nginx: PathBuf, pub php_fpm: PathBuf }`
  - `find_brew_binaries() -> Option<BrewStack>` and `find_brew_binaries_in(prefixes: &[&Path]) -> Option<BrewStack>`
  - `pub const MAX_SOCKET_PATH_BYTES: usize = 103`

- [ ] **Step 1: Branch**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p04-macos-stack
```

- [ ] **Step 2: Add dev-dep and module scaffolding (compile skeleton first so tests can be written against real paths)**

`crates/openvhost-core/Cargo.toml` — append to `[dev-dependencies]`:

```toml
[dev-dependencies]
serde_json = "1"
tempfile = "3"
```

`crates/openvhost-core/src/lib.rs` — after `mod info;` add:

```rust
pub mod platform;
```

`crates/openvhost-core/src/platform/mod.rs` (new):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Platform-specific provisioning. Each OS lives in a `#[cfg(target_os)]`
//! submodule; the master-plan §6.2 ownership glob (`src/platform/macos*`)
//! assigns the macOS tree to platform-macos-specialist.

#[cfg(target_os = "macos")]
pub mod macos;
```

`crates/openvhost-core/src/platform/macos/mod.rs` (new):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS provisioning.

pub mod demo_stack;
```

`crates/openvhost-core/src/error.rs` — add `use std::path::PathBuf;` at the top (below the doc comment) and three variants inside `CoreError`:

```rust
    /// A filesystem operation failed while provisioning.
    #[error("provision: {op} {}: {source}", path.display())]
    ProvisionIo {
        op: &'static str,
        path: PathBuf,
        source: std::io::Error,
    },
    /// The OpenVHost home is not valid UTF-8, so it cannot be written into
    /// text configs faithfully.
    #[error("openvhost home {} is not valid UTF-8", path.display())]
    HomeNotUtf8 { path: PathBuf },
    /// The php-fpm unix socket path would exceed Darwin's 104-byte
    /// `sun_path`. php-fpm does NOT reject longer paths — it warns, silently
    /// truncates, and binds the wrong path while nginx 502s forever
    /// (specialist-proven). Refuse early instead.
    #[error("socket path {} is {len} bytes (max 103); use a shorter OPENVHOST_HOME", path.display())]
    SocketPathTooLong { path: PathBuf, len: usize },
```

`crates/openvhost-core/src/platform/macos/demo_stack.rs` (new — skeleton only; bodies `todo!()` are FORBIDDEN in this repo, so write the real consts now and a provision fn that only validates, to be completed in Step 5):

Write the FULL file below (Step 5 shows the complete final content — write that file now; TDD here means the unit tests in Step 3 are written against this interface and MUST fail meaningfully until the logic is complete, but a compiling skeleton avoids fighting the compiler in the red phase). Concretely: write the whole of Step 5's file EXCEPT leave `provision_macos_demo_stack` returning `Err(CoreError::HomeNotUtf8 { path: home.to_path_buf() })` unconditionally as the stub.

- [ ] **Step 3: Write the failing unit tests**

Append to `crates/openvhost-core/src/platform/macos/demo_stack.rs`:

```rust
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
        assert!(!conf.contains("user ="), "user/group cause non-root warnings");
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
        assert!(leftovers.is_empty(), "stale atomic-write temps: {leftovers:?}");
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
```

- [ ] **Step 4: Run tests — verify they fail for the right reason**

```bash
cargo test -p openvhost-core platform::macos -- --nocapture
```

Expected: `brew_prober_requires_both_binaries_in_one_prefix` PASSES (prober is real already); every `provision_*` test FAILS via the stub's `HomeNotUtf8` error (or `unwrap_err` mismatch in the too-long test). If instead you see compile errors, fix the skeleton until the failures are assertion/Err failures, not compiler ones.

- [ ] **Step 5: Full implementation**

Replace `crates/openvhost-core/src/platform/macos/demo_stack.rs` above the `#[cfg(test)]` module with:

```rust
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
        (nginx.is_file() && php_fpm.is_file()).then(|| BrewStack { nginx, php_fpm })
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
    let home_str = home
        .to_str()
        .ok_or_else(|| CoreError::HomeNotUtf8 {
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
```

- [ ] **Step 6: Run tests — verify all pass**

```bash
cargo test -p openvhost-core -- --nocapture
```

Expected: all existing core tests + the 7 new ones PASS.

- [ ] **Step 7: Gates**

```bash
cargo fmt && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
```

Expected: all green (SPDX check covers the three new .rs files).

- [ ] **Step 8: Commit**

```bash
git add crates/openvhost-core && git commit -s -m "feat(core): macOS demo-stack provisioning with socket-path guard"
```

---

### Task 2: Headless integration proof

**Files:**
- Create: `crates/openvhost-core/tests/macos_stack.rs`
- Modify: `crates/openvhost-core/Cargo.toml` (dev-deps `openvhost-proc`, `tokio`)

**Interfaces:**
- Consumes (Task 1): `provision_macos_demo_stack`, `find_brew_binaries`, `StackPaths`, `BrewStack`.
- Consumes (existing openvhost-proc): `Supervisor::new(default_driver())`, `register(ServiceSpec)`, sync `start(&str) -> Result<(), ProcError>` / `stop(&str)` (must be called inside a tokio runtime — they `tokio::spawn` internally), `snapshot() -> Vec<ServiceStatus>`, `ServiceState`.
- Produces: nothing new — this task is the executable exit-criterion proof.

- [ ] **Step 1: Add dev-deps**

`crates/openvhost-core/Cargo.toml` `[dev-dependencies]` becomes:

```toml
[dev-dependencies]
serde_json = "1"
tempfile = "3"
openvhost-proc = { path = "../openvhost-proc" }
tokio = { workspace = true }
```

- [ ] **Step 2: Write the integration test**

`crates/openvhost-core/tests/macos_stack.rs` (new):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! P0-4 exit-criterion proof: real nginx + php-fpm under the supervisor,
//! phpinfo served over the provisioned unix socket, clean teardown.
//! Auto-skips (loudly) when the Homebrew binaries are absent — NOT
//! `#[ignore]`, so `cargo test --workspace` on a dev Mac always runs it.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use std::ffi::OsString;
use std::time::{Duration, Instant};

use openvhost_core::platform::macos::demo_stack::{
    BrewStack, find_brew_binaries, provision_macos_demo_stack,
};
use openvhost_proc::{ServiceSpec, ServiceState, SpawnSpec, Supervisor, default_driver};

fn ephemeral_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// `curl -sf` — exit 0 only on 2xx; returns the body.
fn curl(port: u16) -> Option<String> {
    let out = std::process::Command::new("/usr/bin/curl")
        .args(["-sf", "-m", "2", &format!("http://127.0.0.1:{port}/")])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

async fn wait_state(
    sup: &Supervisor,
    id: &str,
    timeout: Duration,
    pred: impl Fn(&ServiceState) -> bool,
) {
    let deadline = Instant::now() + timeout;
    loop {
        let st = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == id)
            .map(|s| s.state);
        if let Some(s) = &st {
            if pred(s) {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting on '{id}'; last state: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

fn spec(id: &str, program: std::path::PathBuf, args: Vec<OsString>) -> ServiceSpec {
    ServiceSpec {
        id: id.into(),
        display_name: id.into(),
        endpoint: None,
        spawn: SpawnSpec {
            program,
            args,
            cwd: None,
            env: vec![],
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn stack_serves_phpinfo_and_tears_down_clean() {
    let Some(BrewStack { nginx, php_fpm }) = find_brew_binaries() else {
        eprintln!("SKIP macos_stack: Homebrew nginx/php not found (brew install nginx php)");
        return;
    };

    // /tmp keeps the socket far under Darwin's 104-byte sun_path limit
    // (TMPDIR is /var/folders/... — brittle; scratchpads measured over it).
    let home = tempfile::Builder::new()
        .prefix("ovh")
        .tempdir_in("/tmp")
        .unwrap();
    let port = ephemeral_port();
    let paths = provision_macos_demo_stack(home.path(), port).unwrap();

    let sup = Supervisor::new(default_driver());
    sup.register(spec(
        "php-fpm",
        php_fpm,
        vec![
            OsString::from("-F"),
            OsString::from("-O"),
            OsString::from("-n"),
            OsString::from("-y"),
            paths.fpm_conf.clone().into_os_string(),
        ],
    ));
    sup.register(spec(
        "nginx",
        nginx,
        vec![
            OsString::from("-e"),
            paths.nginx_error_log.clone().into_os_string(),
            OsString::from("-c"),
            paths.nginx_conf.clone().into_os_string(),
        ],
    ));

    // fpm first; wait for the socket file, not just Running.
    sup.start("php-fpm").unwrap();
    wait_state(&sup, "php-fpm", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;
    let sock_deadline = Instant::now() + Duration::from_secs(5);
    while !paths.socket.exists() {
        assert!(Instant::now() < sock_deadline, "php-fpm socket never appeared");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    sup.start("nginx").unwrap();
    wait_state(&sup, "nginx", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;

    // Poll curl until phpinfo arrives (first fpm worker spawn can lag).
    let curl_deadline = Instant::now() + Duration::from_secs(10);
    let body = loop {
        if let Some(b) = curl(port) {
            break b;
        }
        assert!(Instant::now() < curl_deadline, "curl never got HTTP 200");
        tokio::time::sleep(Duration::from_millis(200)).await;
    };
    assert!(body.contains("phpinfo"), "200 body lacks phpinfo");

    // Teardown: group-TERM per service; both must reach Stopped, socket
    // unlinked by fpm, port closed.
    sup.stop("nginx").unwrap();
    wait_state(&sup, "nginx", Duration::from_secs(3), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    sup.stop("php-fpm").unwrap();
    wait_state(&sup, "php-fpm", Duration::from_secs(3), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    assert!(!paths.socket.exists(), "fpm socket not unlinked on stop");
    assert!(curl(port).is_none(), "port still serving after stop");
}
```

- [ ] **Step 3: Run it live (this machine has the binaries — it must NOT skip)**

```bash
cargo test -p openvhost-core --test macos_stack -- --nocapture
```

Expected: `stack_serves_phpinfo_and_tears_down_clean ... ok` in well under 30s, no SKIP line. If you see the SKIP line, STOP — the binaries went missing; do not mark this task done on a skipped run.

- [ ] **Step 4: Orphan audit**

```bash
pgrep -fl 'nginx|php-fpm' | grep -v ServBay || echo "clean"
```

Expected: `clean` (ServBay's own processes, if any, are not ours).

- [ ] **Step 5: Gates**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
```

Expected: green; the new integration test runs (not skips) on this machine.

- [ ] **Step 6: Commit**

```bash
git add crates/openvhost-core && git commit -s -m "test(core): macOS nginx+php-fpm supervision integration proof"
```

---

### Task 3: Desktop registration

**Files:**
- Create: `apps/desktop/src-tauri/src/stack.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs` (mod decl + setup registration)

**Interfaces:**
- Consumes (Task 1): `provision_macos_demo_stack`, `find_brew_binaries`, `BrewStack`; existing `openvhost_core::resolve_home()`; existing `openvhost_proc::{ServiceSpec, SpawnSpec}`.
- Produces: `stack::macos_stack_specs() -> Vec<ServiceSpec>` (macOS-only), consumed by `run()`'s setup.
- Frontend/bindings: NO changes — no new commands/events; the panel is data-driven and `snapshot()` sorts by id, so rows render as `demo-ticker`, `nginx`, `php-fpm`.

- [ ] **Step 1: Write `stack.rs`**

`apps/desktop/src-tauri/src/stack.rs` (new):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! macOS demo-stack registration (P0-4). Data-only: binaries from the
//! Homebrew probe (resolved at registration time), configs provisioned
//! under the OpenVHost home. P0-6 swaps the binary source to packages/.

use std::ffi::OsString;
use std::path::PathBuf;

use openvhost_core::platform::macos::demo_stack::{
    BrewStack, find_brew_binaries, provision_macos_demo_stack,
};
use openvhost_proc::{ServiceSpec, SpawnSpec};

const DEMO_PORT: u16 = 8080;

/// Apple Silicon default paths, used when probing finds nothing: the rows
/// still register, and Start yields an honest Failed naming the missing
/// path (the P0-3 spawn-fail contract) instead of the rows vanishing.
fn fallback_brew() -> BrewStack {
    BrewStack {
        nginx: PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
        php_fpm: PathBuf::from("/opt/homebrew/opt/php/sbin/php-fpm"),
    }
}

/// Build the two supervised stack rows. Provision errors are logged and
/// non-fatal (rows register; Start surfaces the problem honestly). Only a
/// home-resolution failure skips the rows entirely — without a home there
/// are no config paths to point at.
pub fn macos_stack_specs() -> Vec<ServiceSpec> {
    let home = match openvhost_core::resolve_home() {
        Ok(h) => h,
        Err(e) => {
            eprintln!("stack: cannot resolve OPENVHOST_HOME, skipping nginx/php-fpm rows: {e}");
            return vec![];
        }
    };
    if let Err(e) = provision_macos_demo_stack(&home, DEMO_PORT) {
        eprintln!("stack: provisioning failed (rows registered anyway): {e}");
    }
    let brew = find_brew_binaries().unwrap_or_else(fallback_brew);
    let conf = home.join("conf");
    vec![
        ServiceSpec {
            id: "php-fpm".into(),
            display_name: "PHP-FPM".into(),
            endpoint: Some("run/php-fpm.sock".into()),
            spawn: SpawnSpec {
                program: brew.php_fpm,
                args: vec![
                    OsString::from("-F"),
                    OsString::from("-O"),
                    OsString::from("-n"),
                    OsString::from("-y"),
                    conf.join("php-fpm.conf").into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        },
        ServiceSpec {
            id: "nginx".into(),
            display_name: "nginx".into(),
            endpoint: Some(format!("http://127.0.0.1:{DEMO_PORT}")),
            spawn: SpawnSpec {
                program: brew.nginx,
                args: vec![
                    OsString::from("-e"),
                    home.join("logs/nginx.error.log").into_os_string(),
                    OsString::from("-c"),
                    conf.join("nginx.conf").into_os_string(),
                ],
                cwd: None,
                env: vec![],
            },
        },
    ]
}
```

- [ ] **Step 2: Wire into `lib.rs`**

In `apps/desktop/src-tauri/src/lib.rs`, after `mod commands;` add:

```rust
#[cfg(target_os = "macos")]
mod stack;
```

In `run()`'s `.setup(...)`, directly after `supervisor.register(demo_ticker_spec());` add:

```rust
            #[cfg(target_os = "macos")]
            for spec in stack::macos_stack_specs() {
                supervisor.register(spec);
            }
```

- [ ] **Step 3: Build + bindings drift check**

```bash
cargo build -p openvhost-desktop && cargo test -p openvhost-desktop export_bindings && git diff --exit-code -- apps/desktop/src/lib/ipc/bindings.ts
```

Expected: build green; bindings byte-identical (no command/event surface change).

- [ ] **Step 4: Frontend suite untouched but must stay green**

```bash
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all green with zero frontend diffs.

- [ ] **Step 5: Gates**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
```

Expected: green.

- [ ] **Step 6: Commit**

```bash
git add apps/desktop/src-tauri && git commit -s -m "feat(desktop): register supervised nginx + php-fpm rows on macOS"
```

---

### Task 4: Docs truth-up, cross-target check, PR

**Files:**
- Modify: `docs/superpowers/specs/2026-07-22-p04-macos-stack-design.md` (one clause)
- No code changes.

**Interfaces:** none — verification and delivery only.

- [ ] **Step 1: Fix the spec's panel-order clause to match reality**

In `docs/superpowers/specs/2026-07-22-p04-macos-stack-design.md` §5, replace the clause `register php-fpm, nginx (in that display order), keep demo-ticker as the third, dependency-free row` with: `register php-fpm and nginx and keep the dependency-free demo-ticker (the panel orders rows by id — demo-ticker, nginx, php-fpm — because Supervisor::snapshot() sorts by id; registration order is irrelevant)`.

- [ ] **Step 2: Windows-build stand-in evidence (CI is disabled — owner decision, P0-3 spec §2.3)**

```bash
cargo check --target x86_64-pc-windows-msvc -p openvhost-core && cargo clippy --target x86_64-pc-windows-msvc -p openvhost-core -- -D warnings
```

Expected: clean — proves the cfg-gated module leaves the Windows build untouched.

- [ ] **Step 3: Full local gate suite (the merge gate while CI is disabled)**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh && pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: everything green; `macos_stack` runs live (not skipped).

- [ ] **Step 4: Commit docs + push + PR**

```bash
git add docs/superpowers/specs/2026-07-22-p04-macos-stack-design.md && git commit -s -m "docs: align P0-4 spec panel-order clause with snapshot() sorting"
git push -u origin feat/p04-macos-stack
gh pr create --title "feat: P0-4 — supervised macOS nginx + php-fpm stack serving phpinfo" --body "Implements docs/superpowers/specs/2026-07-22-p04-macos-stack-design.md: cfg-gated provisioning in openvhost-core (socket-path guard, all-absolute configs), data-only ServiceSpec registration in the app, and a live integration proof (curl -> phpinfo -> clean teardown). No supervisor/trait/bindings changes. CI disabled (billing; owner decision P0-3 spec \$2.3) - local gate suite + x86_64-pc-windows-msvc cross-check are the evidence; macOS matrix backfills on Actions restoration."
```

Expected: PR opens against main.

- [ ] **Step 5: Hand back to controller** — final whole-branch review, then the owner-visible manual smoke (spec §6: `./scripts/dev.sh` → Start php-fpm → Start nginx → `http://127.0.0.1:8080` shows phpinfo → Stop both → `pgrep` clean), then merge. NOT the implementer's step.
