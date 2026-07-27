// SPDX-License-Identifier: GPL-3.0-or-later
//! Dropping a run must kill the child's whole process group — the P0-8
//! invariant, restated for the one-shot runner.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
use common::testchild_spec;

use std::time::Duration;

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
