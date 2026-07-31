// SPDX-License-Identifier: GPL-3.0-or-later
//! Full-lifecycle integration tests against the real proc_testchild binary.
//! Poll-with-timeout only — never sleep-and-hope.
#![allow(clippy::unwrap_used)]

mod common;
use common::testchild_spec;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use openvhost_proc::{
    DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, SpawnSpec, Supervisor,
    SupervisorEvent, default_driver,
};
use tokio::sync::broadcast;

/// Wait budget for a graceful-stop-then-`Stopped` assertion.
///
/// Windows can never actually deliver graceful stop in v0: children spawn
/// under `CREATE_NO_WINDOW`, which gives each its own hidden console, so
/// `GenerateConsoleCtrlEvent` from us never reaches it — every Windows stop
/// rides the supervisor's 5s grace deadline through to `kill()`. Give it
/// enough headroom above that deadline; Unix's real SIGTERM path resolves
/// fast, so keep that assertion tight.
const STOP_TIMEOUT: Duration = if cfg!(windows) {
    Duration::from_secs(8)
} else {
    Duration::from_secs(3)
};

fn svc(id: &str, args: &[&str]) -> ServiceSpec {
    ServiceSpec {
        id: id.to_string(),
        display_name: id.to_string(),
        endpoint: None,
        spawn: testchild_spec(args),
        readiness: ReadinessProbe::default(),
        grace: DEFAULT_GRACE,
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

#[tokio::test]
async fn lifecycle_running_then_graceful_stop() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc("t1", &["--lines", "100", "--interval-ms", "100"]));
    let mut rx = sup.subscribe();
    sup.start("t1").unwrap();
    wait_state(&mut rx, "t1", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Starting)
    })
    .await;
    wait_state(&mut rx, "t1", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;
    let pid = sup
        .snapshot()
        .into_iter()
        .find(|s| s.id == "t1")
        .unwrap()
        .pid;
    assert!(pid.is_some(), "running service must report a pid");
    sup.stop("t1").unwrap();
    wait_state(&mut rx, "t1", STOP_TIMEOUT, |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
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
    sup.register(svc(
        "t2",
        &["--lines", "500", "--interval-ms", "100", "--ignore-stop"],
    ));
    let mut rx = sup.subscribe();
    sup.start("t2").unwrap();
    wait_state(&mut rx, "t2", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;
    let t0 = Instant::now();
    sup.stop("t2").unwrap();
    let final_state = wait_state(&mut rx, "t2", Duration::from_secs(8), |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
    assert!(
        matches!(final_state, ServiceState::Stopped),
        "kill path must classify as Stopped"
    );
    assert!(
        t0.elapsed() >= Duration::from_secs(5),
        "kill fires only after the 5s grace deadline"
    );
}

#[tokio::test]
async fn nonzero_exit_is_failed_with_stderr_tail() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc(
        "t3",
        &["--lines", "10", "--interval-ms", "10", "--fail-after", "2"],
    ));
    let mut rx = sup.subscribe();
    sup.start("t3").unwrap();
    let state = wait_state(&mut rx, "t3", Duration::from_secs(3), |s| {
        matches!(s, ServiceState::Failed { .. })
    })
    .await;
    match state {
        ServiceState::Failed { exit, stderr_tail } => {
            assert_eq!(exit, Some(1));
            assert!(
                stderr_tail.iter().any(|l| l.contains("ERROR")),
                "tail: {stderr_tail:?}"
            );
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

#[tokio::test]
async fn instant_death_reports_failed_before_500ms_timer() {
    let sup = Supervisor::new(default_driver());
    // --lines 0 --exit 1: exits immediately with code 1.
    sup.register(svc(
        "t4",
        &["--lines", "0", "--exit", "1", "--interval-ms", "1"],
    ));
    let mut rx = sup.subscribe();
    sup.start("t4").unwrap();
    // The raced 500ms bound must win: t4 must reach Failed directly, never
    // passing through Running first. Event ORDER, not a wall-clock
    // threshold, is the deterministic signal for that: under heavy CPU
    // contention, pure scheduling/reaping overhead alone can push wall-clock
    // elapsed past a few hundred ms even though the correct (raced-death)
    // branch was still taken well short of the 500ms timer — a fixed-ms
    // bound flakes on load, this does not.
    wait_state(&mut rx, "t4", Duration::from_secs(2), |s| match s {
        ServiceState::Failed { .. } => true,
        ServiceState::Running => {
            panic!("raced bound must not wait out the timer: observed Running before Failed")
        }
        _ => false,
    })
    .await;
}

#[tokio::test(flavor = "multi_thread")]
async fn concurrent_start_spawns_exactly_once() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc("t6", &["--lines", "50", "--interval-ms", "100"]));
    let mut rx = sup.subscribe();

    // Race two start() calls on separate OS threads. Post-fix, the
    // guard-and-transition in `start()` is atomic under one lock
    // acquisition, so exactly one of these ever gets past the no-op guard
    // and spawns — deterministically, regardless of scheduling.
    let sup_a = sup.clone();
    let sup_b = sup.clone();
    let ha = tokio::spawn(async move { sup_a.start("t6") });
    let hb = tokio::spawn(async move { sup_b.start("t6") });
    let (ra, rb) = tokio::join!(ha, hb);
    ra.unwrap().unwrap();
    rb.unwrap().unwrap();

    wait_state(&mut rx, "t6", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;

    // Poll until quiescent: two consecutive equal reads of the "spawned
    // pid" lines specifically, 100ms apart, bounded at 3s. The raw tail
    // never settles on its own (the child logs a fresh tick line every
    // 100ms for its whole 5s run), so we track the filtered spawn-line
    // view — that's the signal that proves any stray second spawn (had
    // the fix regressed) would have had time to log its own line before
    // we assert the count.
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut prev: Option<Vec<String>> = None;
    let spawn_lines = loop {
        let lines: Vec<String> = sup
            .log_tail("t6", 100)
            .unwrap()
            .into_iter()
            .map(|l| l.line)
            .filter(|l| l.contains("spawned pid"))
            .collect();
        if prev.as_ref() == Some(&lines) {
            break lines;
        }
        assert!(
            Instant::now() < deadline,
            "'spawned pid' lines in log tail never quiesced within 3s: {lines:?}"
        );
        prev = Some(lines);
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(
        spawn_lines.len(),
        1,
        "expected exactly one 'spawned pid' line under concurrent start(), got {}: {spawn_lines:?}",
        spawn_lines.len()
    );

    sup.stop("t6").unwrap();
    wait_state(&mut rx, "t6", STOP_TIMEOUT, |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
}

#[tokio::test]
async fn register_does_not_clobber_live_service() {
    let sup = Supervisor::new(default_driver());
    sup.register(svc("t7", &["--lines", "50", "--interval-ms", "100"]));
    let mut rx = sup.subscribe();
    sup.start("t7").unwrap();
    wait_state(&mut rx, "t7", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Running)
    })
    .await;

    let before = sup.snapshot().into_iter().find(|s| s.id == "t7").unwrap();
    assert_eq!(before.display_name, "t7");
    let original_pid = before.pid;
    assert!(original_pid.is_some(), "running service must report a pid");

    // Re-register the same live id with a different display_name — must be
    // a no-op, not an orphaning clobber of the already-running entry.
    sup.register(ServiceSpec {
        id: "t7".to_string(),
        display_name: "clobbered".to_string(),
        endpoint: None,
        spawn: testchild_spec(&["--lines", "1", "--interval-ms", "1"]),
        readiness: ReadinessProbe::default(),
        grace: DEFAULT_GRACE,
    });

    let after = sup.snapshot().into_iter().find(|s| s.id == "t7").unwrap();
    assert_eq!(
        after.display_name, "t7",
        "register() must not clobber a live entry's display_name"
    );
    assert_eq!(
        after.pid, original_pid,
        "register() must not clobber a live entry's pid"
    );
    assert!(matches!(after.state, ServiceState::Running));

    sup.stop("t7").unwrap();
    wait_state(&mut rx, "t7", STOP_TIMEOUT, |s| {
        matches!(s, ServiceState::Stopped)
    })
    .await;
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
        readiness: ReadinessProbe::default(),
        grace: DEFAULT_GRACE,
    });
    let mut rx = sup.subscribe();
    sup.start("t5").unwrap();
    wait_state(&mut rx, "t5", Duration::from_secs(2), |s| {
        matches!(s, ServiceState::Failed { .. })
    })
    .await;
    let tail = sup.log_tail("t5", 10).unwrap();
    assert!(
        tail.iter().any(|l| l.line.contains("openvhost-missing")),
        "failure log must name the missing program"
    );
}
