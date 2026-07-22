# P0-7 — Minimal Tera Templates (openvhost-conf) — Design

- **Date:** 2026-07-23
- **Status:** Approved in brainstorming (3 sections). Dual platform consultation folded in verbatim-by-requirement: **platform-macos-specialist** APPROVE-WITH-CHANGES (empirical — generated the stack and served real phpinfo through it) and **platform-windows-specialist** APPROVE-WITH-CHANGES (seam-shape feasibility for the future php-cgi pool). This slice introduces a new cross-platform abstraction (`WebServerAdapter` + `RenderCtx`/`PhpUpstream`), which per CLAUDE.md golden rule 3 required both specialists to confirm feasibility before implementation.
- **Source of truth:** `docs/OPENVHOST_MASTER_PLAN.md` v1.2 — row **P0-7**: "Minimal Tera templates: nginx main + one site + php-fpm/php-cgi upstream", owner config-template-engineer, exit criterion "Generated config passes `nginx -t` on both OS". Tera is the locked template engine (§65). The config-template-engineer hard rules (§ agent notes) are binding: DO-NOT-EDIT banner + `include` custom files; generation is a pure function of inputs (byte-identical output; never read prior generated output); atomic writes; native validator before apply, stderr surfaced verbatim; **PHP upstream MUST come from RenderCtx, never hardcoded**; config dirs are per-MAJOR version.
- **Owner decisions (2026-07-22/23):** scope = **generate + validate only** (defer diff-preview, apply/swap, reload, hot-reload); **three templates** (nginx main + nginx site + php-fpm pool), validated with `nginx -t` AND `php-fpm -t`; approach = **embedded templates** (`include_str!`, self-contained + reproducible).

## 1. Context

`openvhost-conf` is a stub. This slice gives it the config-generation core: Tera templates → generated nginx + php-fpm configs, plus the native-validator pass that is P0-7's exit criterion. It supersedes the hand-written `NGINX_CONF`/`FPM_CONF` string constants in `crates/openvhost-core/src/platform/macos/demo_stack.rs` (P0-4) with real templates — the generated output is directive-equivalent to that already-shipped, phpinfo-serving stack (macOS consult proved this live). **macOS-first**: the unix/php-fpm path is implemented and validated; the Windows php-cgi upstream + `PhpRuntimeAdapter`-returns-None path is defined in the seam but deferred (Windows-enablement phase), consistent with [[project-scope-macos-first]].

## 2. Goals

1. A pure `RenderCtx → GeneratedFile` generator for three configs (nginx main, nginx site, php-fpm pool) via embedded Tera templates; same input ⇒ byte-identical output.
2. A `WebServerAdapter` (nginx) + `PhpRuntimeAdapter` (php-fpm) seam whose shape accommodates the future Windows php-cgi pool without a breaking change.
3. `validate()` materializes the generated files into a throwaway temp home and runs the native validators (`nginx -t`, `php-fpm -t`), surfacing stderr verbatim, deriving success from **exit code alone**.
4. Golden-file + boundary tests run in every `cargo test`; a binary-gated live proof shows `nginx -t` + `php-fpm -t` pass on generated output — the exit criterion.

## 3. Non-goals

diff-preview · apply/atomic-swap of the live config · reload/hot-reload (SIGUSR2) · ApacheAdapter/CaddyAdapter · `httpd -t` · state.db / `Site` domain model (P0-7 takes a `RenderCtx` directly) · the Windows php-cgi pool *runtime* (its `PhpUpstream::TcpPorts` render path and `PhpRuntimeAdapter`-None branch are defined and unit-shaped, not runtime-tested) · rewiring the P0-4 demo-stack to consume generated configs (optional later) · reading `state.db`.

## 4. Crate layout & API

`crates/openvhost-conf` (tauri-free; owner config-template-engineer; platform-path questions → the specialists whose findings are folded here):

```
src/
  error.rs      ConfError (thiserror)
  ctx.rs        RenderCtx, PhpUpstream, GeneratedFile, ValidationReport, to_config_path()
  engine.rs     the Tera instance (autoescape OFF), built once from include_str! templates
  webserver.rs  WebServerAdapter trait + NginxAdapter
  phpruntime.rs PhpRuntimeAdapter trait + PhpFpmRuntime
templates/
  nginx/main.conf.tera
  nginx/site.conf.tera
  php-fpm/pool.conf.tera
```

```rust
/// PHP upstream — the #1 cross-platform seam (master plan §3.4). Rendered by
/// a Rust `match` in the adapter, NEVER by a Tera conditional over a serialized
/// tag (keeps the platform branch type-checked — Windows consult).
pub enum PhpUpstream {
    UnixSocket(PathBuf),          // macOS: `fastcgi_pass unix:<path>`
    TcpPorts(Vec<SocketAddr>),    // Windows php-cgi pool — DEFINED, runtime deferred.
                                  // Invariant: never empty (an empty nginx upstream{} fails
                                  // `nginx -t`); the caller fails fast before RenderCtx.
}

pub struct RenderCtx {
    pub home: PathBuf,
    pub server_name: String,      // e.g. "myapp.localhost"
    pub docroot: PathBuf,
    pub listen_addr: SocketAddr,  // 127.0.0.1:8080
    pub php_major: String,        // "8.4" — per-MAJOR dir; never a full version
    pub php_upstream: PhpUpstream,
    pub upstream_name: String,    // stable, pre-sanitized, unique-per-site token (e.g.
                                  // "php_<site>"); unused-but-present for UnixSocket, but
                                  // REQUIRED to name the Windows nginx upstream{} block
                                  // without re-deriving it under pressure later (Windows consult).
}
impl RenderCtx { pub fn new(...) -> Result<Self, ConfError> }  // boundary validation (§5)

pub struct GeneratedFile { pub path: PathBuf, pub contents: String }
pub struct ValidationReport { pub ok: bool, pub stderr: String }  // ok = exit code == 0 ONLY

#[async_trait]
pub trait WebServerAdapter: Send + Sync {
    fn id(&self) -> &'static str;                                                  // "nginx"
    fn generate_main_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;
    fn generate_site_config(&self, ctx: &RenderCtx) -> Result<GeneratedFile, ConfError>;
    async fn validate(&self, nginx_bin: &Path, ctx: &RenderCtx) -> Result<ValidationReport, ConfError>;
    fn supports_hot_reload(&self) -> bool;
}

/// Separate trait — php-fpm.conf has NO Windows analog (php-cgi has no pool/master
/// process or config file). Returns Option so the call site stays uniform: Some on
/// macOS, None on Windows (pool membership is pure Rust state in openvhost-proc).
#[async_trait]
pub trait PhpRuntimeAdapter: Send + Sync {
    fn generate_pool_config(&self, ctx: &RenderCtx) -> Result<Option<GeneratedFile>, ConfError>;
    async fn validate(&self, php_bin: &Path, ctx: &RenderCtx) -> Result<ValidationReport, ConfError>;
}
```

`NginxAdapter` impls `WebServerAdapter`; `PhpFpmRuntime` impls `PhpRuntimeAdapter` (macOS: `Some(pool config)` + `php-fpm -t`). Windows adapters land in the enablement phase (nginx upstream-block render; `PhpRuntimeAdapter` returning `None` + a `php-cgi` invocation smoke). The two-trait split, `upstream_name`, `Send + Sync`, and the OS-branch-in-Rust rule are the Windows consult's required-now shape so nothing breaks when Windows lands.

## 5. Boundary validation & path rendering

`RenderCtx::new` validates at the boundary (fail closed before any render): `server_name` matches a hostname charset (`[a-z0-9.-]`, a dot like `myapp.localhost` is fine), `php_major` is a safe path component (reuse the P0-6-style rule — `[a-z0-9._-]`, not `.`/`..`), `TcpPorts` is non-empty when present.

**`to_config_path(p: &Path) -> Result<String, ConfError>`** is the single render-time chokepoint for every path entering a template: it (a) rejects non-UTF-8 (`ConfError::PathNotUtf8` — Tera cannot render non-UTF-8, and this avoids each adapter re-inventing a `to_str()` check), (b) replaces `\` with `/` and strips a `\\?\`/`\\?\UNC\` extended-length prefix (nginx's parser understands neither) — a no-op on macOS, load-bearing on Windows (master plan line 483). Every path substituted into a template goes through it.

**Socket-length guard (macOS consult E):** `validate()` runs against a SHORT throwaway `/tmp` home, so a clean `-t` there is **not** a proxy for the real home's socket length (Darwin `sun_path` ≤ 104; both `nginx -t` and `php-fpm -t` reject over-long socket paths at parse time). The authoritative `MAX_SOCKET_PATH_BYTES ≤ 103` check against the REAL home stays mandatory and independent at the caller (as in `demo_stack.rs`); a comment in P0-7 records that `-t` does not cover it.

## 6. The three templates

Files under `templates/`, embedded via `include_str!`. Tera instance built with **autoescape OFF** (these are `.conf`, not HTML — `Tera::default()` / no HTML-suffix autoescaping). Every file opens with a **DO-NOT-EDIT banner naming the exact `config/custom/...` path** the user should edit instead. **nginx banners use `#`; the php-fpm banner MUST use `;`** — a `#` first line hard-fails `php-fpm -t` with a ZEND_INI parser error (macOS consult G1, empirically confirmed).

### 6.1 nginx main → `config/generated/nginx/nginx.conf`
Folds P0-4's proven directives: `daemon off;`, `worker_processes 1;`, `pid "{home}/run/nginx.pid";`, `error_log "{home}/logs/nginx.error.log" warn;` + `error_log stderr notice;`, `events {}`, and `http { access_log "{home}/logs/nginx.access.log"; ` all five `*_temp_path "{home}/run/nginx/…";` ` include "{home}/config/generated/nginx/sites/*.conf"; include "{home}/config/custom/sites/*.conf"; }` (generated first, user override after). **Every directive value that embeds a path is double-quoted** — nginx's lexer splits on unquoted whitespace, so a home path containing a space breaks every directive (macOS consult G2, empirically confirmed; php-fpm's `key = value` needs no quoting). Zero-match `include` globs pass `nginx -t` silently (consult B).

### 6.2 nginx site → `config/generated/nginx/sites/{server_name}.conf`
`server { listen {listen_addr}; server_name {server_name}; root "{docroot}"; location / { <PHP fastcgi>; ` the P0-4-proven inline `fastcgi_param` set (SCRIPT_FILENAME `$document_root/index.php`, QUERY_STRING, REQUEST_METHOD, CONTENT_TYPE, CONTENT_LENGTH, SERVER_PROTOCOL, REMOTE_ADDR, SERVER_NAME, SERVER_PORT); ` include "{home}/config/custom/sites/{server_name}.d/*.conf"; } }`. The `<PHP fastcgi>` block is chosen by a Rust `match ctx.php_upstream` in the adapter:
- `UnixSocket(p)` → `fastcgi_pass "unix:{to_config_path(p)}";` (macOS, implemented + validated).
- `TcpPorts(addrs)` → an `upstream {upstream_name} { server 127.0.0.1:PORT max_fails=1 fail_timeout=1s; … }` block (emitted at http scope) + `fastcgi_pass {upstream_name};` + `fastcgi_next_upstream error timeout invalid_header http_500;` — the `next_upstream`/`max_fails` are correctness-load-bearing, not tuning: without them nginx round-robining into a mid-respawn php-cgi worker surfaces as user 502s (Windows consult A). Defined now, runtime-deferred; unit-shaped only.

### 6.3 php-fpm pool → `config/generated/php/{php_major}/php-fpm.conf`
`;`-banner, then `[global] error_log = {home}/logs/php-fpm.log` · `[www] listen = {socket}` (from `UnixSocket`) · `pm = ondemand` · `pm.max_children = 4` (its omission is caught by `php-fpm -t`, exit 78 — consult C) · `catch_workers_output = yes` · `include={home}/config/custom/php/{php_major}/pool.d/*.conf`. Dir is **per-major** (`php/8.4/`). Zero-match `include=` passes `-t` but emits a harmless `WARNING: Nothing matches the include pattern …` to stderr on every fresh validate — hence `ok` must be exit-code-only (consult D).

## 7. Validation flow

`validate()` writes the generated files into a throwaway `tempfile::tempdir_in("/tmp")` home (short path — sun_path headroom), pre-creates exactly `run/`, `run/nginx/`, `logs/` (NOT `www/` — `nginx -t` doesn't need it; consult A), runs the native validator, captures stderr verbatim, returns `{ ok: status.success(), stderr }`, and drops the temp home. It never touches a live config (that's apply/swap — deferred).

- **nginx:** `nginx -e "{home}/logs/nginx.error.log" -t -c "{home}/config/generated/nginx/nginx.conf"`. `-e` is MANDATORY (omitting it leaks `/opt/homebrew/var/log/nginx/` on the real system — consult A); `-p` is a confirmed no-op, omitted. Side effects contained in the temp home: `run/nginx.pid`, the five `run/nginx/*` temp dirs, and empty `logs/nginx.{error,access}.log`.
- **php-fpm:** `php-fpm -t -n -y "{pool.conf}"` (`-n` skips brew php.ini for hermetic stderr — consult C). Only `logs/` must pre-exist; `-t` writes ~100 bytes to `logs/php-fpm.log`. It does real semantic postprocessing, not just a parse.
- **Binaries** are located by probing the Homebrew `opt/` prefixes (reuse the `find_brew_binaries`-style probe from openvhost-core) — **never via `PATH`**, because ServBay shadows `nginx`/`php-fpm` in `PATH` on dev machines (consult environment note). Absent binaries → the live proof skips loudly (like P0-4/P0-6).

## 8. Testing

- **Golden-file (hermetic, every `cargo test`):** a fixed `RenderCtx` → byte-identical main/site/pool output (committed golden files). Assert: the DO-NOT-EDIT banner is present and names the custom path (`;`-style for php-fpm, `#`-style for nginx); the two `include` lines per nginx file; `fastcgi_pass "unix:…"` for the macOS upstream; every nginx path-directive value is double-quoted; all paths absolute; per-major php dir; and the `PhpUpstream::TcpPorts` branch renders a named `upstream{}` block + `next_upstream`/`max_fails` (unit — proves the seam even though it's not runtime-tested).
- **Boundary:** reject bad `server_name`/`php_major`, non-UTF-8 path (`to_config_path`), empty `TcpPorts`.
- **Live proof (exit criterion, binary-gated):** generate the stack → materialize a `/tmp` temp home → `nginx -t` passes AND `php-fpm -t` passes (exit 0), stderr captured; assert the php-fpm empty-glob WARNING does not flip `ok`. Skips loudly without the brew binaries.
- **Gates:** full local suite (fmt, clippy `-D warnings`, `cargo test --workspace`, `cargo deny check licenses advisories` — new deps `tera`, `async-trait`, `tempfile` (dev), all MIT or MIT-OR-Apache-2.0), SPDX, pnpm suite untouched-but-green. CI stays disabled (billing, P0-3 §2.3); local gates are the merge gate. macOS-first: no Windows cross-check this slice (the Windows render paths are unit-shaped, not compiled against a Windows target — deferred with the enablement phase).

## 9. Delivery

Branch `feat/p07-tera-templates` → SDD per-task (config-template-engineer implements templates; rust-core-engineer the crate glue; platform findings above are binding) → final whole-branch review → PR with the live `nginx -t`/`php-fpm -t` evidence → local gates → merge. Conventional Commits + DCO; SPDX on new source (Tera `.tera` templates carry the SPDX as a `#`/`;` comment on line 1 as their target syntax allows, or are exempted like other generated/data files — decide in the plan). No security-auditor gate (no download/helper/cert/hosts/IPC surface in this slice).
