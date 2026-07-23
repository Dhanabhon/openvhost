// SPDX-License-Identifier: GPL-3.0-or-later
//! Behavior tests for the proc_testchild binary. Integration-test placement
//! is deliberate: `CARGO_BIN_EXE_*` is only set when compiling integration
//! tests, not unit tests in src/.
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

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

#[test]
fn fail_after_emits_error_and_exit_1() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_proc_testchild"))
        .args(["--lines", "5", "--interval-ms", "1", "--fail-after", "2"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("ERROR simulated failure after 2 ticks"));
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("tick 2/5") && !stdout.contains("tick 3/5"));
}

/// Kills + reaps the child on drop so a panic mid-assert never leaks it.
struct ChildGuard(std::process::Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn http_mode_serves_200_and_sentinel() {
    let port = common::ephemeral_port();
    let child = std::process::Command::new(env!("CARGO_BIN_EXE_proc_testchild"))
        .args(["--http", &port.to_string()])
        .spawn()
        .unwrap();
    let _guard = ChildGuard(child);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let resp = common::http_get(port, deadline).expect("http server never responded");
    assert!(resp.contains("200 OK"), "not a 200: {resp}");
    assert!(
        resp.contains(openvhost_proc::testchild::E2E_BODY),
        "200 body lacks the sentinel: {resp}"
    );
}
