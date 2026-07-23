# P0-9 — Hermetic E2E Integration Harness (design)

**Status:** approved design, 2026-07-24. Last macOS-first Phase 0 slice.

## 1. Goal & context

Master-plan P0-9 exit criterion: *"Integration test harness: start → HTTP assert → stop … one E2E test runs in CI on both OS."*

A live start→serve→stop proof already exists — `crates/openvhost-core/tests/macos_stack.rs` (P0-4) runs real nginx + php-fpm under the supervisor, `curl`s `phpinfo()`, and asserts clean teardown. But it **auto-skips when the Homebrew binaries are absent**, so it cannot satisfy *"runs in CI on both OS."* P0-9's distinct job is a **hermetic** E2E that runs **unconditionally** on any OS with **no external prerequisites** (no Homebrew, no network, no new dependency), proving the P0-3 supervise→serve→stop loop end-to-end through a real socket and HTTP client.

## 2. Scope decisions (owner-approved)

- **Hermetic, not live.** The test supervises a tiny in-repo HTTP server; it does not depend on service binaries. `macos_stack.rs` stays as the separate live/high-fidelity opt-in (untouched).
- **Pure supervise→serve→stop.** No crash/relaunch/orphan-reap in this flow (that stays covered by `crates/openvhost-proc/tests/orphan_reap.rs`); no `macos_stack.rs` refactor; no config generation (P0-7 config-gen belongs to the live nginx path — a hermetic server serves HTTP directly, so there is no nginx config to render).
- **No new dependency.** The HTTP responder and the HTTP client are raw `std::net` (~30 lines each) — no `tiny_http`/`hyper`/`reqwest`, keeping the license gate untouched.
- **No security surface** (no kill-from-file, download, helper, cert, hosts, IPC) → **no security-auditor gate** for this slice.

## 3. Components

### 3.1 HTTP-serve mode on the existing `testchild` (`crates/openvhost-proc/src/testchild.rs`)

`testchild` is the canonical managed-child test binary (`proc_testchild`, spawned via `CARGO_BIN_EXE_proc_testchild`), a `parse(&[String]) -> TestchildArgs` + `run(TestchildArgs) -> i32` pair. Extend it minimally — no new binary target:

- Add `http_port: Option<u16>` to `TestchildArgs` (default `None`).
- `parse`: recognize `--http <port>` (parse `u16`; reject non-numeric with the existing error style).
- `run`: as its FIRST action, `if let Some(port) = args.http_port { return serve_http(port); }` — so HTTP mode fully replaces the tick loop.
- `serve_http(port) -> i32`: `TcpListener::bind(("127.0.0.1", port))`; on bind error → `eprintln!` + return `1` (the service then goes `Failed`, surfacing the problem loudly rather than hanging). On success, loop: `accept()` → read and discard up to a bounded number of request bytes (best-effort; ignore read errors) → write a fixed response and drop the stream. The loop never returns normally; the supervisor's stop path (SIGTERM on unix / console-ctrl/terminate on Windows) ends the process — no signal handling is added (matches today's default-terminate behavior for a child not passed `--ignore-stop`).
- **Fixed response** (exact bytes): status line `HTTP/1.1 200 OK`, headers `Content-Length: <len>` and `Connection: close`, blank line, then the body **sentinel** `openvhost-e2e-ok`. The sentinel is a single shared `const` so the server and the test assert on the same literal.

A bin-level test (in `tests/testchild_bin.rs`, alongside the existing ones) spawns `proc_testchild --http <port>` directly, `http_get`s it, asserts `200` + sentinel, then kills it — proving the server mode in isolation from the supervisor.

### 3.2 E2E test + harness helpers (`crates/openvhost-proc/tests/e2e.rs`, new)

Self-contained helpers (colocated; no shared-crate extraction in this slice):

- **Shared sentinel:** T1 defines `pub const E2E_BODY: &str = "openvhost-e2e-ok"` in `testchild.rs`, exported from the crate root (make the module / const `pub` as needed) so the server (§3.1) and `e2e.rs` assert on ONE literal — no drift.
- `ephemeral_port() -> u16` — bind `127.0.0.1:0`, read the assigned port, drop the listener, return it (same proven pattern as `macos_stack.rs`; the tiny reuse race manifests as a loud `Failed`/deadline failure, never a hang).
- `http_get(port, deadline) -> Option<String>` — poll: `TcpStream::connect` with a short connect+read timeout → write `GET / HTTP/1.0\r\nHost: localhost\r\nConnection: close\r\n\r\n` → read the whole response to a `String` → return `Some(resp)` once it is obtained, retrying until `deadline`. This is the cross-OS replacement for `macos_stack.rs`'s shell-out to `/usr/bin/curl` (no `curl` dependency; identical on macOS and Windows).
- **Concrete bounds (tunable):** per-attempt connect+read timeout ~1s; overall `http_get` serve deadline ~10s (matches `macos_stack.rs`'s curl deadline, absorbing first-accept lag); `Running`/`Stopped` state-wait deadlines ~5s; teardown `http_get` short deadline ~2s; `serve_http` reads up to ~1 KiB of the request before responding.
- `StopGuard` (RAII) — fires `stop` for the service on drop so a mid-test panic never leaks the child (mirrors the proven `macos_stack.rs::StopGuard`).

**The E2E flow** (`#[tokio::test(flavor = "multi_thread")]` — the multi-thread flavor is required, per the P0-8 lesson that a blocking-wait RAII guard starves a single-thread runtime):

1. `let port = ephemeral_port();`
2. `SpawnSpec { program: CARGO_BIN_EXE_proc_testchild, args: ["--http", &port.to_string()], cwd: None, env: vec![] }` inside a `ServiceSpec` (id `"http-e2e"`).
3. `Supervisor::new(default_driver())`; install the `StopGuard`; `register`; `start("http-e2e")`.
4. Wait until the service reaches `Running` (deadline-bounded poll of `snapshot()`), capturing its `pid`.
5. `http_get(port, deadline)` until it returns a response containing `200` and `E2E_BODY`. Assert both.
6. `stop("http-e2e")`; wait until `Stopped` (deadline-bounded).
7. **Teardown asserts:** `http_get(port, short_deadline)` now returns `None` (port no longer served — the cross-OS proof the process is gone / not orphaned), and the final state is `Stopped`.

Everything uses the existing P0-3 public API (`Supervisor`, `ServiceSpec`, `SpawnSpec`, `ServiceState`, `default_driver`, `snapshot`, `start`, `stop`) — no new production surface in the supervisor.

## 4. Cross-platform & exit-criterion mapping

- **macOS (now):** the E2E runs and passes **unconditionally** (no skip) — this is the active macOS-first exit proof.
- **Windows compile gate:** `cargo check --target x86_64-pc-windows-msvc -p openvhost-proc` stays clean (raw `std::net` is cross-platform; `testchild`'s Windows ctrl-handler path is unchanged).
- **Windows runtime:** the E2E is written to run on Windows too, but the `WindowsDriver` runtime was shipped in P0-3 without a live Windows run (no Windows machine; CI disabled). So the Windows *execution* of this E2E rides the **deferred CI matrix** (Windows-enablement phase), consistent with every prior macOS-first slice. The spec claims macOS-runs + msvc-compiles; it does **not** claim a verified Windows run.
- CI is disabled (billing); the **full local gate suite is the merge gate**, exactly as for P0-3…P0-8.

## 5. Testing & verification

The E2E test **is** the deliverable; there is no separate "test the test." Verification = the standard gate suite, all green on macOS with the new tests running (not skipping):
`cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh && pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build`, plus the msvc cross-check. After the run, no leaked `proc_testchild` processes.

## 6. Non-goals / deferred

- No crash/relaunch/orphan-reap in this E2E (covered by `orphan_reap.rs`).
- No refactor of `macos_stack.rs` to share the harness helpers (possible future DRY; not now).
- No reusable cross-crate test-support crate (YAGNI; helpers are colocated in `e2e.rs`).
- No config generation / real web server in the hermetic path (that is the live `macos_stack.rs` fidelity path; wiring it to the P0-7 generated config is a separate future improvement).
- Verified Windows runtime (deferred to the Windows-enablement CI matrix).

## 7. Delivery constraints

- Branch `feat/p09-e2e-harness` off `main`. SPDX `// SPDX-License-Identifier: GPL-3.0-or-later` line 1 on `tests/e2e.rs`. No `unwrap()`/`expect()` outside `#[cfg(test)]` (test code may use them under the existing `#[allow(clippy::unwrap_used)]` convention). Every `unsafe` block (none expected here) carries a `// SAFETY:` comment. `openvhost-proc` stays tauri-free. DCO `git commit -s`, no `Co-Authored-By`, Conventional Commits.
- Likely 2 tasks: (T1) `testchild` HTTP mode + its bin-level test; (T2) the `e2e.rs` supervisor serve→stop test + msvc cross-check + full gate + PR. No security-auditor gate.
