<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
# Status Bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a persistent bottom strip to the main window showing the resident memory of supervised services and the size of `~/.openvhost`.

**Architecture:** A macOS `proc_pidinfo` reader in `openvhost-proc`'s existing platform seam and a symlink-safe directory walk in `openvhost-core` feed two separate Tauri commands, deliberately split because their sampling rates differ by 30×. The frontend polls them from one store and renders a presentational strip as a third grid row in `AppShell`.

**Tech Stack:** Rust (libc FFI, tauri 2.11, specta), SvelteKit + Svelte 5 runes, vitest.

Spec: `docs/superpowers/specs/2026-07-26-status-bar-design.md`. Read the spec section named in each task; it carries the measured facts behind these numbers.

## Global Constraints

- SPDX header `// SPDX-License-Identifier: GPL-3.0-or-later` (or `<!-- ... -->` for `.svelte`) on line 1 of every new file.
- No `unwrap()` / `expect()` outside `#[cfg(test)]`.
- `openvhost-core` / `-proc` / `-conf` / `-pkg` must never depend on `tauri`.
- Conventional Commits, and every commit signed off: `git commit -s`. **No `Co-Authored-By` trailer.**
- macOS-first: real implementation behind `#[cfg(target_os = "macos")]`, with stub arms on every other target so the crate still compiles.
- CI is disabled on GitHub. **The local gates ARE the merge gate:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, and `pnpm -C apps/desktop lint`, `check`, `test`, `build`. All seven must pass before any task is considered done.
- `formatBytes` is 1024-based, labelled `B / KB / MB / GB`, one decimal when the mantissa is below 10 and none at or above it (spec §4.4).
- Sampling intervals: services memory **2000 ms**, home size **60000 ms** (spec §5).
- The strip never raises a banner or a toast. Unknown values render as `—` (spec §6).
- **Every test must be verified to fail under a named mutation**, then restored. A test that cannot fail is not coverage. State the mutation and its result in the task report.

## File Structure

| File | Responsibility |
|---|---|
| `crates/openvhost-proc/src/platform/unix.rs` | *modify* — add `process_rss` (macOS body + non-macOS stub) |
| `crates/openvhost-proc/src/platform/windows.rs` | *modify* — add `process_rss` stub |
| `crates/openvhost-proc/src/platform/mod.rs` | *modify* — add the `pub fn process_rss` dispatch |
| `crates/openvhost-core/src/home.rs` | *modify* — add `dir_size_no_follow` + `home_disk_usage` |
| `crates/openvhost-core/src/lib.rs` | *modify* — export `home_disk_usage` |
| `apps/desktop/src-tauri/src/commands.rs` | *modify* — 2 DTOs + 2 commands |
| `apps/desktop/src-tauri/src/lib.rs` | *modify* — register both commands |
| `apps/desktop/src/lib/ipc/index.ts` | *modify* — 2 typed wrappers |
| `apps/desktop/src/lib/stats.derive.ts` | **new** — pure formatting (`formatBytes`, `formatProcessCount`) |
| `apps/desktop/src/lib/stats.svelte.ts` | **new** — `StatsStore`: both figures, their timers, their unknown states |
| `apps/desktop/src/lib/stats.shared.svelte.ts` | **new** — the app's one instance, bound to real IPC |
| `apps/desktop/src/lib/components/StatusBar.svelte` | **new** — presentational strip |
| `apps/desktop/src/lib/components/AppShell.svelte` | *modify* — third grid row |
| `apps/desktop/src/routes/+layout.svelte` | *modify* — start/stop the timers on visibility |

---

### Task 1: `platform::process_rss` — read resident memory on macOS

Read spec §3.1 and §4.1 first. The measured facts there (bytes not pages; `rc <= 0` cannot distinguish dead from not-permitted) are the reason this function has the exact shape below.

**Files:**
- Modify: `crates/openvhost-proc/src/platform/unix.rs` (add after `process_start_time`'s non-macOS stub, around line 132)
- Modify: `crates/openvhost-proc/src/platform/windows.rs` (add after `process_start_time`'s stub, around line 86)
- Modify: `crates/openvhost-proc/src/platform/mod.rs` (add after the `process_start_time` dispatch pair, around line 191)
- Test: `crates/openvhost-proc/src/platform/unix.rs` (its existing `#[cfg(test)]` module at line 257)

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `openvhost_proc::platform::process_rss(pid: u32) -> std::io::Result<Option<u64>>` — bytes. `Ok(None)` = no figure for this pid (dead or not inspectable); `Err` = unsupported platform. Task 3 calls this.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` in `crates/openvhost-proc/src/platform/unix.rs`:

```rust
    /// A live process must report a non-zero resident size. This is the test that
    /// catches a units mistake or a wrong struct field: `/bin/sleep`'s RSS is a
    /// few hundred KB, so a pages-vs-bytes error would show up as an absurd value
    /// and a wrong-field error as zero.
    #[cfg(target_os = "macos")]
    #[test]
    fn rss_of_a_live_process_is_plausible() {
        use std::process::{Command, Stdio};
        #[allow(clippy::zombie_processes)]
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        let rss = process_rss(pid).unwrap().expect("a live process has an RSS");
        // Lower bound: any real process has more than 64 KB resident.
        assert!(rss > 64 * 1024, "rss {rss} is implausibly small");
        // Upper bound: `sleep` is tiny. 1 GB would mean we read pages as bytes
        // (4096x) or picked up pti_virtual_size instead of pti_resident_size.
        assert!(rss < 1024 * 1024 * 1024, "rss {rss} is implausibly large");
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A pid that no longer exists is `Ok(None)`, NOT `Err`. The caller samples
    /// pids the supervisor listed a moment earlier; a process exiting in that gap
    /// is normal and must not surface as a failure (spec §4.1).
    #[cfg(target_os = "macos")]
    #[test]
    fn rss_of_a_dead_pid_is_none_not_an_error() {
        use std::process::{Command, Stdio};
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid = child.id();
        child.kill().unwrap();
        child.wait().unwrap(); // reap, or the zombie still answers
        assert_eq!(process_rss(pid).unwrap(), None);
    }

    /// Guarded before any FFI: pid 0 is `kernel_task` and anything above
    /// `i32::MAX` cannot be a pid we spawned.
    #[cfg(target_os = "macos")]
    #[test]
    fn rss_rejects_pid_zero_and_out_of_range_without_calling_ffi() {
        assert_eq!(process_rss(0).unwrap(), None);
        assert_eq!(process_rss(i32::MAX as u32 + 1).unwrap(), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openvhost-proc rss_ 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find function 'process_rss' in this scope`.

- [ ] **Step 3: Write the macOS implementation**

In `crates/openvhost-proc/src/platform/unix.rs`, immediately after the
`#[cfg(all(unix, not(target_os = "macos")))] process_start_time` stub:

```rust
/// Resident set size in BYTES for a live process.
///
/// `pti_resident_size` is in BYTES — not pages, not kilobytes. Verified against
/// `ps -o rss=` on macOS: 14,254,080 here vs 13,952 KB = 14,286,848 there, a
/// ratio of 0.998 (the same value sampled a moment apart). Reading it as pages
/// would be 4096x wrong. (spec §3.1)
///
/// `Ok(None)` means "no figure for this pid". `proc_pidinfo` cannot distinguish a
/// dead pid from one we may not inspect — a nonexistent pid returned `rc == 0`
/// with `errno == ESRCH`, and pid 1 (launchd) returned `rc == 0` too. Both are
/// `Ok(None)` deliberately: the caller samples pids the supervisor listed a
/// moment ago, and a process exiting in that gap is a normal race, not a
/// failure. We only ever read our own children, so the permission case does not
/// arise in practice.
#[cfg(target_os = "macos")]
pub(crate) fn process_rss(pid: u32) -> io::Result<Option<u64>> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(None); // pid 0 is kernel_task; out-of-range can't be ours
    }
    // SAFETY: `proc_taskinfo` is a POD of `u64`/`i32` fields (libc 0.2.189,
    // unix/bsd/apple/mod.rs:585), so an all-zero bit pattern is a valid value.
    let mut ti: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    // SAFETY: `ti` is a valid writable region of exactly `size` bytes, which is
    // what PROC_PIDTASKINFO writes; `arg` is unused for this flavor (0). No
    // pointer is retained past the call.
    let rc = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut ti as *mut libc::proc_taskinfo as *mut libc::c_void,
            size,
        )
    };
    if rc <= 0 {
        return Ok(None); // dead pid, or not inspectable — see the doc comment
    }
    if rc < size {
        // Short write: the struct is partial, so the field we want may be
        // untouched zero rather than a real reading. Report no figure instead of
        // a fabricated one. (Mirrors process_start_time's undersized-record check.)
        return Ok(None);
    }
    Ok(Some(ti.pti_resident_size))
}

/// Non-macOS unix `process_rss` is deferred to the Windows/Linux-enablement
/// phase (macOS-first). Returns `Err`, mirroring `process_start_time`'s stub
/// above — and `Err` is the CORRECT answer, not merely the consistent one:
/// `Ok(None)` would make every pid here "no figure", which the caller sums to 0
/// with a count of 0 and the status strip renders as "services 0 MB · no
/// processes" — false while services are running. `Err` renders as "—", which
/// is true: unknown, not zero.
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn process_rss(_pid: u32) -> io::Result<Option<u64>> {
    Err(io::Error::other(
        "process_rss is not implemented on this platform in v1 (macOS-first)",
    ))
}
```

In `crates/openvhost-proc/src/platform/windows.rs`, after the `process_start_time` stub:

```rust
/// See `unix.rs`'s non-macOS arm for why this returns `Err` rather than
/// `Ok(None)`: a false zero would read as "nothing running".
#[cfg(windows)]
pub(crate) fn process_rss(_pid: u32) -> io::Result<Option<u64>> {
    Err(io::Error::other(
        "process_rss is not implemented on Windows in v1 (macOS-first)",
    ))
}
```

In `crates/openvhost-proc/src/platform/mod.rs`, after the `process_start_time` dispatch pair:

```rust
/// Resident set size in bytes for a live pid. See the platform impls for the
/// `Ok(None)` vs `Err` contract.
#[cfg(unix)]
pub fn process_rss(pid: u32) -> std::io::Result<Option<u64>> {
    unix::process_rss(pid)
}
#[cfg(windows)]
pub fn process_rss(pid: u32) -> std::io::Result<Option<u64>> {
    windows::process_rss(pid)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p openvhost-proc rss_ 2>&1 | grep -E "^test |test result"`
Expected: 3 tests, all `ok`.

- [ ] **Step 5: Verify each test can fail**

Apply each mutation, run the tests, confirm the named test fails, then restore.

| Mutation | Must fail |
|---|---|
| return `ti.pti_virtual_size` instead of `pti_resident_size` | `rss_of_a_live_process_is_plausible` (virtual size is > 1 GB) |
| return `Ok(Some(ti.pti_resident_size * 4096))` (pages-as-bytes) | `rss_of_a_live_process_is_plausible` |
| `if rc <= 0 { return Err(io::Error::last_os_error()) }` | `rss_of_a_dead_pid_is_none_not_an_error` |
| delete the `pid == 0 \|\| pid > i32::MAX` guard | `rss_rejects_pid_zero_and_out_of_range_without_calling_ffi` |

Record the outcome of all four in the task report.

- [ ] **Step 6: Run the full Rust gate**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/openvhost-proc/src/platform/
git commit -s -m "feat(proc): read a process's resident memory on macOS

platform::process_rss via one proc_pidinfo(PROC_PIDTASKINFO) call.
pti_resident_size is in BYTES, verified against ps -o rss= (ratio 0.998), so a
pages reading would have been 4096x wrong.

Ok(None) rather than Err for rc <= 0: proc_pidinfo cannot distinguish a dead pid
from one we may not inspect (a nonexistent pid and pid 1 both return 0), and the
caller samples pids the supervisor listed a moment earlier - a process exiting in
that gap is a normal race, not a failure.

The stub arms return Err on purpose. Ok(None) there would make every pid 'no
figure', which the caller sums to zero and a status strip renders as 'no
processes' - false while services run. Err renders as unknown, which is true."
```

---

### Task 2: `openvhost-core::home_disk_usage` — a symlink-safe directory walk

Read spec §3.2 and §4.2 first. The symlink rule is the whole point of this task: the package layout puts a `current` link beside each version directory, so a link-following walk double-counts every installed version.

**Files:**
- Modify: `crates/openvhost-core/src/home.rs` (add after `resolve_home_from`, before the `#[cfg(test)]` module)
- Modify: `crates/openvhost-core/src/lib.rs:16` (extend the `home` re-export)
- Test: `crates/openvhost-core/src/home.rs` (its existing `#[cfg(test)]` module)

**Interfaces:**
- Consumes: `resolve_home() -> Result<PathBuf, CoreError>` (already exists at `home.rs:12`).
- Produces: `openvhost_core::home_disk_usage() -> Result<u64, CoreError>`. Task 3 calls this.

- [ ] **Step 1: Write the failing tests**

Append inside the existing `#[cfg(test)] mod tests` in `crates/openvhost-core/src/home.rs`:

```rust
    /// Files at several depths all count, and the total is exact.
    #[test]
    fn sums_regular_files_at_every_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.bin"), vec![0u8; 100]).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/b.bin"), vec![0u8; 250]).unwrap();
        std::fs::create_dir(root.join("sub/deeper")).unwrap();
        std::fs::write(root.join("sub/deeper/c.bin"), vec![0u8; 5]).unwrap();
        assert_eq!(dir_size_no_follow(root), 355);
    }

    /// THE test this function exists for. The package layout places a `current`
    /// symlink beside each version directory
    /// (`packages/<name>/<major>/current` -> a sibling version dir), so a walk
    /// that follows links counts every installed version twice. Adding a link
    /// must not change the total by one byte.
    #[cfg(unix)]
    #[test]
    fn a_symlink_does_not_add_to_the_total() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir(root.join("8.3.7")).unwrap();
        std::fs::write(root.join("8.3.7/php"), vec![0u8; 1000]).unwrap();
        let before = dir_size_no_follow(root);
        assert_eq!(before, 1000);

        // A link to the DIRECTORY — the `current` case. Following it would walk
        // 8.3.7 a second time and double the figure.
        std::os::unix::fs::symlink(root.join("8.3.7"), root.join("current")).unwrap();
        // A link to a FILE — following it would add that file's bytes again.
        std::os::unix::fs::symlink(root.join("8.3.7/php"), root.join("php-link")).unwrap();

        assert_eq!(dir_size_no_follow(root), before);
    }

    /// A directory we cannot read is skipped, not fatal: a partial figure beats
    /// no figure in a status strip. (Skipped when running as root, which can read
    /// a 0o000 directory regardless.)
    /// Our effective uid, via the owner of a file we just create: root bypasses
    /// the permission bits the next test relies on. Uses `std` only, so this crate
    /// needs no new dev-dependency for it. The probe lives in its own temp file so
    /// it cannot contribute bytes to any measured tree.
    #[cfg(unix)]
    fn running_as_root() -> bool {
        use std::os::unix::fs::MetadataExt;
        let probe = tempfile::NamedTempFile::new().unwrap();
        probe.as_file().metadata().unwrap().uid() == 0
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_directory_is_skipped_not_fatal() {
        use std::os::unix::fs::PermissionsExt;
        if running_as_root() {
            return; // root can read a 0o000 directory regardless
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("readable.bin"), vec![0u8; 42]).unwrap();
        let locked = root.join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::write(locked.join("hidden.bin"), vec![0u8; 9999]).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();

        let total = dir_size_no_follow(root);

        // Restore before the assert so the tempdir can always be cleaned up.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(total, 42, "the readable part must still be counted");
    }

    /// A path that does not exist is 0, not an error — `~/.openvhost` may not
    /// have been provisioned yet on a first run.
    #[test]
    fn a_missing_root_is_zero() {
        let tmp = tempfile::tempdir().unwrap();
        assert_eq!(dir_size_no_follow(&tmp.path().join("nope")), 0);
    }
```

**No new dependency is needed.** `tempfile = "3"` is already a dev-dependency of
`openvhost-core` (`Cargo.toml:21`), and `running_as_root` above uses only
`std::os::unix::fs::MetadataExt`. Do NOT add `libc` here just for a uid check — every new
dependency has to clear the license gate (golden rule 7), and this one buys nothing that
`std` does not already provide.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p openvhost-core home:: 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find function 'dir_size_no_follow' in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/openvhost-core/src/home.rs`, after `resolve_home_from`:

```rust
/// Total bytes of regular files under `root`, NOT following symlinks.
///
/// **Symlinks contribute nothing and are never descended into.** The package
/// layout places a `current` link per major version at
/// `packages/<name>/<major>/current`, pointing at a sibling version directory,
/// so a link-following walk would count every installed version twice.
/// `symlink_metadata` is used rather than `entry.metadata()` — both avoid
/// traversal on unix, but the name states the intent at the call site, and that
/// intent is the entire hazard here.
///
/// A directory that cannot be read is SKIPPED rather than fatal: a partial
/// figure beats no figure in a status strip, and an unreadable subdirectory is
/// not something the user can act on from there. A `root` that does not exist
/// yields 0 for the same reason — a first run may not have provisioned it.
///
/// Iterative with an explicit stack, not recursive: a deep tree must not risk
/// the call stack.
pub(crate) fn dir_size_no_follow(root: &Path) -> u64 {
    let mut total: u64 = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue; // unreadable or missing — skip, see the doc comment
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(md) = path.symlink_metadata() else {
                continue; // vanished mid-walk; nothing to count
            };
            let ft = md.file_type();
            if ft.is_symlink() {
                continue; // never counted, never descended
            }
            if ft.is_dir() {
                stack.push(path);
            } else if ft.is_file() {
                total = total.saturating_add(md.len());
            }
        }
    }
    total
}

/// Total bytes under the resolved OpenVHost home. See [`dir_size_no_follow`] for
/// the symlink and unreadable-directory rules.
pub fn home_disk_usage() -> Result<u64, CoreError> {
    Ok(dir_size_no_follow(&resolve_home()?))
}
```

In `crates/openvhost-core/src/lib.rs`, change line 16 from:

```rust
pub use home::resolve_home;
```

to:

```rust
pub use home::{home_disk_usage, resolve_home};
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p openvhost-core home:: 2>&1 | grep -E "^test |test result"`
Expected: the four new tests plus the four pre-existing `resolve_home_from` tests, all `ok`.

- [ ] **Step 5: Verify each test can fail**

| Mutation | Must fail |
|---|---|
| use `path.metadata()` and drop the `is_symlink()` branch (follows links) | `a_symlink_does_not_add_to_the_total` |
| `let entries = std::fs::read_dir(&dir).unwrap();` (unreadable becomes a panic) | `an_unreadable_directory_is_skipped_not_fatal` and `a_missing_root_is_zero` |
| don't push subdirectories onto the stack (top level only) | `sums_regular_files_at_every_depth` |

Record all three outcomes in the task report.

- [ ] **Step 6: Run the full Rust gate**

Run: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`
Expected: all pass.

- [ ] **Step 7: Commit**

```bash
git add crates/openvhost-core/
git commit -s -m "feat(core): total the home directory's size without following symlinks

home_disk_usage over an iterative, symlink-safe walk.

The symlink rule is the reason this function exists rather than a du call: the
package layout places a 'current' link beside each version directory, so a
link-following walk would count every installed PHP/MySQL version twice. Nothing
would reveal that today - no packages/ exists yet - so it would ship silently and
only go wrong once a second version is installed. There is a test for it.

An unreadable directory is skipped and a missing root is zero, both because the
caller is a status strip where a partial figure beats no figure."
```

---

### Task 3: The IPC boundary — two DTOs, two commands

Read spec §4.3 first, including the note on the builder-global bigint cast.

**Files:**
- Modify: `apps/desktop/src-tauri/src/commands.rs` (add the DTOs beside the other DTOs; add the commands after `service_log_tail`, around line 165)
- Modify: `apps/desktop/src-tauri/src/lib.rs` (register both in `collect_commands!`, around line 38)
- Modify: `apps/desktop/src/lib/ipc/index.ts` (append two wrappers)
- Test: `apps/desktop/src-tauri/src/commands.rs` (its existing `#[cfg(test)] mod site_ipc_tests`)

**Interfaces:**
- Consumes: `openvhost_proc::platform::process_rss(pid: u32) -> io::Result<Option<u64>>` (Task 1); `openvhost_core::home_disk_usage() -> Result<u64, CoreError>` (Task 2).
- Produces, for Task 4's store to call:
  - `servicesMemory(): Promise<ServicesMemoryDto>` where `ServicesMemoryDto = { bytes: number; processCount: number }`
  - `homeDiskUsage(): Promise<HomeUsageDto>` where `HomeUsageDto = { bytes: number }`

- [ ] **Step 1: Write the failing test**

The two commands need a live Tauri `AppHandle` and a real supervisor, so they are not unit-testable here — the existing `site_ipc_tests` module tests DTO mapping and error conversion, not command invocation, and this task follows that precedent. What IS testable is the summing rule, so extract it as a pure function and test that.

Append inside `#[cfg(test)] mod site_ipc_tests` in `apps/desktop/src-tauri/src/commands.rs`:

```rust
    /// The count must report pids that actually produced a figure, not pids that
    /// were listed. A service that exits between the snapshot and the read drops
    /// out of BOTH the sum and the count, so the number and its label can never
    /// disagree (spec §6).
    #[test]
    fn memory_sum_counts_only_the_pids_that_answered() {
        let readings = vec![Some(1000u64), None, Some(2500u64), None];
        let (bytes, count) = sum_readings(readings.into_iter());
        assert_eq!(bytes, 3500);
        assert_eq!(count, 2);
    }

    #[test]
    fn memory_sum_of_nothing_is_zero_with_a_zero_count() {
        let (bytes, count) = sum_readings(std::iter::empty());
        assert_eq!(bytes, 0);
        assert_eq!(count, 0);
    }

    /// Saturating, not wrapping: an absurd reading must not wrap the total to a
    /// small number that looks plausible.
    #[test]
    fn memory_sum_saturates_instead_of_wrapping() {
        let readings = vec![Some(u64::MAX), Some(1000u64)];
        let (bytes, _) = sum_readings(readings.into_iter());
        assert_eq!(bytes, u64::MAX);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p openvhost-desktop memory_sum 2>&1 | tail -15`
Expected: FAIL to compile — `cannot find function 'sum_readings' in this scope`.

- [ ] **Step 3: Write the implementation**

In `apps/desktop/src-tauri/src/commands.rs`, add the DTOs near the other DTO definitions:

```rust
/// Summed resident memory of the supervised services.
///
/// `bytes` and `process_count` are both u64/u32 crossing a
/// `.dangerously_cast_bigints_to_number()` boundary — see `lib.rs`'s standing
/// warning, which names "byte totals" as the case requiring a conscious check.
/// `2^53` bytes is 9 petabytes; a resident set is many orders of magnitude
/// below it. Checked, not assumed.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ServicesMemoryDto {
    pub bytes: u64,
    /// How many pids actually produced a figure — NOT how many services are
    /// running. See `sum_readings`.
    pub process_count: u32,
}

/// Total bytes under the OpenVHost home. Same bigint check as
/// [`ServicesMemoryDto`]: a home directory is nowhere near 9 PB.
#[derive(Debug, Clone, serde::Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct HomeUsageDto {
    pub bytes: u64,
}

/// Sum the readings that produced a figure, and count them.
///
/// Extracted from `services_memory` so the rule is testable without a live
/// `AppHandle` and a real supervisor: `None` readings (a pid that exited between
/// the snapshot and the read) drop out of the sum AND the count together, so the
/// figure and its "N processes" label can never contradict each other.
/// `saturating_add` so an absurd reading cannot wrap the total into a small,
/// plausible-looking number.
fn sum_readings(readings: impl Iterator<Item = Option<u64>>) -> (u64, u32) {
    let mut bytes: u64 = 0;
    let mut count: u32 = 0;
    for r in readings.flatten() {
        bytes = bytes.saturating_add(r);
        count = count.saturating_add(1);
    }
    (bytes, count)
}
```

Then the two commands, after `service_log_tail`:

```rust
/// Resident memory of everything the supervisor is running.
///
/// `try_state` rather than `State<'_, Arc<Supervisor>>`: the supervisor is only
/// managed when the setup bootstrap succeeded, and an unmanaged one must give a
/// clean error the strip renders as "—" rather than Tauri's raw state panic
/// message. Same precedent as the quit path.
///
/// An `Err` from `process_rss` aborts the whole read — it means measurement is
/// impossible on this platform, and reporting a partial sum as if it were
/// complete would be a false figure (spec §4.1).
#[tauri::command]
#[specta::specta]
pub async fn services_memory(app: tauri::AppHandle) -> Result<ServicesMemoryDto, IpcError> {
    use tauri::Manager;
    let Some(sup) = app.try_state::<Arc<Supervisor>>() else {
        return Err(IpcError::Proc {
            message: "the supervisor is not running".to_string(),
        });
    };
    let mut readings: Vec<Option<u64>> = Vec::new();
    for status in sup.snapshot() {
        let Some(pid) = status.pid else { continue };
        readings.push(
            openvhost_proc::platform::process_rss(pid).map_err(|e| IpcError::Proc {
                message: e.to_string(),
            })?,
        );
    }
    let (bytes, process_count) = sum_readings(readings.into_iter());
    Ok(ServicesMemoryDto {
        bytes,
        process_count,
    })
}

/// Total size of the OpenVHost home.
///
/// The walk runs on `spawn_blocking`, not inline: it measured 40 ms over 6,470
/// files (spec §3.2), which is long enough to matter on a runtime thread, and
/// the figure is not urgent.
#[tauri::command]
#[specta::specta]
pub async fn home_disk_usage() -> Result<HomeUsageDto, IpcError> {
    let bytes = tauri::async_runtime::spawn_blocking(openvhost_core::home_disk_usage)
        .await
        .map_err(|e| IpcError::Core {
            message: format!("the disk-usage task failed to run: {e}"),
        })?
        .map_err(IpcError::from)?;
    Ok(HomeUsageDto { bytes })
}
```

In `apps/desktop/src-tauri/src/lib.rs`, inside `collect_commands!`, after `commands::quit_dialog_ready,`:

```rust
            commands::services_memory,
            commands::home_disk_usage,
```

In `apps/desktop/src/lib/ipc/index.ts`, append:

```ts
/** Resident memory of the supervised services, plus how many pids answered. */
export async function servicesMemory(): Promise<ServicesMemoryDto> {
	return unwrap(commands.servicesMemory());
}

/** Total bytes under the OpenVHost home. */
export async function homeDiskUsage(): Promise<HomeUsageDto> {
	return unwrap(commands.homeDiskUsage());
}
```

and extend both the `import type { … }` list and the `export type { … }` list at the top of that file with `HomeUsageDto` and `ServicesMemoryDto`.

- [ ] **Step 4: Run the test to verify it passes, and regenerate the bindings**

```bash
cargo test -p openvhost-desktop memory_sum 2>&1 | grep -E "^test |test result"
cargo test -p openvhost-desktop export_bindings
grep -nE "servicesMemory|homeDiskUsage|ServicesMemoryDto|HomeUsageDto" apps/desktop/src/lib/ipc/bindings.ts
```

Expected: 3 tests `ok`; `export_bindings` `ok`; the grep shows both commands and both types. `bindings.ts` is generated — never hand-edit it, always regenerate.

- [ ] **Step 5: Verify each test can fail**

| Mutation | Must fail |
|---|---|
| `count` incremented for every reading including `None` | `memory_sum_counts_only_the_pids_that_answered` |
| `bytes += r` (wrapping in release / panicking in debug) instead of `saturating_add` | `memory_sum_saturates_instead_of_wrapping` |

Record both outcomes in the task report.

- [ ] **Step 6: Run every gate**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all seven pass.

- [ ] **Step 7: Commit**

```bash
git add apps/desktop/src-tauri/src/commands.rs apps/desktop/src-tauri/src/lib.rs apps/desktop/src/lib/ipc/
git commit -s -m "feat(ipc): expose services memory and home disk usage

Two commands rather than one, because their sampling rates differ by 30x: a
combined call would force the cheap read to pay the expensive one's cost.

sum_readings is extracted so the rule that matters is testable without a live
AppHandle: a pid that exits between the snapshot and the read drops out of the
sum AND the count together, so the figure and its 'N processes' label can never
contradict each other. saturating_add so an absurd reading cannot wrap the total
into a small plausible number.

The home walk runs on spawn_blocking - 40 ms is long enough to matter on a
runtime thread. Both DTOs carry byte totals across the builder-global
dangerously_cast_bigints_to_number boundary, which lib.rs's standing warning
names as requiring a conscious check: 2^53 bytes is 9 PB, so both are safe."
```

---

### Task 4: The frontend data layer — formatting and the polling store

Read spec §4.4 and §5 first.

**Files:**
- Create: `apps/desktop/src/lib/stats.derive.ts`
- Create: `apps/desktop/src/lib/stats.derive.test.ts`
- Create: `apps/desktop/src/lib/stats.svelte.ts`
- Create: `apps/desktop/src/lib/stats.svelte.test.ts`
- Create: `apps/desktop/src/lib/stats.shared.svelte.ts`

**Interfaces:**
- Consumes: `servicesMemory()` and `homeDiskUsage()` from `$lib/ipc` (Task 3).
- Produces, for Task 5's component and layout:
  - `formatBytes(bytes: number): string`, `formatProcessCount(n: number): string`, `UNKNOWN: string`
  - `class StatsStore` with reactive `servicesBytes: number | null`, `processCount: number | null`, `homeBytes: number | null`, `homePending: boolean`, and methods `start(): void`, `stop(): void`, `refreshMemory(): Promise<void>`, `refreshHome(): Promise<void>`
  - `statsStore` — the app's single instance, from `stats.shared.svelte.ts`

- [ ] **Step 1: Write the failing formatting tests**

Create `apps/desktop/src/lib/stats.derive.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { describe, expect, it } from 'vitest';
import { formatBytes, formatProcessCount } from './stats.derive';

describe('formatBytes', () => {
	// Exact strings, because the strip's whole job is to be readable at a glance
	// and a rounding change is a visible change.
	it('steps through 1024-based units with the specified precision', () => {
		expect(formatBytes(999)).toBe('999 B');
		expect(formatBytes(1024)).toBe('1.0 KB');
		// The mantissa-10 switch: one decimal below 10, none at or above.
		expect(formatBytes(10 * 1024)).toBe('10 KB');
		expect(formatBytes(1024 ** 3)).toBe('1.0 GB');
		expect(formatBytes(1024 ** 3 * 12)).toBe('12 GB');
	});

	// Zero is only ever "nothing is running". "0 B" reads like a measurement
	// error; "0 MB" reads like a total, and keeps the unit from jumping between
	// the running and idle states.
	it('renders zero as 0 MB', () => {
		expect(formatBytes(0)).toBe('0 MB');
	});

	// The store passes `null` through as unknown, but a negative or non-finite
	// number reaching here would be a bug — render it as unknown rather than
	// printing "NaN B" at the user.
	it('renders a nonsensical input as unknown rather than NaN', () => {
		expect(formatBytes(-1)).toBe('—');
		expect(formatBytes(Number.NaN)).toBe('—');
		expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('—');
	});
});

describe('formatProcessCount', () => {
	it('agrees in number and names the empty case', () => {
		expect(formatProcessCount(0)).toBe('no processes');
		expect(formatProcessCount(1)).toBe('1 process');
		expect(formatProcessCount(2)).toBe('2 processes');
	});
});
```

- [ ] **Step 2: Run it to verify it fails**

Run: `pnpm -C apps/desktop test src/lib/stats.derive.test.ts 2>&1 | tail -10`
Expected: FAIL — cannot resolve `./stats.derive`.

- [ ] **Step 3: Write the formatting module**

Create `apps/desktop/src/lib/stats.derive.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// Pure formatting for the status bar. Kept out of the store so the strings the
// user actually reads are testable without timers or IPC.

const STEP = 1024;
const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'] as const;

/** Rendered when a figure is unknown — a failed sample, or one not taken yet. */
export const UNKNOWN = '—';

/**
 * Human-readable byte count: 1024-based steps labelled B/KB/MB/GB/TB, one
 * decimal place when the mantissa is below 10 and none at or above it. So
 * `1.0 KB`, `10 KB`, `1.2 GB`, `12 GB`.
 *
 * 1024-based with decimal-looking labels is the convention `ps` and developer
 * tooling use; Finder's 1000-based labels would disagree with every other number
 * a developer sees next to this one.
 *
 * Zero is special-cased to `0 MB`: the only zero we ever render is "nothing is
 * running", and `0 B` reads like a measurement error while `0 MB` reads like a
 * total. It also stops the unit jumping when the first service starts.
 */
export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes < 0) return UNKNOWN;
	if (bytes === 0) return '0 MB';
	let value = bytes;
	let unit = 0;
	while (value >= STEP && unit < UNITS.length - 1) {
		value /= STEP;
		unit += 1;
	}
	const digits = value < 10 && unit > 0 ? 1 : 0;
	return `${value.toFixed(digits)} ${UNITS[unit]}`;
}

/** `0 → "no processes"`, `1 → "1 process"`, `n → "n processes"`. */
export function formatProcessCount(n: number): string {
	if (n === 0) return 'no processes';
	return `${n} ${n === 1 ? 'process' : 'processes'}`;
}
```

- [ ] **Step 4: Run it to verify it passes**

Run: `pnpm -C apps/desktop test src/lib/stats.derive.test.ts 2>&1 | grep -E "Tests "`
Expected: `Tests  4 passed (4)`.

- [ ] **Step 5: Write the failing store tests**

Create `apps/desktop/src/lib/stats.svelte.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { StatsStore, MEMORY_INTERVAL_MS, HOME_INTERVAL_MS } from './stats.svelte';
import type { StatsApi } from './stats.svelte';

function api(overrides: Partial<Record<string, unknown>> = {}): StatsApi {
	return {
		servicesMemory: vi.fn(async () => ({ bytes: 1000, processCount: 2 })),
		homeDiskUsage: vi.fn(async () => ({ bytes: 9999 })),
		...overrides
	} as unknown as StatsApi;
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => vi.useRealTimers());

describe('StatsStore', () => {
	it('starts with everything unknown, so nothing renders as a false zero', () => {
		const s = new StatsStore(api());
		expect(s.servicesBytes).toBeNull();
		expect(s.processCount).toBeNull();
		expect(s.homeBytes).toBeNull();
		// Distinguishes "not measured yet" from "measurement failed".
		expect(s.homePending).toBe(true);
	});

	it('takes both readings immediately on start', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(a.servicesMemory).toHaveBeenCalledTimes(1);
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(1);
		expect(s.servicesBytes).toBe(1000);
		expect(s.processCount).toBe(2);
		expect(s.homeBytes).toBe(9999);
		expect(s.homePending).toBe(false);
		s.stop();
	});

	// The two cadences are the point of the design: memory is one syscall per pid,
	// the home figure is a directory walk. Sampling them together would either
	// throttle memory or hammer the disk.
	it('samples memory 30x more often than the home size', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(HOME_INTERVAL_MS);
		// 1 immediate + HOME_INTERVAL_MS / MEMORY_INTERVAL_MS ticks
		expect(a.servicesMemory).toHaveBeenCalledTimes(1 + HOME_INTERVAL_MS / MEMORY_INTERVAL_MS);
		expect(a.homeDiskUsage).toHaveBeenCalledTimes(2); // immediate + one tick
		s.stop();
	});

	// The whole point of stop(): an app left open behind an IDE must cost nothing.
	it('issues no further calls after stop', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		const before = (a.servicesMemory as unknown as { mock: { calls: unknown[] } }).mock.calls
			.length;
		s.stop();
		await vi.advanceTimersByTimeAsync(MEMORY_INTERVAL_MS * 10);
		expect(a.servicesMemory).toHaveBeenCalledTimes(before);
	});

	// A failed sample must go back to unknown, NOT to zero: "0 MB · no processes"
	// is a specific, wrong claim, whereas "—" is the truth.
	it('returns a figure to unknown when its sample fails', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(s.servicesBytes).toBe(1000);

		(a.servicesMemory as unknown as { mockRejectedValue: (e: unknown) => void }).mockRejectedValue(
			{ kind: 'proc', message: 'gone' }
		);
		await vi.advanceTimersByTimeAsync(MEMORY_INTERVAL_MS);
		expect(s.servicesBytes).toBeNull();
		expect(s.processCount).toBeNull();
		s.stop();
	});

	// A failed FIRST home reading is a failure, not "still measuring" — otherwise
	// the strip says "measuring…" forever.
	it('clears homePending even when the first home reading fails', async () => {
		const s = new StatsStore(
			api({
				homeDiskUsage: vi.fn(async () => {
					throw { kind: 'core', message: 'nope' };
				})
			})
		);
		s.start();
		await vi.advanceTimersByTimeAsync(0);
		expect(s.homePending).toBe(false);
		expect(s.homeBytes).toBeNull();
		s.stop();
	});

	// start() twice (a dev-HMR double mount) must not double the polling rate.
	it('is idempotent across a second start', async () => {
		const a = api();
		const s = new StatsStore(a);
		s.start();
		s.start();
		await vi.advanceTimersByTimeAsync(MEMORY_INTERVAL_MS * 3);
		expect(a.servicesMemory).toHaveBeenCalledTimes(4); // 1 immediate + 3 ticks
		s.stop();
	});
});
```

- [ ] **Step 6: Run them to verify they fail**

Run: `pnpm -C apps/desktop test src/lib/stats.svelte.test.ts 2>&1 | tail -10`
Expected: FAIL — cannot resolve `./stats.svelte`.

- [ ] **Step 7: Write the store**

Create `apps/desktop/src/lib/stats.svelte.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// Status-bar figures and their polling.
//
// Two independent cadences, because the two readings cost wildly different
// amounts: services memory is one syscall per pid, while the home figure is a
// directory walk (measured at 40 ms over 6,470 files). Sampling them together
// would either throttle the cheap one or hammer the disk with the expensive one.
//
// `null` means UNKNOWN and is never coerced to 0 anywhere in this file. A zero
// is a specific claim — "nothing is running" — and rendering a failed sample as
// zero would state it falsely.
//
// DOM-free on purpose: `start()`/`stop()` are called by the layout, which owns
// the `visibilitychange` listener. That keeps this class unit-testable with fake
// timers and no jsdom.
import type { HomeUsageDto, IpcError, ServicesMemoryDto } from './ipc';

export interface StatsApi {
	servicesMemory(): Promise<ServicesMemoryDto>;
	homeDiskUsage(): Promise<HomeUsageDto>;
}

/** Services memory: one syscall per pid, so it can feel live. */
export const MEMORY_INTERVAL_MS = 2000;
/** Home size: a directory walk, so it is deliberately rare. */
export const HOME_INTERVAL_MS = 60000;

export class StatsStore {
	/** Bytes, or `null` for unknown. Never 0 as a stand-in for unknown. */
	servicesBytes = $state<number | null>(null);
	processCount = $state<number | null>(null);
	homeBytes = $state<number | null>(null);
	/**
	 * True until the first home reading SETTLES, either way. Lets the strip say
	 * "measuring…" for a walk in progress while still showing "—" for one that
	 * failed — two different things that would otherwise look identical.
	 */
	homePending = $state(true);
	/** Last sampling error, for diagnostics only. The strip renders "—", never this. */
	lastError = $state<IpcError | null>(null);

	private memoryTimer: ReturnType<typeof setInterval> | null = null;
	private homeTimer: ReturnType<typeof setInterval> | null = null;

	constructor(private api: StatsApi) {}

	async refreshMemory(): Promise<void> {
		try {
			const r = await this.api.servicesMemory();
			this.servicesBytes = r.bytes;
			this.processCount = r.processCount;
		} catch (e) {
			// Back to unknown, not to zero — see the file header.
			this.servicesBytes = null;
			this.processCount = null;
			this.lastError = e as IpcError;
		}
	}

	async refreshHome(): Promise<void> {
		try {
			this.homeBytes = (await this.api.homeDiskUsage()).bytes;
		} catch (e) {
			this.homeBytes = null;
			this.lastError = e as IpcError;
		} finally {
			// `finally`, so a failed FIRST reading stops claiming "measuring…"
			// forever.
			this.homePending = false;
		}
	}

	/**
	 * Begin polling. Idempotent: a second call while already running is a no-op
	 * rather than a second set of timers, because a dev-HMR double mount would
	 * otherwise silently double the sampling rate.
	 */
	start(): void {
		if (this.memoryTimer !== null) return;
		void this.refreshMemory();
		void this.refreshHome();
		this.memoryTimer = setInterval(() => void this.refreshMemory(), MEMORY_INTERVAL_MS);
		this.homeTimer = setInterval(() => void this.refreshHome(), HOME_INTERVAL_MS);
	}

	/** Stop polling. Safe to call when not started. */
	stop(): void {
		if (this.memoryTimer !== null) clearInterval(this.memoryTimer);
		if (this.homeTimer !== null) clearInterval(this.homeTimer);
		this.memoryTimer = null;
		this.homeTimer = null;
	}
}
```

Create `apps/desktop/src/lib/stats.shared.svelte.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
// The app's ONE StatsStore, wired to the real IPC layer.
//
// Shared for the same reason `services.shared.svelte.ts` is: the status bar is
// rendered by AppShell on every route, so a per-page instance would restart the
// timers on every navigation and throw away the home figure each time.
//
// Kept out of `stats.svelte.ts` so that module stays a pure, api-injected store
// whose tests hand it a fake.
import { homeDiskUsage, servicesMemory } from './ipc';
import { StatsStore } from './stats.svelte';

export const statsStore = new StatsStore({ servicesMemory, homeDiskUsage });
```

- [ ] **Step 8: Run the store tests to verify they pass**

Run: `pnpm -C apps/desktop test src/lib/stats.svelte.test.ts 2>&1 | grep -E "Tests "`
Expected: `Tests  7 passed (7)`.

- [ ] **Step 9: Verify each test can fail**

| Mutation | Must fail |
|---|---|
| `formatBytes` uses `1000` instead of `1024` for `STEP` | `steps through 1024-based units…` |
| `formatBytes` always uses `toFixed(1)` | `steps through 1024-based units…` (`10 KB` becomes `10.0 KB`) |
| `formatBytes(0)` falls through to the loop | `renders zero as 0 MB` (gives `0 B`) |
| `formatProcessCount` always appends `processes` | `agrees in number…` |
| `refreshMemory`'s catch sets `0` instead of `null` | `returns a figure to unknown when its sample fails` |
| move `this.homePending = false` from `finally` into the `try` | `clears homePending even when the first home reading fails` |
| drop the `if (this.memoryTimer !== null) return;` guard in `start` | `is idempotent across a second start` |
| `stop()` clears only `memoryTimer` | `issues no further calls after stop` |
| give both timers `MEMORY_INTERVAL_MS` | `samples memory 30x more often than the home size` |

Record every outcome in the task report.

- [ ] **Step 10: Run the frontend gate**

```bash
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all four pass, `check` reporting 0 errors and 0 warnings.

- [ ] **Step 11: Commit**

```bash
git add apps/desktop/src/lib/stats.derive.ts apps/desktop/src/lib/stats.derive.test.ts apps/desktop/src/lib/stats.svelte.ts apps/desktop/src/lib/stats.svelte.test.ts apps/desktop/src/lib/stats.shared.svelte.ts
git commit -s -m "feat(ui): status-bar figures, formatting, and their two cadences

StatsStore polls services memory every 2s and the home size every 60s -
independently, because one is a syscall per pid and the other is a directory
walk; sampling them together would throttle the cheap one or hammer the disk.

null means unknown and is never coerced to 0. A failed sample rendering as
'0 MB - no processes' would be a specific, false claim; '—' is the truth.
homePending is cleared in a finally so a failed FIRST home reading stops saying
'measuring…' forever.

start() is idempotent - a dev-HMR double mount would otherwise silently double
the sampling rate. The store is DOM-free; the layout owns visibilitychange."
```

---

### Task 5: The strip, the shell row, and the visibility wiring

Read spec §1, §4.4 and §6 first. Placement was decided visually: full width beneath both rail and content, because it is window-level state.

**Files:**
- Create: `apps/desktop/src/lib/components/StatusBar.svelte`
- Create: `apps/desktop/src/lib/components/StatusBar.svelte.test.ts`
- Modify: `apps/desktop/src/lib/components/AppShell.svelte` (the `.window` grid and the markup)
- Modify: `apps/desktop/src/routes/+layout.svelte` (start/stop on visibility)

**Interfaces:**
- Consumes: `formatBytes`, `formatProcessCount`, `UNKNOWN` from `$lib/stats.derive`; `statsStore` from `$lib/stats.shared.svelte` (Task 4).
- Produces: nothing later tasks depend on. This is the last implementation task.

**Deliberate deviation from spec §4.4.** The spec says the component "takes formatted
values plus explicit unknown states". It instead takes RAW `number | null` values and
formats internally, because the home segment has THREE states — a figure, a walk in
flight, and a failure — and a pre-formatted string cannot express which one it is without
a second flag travelling beside it. Formatting inside also means the SSR tests cover
value → markup end to end instead of only half of it. Recorded here rather than taken
silently.

- [ ] **Step 1: Write the failing component tests**

Create `apps/desktop/src/lib/components/StatusBar.svelte.test.ts`:

```ts
// SPDX-License-Identifier: GPL-3.0-or-later
//
// Rendered server-side (`svelte/server`), so it runs in the existing `node`
// vitest project with no DOM — same approach as SiteDrawer/SiteListRow/QuitDialog.
//
// WHAT THIS FILE CANNOT COVER: there is no DOM, so the polling itself and the
// pause-on-hidden wiring are out of reach here. Those live in
// `stats.svelte.test.ts` (with fake timers) and in the PR's manual click-through.
import { describe, expect, it } from 'vitest';
import { render } from 'svelte/server';
import StatusBar from './StatusBar.svelte';

function html(props: {
	servicesBytes?: number | null;
	processCount?: number | null;
	homeBytes?: number | null;
	homePending?: boolean;
}): string {
	return render(StatusBar, {
		props: {
			servicesBytes: props.servicesBytes ?? null,
			processCount: props.processCount ?? null,
			homeBytes: props.homeBytes ?? null,
			homePending: props.homePending ?? false
		}
	}).body;
}

function text(markup: string): string {
	return markup
		.replace(/<[^>]*>/g, '')
		.replace(/\s+/g, ' ')
		.trim();
}

describe('StatusBar', () => {
	it('shows all three segments when everything is known', () => {
		const t = text(html({ servicesBytes: 89128960, processCount: 2, homeBytes: 1288490188 }));
		expect(t).toContain('services 85 MB');
		expect(t).toContain('2 processes');
		expect(t).toContain('~/.openvhost');
		expect(t).toContain('1.2 GB');
	});

	// The failure mode this guards: a failed sample must not read as a measured
	// zero. "0 MB · no processes" is a specific claim; "—" is the truth.
	it('renders unknown figures as a dash, never as zero', () => {
		const t = text(html({ servicesBytes: null, processCount: null, homeBytes: null }));
		expect(t).toContain('—');
		expect(t).not.toContain('0 MB');
		expect(t).not.toContain('no processes');
	});

	it('says measuring while the first home walk is in flight', () => {
		const t = text(html({ homeBytes: null, homePending: true }));
		expect(t).toContain('measuring');
		// "measuring…" and "—" are different states and must not both show.
		expect(t).not.toMatch(/~\/\.openvhost\s+—/);
	});

	it('reports an idle app as a real zero, not as unknown', () => {
		const t = text(html({ servicesBytes: 0, processCount: 0, homeBytes: 1024 }));
		expect(t).toContain('0 MB');
		expect(t).toContain('no processes');
	});

	// A screen reader should not have this re-announced every 2 seconds.
	it('is a labelled, non-live region', () => {
		const m = html({ servicesBytes: 1024, processCount: 1, homeBytes: 1024 });
		expect(m).toContain('aria-label="Resource usage"');
		expect(m).not.toContain('aria-live');
	});
});
```

- [ ] **Step 2: Run them to verify they fail**

Run: `pnpm -C apps/desktop test src/lib/components/StatusBar.svelte.test.ts 2>&1 | tail -10`
Expected: FAIL — cannot resolve `./StatusBar.svelte`.

- [ ] **Step 3: Write the component**

Create `apps/desktop/src/lib/components/StatusBar.svelte`:

```svelte
<!-- SPDX-License-Identifier: GPL-3.0-or-later -->
<script lang="ts">
	import { UNKNOWN, formatBytes, formatProcessCount } from '$lib/stats.derive';

	let {
		servicesBytes,
		processCount,
		homeBytes,
		homePending = false
	}: {
		/** `null` = unknown. Never pass 0 to mean unknown. */
		servicesBytes: number | null;
		processCount: number | null;
		homeBytes: number | null;
		/** True only while the FIRST home walk is in flight. */
		homePending?: boolean;
	} = $props();

	const memory = $derived(servicesBytes === null ? UNKNOWN : formatBytes(servicesBytes));
	const processes = $derived(processCount === null ? UNKNOWN : formatProcessCount(processCount));
	// Three states, not two: a walk in progress is not a failure, and saying "—"
	// for it would be as wrong as saying "measuring…" for a read that failed.
	const home = $derived(
		homeBytes !== null ? formatBytes(homeBytes) : homePending ? 'measuring…' : UNKNOWN
	);
</script>

<!-- No `aria-live`: this updates every 2 seconds and a live region would have a
     screen reader announce resource figures over whatever the user is doing. It is
     a labelled region they can visit deliberately instead. -->
<div class="statusbar" aria-label="Resource usage" data-testid="statusbar">
	<span>services <span class="num">{memory}</span></span>
	<span class="sep" aria-hidden="true">·</span>
	<span class="num">{processes}</span>
	<span class="sep" aria-hidden="true">·</span>
	<span class="mono">~/.openvhost</span>
	<span class="num">{home}</span>
</div>

<style>
	/* Ported from docs/design/mock.css's `.statusline` (flex row, --vh-space-4 gaps,
	   --vh-text-2, --vh-text-caption, values in .num/.mono), promoted from the log
	   viewer's pane-level strip to window level. Two adaptations: a fixed height and
	   a `border-top`, because at window level it is a chrome edge rather than a
	   trailing line inside a scrolling pane, and horizontal-only padding since the
	   height now does the vertical spacing. */
	.statusbar {
		display: flex;
		align-items: center;
		gap: var(--vh-space-4);
		height: 26px;
		padding: 0 var(--vh-space-6);
		border-top: 1px solid var(--vh-border);
		color: var(--vh-text-2);
		font-size: var(--vh-text-caption);
		/* The strip must never be the reason the window scrolls: it is a fixed grid
		   row, and a long value ellipsizes rather than pushing the row wider. */
		white-space: nowrap;
		overflow: hidden;
	}
	.statusbar .mono {
		font-family: var(--vh-font-mono);
	}
	/* Values in the app's foreground colour against the muted labels, so the eye
	   lands on the numbers. `.num` is the global tabular-nums utility from
	   tokens.css — redeclaring the colour here does not override that. */
	.statusbar .num {
		color: var(--vh-text);
	}
	/* Decorative separator: `aria-hidden` in the markup, and the faintest colour
	   here so it reads as punctuation rather than content. */
	.statusbar .sep {
		color: var(--vh-border);
	}
</style>
```

- [ ] **Step 4: Run them to verify they pass**

Run: `pnpm -C apps/desktop test src/lib/components/StatusBar.svelte.test.ts 2>&1 | grep -E "Tests "`
Expected: `Tests  5 passed (5)`.

- [ ] **Step 5: Add the shell row**

In `apps/desktop/src/lib/components/AppShell.svelte`, add the import:

```svelte
	import StatusBar from './StatusBar.svelte';
	import { statsStore } from '$lib/stats.shared.svelte';
```

Add the strip as the last child of `.window`, after the closing `</div>` of `.shell`:

```svelte
	<StatusBar
		servicesBytes={statsStore.servicesBytes}
		processCount={statsStore.processCount}
		homeBytes={statsStore.homeBytes}
		homePending={statsStore.homePending}
	/>
```

Change `.window`'s grid rows from `auto 1fr` to `auto 1fr auto` and extend the comment:

```css
	/* `auto 1fr auto`: titlebar, the shell, and the status strip. The strip is a
	   THIRD ROW rather than a child of `.content` because it reports window-level
	   state — putting it inside `.content` (the one scrolling region) would make it
	   scroll away with the page and read as part of it. */
	.window {
		display: grid;
		grid-template-rows: auto 1fr auto;
		height: 100%;
		width: 100%;
		background: var(--vh-bg);
	}
```

Reading the shared store directly here matches the file's existing precedent — it already reads `servicesStore.error` for `ErrorBanner` with a comment explaining why that is the same coupling rather than a new one.

- [ ] **Step 6: Wire start/stop to window visibility**

In `apps/desktop/src/routes/+layout.svelte`, add the import:

```svelte
	import { statsStore } from '$lib/stats.shared.svelte';
```

and add a third `onMount` block after the existing two:

```svelte
	// Sampling is paused whenever the window is hidden. The master plan's first
	// principle is "lightweight always-on … idle RAM budget for the app itself
	// < 100 MB. This is why Tauri was chosen over Electron" — an app left open
	// behind an IDE all day must cost nothing while nobody is looking at it.
	//
	// The store owns the timers and the layout owns this listener, so the store
	// stays DOM-free and unit-testable with fake timers.
	onMount(() => {
		const sync = () => {
			if (document.visibilityState === 'visible') statsStore.start();
			else statsStore.stop();
		};
		sync();
		document.addEventListener('visibilitychange', sync);
		return () => {
			document.removeEventListener('visibilitychange', sync);
			statsStore.stop();
		};
	});
```

- [ ] **Step 7: Run every gate**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all seven pass, `check` reporting 0 errors and 0 warnings.

- [ ] **Step 8: Verify each component test can fail**

| Mutation | Must fail |
|---|---|
| `servicesBytes === null ? '0 MB' : …` | `renders unknown figures as a dash, never as zero` |
| collapse `home` to `homeBytes !== null ? formatBytes(homeBytes) : UNKNOWN` | `says measuring while the first home walk is in flight` |
| add `aria-live="polite"` to the strip | `is a labelled, non-live region` |
| drop `aria-label="Resource usage"` | `is a labelled, non-live region` |
| swap `processCount === null` for `!processCount` | `renders unknown figures as a dash…` (0 would then render as `—`) and `reports an idle app as a real zero` |

Record every outcome in the task report.

- [ ] **Step 9: Visual proof**

The Tauri GUI cannot be automated in this environment, so verify the strip's
layout the way `ServiceRow` and `QuitDialog` were: build a static mockup that
copies the component's final CSS verbatim, render it at the real content width
(1180 px window − 216 px rail = 964 px), screenshot it, and confirm

- the strip sits flush at the bottom with its `border-top` reading as a chrome edge;
- the three segments and their separators align on one 26 px line;
- the degraded states (`—` for both, `measuring…`) do not change the strip's height.

Attach the screenshot to the task report.

- [ ] **Step 10: Commit**

```bash
git add apps/desktop/src/lib/components/StatusBar.svelte apps/desktop/src/lib/components/StatusBar.svelte.test.ts apps/desktop/src/lib/components/AppShell.svelte apps/desktop/src/routes/+layout.svelte
git commit -s -m "feat(ui): add the status bar to the window

A third grid row in AppShell rather than a child of .content: it reports
window-level state, and inside the one scrolling region it would scroll away with
the page. Styling ported from mock.css's .statusline, promoted from the log
viewer's pane-level strip to window level.

Three states for the home figure, not two: a walk in progress is not a failure,
so 'measuring…' and '—' are distinct. Unknown never renders as 0 MB - that
would be a specific, false claim about an idle app.

No aria-live: the figures update every 2 seconds and a live region would
announce them over whatever the user is doing.

Sampling pauses whenever the window is hidden, per the plan's lightweight
always-on principle. The store owns the timers, the layout owns the listener."
```

---

### Task 6: PR

**Files:** none — this task only runs commands.

**Interfaces:** consumes the five committed tasks.

- [ ] **Step 1: Confirm the branch and re-run every gate from a clean state**

```bash
git rev-parse --abbrev-ref HEAD
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace 2>&1 | grep -E "^test result" | awk '{p+=$4; f+=$6} END {print p" passed, "f" failed"}'
for g in lint check test build; do printf "%-6s " "$g"; pnpm -C apps/desktop $g >/dev/null 2>&1 && echo PASS || echo FAIL; done
```

Expected: 0 failed, four PASS. **If any gate fails, stop and fix it — CI is off, so this is the merge gate.**

- [ ] **Step 2: Push and open the PR**

```bash
git push -u origin HEAD
```

Then open a PR whose body states: what the strip shows and why it is our footprint rather than the machine's; the three measured facts from spec §3 (bytes not pages, `rc <= 0` ambiguity, the 40 ms walk); the symlink double-count the walk avoids and the test that proves it; the u64→number bigint check; the two cadences and the pause-on-hidden rule; the full gate results with test counts; the mutation table from every task; and the owed human click-throughs:

- [ ] Strip visible on all three routes (Sites, Services, Web server).
- [ ] Start nginx → the memory figure rises and the count goes to `1 process` within ~2 s.
- [ ] Stop everything → `services 0 MB · no processes` (a real zero, not `—`).
- [ ] Home figure appears within a second of launch, having said `measuring…` first.
- [ ] Hide the window (Cmd+H or another app in front) for a minute → on return the figures resume; nothing was sampled meanwhile.
- [ ] Strip does not scroll with the page, and does not make the window scroll horizontally.

Also flag in the PR: this pulls a Phase 2 roadmap item into Phase 1 (spec §8), and it adds two IPC commands, so **golden rule 2 wants a security-auditor APPROVE before merge** — the owner waived that gate for PR #19 but that waiver was explicitly per-PR, not standing.

---

## Notes for the implementer

- **`bindings.ts` is generated.** Never hand-edit it. Regenerate with
  `cargo test -p openvhost-desktop export_bindings`.
- **`.num` and `.mono` are global utilities** from `apps/desktop/src/lib/styles/tokens.css`
  (`.num { font-variant-numeric: tabular-nums }`, `code, pre, .mono { font-family: var(--vh-font-mono) }`).
  Do not redeclare those properties in a component; add only what the component changes.
- **Two `tokens.css` files exist** — `apps/desktop/src/lib/styles/tokens.css` (the app's, the
  one that matters) and `docs/design/tokens.css` (the mockups'). They are NOT identical.
  Use the app's for anything shipping.
- **Worktrees have their own `node_modules`.** If working in `.claude/worktrees/*`, run
  `pnpm install --offline --frozen-lockfile` first or the desktop gates fail with a bogus
  "Cannot find package".
- If a mutation in a "verify each test can fail" step does **not** make the named test
  fail, that is a finding: the test does not cover what it claims. Fix the test, re-run,
  and report both.
