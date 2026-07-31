// SPDX-License-Identifier: GPL-3.0-or-later
//! `ServiceState::Failed { stderr_tail }` must describe the run that failed —
//! all of it, and nothing else.
//!
//! Two independent defects made that false, and each half of the fix is
//! proven separately here:
//!
//! - **The tail was never cleared between runs.** It is created once in
//!   `Supervisor::register` and only ever appended to, so a failure inherited
//!   the previous run's lines. Measured against real nginx over five identical
//!   start-fail cycles, three failures reported *nothing* from the failing run
//!   — only the prior run's clean-shutdown `[notice]` lines, offered as the
//!   reason a different process failed to start.
//! - **The tail was snapshotted without draining the reader.** `child.wait()`
//!   resolving is the only thing gating classification, and nothing between it
//!   and the snapshot yields, so whatever was still in the pipe never made it
//!   in. That is exactly the shape of `nginx: [emerg] bind() … failed` — write
//!   the reason, then die.
//!
//! Only the clear half is proven here. **The drain half is proven at its seam
//! instead**, in `service_task.rs`'s `finish_waits_for_a_reader_that_has_not_pushed_yet`
//! — not out of convenience, but because two end-to-end reproductions were
//! written against `proc_testchild` first and both were thrown away for being
//! VACUOUS: they passed with the drain reverted, 5/5 and 40/40. A burst big
//! enough to overflow the pipe (4000 lines, ~230 KiB) forces the reader to be
//! running, and it then wins every time; a small burst followed by an instant
//! exit does not lose either, because tokio drains a ready task before it
//! re-polls the reactor. A test child cannot be made to lose the race a real
//! service loses under load. What `finish` genuinely lacked is a
//! happens-before edge between "the reader consumed the last line" and "the
//! tail is snapshotted", and that is exactly provable at the seam.
#![allow(clippy::unwrap_used)]

mod common;
use common::testchild_spec;

use std::time::{Duration, Instant};

use openvhost_proc::testchild::stderr_line;
use openvhost_proc::{
    DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, Supervisor, default_driver,
};

/// Generous: these runs are milliseconds long, so this only ever bounds a
/// hang.
const TERMINAL_TIMEOUT: Duration = Duration::from_secs(20);

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

/// Poll the supervisor's own snapshot until `id` settles.
///
/// Polling rather than the broadcast stream on purpose: the 4000-line test
/// pushes thousands of `Log` events through a 256-slot channel, and a test
/// that has to reason about receiver lag to read a terminal state is testing
/// the wrong thing. Safe against reading the *pre-start* `Stopped`, because
/// `Supervisor::start` writes `Starting` under its lock before it returns —
/// every caller here polls strictly after that.
async fn wait_terminal(sup: &Supervisor, id: &str) -> ServiceState {
    let deadline = Instant::now() + TERMINAL_TIMEOUT;
    loop {
        let state = sup
            .snapshot()
            .into_iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("'{id}' is not registered"))
            .state;
        match state {
            ServiceState::Stopped | ServiceState::Failed { .. } => return state,
            ServiceState::Starting | ServiceState::Running => {}
        }
        assert!(
            Instant::now() < deadline,
            "'{id}' never reached a terminal state within {TERMINAL_TIMEOUT:?}"
        );
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn failed_tail(state: &ServiceState) -> &[String] {
    match state {
        ServiceState::Failed { stderr_tail, .. } => stderr_tail,
        other => panic!("expected Failed, got {other:?}"),
    }
}

/// THE CLEAR HALF. A service that ran, was stopped, and is then started again
/// and fails must report the failing run's stderr and **none** of the earlier
/// run's.
///
/// Both runs use the same argv (the entry, and therefore the tail, is per-id
/// — re-registering would construct a fresh `Entry` and hide the very bug
/// under test), so the two runs write byte-identical lines. That is what
/// makes the length assertion the sharp one: with the clear reverted the tail
/// holds SIX lines — run 1's three followed by run 2's three — and `assert_eq`
/// against three fails.
///
/// Run 1 ends `Stopped` rather than `Failed` because `stop()` is called while
/// it is still `Starting`: `classify_exit` checks the stop flag before the
/// exit status, so a user-requested stop is a clean stop however the child
/// exits. That is deterministic, not raced — `start()` only spawns the service
/// task, so the `stop()` on the next line runs before the child is even
/// spawned.
#[tokio::test]
async fn a_failing_run_reports_its_own_stderr_and_none_of_the_previous_runs() {
    const N: u64 = 3;
    let sup = Supervisor::new(default_driver());
    sup.register(svc(
        "tail-clear",
        &["--stderr-lines", "3", "--lines", "0", "--exit", "1"],
    ));

    // Run 1: stopped by the user while starting → Stopped, leaving its own
    // three stderr lines in the tail.
    sup.start("tail-clear").unwrap();
    sup.stop("tail-clear").unwrap();
    let first = wait_terminal(&sup, "tail-clear").await;
    assert!(
        matches!(first, ServiceState::Stopped),
        "the premise of this test is a previous run that did NOT fail; got {first:?}"
    );

    // Run 2: the same child, nobody stopping it → Failed.
    sup.start("tail-clear").unwrap();
    let second = wait_terminal(&sup, "tail-clear").await;
    let tail = failed_tail(&second);
    let expected: Vec<String> = (1..=N).map(|i| stderr_line(i, N)).collect();
    assert_eq!(
        tail, expected,
        "the failure must carry exactly this run's stderr; a longer tail means the previous run's \
         lines were never cleared"
    );
}
