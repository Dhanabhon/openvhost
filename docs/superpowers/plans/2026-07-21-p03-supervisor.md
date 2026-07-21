# P0-3 Supervisor v0 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `openvhost-proc` v0 — spawn/stop/status with the Stopped→Starting→Running→Failed state machine, ring-buffer log capture, broadcast events — proven end-to-end by a real Services panel in the desktop app driving a deterministic test child.

**Architecture:** `Supervisor` (registry + broadcast events) spawns one `ServiceTask` per running service; all process operations go through the `ProcessDriver` platform trait (post-consultation form: opaque `SpawnedChild`, pgid/handle snapshots, documented behavioral contracts). The desktop crate bridges events to tauri-specta typed events; `openvhost-proc` stays tauri-free.

**Tech Stack:** tokio 1.53 (process/sync/time/io-util), thiserror, libc (unix), windows-sys (win), tauri-specta events, Svelte 5 runes store.

**Spec:** `docs/superpowers/specs/2026-07-21-p03-supervisor-design.md` (§ references below point there)

## Global Constraints

- Branch: `feat/p03-supervisor`; every commit Conventional Commits + DCO (`git commit -s`); do not push until Task 7.
- SPDX `GPL-3.0-or-later` first-line comment on every new `.rs`/`.ts`/`.svelte` file (established repo rule).
- No `unwrap()`/`expect()` outside `#[cfg(test)]` (workspace lint promotes to error in CI); `thiserror` in lib crates; test modules carry `#[allow(clippy::unwrap_used)]`.
- `openvhost-proc` and `openvhost-core` must never depend on tauri (Task 4 extends the CI guard to proc).
- Behavioral contracts from spec §5 are binding: stdin always null; env = clear-then-allow-list (`PATH,HOME,TMPDIR,LANG` unix; + `SystemRoot,windir,TEMP,TMP` and `C:\Windows\System32`-style PATH on Windows) + spec.env on top; `program` absolute; unix containment via `process_group(0)` and group-targeted signals from snapshotted pgid; Windows `CREATE_NEW_PROCESS_GROUP|CREATE_NO_WINDOW`, packaged-GUI graceful documented hard-kill-only; ESRCH ⇒ try_wait, not error; never `tokio::process::Child::kill()` semantics for group kill.
- State machine per spec §4: raced 500ms bound via `select!`; stop-requested flag recorded before exit classification; stop = graceful → 5s → kill; Failed carries exit code + last-10 stderr tail.
- Ring buffer 2,000 lines/service; UI log tail 50; stderr tail 10.
- Full local gates before every commit that touches the area: `cargo fmt --check`, `clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check licenses advisories`, `bash scripts/check-spdx.sh`; frontend tasks add `pnpm -C apps/desktop lint/check/test/build`.
- Merge gate (Task 7): one on-demand CI matrix run green (workflow enable → dispatch → optionally re-disable).

---

## File Structure

```
crates/openvhost-proc/
├── Cargo.toml                  # tokio, thiserror, serde; libc(unix), windows-sys(win); optional specta
└── src/
    ├── lib.rs                  # crate doc + re-exports
    ├── error.rs                # ProcError
    ├── events.rs               # ServiceState, LogLevel, LogLine, SupervisorEvent, ServiceStatus (serde+optional specta)
    ├── log.rs                  # RingBuffer (2000), level heuristic
    ├── state.rs                # pure exit-classification (stop_flag × exit status × phase)
    ├── supervisor.rs           # Supervisor: registry, start/stop/snapshot/log_tail/subscribe
    ├── service_task.rs         # per-service task: spawn → raced 500ms → run loop → classify
    ├── testchild.rs            # pub deterministic child (args parse + run); shared by both bins
    ├── bin/proc_testchild.rs   # thin wrapper → CARGO_BIN_EXE for this crate's integration tests
    └── platform/
        ├── mod.rs              # SpawnSpec, OutputStream, PlatformHandle, SpawnedChild, ProcessDriver, env allow-list
        ├── unix.rs             # UnixDriver (process_group(0), SIGTERM/-SIGKILL to -pgid)
        └── windows.rs          # WindowsDriver v0 (flags, opportunistic CTRL_BREAK, TerminateProcess)
crates/openvhost-proc/tests/supervisor.rs   # integration: full lifecycle via proc_testchild
apps/cli/src/main.rs            # + `__testchild` dispatch (calls openvhost_proc::testchild)
apps/desktop/src-tauri/src/commands.rs      # + list/start/stop/log_tail commands, IpcError::Proc
apps/desktop/src-tauri/src/lib.rs           # Supervisor in managed state; event bridge; collect_events
apps/desktop/src/lib/services.svelte.ts     # runes store (DI'd api) — state map + capped log feed
apps/desktop/src/lib/ipc/index.ts           # + typed wrappers & event re-exports
apps/desktop/src/lib/ipc/ipc.test.ts        # + store/wrapper tests
apps/desktop/src/routes/+page.svelte        # Services panel v0
.github/workflows/ci.yml        # guard step also checks openvhost-proc
```

---

### Task 1: Platform layer — SpawnSpec, opaque SpawnedChild, ProcessDriver, both drivers

**Files:**
- Rewrite: `crates/openvhost-proc/Cargo.toml`, `crates/openvhost-proc/src/lib.rs`
- Create: `crates/openvhost-proc/src/platform/mod.rs`, `.../platform/unix.rs`, `.../platform/windows.rs`, `crates/openvhost-proc/src/error.rs`
- Modify: root `Cargo.toml` (workspace deps)

**Interfaces:**
- Consumes: nothing prior.
- Produces (Tasks 2–5 rely on these exact names): `SpawnSpec { program: PathBuf, args: Vec<OsString>, cwd: Option<PathBuf>, env: Vec<(OsString, OsString)> }` · `SpawnedChild::{id() -> Option<u32>, take_stdout()/take_stderr() -> Option<OutputStream>, wait() -> io::Result<ExitStatus> (async), try_wait()}` · `trait ProcessDriver { spawn(&self,&SpawnSpec)->io::Result<SpawnedChild>; request_graceful_stop(&self,&SpawnedChild)->io::Result<()>; kill(&self,&mut SpawnedChild)->io::Result<()> }` · `default_driver() -> Arc<dyn ProcessDriver>` · `ProcError { NotFound(String), Io(std::io::Error) }` · `pub(crate) fn assemble_env(extra: &[(OsString,OsString)]) -> Vec<(OsString,OsString)>`.

- [ ] **Step 1: Branch + workspace deps**

```bash
git switch main && git pull --quiet && git switch -c feat/p03-supervisor
```

Root `Cargo.toml` — extend `[workspace.dependencies]`:

```toml
tokio = { version = "1.53", features = ["process", "rt-multi-thread", "macros", "sync", "time", "io-util"] }
```

`crates/openvhost-proc/Cargo.toml` — replace entirely:

```toml
[package]
name = "openvhost-proc"
version.workspace = true
edition.workspace = true
license.workspace = true
publish.workspace = true

[dependencies]
serde.workspace = true
thiserror.workspace = true
tokio.workspace = true
specta = { version = "2", optional = true, features = ["derive"] }

[target.'cfg(unix)'.dependencies]
libc = "0.2"

[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = ["Win32_Foundation", "Win32_System_Console", "Win32_System_Threading"] }

[features]
specta = ["dep:specta"]

[dev-dependencies]

[lints]
workspace = true
```

(Match the `specta` version to what `openvhost-core` already pins if `cargo add --dry-run specta@2` disagrees.)

- [ ] **Step 2: Write the failing env allow-list test + module skeleton**

`crates/openvhost-proc/src/platform/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Platform seam for process operations (spec §5). Core code never branches
//! on OS inline; the two driver impls live in `unix.rs` / `windows.rs`.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// What to spawn. `program` MUST be a fully-resolved absolute path — the
/// drivers never consult $PATH (deterministic versioned installs).
/// Managed services must run in the FOREGROUND (no self-daemonize/setsid):
/// a daemonizing child escapes the containment group and stop would lie.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Applied ON TOP of the allow-list base env (see [`assemble_env`]).
    pub env: Vec<(OsString, OsString)>,
}

/// Base environment allow-list + `extra` on top. The child NEVER inherits
/// the supervisor's full ambient environment (reproducible-env principle).
pub(crate) fn assemble_env(extra: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = Vec::new();
    let mut push_from_parent = |key: &str| {
        if let Some(v) = std::env::var_os(key) {
            out.push((OsString::from(key), v));
        }
    };
    for key in ["PATH", "HOME", "TMPDIR", "LANG"] {
        push_from_parent(key);
    }
    #[cfg(windows)]
    {
        for key in ["SystemRoot", "windir", "TEMP", "TMP"] {
            push_from_parent(key);
        }
        // CRT startup needs System32 resolvable even with a cleared env.
        if let Some(root) = std::env::var_os("SystemRoot") {
            let mut p = root.clone();
            p.push("\\System32");
            let path_entry = match out.iter_mut().find(|(k, _)| k == "PATH") {
                Some((_, existing)) => {
                    existing.push(";");
                    existing.push(&p);
                    None
                }
                None => Some((OsString::from("PATH"), p)),
            };
            if let Some(e) = path_entry {
                out.push(e);
            }
        }
    }
    for (k, v) in extra {
        match out.iter_mut().find(|(key, _)| key == k) {
            Some((_, existing)) => *existing = v.clone(),
            None => out.push((k.clone(), v.clone())),
        }
    }
    out
}

/// Opaque per-OS identity captured ONCE at spawn (never re-derived from the
/// child, whose id() becomes None after reaping).
pub struct PlatformHandle {
    #[cfg(unix)]
    pub(crate) pgid: i32,
    #[cfg(windows)]
    pub(crate) pid: u32,
}

/// Opaque spawned child. All fields private ON PURPOSE (Windows P0-5 may
/// swap the internals for a raw CreateProcessW route without breaking this
/// API — dual-specialist consultation, spec §5).
pub struct SpawnedChild {
    pub(crate) child: tokio::process::Child,
    pub(crate) handle: PlatformHandle,
}

/// Opaque async-readable pipe (keeps tokio types out of the public API).
pub struct OutputStream(pub(crate) OutputInner);
pub(crate) enum OutputInner {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

impl tokio::io::AsyncRead for OutputStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            OutputInner::Out(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            OutputInner::Err(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl SpawnedChild {
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub fn take_stdout(&mut self) -> Option<OutputStream> {
        self.child.stdout.take().map(|s| OutputStream(OutputInner::Out(s)))
    }
    pub fn take_stderr(&mut self) -> Option<OutputStream> {
        self.child.stderr.take().map(|s| OutputStream(OutputInner::Err(s)))
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    #[cfg(unix)]
    pub(crate) fn pgid(&self) -> i32 {
        self.handle.pgid
    }
    #[cfg(windows)]
    pub(crate) fn pid_snapshot(&self) -> u32 {
        self.handle.pid
    }
}

/// Process operations, one impl per OS. This trait is the LCD fallback:
/// real services get protocol shutdown (mysql admin cmd, `nginx -s quit`)
/// at their per-service adapter layer in later slices. A signal-delivery
/// error meaning "no such process/group" is NOT a failure — callers should
/// `try_wait()` (the target may have exited on its own). Reload (e.g.
/// SIGUSR2) is deliberately NOT a trait method — it lands as a
/// platform/macos capability in P0-4 against the snapshotted pid (spec §5).
pub trait ProcessDriver: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild>;
    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()>;
    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()>;
}

pub fn default_driver() -> Arc<dyn ProcessDriver> {
    #[cfg(unix)]
    {
        Arc::new(unix::UnixDriver)
    }
    #[cfg(windows)]
    {
        Arc::new(windows::WindowsDriver)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn env_is_allowlist_not_inherit() {
        // SAFETY of assumption: PATH exists in any test environment.
        std::env::set_var("OPENVHOST_TEST_SHOULD_NOT_LEAK", "1");
        let env = assemble_env(&[]);
        assert!(env.iter().any(|(k, _)| k == "PATH"));
        assert!(!env.iter().any(|(k, _)| k == "OPENVHOST_TEST_SHOULD_NOT_LEAK"));
        std::env::remove_var("OPENVHOST_TEST_SHOULD_NOT_LEAK");
    }

    #[test]
    fn extra_env_overrides_base() {
        let extra = vec![(OsString::from("PATH"), OsString::from("/only/this"))];
        let env = assemble_env(&extra);
        let path = env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap();
        assert_eq!(path, OsString::from("/only/this"));
        assert_eq!(env.iter().filter(|(k, _)| k == "PATH").count(), 1);
    }
}
```

`crates/openvhost-proc/src/error.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Crate error type (thiserror in lib crates — master plan §5).

/// Errors produced by openvhost-proc.
#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    /// No service registered under this id.
    #[error("unknown service '{0}'")]
    NotFound(String),
    /// Underlying process/system operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
```

`crates/openvhost-proc/src/lib.rs` — replace entirely:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-proc — process supervisor for OpenVHost.
//!
//! Responsibility (master plan §3.1): spawn/stop/status for every managed
//! service with the state machine Stopped → Starting → Running → Failed,
//! log capture, and a broadcast event stream. MUST stay tauri-free.
//! v0 scope per spec 2026-07-21-p03-supervisor-design.md.

mod error;
pub mod platform;

pub use error::ProcError;
pub use platform::{default_driver, OutputStream, ProcessDriver, SpawnSpec, SpawnedChild};
```

- [ ] **Step 3: Run tests — expect the platform tests to fail to compile (drivers missing)**

Run: `cargo test -p openvhost-proc`
Expected: compile error — `unix`/`windows` modules referenced by `default_driver` don't exist yet. That is the red step.

- [ ] **Step 4: Implement both drivers**

`crates/openvhost-proc/src/platform/unix.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Unix driver: containment = own process group set atomically at spawn
//! (`process_group(0)` → posix_spawn attribute; closes the ESRCH race a
//! post-fork setpgid would leave). Signals target the SNAPSHOTTED -pgid.

use std::io;
use std::process::Stdio;

use super::{assemble_env, PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild};

pub(crate) struct UnixDriver;

fn signal_group(pgid: i32, sig: libc::c_int) -> io::Result<()> {
    // SAFETY: plain syscall; no memory handed over.
    let rc = unsafe { libc::kill(-pgid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

impl ProcessDriver for UnixDriver {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild> {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .env_clear()
            .envs(assemble_env(&spec.env))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let child = cmd.spawn()?;
        let pid = child.id().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "spawned child has no pid")
        })?;
        Ok(SpawnedChild {
            child,
            // process_group(0) makes the child the leader: pgid == pid.
            handle: PlatformHandle { pgid: pid as i32 },
        })
    }

    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()> {
        signal_group(child.pgid(), libc::SIGTERM)
    }

    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()> {
        // NEVER tokio's Child::kill() — that signals the direct child only
        // and would orphan grandchildren (spec §5).
        signal_group(child.pgid(), libc::SIGKILL)
    }
}
```

`crates/openvhost-proc/src/platform/windows.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows driver v0. Containment flags set at spawn; graceful stop is an
//! OPPORTUNISTIC CTRL_BREAK (works when a console exists — dev shells).
//! The packaged GUI app (windows_subsystem = "windows") has no console, so
//! v0/v1 graceful stop there is effectively hard-kill-only — documented
//! honestly (spec §5). From P0-5, kill() means TerminateJobObject on the
//! app-wide Job Object (ONE job per app); never simplify back to
//! per-process termination. FFI via windows-sys (already in-tree via tokio).

use std::io;
use std::os::windows::process::CommandExt;
use std::process::Stdio;

use windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent;

use super::{assemble_env, PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CTRL_BREAK_EVENT: u32 = 1;

pub(crate) struct WindowsDriver;

impl ProcessDriver for WindowsDriver {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild> {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .env_clear()
            .envs(assemble_env(&spec.env))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let child = cmd.spawn()?;
        let pid = child.id().ok_or_else(|| {
            io::Error::new(io::ErrorKind::Other, "spawned child has no pid")
        })?;
        Ok(SpawnedChild { child, handle: PlatformHandle { pid } })
    }

    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()> {
        // Opportunistic: reaches the child only when it shares a console
        // with us (dev). Failure here is expected in the GUI app; the
        // supervisor's 5s-deadline → kill() path is the real reclaimer.
        // SAFETY: plain Win32 call, no pointers.
        let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.pid_snapshot()) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()> {
        // v0: direct TerminateProcess via tokio (single process, no tree).
        // P0-5 replaces this with TerminateJobObject on the app-wide job.
        child.child.start_kill()
    }
}
```

- [ ] **Step 5: Add a unix spawn smoke test, run all green, commit**

Append to `platform/mod.rs` tests:

```rust
    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_true_exits_zero() {
        let driver = default_driver();
        let spec = SpawnSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from("exit 0")],
            cwd: None,
            env: vec![],
        };
        let mut child = driver.spawn(&spec).unwrap();
        let status = child.wait().await.unwrap();
        assert!(status.success());
    }
```

Run: `cargo test -p openvhost-proc` → Expected: PASS (3 tests on unix). Then full gates:

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh
git add -A && git commit -s -m "feat(proc): platform layer — ProcessDriver trait with unix and windows v0 drivers"
```

---

### Task 2: testchild — shared module, proc bin, CLI subcommand

**Files:**
- Create: `crates/openvhost-proc/src/testchild.rs`, `crates/openvhost-proc/src/bin/proc_testchild.rs`
- Modify: `crates/openvhost-proc/src/lib.rs` (export), `apps/cli/Cargo.toml`, `apps/cli/src/main.rs`

**Interfaces:**
- Consumes: nothing from Task 1 (std-only child).
- Produces: `openvhost_proc::testchild::{TestchildArgs, parse(&[String]) -> Result<TestchildArgs, String>, run(TestchildArgs) -> i32}`; binaries `proc_testchild` (this crate, for `env!("CARGO_BIN_EXE_proc_testchild")` in Task 4 tests) and `openvhost __testchild <flags>` (CLI). Flags: `--lines N` (default 10) `--interval-ms N` (default 200) `--exit N` (default 0) `--ignore-stop` `--fail-after N`.

- [ ] **Step 1: Write the failing parse tests**

`crates/openvhost-proc/src/testchild.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Deterministic cross-platform test child (spec §7). Std-only, sync.
//! `--ignore-stop` really ignores the platform stop request so the
//! supervisor's kill path gets exercised (Windows: a Ctrl handler that
//! returns TRUE — without it the OS default handler would terminate us
//! and the test would validate the wrong thing).

use std::io::Write;

#[derive(Debug, PartialEq, Eq)]
pub struct TestchildArgs {
    pub lines: u64,
    pub interval_ms: u64,
    pub exit_code: i32,
    pub ignore_stop: bool,
    pub fail_after: Option<u64>,
}

impl Default for TestchildArgs {
    fn default() -> Self {
        Self { lines: 10, interval_ms: 200, exit_code: 0, ignore_stop: false, fail_after: None }
    }
}

pub fn parse(args: &[String]) -> Result<TestchildArgs, String> {
    let mut out = TestchildArgs::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut next_u64 = |name: &str| -> Result<u64, String> {
            it.next()
                .ok_or_else(|| format!("{name} needs a value"))?
                .parse::<u64>()
                .map_err(|_| format!("{name} needs a number"))
        };
        match a.as_str() {
            "--lines" => out.lines = next_u64("--lines")?,
            "--interval-ms" => out.interval_ms = next_u64("--interval-ms")?,
            "--exit" => {
                out.exit_code = it
                    .next()
                    .ok_or("--exit needs a value")?
                    .parse::<i32>()
                    .map_err(|_| "--exit needs a number".to_string())?;
            }
            "--fail-after" => out.fail_after = Some(next_u64("--fail-after")?),
            "--ignore-stop" => out.ignore_stop = true,
            other => return Err(format!("unknown flag {other}")),
        }
    }
    Ok(out)
}

pub fn run(args: TestchildArgs) -> i32 {
    if args.ignore_stop {
        install_ignore_stop();
    }
    let stdout = std::io::stdout();
    for i in 1..=args.lines {
        if let Some(n) = args.fail_after {
            if i > n {
                eprintln!("ERROR simulated failure after {n} ticks");
                return 1;
            }
        }
        {
            let mut lock = stdout.lock();
            let _ = writeln!(lock, "tick {i}/{}", args.lines);
            let _ = lock.flush();
        }
        std::thread::sleep(std::time::Duration::from_millis(args.interval_ms));
    }
    args.exit_code
}

#[cfg(unix)]
fn install_ignore_stop() {
    // SAFETY: setting a signal disposition to SIG_IGN takes no user memory.
    unsafe {
        libc::signal(libc::SIGTERM, libc::SIG_IGN);
        libc::signal(libc::SIGINT, libc::SIG_IGN);
    }
}

#[cfg(windows)]
fn install_ignore_stop() {
    use windows_sys::Win32::Foundation::BOOL;
    use windows_sys::Win32::System::Console::SetConsoleCtrlHandler;
    unsafe extern "system" fn handler(_ctrl_type: u32) -> BOOL {
        1 // handled: ignore CTRL_C / CTRL_BREAK
    }
    // SAFETY: registering a static handler fn.
    unsafe {
        SetConsoleCtrlHandler(Some(handler), 1);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn parse_defaults() {
        assert_eq!(parse(&[]).unwrap(), TestchildArgs::default());
    }

    #[test]
    fn parse_all_flags() {
        let a = parse(&s(&["--lines", "3", "--interval-ms", "50", "--exit", "2", "--ignore-stop", "--fail-after", "1"])).unwrap();
        assert_eq!(a, TestchildArgs { lines: 3, interval_ms: 50, exit_code: 2, ignore_stop: true, fail_after: Some(1) });
    }

    #[test]
    fn parse_rejects_unknown() {
        assert!(parse(&s(&["--nope"])).is_err());
    }
}
```

`crates/openvhost-proc/src/lib.rs` — add:

```rust
pub mod testchild;
```

- [ ] **Step 2: Run — parse tests pass; then add the bin wrappers**

Run: `cargo test -p openvhost-proc testchild` → Expected: 3 passed.

`crates/openvhost-proc/src/bin/proc_testchild.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Test-only child binary for this crate's integration tests.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match openvhost_proc::testchild::parse(&args) {
        Ok(a) => std::process::exit(openvhost_proc::testchild::run(a)),
        Err(e) => {
            eprintln!("proc_testchild: {e}");
            std::process::exit(64);
        }
    }
}
```

`apps/cli/Cargo.toml` — add the dependency:

```toml
[dependencies]
openvhost-proc = { path = "../../crates/openvhost-proc" }
```

`apps/cli/src/main.rs` — replace entirely:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost — OpenVHost CLI (stub: prints version and exits 0).
//! Real verbs (start|stop|restart|status|list --json) land in Phase 1.
//! `__testchild` is an internal deterministic child for supervisor
//! development and demos — not a public interface.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("__testchild") {
        match openvhost_proc::testchild::parse(&args[1..]) {
            Ok(a) => std::process::exit(openvhost_proc::testchild::run(a)),
            Err(e) => {
                eprintln!("openvhost __testchild: {e}");
                std::process::exit(64);
            }
        }
    }
    println!("openvhost {}", env!("CARGO_PKG_VERSION"));
}
```

- [ ] **Step 3: Behavior test via the crate's own bin, gates, commit**

Append to `testchild.rs` tests:

```rust
    #[test]
    fn bin_emits_lines_and_exit_code() {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_proc_testchild"))
            .args(["--lines", "2", "--interval-ms", "1", "--exit", "3"])
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(3));
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(stdout.contains("tick 1/2") && stdout.contains("tick 2/2"));
    }
```

Run: `cargo test -p openvhost-proc && cargo run -p openvhost -- __testchild --lines 1 --interval-ms 1`
Expected: tests pass; the CLI prints `tick 1/1` and exits 0. Full gates, then:

```bash
git add -A && git commit -s -m "feat(proc): deterministic testchild shared by proc tests and the openvhost CLI"
```

---

### Task 3: Events, log ring buffer, exit classification (pure core, TDD)

**Files:**
- Create: `crates/openvhost-proc/src/events.rs`, `crates/openvhost-proc/src/log.rs`, `crates/openvhost-proc/src/state.rs`
- Modify: `crates/openvhost-proc/src/lib.rs` (exports)

**Interfaces:**
- Consumes: nothing from earlier tasks (pure).
- Produces (Tasks 4–6 rely on these exact shapes; all serde `rename_all = "camelCase"`, all `#[cfg_attr(feature = "specta", derive(specta::Type))]`):
  - `ServiceState` = tagged enum `{ kind: "stopped" | "starting" | "running" } | { kind: "failed", exit: Option<i32>, stderrTail: Vec<String> }` (serde `tag = "kind"`).
  - `LogLevel` = `"info" | "warn" | "error"` (serde lowercase) · `LogLine { ts_ms: u64, level: LogLevel, line: String }`.
  - `ServiceStatus { id: String, display_name: String, endpoint: Option<String>, pid: Option<u32>, state: ServiceState }`.
  - `SupervisorEvent::StateChanged { id, state, detail: Option<String> }` / `SupervisorEvent::Log { id, ts_ms, level, line }`.
  - `RingBuffer::new(cap)` / `push(LogLine)` / `tail(n) -> Vec<LogLine>` / `len()`.
  - `classify_level(source: StreamSource, line: &str) -> LogLevel` (`StreamSource::{Stdout, Stderr}`) — contains "ERROR"→Error else "WARN"→Warn else Info.
  - `classify_exit(stop_requested: bool, status: Option<ExitStatus>, stderr_tail: Vec<String>) -> ServiceState` — stop_requested → Stopped regardless of status; else success → Stopped; else → Failed with code + tail.

- [ ] **Step 1: Write failing tests (state.rs first — the heart)**

`crates/openvhost-proc/src/state.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure exit classification (spec §4). The stop-requested flag is recorded
//! BEFORE inspecting the exit status so a timeout-kill during a requested
//! stop lands as Stopped, never Failed.

use std::process::ExitStatus;

use crate::events::ServiceState;

pub(crate) fn classify_exit(
    stop_requested: bool,
    status: Option<&ExitStatus>,
    stderr_tail: Vec<String>,
) -> ServiceState {
    if stop_requested {
        return ServiceState::Stopped;
    }
    match status {
        Some(s) if s.success() => ServiceState::Stopped,
        Some(s) => ServiceState::Failed { exit: s.code(), stderr_tail },
        None => ServiceState::Failed { exit: None, stderr_tail },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::process::Command;

    fn exit_status(code: i32) -> ExitStatus {
        #[cfg(unix)]
        let out = Command::new("/bin/sh").args(["-c", &format!("exit {code}")]).status().unwrap();
        #[cfg(windows)]
        let out = Command::new("cmd").args(["/C", &format!("exit {code}")]).status().unwrap();
        out
    }

    #[test]
    fn requested_stop_wins_even_after_kill() {
        let st = exit_status(137); // looks like a crash
        let s = classify_exit(true, Some(&st), vec![]);
        assert!(matches!(s, ServiceState::Stopped));
    }

    #[test]
    fn clean_exit_is_stopped() {
        let st = exit_status(0);
        assert!(matches!(classify_exit(false, Some(&st), vec![]), ServiceState::Stopped));
    }

    #[test]
    fn nonzero_is_failed_with_tail() {
        let st = exit_status(2);
        let s = classify_exit(false, Some(&st), vec!["ERROR boom".into()]);
        match s {
            ServiceState::Failed { exit, stderr_tail } => {
                assert_eq!(exit, Some(2));
                assert_eq!(stderr_tail, vec!["ERROR boom".to_string()]);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
```

`crates/openvhost-proc/src/events.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Event and status DTOs — the shapes the UI contract demands
//! (docs/design/README.md). serde camelCase; optional specta derive.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    #[serde(rename_all = "camelCase")]
    Failed { exit: Option<i32>, stderr_tail: Vec<String> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts_ms: u64,
    pub level: LogLevel,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub id: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    pub pid: Option<u32>,
    pub state: ServiceState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorEvent {
    StateChanged { id: String, state: ServiceState, detail: Option<String> },
    Log { id: String, ts_ms: u64, level: LogLevel, line: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSource {
    Stdout,
    Stderr,
}
```

`crates/openvhost-proc/src/log.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Bounded log storage (2,000 lines/service, drop-oldest) + level heuristic.

use std::collections::VecDeque;

use crate::events::{LogLevel, LogLine, StreamSource};

pub(crate) const RING_CAPACITY: usize = 2000;
pub(crate) const STDERR_TAIL: usize = 10;

pub(crate) struct RingBuffer {
    cap: usize,
    items: VecDeque<LogLine>,
}

impl RingBuffer {
    pub(crate) fn new(cap: usize) -> Self {
        Self { cap, items: VecDeque::with_capacity(cap.min(256)) }
    }
    pub(crate) fn push(&mut self, line: LogLine) {
        if self.items.len() == self.cap {
            self.items.pop_front();
        }
        self.items.push_back(line);
    }
    pub(crate) fn tail(&self, n: usize) -> Vec<LogLine> {
        self.items.iter().rev().take(n).rev().cloned().collect()
    }
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }
}

/// v0 heuristic (spec §3): "ERROR" anywhere → Error; else "WARN" → Warn;
/// else Info. Same rule for both streams.
pub(crate) fn classify_level(_source: StreamSource, line: &str) -> LogLevel {
    if line.contains("ERROR") {
        LogLevel::Error
    } else if line.contains("WARN") {
        LogLevel::Warn
    } else {
        LogLevel::Info
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn line(s: &str) -> LogLine {
        LogLine { ts_ms: 0, level: LogLevel::Info, line: s.to_string() }
    }

    #[test]
    fn ring_drops_oldest_at_capacity() {
        let mut rb = RingBuffer::new(3);
        for i in 0..5 {
            rb.push(line(&format!("l{i}")));
        }
        assert_eq!(rb.len(), 3);
        let tail: Vec<String> = rb.tail(3).into_iter().map(|l| l.line).collect();
        assert_eq!(tail, vec!["l2", "l3", "l4"]);
    }

    #[test]
    fn tail_smaller_than_len() {
        let mut rb = RingBuffer::new(10);
        for i in 0..4 {
            rb.push(line(&format!("l{i}")));
        }
        let tail: Vec<String> = rb.tail(2).into_iter().map(|l| l.line).collect();
        assert_eq!(tail, vec!["l2", "l3"]);
    }

    #[test]
    fn level_heuristic() {
        assert_eq!(classify_level(StreamSource::Stderr, "ERROR boom"), LogLevel::Error);
        assert_eq!(classify_level(StreamSource::Stdout, "some WARN here"), LogLevel::Warn);
        assert_eq!(classify_level(StreamSource::Stderr, "hello"), LogLevel::Info);
    }
}
```

`lib.rs` — add modules/exports:

```rust
pub mod events;
mod log;
mod state;

pub use events::{LogLevel, LogLine, ServiceState, ServiceStatus, StreamSource, SupervisorEvent};
```

- [ ] **Step 2: Run red→green, gates, commit**

Run: `cargo test -p openvhost-proc` → Expected: all pass (parse 4 + platform 3 + state 3 + log 3 on unix). Full gates, then:

```bash
git add -A && git commit -s -m "feat(proc): event DTOs, bounded ring buffer, and pure exit classification"
```

---

### Task 4: Supervisor + ServiceTask + integration tests + CI guard extension

**Files:**
- Create: `crates/openvhost-proc/src/supervisor.rs`, `crates/openvhost-proc/src/service_task.rs`, `crates/openvhost-proc/tests/supervisor.rs`
- Modify: `crates/openvhost-proc/src/lib.rs`, `.github/workflows/ci.yml` (guard step)

**Interfaces:**
- Consumes: Task 1 (`ProcessDriver`, `SpawnSpec`, `SpawnedChild`, `default_driver`), Task 2 (`proc_testchild` bin), Task 3 (DTOs, `RingBuffer`, `classify_level`, `classify_exit`).
- Produces (Task 5 relies on): `ServiceSpec { id: String, display_name: String, endpoint: Option<String>, spawn: SpawnSpec }` · `Supervisor::new(driver: Arc<dyn ProcessDriver>) -> Supervisor` · `.register(ServiceSpec)` · `.start(&self, id: &str) -> Result<(), ProcError>` · `.stop(&self, id: &str) -> Result<(), ProcError>` · `.snapshot(&self) -> Vec<ServiceStatus>` · `.log_tail(&self, id: &str, n: usize) -> Result<Vec<LogLine>, ProcError>` · `.subscribe(&self) -> broadcast::Receiver<SupervisorEvent>`. `Supervisor` is `Clone` (Arc inside) and must be constructed inside a tokio runtime.

- [ ] **Step 1: Write the failing integration tests**

`crates/openvhost-proc/tests/supervisor.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Full-lifecycle integration tests against the real proc_testchild binary.
//! Poll-with-timeout only — never sleep-and-hope.
#![allow(clippy::unwrap_used)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use openvhost_proc::{
    default_driver, ServiceSpec, ServiceState, SpawnSpec, Supervisor, SupervisorEvent,
};
use tokio::sync::broadcast;

fn testchild_spec(args: &[&str]) -> SpawnSpec {
    SpawnSpec {
        program: PathBuf::from(env!("CARGO_BIN_EXE_proc_testchild")),
        args: args.iter().map(OsString::from).collect(),
        cwd: None,
        env: vec![],
    }
}

fn svc(id: &str, args: &[&str]) -> ServiceSpec {
    ServiceSpec {
        id: id.to_string(),
        display_name: id.to_string(),
        endpoint: None,
        spawn: testchild_spec(args),
    }
}

/// Consume events until `pred` matches a StateChanged for `id`, or panic at timeout.
async fn wait_state(
    rx: &mut broadcast::Receiver<SupervisorEvent>,
    id: &str,
    timeout: Duration,
    pred: impl Fn(&ServiceState) -> bool,
) -> ServiceState {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(!remaining.is_zero(), "timed out waiting for state on '{id}'");
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(SupervisorEvent::StateChanged { id: eid, state, .. }))
                if eid == id && pred(&state) =>
            {
                return state;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("event channel closed: {e}"),
            Err(_) => panic!("timed out waiting for state on '{id}'"),
        }
    }
}

#[tokio::test]
async fn lifecycle_running_then_graceful_stop() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc("t1", &["--lines", "100", "--interval-ms", "100"]));
    let mut rx = sup.subscribe();
    sup.start("t1").unwrap();
    wait_state(&mut rx, "t1", Duration::from_secs(2), |s| matches!(s, ServiceState::Starting)).await;
    wait_state(&mut rx, "t1", Duration::from_secs(2), |s| matches!(s, ServiceState::Running)).await;
    let pid = sup.snapshot().into_iter().find(|s| s.id == "t1").unwrap().pid;
    assert!(pid.is_some(), "running service must report a pid");
    sup.stop("t1").unwrap();
    wait_state(&mut rx, "t1", Duration::from_secs(3), |s| matches!(s, ServiceState::Stopped)).await;
    // zero-orphan probe: the whole group must be gone (unix).
    #[cfg(unix)]
    {
        let pgid = pid.unwrap() as i32;
        // SAFETY: signal 0 = existence probe only.
        let rc = unsafe { libc::kill(-pgid, 0) };
        assert_eq!(rc, -1, "process group must not exist after stop");
    }
}

#[tokio::test]
async fn ignore_stop_takes_kill_path_and_ends_stopped() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc("t2", &["--lines", "500", "--interval-ms", "100", "--ignore-stop"]));
    let mut rx = sup.subscribe();
    sup.start("t2").unwrap();
    wait_state(&mut rx, "t2", Duration::from_secs(2), |s| matches!(s, ServiceState::Running)).await;
    let t0 = Instant::now();
    sup.stop("t2").unwrap();
    let final_state =
        wait_state(&mut rx, "t2", Duration::from_secs(8), |s| matches!(s, ServiceState::Stopped)).await;
    assert!(matches!(final_state, ServiceState::Stopped), "kill path must classify as Stopped");
    assert!(t0.elapsed() >= Duration::from_secs(5), "kill fires only after the 5s grace deadline");
}

#[tokio::test]
async fn nonzero_exit_is_failed_with_stderr_tail() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc("t3", &["--lines", "10", "--interval-ms", "10", "--fail-after", "2"]));
    let mut rx = sup.subscribe();
    sup.start("t3").unwrap();
    let state =
        wait_state(&mut rx, "t3", Duration::from_secs(3), |s| matches!(s, ServiceState::Failed { .. }))
            .await;
    match state {
        ServiceState::Failed { exit, stderr_tail } => {
            assert_eq!(exit, Some(1));
            assert!(stderr_tail.iter().any(|l| l.contains("ERROR")), "tail: {stderr_tail:?}");
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn instant_death_reports_failed_before_500ms_timer() {
    let sup = Supervisor::new(default_driver());
    // --lines 0 --exit 1: exits immediately with code 1.
    sup.register(svc("t4", &["--lines", "0", "--exit", "1", "--interval-ms", "1"]));
    let mut rx = sup.subscribe();
    let t0 = Instant::now();
    sup.start("t4").unwrap();
    wait_state(&mut rx, "t4", Duration::from_secs(2), |s| matches!(s, ServiceState::Failed { .. })).await;
    assert!(t0.elapsed() < Duration::from_millis(400), "raced bound must not wait out the timer");
}

#[tokio::test]
async fn spawn_failure_is_failed_with_pointing_detail() {
    let sup = Supervisor::new(default_driver());
    sup.register(ServiceSpec {
        id: "t5".into(),
        display_name: "t5".into(),
        endpoint: None,
        spawn: SpawnSpec {
            program: PathBuf::from("/definitely/not/here/openvhost-missing"),
            args: vec![],
            cwd: None,
            env: vec![],
        },
    });
    let mut rx = sup.subscribe();
    sup.start("t5").unwrap();
    wait_state(&mut rx, "t5", Duration::from_secs(2), |s| matches!(s, ServiceState::Failed { .. })).await;
    let tail = sup.log_tail("t5", 10).unwrap();
    assert!(
        tail.iter().any(|l| l.line.contains("openvhost-missing")),
        "failure log must name the missing program"
    );
}
```

Also add `libc` to proc dev-deps for the orphan probe — `crates/openvhost-proc/Cargo.toml`:

```toml
[dev-dependencies]
libc = "0.2"
```

- [ ] **Step 2: Run to verify red**

Run: `cargo test -p openvhost-proc --test supervisor`
Expected: compile FAIL — `Supervisor`, `ServiceSpec` not defined yet.

- [ ] **Step 3: Implement Supervisor + ServiceTask**

`crates/openvhost-proc/src/supervisor.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Registry + control surface + single broadcast event stream (spec §3).
//! Locks are short and never held across an await.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc};

use crate::error::ProcError;
use crate::events::{LogLevel, LogLine, ServiceState, ServiceStatus, StreamSource, SupervisorEvent};
use crate::log::{classify_level, RingBuffer, RING_CAPACITY, STDERR_TAIL};
use crate::platform::{ProcessDriver, SpawnSpec};

#[derive(Debug, Clone)]
pub struct ServiceSpec {
    pub id: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    pub spawn: SpawnSpec,
}

pub(crate) struct Entry {
    pub(crate) spec: ServiceSpec,
    pub(crate) state: ServiceState,
    pub(crate) pid: Option<u32>,
    pub(crate) logs: RingBuffer,
    pub(crate) stderr_tail: VecDeque<String>,
    pub(crate) stop_requested: Arc<AtomicBool>,
    pub(crate) control: Option<mpsc::Sender<()>>,
}

pub(crate) struct Inner {
    pub(crate) driver: Arc<dyn ProcessDriver>,
    pub(crate) entries: Mutex<HashMap<String, Entry>>,
    pub(crate) tx: broadcast::Sender<SupervisorEvent>,
}

#[derive(Clone)]
pub struct Supervisor {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

impl Supervisor {
    pub fn new(driver: Arc<dyn ProcessDriver>) -> Self {
        let (tx, _) = broadcast::channel(256);
        Self { inner: Arc::new(Inner { driver, entries: Mutex::new(HashMap::new()), tx }) }
    }

    pub fn register(&self, spec: ServiceSpec) {
        let entry = Entry {
            id_state_defaults(spec)
        };
        // (see below — expanded in real code)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<SupervisorEvent> {
        self.inner.tx.subscribe()
    }

    pub fn snapshot(&self) -> Vec<ServiceStatus> {
        let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        let mut out: Vec<ServiceStatus> = entries
            .values()
            .map(|e| ServiceStatus {
                id: e.spec.id.clone(),
                display_name: e.spec.display_name.clone(),
                endpoint: e.spec.endpoint.clone(),
                pid: e.pid,
                state: e.state.clone(),
            })
            .collect();
        out.sort_by(|a, b| a.id.cmp(&b.id));
        out
    }

    pub fn log_tail(&self, id: &str, n: usize) -> Result<Vec<LogLine>, ProcError> {
        let entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        let e = entries.get(id).ok_or_else(|| ProcError::NotFound(id.to_string()))?;
        Ok(e.logs.tail(n))
    }

    pub fn start(&self, id: &str) -> Result<(), ProcError> {
        let (spawn, stop_flag, control_rx) = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries.get_mut(id).ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            if matches!(e.state, ServiceState::Starting | ServiceState::Running) {
                return Ok(());
            }
            e.stop_requested.store(false, Ordering::SeqCst);
            let (ctl_tx, ctl_rx) = mpsc::channel(1);
            e.control = Some(ctl_tx);
            (e.spec.spawn.clone(), Arc::clone(&e.stop_requested), ctl_rx)
        };
        Inner::set_state(&self.inner, id, ServiceState::Starting, Some("requested by user".into()));
        Inner::push_supervisor_log(&self.inner, id, "state Stopped → Starting (requested by user)".to_string());
        let inner = Arc::clone(&self.inner);
        let id_owned = id.to_string();
        tokio::spawn(crate::service_task::run(inner, id_owned, spawn, stop_flag, control_rx));
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<(), ProcError> {
        let control = {
            let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            let e = entries.get_mut(id).ok_or_else(|| ProcError::NotFound(id.to_string()))?;
            if matches!(e.state, ServiceState::Stopped | ServiceState::Failed { .. }) {
                return Ok(());
            }
            // Flag FIRST — classification consults it (spec §4).
            e.stop_requested.store(true, Ordering::SeqCst);
            e.control.clone()
        };
        Inner::push_supervisor_log(&self.inner, id, "stop requested by user".to_string());
        if let Some(ctl) = control {
            let _ = ctl.try_send(());
        }
        Ok(())
    }
}

impl Inner {
    pub(crate) fn set_state(inner: &Arc<Inner>, id: &str, state: ServiceState, detail: Option<String>) {
        {
            let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(id) {
                e.state = state.clone();
                if matches!(state, ServiceState::Stopped | ServiceState::Failed { .. }) {
                    e.pid = None;
                    e.control = None;
                }
            }
        }
        let _ = inner.tx.send(SupervisorEvent::StateChanged { id: id.to_string(), state, detail });
    }

    pub(crate) fn set_pid(inner: &Arc<Inner>, id: &str, pid: Option<u32>) {
        let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(e) = entries.get_mut(id) {
            e.pid = pid;
        }
    }

    pub(crate) fn stderr_tail_snapshot(inner: &Arc<Inner>, id: &str) -> Vec<String> {
        let entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.get(id).map(|e| e.stderr_tail.iter().cloned().collect()).unwrap_or_default()
    }

    pub(crate) fn push_log(inner: &Arc<Inner>, id: &str, source: StreamSource, line: String) {
        let level = classify_level(source, &line);
        let ts_ms = now_ms();
        {
            let mut entries = inner.entries.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(e) = entries.get_mut(id) {
                e.logs.push(LogLine { ts_ms, level, line: line.clone() });
                if source == StreamSource::Stderr {
                    if e.stderr_tail.len() == STDERR_TAIL {
                        e.stderr_tail.pop_front();
                    }
                    e.stderr_tail.push_back(line.clone());
                }
            }
        }
        let _ = inner.tx.send(SupervisorEvent::Log { id: id.to_string(), ts_ms, level, line });
    }

    pub(crate) fn push_supervisor_log(inner: &Arc<Inner>, id: &str, line: String) {
        Self::push_log(inner, id, StreamSource::Stdout, format!("supervisor: {line}"));
    }
}
```

In `register` replace the sketch with the real body:

```rust
    pub fn register(&self, spec: ServiceSpec) {
        let id = spec.id.clone();
        let entry = Entry {
            spec,
            state: ServiceState::Stopped,
            pid: None,
            logs: RingBuffer::new(RING_CAPACITY),
            stderr_tail: VecDeque::new(),
            stop_requested: Arc::new(AtomicBool::new(false)),
            control: None,
        };
        let mut entries = self.inner.entries.lock().unwrap_or_else(|e| e.into_inner());
        entries.insert(id, entry);
    }
```

(The plan shows both forms so no reader is left with the `id_state_defaults` sketch — the real body above is authoritative. Note: `Mutex::lock().unwrap_or_else(|e| e.into_inner())` is the poisoned-lock recovery idiom, not an `unwrap()` — clippy-clean without exceptions.)

`crates/openvhost-proc/src/service_task.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! One task per running service: spawn → raced 500ms bound → run loop →
//! two-phase stop → classify (spec §4). Readers drain pipes immediately.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc;

use crate::events::{ServiceState, StreamSource};
use crate::platform::{OutputStream, SpawnSpec, SpawnedChild};
use crate::state::classify_exit;
use crate::supervisor::Inner;

const RUNNING_AFTER: Duration = Duration::from_millis(500);
const GRACE_DEADLINE: Duration = Duration::from_secs(5);

fn spawn_reader(inner: Arc<Inner>, id: String, source: StreamSource, stream: OutputStream) {
    tokio::spawn(async move {
        let mut lines = BufReader::new(stream).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            Inner::push_log(&inner, &id, source, line);
        }
    });
}

async fn finish(inner: &Arc<Inner>, id: &str, stop_flag: &AtomicBool, status: Option<std::process::ExitStatus>) {
    let tail = Inner::stderr_tail_snapshot(inner, id);
    let state = classify_exit(stop_flag.load(Ordering::SeqCst), status.as_ref(), tail);
    let detail = match (&state, status) {
        (ServiceState::Failed { .. }, Some(s)) => Some(format!("exited with {s}")),
        (ServiceState::Failed { .. }, None) => Some("exited before startup completed".to_string()),
        (_, Some(s)) => Some(format!("{s}")),
        _ => None,
    };
    let label = match &state {
        ServiceState::Stopped => "Stopped",
        ServiceState::Failed { .. } => "Failed",
        _ => "?",
    };
    Inner::push_supervisor_log(inner, id, format!("state → {label}"));
    Inner::set_state(inner, id, state, detail);
}

pub(crate) async fn run(
    inner: Arc<Inner>,
    id: String,
    spec: SpawnSpec,
    stop_flag: Arc<AtomicBool>,
    mut control_rx: mpsc::Receiver<()>,
) {
    let mut child: SpawnedChild = match inner.driver.spawn(&spec) {
        Ok(c) => c,
        Err(e) => {
            Inner::push_log(
                &inner,
                &id,
                StreamSource::Stderr,
                format!("ERROR spawn failed: {e} — {}", spec.program.display()),
            );
            finish(&inner, &id, &stop_flag, None).await;
            return;
        }
    };
    Inner::set_pid(&inner, &id, child.id());
    Inner::push_supervisor_log(&inner, &id, format!("spawned pid {:?}", child.id()));
    if let Some(out) = child.take_stdout() {
        spawn_reader(Arc::clone(&inner), id.clone(), StreamSource::Stdout, out);
    }
    if let Some(err) = child.take_stderr() {
        spawn_reader(Arc::clone(&inner), id.clone(), StreamSource::Stderr, err);
    }

    // Raced 500ms bound: death during the window reports instantly (spec §4).
    tokio::select! {
        _ = tokio::time::sleep(RUNNING_AFTER) => {
            Inner::push_supervisor_log(&inner, &id, "state Starting → Running".to_string());
            Inner::set_state(&inner, &id, ServiceState::Running, None);
        }
        status = child.wait() => {
            finish(&inner, &id, &stop_flag, status.ok()).await;
            return;
        }
    }

    loop {
        tokio::select! {
            status = child.wait() => {
                finish(&inner, &id, &stop_flag, status.ok()).await;
                return;
            }
            ctl = control_rx.recv() => {
                if ctl.is_none() {
                    continue; // sender dropped without a stop request
                }
                // Two-phase stop. ESRCH-style errors mean "already gone" —
                // fall through to wait() either way (spec §5).
                let _ = inner.driver.request_graceful_stop(&child);
                tokio::select! {
                    status = child.wait() => {
                        finish(&inner, &id, &stop_flag, status.ok()).await;
                        return;
                    }
                    _ = tokio::time::sleep(GRACE_DEADLINE) => {
                        Inner::push_supervisor_log(&inner, &id, "grace deadline passed — killing".to_string());
                        let _ = inner.driver.kill(&mut child);
                        let status = child.wait().await.ok();
                        finish(&inner, &id, &stop_flag, status).await;
                        return;
                    }
                }
            }
        }
    }
}
```

`lib.rs` — add:

```rust
mod service_task;
mod supervisor;

pub use supervisor::{ServiceSpec, Supervisor};
```

- [ ] **Step 4: Run integration tests to green**

Run: `cargo test -p openvhost-proc` → Expected: all unit + 5 integration tests pass (~12s wall: the kill-path test waits out the 5s grace). If `lifecycle_running_then_graceful_stop`'s orphan probe flakes, the probe races reaping — re-check that `stop` completed via the Stopped event before probing (the test already orders it correctly; do not add sleeps).

- [ ] **Step 5: Extend the CI no-tauri guard to proc**

`.github/workflows/ci.yml` — replace the guard step's `run` block:

```yaml
      - name: Guard - core and proc must not depend on tauri
        run: |
          for crate in openvhost-core openvhost-proc; do
            if cargo tree -p "$crate" -e normal --all-features | grep -qi tauri; then
              echo "::error::$crate depends on tauri"; exit 1
            fi
          done
```

(Keep the step in the same position; `actionlint .github/workflows/ci.yml` must stay silent.)

- [ ] **Step 6: Full gates, commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh
actionlint .github/workflows/ci.yml
git add -A && git commit -s -m "feat(proc): supervisor with per-service tasks, two-phase stop, raced startup bound"
```

---

### Task 5: Desktop bridge — commands, typed events, demo-ticker

**Files:**
- Modify: `apps/desktop/src-tauri/Cargo.toml`, `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src-tauri/src/lib.rs`
- Regenerated: `apps/desktop/src/lib/ipc/bindings.ts` (via the export test; CI drift gate covers it)

**Interfaces:**
- Consumes: Task 4 `Supervisor` API; existing `IpcError` pattern; existing specta builder + `export_bindings` test in `lib.rs`.
- Produces (Task 6 relies on): commands `list_services() -> ServiceStatus[]`, `start_service(id: string)`, `stop_service(id: string)`, `service_log_tail(id: string, n: number) -> LogLine[]`; events `ServiceStateEvent { id, state, detail }`, `ServiceLogEvent { id, tsMs, level, line }` exported in bindings as `events.serviceStateEvent` / `events.serviceLogEvent`; `IpcError` gains `{ kind: 'proc', message: string }`.

- [ ] **Step 1: Wire the crate dependency + specta feature**

`apps/desktop/src-tauri/Cargo.toml` — add:

```toml
openvhost-proc = { path = "../../../crates/openvhost-proc", features = ["specta"] }
```

- [ ] **Step 2: Commands + error variant + event structs**

`apps/desktop/src-tauri/src/commands.rs` — add below the existing code:

```rust
use std::sync::Arc;

use openvhost_proc::{LogLine, LogLevel, ProcError, ServiceState, ServiceStatus, Supervisor};

impl From<ProcError> for IpcError {
    fn from(e: ProcError) -> Self {
        IpcError::Proc { message: e.to_string() }
    }
}

#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServiceStateEvent {
    pub id: String,
    pub state: ServiceState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
#[serde(rename_all = "camelCase")]
pub struct ServiceLogEvent {
    pub id: String,
    pub ts_ms: u64,
    pub level: LogLevel,
    pub line: String,
}

#[tauri::command]
#[specta::specta]
pub fn list_services(sup: tauri::State<'_, Arc<Supervisor>>) -> Result<Vec<ServiceStatus>, IpcError> {
    Ok(sup.snapshot())
}

#[tauri::command]
#[specta::specta]
pub fn start_service(sup: tauri::State<'_, Arc<Supervisor>>, id: String) -> Result<(), IpcError> {
    sup.start(&id).map_err(IpcError::from)
}

#[tauri::command]
#[specta::specta]
pub fn stop_service(sup: tauri::State<'_, Arc<Supervisor>>, id: String) -> Result<(), IpcError> {
    sup.stop(&id).map_err(IpcError::from)
}

#[tauri::command]
#[specta::specta]
pub fn service_log_tail(
    sup: tauri::State<'_, Arc<Supervisor>>,
    id: String,
    n: u32,
) -> Result<Vec<LogLine>, IpcError> {
    sup.log_tail(&id, n as usize).map_err(IpcError::from)
}
```

And extend the existing `IpcError` enum with the new variant:

```rust
    /// An error bubbled up from the process supervisor.
    #[error("{message}")]
    Proc { message: String },
```

- [ ] **Step 3: Supervisor state + event bridge + demo-ticker in lib.rs**

Rewrite `apps/desktop/src-tauri/src/lib.rs`'s `run()` (keep `specta_builder()` and the export test; add events + setup):

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenVHost desktop — Tauri entry point with typed (tauri-specta) commands
//! and events. The supervisor lives here as managed state; openvhost-proc
//! stays tauri-free and this crate owns the bridge.

mod commands;

use std::ffi::OsString;
use std::sync::Arc;

use openvhost_proc::{default_driver, ServiceSpec, SpawnSpec, Supervisor, SupervisorEvent};
use tauri_specta::{collect_commands, collect_events, Builder, Event};

fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::core_info,
            commands::list_services,
            commands::start_service,
            commands::stop_service,
            commands::service_log_tail,
        ])
        .events(collect_events![commands::ServiceStateEvent, commands::ServiceLogEvent])
}

/// Dev convenience: the demo ticker runs the openvhost CLI sitting next to
/// this executable in target/. A missing binary is an HONEST Failed state
/// in the UI (the spawn-failure log names the path), not a crash.
fn demo_ticker_spec() -> ServiceSpec {
    let cli = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(if cfg!(windows) { "openvhost.exe" } else { "openvhost" })))
        .unwrap_or_else(|| std::path::PathBuf::from("openvhost"));
    ServiceSpec {
        id: "demo-ticker".into(),
        display_name: "demo ticker".into(),
        endpoint: Some("__testchild · 1s interval · fails after 45 ticks".into()),
        spawn: SpawnSpec {
            program: cli,
            args: ["__testchild", "--lines", "100000", "--interval-ms", "1000", "--fail-after", "45"]
                .iter()
                .map(OsString::from)
                .collect(),
            cwd: None,
            env: vec![],
        },
    }
}

pub fn run() {
    let specta_builder = specta_builder();

    #[cfg(debug_assertions)]
    if let Err(e) = specta_builder.export(
        specta_typescript::Typescript::default(),
        "../src/lib/ipc/bindings.ts",
    ) {
        eprintln!("fatal: failed to export TS bindings: {e}");
        std::process::exit(1);
    }

    let result = tauri::Builder::default()
        .invoke_handler(specta_builder.invoke_handler())
        .setup(move |app| {
            specta_builder.mount_events(app);
            let supervisor = Arc::new(Supervisor::new(default_driver()));
            supervisor.register(demo_ticker_spec());
            let mut rx = supervisor.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(SupervisorEvent::StateChanged { id, state, detail }) => {
                            let _ = commands::ServiceStateEvent { id, state, detail }.emit(&handle);
                        }
                        Ok(SupervisorEvent::Log { id, ts_ms, level, line }) => {
                            let _ = commands::ServiceLogEvent { id, ts_ms, level, line }.emit(&handle);
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            app.manage(supervisor);
            Ok(())
        })
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("fatal: tauri failed to run: {e}");
        std::process::exit(1);
    }
}
```

Keep the existing `#[cfg(test)] mod tests { fn export_bindings() ... }` — it must now construct the builder via the same `specta_builder()` helper so events are exported too. Add `tokio` where needed: `tauri::async_runtime` already wraps tokio (no new dependency).

- [ ] **Step 4: Regenerate bindings + verify drift gate + gates + commit**

```bash
cargo test -p openvhost-desktop export_bindings
git diff --stat apps/desktop/src/lib/ipc/bindings.ts   # must show changes (new commands + events)
cargo build --workspace && cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace && bash scripts/check-spdx.sh && cargo deny check licenses advisories
git add -A && git commit -s -m "feat(desktop): supervisor bridge — typed commands, events, demo-ticker service"
```

Expected: bindings.ts now contains `listServices`, `startService`, `stopService`, `serviceLogTail`, `events` export with `serviceStateEvent`/`serviceLogEvent`, and the `ServiceState`/`LogLine` types.

---

### Task 6: Services panel v0 — store, page, tests

**Files:**
- Create: `apps/desktop/src/lib/services.svelte.ts`
- Modify: `apps/desktop/src/lib/ipc/index.ts`, `apps/desktop/src/lib/ipc/ipc.test.ts`, `apps/desktop/src/routes/+page.svelte`

**Interfaces:**
- Consumes: Task 5 bindings (`commands.*`, `events.*`, types `ServiceStatus`, `ServiceState`, `LogLine`, `IpcError`).
- Produces: `ipc/index.ts` exports `listServices(): Promise<ServiceStatus[]>`, `startService(id)`, `stopService(id)`, `serviceLogTail(id, n)`, `onServiceState(cb): Promise<() => void>`, `onServiceLog(cb): Promise<() => void>` and re-exports the types; `ServicesStore` class (DI'd api) with `services`, `logs`, `init()`, `applyState(ev)`, `applyLog(ev)`.

- [ ] **Step 1: Extend the IPC module (only place touching bindings)**

`apps/desktop/src/lib/ipc/index.ts` — append:

```ts
import { events } from './bindings';
import type { LogLine, ServiceStatus } from './bindings';
import type { ServiceLogEvent, ServiceStateEvent } from './bindings';

export type { LogLine, ServiceLogEvent, ServiceStateEvent, ServiceStatus };

function unwrap<T>(r: { status: 'ok'; data: T } | { status: 'error'; error: unknown }): T {
	if (r.status === 'error') throw r.error;
	return r.data;
}

export async function listServices(): Promise<ServiceStatus[]> {
	return unwrap(await commands.listServices());
}
export async function startService(id: string): Promise<void> {
	unwrap(await commands.startService(id));
}
export async function stopService(id: string): Promise<void> {
	unwrap(await commands.stopService(id));
}
export async function serviceLogTail(id: string, n: number): Promise<LogLine[]> {
	return unwrap(await commands.serviceLogTail(id, n));
}
export function onServiceState(cb: (ev: ServiceStateEvent) => void): Promise<() => void> {
	return events.serviceStateEvent.listen((e) => cb(e.payload));
}
export function onServiceLog(cb: (ev: ServiceLogEvent) => void): Promise<() => void> {
	return events.serviceLogEvent.listen((e) => cb(e.payload));
}
```

(If the generated result-object shape differs — e.g. plain returns for non-Result commands — match `unwrap` to what `bindings.ts` actually generates; the vitest below pins the behavior.)

- [ ] **Step 2: The store (DI so vitest needs no tauri)**

`apps/desktop/src/lib/services.svelte.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// Services panel state: snapshot seeds it, events drive it (UI contract).
import type { LogLine, ServiceLogEvent, ServiceStateEvent, ServiceStatus } from './ipc';

export interface ServicesApi {
	listServices(): Promise<ServiceStatus[]>;
	serviceLogTail(id: string, n: number): Promise<LogLine[]>;
}

export interface UiLog extends LogLine {
	id: string;
}

const LOG_CAP = 50;

export class ServicesStore {
	services = $state<ServiceStatus[]>([]);
	logs = $state<UiLog[]>([]);

	constructor(private api: ServicesApi) {}

	async init(): Promise<void> {
		this.services = await this.api.listServices();
		const first = this.services[0];
		if (first) {
			const tail = await this.api.serviceLogTail(first.id, LOG_CAP);
			this.logs = tail.map((l) => ({ ...l, id: first.id }));
		}
	}

	applyState(ev: ServiceStateEvent): void {
		this.services = this.services.map((s) =>
			s.id === ev.id ? { ...s, state: ev.state, pid: ev.state.kind === 'running' ? s.pid : null } : s
		);
	}

	applyLog(ev: ServiceLogEvent): void {
		const next = [...this.logs, { id: ev.id, tsMs: ev.tsMs, level: ev.level, line: ev.line }];
		this.logs = next.length > LOG_CAP ? next.slice(next.length - LOG_CAP) : next;
	}
}
```

- [ ] **Step 3: Write the failing store tests, then run**

Append to `apps/desktop/src/lib/ipc/ipc.test.ts`:

```ts
import { ServicesStore } from '../services.svelte';
import type { ServiceStatus } from './index';

const svc = (id: string, kind: 'stopped' | 'running'): ServiceStatus =>
	({ id, displayName: id, endpoint: null, pid: null, state: { kind } }) as ServiceStatus;

describe('ServicesStore', () => {
	const api = {
		listServices: async () => [svc('demo-ticker', 'stopped')],
		serviceLogTail: async () => [{ tsMs: 1, level: 'info', line: 'seed' }] as never[]
	};

	it('init seeds services and log tail', async () => {
		const store = new ServicesStore(api as never);
		await store.init();
		expect(store.services).toHaveLength(1);
		expect(store.logs[0]?.line).toBe('seed');
	});

	it('applyState replaces the matching service state', async () => {
		const store = new ServicesStore(api as never);
		await store.init();
		store.applyState({ id: 'demo-ticker', state: { kind: 'running' }, detail: null } as never);
		expect(store.services[0]?.state.kind).toBe('running');
	});

	it('applyLog caps the feed at 50', async () => {
		const store = new ServicesStore(api as never);
		for (let i = 0; i < 60; i++) {
			store.applyLog({ id: 'x', tsMs: i, level: 'info', line: `l${i}` } as never);
		}
		expect(store.logs).toHaveLength(50);
		expect(store.logs[0]?.line).toBe('l10');
	});
});
```

Run: `pnpm -C apps/desktop test` → Expected: new tests PASS alongside the existing coreInfo tests (svelte files with runes compile under the sv vitest setup; if `$state` in `.svelte.ts` needs the svelte plugin in the vitest project config, sv's default template already wires it — verify `vite.config.ts` includes the svelte plugin for tests before debugging further).

- [ ] **Step 4: The Services panel page**

Replace `apps/desktop/src/routes/+page.svelte`:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { onMount } from 'svelte';
	import {
		coreInfo,
		listServices,
		onServiceLog,
		onServiceState,
		serviceLogTail,
		startService,
		stopService,
		type CoreInfo,
		type IpcError
	} from '$lib/ipc';
	import { ServicesStore } from '$lib/services.svelte';

	const store = new ServicesStore({ listServices, serviceLogTail });
	let info = $state<CoreInfo | null>(null);
	let error = $state<IpcError | null>(null);

	onMount(() => {
		let unsubs: Array<() => void> = [];
		(async () => {
			try {
				await store.init();
				info = await coreInfo();
				unsubs = await Promise.all([
					onServiceState((ev) => store.applyState(ev)),
					onServiceLog((ev) => store.applyLog(ev))
				]);
			} catch (e) {
				error = e as IpcError;
			}
		})();
		return () => unsubs.forEach((u) => u());
	});

	async function act(fn: (id: string) => Promise<void>, id: string) {
		error = null;
		try {
			await fn(id);
		} catch (e) {
			error = e as IpcError;
		}
	}

	const levelClass = (level: string) =>
		level === 'error' ? 'text-red-700' : level === 'warn' ? 'text-amber-700' : 'text-neutral-500';
	const fmtTs = (t: number) => new Date(t).toLocaleTimeString(undefined, { hour12: false });
</script>

<main class="mx-auto flex h-screen max-w-3xl flex-col p-6 font-sans">
	<h1 class="text-xl font-semibold">Services</h1>

	{#if error}
		<div class="mt-3 rounded border border-red-400 bg-red-50 p-3 text-red-800" role="alert" data-testid="error-banner">
			<strong>Command failed ({error.kind})</strong>
			<span>{'message' in error ? error.message : ''}</span>
		</div>
	{/if}

	<section class="mt-4 divide-y rounded border" data-testid="services">
		{#each store.services as s (s.id)}
			<div class="flex items-center gap-4 p-3">
				<div class="min-w-0 flex-1">
					<div class="font-semibold">{s.displayName}</div>
					{#if s.endpoint}<div class="truncate font-mono text-xs text-neutral-500">{s.endpoint}</div>{/if}
				</div>
				<span
					class="rounded-full border px-2.5 py-0.5 text-xs font-semibold"
					class:text-emerald-700={s.state.kind === 'running'}
					class:text-amber-700={s.state.kind === 'starting'}
					class:text-red-700={s.state.kind === 'failed'}
					class:text-neutral-500={s.state.kind === 'stopped'}
					data-testid="pill-{s.id}"
				>
					● {s.state.kind}
				</span>
				{#if s.state.kind === 'stopped'}
					<button class="rounded border px-3 py-1 text-sm font-medium" onclick={() => act(startService, s.id)}>Start</button>
				{:else if s.state.kind === 'failed'}
					<button class="rounded border px-3 py-1 text-sm font-medium" onclick={() => act(startService, s.id)}>Retry</button>
				{:else}
					<button class="rounded border px-3 py-1 text-sm font-medium" onclick={() => act(stopService, s.id)}>Stop</button>
				{/if}
			</div>
			{#if s.state.kind === 'failed'}
				<div class="border-t bg-red-50 p-3 text-sm" data-testid="failed-{s.id}">
					<div class="font-semibold text-red-700">
						{s.displayName} failed{#if s.state.exit != null}&nbsp;(exit {s.state.exit}){/if}
					</div>
					<pre class="mt-2 overflow-x-auto rounded border bg-white p-2 font-mono text-xs">{s.state.stderrTail.join('\n')}</pre>
				</div>
			{/if}
		{/each}
	</section>

	<h2 class="mt-6 text-xs font-semibold tracking-wide text-neutral-500 uppercase">Log</h2>
	<div class="mt-2 flex-1 overflow-auto rounded border bg-neutral-50 p-2 font-mono text-xs leading-6" data-testid="log">
		{#each store.logs as l, i (i)}
			<div class="grid grid-cols-[70px_44px_1fr] gap-2">
				<span class="text-neutral-400 tabular-nums">{fmtTs(l.tsMs)}</span>
				<span class="font-bold {levelClass(l.level)}">{l.level}</span>
				<span class="whitespace-pre-wrap">{l.line}</span>
			</div>
		{/each}
	</div>

	{#if info}
		<p class="mt-3 text-xs text-neutral-500">
			OpenVHost {info.appVersion} · {info.os}/{info.arch} · <span class="font-mono">{info.openvhostHome}</span>
		</p>
	{/if}
</main>
```

- [ ] **Step 5: Frontend gates + manual smoke + commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop check && pnpm -C apps/desktop lint && pnpm -C apps/desktop build
cargo build --workspace   # openvhost CLI must exist next to the app binary for the demo
./scripts/dev.sh          # manual: Start → starting→running pills; ticks stream into the log;
                          # after 45 ticks the service fails (red pill + stderr tail + Retry);
                          # Stop while running → stopped. Ctrl+C when done.
git add -A && git commit -s -m "feat(desktop): services panel v0 driven by supervisor events"
```

---

### Task 7: On-demand matrix gate, PR, merge

**Files:** none new — pipeline exercise.

**Interfaces:**
- Consumes: branch `feat/p03-supervisor` complete; CI workflow currently DISABLED on GitHub (cost decision).
- Produces: merged main with the matrix (incl. first real Windows build of proc) green; workflow returned to disabled state afterward.

- [ ] **Step 1: Full local gate suite one more time**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cargo deny check licenses advisories && bash scripts/check-spdx.sh
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: everything green locally before spending any CI minutes.

- [ ] **Step 2: Enable CI, push, open the PR**

```bash
gh workflow enable ci.yml -R Dhanabhon/openvhost
git push -u origin feat/p03-supervisor
gh pr create --title "feat: P0-3 — openvhost-proc supervisor v0 with services panel" --body "$(cat <<'EOF'
Implements docs/superpowers/specs/2026-07-21-p03-supervisor-design.md:
ProcessDriver platform trait (post dual-specialist consultation), tokio
supervisor with the Stopped→Starting→Running→Failed state machine (raced
500ms bound, two-phase stop with stop-requested classification), ring-buffer
log capture, deterministic testchild, typed tauri-specta events, and a real
Services panel v0 driven by supervisor events.

## Platform test checklist (master plan §5)
- [x] macOS — full test suite + manual dev smoke (all four states via demo-ticker)
- [ ] Windows — matrix build + `#[cfg(windows)]` driver tests (this PR's on-demand run)

## Gates
- [x] quick green
- [ ] matrix green (first real Windows code in the repo)
- [x] Security-sensitive paths touched? → none (supervisor is not on the §6.2 security list)
EOF
)"
```

Expected: pushing after enable triggers `quick`; the PR triggers `matrix` on both OSes automatically (no separate dispatch needed).

- [ ] **Step 3: Watch checks; fix forward if the Windows leg is red**

Run: `gh pr checks --watch`
Likely first-run Windows issues and their shape: `windows-sys` feature list missing a symbol (add the exact `Win32_*` feature the compile error names), `creation_flags` import path (`std::os::windows::process::CommandExt`), or `#[cfg(windows)]` test assumptions. Fix with the narrowest change, `git commit -s`, push, watch again. Never weaken the guard/gates to pass.

- [ ] **Step 4: Merge, verify main, return CI to disabled**

```bash
gh pr merge --squash --delete-branch
git switch main && git pull --quiet
cargo test --workspace && pnpm -C apps/desktop build
gh workflow disable ci.yml -R Dhanabhon/openvhost   # back to the cost-saving default; re-enable per slice
```

Expected: main green locally; workflow state `disabled_manually` again (documented in the PR + memory that local gates remain the merge gate between on-demand runs).

---

## Spec traceability

| Spec requirement | Task |
|---|---|
| §3 architecture (Supervisor/ServiceTask/platform/events) | 1, 3, 4 |
| §4 state machine, raced bound, stop-requested classification, two-phase stop | 3 (pure), 4 (live) |
| §5 trait + behavioral contracts (opaque child, env allow-list, groups/flags, honest Windows graceful) | 1 |
| §5 reload-not-in-trait, LCD-fallback doc, ESRCH semantics | 1 (doc comments) |
| §6 commands + typed events, proc stays tauri-free | 5 (+ guard in 4) |
| §7 Services panel v0, demo-ticker via `openvhost __testchild`, ignore-stop Ctrl handler | 2, 5, 6 |
| §8 unit TDD, integration incl. zero-orphan probe + kill path + instant-death race | 3, 4 |
| §8 Windows `#[cfg(windows)]` driver code + one on-demand matrix run before merge | 1, 7 |
| §8 vitest store mapping; manual smoke via dev.sh | 6 |
| §9 IpcError extension with pointing details | 4 (spawn-failure log), 5 |
| §10 non-goals honored (no health checks/persistence/Job Objects/reload) | all |

