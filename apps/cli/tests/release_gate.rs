// SPDX-License-Identifier: GPL-3.0-or-later
//! What the hidden `__testchild` fixture does, asserted **per build profile**
//! (install design D7).
//!
//! Until this slice the fixture was unreachable in practice: the binary only
//! ever existed at `target/{debug,release}/openvhost`. Putting it on a user's
//! PATH turns `--probe-state P` — two `std::fs::write`s at a path the caller
//! names, with no confinement of any kind — into an undocumented capability of
//! a shipped tool. So it is gated out of release builds, and this file is the
//! proof.
//!
//! **The two tests are mirror images, and that is the point.** Each runs the
//! real binary and asserts on the *files it did or did not create*, never on a
//! `cfg!(debug_assertions)` expression: reading the flag the implementation is
//! written against would prove nothing about what actually shipped. The `#[cfg]`
//! attributes only choose which half applies to the profile being compiled;
//! `CARGO_BIN_EXE_openvhost` always points at the binary built with the test's
//! own profile, so each half is aimed at exactly the artifact it describes.
//!
//! Consequently `cargo test --workspace` (a debug profile) only ever runs the
//! debug half. The release claim is checked by:
//!
//! ```text
//! cargo test -p openvhost --release --test release_gate
//! ```
//!
//! and again, independently, by `scripts/stage-cli-sidecar.sh` — which refuses
//! to stage a binary that still answers `__testchild` — so the bundle cannot be
//! built from a binary that carries the fixture even if this file is skipped.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Output;

/// `clap`'s verdict on an argument it cannot make sense of (`Exit::Usage`).
#[cfg(not(debug_assertions))]
const USAGE: i32 = 64;

/// Run the binary built with **this test's own profile**.
fn openvhost(args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_openvhost"))
        .args(args)
        .output()
        .expect("run the openvhost binary")
}

/// The two paths `--probe-state P --probe-succeed-after N` writes: `P` itself
/// and `P.pid` (`testchild::run_probe`). Neither is validated, confined or
/// cleaned up — which is the whole reason this fixture must not ship.
fn probe_targets(dir: &Path) -> (PathBuf, PathBuf) {
    let state = dir.join("state");
    let pid = state.with_extension("pid");
    (state, pid)
}

/// Ask the fixture to do the one thing that leaves evidence on disk.
fn ask_the_fixture_to_write(state: &Path) -> Output {
    openvhost(&[
        "__testchild",
        "--probe-state",
        state.to_str().expect("a UTF-8 tempdir path"),
        "--probe-succeed-after",
        "1",
    ])
}

/// The desktop app's `demo_ticker_spec` — itself `#[cfg(debug_assertions)]` —
/// spawns the sibling `openvhost __testchild`, so a debug build must keep
/// answering it. Both profiles are gated on the same flag, which is what keeps
/// the spawner and the fixture from drifting apart.
#[cfg(debug_assertions)]
#[test]
fn a_debug_build_still_runs_the_hidden_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let (state, pid) = probe_targets(dir.path());

    let out = ask_the_fixture_to_write(&state);
    assert_eq!(
        out.status.code(),
        Some(0),
        "the probe fixture should have succeeded on attempt 1; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        state.exists(),
        "a debug build writes the counter at {state:?}"
    );
    assert!(pid.exists(), "a debug build writes the pid file at {pid:?}");

    // The tick loop, the other half of the fixture: `--exit` is honoured and
    // the ticks reach stdout.
    let ticks = openvhost(&["__testchild", "--lines", "2", "--exit", "3"]);
    assert_eq!(ticks.status.code(), Some(3), "--exit 3 is honoured");
    assert_eq!(
        String::from_utf8(ticks.stdout).unwrap().lines().count(),
        2,
        "--lines 2 emits two ticks"
    );
}

/// The shipped artifact. `__testchild` must reach `clap` as an unrecognised
/// verb and be refused like any other typo — writing nothing, anywhere.
///
/// The assertion is deliberately on the **filesystem**, not on the exit code
/// alone: an implementation that still ran the fixture and then failed would
/// satisfy a `!= 0` check while having written both files.
#[cfg(not(debug_assertions))]
#[test]
fn a_release_build_does_not_carry_the_hidden_fixture() {
    let dir = tempfile::tempdir().unwrap();
    let (state, pid) = probe_targets(dir.path());

    let out = ask_the_fixture_to_write(&state);
    assert!(
        !state.exists(),
        "a release build must not write the counter at {state:?}"
    );
    assert!(
        !pid.exists(),
        "a release build must not write the pid file at {pid:?}"
    );
    assert_eq!(
        out.status.code(),
        Some(USAGE),
        "__testchild should be refused as an unknown verb; stdout: {} stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );

    // The tick loop is gone too, so nothing reaches stdout.
    let ticks = openvhost(&["__testchild", "--lines", "2", "--exit", "3"]);
    assert_eq!(
        ticks.status.code(),
        Some(USAGE),
        "--exit must not be honoured by a release build"
    );
    assert!(
        String::from_utf8(ticks.stdout).unwrap().is_empty(),
        "a release build emits no ticks"
    );
}
