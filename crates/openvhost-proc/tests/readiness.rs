// SPDX-License-Identifier: GPL-3.0-or-later
//! `ReadinessProbe::Command` + per-service `grace` (P1 MySQL lifecycle
//! design, spec D4). Poll-with-timeout / event-order assertions only — never
//! sleep-and-hope (mirrors `tests/supervisor.rs`).
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
use common::testchild_spec;

use std::path::Path;
use std::time::{Duration, Instant};

use openvhost_proc::{
    DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, Supervisor, SupervisorEvent,
    default_driver,
};
use tokio::sync::broadcast;

/// Mirrors `tests/supervisor.rs`'s non-Windows bound: Unix's real SIGTERM
/// path resolves fast for a child that does not `--ignore-stop`.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);

fn probe_argv(state: &Path, succeed_after: u64) -> Vec<String> {
    vec![
        env!("CARGO_BIN_EXE_proc_testchild").to_string(),
        "--probe-state".to_string(),
        state.to_string_lossy().into_owned(),
        "--probe-succeed-after".to_string(),
        succeed_after.to_string(),
    ]
}

fn probe_argv_with_delay(state: &Path, succeed_after: u64, delay_ms: u64) -> Vec<String> {
    let mut v = probe_argv(state, succeed_after);
    v.push("--probe-delay-ms".to_string());
    v.push(delay_ms.to_string());
    v
}

fn command_probe_svc(
    id: &str,
    service_args: &[&str],
    argv: Vec<String>,
    deadline: Duration,
) -> ServiceSpec {
    ServiceSpec {
        id: id.to_string(),
        display_name: id.to_string(),
        endpoint: None,
        spawn: testchild_spec(service_args),
        readiness: ReadinessProbe::Command { argv, deadline },
        grace: DEFAULT_GRACE,
    }
}

/// A child that ignores SIGTERM (`--ignore-stop`) and runs its own tick loop
/// for exactly `ignore_seconds` seconds before exiting cleanly ON ITS OWN —
/// no new testchild flag needed: `--lines N --interval-ms 1000` already
/// gives a process that is unkillable-by-signal for N seconds and then
/// self-terminates, which is exactly the shape group (c) needs.
fn ignoring_child_spec(id: &str, grace: Duration, ignore_seconds: u64) -> ServiceSpec {
    let lines = ignore_seconds.to_string();
    ServiceSpec {
        id: id.to_string(),
        display_name: id.to_string(),
        endpoint: None,
        spawn: testchild_spec(&["--ignore-stop", "--lines", &lines, "--interval-ms", "1000"]),
        readiness: ReadinessProbe::default(),
        grace,
    }
}

/// Consume events until `pred` matches a `StateChanged` for `id`, or panic at
/// timeout. Mirrors `tests/supervisor.rs`'s helper of the same name/shape.
async fn wait_state(
    rx: &mut broadcast::Receiver<SupervisorEvent>,
    id: &str,
    timeout: Duration,
    pred: impl Fn(&ServiceState) -> bool,
) -> ServiceState {
    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        assert!(
            !remaining.is_zero(),
            "timed out waiting for state on '{id}'"
        );
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(SupervisorEvent::StateChanged { id: eid, state, .. }))
                if eid == id && pred(&state) =>
            {
                return state;
            }
            // Exhaustive rather than `Ok(Ok(_))`: a new `SupervisorEvent`
            // variant must fail to compile HERE too. A wildcard would keep
            // compiling and keep skipping — which is correct for THIS helper
            // (it only ever waits on states), but it is exactly how a variant
            // that a future waiter DOES need slips past unnoticed.
            Ok(Ok(SupervisorEvent::StateChanged { .. }))
            | Ok(Ok(SupervisorEvent::Log { .. }))
            | Ok(Ok(SupervisorEvent::Registered { .. }))
            | Ok(Ok(SupervisorEvent::Unregistered { .. })) => continue,
            Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
            Ok(Err(e)) => panic!("event channel closed: {e}"),
            Err(_) => panic!("timed out waiting for state on '{id}'"),
        }
    }
}

/// Force-stops the service on drop so a panicking assertion never leaks a
/// child (mirrors `tests/e2e.rs` / `tests/orphan_reap.rs`'s `StopGuard`).
/// Blocking: `flavor = "multi_thread"` is required on every test using this,
/// or the guard's blocking poll would starve the very service_task it is
/// waiting on.
struct StopGuard<'a> {
    sup: &'a Supervisor,
    id: &'static str,
}
impl Drop for StopGuard<'_> {
    fn drop(&mut self) {
        let _ = self.sup.stop(self.id);
        let deadline = Instant::now() + Duration::from_secs(20);
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

#[cfg(unix)]
fn pid_is_alive(pid: i32) -> bool {
    // SAFETY: signal 0 performs no action; it only checks existence/permission.
    unsafe { libc::kill(pid, 0) == 0 }
}

// -------------------------------------------------------------------------
// (a) Command probe that succeeds on the Nth attempt.
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn command_probe_succeeds_on_nth_attempt_then_running() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("counter");
    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        id: "probe-a",
    };
    sup.register(command_probe_svc(
        "probe-a",
        &["--lines", "100", "--interval-ms", "100"],
        probe_argv(&state, 3),
        Duration::from_secs(5),
    ));
    let mut rx = sup.subscribe();
    sup.start("probe-a").unwrap();

    wait_state(&mut rx, "probe-a", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Starting)
    })
    .await;

    // Event-order assertion (mirrors
    // `instant_death_reports_failed_before_500ms_timer`): the first
    // non-Starting state observed must be Running, never Failed — a
    // timing-independent proof that the probe actually gated the
    // transition rather than firing immediately or failing outright.
    let final_state = wait_state(&mut rx, "probe-a", Duration::from_secs(5), |s| {
        !matches!(s, ServiceState::Starting)
    })
    .await;
    assert!(
        matches!(final_state, ServiceState::Running),
        "expected Running, got {final_state:?}"
    );

    // Exactly 3 attempts — not more (would mean the deadline/backoff logic
    // over-ran), not fewer (would mean a bug let it through early).
    let counter: u64 = std::fs::read_to_string(&state)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    assert_eq!(counter, 3, "expected exactly 3 probe attempts");

    sup.stop("probe-a").unwrap();
    wait_state(&mut rx, "probe-a", STOP_TIMEOUT, |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
}

// -------------------------------------------------------------------------
// (b) Probe that never succeeds within its deadline.
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn command_probe_deadline_elapsed_fails_with_probe_diagnostics_and_kills_the_child() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("counter");
    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        id: "probe-b",
    };
    sup.register(command_probe_svc(
        "probe-b",
        // Stays alive well past the probe's own short deadline below.
        &["--lines", "1000", "--interval-ms", "50"],
        probe_argv(&state, 1_000_000), // never reaches succeed_after
        Duration::from_millis(700),
    ));
    let mut rx = sup.subscribe();
    sup.start("probe-b").unwrap();
    wait_state(&mut rx, "probe-b", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Starting)
    })
    .await;
    // The `Starting` EVENT fires synchronously inside `Supervisor::start`,
    // before the tokio-spawned `service_task::run` (which sets the pid) is
    // necessarily scheduled — so the pid is not guaranteed present the
    // instant `Starting` is observed (existing tests only ever read it after
    // `Running`; poll here instead of assuming it's immediate).
    let pid_deadline = Instant::now() + Duration::from_secs(2);
    let pid = loop {
        if let Some(p) = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == "probe-b")
            .and_then(|s| s.pid)
        {
            break p;
        }
        assert!(
            Instant::now() < pid_deadline,
            "a Starting service never reported a pid"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };

    let final_state = wait_state(&mut rx, "probe-b", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Failed { .. })
    })
    .await;
    match final_state {
        ServiceState::Failed { stderr_tail, .. } => {
            // Self-review requirement: the probe's OWN stderr (not just the
            // fact that it failed) must actually reach the diagnostics.
            assert!(
                stderr_tail
                    .iter()
                    .any(|l| l.contains("probe") && l.contains("not ready")),
                "expected probe stderr diagnostics in stderr_tail, got {stderr_tail:?}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }

    // The underlying child must actually be torn down — a probe-deadline
    // Failed must never leave a process running unmanaged.
    #[cfg(unix)]
    {
        let pid = pid as i32;
        let deadline = Instant::now() + Duration::from_secs(3);
        while pid_is_alive(pid) {
            assert!(
                Instant::now() < deadline,
                "service child pid {pid} still alive after a probe-deadline Failed"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

/// Review fix: the deadline can also elapse WHILE a probe attempt is still
/// in flight (spec D4's real consumer, `mysqladmin --connect-timeout=1
/// ... ping`, can run ~1s — a plausible case, not a corner). The prior test
/// above only ever hits the "deadline elapsed BETWEEN attempts" branch
/// (700ms deadline / near-instant attempts), so it cannot exercise this.
/// This probe deliberately hangs for far longer than the deadline, after
/// printing a distinctive marker to stderr — proving the KILLED in-flight
/// attempt's own output (not a stale previous attempt's, and not just "no
/// attempt completed") reaches the Failed diagnostics.
const MID_ATTEMPT_MARKER: &str = "PROBE_MID_ATTEMPT_STDERR_MARKER_7f3a";

#[tokio::test(flavor = "multi_thread")]
async fn command_probe_deadline_mid_attempt_carries_that_attempts_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("counter");
    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        id: "probe-mid",
    };
    // Never reaches succeed_after and hangs 10s per attempt — far longer
    // than the 1s deadline below, so the deadline reliably fires while the
    // FIRST attempt is still asleep, well after it has already printed its
    // marker (immediately, before the sleep) but well before it could ever
    // exit on its own.
    let mut argv = probe_argv_with_delay(&state, 1_000_000, 10_000);
    argv.push("--probe-stderr-marker".to_string());
    argv.push(MID_ATTEMPT_MARKER.to_string());
    sup.register(command_probe_svc(
        "probe-mid",
        // Stays alive well past the probe's own short deadline below.
        &["--lines", "100", "--interval-ms", "100"],
        argv,
        Duration::from_secs(1),
    ));
    let mut rx = sup.subscribe();
    sup.start("probe-mid").unwrap();
    wait_state(&mut rx, "probe-mid", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Starting)
    })
    .await;

    let final_state = wait_state(&mut rx, "probe-mid", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Failed { .. })
    })
    .await;
    match final_state {
        ServiceState::Failed { stderr_tail, .. } => {
            assert!(
                stderr_tail.iter().any(|l| l.contains(MID_ATTEMPT_MARKER)),
                "expected the killed in-flight attempt's own stderr marker \
                 in stderr_tail, got {stderr_tail:?}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

// -------------------------------------------------------------------------
// (c) Per-service grace: a long grace outlasts an 8s-ignoring child; the
// PAIRED short-grace test is the vacuity proof (brief: "set grace back to
// 5s, watch it get SIGKILLed/fail").
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn long_grace_lets_the_child_exit_via_its_own_timer_not_a_kill() {
    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        id: "grace-long",
    };
    sup.register(ignoring_child_spec(
        "grace-long",
        Duration::from_secs(15),
        8,
    ));
    let mut rx = sup.subscribe();
    sup.start("grace-long").unwrap();
    wait_state(&mut rx, "grace-long", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;

    let t0 = Instant::now();
    sup.stop("grace-long").unwrap();
    let final_state = wait_state(&mut rx, "grace-long", Duration::from_secs(12), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    let elapsed = t0.elapsed();

    assert!(matches!(final_state, ServiceState::Stopped));
    assert!(
        elapsed >= Duration::from_secs(7) && elapsed < Duration::from_secs(12),
        "expected the child's own ~8s timer to end this (grace=15s never fires), got {elapsed:?}"
    );
    let tail = sup.log_tail("grace-long", 200).unwrap();
    assert!(
        !tail.iter().any(|l| l.line.contains("killing")),
        "grace=15s must not have escalated to a kill for an 8s-ignoring child: {tail:?}"
    );
}

/// THE VACUITY PROOF for the test above: the IDENTICAL 8s-ignoring child,
/// but with `grace: 5s` — the value `long_grace_...` would silently keep
/// passing under if `ServiceSpec.grace` were ignored and the old hardcoded
/// 5s constant used instead. Standing here as a permanent regression guard
/// (not a one-off manual step): if `grace` ever stopped being honored
/// per-spec, THIS test would start failing (elapsed would jump to ~8s and
/// "killing" would disappear), not the long-grace test.
#[tokio::test(flavor = "multi_thread")]
async fn vacuity_short_grace_kills_the_same_8s_ignoring_child_well_before_its_timer() {
    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        id: "grace-short",
    };
    sup.register(ignoring_child_spec(
        "grace-short",
        Duration::from_secs(5),
        8,
    ));
    let mut rx = sup.subscribe();
    sup.start("grace-short").unwrap();
    wait_state(&mut rx, "grace-short", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;

    let t0 = Instant::now();
    sup.stop("grace-short").unwrap();
    wait_state(&mut rx, "grace-short", Duration::from_secs(9), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    let elapsed = t0.elapsed();

    assert!(
        elapsed < Duration::from_secs(7),
        "expected the 5s grace to kill it well before the child's own 8s timer, got {elapsed:?}"
    );
    let tail = sup.log_tail("grace-short", 200).unwrap();
    assert!(
        tail.iter().any(|l| l.line.contains("killing")),
        "expected the grace deadline's kill path to have fired: {tail:?}"
    );
}

// -------------------------------------------------------------------------
// Extra: the supervised child exiting WHILE a probe attempt is still in
// flight must not leak the probe subprocess (self-review requirement).
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn service_exit_mid_probe_does_not_leak_the_probe_subprocess() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("counter");
    let sup = Supervisor::new(default_driver());
    let _guard = StopGuard {
        sup: &sup,
        id: "probe-leak",
    };
    sup.register(command_probe_svc(
        "probe-leak",
        // Exits ~150ms in — the probe below outlives it by design: its own
        // 60s delay is far longer than this test's bounded checks, so the
        // ONLY way it can disappear within those checks is an explicit
        // kill. (A shorter delay would make this test pass vacuously if the
        // kill/reap code were removed, since the probe would eventually die
        // of its own accord anyway — caught in review by literally
        // reverting the kill/reap and re-running this test.)
        &["--lines", "3", "--interval-ms", "50"],
        probe_argv_with_delay(&state, 1_000_000, 60_000),
        Duration::from_secs(10),
    ));
    let mut rx = sup.subscribe();
    sup.start("probe-leak").unwrap();
    wait_state(&mut rx, "probe-leak", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Starting)
    })
    .await;

    // Wait for proof the probe subprocess actually started (it writes its
    // own pid before sleeping).
    let pid_path = state.with_extension("pid");
    let start_deadline = Instant::now() + Duration::from_secs(3);
    let probe_pid: i32 = loop {
        if let Ok(s) = std::fs::read_to_string(&pid_path)
            && let Ok(p) = s.trim().parse()
        {
            break p;
        }
        assert!(
            Instant::now() < start_deadline,
            "probe subprocess never wrote its pid sentinel"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    #[cfg(unix)]
    assert!(
        pid_is_alive(probe_pid),
        "probe subprocess pid {probe_pid} should be alive right after starting"
    );

    // Spec D4: any exit during a Command probe is Failed — the service
    // child exits on its own while the (3s-delayed) probe is still in flight.
    let final_state = wait_state(&mut rx, "probe-leak", Duration::from_secs(5), |s| {
        matches!(s, ServiceState::Failed { .. })
    })
    .await;
    assert!(matches!(final_state, ServiceState::Failed { .. }));

    // The probe subprocess itself must be gone shortly after — proving the
    // outer race killed and reaped it rather than merely dropping its future.
    #[cfg(unix)]
    {
        let leak_deadline = Instant::now() + Duration::from_secs(3);
        while pid_is_alive(probe_pid) {
            assert!(
                Instant::now() < leak_deadline,
                "probe subprocess pid {probe_pid} leaked after the service exited mid-probe"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

// -------------------------------------------------------------------------
// (d) Default-spec behavior byte-compatible: the REAL proof is that every
// EXISTING test in tests/supervisor.rs, tests/e2e.rs, and tests/orphan_reap.rs
// — unmodified assertions — stays green now that their `ServiceSpec` literals
// carry `readiness: ReadinessProbe::default()` / `grace: DEFAULT_GRACE` (see
// the task report for the full-workspace run proving this). This one test
// just pins the two constants' actual VALUES directly, so a future change to
// either fails here first with a precise message, rather than only
// indirectly via a timing assertion elsewhere.
// -------------------------------------------------------------------------

#[test]
fn defaults_match_todays_hardcoded_values() {
    assert!(
        matches!(
            ReadinessProbe::default(),
            ReadinessProbe::AliveAfter(d) if d == Duration::from_millis(500)
        ),
        "ReadinessProbe::default() must stay AliveAfter(500ms) — today's behavior"
    );
    assert_eq!(
        DEFAULT_GRACE,
        Duration::from_secs(5),
        "DEFAULT_GRACE must stay 5s — today's hardcoded GRACE_DEADLINE"
    );
}
