# P0-7 — Minimal Tera Templates (openvhost-conf) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `openvhost-conf` generates a complete nginx + php-fpm config stack from embedded Tera templates and proves it with `nginx -t` + `php-fpm -t`.

**Architecture:** A pure `RenderCtx → GeneratedFile` generator behind two traits — `WebServerAdapter` (nginx main + site) and `PhpRuntimeAdapter` (php-fpm pool, returns `Option` so Windows can return `None`). All path logic funnels through one `to_config_path` chokepoint; the PHP-upstream OS branch is a Rust `match`, never a Tera conditional. `validate()` materializes into a throwaway `/tmp` home and runs the native validators, deriving success from exit code alone.

**Tech Stack:** Rust 2024, Tera (embedded via `include_str!`, autoescape off), async-trait, tokio process; Homebrew nginx 1.31.x + php-fpm 8.5.x for the live proof.

**Spec:** `docs/superpowers/specs/2026-07-23-p07-tera-templates-design.md` — the dual-consult findings there are binding and empirically verified (macOS specialist served real phpinfo through the generated shape). Do NOT "simplify" the quoting, banner, or validate-invocation rules; each exists because its absence was proven to fail.

## Global Constraints

- Branch `feat/p07-tera-templates` off current `main`.
- SPDX `// SPDX-License-Identifier: GPL-3.0-or-later` as line 1 of every new `.rs`. **`.tera` templates get NO SPDX line** — their content is rendered verbatim into user configs, so an SPDX header would leak into every generated file; `.tera` is not in `scripts/check-spdx.sh`'s checked globs, so this needs no exemption edit. The generated output leads with the DO-NOT-EDIT banner instead.
- No `unwrap()`/`expect()` outside `#[cfg(test)]` (workspace lints warn; tests use module-level allows).
- `openvhost-conf` must never depend on tauri.
- Every new dependency passes `cargo deny check licenses advisories`; name the license in the commit body. New deps: `tera` (locked engine, master plan §65), `async-trait`, `tempfile` (dev) — all MIT or MIT-OR-Apache-2.0.
- Conventional Commits, DCO-signed: always `git commit -s`. NO `Co-Authored-By` trailer (attribution disabled).
- Gates each task: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh`.
- **Binding platform rules (from the consults):** php-fpm banner uses `;` not `#` (a `#` first line hard-fails `php-fpm -t`); every nginx directive value embedding a path is double-quoted (nginx splits unquoted whitespace); nginx validate is `nginx -e <errlog> -t -c <main>` (`-e` mandatory — omitting leaks to `/opt/homebrew/var`); php-fpm validate is `php-fpm -t -n -y <pool>`; `ValidationReport.ok` = exit code == 0 ONLY (php-fpm emits a harmless empty-glob WARNING on every fresh validate); binaries are found by probing Homebrew `opt/` prefixes, NEVER `PATH` (ServBay shadows it); the real-home `MAX_SOCKET_PATH_BYTES ≤ 103` guard stays the caller's job (validate's short `/tmp` home is not a proxy).
- **macOS-first:** the unix path is implemented + validated; the Windows `PhpUpstream::TcpPorts` render and `PhpRuntimeAdapter`-None branch are defined and unit-shaped, not runtime-tested; no Windows cross-check this slice.

---

### Task 1: Crate foundation — deps, error, RenderCtx, to_config_path

**Files:**
- Modify: `crates/openvhost-conf/Cargo.toml`
- Modify: `crates/openvhost-conf/src/lib.rs` (replace stub)
- Create: `crates/openvhost-conf/src/error.rs`
- Create: `crates/openvhost-conf/src/ctx.rs`

**Interfaces:**
- Produces (later tasks use these):
  - `ConfError` (thiserror).
  - `PhpUpstream { UnixSocket(PathBuf), TcpPorts(Vec<SocketAddr>) }`
  - `RenderCtx { home, server_name, docroot, listen_addr, php_major, php_upstream, upstream_name }` + `RenderCtx::new(...) -> Result<Self, ConfError>` (boundary validation).
  - `GeneratedFile { path: PathBuf, contents: String }`
  - `ValidationReport { ok: bool, stderr: String }`
  - `pub(crate) fn to_config_path(p: &Path) -> Result<String, ConfError>`

- [ ] **Step 1: Branch**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p07-tera-templates
```

- [ ] **Step 2: Dependencies**

Replace `crates/openvhost-conf/Cargo.toml`:

```toml
[package]
name = "openvhost-conf"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
thiserror.workspace = true
tera = { version = "1", default-features = false }
async-trait = "0.1"
tokio = { workspace = true }

[dev-dependencies]
tempfile = "3"

[lints]
workspace = true
```

(`tera` `default-features = false` drops its optional `builtins`/`chrono`/`humansize` filters we don't use — smaller tree, fewer transitive licenses. `tokio` for `tokio::process::Command` in Task 3's async `validate`.)

- [ ] **Step 3: Write `error.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors for config generation and validation.

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum ConfError {
    #[error("path {0} is not valid UTF-8 (cannot render into a config template)")]
    PathNotUtf8(PathBuf),
    #[error("invalid {field}: {value:?} ({reason})")]
    InvalidField {
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("php upstream TcpPorts list must not be empty")]
    EmptyUpstream,
    #[error("template render failed: {0}")]
    Render(String),
    #[error("io error {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("validator {bin} could not be launched: {source}")]
    ValidatorSpawn {
        bin: String,
        #[source]
        source: std::io::Error,
    },
}
```

- [ ] **Step 4: Write the failing tests for `ctx.rs`**

Put at the BOTTOM of `crates/openvhost-conf/src/ctx.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
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
    fn accepts_clean_ctx() {
        let c = unix_ctx();
        assert_eq!(c.server_name, "myapp.localhost");
        assert_eq!(c.php_major, "8.4");
    }

    #[test]
    fn rejects_bad_server_name() {
        for bad in ["", "a b", "UPPER", "has_underscore", "sl/ash", "semi;colon"] {
            let r = RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                bad,
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                "8.4",
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
                "php_x",
            );
            assert!(r.is_err(), "should reject server_name {bad:?}");
        }
    }

    #[test]
    fn rejects_bad_php_major() {
        for bad in ["", "..", "8/4", "8 4", "../etc"] {
            let r = RenderCtx::new(
                PathBuf::from("/tmp/ovh"),
                "a.localhost",
                PathBuf::from("/tmp/ovh/www"),
                "127.0.0.1:8080".parse().unwrap(),
                bad,
                PhpUpstream::UnixSocket(PathBuf::from("/tmp/ovh/run/php-fpm.sock")),
                "php_x",
            );
            assert!(r.is_err(), "should reject php_major {bad:?}");
        }
    }

    #[test]
    fn rejects_empty_tcp_upstream() {
        let r = RenderCtx::new(
            PathBuf::from("/tmp/ovh"),
            "a.localhost",
            PathBuf::from("/tmp/ovh/www"),
            "127.0.0.1:8080".parse().unwrap(),
            "8.4",
            PhpUpstream::TcpPorts(vec![]),
            "php_x",
        );
        assert!(matches!(r, Err(ConfError::EmptyUpstream)));
    }

    #[test]
    fn to_config_path_forward_slashes_and_checks_utf8() {
        // On unix the path is already forward-slash; the fn is identity there.
        let s = to_config_path(&PathBuf::from("/tmp/ovh/run/php-fpm.sock")).unwrap();
        assert_eq!(s, "/tmp/ovh/run/php-fpm.sock");
        // Backslashes (a Windows-style path, exercised even on unix) become '/'.
        let s2 = to_config_path(std::path::Path::new(r"C:\Users\a\www")).unwrap();
        assert_eq!(s2, "C:/Users/a/www");
        // A verbatim prefix is stripped.
        let s3 = to_config_path(std::path::Path::new(r"\\?\C:\x")).unwrap();
        assert_eq!(s3, "C:/x");
    }
}
```

- [ ] **Step 5: Run tests to verify they fail**

Run: `cargo test -p openvhost-conf 2>&1 | tail -5`
Expected: compile errors (types/functions undefined). Implement next.

- [ ] **Step 6: Implement `ctx.rs` (above the tests)**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Render context and the single path-rendering chokepoint.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use crate::error::ConfError;

/// The PHP upstream — the #1 cross-platform seam (master plan §3.4). Rendered
/// by a Rust `match` in the adapter, never by a Tera conditional.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhpUpstream {
    /// macOS: `fastcgi_pass unix:<path>`.
    UnixSocket(PathBuf),
    /// Windows php-cgi pool — defined now, runtime deferred. Invariant:
    /// never empty (an empty nginx `upstream{}` fails `nginx -t`).
    TcpPorts(Vec<SocketAddr>),
}

#[derive(Debug, Clone)]
pub struct RenderCtx {
    pub home: PathBuf,
    pub server_name: String,
    pub docroot: PathBuf,
    pub listen_addr: SocketAddr,
    pub php_major: String,
    pub php_upstream: PhpUpstream,
    /// Stable, pre-sanitized, unique-per-site token that names the Windows
    /// nginx `upstream{}` block. Unused-but-present for `UnixSocket`.
    pub upstream_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub contents: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationReport {
    /// True iff the native validator exited 0. Never derived from stderr
    /// emptiness — php-fpm prints a harmless empty-glob WARNING every time.
    pub ok: bool,
    pub stderr: String,
}

fn valid_hostname_char(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-')
}

fn valid_component(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'_' | b'-'))
}

impl RenderCtx {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        home: PathBuf,
        server_name: impl Into<String>,
        docroot: PathBuf,
        listen_addr: SocketAddr,
        php_major: impl Into<String>,
        php_upstream: PhpUpstream,
        upstream_name: impl Into<String>,
    ) -> Result<Self, ConfError> {
        let server_name = server_name.into();
        let php_major = php_major.into();
        let upstream_name = upstream_name.into();

        if server_name.is_empty() || !server_name.bytes().all(valid_hostname_char) {
            return Err(ConfError::InvalidField {
                field: "server_name",
                value: server_name,
                reason: "must be a non-empty [a-z0-9.-] hostname",
            });
        }
        if !valid_component(&php_major) {
            return Err(ConfError::InvalidField {
                field: "php_major",
                value: php_major,
                reason: "must be a safe [a-z0-9._-] path component",
            });
        }
        if let PhpUpstream::TcpPorts(ports) = &php_upstream {
            if ports.is_empty() {
                return Err(ConfError::EmptyUpstream);
            }
        }
        Ok(Self {
            home,
            server_name,
            docroot,
            listen_addr,
            php_major,
            php_upstream,
            upstream_name,
        })
    }
}

/// The single chokepoint for embedding a path into a config template: reject
/// non-UTF-8 (Tera cannot render it), normalize `\` to `/`, and strip a
/// `\\?\` / `\\?\UNC\` verbatim prefix (nginx's parser understands neither).
/// A no-op on ordinary unix paths.
pub(crate) fn to_config_path(p: &Path) -> Result<String, ConfError> {
    let s = p.to_str().ok_or_else(|| ConfError::PathNotUtf8(p.to_path_buf()))?;
    let s = s
        .strip_prefix(r"\\?\UNC\")
        .map(|rest| format!(r"\\{rest}"))
        .unwrap_or_else(|| s.strip_prefix(r"\\?\").unwrap_or(s).to_string());
    Ok(s.replace('\\', "/"))
}
```

- [ ] **Step 7: Write `lib.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-conf — generated-config engine (Tera templates → nginx + php-fpm
//! configs) with a native-validator pass. Pure generation: same input ⇒
//! byte-identical output; never reads prior generated output. See
//! docs/superpowers/specs/2026-07-23-p07-tera-templates-design.md.

mod ctx;
mod error;

pub use ctx::{GeneratedFile, PhpUpstream, RenderCtx, ValidationReport};
pub use error::ConfError;
```

(Later tasks add `mod engine; mod webserver; mod phpruntime;` and re-export the adapters/traits.)

- [ ] **Step 8: Run tests, gates, commit**

```bash
cargo test -p openvhost-conf 2>&1 | tail -5
cargo fmt && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-conf Cargo.lock && git commit -s -m "feat(conf): RenderCtx, PhpUpstream seam, to_config_path, error type

New deps (MIT or MIT-OR-Apache-2.0, pass cargo deny): tera (locked engine,
plan §65; default-features off), async-trait, tempfile (dev)."
```

Expected: all pass.

---

### Task 2: Tera engine, the three templates, and the generate methods

**Files:**
- Create: `crates/openvhost-conf/templates/nginx/main.conf.tera`
- Create: `crates/openvhost-conf/templates/nginx/site.conf.tera`
- Create: `crates/openvhost-conf/templates/php-fpm/pool.conf.tera`
- Create: `crates/openvhost-conf/src/engine.rs`
- Create: `crates/openvhost-conf/src/webserver.rs`
- Create: `crates/openvhost-conf/src/phpruntime.rs`
- Modify: `crates/openvhost-conf/src/lib.rs` (add modules + re-exports)

**Interfaces:**
- Consumes (Task 1): `RenderCtx`, `PhpUpstream`, `GeneratedFile`, `ConfError`, `to_config_path`.
- Produces (Task 3 uses these):
  - `pub trait WebServerAdapter: Send + Sync` with `id`, `generate_main_config`, `generate_site_config`, `async validate` (validate lands in Task 3 — declared here), `supports_hot_reload`.
  - `pub struct NginxAdapter;` impl.
  - `pub trait PhpRuntimeAdapter: Send + Sync` with `generate_pool_config`, `async validate` (Task 3).
  - `pub struct PhpFpmRuntime;` impl.
  - `pub(crate) fn engine() -> &'static tera::Tera`

- [ ] **Step 1: Write the three templates**

`crates/openvhost-conf/templates/nginx/main.conf.tera` (note: EVERY path value is double-quoted; the banner is `#`-style):

```
# ---------------------------------------------------------------------------
# GENERATED by OpenVHost — DO NOT EDIT. Regenerated idempotently; your edits
# will be lost. To customize, add files under:
#   {{ custom_sites_dir }}
# ---------------------------------------------------------------------------
daemon off;
worker_processes 1;
pid "{{ pid_path }}";
error_log "{{ error_log }}" warn;
error_log stderr notice;

events {}

http {
    access_log "{{ access_log }}";
    client_body_temp_path "{{ temp_dir }}/client_body";
    proxy_temp_path "{{ temp_dir }}/proxy";
    fastcgi_temp_path "{{ temp_dir }}/fastcgi";
    uwsgi_temp_path "{{ temp_dir }}/uwsgi";
    scgi_temp_path "{{ temp_dir }}/scgi";

    include "{{ generated_sites_glob }}";
    include "{{ custom_sites_glob }}";
}
```

`crates/openvhost-conf/templates/nginx/site.conf.tera` (the `php_upstream_block` and `php_pass` come pre-rendered from the Rust `match` so there are no Tera conditionals over the enum):

```
# ---------------------------------------------------------------------------
# GENERATED by OpenVHost — DO NOT EDIT. To customize this site, add files under:
#   {{ custom_site_dir }}
# ---------------------------------------------------------------------------
{{ php_upstream_block }}server {
    listen {{ listen_addr }};
    server_name {{ server_name }};
    root "{{ docroot }}";

    location / {
        {{ php_pass }}
        fastcgi_param SCRIPT_FILENAME $document_root/index.php;
        fastcgi_param QUERY_STRING $query_string;
        fastcgi_param REQUEST_METHOD $request_method;
        fastcgi_param CONTENT_TYPE $content_type;
        fastcgi_param CONTENT_LENGTH $content_length;
        fastcgi_param SERVER_PROTOCOL $server_protocol;
        fastcgi_param REMOTE_ADDR $remote_addr;
        fastcgi_param SERVER_NAME $server_name;
        fastcgi_param SERVER_PORT $server_port;
        include "{{ custom_site_glob }}";
    }
}
```

`crates/openvhost-conf/templates/php-fpm/pool.conf.tera` (banner is `;`-style — a `#` first line hard-fails `php-fpm -t`):

```
; ---------------------------------------------------------------------------
; GENERATED by OpenVHost — DO NOT EDIT. To customize this pool, add files under:
;   {{ custom_pool_dir }}
; ---------------------------------------------------------------------------
[global]
error_log = {{ error_log }}

[www]
listen = {{ socket }}
pm = ondemand
pm.max_children = 4
catch_workers_output = yes
include={{ custom_pool_glob }}
```

- [ ] **Step 2: Write `engine.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! The process-wide Tera instance, built once from the embedded templates.
//! Autoescaping is OFF: these render `.conf` files, not HTML, so `&`/`<`/`>`
//! in a path or directive must pass through verbatim.

use std::sync::OnceLock;

use tera::Tera;

use crate::error::ConfError;

const MAIN_NGINX: &str = include_str!("../templates/nginx/main.conf.tera");
const SITE_NGINX: &str = include_str!("../templates/nginx/site.conf.tera");
const POOL_FPM: &str = include_str!("../templates/php-fpm/pool.conf.tera");

pub(crate) fn engine() -> &'static Tera {
    static ENGINE: OnceLock<Tera> = OnceLock::new();
    ENGINE.get_or_init(|| {
        let mut t = Tera::default();
        t.autoescape_on(vec![]); // no HTML escaping for any template
        // The templates are compile-time constants (`include_str!`), so a parse
        // error is a programmer error, not a runtime condition. The workspace
        // denies `expect_used`/`unwrap_used` under `-D warnings`, so use an
        // explicit `panic!` (not restricted) rather than `.expect()`.
        if let Err(e) = t.add_raw_templates(vec![
            ("nginx/main.conf", MAIN_NGINX),
            ("nginx/site.conf", SITE_NGINX),
            ("php-fpm/pool.conf", POOL_FPM),
        ]) {
            panic!("embedded templates must parse: {e}");
        }
        t
    })
}

pub(crate) fn render(name: &str, ctx: &tera::Context) -> Result<String, ConfError> {
    engine()
        .render(name, ctx)
        .map_err(|e| ConfError::Render(format!("{name}: {e}")))
}
```

Note the one `expect` is on a compile-time-constant input (the embedded templates); it is unreachable at runtime and is the standard pattern for `include_str!` template registration. If clippy `-D warnings` rejects it in non-test code, convert to `unwrap_or_else(|e| panic!("embedded templates must parse: {e}"))` — same effect, or gate the `OnceLock` init behind a `#[allow(clippy::expect_used)]` on the function with a comment. Pick whichever keeps the gate green; document why.

- [ ] **Step 3: Write the failing golden tests for `webserver.rs` + `phpruntime.rs`**

Bottom of `crates/openvhost-conf/src/webserver.rs`:

```rust
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
```

Bottom of `crates/openvhost-conf/src/phpruntime.rs`:

```rust
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
        let f = PhpFpmRuntime.generate_pool_config(&unix_ctx()).unwrap().unwrap();
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
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p openvhost-conf 2>&1 | tail -5`
Expected: compile errors (adapters/traits undefined).

Also update the engine-note reference: the `.expect()` hazard no longer applies (the code above already uses `panic!`). Keep the `render()` helper as-is.

- [ ] **Step 5: Implement `webserver.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Web-server config generation. NginxAdapter renders the main + site
//! configs; the PHP-upstream OS branch is a Rust `match`, never a Tera
//! conditional (keeps the platform seam type-checked).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::ctx::{to_config_path, PhpUpstream, RenderCtx};
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
        tc.insert("custom_sites_glob", &format!("{home}/config/custom/sites/*.conf"));
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
                    block.push_str(&format!(
                        "    server {addr} max_fails=1 fail_timeout=1s;\n"
                    ));
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
        _nginx_bin: &Path,
        _ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError> {
        // Implemented in Task 3.
        unimplemented!("nginx validate lands in Task 3")
    }

    fn supports_hot_reload(&self) -> bool {
        true
    }
}
```

**Note:** the `unimplemented!()` placeholder for `validate` will trip clippy/tests only if called; it is NOT called until Task 3 wires it. If the workspace's `-D warnings` flags `unimplemented!` in non-test code, replace the body with a real Task-3 implementation now by pulling Task 3's Step 2 code forward, OR return `Err(ConfError::Render("validate not yet wired".into()))` as a temporary — but the cleanest path is to implement Task 3's validate in the same PR flow; since this plan runs task-by-task, leave `unimplemented!()` and let Task 3 replace it (no caller exists in Task 2, so no test hits it). Confirm `cargo clippy -D warnings` stays green with `unimplemented!()` here; if not, this note's fallback applies.

- [ ] **Step 6: Implement `phpruntime.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtime config generation. Separate from WebServerAdapter because
//! php-fpm.conf has no Windows analog — `generate_pool_config` returns
//! `Option`, `None` on Windows (php-cgi pool membership is pure Rust state).

use std::path::Path;

use async_trait::async_trait;

use crate::ctx::{to_config_path, RenderCtx};
use crate::engine::render;
use crate::error::ConfError;
use crate::{GeneratedFile, ValidationReport};

#[async_trait]
pub trait PhpRuntimeAdapter: Send + Sync {
    fn generate_pool_config(&self, ctx: &RenderCtx) -> Result<Option<GeneratedFile>, ConfError>;
    async fn validate(&self, php_bin: &Path, ctx: &RenderCtx) -> Result<ValidationReport, ConfError>;
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
```

Add to `lib.rs`: `mod engine; mod webserver; mod phpruntime;` and `pub use webserver::{NginxAdapter, WebServerAdapter}; pub use phpruntime::{PhpFpmRuntime, PhpRuntimeAdapter};`.

- [ ] **Step 7: Run tests to green**

Run: `cargo test -p openvhost-conf 2>&1 | tail -8`
Expected: all golden tests pass (nginx main/site/tcp-seam/deterministic, php-fpm pool). If the tcp-seam or a quoting assertion fails, fix the template/adapter until green — the assertions encode the consult's binding rules.

- [ ] **Step 8: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-conf && git commit -s -m "feat(conf): Tera engine + nginx/php-fpm templates + generate methods"
```

---

### Task 3: Native validators + live proof (the exit criterion)

**Files:**
- Create: `crates/openvhost-conf/src/validate.rs` (shared materialize + run helper + brew probe)
- Modify: `crates/openvhost-conf/src/webserver.rs` (real `NginxAdapter::validate`)
- Modify: `crates/openvhost-conf/src/phpruntime.rs` (real `PhpFpmRuntime::validate`)
- Modify: `crates/openvhost-conf/src/lib.rs` (`mod validate;` + re-export `find_brew_binaries`)
- Create: `crates/openvhost-conf/tests/validate_live.rs`

**Interfaces:**
- Consumes: everything from Tasks 1–2.
- Produces: `pub fn find_brew_binaries() -> Option<BrewStack>` where `pub struct BrewStack { pub nginx: PathBuf, pub php_fpm: PathBuf }`; the two `validate` methods return real `ValidationReport`s.

- [ ] **Step 1: Write `validate.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Native-validator plumbing: locate the Homebrew binaries (never via PATH —
//! ServBay shadows nginx/php-fpm there), materialize generated files into a
//! throwaway home, and run the validator capturing stderr. `ok` is derived
//! from the exit code alone.

use std::path::{Path, PathBuf};

use crate::error::ConfError;
use crate::GeneratedFile;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BrewStack {
    pub nginx: PathBuf,
    pub php_fpm: PathBuf,
}

/// Probe the standard Homebrew prefixes (Apple Silicon, then Intel). NEVER
/// resolves via PATH — a ServBay install shadows `nginx`/`php-fpm` there.
pub fn find_brew_binaries() -> Option<BrewStack> {
    for prefix in [Path::new("/opt/homebrew"), Path::new("/usr/local")] {
        let nginx = prefix.join("opt/nginx/bin/nginx");
        let php_fpm = prefix.join("opt/php/sbin/php-fpm");
        if nginx.is_file() && php_fpm.is_file() {
            return Some(BrewStack { nginx, php_fpm });
        }
    }
    None
}

/// Write each generated file to disk under its `path`, creating parents.
pub(crate) fn materialize(files: &[GeneratedFile]) -> Result<(), ConfError> {
    for f in files {
        if let Some(parent) = f.path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| ConfError::Io {
                op: "create_dir",
                path: parent.to_path_buf(),
                source: e,
            })?;
        }
        std::fs::write(&f.path, &f.contents).map_err(|e| ConfError::Io {
            op: "write",
            path: f.path.clone(),
            source: e,
        })?;
    }
    Ok(())
}
```

- [ ] **Step 2: Implement `NginxAdapter::validate`**

Replace the `unimplemented!()` body in `webserver.rs`:

```rust
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
```

Add `#[derive(Clone)]` to `GeneratedFile` is already present (Task 1). Ensure `use crate::ctx::to_config_path;` etc. remain; add nothing else.

- [ ] **Step 3: Implement `PhpFpmRuntime::validate`**

Replace the `unimplemented!()` body in `phpruntime.rs`:

```rust
    async fn validate(
        &self,
        php_bin: &Path,
        ctx: &RenderCtx,
    ) -> Result<ValidationReport, ConfError> {
        let Some(pool) = self.generate_pool_config(ctx)? else {
            // No pool file on the TCP/Windows path — nothing to validate here.
            return Ok(ValidationReport { ok: true, stderr: String::new() });
        };
        crate::validate::materialize(&[pool.clone()])?;
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
```

Add `mod validate;` + `pub use validate::{find_brew_binaries, BrewStack};` to `lib.rs`.

- [ ] **Step 4: Write the live proof**

`crates/openvhost-conf/tests/validate_live.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Exit-criterion proof (master plan P0-7): the generated stack passes the
//! native validators on real Homebrew nginx + php-fpm. Auto-skips (loudly)
//! when the binaries are absent. The temp home path deliberately CONTAINS A
//! SPACE to prove the quoting rule (nginx splits unquoted whitespace) end to
//! end, including quoted `include` globs.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use openvhost_conf::{
    find_brew_binaries, NginxAdapter, PhpFpmRuntime, PhpRuntimeAdapter, PhpUpstream, RenderCtx,
    WebServerAdapter,
};

#[tokio::test]
async fn generated_stack_passes_native_validators() {
    let Some(brew) = find_brew_binaries() else {
        eprintln!("SKIP validate_live: Homebrew nginx/php-fpm not found (brew install nginx php)");
        return;
    };

    // Short /tmp base (sun_path headroom) with a SPACE in the dir name.
    let base = tempfile::Builder::new()
        .prefix("ovh conf ") // <- space is intentional
        .tempdir_in("/tmp")
        .unwrap();
    let home = base.path().to_path_buf();
    let socket = home.join("run/php-fpm.sock");

    let ctx = RenderCtx::new(
        home.clone(),
        "myapp.localhost",
        home.join("www"),
        "127.0.0.1:8080".parse().unwrap(),
        "8.4",
        PhpUpstream::UnixSocket(socket),
        "php_myapp",
    )
    .unwrap();

    let nginx_report = NginxAdapter.validate(&brew.nginx, &ctx).await.unwrap();
    assert!(
        nginx_report.ok,
        "nginx -t failed:\n{}",
        nginx_report.stderr
    );

    let fpm_report = PhpFpmRuntime.validate(&brew.php_fpm, &ctx).await.unwrap();
    assert!(
        fpm_report.ok,
        "php-fpm -t failed:\n{}",
        fpm_report.stderr
    );

    // The php-fpm empty-glob WARNING is expected and must NOT flip ok.
    // (No assertion on stderr emptiness — that is the whole point.)
    let _ = PathBuf::new();
}
```

- [ ] **Step 5: Run the hermetic suite, then the live proof for real**

```bash
cargo test -p openvhost-conf 2>&1 | tail -8
cargo test -p openvhost-conf --test validate_live -- --nocapture 2>&1 | tail -15
```

Expected: unit/golden tests pass; the live test prints neither a SKIP line nor a panic — `nginx -t` AND `php-fpm -t` both pass against the generated stack in a **space-containing** home (proving the quoting + quoted-glob-include story). If nginx rejects a quoted glob include, that surfaces here: adjust the template (e.g. drop quotes on the include globs only, since a `/tmp/ovh conf X/...*.conf` glob still needs whitespace handling — if quoted globs don't expand, the fix is to ensure the home has no space in production, but the guard is: this test MUST pass, so make the template shape that does). Record the exact `nginx -t`/`php-fpm -t` success in the report.

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-conf && git commit -s -m "feat(conf): native nginx -t/php-fpm -t validators + live proof"
```

---

### Task 4: Deny gate, docs truth-up, PR

**Files:**
- Modify: `templates/README.md` (note first content landed) — optional
- No production code changes.

**Interfaces:** none — verification and delivery.

- [ ] **Step 1: License gate for the new deps**

```bash
cargo deny check licenses advisories 2>&1 | tail -20
```

Expected: exit 0. `tera`, `async-trait`, `tempfile` are MIT/Apache — should pass. If a transitive dep's license is not on the allowlist, STOP and report (do not edit `deny.toml` without confirming GPLv3 compatibility). Record the outcome for the PR body.

- [ ] **Step 2: Full local gate suite (the merge gate while CI is off)**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh && pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all green; `validate_live` runs (not skips) on this machine.

- [ ] **Step 3: Push + PR**

```bash
git push -u origin feat/p07-tera-templates
gh pr create --title "feat: P0-7 — minimal Tera config templates (openvhost-conf)" --body "Implements docs/superpowers/specs/2026-07-23-p07-tera-templates-design.md: openvhost-conf generates a complete nginx + php-fpm stack from embedded Tera templates behind a WebServerAdapter (nginx) + PhpRuntimeAdapter (php-fpm, returns Option for Windows) seam, then proves it with the native validators.

macOS-first (v1): the unix/php-fpm path is implemented and validated; the Windows PhpUpstream::TcpPorts render + PhpRuntimeAdapter-None branch are defined and unit-shaped, not runtime-tested (Windows-enablement phase). Dual-consult findings baked in: php-fpm banner uses \`;\` (a \`#\` first line hard-fails php-fpm -t); every nginx path directive is double-quoted; nginx validate is \`nginx -e <errlog> -t -c\`; php-fpm is \`php-fpm -t -n -y\`; ok = exit code only; binaries probed from Homebrew prefixes not PATH.

Verification: golden-file + boundary tests green; the env-independent live proof runs \`nginx -t\` + \`php-fpm -t\` against the generated stack in a SPACE-containing temp home (proving the quoting rule end to end). Byte-equivalent to the P0-4 hand-written stack the macOS specialist proved serves phpinfo. CI disabled (billing, P0-3 §2.3); local gates are the merge gate. No security-auditor surface (no download/helper/cert/hosts/IPC in this slice)."
```

- [ ] **Step 4: Hand back to controller** — final whole-branch review, then merge (no security-auditor gate; no owner smoke needed beyond the live `nginx -t`/`php-fpm -t` proof already captured). NOT the implementer's step.

---

## Self-review (controller: verify before dispatching Task 1)

- **Spec coverage:** §4 API (T1 types + T2 traits/adapters + T3 validate); §5 boundary validation + to_config_path (T1); §6 three templates + banner rules + quoting (T2); §7 validate flow + invocations + dir pre-create + brew probe (T3); §8 golden/boundary/live tests (T2/T3) + gates (T3/T4). No unmet requirement.
- **Type consistency:** `RenderCtx`/`PhpUpstream`/`GeneratedFile`/`ValidationReport`/`ConfError`/`to_config_path`/`WebServerAdapter`/`NginxAdapter`/`PhpRuntimeAdapter`/`PhpFpmRuntime`/`find_brew_binaries`/`BrewStack` — consistent across tasks.
- **Known implementer hazards flagged in-plan:** the `unimplemented!()` validate bodies in T2 (replaced in T3; confirm `-D warnings` tolerates them with no caller — if not, pull T3's validate forward); the `expect` on `include_str!` template registration in engine.rs (compile-time-constant input; convert per the note if clippy rejects); and the quoted-glob-`include` behavior, which the T3 live proof in a space-containing home is designed to nail down empirically rather than assume.
