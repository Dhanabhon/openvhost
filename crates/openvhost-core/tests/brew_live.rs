// SPDX-License-Identifier: GPL-3.0-or-later
//! Proves the task runner works against the real brew binary, not only
//! against proc_testchild. Read-only and fast: no install is ever run here —
//! that would take minutes and change the machine of whoever runs the suite.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[tokio::test]
async fn brew_version_runs_through_the_task_runner() {
    let Some(brew) = openvhost_core::find_brew() else {
        eprintln!("SKIP brew_live: no brew found in the known prefixes");
        return;
    };
    let spec = openvhost_proc::SpawnSpec {
        program: brew,
        args: vec![std::ffi::OsString::from("--version")],
        cwd: None,
        env: vec![],
    };
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let code = openvhost_proc::run_task(openvhost_proc::default_driver(), spec, tx)
        .await
        .unwrap();
    assert_eq!(code, Some(0));

    let mut saw_banner = false;
    while let Some(e) = rx.recv().await {
        if let openvhost_proc::TaskEvent::Line { text, .. } = e
            && text.starts_with("Homebrew")
        {
            saw_banner = true;
        }
    }
    assert!(saw_banner, "no Homebrew banner in the output");
}
