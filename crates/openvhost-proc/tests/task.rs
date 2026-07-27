// SPDX-License-Identifier: GPL-3.0-or-later
//! Behavior tests for `run_task` (the one-shot task runner). Integration-test
//! placement is deliberate, same reason as `tests/testchild_bin.rs`:
//! `CARGO_BIN_EXE_*` is only populated when compiling an integration test,
//! not a unit test compiled inside `src/`.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
use common::testchild_spec;

use openvhost_proc::{TaskEvent, TaskStream, default_driver, run_task};

fn collect(rx: &mut tokio::sync::mpsc::Receiver<TaskEvent>) -> Vec<TaskEvent> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn streams_every_line_in_order_then_reports_the_exit_code() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let code = run_task(
        default_driver(),
        testchild_spec(&["--lines", "3", "--interval-ms", "1", "--exit", "0"]),
        tx,
    )
    .await
    .unwrap();
    assert_eq!(code, Some(0));

    let events = collect(&mut rx);
    let lines: Vec<&str> = events
        .iter()
        .filter_map(|e| match e {
            TaskEvent::Line { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 3, "got {events:?}");
    // Order matters: a reader that races its two pipes would interleave.
    assert!(
        lines[0] < lines[1] && lines[1] < lines[2],
        "out of order: {lines:?}"
    );
    assert!(matches!(
        events.last(),
        Some(TaskEvent::Finished { code: Some(0) })
    ));
}

#[tokio::test]
async fn a_non_zero_exit_is_an_outcome_not_an_error() {
    // "brew said no" must reach the caller as data it can render, not as
    // a ProcError that looks like the runner itself broke.
    let (tx, _rx) = tokio::sync::mpsc::channel(64);
    let code = run_task(
        default_driver(),
        testchild_spec(&["--lines", "1", "--exit", "3"]),
        tx,
    )
    .await
    .unwrap();
    assert_eq!(code, Some(3));
}

#[tokio::test]
async fn a_missing_program_is_a_proc_error() {
    let (tx, _rx) = tokio::sync::mpsc::channel(4);
    let mut spec = testchild_spec(&[]);
    spec.program = std::path::PathBuf::from("/nonexistent/openvhost-not-a-program");
    assert!(run_task(default_driver(), spec, tx).await.is_err());
}

#[tokio::test]
async fn stderr_lines_are_tagged_as_stderr() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    // proc_testchild's --fail-after writes its "simulated failure" diagnostic
    // to stderr (confirmed in tests/testchild_bin.rs), then exits 1.
    let _ = run_task(
        default_driver(),
        testchild_spec(&["--lines", "2", "--fail-after", "1"]),
        tx,
    )
    .await;
    let events = collect(&mut rx);
    assert!(
        events.iter().any(|e| matches!(
            e,
            TaskEvent::Line {
                stream: TaskStream::Stderr,
                ..
            }
        )),
        "no stderr line was tagged: {events:?}"
    );
}
