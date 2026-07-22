// SPDX-License-Identifier: GPL-3.0-or-later
//! Behavior tests for the proc_testchild binary. Integration-test placement
//! is deliberate: `CARGO_BIN_EXE_*` is only set when compiling integration
//! tests, not unit tests in src/.
#![allow(clippy::unwrap_used)]

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
