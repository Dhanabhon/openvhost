// SPDX-License-Identifier: GPL-3.0-or-later
//! P0-8 Task 4 wiring proof: crash-orphan cleanup end to end through
//! `Supervisor::with_orphan_cleanup` — record at spawn, remove on a clean
//! terminal state, reap at construction, and the single-instance lock.
//!
//! `relaunch_reaps_a_recorded_live_process` is the master-plan exit
//! criterion ("kill app hard → relaunch → orphan reaped"), modeled
//! headlessly WITHOUT the brief's drop-a-live-`Supervisor` dance: dropping a
//! `Supervisor` does NOT abort its tokio service task (the task holds its
//! own `Arc<Inner>` clone independent of the `Supervisor` handle), so the
//! child would stay supervised — and could even auto-restart — racing the
//! very assertions the test is trying to make. Instead this spawns a
//! process directly as its own process-group leader, entirely OUTSIDE any
//! supervisor (modeling "a prior run of the app recorded it, then the app
//! was killed hard"), hand-writes a matching `SupervisedRecord`, and
//! constructs a FRESH `Supervisor::with_orphan_cleanup` on that same
//! registry — proving the constructor reaps a recorded, identity-matching
//! live process at startup, exactly as a relaunch would.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use openvhost_proc::platform::process_start_time;
use openvhost_proc::{
    FileRegistry, InstanceLock, ProcIdentity, ProcessRegistry, ServiceSpec, ServiceState,
    SpawnSpec, SupervisedRecord, Supervisor, default_driver, default_reaper,
};

fn sleeper_spec() -> ServiceSpec {
    ServiceSpec {
        id: "orphan-svc".into(),
        display_name: "orphan svc".into(),
        endpoint: None,
        spawn: SpawnSpec {
            program: "/bin/sleep".into(),
            args: ["600"].iter().map(std::ffi::OsString::from).collect(),
            cwd: None,
            env: vec![],
        },
    }
}

// ---------------------------------------------------------------------
// Test 1: record-at-spawn writes a matching record; a clean stop removes it.
// ---------------------------------------------------------------------

/// Poll `sup.snapshot()` until `id` reaches `Running`, returning its pid.
async fn wait_running_pid(sup: &Supervisor, id: &str, timeout: Duration) -> u32 {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(s) = sup.snapshot().into_iter().find(|s| s.id == id)
            && matches!(s.state, ServiceState::Running)
        {
            return s.pid.unwrap();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for '{id}' to reach Running"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Poll `sup.snapshot()` until `id`'s state matches `pred`.
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
        if let Some(s) = &st
            && pred(s)
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timeout waiting on '{id}'; last state: {st:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

/// Force-stops the service even when an assertion panics mid-test: a
/// failing run must never leak a live `/bin/sleep` (mirrors
/// `openvhost-core`'s `macos_stack.rs` `StopGuard`).
struct StopGuard<'a> {
    sup: &'a Supervisor,
    id: &'static str,
}

impl Drop for StopGuard<'_> {
    fn drop(&mut self) {
        let _ = self.sup.stop(self.id);
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            let all_terminal = self
                .sup
                .snapshot()
                .iter()
                .all(|s| !matches!(s.state, ServiceState::Starting | ServiceState::Running));
            if all_terminal {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

// `flavor = "multi_thread"`: `StopGuard::drop()` (the safety net for a
// panic before the test body's own `stop()`+`wait_state()`) blocks the
// CURRENT thread with `std::thread::sleep` (Drop cannot be `async`), which
// on a single-threaded runtime would starve the very service_task it is
// waiting on — nothing else could ever poll it forward, so the guard would
// spin out its whole deadline and leak the child. A second worker thread
// lets the service_task keep making progress while the guard's thread
// blocks (same reason `openvhost-core`'s `macos_stack.rs` `StopGuard` test
// uses this flavor).
#[tokio::test(flavor = "multi_thread")]
async fn record_at_spawn_writes_and_clean_stop_removes() {
    let home = tempfile::Builder::new()
        .prefix("ovh-record")
        .tempdir_in("/tmp")
        .unwrap();
    let registry = Arc::new(FileRegistry::new(&home.path().join("run")));

    let sup = Supervisor::with_orphan_cleanup(default_driver(), registry.clone(), default_reaper());
    let _guard = StopGuard {
        sup: &sup,
        id: "orphan-svc",
    };
    sup.register(sleeper_spec());
    sup.start("orphan-svc").unwrap();

    let pid = wait_running_pid(&sup, "orphan-svc", Duration::from_secs(5)).await;

    // Record-at-spawn: the registry must already hold a record for this
    // service whose identity.pid matches the pid the supervisor reports.
    let recorded = registry.list_current_boot().unwrap();
    let rec = recorded.iter().find(|r| r.service_id == "orphan-svc");
    assert!(
        rec.is_some_and(|r| r.identity.pid == pid),
        "expected a record for 'orphan-svc' with pid {pid}, got {recorded:?}"
    );

    // Clean stop.
    sup.stop("orphan-svc").unwrap();
    wait_state(&sup, "orphan-svc", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;

    // Remove-on-terminal-state: the registry must no longer contain it.
    let after = registry.list_current_boot().unwrap();
    assert!(
        after.is_empty(),
        "expected the registry empty after a clean stop, got {after:?}"
    );
}

// ---------------------------------------------------------------------
// Test 2: THE EXIT CRITERION — a fresh Supervisor reaps a recorded,
// identity-matching live process at construction.
// ---------------------------------------------------------------------

/// Spawn `/bin/sleep 600` as its OWN process-group leader (mirrors the
/// supervisor's `process_group(0)` containment), entirely OUTSIDE any
/// supervisor. Returns its pid.
///
/// `#[allow(clippy::zombie_processes)]`: the `Child` handle is deliberately
/// dropped here — cleanup reaps the returned pid itself via raw
/// `libc::waitpid` calls (`KillOnDrop`, `wait_until_dead`), which this
/// lint's local dataflow analysis can't see across the function boundary
/// (same pattern already established by the reap-orchestration tests in
/// `src/orphan/reap.rs`).
#[allow(clippy::zombie_processes)]
fn spawn_sleeper() -> u32 {
    use std::os::unix::process::CommandExt;
    let child = std::process::Command::new("/bin/sleep")
        .arg("600")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .unwrap();
    child.id()
}

/// Zombie-proof liveness check: unlike `kill(pid, 0)` (which reports success
/// for a ZOMBIE too — a killed-but-unreaped child's process entry persists
/// until something reaps it), `waitpid(pid, WNOHANG)` returns `0` only while
/// the child is genuinely still running, and non-zero the moment it has
/// exited (reaping it as a side effect). Polls with a bounded deadline
/// rather than a blocking wait: a blocking wait cannot distinguish "just
/// needs a moment to finish exiting" from "the reap silently failed and the
/// process is still running" — it would hang for the fixture's full `sleep
/// 600` and then report a spurious pass.
fn wait_until_dead(pid: u32, deadline: Duration) {
    let start = Instant::now();
    loop {
        let mut status: libc::c_int = 0;
        // SAFETY: pid is a plain integer parameter; `status` is a valid,
        // writable local; WNOHANG never blocks; this test is the real
        // OS-level parent of `pid` (spawned directly by `spawn_sleeper`).
        let ret = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        if ret != 0 {
            return; // reaped (ret == pid), or already gone (ret < 0, ECHILD)
        }
        assert!(
            start.elapsed() < deadline,
            "pid {pid} did not exit within {deadline:?} — the reap likely failed to kill it"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// RAII safety net: even if an assertion above panics, this unconditionally
/// SIGKILLs the fixture's process group and reaps it, so a failing test run
/// never leaks a live `/bin/sleep`.
struct KillOnDrop {
    pid: libc::pid_t,
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        // SAFETY: `pid` is this test's own fixture — its own process-group
        // leader (spawned via `process_group(0)`, so pgid == pid); ESRCH
        // (already gone) is expected and fine.
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
        // SAFETY: pid is our own direct child, spawned by this same test.
        // Blocking waitpid is safe here: either it was just sent SIGKILL
        // (unblockable/unignorable, so this returns as soon as the kernel
        // finishes the exit) or it's already reaped (returns immediately
        // with ECHILD) — no hang path either way.
        unsafe {
            libc::waitpid(self.pid, std::ptr::null_mut(), 0);
        }
    }
}

#[test]
fn relaunch_reaps_a_recorded_live_process() {
    // Spawn a long-lived process as its own process-group leader, entirely
    // OUTSIDE any supervisor — models "a prior run of the app recorded this
    // process, then the app was killed hard" (no clean stop, no remove()).
    let pid = spawn_sleeper();
    let _guard = KillOnDrop {
        pid: pid as libc::pid_t,
    };
    let start_time = process_start_time(pid).unwrap().unwrap();

    let home = tempfile::Builder::new()
        .prefix("ovh-relaunch")
        .tempdir_in("/tmp")
        .unwrap();
    let registry = Arc::new(FileRegistry::new(&home.path().join("run")));
    registry
        .record(&SupervisedRecord {
            service_id: "orphan-svc".into(),
            identity: ProcIdentity { pid, start_time },
            recorded_at_ms: 0,
        })
        .unwrap();

    // Relaunch: a FRESH supervisor construction on the SAME registry reaps
    // at construction — before anything can be registered or started.
    let _sup =
        Supervisor::with_orphan_cleanup(default_driver(), registry.clone(), default_reaper());

    // Any kill was issued synchronously inside the constructor above; give
    // the kernel a bounded window to finish the exit (never a fixed
    // sleep-and-hope — see `wait_until_dead`'s doc comment).
    wait_until_dead(pid, Duration::from_secs(2));
    let after = registry.list_current_boot().unwrap();
    assert!(
        after.is_empty(),
        "registry must be cleared after the reap, got {after:?}"
    );
}

// ---------------------------------------------------------------------
// Test 3: single-instance lock.
// ---------------------------------------------------------------------

#[test]
fn second_instance_cannot_acquire_the_lock() {
    let home = tempfile::Builder::new()
        .prefix("ovh-lock")
        .tempdir_in("/tmp")
        .unwrap();
    let run = home.path().join("run");
    let a = InstanceLock::acquire(&run).unwrap();
    assert!(a.is_some(), "first acquires");
    let b = InstanceLock::acquire(&run).unwrap();
    assert!(b.is_none(), "second is refused while the first is held");
    drop(a);
    let c = InstanceLock::acquire(&run).unwrap();
    assert!(c.is_some(), "acquirable again once released");
}
