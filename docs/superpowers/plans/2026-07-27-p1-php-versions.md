<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# PHP Version Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a developer keep several PHP versions on the machine, install another from inside OpenVHost, and point each site at the one it needs — including on a machine that has never installed PHP, or Homebrew.

**Architecture:** A one-shot task runner in `openvhost-proc` runs a command to completion and streams its output; `openvhost-core::php` knows about Homebrew (where versions live, which ones we offer, how to compose the install command) but executes nothing; the desktop app joins the two, rescans, and registers a `php-fpm-<major>` service row for anything new.

**Tech Stack:** Rust 2021 (tokio, thiserror), Tauri 2 + tauri-specta, SvelteKit + Svelte 5 runes, vitest.

**Source spec:** `docs/superpowers/specs/2026-07-27-p1-php-versions-design.md`

## Global Constraints

- Every new source file starts with `// SPDX-License-Identifier: GPL-3.0-or-later` (`<!-- ... -->` for `.svelte`).
- Commits are DCO-signed (`git commit -s`) and use Conventional Commits.
- No `unwrap()`/`expect()` outside `#[cfg(test)]`. The workspace denies `clippy::unwrap_used`/`expect_used` under `-D warnings`, and **clippy compiles the lib without `cfg(test)`** — an import used only by tests must be `#[cfg(test)]`-gated.
- `openvhost-core` must never depend on `tauri`. `openvhost-proc` must stay tauri-free too.
- Every child process goes through `openvhost-proc`. Nothing is resolved through `PATH` — not `php-fpm`, not `brew`.
- The catalogue of offered versions is `["8.1", "8.2", "8.3", "8.4", "8.5"]`. The install formula is composed as `format!("php@{major}")`, never taken from the caller.
- Tauri DTOs must not expose `usize`/`isize` — specta rejects them. Use `u32`.
- Gate before every commit: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`, plus `pnpm -C apps/desktop test`, `pnpm -C apps/desktop lint` and `pnpm -C apps/desktop exec svelte-check` for frontend tasks.
- In a fresh worktree run `pnpm install --offline --frozen-lockfile` in `apps/desktop` first, or the frontend gate fails with a bogus "Cannot find package".
- Task 5 adds IPC commands that cause the app to execute an external program, so the branch is **merge-blocked pending a security-auditor APPROVE** (CLAUDE.md golden rule 2).

## Correction to the spec, found while planning

Spec §3.3 says the two copies of `find_brew_binaries` collapse into one source. They cannot: one lives in `openvhost-conf` and one in `openvhost-core`, and **`openvhost-core` depends on `openvhost-conf`, not the reverse** — so a single shared definition would have to live in `openvhost-conf`, which has no business knowing about Homebrew layout beyond its own validator.

What this plan does instead: `openvhost-core::php` becomes the single source of brew-prefix knowledge *for core*, `demo_stack`'s copy delegates to it, and `openvhost-conf`'s copy gets a comment stating why it is separate and what it is for. Two definitions with a stated reason beat a merge that inverts the dependency graph.

## File Structure

**`crates/openvhost-proc`**
- `src/task.rs` — create: `run_task`, `TaskEvent`, `Stream`, the kill-on-drop guard.
- `src/lib.rs` — modify: `pub mod task;` and re-exports.

**`crates/openvhost-core`**
- `src/php/mod.rs` — create: module root, re-exports, `PhpMajor`.
- `src/php/discover.rs` — create: prefix walking, dedup, `discover_php`.
- `src/php/brew.rs` — create: brew location, `CATALOGUE`, `brew_install_spec`.
- `src/lib.rs` — modify: `pub mod php;` and re-exports.
- `src/platform/macos/demo_stack.rs` — modify: delegate prefix knowledge to `php::brew`.

**`apps/desktop`**
- `src-tauri/src/lib.rs` — modify: manage `RwLock<Option<InstalledRuntimes>>`, register the new commands.
- `src-tauri/src/commands.rs` — modify: take the read lock everywhere runtimes are read; add `list_php_runtimes`, `install_php`, their DTOs and the log event.
- `src/lib/ipc/bindings.ts` — regenerated, committed.
- `src/lib/ipc/index.ts` — modify: wrappers and type re-exports.
- `src/lib/languages.svelte.ts` (+ `.test.ts`) — create: the store.
- `src/lib/components/LanguageRow.svelte` (+ `.test.ts`) — create.
- `src/routes/languages/+page.svelte` — create.
- `src/lib/components/Rail.svelte` — modify: a Languages nav item.
- `src/lib/sites.derive.ts` (+ `.test.ts`) — modify: `phpVersionOptions` takes the installed list.
- `src/lib/components/SiteDrawer.svelte` — modify: pass the installed list through.

---

## Task 1: A one-shot task runner in openvhost-proc

The supervisor models long-lived services with a state machine; `brew install` runs once, prints a lot, and exits. Reaching `Stopped` is success here, so a supervised entry would render a clean `exit 0` as `Stopped` or `Failed` in the Services panel. What this borrows from the supervisor is the part that matters: its own process group, killed as a group on drop.

**Files:**
- Create: `crates/openvhost-proc/src/task.rs`
- Modify: `crates/openvhost-proc/src/lib.rs`
- Test: `crates/openvhost-proc/src/task.rs` (`#[cfg(test)] mod tests`) and `crates/openvhost-proc/tests/task_group.rs`

**Interfaces:**
- Consumes: the crate's existing `SpawnSpec { program, args, cwd, env }`, `ProcessDriver { spawn, request_graceful_stop, kill }`, `SpawnedChild { take_stdout, take_stderr, wait }`, `default_driver()`.
- Produces:
  ```rust
  #[derive(Debug, Clone, Copy, PartialEq, Eq)]
  pub enum Stream { Stdout, Stderr }

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub enum TaskEvent {
      Line { stream: Stream, text: String },
      Finished { code: Option<i32> },
  }

  /// Runs `spec` to completion, sending every output line then a final
  /// `Finished`. `Ok(None)` means the process was killed by a signal.
  pub async fn run_task(
      driver: std::sync::Arc<dyn ProcessDriver>,
      spec: SpawnSpec,
      tx: tokio::sync::mpsc::Sender<TaskEvent>,
  ) -> Result<Option<i32>, ProcError>;
  ```
  `ProcError` is only for failing to *run* the program (missing binary, spawn refused). A program that runs and exits non-zero is `Ok(Some(code))` — "brew said no" is an outcome to render, not a runner error.

- [ ] **Step 1: Write the failing unit tests**

Create `crates/openvhost-proc/src/task.rs` with only a test module for now. `testchild_spec` mirrors how `tests/supervisor.rs` builds a spec for the `proc_testchild` helper binary — read that file first and match how it locates the binary.

```rust
#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::ffi::OsString;

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
            crate::default_driver(),
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
        assert!(lines[0] < lines[1] && lines[1] < lines[2], "out of order: {lines:?}");
        assert!(matches!(events.last(), Some(TaskEvent::Finished { code: Some(0) })));
    }

    #[tokio::test]
    async fn a_non_zero_exit_is_an_outcome_not_an_error() {
        // "brew said no" must reach the caller as data it can render, not as
        // a ProcError that looks like the runner itself broke.
        let (tx, _rx) = tokio::sync::mpsc::channel(64);
        let code = run_task(
            crate::default_driver(),
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
        assert!(run_task(crate::default_driver(), spec, tx).await.is_err());
    }

    #[tokio::test]
    async fn stderr_lines_are_tagged_as_stderr() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        // proc_testchild writes its "fail" diagnostics to stderr.
        let _ = run_task(
            crate::default_driver(),
            testchild_spec(&["--lines", "2", "--fail-after", "1"]),
            tx,
        )
        .await;
        let events = collect(&mut rx);
        assert!(
            events.iter().any(|e| matches!(e, TaskEvent::Line { stream: Stream::Stderr, .. })),
            "no stderr line was tagged: {events:?}"
        );
    }
}
```

Read `crates/openvhost-proc/src/testchild.rs` to confirm which flag makes it write to stderr, and adjust the last test's arguments to match what that binary actually does — do not assume `--fail-after` is the one.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-proc task`
Expected: FAIL — `run_task` does not exist.

- [ ] **Step 3: Implement**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! One-shot task runner: run a command to completion, streaming its output.
//!
//! Deliberately NOT a `Supervisor` entry. A supervised service has a state
//! machine where `Stopped` means something went away; a task that exits 0 has
//! simply finished, and rendering that as `Stopped`/`Failed` in the Services
//! panel would be worse than useless.
//!
//! What it does borrow is the containment: the child gets its own process
//! group (via the same `ProcessDriver` the supervisor uses) and the group is
//! killed if this future is dropped. `brew install` forks a tree — curl, tar,
//! ruby, sometimes a compiler — and abandoning that tree when the app quits
//! mid-install is exactly the orphan problem P0-8 closed.
//!
//! There is deliberately NO timeout: a twenty-minute compile is normal, so a
//! clock could only ever fire on legitimate work. Cancellation is expressed by
//! dropping the future.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::mpsc::Sender;

use crate::error::ProcError;
use crate::platform::{ProcessDriver, SpawnSpec, SpawnedChild};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskEvent {
    Line { stream: Stream, text: String },
    Finished { code: Option<i32> },
}

/// Kills the whole process group if the run is abandoned. `Drop` cannot be
/// async, which is why `ProcessDriver::kill` is a synchronous call.
struct KillOnDrop {
    driver: Arc<dyn ProcessDriver>,
    child: SpawnedChild,
    finished: bool,
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if !self.finished {
            let _ = self.driver.kill(&mut self.child);
        }
    }
}

pub async fn run_task(
    driver: Arc<dyn ProcessDriver>,
    spec: SpawnSpec,
    tx: Sender<TaskEvent>,
) -> Result<Option<i32>, ProcError> {
    let mut child = driver.spawn(&spec).map_err(|source| ProcError::Spawn {
        program: spec.program.display().to_string(),
        source,
    })?;

    let stdout = child.take_stdout();
    let stderr = child.take_stderr();

    let mut guard = KillOnDrop {
        driver,
        child,
        finished: false,
    };

    let pump = |maybe, stream, tx: Sender<TaskEvent>| async move {
        let Some(s) = maybe else { return };
        let mut lines = BufReader::new(s).lines();
        while let Ok(Some(text)) = lines.next_line().await {
            if tx.send(TaskEvent::Line { stream, text }).await.is_err() {
                return; // receiver gone: stop reading, let the guard clean up
            }
        }
    };

    let out = tokio::spawn(pump(stdout, Stream::Stdout, tx.clone()));
    let err = tokio::spawn(pump(stderr, Stream::Stderr, tx.clone()));

    let status = guard.child.wait().await.map_err(|source| ProcError::Spawn {
        program: spec.program.display().to_string(),
        source,
    })?;
    guard.finished = true;

    // Await the readers AFTER wait() so no line is dropped on the floor.
    let _ = out.await;
    let _ = err.await;

    let code = status.code();
    let _ = tx.send(TaskEvent::Finished { code }).await;
    Ok(code)
}
```

`ProcError`'s variants may not match the `Spawn { program, source }` shape used above — read `crates/openvhost-proc/src/error.rs` and use the variant that already exists for a failed spawn, or add one following the file's conventions. Do not invent a second error type.

Export from `crates/openvhost-proc/src/lib.rs`:

```rust
pub mod task;
pub use task::{Stream as TaskStream, TaskEvent, run_task};
```

`Stream` is re-exported as `TaskStream` because `openvhost-proc` already exports `StreamSource` from `events`, and two similarly-named types in one namespace is how call sites end up importing the wrong one.

- [ ] **Step 4: Run the unit tests**

Run: `cargo test -p openvhost-proc task`
Expected: PASS — 4 tests.

- [ ] **Step 5: Write the containment test**

Create `crates/openvhost-proc/tests/task_group.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Dropping a run must kill the child's whole process group — the P0-8
//! invariant, restated for the one-shot runner.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::time::Duration;

#[tokio::test]
async fn dropping_the_run_kills_a_child_that_ignores_a_polite_stop() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    // --ignore-stop: only a group kill ends this. If the runner merely dropped
    // its handle, the process would outlive the test and hold its pipes open.
    let spec = common::testchild_spec(&["--lines", "1000", "--interval-ms", "50", "--ignore-stop"]);
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
    assert!(closed.is_ok(), "the abandoned child was still alive and writing");
}
```

`tests/common/mod.rs` already exists in this crate (used by `tests/e2e.rs`). If it has no `testchild_spec`, add one there rather than duplicating the binary-path logic per test file, and adjust the existing tests' imports if needed.

- [ ] **Step 6: Run it**

Run: `cargo test -p openvhost-proc --test task_group -- --nocapture`
Expected: PASS. If it hangs, the group kill is not happening — fix the guard, never the test's timeout.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/openvhost-proc
git commit -s -m "feat(proc): run one-shot tasks with streamed output

A command that runs once and exits is not a supervised service: reaching
Stopped is success, not something to report. It keeps the supervisor's
containment — its own process group, killed as a group if the run is
abandoned — because brew forks a tree that must not outlive the app."
```

---

## Task 2: Discover the installed PHP versions

**Files:**
- Create: `crates/openvhost-core/src/php/mod.rs`
- Create: `crates/openvhost-core/src/php/discover.rs`
- Modify: `crates/openvhost-core/src/lib.rs`
- Test: `crates/openvhost-core/src/php/discover.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `PhpRuntime { major: String, fpm_bin: PathBuf }` — already defined in `openvhost-core::site::apply` and consumed by `render_set`. **Reuse it; do not define a second runtime type.**
- Produces:
  ```rust
  /// Homebrew prefixes, most-likely first. Apple Silicon, then Intel.
  pub const BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

  /// Every PHP runtime found under `prefixes`, deduplicated by major and
  /// sorted by major. `probe` returns a runtime's `major.minor`, or `None`
  /// when the binary is not a usable php-fpm.
  pub fn discover_php_in(
      prefixes: &[&std::path::Path],
      probe: &dyn Fn(&std::path::Path) -> Option<String>,
  ) -> Vec<PhpRuntime>;
  ```

- [ ] **Step 1: Write the failing tests**

Create `crates/openvhost-core/src/php/discover.rs` with the test module:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use std::path::{Path, PathBuf};

    /// Build a fake brew prefix: `opt/<formula>/sbin/php-fpm` for each entry,
    /// mapping the created binary path to the version the probe should report.
    fn fake_prefix(formulae: &[(&str, &str)]) -> (tempfile::TempDir, BTreeMap<PathBuf, String>) {
        let dir = tempfile::tempdir().unwrap();
        let mut versions = BTreeMap::new();
        for (formula, version) in formulae {
            let bin = dir.path().join("opt").join(formula).join("sbin/php-fpm");
            std::fs::create_dir_all(bin.parent().unwrap()).unwrap();
            std::fs::write(&bin, b"#!/bin/sh\n").unwrap();
            versions.insert(bin, (*version).to_string());
        }
        (dir, versions)
    }

    fn probe_from(map: BTreeMap<PathBuf, String>) -> impl Fn(&Path) -> Option<String> {
        move |p: &Path| map.get(p).cloned()
    }

    #[test]
    fn finds_a_versioned_formula() {
        let (dir, versions) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].major, "8.3");
        assert!(found[0].fpm_bin.ends_with("opt/php@8.3/sbin/php-fpm"));
    }

    #[test]
    fn the_unversioned_alias_does_not_double_count_its_own_version() {
        // On a real machine /opt/homebrew/opt/php and /opt/homebrew/opt/php@8.5
        // both resolve to the same Cellar directory — the unversioned formula
        // is an alias for the current one. Two entries would mean two service
        // rows and two pools listening on two sockets for one binary.
        let (dir, versions) = fake_prefix(&[("php", "8.5"), ("php@8.5", "8.5")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        assert_eq!(found.len(), 1, "got {found:?}");
        assert_eq!(found[0].major, "8.5");
        // The versioned path is the stable one: `php` moves when brew upgrades it.
        assert!(
            found[0].fpm_bin.to_string_lossy().contains("php@8.5"),
            "the versioned path should win: {:?}",
            found[0].fpm_bin
        );
    }

    #[test]
    fn several_versions_come_back_sorted_and_distinct() {
        let (dir, versions) = fake_prefix(&[("php@8.4", "8.4"), ("php@8.1", "8.1"), ("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &probe_from(versions));
        let majors: Vec<&str> = found.iter().map(|r| r.major.as_str()).collect();
        assert_eq!(majors, vec!["8.1", "8.3", "8.4"]);
    }

    #[test]
    fn a_prefix_that_does_not_exist_is_not_an_error() {
        let found = discover_php_in(&[Path::new("/nonexistent/openvhost-prefix")], &|_| None);
        assert!(found.is_empty());
    }

    #[test]
    fn a_formula_whose_binary_is_not_php_fpm_is_skipped() {
        // The probe is what decides. A directory that looks right but holds
        // something else must not become a runtime.
        let (dir, _) = fake_prefix(&[("php@8.3", "8.3")]);
        let found = discover_php_in(&[dir.path()], &|_| None);
        assert!(found.is_empty(), "got {found:?}");
    }

    #[test]
    fn an_earlier_prefix_wins_over_a_later_one() {
        // Apple Silicon before Intel: a machine with both must not report the
        // same major twice.
        let (a, va) = fake_prefix(&[("php@8.3", "8.3")]);
        let (b, vb) = fake_prefix(&[("php@8.3", "8.3")]);
        let mut merged = va.clone();
        merged.extend(vb);
        let found = discover_php_in(&[a.path(), b.path()], &probe_from(merged));
        assert_eq!(found.len(), 1);
        assert!(found[0].fpm_bin.starts_with(a.path()));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core php::discover`
Expected: FAIL — `discover_php_in` not found.

- [ ] **Step 3: Implement**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Find the PHP runtimes installed on this machine.
//!
//! Never resolves anything through `PATH`: a ServBay install shadows
//! `php-fpm` there, which is why the existing probe code walks known prefixes
//! instead. The same rule applies here.

use std::path::{Path, PathBuf};

use crate::site::apply::PhpRuntime;

/// Homebrew prefixes, most-likely first: Apple Silicon, then Intel.
pub const BREW_PREFIXES: [&str; 2] = ["/opt/homebrew", "/usr/local"];

/// A formula directory holds a runtime when this file exists under it.
const FPM_REL: &str = "sbin/php-fpm";

/// Directory entries under `<prefix>/opt` that could be a PHP formula:
/// `php` (the alias for the current version) and `php@<major>`.
fn is_php_formula(name: &str) -> bool {
    name == "php" || name.starts_with("php@")
}

pub fn discover_php_in(
    prefixes: &[&Path],
    probe: &dyn Fn(&Path) -> Option<String>,
) -> Vec<PhpRuntime> {
    let mut found: Vec<PhpRuntime> = Vec::new();

    for prefix in prefixes {
        let opt = prefix.join("opt");
        let Ok(entries) = std::fs::read_dir(&opt) else {
            continue; // a prefix that is not installed is not an error
        };
        // Sorted so a machine with both `php` and `php@8.5` is deterministic:
        // `php@8.5` sorts after `php`, and the versioned path is preferred
        // below, so ordering here only has to be stable.
        let mut candidates: Vec<PathBuf> = entries
            .flatten()
            .filter(|e| e.file_name().to_str().is_some_and(is_php_formula))
            .map(|e| e.path())
            .collect();
        candidates.sort();

        for dir in candidates {
            let bin = dir.join(FPM_REL);
            if !bin.is_file() {
                continue;
            }
            let Some(major) = probe(&bin) else {
                continue;
            };
            match found.iter_mut().find(|r| r.major == major) {
                // Already known. Prefer the versioned path: `php` is an alias
                // that moves the day brew upgrades the current formula, while
                // `php@8.5` keeps pointing at 8.5.
                Some(existing) => {
                    let existing_is_alias = existing
                        .fpm_bin
                        .parent()
                        .and_then(|p| p.parent())
                        .and_then(|p| p.file_name())
                        .is_some_and(|n| n == "php");
                    if existing_is_alias {
                        existing.fpm_bin = bin;
                    }
                }
                None => found.push(PhpRuntime { major, fpm_bin: bin }),
            }
        }
    }

    found.sort_by(|a, b| a.major.cmp(&b.major));
    found
}
```

Create `crates/openvhost-core/src/php/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! PHP runtimes: which are installed, and how to install another.

mod discover;

pub use discover::{BREW_PREFIXES, discover_php_in};
```

Add `pub mod php;` to `crates/openvhost-core/src/lib.rs` and re-export `discover_php_in` and `BREW_PREFIXES` next to the existing `site::apply` re-exports.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p openvhost-core php`
Expected: PASS — 6 tests.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
git add crates/openvhost-core
git commit -s -m "feat(core): discover the PHP runtimes installed via Homebrew

Deduplicated by major, because /opt/homebrew/opt/php is an alias for the
current formula and would otherwise produce a second service row and a
second pool for one binary."
```

---

## Task 3: The catalogue, the allowlist, and the install command

This is the security core of the slice. Arguments are passed as an argv vector, never through a shell, so *command* injection is not the risk — *flag* injection is. Without the catalogue check a value like `--build-from-source`, `--HEAD` or an unrelated formula name flows straight into `brew install`.

**Files:**
- Create: `crates/openvhost-core/src/php/brew.rs`
- Modify: `crates/openvhost-core/src/php/mod.rs`
- Modify: `crates/openvhost-core/src/lib.rs`
- Modify: `crates/openvhost-core/src/platform/macos/demo_stack.rs`
- Test: `crates/openvhost-core/src/php/brew.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `openvhost_proc::SpawnSpec { program, args, cwd, env }`, `BREW_PREFIXES` from Task 2.
- Produces:
  ```rust
  pub const CATALOGUE: [&str; 5] = ["8.1", "8.2", "8.3", "8.4", "8.5"];

  /// A PHP `major.minor` this build offers. Parsing enforces the shape;
  /// membership of CATALOGUE enforces the policy.
  pub struct PhpMajor(String);
  impl PhpMajor {
      pub fn parse(s: &str) -> Result<Self, CoreError>;
      pub fn as_str(&self) -> &str;
  }

  pub fn find_brew() -> Option<std::path::PathBuf>;

  /// The command that installs `major`. Composed here — the formula name is
  /// never accepted from a caller.
  /// Rejects a non-absolute `brew` path: composing PATH from a relative one
  /// yields an empty leading component, which exec resolves as the CWD.
  pub fn brew_install_spec(brew: &std::path::Path, major: &PhpMajor)
      -> Result<openvhost_proc::SpawnSpec, CoreError>;
  ```
  `openvhost-core` gains a dependency on `openvhost-proc` (it already has one as a dev-dependency; this promotes it to a normal one). `openvhost-proc` depends on neither core nor tauri, so no cycle.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_version_this_build_offers() {
        assert_eq!(PhpMajor::parse("8.3").unwrap().as_str(), "8.3");
    }

    #[test]
    fn rejects_anything_that_is_not_major_dot_minor() {
        // Shape guard. Every one of these would otherwise become an argv entry.
        for bad in ["", "8", "8.", ".3", "8.3.1", "eight.three", " 8.3", "8.3 ", "8_3"] {
            assert!(PhpMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_a_flag_even_though_argv_prevents_command_injection() {
        // argv stops `; rm -rf` but NOT `--build-from-source`, which brew would
        // happily honour. This is the reason the allowlist exists.
        for bad in ["--build-from-source", "--HEAD", "-f", "--cask", "nginx"] {
            assert!(PhpMajor::parse(bad).is_err(), "accepted {bad:?}");
        }
    }

    #[test]
    fn rejects_a_well_formed_version_this_build_does_not_offer() {
        // Shape alone is not enough: policy is the second layer.
        assert!(PhpMajor::parse("9.9").is_err());
        assert!(PhpMajor::parse("7.4").is_err());
    }

    #[test]
    fn the_install_command_is_exactly_install_and_the_formula() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        );
        assert_eq!(spec.program, std::path::PathBuf::from("/opt/homebrew/bin/brew"));
        let args: Vec<String> = spec
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        // Pinned exactly. This test fails the moment anyone adds a flag —
        // which is both a security property and a no-surprises property.
        assert_eq!(args, vec!["install".to_string(), "php@8.3".to_string()]);
    }

    #[test]
    fn the_install_command_disables_homebrews_own_auto_update() {
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        );
        let env: Vec<(String, String)> = spec
            .env
            .iter()
            .map(|(k, v)| (k.to_string_lossy().into_owned(), v.to_string_lossy().into_owned()))
            .collect();
        assert!(
            env.contains(&("HOMEBREW_NO_AUTO_UPDATE".to_string(), "1".to_string())),
            "got {env:?}"
        );
    }

    #[test]
    fn the_install_command_puts_brews_own_bin_on_path() {
        // The app launched from Finder has a minimal PATH. brew shells out to
        // git and curl, so its own prefix has to be reachable or the install
        // fails only in a bundled build and not in `tauri dev`.
        let spec = brew_install_spec(
            std::path::Path::new("/opt/homebrew/bin/brew"),
            &PhpMajor::parse("8.3").unwrap(),
        );
        let path = spec
            .env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.to_string_lossy().into_owned())
            .expect("PATH must be set explicitly");
        assert!(path.starts_with("/opt/homebrew/bin"), "got {path}");
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-core php::brew`
Expected: FAIL — module does not exist.

- [ ] **Step 3: Implement**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Homebrew as a PHP source: which versions we offer, where brew lives, and
//! the exact command that installs one.
//!
//! SECURITY: this module composes the argv. A caller supplies a version, never
//! a formula and never a flag. Arguments are passed as a vector rather than
//! through a shell, which stops command injection — but not flag injection, so
//! `PhpMajor::parse` enforces the shape AND membership of [`CATALOGUE`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use openvhost_proc::SpawnSpec;

use crate::error::CoreError;
use super::BREW_PREFIXES;

/// The versions this build offers. Hand-maintained: asking `brew` would mean
/// spawning a process on a path that has to stay cheap, and a stale entry
/// fails loudly at install time rather than silently.
pub const CATALOGUE: [&str; 5] = ["8.1", "8.2", "8.3", "8.4", "8.5"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhpMajor(String);

impl PhpMajor {
    pub fn parse(s: &str) -> Result<Self, CoreError> {
        // Layer 1: shape. Digits, one dot, digits — nothing else.
        let mut parts = s.split('.');
        let ok = match (parts.next(), parts.next(), parts.next()) {
            (Some(a), Some(b), None) => {
                !a.is_empty()
                    && !b.is_empty()
                    && a.bytes().all(|c| c.is_ascii_digit())
                    && b.bytes().all(|c| c.is_ascii_digit())
            }
            _ => false,
        };
        if !ok {
            return Err(CoreError::Validation {
                field: "php_version",
                reason: format!("{s:?} is not a major.minor version"),
            });
        }
        // Layer 2: policy. Shape alone would still let a flag-shaped-but-numeric
        // value, or a version we have never tested, reach `brew install`.
        if !CATALOGUE.contains(&s) {
            return Err(CoreError::Validation {
                field: "php_version",
                reason: format!("PHP {s} is not offered by this build (offered: {})", CATALOGUE.join(", ")),
            });
        }
        Ok(Self(s.to_string()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Locate `brew` by absolute path. NEVER via `PATH` — the same rule the
/// php-fpm and nginx probes follow, for the same reason.
pub fn find_brew() -> Option<PathBuf> {
    BREW_PREFIXES
        .iter()
        .map(|p| Path::new(p).join("bin/brew"))
        .find(|p| p.is_file())
}

pub fn brew_install_spec(brew: &Path, major: &PhpMajor) -> SpawnSpec {
    // brew shells out to git, curl and friends. The supervisor's env
    // allow-list forwards the parent's PATH, which for an app launched from
    // Finder is the bare system one — so brew's own prefix is prepended
    // explicitly rather than hoping the launch context had it.
    let brew_bin = brew.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut path = OsString::from(brew_bin);
    if let Some(inherited) = std::env::var_os("PATH") {
        path.push(":");
        path.push(inherited);
    }

    SpawnSpec {
        program: brew.to_path_buf(),
        args: vec![
            OsString::from("install"),
            OsString::from(format!("php@{}", major.as_str())),
        ],
        cwd: None,
        env: vec![
            // Without this, pressing Install can spend five minutes updating
            // Homebrew itself before starting the work the user asked for.
            (OsString::from("HOMEBREW_NO_AUTO_UPDATE"), OsString::from("1")),
            (OsString::from("PATH"), path),
        ],
    }
}
```

Add `mod brew;` plus `pub use brew::{CATALOGUE, PhpMajor, brew_install_spec, find_brew};` to `crates/openvhost-core/src/php/mod.rs`, and re-export the same four at the **crate root** in `crates/openvhost-core/src/lib.rs` — Task 5's tests and its live brew test reach them as `openvhost_core::CATALOGUE`, `openvhost_core::PhpMajor` and `openvhost_core::find_brew`.

Promote `openvhost-proc` from `[dev-dependencies]` to `[dependencies]` in `crates/openvhost-core/Cargo.toml`, removing the duplicate dev entry.

- [ ] **Step 4: Settle the duplicate prefix knowledge**

`crates/openvhost-core/src/platform/macos/demo_stack.rs` hardcodes `/opt/homebrew` and `/usr/local` in its own `find_brew_binaries`. Change it to iterate `crate::php::BREW_PREFIXES` so core has one list.

Leave `openvhost-conf::validate::find_brew_binaries` alone, and add a comment there stating why it is separate: `openvhost-core` depends on `openvhost-conf`, so a shared definition would have to live in conf, which has no business knowing Homebrew's layout beyond its own validator. Two definitions with a stated reason beat inverting the dependency graph.

- [ ] **Step 5: Run the tests**

Run: `cargo test -p openvhost-core php`
Expected: PASS — Task 2's 6 plus 7 new.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
cargo deny check licenses
git add crates/openvhost-core crates/openvhost-conf
git commit -s -m "feat(core): compose the brew install command behind an allowlist

argv stops command injection but not flag injection: without the
catalogue check, --build-from-source or another formula name would flow
straight into brew install. The composed argv is pinned by a test."
```

---


> **Tasks 1-3 are merged** (PR #26, `e0816d0`). The tasks below were restructured on
> 2026-07-27 after the owner hit a real dead end on their own machine (spec §5.0) and after
> reviewing ServBay's equivalent page (spec §6). Task 6 grew, and two tasks are new: the
> zero-state/degradation path, and the Sites-side recovery affordances.

## Task 4: Make the installed-runtime set replaceable

`InstalledRuntimes` is managed state set once at startup, and Tauri cannot replace managed
state. Until it can be updated, a version installed after launch is invisible to the apply
pipeline — the Languages page would appear to work while changing nothing.

**Files:**
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Test: `apps/desktop/src-tauri/src/commands.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: the managed type becomes `std::sync::RwLock<Option<InstalledRuntimes>>`. Every reader takes the read lock; Task 5 takes the write lock after an install or a rescan.

- [ ] **Step 1: Find every reader**

Run: `grep -rn "InstalledRuntimes" apps/desktop/src-tauri/src`
There are readers in `apply_input`, `apply_sites` and the setup in `lib.rs`. Read each before changing it — that list is a starting point, not an inventory.

- [ ] **Step 2: Change the managed type**

In `apps/desktop/src-tauri/src/lib.rs`:

```rust
app.manage(std::sync::RwLock::new(stack_runtimes));
```

In `commands.rs`, every `tauri::State<'_, Option<InstalledRuntimes>>` becomes
`tauri::State<'_, std::sync::RwLock<Option<InstalledRuntimes>>>`, and each reader clones what it needs out of the guard rather than holding the lock across an `.await`:

```rust
let runtimes = runtimes
    .read()
    .map_err(|_| IpcError::Core { message: "runtime list is poisoned".into() })?
    .clone();
```

Holding a `std::sync::RwLockReadGuard` across an await point makes the future non-`Send`, which fails to compile as a Tauri command — clone first, then await.

- [ ] **Step 3: Add the test**

```rust
#[test]
fn the_runtime_set_can_be_replaced_after_startup() {
    // The Languages page installs a version at runtime; if this state could not
    // be replaced, apply would never learn about it and Install would appear
    // to succeed while changing nothing.
    let state = std::sync::RwLock::new(None::<InstalledRuntimes>);
    assert!(state.read().unwrap().is_none());
    *state.write().unwrap() = Some(InstalledRuntimes {
        nginx_bin: std::path::PathBuf::from("/opt/homebrew/opt/nginx/bin/nginx"),
        php: vec![openvhost_core::PhpRuntime {
            major: "8.3".into(),
            fpm_bin: std::path::PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
        }],
    });
    let seen = state.read().unwrap().clone().unwrap();
    assert_eq!(seen.php.len(), 1);
    assert_eq!(seen.php[0].major, "8.3");
}
```

- [ ] **Step 4: Run everything**

```bash
cargo test -p openvhost-desktop
cargo test --workspace
```
Expected: PASS. The existing apply tests must be unchanged in behaviour — if any needed more than the lock, say so in your report.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
git add apps/desktop/src-tauri
git commit -s -m "refactor(desktop): make the installed-runtime set replaceable

Tauri cannot replace managed state, so a version installed after launch
was invisible to apply. Now behind an RwLock, ready for a rescan."
```

---

## Task 5: The IPC surface — list, install, rescan, register

**Merge-blocked: this task adds commands that cause the app to execute an external program, so the branch needs a security-auditor APPROVE before merge (golden rule 2).**

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs`
- Modify: `apps/desktop/src-tauri/src/lib.rs`
- Modify: `apps/desktop/src/lib/ipc/bindings.ts` (regenerated)
- Modify: `apps/desktop/src/lib/ipc/index.ts`
- Test: `apps/desktop/src-tauri/src/commands.rs`, `apps/desktop/src/lib/ipc/ipc.test.ts`

**Interfaces:**
- Consumes: `run_task`, `TaskEvent`, `TaskStream`; `discover_php_in`, `BREW_PREFIXES`, `CATALOGUE`, `PhpMajor`, `find_brew`, `brew_install_spec` (note: **returns `Result<SpawnSpec, CoreError>`** — it refuses a non-absolute brew path); the `RwLock` from Task 4; `openvhost_conf::probe_php_fpm_version`; `Supervisor::register`. If `stack.rs` builds its php-fpm `ServiceSpec` inline, extract that into a named `php_fpm_spec(home, runtime)` so startup and this task build the row the same way rather than twice.
- Produces:
  ```rust
  pub struct PhpRuntimeDto {
      pub major: String,
      pub installed: bool,
      pub recommended: bool,
      pub full_version: Option<String>,
      pub path: Option<String>,
      /// Where this version's pool listens. `None` until installed.
      pub socket_path: Option<String>,
      /// The supervisor id for this version's pool, so the UI can drive
      /// start/stop from the row without inventing the id itself.
      pub service_id: Option<String>,
  }

  /// What the Languages page needs to decide which of the three states to show
  /// (spec §6.1). `brew_found` false means the page must guide, not list.
  pub struct PhpEnvironmentDto {
      pub brew_found: bool,
      pub brew_searched: Vec<String>,
      pub runtimes: Vec<PhpRuntimeDto>,
  }

  pub struct InstallOutcomeDto { pub major: String, pub exit_code: Option<i32>, pub detected: bool }

  #[tauri::command] pub async fn php_environment(...) -> Result<PhpEnvironmentDto, IpcError>;
  #[tauri::command] pub async fn rescan_php_runtimes(...) -> Result<PhpEnvironmentDto, IpcError>;
  #[tauri::command] pub async fn install_php(major: String, ...) -> Result<InstallOutcomeDto, IpcError>;
  ```
  Plus `PhpInstallLogEvent { major, ts_ms, stream, line }` per output line, following the existing `ServiceLogEvent` pattern.
  TS: `phpEnvironment()`, `rescanPhpRuntimes()`, `installPhp(major)`, `PhpRuntimeDto.fullVersion`, `.socketPath`, `.serviceId`, `PhpEnvironmentDto.brewFound`, `.brewSearched`.

**Why `php_environment` rather than `list_php_runtimes`:** the page has to distinguish "no PHP" from "no Homebrew", and those are different states with different remedies (spec §6.1). A bare `Vec<PhpRuntimeDto>` cannot express the second, and the page would have to infer it from an error string.

**Why a separate `rescan_php_runtimes`:** the user leaves to install Homebrew in a terminal and comes back. `php_environment` reads cached state and spawns nothing (the property that keeps `plan_site_apply` cheap); `rescan` is the explicit, user-initiated probe behind the **Check again** button.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn every_catalogue_entry_is_listed_with_its_installed_state() {
    let installed = vec![openvhost_core::PhpRuntime {
        major: "8.3".into(),
        fpm_bin: std::path::PathBuf::from("/opt/homebrew/opt/php@8.3/sbin/php-fpm"),
    }];
    let rows = php_rows(std::path::Path::new("/tmp/ovh"), &installed, &[("8.3", "8.3.14")]);
    assert_eq!(rows.len(), openvhost_core::CATALOGUE.len());
    let three = rows.iter().find(|r| r.major == "8.3").unwrap();
    assert!(three.installed);
    assert_eq!(three.full_version.as_deref(), Some("8.3.14"));
    assert_eq!(three.service_id.as_deref(), Some("php-fpm-8.3"));
    assert!(three.socket_path.as_deref().is_some_and(|s| s.ends_with("php-fpm-8.3.sock")));
    let one = rows.iter().find(|r| r.major == "8.1").unwrap();
    assert!(!one.installed);
    assert!(one.path.is_none());
    assert!(one.service_id.is_none(), "a version that is not installed has no pool");
}

#[test]
fn exactly_one_catalogue_entry_is_recommended_and_it_is_the_newest() {
    // A first-time user should not have to know how 8.1 differs from 8.5.
    let rows = php_rows(std::path::Path::new("/tmp/ovh"), &[], &[]);
    let rec: Vec<&str> = rows.iter().filter(|r| r.recommended).map(|r| r.major.as_str()).collect();
    assert_eq!(rec, vec![*openvhost_core::CATALOGUE.last().unwrap()]);
}

#[test]
fn an_installed_version_outside_the_catalogue_is_still_listed() {
    // Otherwise a version installed by hand — or dropped from a later
    // catalogue — vanishes from the page while still serving sites.
    let installed = vec![openvhost_core::PhpRuntime {
        major: "7.4".into(),
        fpm_bin: std::path::PathBuf::from("/opt/homebrew/opt/php@7.4/sbin/php-fpm"),
    }];
    let rows = php_rows(std::path::Path::new("/tmp/ovh"), &installed, &[("7.4", "7.4.33")]);
    assert!(rows.iter().any(|r| r.major == "7.4" && r.installed));
}

#[test]
fn a_rejected_version_names_the_field_so_the_ui_can_mark_it() {
    let e: IpcError = openvhost_core::PhpMajor::parse("--build-from-source")
        .unwrap_err()
        .into();
    match e {
        IpcError::Validation { field, .. } => assert_eq!(field, "php_version"),
        other => panic!("expected Validation, got {other:?}"),
    }
}
```

`php_rows(home, installed, full_versions)` is a pure helper you extract so the row-building logic is testable without Tauri state.

In `ipc.test.ts`, add wrapper tests for `phpEnvironment`, `rescanPhpRuntimes` and `installPhp` following the existing shape.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p openvhost-desktop php_rows`
Expected: FAIL — `php_rows` not found.

- [ ] **Step 3: Implement the read commands**

`php_environment` reads the `RwLock` and `find_brew()`, probes nothing, and returns
`PhpEnvironmentDto { brew_found, brew_searched, runtimes: php_rows(...) }`. `brew_searched` is `BREW_PREFIXES` joined with `bin/brew` so the UI can name exactly where it looked. **This command must not spawn a process** — it is called on page mount and after every install, and the discipline that keeps `plan_site_apply` cheap applies here too.

`rescan_php_runtimes` does the probing, writes the result into the `RwLock`, registers a service row for any newly found major, and returns the same DTO.

- [ ] **Step 4: Implement `install_php`**

```rust
#[tauri::command]
#[specta::specta]
pub async fn install_php(
    app: tauri::AppHandle,
    major: String,
    runtimes: tauri::State<'_, std::sync::RwLock<Option<InstalledRuntimes>>>,
    paths: tauri::State<'_, Option<StackPaths>>,
    sup: tauri::State<'_, Arc<Supervisor>>,
    lock: tauri::State<'_, InstallLock>,
) -> Result<InstallOutcomeDto, IpcError> {
    // Both guard layers, before anything else happens.
    let major = openvhost_core::PhpMajor::parse(&major)?;

    // One at a time. `try_lock` rather than `lock`: a second press should be
    // refused with an explanation, not silently queued behind a 20-minute build.
    let Ok(_guard) = lock.0.try_lock() else {
        return Err(IpcError::Core { message: "an install is already running".into() });
    };

    let before: Vec<String> = runtimes
        .read()
        .map_err(|_| IpcError::Core { message: "runtime list is poisoned".into() })?
        .as_ref()
        .map(|r| r.php.iter().map(|p| p.major.clone()).collect())
        .unwrap_or_default();

    if before.iter().any(|m| m == major.as_str()) {
        return Err(IpcError::Core {
            message: format!("PHP {} is already installed", major.as_str()),
        });
    }

    let brew = openvhost_core::find_brew().ok_or_else(|| IpcError::Core {
        message: format!(
            "Homebrew was not found. Looked for bin/brew under: {}",
            openvhost_core::BREW_PREFIXES.join(", ")
        ),
    })?;

    // Returns Result: it refuses a non-absolute brew path, because composing
    // PATH from one yields an empty leading component and exec resolves that
    // as the working directory.
    let spec = openvhost_core::brew_install_spec(&brew, &major)?;
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);

    // Forward brew's output as it arrives, so a long install is visibly
    // working rather than apparently hung.
    let emitter = app.clone();
    let for_event = major.as_str().to_string();
    let pump = tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let openvhost_proc::TaskEvent::Line { stream, text } = ev {
                let _ = PhpInstallLogEvent {
                    major: for_event.clone(),
                    ts_ms: now_ms(),
                    stream: match stream {
                        openvhost_proc::TaskStream::Stdout => "stdout".into(),
                        openvhost_proc::TaskStream::Stderr => "stderr".into(),
                    },
                    line: text,
                }
                .emit(&emitter);
            }
        }
    });

    let exit_code = openvhost_proc::run_task(openvhost_proc::default_driver(), spec, tx).await?;
    let _ = pump.await;

    // Rescan even on a non-zero exit: brew can fail late having already linked
    // the formula, and the truth is on disk either way.
    let found = rescan_into_state(&runtimes, &sup, &paths).await?;
    let detected = found.iter().any(|r| r.major == major.as_str());

    Ok(InstallOutcomeDto {
        major: major.as_str().to_string(),
        exit_code,
        detected,
    })
}
```

`rescan_into_state` is shared with `rescan_php_runtimes`: probe, write the `RwLock`, and
`sup.register(php_fpm_spec(home, rt))` for each major that was not there before. Registration
at runtime is supported — `register` takes `&self` and refuses to replace a live entry.

`InstallLock` is a newtype over `tokio::sync::Mutex<()>` managed in `lib.rs`, mirroring the
apply lock. `now_ms()` and `.emit()` follow whatever `ServiceLogEvent` already does — read it
rather than inventing a second convention.

**`detected: false` with `exit_code: Some(0)` is the case that matters.** brew reporting
success while no `php-fpm` appears is the silent-failure class this project keeps catching;
the DTO carries it so the UI can say it plainly instead of showing nothing.

The probe is async while `discover_php_in` takes a synchronous closure. Resolve that with
`spawn_blocking` around the whole rescan, or by pre-building a version map and handing
discovery a lookup — **pick one, make it work, and say which you chose and why in your report.**

- [ ] **Step 5: Regenerate the bindings**

Run: `cargo test -p openvhost-desktop export_bindings`, add the wrappers and types to
`apps/desktop/src/lib/ipc/index.ts`, and confirm with
`git diff --stat apps/desktop/src/lib/ipc/bindings.ts` that it changed. No diff means the
export test did not run.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings
cargo test -p openvhost-desktop && pnpm -C apps/desktop test
git add apps/desktop
git commit -s -m "feat(desktop): expose the PHP environment, install and rescan over IPC"
```

- [ ] **Step 7: Prove the runner works against real brew**

Add `crates/openvhost-core/tests/brew_live.rs`:

```rust
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
        if let openvhost_proc::TaskEvent::Line { text, .. } = e {
            if text.starts_with("Homebrew") {
                saw_banner = true;
            }
        }
    }
    assert!(saw_banner, "no Homebrew banner in the output");
}
```

Run: `cargo test -p openvhost-core --test brew_live -- --nocapture`

- [ ] **Step 8: Request the security audit**

Dispatch the `security-auditor` subagent over the branch diff. Point it at: whether the catalogue allowlist closes flag injection, that the argv is composed rather than accepted, that `brew` is located by absolute path and never `PATH`, that the process group covers brew's whole child tree, and that only one install can run at a time. A written APPROVE is required before merge.

---

## Task 6: The Languages page

**Files:**
- Create: `apps/desktop/src/lib/languages.svelte.ts` + `languages.svelte.test.ts`
- Create: `apps/desktop/src/lib/components/LanguageRow.svelte` + `LanguageRow.svelte.test.ts`
- Create: `apps/desktop/src/routes/languages/+page.svelte`
- Modify: `apps/desktop/src/lib/components/Rail.svelte`

**Interfaces:**
- Consumes: `phpEnvironment()`, `rescanPhpRuntimes()`, `installPhp(major)`, `startService`/`stopService` (already in `index.ts`), the install-log event.
- Produces:
  ```ts
  export class LanguagesStore {
      env: PhpEnvironmentDto | null;
      installing: string;         // '' when idle, otherwise the major
      log: UiLog[];
      error: string;
      outcome: InstallOutcomeDto | null;
      get brewFound(): boolean;
      get anyInstalled(): boolean;
      refresh(): Promise<void>;
      rescan(): Promise<void>;
      install(major: string): Promise<boolean>;
  }
  ```

- [ ] **Step 1: Write the failing store tests**

```ts
it('lists what the backend returns', async () => {
	const s = new LanguagesStore(api({ env: env([row('8.3', true), row('8.4', false)]) }));
	await s.refresh();
	expect(s.env?.runtimes.map((r) => r.major)).toEqual(['8.3', '8.4']);
});

it('knows the difference between no PHP and no Homebrew', async () => {
	// Different states, different remedies — the page cannot infer the second
	// from an empty list.
	const noPhp = new LanguagesStore(api({ env: { brewFound: true, brewSearched: [], runtimes: [row('8.4', false)] } }));
	await noPhp.refresh();
	expect(noPhp.brewFound).toBe(true);
	expect(noPhp.anyInstalled).toBe(false);

	const noBrew = new LanguagesStore(api({ env: { brewFound: false, brewSearched: ['/opt/homebrew/bin/brew'], runtimes: [] } }));
	await noBrew.refresh();
	expect(noBrew.brewFound).toBe(false);
});

it('marks which version is installing and clears it when done', async () => {
	const s = new LanguagesStore(api({ outcome: { major: '8.4', exitCode: 0, detected: true } }));
	const p = s.install('8.4');
	expect(s.installing).toBe('8.4');
	expect(await p).toBe(true);
	expect(s.installing).toBe('');
});

it('refuses a second install while one is running', async () => {
	let calls = 0;
	const s = new LanguagesStore({
		phpEnvironment: async () => env([]),
		rescanPhpRuntimes: async () => env([]),
		installPhp: async () => {
			calls += 1;
			await new Promise((r) => setTimeout(r, 5));
			return { major: '8.4', exitCode: 0, detected: true };
		}
	});
	await Promise.all([s.install('8.4'), s.install('8.3')]);
	expect(calls).toBe(1);
});

it('keeps the log and surfaces the error when the install fails', async () => {
	const s = new LanguagesStore({
		phpEnvironment: async () => env([]),
		rescanPhpRuntimes: async () => env([]),
		installPhp: async () => {
			throw { kind: 'core', message: 'brew: no such formula' };
		}
	});
	s.appendLog('8.4', 'fetching');
	expect(await s.install('8.4')).toBe(false);
	expect(s.error).toContain('no such formula');
	expect(s.log.length).toBe(1);
	expect(s.installing).toBe('');
});

it('re-reads the environment after a successful install rather than assuming', async () => {
	// Assuming would show the version as installed even when the rescan did
	// not find it — the exact case `detected` exists to report.
	let calls = 0;
	const s = new LanguagesStore({
		phpEnvironment: async () => {
			calls += 1;
			return env([row('8.4', calls > 1)]);
		},
		rescanPhpRuntimes: async () => env([row('8.4', true)]),
		installPhp: async () => ({ major: '8.4', exitCode: 0, detected: true })
	});
	await s.refresh();
	expect(s.env?.runtimes[0].installed).toBe(false);
	await s.install('8.4');
	expect(calls).toBe(2);
	expect(s.env?.runtimes[0].installed).toBe(true);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C apps/desktop test languages.svelte`
Expected: FAIL — cannot resolve `./languages.svelte`.

- [ ] **Step 3: Implement the store**

Follow `apps/desktop/src/lib/apply.svelte.ts` for shape: an injected API object, `$state` fields, an `errorMessage(e: unknown)` helper, and a re-entrancy guard **in the store** rather than only on a button's `disabled` attribute — deleting an attribute must leave a test failing. `install()` always re-reads the environment on success.

- [ ] **Step 4: Write the failing row tests**

`LanguageRow.svelte.test.ts`, SSR render-to-string in the style of `ApplyDialog.svelte.test.ts`:

```ts
it('shows the version, path and socket when installed', () => {
	const body = renderRow({ row: r('8.3', true, { fullVersion: '8.3.14', path: '/opt/homebrew/opt/php@8.3/sbin/php-fpm', socketPath: '/Users/x/.openvhost/run/php-fpm-8.3.sock', serviceId: 'php-fpm-8.3' }) });
	expect(body).toContain('8.3.14');
	expect(body).toContain('/opt/homebrew/opt/php@8.3');
	expect(body).toContain('php-fpm-8.3.sock');
	expect(body).not.toContain('data-testid="install-8.3"');
});

it('offers start and stop for an installed version', () => {
	// The install-to-running flow otherwise spans three pages.
	const body = renderRow({ row: r('8.3', true, { serviceId: 'php-fpm-8.3' }), running: false });
	expect(body).toContain('data-testid="start-php-fpm-8.3"');
});

it('offers no lifecycle control for a version that is not installed', () => {
	const body = renderRow({ row: r('8.4', false) });
	expect(body).toContain('data-testid="install-8.4"');
	expect(body).not.toMatch(/data-testid="(start|stop)-/);
});

it('marks the recommended version', () => {
	expect(renderRow({ row: r('8.5', false, { recommended: true }) })).toMatch(/recommended/i);
	expect(renderRow({ row: r('8.1', false, { recommended: false }) })).not.toMatch(/recommended/i);
});

it('disables the install button while any install is running', () => {
	expect(renderRow({ row: r('8.4', false), installing: '8.3' })).toContain('disabled');
	expect(renderRow({ row: r('8.4', false), installing: '' })).not.toContain('disabled');
});

it('says plainly when brew succeeded but the version was not found', () => {
	// exitCode 0 with detected false. Without this the user presses Install
	// again and again with nothing to explain the silence.
	const body = renderRow({ row: r('8.4', false), outcome: { major: '8.4', exitCode: 0, detected: false } });
	expect(body).toMatch(/could not find|was not found/i);
	expect(body).not.toContain('data-testid="install-success-8.4"');
});

it('keeps the failure output on screen with its line breaks', () => {
	const body = renderRow({ row: r('8.4', false), error: 'Error: line 1\nline 2' });
	expect(body).toContain('line 2');
	expect(body).toMatch(/white-space:\s*pre-wrap/);
});

it('tells the user a pool still has to be created after a successful install', () => {
	const body = renderRow({ row: r('8.4', true, { fullVersion: '8.4.12' }), outcome: { major: '8.4', exitCode: 0, detected: true } });
	expect(body).toMatch(/apply/i);
});
```

- [ ] **Step 5: Run to verify failure**

Run: `pnpm -C apps/desktop test LanguageRow`
Expected: FAIL — component does not exist.

- [ ] **Step 6: Build the components and the route**

`LanguageRow.svelte` — props `{ row, running, installing, log, error, outcome, onInstall, onStart, onStop }`. Reuses `LogPane.svelte` (`{ logs: UiLog[] }`, where `UiLog` has `tsMs`, `level`, `line`) beneath the row that is installing. **Never `{@html}`** — brew's output is third-party text.

`routes/languages/+page.svelte` — constructs `LanguagesStore`, calls `refresh()` on mount, subscribes to the install-log event, and **groups rows under a "PHP" heading** even though PHP is the only language, so adding Node.js later is a new group rather than a redesign (spec §6). Start/stop read their running state from the existing services store rather than a second copy.

`Rail.svelte` — a **Languages** nav item after Web Server, matching the existing enabled items (`<a>` with `href={resolve('/languages')}` and `aria-current`), not the `aria-disabled` placeholder style used for Logs and Settings.

- [ ] **Step 7: Run the frontend gate**

```bash
pnpm -C apps/desktop test
pnpm -C apps/desktop lint
pnpm -C apps/desktop exec svelte-check
```

- [ ] **Step 8: Commit**

```bash
git add apps/desktop/src
git commit -s -m "feat(ui): a Languages page for installing and running PHP versions

Grouped by language so a second runtime is a new group rather than a
redesign, with start/stop on the row so install-to-running does not span
three pages. Reports brew exiting 0 while the version still cannot be
found, rather than showing nothing and inviting a retry."
```

---

## Task 7: The states a first-time machine actually lands in

The page a user opens when they have never installed PHP — and quite possibly never installed Homebrew. Split from Task 6 because it is the state that decides whether a new user gets anywhere at all, and it deserves its own review rather than being the last thing squeezed into a big task.

**Files:**
- Create: `apps/desktop/src/lib/components/LanguagesEmpty.svelte` + `LanguagesEmpty.svelte.test.ts`
- Modify: `apps/desktop/src/routes/languages/+page.svelte`

**Interfaces:**
- Consumes: `LanguagesStore.brewFound`, `.anyInstalled`, `.rescan()`, `env.brewSearched`.

- [ ] **Step 1: Write the failing tests**

```ts
it('invites the user to install when brew is present and no PHP is', () => {
	const body = render({ brewFound: true, anyInstalled: false });
	expect(body).toContain('data-testid="languages-no-php"');
	expect(body).toMatch(/install/i);
	expect(body).not.toContain('data-testid="languages-no-brew"');
});

it('explains the dependency and how to satisfy it when brew is missing', () => {
	// Otherwise the user came here to solve a problem and was handed a
	// different one with no way forward.
	const body = render({ brewFound: false, brewSearched: ['/opt/homebrew/bin/brew', '/usr/local/bin/brew'] });
	expect(body).toContain('data-testid="languages-no-brew"');
	expect(body).toContain('/opt/homebrew/bin/brew');
	expect(body).toContain('brew.sh');
	expect(body).toContain('data-testid="languages-check-again"');
});

it('offers the brew install command as copyable text, never as a button that runs it', () => {
	// A curl | bash that asks for sudo is the machine owner's decision, and
	// our spawned process has no tty to answer the prompt anyway.
	const body = render({ brewFound: false, brewSearched: [] });
	expect(body).toContain('/bin/bash -c');
	expect(body).not.toMatch(/data-testid="install-homebrew"/);
});

it('shows neither empty state once a version is installed', () => {
	const body = render({ brewFound: true, anyInstalled: true });
	expect(body).not.toContain('data-testid="languages-no-php"');
	expect(body).not.toContain('data-testid="languages-no-brew"');
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C apps/desktop test LanguagesEmpty`
Expected: FAIL — component does not exist.

- [ ] **Step 3: Build it**

Three states, one component, chosen in this order: no brew → no PHP → neither. The no-brew state names every path that was searched (from `brewSearched`, not hardcoded), shows the official install command as selectable text, links to `https://brew.sh`, and offers **Check again**, which calls `rescan()`.

**Do not add a button that runs Homebrew's installer.** State the reason in a comment so nobody adds one later as a convenience.

- [ ] **Step 4: Gate and commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop exec svelte-check
git add apps/desktop/src
git commit -s -m "feat(ui): guide a machine that has no PHP — or no Homebrew

A user who has never installed PHP has very likely never installed
Homebrew either, and 'not found, here are the paths' is a dead end one
level further up. Explains the dependency, offers the command to copy,
and offers Check again so they need not relaunch the app."
```

---

## Task 8: Offer the installed versions in the site editor

**Files:**
- Modify: `apps/desktop/src/lib/sites.derive.ts` + `sites.derive.test.ts`
- Modify: `apps/desktop/src/lib/components/SiteDrawer.svelte`
- Modify: `apps/desktop/src/routes/+page.svelte`

**Interfaces:**
- Consumes: `phpEnvironment()`.
- Produces: `phpVersionOptions(current: string | undefined, installed: readonly string[])` — the existing function gains a second parameter.

- [ ] **Step 1: Write the failing tests**

`sites.derive.ts:33` holds `PHP_VERSIONS = ['8.4','8.3','8.2','8.1']`, a closed list unrelated to the machine. Read the existing `phpVersionOptions` tests first and keep their intent — the "stored value stays selectable" behaviour must survive.

```ts
it('offers the versions actually installed', () => {
	const opts = phpVersionOptions(undefined, ['8.1', '8.3']);
	expect(opts.map((o) => o.value)).toEqual(['8.1', '8.3']);
});

it('keeps the stored version selectable when it is not installed', () => {
	// Dropping it would make the <select> render blank and silently rewrite
	// the site's PHP version to something the user never chose.
	const opts = phpVersionOptions('7.4', ['8.3']);
	expect(opts[0].value).toBe('7.4');
	expect(opts[0].label).toMatch(/not available|not installed/i);
});

it('does not duplicate the stored version when it is installed', () => {
	const opts = phpVersionOptions('8.3', ['8.1', '8.3']);
	expect(opts.filter((o) => o.value === '8.3')).toHaveLength(1);
});

it('still offers something when nothing is installed', () => {
	// An empty <select> would leave the user unable to save at all.
	const opts = phpVersionOptions('8.3', []);
	expect(opts.length).toBeGreaterThan(0);
	expect(opts[0].value).toBe('8.3');
});

it('defaults a new site to the newest installed version', () => {
	// A site that is broken before the user has touched anything is the
	// second of the three mistakes in spec §5.0.
	expect(defaultPhpVersion(['8.1', '8.3', '8.5'])).toBe('8.5');
});

it('has no default to offer when nothing is installed', () => {
	expect(defaultPhpVersion([])).toBeUndefined();
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C apps/desktop test sites.derive`
Expected: FAIL — `phpVersionOptions` takes one argument, `defaultPhpVersion` does not exist.

- [ ] **Step 3: Implement**

Change `phpVersionOptions` to take the installed list, keep the prepend-the-stored-value behaviour and its explanatory comment, and add `defaultPhpVersion(installed)`. Delete `PHP_VERSIONS` if nothing else imports it (`grep -rn "PHP_VERSIONS" apps/desktop/src`); if something does, leave it and say what in your report.

Thread the installed list from `+page.svelte` (which can call `phpEnvironment()` alongside its existing loads) into `SiteDrawer.svelte` as a prop, and use `defaultPhpVersion` for a new site instead of `PHP_VERSIONS[0]`.

With nothing installed, the drawer says so and links to Languages rather than presenting an empty `<select>` above a Save button that cannot lead anywhere.

- [ ] **Step 4: Gate and commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop exec svelte-check
git add apps/desktop/src
git commit -s -m "feat(ui): offer the installed PHP versions in the site editor

The list was a hard-coded constant unrelated to the machine, and a new
site defaulted into it — so a site could be born pointing at a version
Apply would then refuse."
```

---

## Task 9: A way out when a site's PHP version is missing

Prevention cannot be complete: a user can `brew uninstall php@8.3` at any time and strand a site that worked yesterday. This is the recovery path.

**Files:**
- Modify: `apps/desktop/src/routes/+page.svelte`
- Modify: `apps/desktop/src/lib/components/SiteListRow.svelte` + its test
- Create/modify: the apply-error banner markup and its test

**Interfaces:**
- Consumes: `phpEnvironment()` for the installed list; the existing `applyStore.error`.

- [ ] **Step 1: Write the failing tests**

```ts
it('warns on the row when a site wants a version that is not installed', () => {
	// Visible when the site is created, rather than as a surprise at Apply.
	const body = renderRow({ site: site({ phpVersion: '8.4' }), installed: ['8.5'] });
	expect(body).toContain('data-testid="php-missing"');
	expect(body).toContain('8.4');
});

it('does not warn when the version is installed', () => {
	const body = renderRow({ site: site({ phpVersion: '8.5' }), installed: ['8.5'] });
	expect(body).not.toContain('data-testid="php-missing"');
});

it('offers both ways out of a missing-runtime failure', () => {
	// Stating the problem without an exit is what left the user stuck.
	const body = renderBanner({
		error: 'site hello needs PHP 8.4, which is not installed (installed: 8.5)',
		missing: { site: 'hello', requested: '8.4' }
	});
	expect(body).toContain('data-testid="go-install-8.4"');
	expect(body).toContain('data-testid="edit-site-hello"');
});

it('shows no actions for a failure that is not about a missing runtime', () => {
	// A nginx -t syntax error has no "install this" remedy; offering one
	// would be worse than offering nothing.
	const body = renderBanner({ error: 'nginx: [emerg] unknown directive', missing: null });
	expect(body).not.toMatch(/data-testid="go-install-/);
});
```

- [ ] **Step 2: Run to verify failure**

Run: `pnpm -C apps/desktop test SiteListRow`
Expected: FAIL — no such testid.

- [ ] **Step 3: Implement**

The row badge compares the site's `phpVersion` against the installed list threaded from `+page.svelte`.

For the banner, the frontend must know *whether* the failure was a missing runtime and *which* version. Do not parse the message string — that is a contract nobody agreed to. Either add a structured field to the error DTO in `commands.rs` (`missingRuntime: { site, requested } | null`) or derive it in the frontend by comparing the site list against the installed list before calling apply. **Pick one, and say which and why in your report** — the structured field is more honest but touches the Rust surface again.

`Install PHP 8.4` navigates to `/languages`; `Edit hello` opens that site's drawer.

- [ ] **Step 4: Gate and commit**

```bash
pnpm -C apps/desktop test && pnpm -C apps/desktop lint && pnpm -C apps/desktop exec svelte-check
git add apps/desktop/src
git commit -s -m "feat(ui): offer a way out when a site's PHP version is missing

The banner stated the problem and offered nothing to press, which is how
a user ended up with a site that could never apply. Now it offers the two
actions that resolve it, and the row warns before Apply rather than after."
```

---

## Definition of Done

- [ ] A machine with no PHP shows an invitation to install one, not an inventory of things it lacks.
- [ ] A machine with no Homebrew is told what OpenVHost needs, given the command, and can press **Check again** without relaunching.
- [ ] The Languages page lists every catalogue version plus anything installed outside it, marks one as recommended, and shows the full version, path and socket for installed ones.
- [ ] Pressing Install streams brew's output live and finishes with the version installed and its pool startable from the same row.
- [ ] A second install cannot start while one is running.
- [ ] brew exiting 0 without the version appearing is reported in words, not by showing nothing.
- [ ] A newly installed version appears in the site editor without relaunching, and the Sites banner offers the pool as a pending change.
- [ ] A new site defaults to an installed version; a site pointing at a missing one is flagged in its row and offers both ways out from the failure banner.
- [ ] A site can be set to a newly installed version and served by it while another site uses a different one.
- [ ] `--build-from-source`, `9.9` and `nginx` are all rejected at the IPC boundary naming `php_version`.
- [ ] security-auditor APPROVE recorded on the branch.
- [ ] Full gate green: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C apps/desktop test`, `pnpm -C apps/desktop lint`, `pnpm -C apps/desktop exec svelte-check`.
