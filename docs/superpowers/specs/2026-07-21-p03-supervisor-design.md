# P0-3 — `openvhost-proc` Supervisor v0 — Design

- **Date:** 2026-07-21
- **Status:** Approved in brainstorming session (3 sections + dual platform-specialist consultation); pending user review of this document
- **Master plan:** P0-3 row (§4) — exit criterion "generic child process managed with state machine"; responsibilities §3.1
- **UI contract input:** `docs/design/README.md` — "What these screens demand from the supervisor"
- **Consultations (plan §6.1, completed 2026-07-21):** platform-macos-specialist — SIGNATURE OK AS-IS + companion decisions; platform-windows-specialist — one signature change (opaque `SpawnedChild`), adopted below. Both reviews' behavioral contracts are folded into §5.

## 1. Scope

`openvhost-proc` becomes real: spawn/stop/status for a generic child process with the `Stopped → Starting → Running → Failed` state machine, live log capture into per-service ring buffers, a broadcast event stream — plus the slice proves it end-to-end with a real **Services panel v0** in the desktop app driven by a real test process. Chosen over tests-only scope to exercise the full UI→IPC→supervisor contract now.

## 2. Exit criteria

1. `Supervisor` manages a generic child on macOS and Linux (CI): all four states reachable and observed via events; graceful stop and kill paths both proven; zero orphans after stop (pgid probe).
2. Desktop app shows Services panel v0: real `demo-ticker` service with state pill, Start/Stop/Retry per state, live log tail, failed block with bounded stderr tail + forward action.
3. Full local gate suite green; **one on-demand CI matrix run green before merge** (first real Windows code — re-enable workflow, `workflow_dispatch`, then re-disable if desired).
4. Unit + integration tests per §8; TDD for the state machine.

## 3. Architecture

Inside `crates/openvhost-proc` (tokio added; still tauri-free):

- **`Supervisor`** — owns the service registry (in-memory, programmatic `register()` in v0), hands out snapshots, routes start/stop, owns the single `tokio::sync::broadcast` event channel.
- **`ServiceTask`** — one tokio task per running service: drives spawn via the platform driver, drains stdout/stderr immediately (pipe buffers are ~64KB), applies the state machine, emits events.
- **`platform/`** — `ProcessDriver` trait + `unix.rs` / `windows.rs` impls (§5). Core code contains no inline OS branches.
- **`events`/`log`** — event types; per-service ring buffer (`VecDeque`, **2,000 lines**, drop-oldest).

Public API (consumed by desktop now, CLI later):

```rust
Supervisor::new(driver: Arc<dyn ProcessDriver>)
  .register(ServiceSpec { id, display_name, spawn: SpawnSpec })
  .start(&self, id) -> Result<(), ProcError>      // no-op if Starting/Running
  .stop(&self, id) -> Result<(), ProcError>       // two-phase; cancel-safe during Starting
  .snapshot(&self) -> Vec<ServiceStatus>          // initial UI state
  .log_tail(&self, id, n) -> Vec<LogLine>
  .subscribe(&self) -> broadcast::Receiver<SupervisorEvent>
```

`SupervisorEvent`: `StateChanged { id, state: ServiceState, detail: Option<String> }` · `Log { id, ts_ms, level: LogLevel, line }`. `ServiceState::Failed { exit: Option<i32>, stderr_tail: Vec<String> /* last 10 */ }`. The supervisor writes its own lifecycle lines into the log stream ("state Stopped → Starting (requested by user)") per the UI contract.

## 4. State machine

- spawn ok → **Starting**; alive for 500ms → **Running**. The 500ms check is a **raced bound** (`tokio::select!` between the timer and `child.wait()`) — never sleep-then-poll; death during the window reports instantly.
- Exit code ≠ 0, or any exit during Starting → **Failed** with exit code + last-10 stderr tail.
- Clean exit (code 0) while Running → **Stopped**.
- `stop()` = set **stop-requested flag first**, then `request_graceful_stop` → 5s deadline → `kill` → **Stopped**. The flag is consulted before exit-status classification so a timeout-kill during a requested stop is recorded as Stopped, never Failed (macOS review finding).
- Retry is `start()` from Failed. Starting is cancel-safe (stop during Starting takes the same two-phase path).
- The 500ms alive-check is explicitly provisional: P0-4 layers real readiness probes (socket/port connect) above the driver; the timer remains the no-probe fallback.

## 5. Platform layer (post-consultation form)

```rust
pub struct SpawnSpec {
    pub program: PathBuf,          // ALWAYS fully-resolved absolute; never $PATH lookup
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    pub env: Vec<(OsString, OsString)>,
}

pub struct SpawnedChild { /* all fields private — opaque (Windows review, blocking) */ }
impl SpawnedChild {
    pub fn id(&self) -> Option<u32>;
    pub fn take_stdout(&mut self) -> Option<OutputStream>;  // OutputStream = opaque newtype
    pub fn take_stderr(&mut self) -> Option<OutputStream>;  // implementing AsyncRead — keeps
                                                            // the tokio types out of the API
    pub async fn wait(&mut self) -> io::Result<ExitStatus>;
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>>;
}

pub trait ProcessDriver: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild>;
    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()>;
    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()>;
}
```

Binding behavioral contracts (doc-comments on the trait/impls; agreed by both specialists):

- **stdin is always `Stdio::null()`** — never inherited.
- **Environment: clear-then-allow-list.** Base allow-list `PATH, HOME, TMPDIR, LANG` (unix); Windows additionally re-injects `SystemRoot, windir, TEMP, TMP` and a minimal System32 `PATH` (CRT startup fails without them). `spec.env` applies on top. Reproducible-environments principle over ambient inheritance.
- **Unix containment:** `Command::process_group(0)` (atomic via posix_spawn attribute — not `pre_exec`; closes the signal-before-group-exists ESRCH race). `PlatformHandle` snapshots `{pid, pgid}` at spawn; signals target `-pgid` via nix/libc from the snapshot — never re-derived from `child.id()` (None after reap). `kill` = SIGKILL to the group; **never** `tokio::process::Child::kill()` (direct child only — orphans grandchildren).
- **Windows v0:** `CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW` always (console-window flashes otherwise); graceful = opportunistic `GenerateConsoleCtrlEvent(CTRL_BREAK, pid)` — works in dev consoles; **the packaged GUI app (`windows_subsystem="windows"`, no console) is documented hard-kill-only in v0/v1**. `kill` = `TerminateProcess` in v0; the doc-comment states that from P0-5 it means `TerminateJobObject` on the app-wide Job Object (one job per app, not per child) and must never be simplified back. FFI via `windows-sys` (already in-tree through tokio), not the COM `windows` crate. Job assignment in P0-5 is spawn-then-assign with a documented bounded containment race (correctness/cleanup guarantee, not a security boundary); the opaque `SpawnedChild` is what lets P0-5 switch to raw `CreateProcessW` (suspended→assign→resume needs the thread handle std discards) without a breaking change — verify with a spike at P0-5 start.
- **Signal-delivery errors** meaning "no such process/group" → treat as "check `try_wait()` now", not failure.
- **`request_graceful_stop` is the LCD fallback**, not the graceful-shutdown mechanism: real services get protocol shutdown (mysql admin command, `nginx -s quit`) at their per-service adapter layer in later slices.
- **Managed services must run in the foreground** (no self-daemonize/setsid — e.g. php-fpm always `--nodaemonize`); a daemonizing child escapes the containment group and stop would lie. Documented `SpawnSpec` usage contract.
- **Reload (SIGUSR2) is not a trait method** — it lands in `platform/macos` (P0-4) as a pid-targeted capability; if generic dispatch is ever needed it becomes a closed intent enum, never a raw send-any-signal primitive.
- Deferred with pointers: fd-inheritance for the Phase 3 80/443 handoff (one plausible future `SpawnSpec` field); long-path `\\?\` prefixing inside the Windows `spawn`; `RLIMIT_NOFILE` for Phase 1 databases.

## 6. Desktop integration

- Commands (thin, `Result<_, IpcError>`): `list_services()`, `start_service(id)`, `stop_service(id)`, `service_log_tail(id, n)`.
- **Typed events via tauri-specta**: `ServiceStateEvent { id, state, detail }`, `ServiceLogEvent { id, ts_ms, level, line }` — bridged in the Tauri setup hook from `Supervisor::subscribe()`. Bindings regenerate through the existing `export_bindings` test; the CI drift gate already covers the file.
- `openvhost-proc` stays tauri-free; the desktop crate owns the bridge. DTOs derive specta behind the same optional-feature pattern as `CoreInfo`.

## 7. Services panel v0 (UI)

Replaces the demo page as the app's main view, using the mockup vocabulary (interim Tailwind approximations; real tokens.css is Phase 1):

- Service rows: display name, endpoint/args in mono, status pill (running/starting/failed/stopped), per-state actions (Start / Stop / Retry; Starting shows Stop).
- Live log tail: last 50 lines, JetBrains Mono stack, auto-follow, level coloring (text-safe values).
- Failed block: headline + last-10 stderr tail + forward action (Retry).
- One registered service: **`demo-ticker`** running `openvhost __testchild` — a hidden CLI subcommand (`--lines N --interval MS --exit CODE --ignore-stop --fail-after N`) giving a deterministic cross-platform child; `--ignore-stop` on Windows must actually `SetConsoleCtrlHandler(TRUE)` so the test exercises the real kill path (Windows review).
- `core_info` shrinks to a statusline at the bottom (command and its tests remain).

## 8. Testing

- **Unit (TDD):** state-machine transitions incl. stop-requested-flag classification; ring-buffer truncation; env allow-list assembly; raced 500ms bound (Failed reported before the timer elapses on instant-death).
- **Integration (ubuntu CI + macOS dev):** spawn testchild → assert `Starting→Running` event order → graceful stop path; `--ignore-stop` → kill path, final state Stopped (not Failed); `--exit 1` → Failed with correct tail; **zero-orphan probe** (`kill(-pgid, 0)` → ESRCH after stop). Hermetic via temp dirs; poll-with-timeout, never sleep-and-hope.
- **Windows:** `#[cfg(windows)]` driver tests written in-slice; validated by **one on-demand matrix run before merge** (workflow enable → dispatch → optionally re-disable). This is the slice's CI-cost decision, agreed in advance.
- **Frontend:** vitest for the event→store mapping (mocked events); manual smoke via `./scripts/dev.sh` — click through all four states.

## 9. Error handling

Commands map `ProcError` → the established `IpcError { kind, message }` pattern. Spawn failures (missing binary, permissions) land as Failed with a detail line that names the path and the next action. All failure surfaces follow "errors explain and point forward".

## 10. Non-goals

Health checks/readiness probes (P0-4) · auto-restart policy · PID persistence and cross-session orphan reaping (P0-8) · real Job Objects (P0-5) · reload/SIGUSR2 (P0-4) · config-driven service registry (arrives with P0-4/5) · any `state.db`/sqlx usage (supervisor is in-memory in v0) · tray icon.

## 11. Risks

| Risk | Mitigation |
|---|---|
| tokio `process_group` availability/behavior differs on pinned 1.53 | Verify against pinned-version docs at implementation start (macOS review caveat); pre_exec+setpgid is the documented fallback with its known hazards |
| Windows graceful-stop expectations | Honesty by design: documented hard-kill-only for packaged GUI; testchild validates the kill path |
| First Windows code with CI normally disabled | Mandatory on-demand matrix run gates the merge (exit criterion 3) |
| Event flooding from chatty children | Ring buffer bounds memory; UI tail bounded at 50; event channel is lossy-by-design broadcast (lagged receivers documented) |

## 12. After this slice

P0-4 (macOS nginx+php-fpm, readiness probes, reload capability) and P0-5 (Job Objects + php-cgi pool) proceed **in parallel** per plan §6.1, both building on this trait without signature changes — that is what the consultation bought us.
