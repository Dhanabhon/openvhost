# P0-4 — macOS nginx + php-fpm Supervision Proof — Design

- **Date:** 2026-07-22
- **Status:** Approved in brainstorming session (3 sections); platform-macos-specialist consultation verdict **APPROVE-WITH-CHANGES** (2026-07-22, every load-bearing claim verified live on the development Mac) — all required changes are folded into this document. No `ProcessDriver`/`ServiceSpec` trait change, so no dual-specialist escalation.
- **Source of truth:** `docs/OPENVHOST_MASTER_PLAN.md` v1.2 — row **P0-4**: "macOS: nginx + php-fpm serve `phpinfo()` via unix socket", exit criterion `curl http://127.0.0.1:8080` returns phpinfo. Owner: platform-macos-specialist.
- **Builds on:** the shipped P0-3 supervisor (`docs/superpowers/specs/2026-07-21-p03-supervisor-design.md`) — unix driver `process_group(0)`, graceful stop = SIGTERM → snapshotted `-pgid`, 5s deadline → SIGKILL, 500ms Running classifier, env clear-then-allow-list, ring buffer + events.

## 1. Context

The supervisor has only ever managed the deterministic test child. P0-4 de-risks the real thing on macOS: two production binaries (nginx master+workers, php-fpm master+pool) under our supervision, wired together over a unix socket in `~/.openvhost/run/`, serving PHP through the same Services panel shipped in P0-3 — with zero new UI code.

**Owner decisions already made (2026-07-22):**
- **Binary source: Homebrew** (`/opt/homebrew/opt/nginx/bin/nginx` 1.31.3, `/opt/homebrew/opt/php/sbin/php-fpm` 8.5.8 on the dev Mac; installed during brainstorming). This is a **stand-in seam**: binary paths are plain `ServiceSpec` data, swapped to `~/.openvhost/packages/` when P0-6 lands — data-only change.
- **Config source: minimal hand-written by this slice** (embedded const templates in Rust). Tera and the real config pipeline remain P0-7 / config-template-engineer property; this module carries a "P0 throwaway — superseded by P0-7" doc marker.

## 2. Goals

1. `provision_macos_demo_stack(home, port)` writes a self-contained nginx + php-fpm config set under the OpenVHost home, atomically and idempotently.
2. The desktop app registers `nginx` and `php-fpm` as supervised services next to `demo-ticker`; Start/Stop/logs/Failed-tail all work through the existing panel.
3. `curl http://127.0.0.1:8080` returns phpinfo while both run under supervision; stopping through the panel tears the whole process groups down with no orphans.
4. An integration test proves the full loop headlessly on any Mac with the brew binaries, and skips (loudly) elsewhere.

## 3. Non-goals

SIGUSR2 / `nginx -s` reload (per the P0-3 consultation: service-level, later slice) · real health probes (the 500ms classifier stays, documented) · MySQL/MariaDB · dependency ordering between fpm and nginx (nginx 502s until the socket exists; per-request connect) · Windows anything · Tera/openvhost-conf work (P0-7) · package download (P0-6) · orphan cleanup on app crash (P0-8).

## 4. Provision module

**Location:** `crates/openvhost-core/src/platform/macos/demo_stack.rs` (new `platform/macos/` tree, `#[cfg(target_os = "macos")]`) — matches the plan §6.2 ownership glob so platform-macos-specialist owns it; openvhost-core stays tauri-free. Module doc: *"P0 throwaway — superseded by openvhost-conf/Tera templates (P0-7)."*

**Signature (shape, not frozen):**
```rust
pub struct StackPaths { pub nginx_conf: PathBuf, pub fpm_conf: PathBuf, pub docroot: PathBuf,
                        pub socket: PathBuf, pub nginx_error_log: PathBuf, pub port: u16 }
pub fn provision_macos_demo_stack(home: &Path, port: u16) -> Result<StackPaths, CoreError>
```

**Behavior:**
1. Create `conf/`, `www/`, `run/`, **`run/nginx/`** (nginx only mkdirs the *leaf* temp dir — a missing parent is an instant `[emerg]` exit; provision must pre-create it), `logs/`.
2. **Validate the socket path is ≤ 103 bytes** and fail with a clear error otherwise. Rationale (specialist-proven): Darwin `sun_path` is 104 bytes; php-fpm does **not** fail on overflow — it *warns, silently truncates, binds the wrong path, and keeps running*, while nginx 502s forever against a healthy-looking fpm. The check protects long real homes too.
3. Write `conf/nginx.conf`, `conf/php-fpm.conf`, `www/index.php` (`<?php phpinfo();`) — **atomic: temp file created in the same directory as the target, then rename** (never TMPDIR; same-volume rename), overwrite-always so reruns are deterministic.

**nginx.conf (rendered with absolute `{home}` and `{port}`):**
```nginx
daemon off;
worker_processes 1;
pid {home}/run/nginx.pid;
error_log {home}/logs/nginx.error.log warn;
error_log stderr notice;            # master lines also reach the supervisor ring buffer

events {}

http {
    access_log {home}/logs/nginx.access.log;
    client_body_temp_path {home}/run/nginx/client_body;
    proxy_temp_path       {home}/run/nginx/proxy;
    fastcgi_temp_path     {home}/run/nginx/fastcgi;
    uwsgi_temp_path       {home}/run/nginx/uwsgi;
    scgi_temp_path        {home}/run/nginx/scgi;

    server {
        listen 127.0.0.1:{port};
        root {home}/www;
        location / {
            fastcgi_pass unix:{home}/run/php-fpm.sock;
            fastcgi_param SCRIPT_FILENAME $document_root/index.php;
            fastcgi_param QUERY_STRING    $query_string;
            fastcgi_param REQUEST_METHOD  $request_method;
            fastcgi_param CONTENT_TYPE    $content_type;
            fastcgi_param CONTENT_LENGTH  $content_length;
            fastcgi_param SERVER_PROTOCOL $server_protocol;
            fastcgi_param REMOTE_ADDR     $remote_addr;
            fastcgi_param SERVER_NAME     $server_name;
            fastcgi_param SERVER_PORT     $server_port;
        }
    }
}
```
Deliberate omissions (specialist-verified): no `include mime.types;` (file not provisioned; FastCGI supplies Content-Type), no `include fastcgi_params;` (inlined above — the minimal set that serves full phpinfo), **all five** `*_temp_path` pinned because brew compiles absolute `/opt/homebrew/var/...` defaults that `-p` does *not* remap.

**php-fpm.conf:**
```ini
[global]
error_log = {home}/logs/php-fpm.log   ; required — compiled default points into /opt/homebrew/var

[www]
listen = {home}/run/php-fpm.sock
pm = ondemand
pm.max_children = 4                   ; required — omitting it is a startup FATAL
catch_workers_output = yes
```
Deliberate omissions: **no `user`/`group`** (present + non-root = runtime warning noise that `php-fpm -t` does not surface), no `pid` (no pid file needed; fpm unlinks a stale socket before bind, so crash recovery is self-healing).

## 5. Service specs & app wiring

Helper `find_brew_binaries() -> Option<BrewStack>` probes **both** prefixes (`/opt/homebrew/opt/...` Apple Silicon, `/usr/local/opt/...` Intel), resolving the `opt/` symlinks at registration time (they silently retarget on major version bumps). Spec data:

| id | program | args |
|---|---|---|
| `php-fpm` | `<prefix>/opt/php/sbin/php-fpm` | `-F -O -n -y {home}/conf/php-fpm.conf` |
| `nginx` | `<prefix>/opt/nginx/bin/nginx` | `-e {home}/logs/nginx.error.log -c {home}/conf/nginx.conf` |

- `-F` foreground; **`-O`** forces master lines to piped stderr (otherwise the supervisor tail is blind in nodaemonize mode; the `error_log` file will exist but stay empty — accepted trade-off); **`-n`** skips brew `php.ini`/`conf.d` for hermetic behavior (timezone silently UTC — fine for phpinfo).
- **`-e`** (nginx ≥ 1.19.5) keeps the *pre-config-read* window inside our home — without it nginx opens `/opt/homebrew/var/log/nginx/error.log` before ever reading `-c`.
- Endpoints shown in the panel: `http://127.0.0.1:8080` and `run/php-fpm.sock`.

**App wiring (`lib.rs` setup, `#[cfg(target_os = "macos")]` block):** resolve home → `provision_macos_demo_stack(home, 8080)` → register php-fpm and nginx and keep the dependency-free demo-ticker (the panel orders rows by id — demo-ticker, nginx, php-fpm — because Supervisor::snapshot() sorts by id; registration order is irrelevant). Provision errors are **logged and never crash the app** (the two rows are still registered; Start then yields an honest Failed naming the missing path — the P0-3 spawn-fail contract). Machines without brew binaries get the same honest-Failed demo of the error path. Non-macOS builds register only the ticker.

**Invariant (specialist-mandated):** no PATH-dependent behavior anywhere — every path in specs and configs is absolute, so the stack behaves identically from a terminal `dev.sh` run and a packaged `.app` under launchd's bare GUI env. Proven to serve phpinfo under exactly `{PATH=/usr/bin:/bin, HOME, LANG=C}`; the P0-3 allow-list needs **no additions**.

## 6. Verification

**Unit tests (core):** socket-path validation (accept short, reject > 103 with the clear error), provision file contents (golden substrings: `daemon off;`, all five temp paths, `pm.max_children`, no `user =`), idempotent rerun, atomic same-dir temp naming.

**Integration test `crates/openvhost-core/tests/macos_stack.rs`** (dev-deps: `openvhost-proc`, `tokio`, `tempfile` — dev-only, no cycle):
1. Whole file `#[cfg(target_os = "macos")]`; at runtime **skip with an eprintln'd reason** when `find_brew_binaries()` is `None` (ubuntu CI, brew-less Macs) — *not* `#[ignore]`, so `cargo test --workspace` on the dev Mac always exercises it.
2. Home = `tempfile::Builder::new().prefix("ovh").tempdir_in("/tmp")` — **never TMPDIR** (`/var/folders/...` is brittle-long) and never harness scratchpads (measured 125 B > limit); `/tmp` keeps the socket ≈ 33 bytes.
3. Ephemeral port (bind `:0`, snapshot, drop). Provision → start `php-fpm` → poll socket file exists (≤ 5s) → start `nginx` → poll `/usr/bin/curl` (no new deps) until HTTP 200 + body contains `phpinfo` (≤ 10s) → stop `nginx`, stop `php-fpm` → assert both `Stopped`, socket unlinked, subsequent curl fails. Whole-test budget ≤ 30s.

**Manual smoke (exit criterion):** `./scripts/dev.sh` → Start php-fpm → Start nginx → `http://127.0.0.1:8080` shows phpinfo → Stop both → both `stopped`, no `nginx`/`php-fpm` processes left (`pgrep`).

**Gates:** the full local suite (fmt, clippy `-D warnings`, `cargo test --workspace`, deny, SPDX, pnpm lint/check/test/build). CI workflow remains disabled (billing block — owner decision recorded in the P0-3 spec §2.3); same stand-in policy: local gates + clean `cargo check --target x86_64-pc-windows-msvc -p openvhost-core` (this slice is cfg-gated; the cross-check proves the Windows build is untouched). macOS matrix backfills on Actions restoration.

## 7. Risks & documented behaviors

| Risk / behavior | Handling |
|---|---|
| Port 8080 busy (e.g. ServBay's nginx started) | nginx retries bind 5× at 500ms and exits after ~2.6s → the UI shows **Starting → Running (~500ms) → Failed (~2.6s)** with the `bind() ... (48: Address already in use)` tail. Documented, accepted v0 classifier behavior — an honest, visible error, not a bug. fpm has no equivalent (unlink-before-bind). |
| Socket path > 103 bytes | Provision refuses with a clear error (fpm's silent-truncation trap makes this the worst failure mode; see §4). |
| Group-TERM vs master-orchestrated shutdown | SIGTERM to `-pgid` hits master and workers at once. Specialist-proven: both masters exit 0 in ~10ms, zero orphans (ESRCH on the group), nginx unlinks its pid, fpm its socket. Identical signal nginx's own fast shutdown sends workers. Real graceful (SIGQUIT) is later per-service-adapter work. |
| `opt/` symlink drift on major bumps | Resolved at registration time; replaced by `packages/` in P0-6. |
| App quit without Stop | Orphaned masters keep running — the known P0-8 gap, unchanged by this slice. |
| `nginx -t` is not side-effect-free (creates temp dirs) | Irrelevant here; **recorded for P0-7's** future validate flow. |
| P0-3 minor carry-overs (reader generation token, control_rx startup race, store pid refresh) | Out of scope unless implementation trips on them; they stay in the fix-soon pool. |

## 8. Delivery

Branch `feat/p04-macos-stack`, SDD per-task with fresh implementer + reviewer, final whole-branch review, PR to main, local gates green, manual smoke, squash-merge. Conventional Commits + DCO, SPDX headers on new source files (embedded config templates are string constants inside `.rs` files; generated runtime files in `~/.openvhost` carry no headers).
