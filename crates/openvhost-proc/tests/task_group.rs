// SPDX-License-Identifier: GPL-3.0-or-later
//! Dropping a run must kill the child's whole process group — the P0-8
//! invariant, restated for the one-shot runner.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
use common::testchild_spec;

use std::time::{Duration, Instant};

#[tokio::test]
async fn dropping_the_run_kills_a_child_that_ignores_a_polite_stop() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    // --ignore-stop: only a group kill ends this. If the runner merely dropped
    // its handle, the process would outlive the test and hold its pipes open.
    let spec = testchild_spec(&["--lines", "1000", "--interval-ms", "50", "--ignore-stop"]);
    let run = tokio::spawn(async move {
        let _ = openvhost_proc::run_task(openvhost_proc::default_driver(), spec, tx).await;
    });

    // Wait for proof it is actually running before abandoning it.
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("child produced no output")
        .expect("channel closed");
    assert!(
        matches!(first, openvhost_proc::TaskEvent::Line { .. }),
        "expected output before abandoning the run, got {first:?}"
    );

    run.abort();
    let _ = run.await;

    // The channel closes once every sender is dropped, which only happens
    // after the reader tasks end — which only happens when the pipes close,
    // which only happens when the process actually dies.
    let closed = tokio::time::timeout(Duration::from_secs(10), async {
        while rx.recv().await.is_some() {}
    })
    .await;
    assert!(
        closed.is_ok(),
        "the abandoned child was still alive and writing"
    );
}

/// The test above proves the *direct* child dies, but `proc_testchild` never
/// forks a descendant of its own, and `process_group(0)` makes it the leader
/// of a group of exactly one — so `pgid == pid` and a regression that swapped
/// the group kill for a plain `tokio::process::Child::kill()` (single pid
/// only, the anti-pattern `platform/unix.rs` warns against) would still pass
/// it. This test forks a real grandchild — inheriting the same process
/// group — and asserts IT dies too, which is the actual P0-8 claim: an
/// abandoned `brew install` must not leave its forked tree (curl, tar, ruby,
/// ...) running.
#[tokio::test]
async fn dropping_the_run_kills_a_forked_grandchild_too() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let spec = testchild_spec(&[
        "--spawn-child",
        "--lines",
        "1000",
        "--interval-ms",
        "50",
        "--ignore-stop",
    ]);
    let run = tokio::spawn(async move {
        let _ = openvhost_proc::run_task(openvhost_proc::default_driver(), spec, tx).await;
    });

    // Read lines until the helper reports the grandchild's pid.
    let grandchild_pid: i32 = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = rx
                .recv()
                .await
                .expect("channel closed before a child-pid line appeared");
            if let openvhost_proc::TaskEvent::Line { text, .. } = &event
                && let Some(rest) = text.strip_prefix("child-pid: ")
            {
                return rest
                    .trim()
                    .parse::<i32>()
                    .expect("child-pid line was not a number");
            }
        }
    })
    .await
    .expect("no child-pid line within the deadline");

    // SAFETY: signal 0 performs no action; it only checks existence/permission.
    let alive_before = unsafe { libc::kill(grandchild_pid, 0) } == 0;
    assert!(
        alive_before,
        "grandchild {grandchild_pid} was not alive before the run was abandoned"
    );

    run.abort();
    let _ = run.await;

    // Poll for the grandchild's death with a generous deadline. If the group
    // kill regresses to a single-pid kill, this exhausts the deadline and the
    // assertion below fails with a clear message — it does not hang forever.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut still_alive = true;
    while Instant::now() < deadline {
        // SAFETY: signal 0 performs no action; it only checks existence/permission.
        if unsafe { libc::kill(grandchild_pid, 0) } != 0 {
            still_alive = false;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Defensive cleanup on BOTH the pass and fail path: if the grandchild
    // somehow survived, kill it directly before this test returns so a
    // failing run does not leave a stray process on the developer's machine.
    if still_alive {
        // SAFETY: plain kill syscall, cleaning up a leaked descendant.
        unsafe { libc::kill(grandchild_pid, libc::SIGKILL) };
    }

    assert!(
        !still_alive,
        "grandchild {grandchild_pid} was still alive {:?} after the run was abandoned — \
         the group kill did not reach it (regression: a direct-child-only kill, e.g. \
         tokio::process::Child::kill(), would leave this process running)",
        Duration::from_secs(10)
    );
}
