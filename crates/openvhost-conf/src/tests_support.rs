// SPDX-License-Identifier: GPL-3.0-or-later
//! Executable test fixtures that have already paid macOS's XProtect
//! first-execution cost. `#[cfg(test)]`-only.
//!
//! Why this exists — see
//! docs/superpowers/specs/2026-08-09-p1-validator-timeout-design.md, which
//! carries the measurements. In short: the first exec of a file with
//! never-before-seen bytes costs ~396 ms, XProtect serializes it through one
//! XPC service, and the cost rises ~390 ms per CONCURRENT first-exec — so
//! around thirteen of them push a single probe past [`crate::PROBE_TIMEOUT`]'s
//! 5 s and the probe reports `ValidatorTimeout` against a fake whose whole job
//! is to `exit 1` immediately. Ordinary build churn in another worktree is
//! enough to get there. The evaluation is keyed to the INODE (not the path,
//! not the bytes) and a file's SECOND exec costs 5.6 ms, so running the
//! fixture once at creation time moves the entire cost outside the window the
//! test is timing. Measured: 307/320 timeouts without this, 0/320 with it.
//!
//! `PROBE_TIMEOUT` itself is right for the product by 300–1800× (a real
//! `nginx -v` under the same pressure is 2.7 ms p50, 17 ms max), which is why
//! the fixtures move and the constant does not.
//!
//! The warm-up must not become an extra RUN of the fixture. Bodies in this
//! workspace `sleep 30`, poll for a file a sibling fixture creates, and write
//! marker files another fixture's assertion depends on. [`fixture_script`]
//! therefore prefixes every fixture with a line that exits before the body
//! when [`WARMUP_ENV`] is set, and [`warm_up`] is the only caller that ever
//! sets it — so the extra exec pays XProtect and observably does nothing else.

#![allow(clippy::unwrap_used)]

use std::path::Path;

/// Set by [`warm_up`] on the warm-up child alone, never on this process. A
/// real probe cannot see it even by accident: [`crate::run_bounded`]
/// `env_clear()`s and re-assembles the child environment from `probe_env`
/// before every spawn.
const WARMUP_ENV: &str = "OPENVHOST_FIXTURE_WARMUP";

/// A `#!/bin/sh` fixture that runs `body` for every caller except [`warm_up`].
///
/// The guard reads `${VAR:-}` rather than `$VAR` so it is also correct under
/// the cleared environment a real probe runs in, where the name is unset.
pub(crate) fn fixture_script(body: &str) -> String {
    format!("#!/bin/sh\nif [ -n \"${{{WARMUP_ENV}:-}}\" ]; then exit 0; fi\n{body}\n")
}

/// Exec `path` once and discard everything about it, so macOS evaluates this
/// inode HERE instead of inside a bounded probe.
///
/// Blocking [`std::process`], never `tokio::process`, and never `.await`:
/// `inspect.rs`'s timeout test runs on a PAUSED clock, and awaiting anything
/// here would let the runtime park and auto-advance virtual time — the same
/// reason `wait_for_grandchild_pid` in that file is deliberately not async.
/// (Spec D4: `start_paused` was measured to make these probes time out in
/// 187–408 µs deterministically, which is worse than the bug.)
///
/// All three streams are `null` so a fixture can neither block on a pipe
/// nobody drains nor leak a line into the test harness's captured output.
///
/// The result is dropped on purpose. This is an optimization: a fixture that
/// cannot be spawned at all is something the test itself is about to report
/// far more usefully than a panic from in here would.
pub(crate) fn warm_up(path: &Path) {
    use std::process::{Command, Stdio};

    let _ = Command::new(path)
        .env(WARMUP_ENV, "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/// Write `body` as an executable fixture at `path` and warm it.
///
/// The order is load-bearing: the warm-up can only pay the first-execution
/// cost once the file is actually executable, so it must follow the mode
/// change rather than the write.
pub(crate) fn write_exec_fixture(path: &Path, body: &str) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(path, fixture_script(body)).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    warm_up(path);
}
