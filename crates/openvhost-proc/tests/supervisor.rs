// SPDX-License-Identifier: GPL-3.0-or-later
//! Full-lifecycle integration tests against the real proc_testchild binary.
//! Poll-with-timeout only — never sleep-and-hope.
#![allow(clippy::unwrap_used)]

use std::ffi::OsString;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use openvhost_proc::{
    ServiceSpec, ServiceState, SpawnSpec, Supervisor, SupervisorEvent, default_driver,
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
    wait_state(&mut rx, "t1", Duration::from_secs(3), |s| {
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
