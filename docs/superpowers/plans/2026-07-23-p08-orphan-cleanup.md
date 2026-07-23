# P0-8 — Crash-Orphan Cleanup (openvhost-proc) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** On app start the supervisor reaps crash-orphaned services recorded in a file registry, killing ONLY after a `(pid, start_time)` + boot-id identity match and a safety-validation gate — never an innocent process.

**Architecture:** A `ProcessRegistry` (persistence, file-JSON) + `OrphanReaper` (platform kill) + a shared `process_start_time`/`current_boot_id` identity reader; `reap_orphans` runs synchronously at `Supervisor::new` behind a single-instance lock, applying a validation floor and a four-way decision table. macOS-first; the Windows shapes are defined and unit-shaped, not runtime-tested.

**Tech Stack:** Rust 2024, libc (macOS sysctl `KERN_PROC_PID` / `kern.boottime`, `flock`, `getpgid`, `kill`), serde/serde_json, windows-sys (stubs). No new heavy deps.

**Spec:** `docs/superpowers/specs/2026-07-23-p08-orphan-cleanup-design.md` — the three-consult findings there are binding and empirically verified (macOS specialist ran the sysctl + proved group-kill; security-auditor found three false-kill paths). Do NOT weaken the validation floor, the boot gate, the four-way table, the `getpgid` re-check, the contiguous check→kill, or the single-instance lock — each closes a specific false-kill path. The **security-auditor audits this diff as the merge gate** (it is a SIGKILL-from-a-file path).

## Global Constraints

- Branch `feat/p08-orphan-cleanup` off current `main`.
- SPDX `// SPDX-License-Identifier: GPL-3.0-or-later` as line 1 of every new `.rs`.
- No `unwrap()`/`expect()` outside `#[cfg(test)]` (workspace lints warn; use `panic!` for compile-time-constant invariants, module-level allows in tests).
- `openvhost-proc` stays tauri-free. Every `unsafe` block carries a `// SAFETY:` comment.
- Conventional Commits, DCO-signed: always `git commit -s`. NO `Co-Authored-By` trailer.
- Gates each task: `cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh`.
- **Binding safety rules (from the consults — a weakened one is a false-kill path):**
  - Validation floor before any action on a record: `pid > 1`, `pid <= i32::MAX`, `pid != std::process::id()`, `pid != getpgrp()`, `start_time` present, `service_id` charset-clean.
  - Four-way table: `Err` → no-kill; `Ok(None)` → probe `kill(-pid,0)`, group-kill if members survive else remove; start-time mismatch → remove, no-kill; match → `getpgid(pid)==pid` re-check → group-kill else single-pid fallback.
  - Boot-id gate: registry stores a boot id; boot mismatch on load → purge all, reap nothing.
  - Single-instance lock held before reap; `process_start_time` → `getpgid` → `kill` contiguous (no `.await`/I/O between); one structured `tracing` line per record; `kill` EPERM → warn canary, never retry.
  - macOS FFI: read a `libc::timeval` from byte offset 0 of the `KERN_PROC_PID` buffer (libc has no `kinfo_proc` on macOS); one call, fixed 1 KiB buffer, no two-call size query; `rc!=0`→Err, `rc==0 && len==0`→None, `rc==0 && len>=size_of::<timeval>()`→Some.
  - Record at spawn (not at Running); remove on clean stop; reap before any spawn (tested invariant).
- **macOS-first:** unix implemented + validated; Windows `OrphanReaper`/`ProcStartTime::Windows`/lock are stubs that compile (msvc cross-check) but are runtime-deferred.

---

### Task 1: Identity types + platform start-time / boot-id readers

**Files:**
- Create: `crates/openvhost-proc/src/orphan/mod.rs`
- Modify: `crates/openvhost-proc/src/platform/mod.rs` (add cfg-dispatched `process_start_time`, `current_boot_id`)
- Modify: `crates/openvhost-proc/src/platform/unix.rs` (macOS sysctl impls + `getpgid` helper; promote `signal_group` to `pub(crate)`)
- Modify: `crates/openvhost-proc/src/platform/windows.rs` (stubs)
- Modify: `crates/openvhost-proc/src/lib.rs` (`mod orphan;` + re-exports)

**Interfaces produced:**
- `ProcStartTime` (tagged enum), `ProcIdentity{pid,start_time}`, `SupervisedRecord{service_id,identity,recorded_at_ms}`, `BootId`, `RegistrySnapshot{boot_id,records}`.
- `pub(crate) fn platform::process_start_time(pid: u32) -> io::Result<Option<ProcStartTime>>`
- `pub(crate) fn platform::current_boot_id() -> io::Result<BootId>`
- `pub(crate) fn platform::getpgid(pid: u32) -> io::Result<u32>`
- `pub(crate) fn platform::unix::signal_group(pgid: i32, sig: libc::c_int) -> io::Result<()>` (promoted)

- [ ] **Step 1: Branch**

```bash
git checkout main && git pull --ff-only && git checkout -b feat/p08-orphan-cleanup
```

- [ ] **Step 2: Write `orphan/mod.rs` types (+ the test module at the bottom)**

`crates/openvhost-proc/src/orphan/mod.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash-orphan cleanup: persist supervised (pid, start-time, boot-id) and, on
//! the next start, reap confirmed orphans — killing ONLY after an identity
//! match and the safety gate in `reap` (spec §6). This is a SIGKILL-from-a-file
//! path; every check here closes a specific false-kill scenario.

use serde::{Deserialize, Serialize};

/// Start-time identity token that defeats PID reuse. Tagged so a registry
/// written under one OS cannot be misread as the other's numeric shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "camelCase")]
pub enum ProcStartTime {
    Unix { sec: i64, usec: i64 },
    Windows { creation_filetime: u64 },
}

/// Boot identity: registry records are only actionable within the same boot
/// (after a reboot no orphan can exist). macOS uses kern.boottime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "camelCase")]
pub enum BootId {
    Unix { sec: i64, usec: i64 },
    Windows { boot_ms: u64 },
}

impl BootId {
    /// Same boot within a small tolerance (kern.boottime shifts on clock steps).
    pub fn matches(&self, other: &BootId) -> bool {
        match (self, other) {
            (BootId::Unix { sec: a, .. }, BootId::Unix { sec: b, .. }) => (a - b).abs() <= 5,
            (BootId::Windows { boot_ms: a }, BootId::Windows { boot_ms: b }) => {
                a.abs_diff(*b) <= 5000
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcIdentity {
    /// INVARIANT: services spawn as group leaders (`process_group(0)`), so
    /// `pgid == pid` and `kill(-pid)` hits the whole tree. Re-verified via
    /// `getpgid` at reap — never trusted on the spawn invariant alone.
    pub pid: u32,
    pub start_time: ProcStartTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedRecord {
    pub service_id: String,
    pub identity: ProcIdentity,
    pub recorded_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub boot_id: BootId,
    pub records: Vec<SupervisedRecord>,
}
```

(The `ProcessRegistry` trait lands in Task 2, `OrphanReaper`/`reap_orphans` in Task 3 — add `pub(crate) mod registry;` etc. as those tasks land. For now the module is types-only; add `mod orphan;` + `pub use orphan::{BootId, ProcIdentity, ProcStartTime, RegistrySnapshot, SupervisedRecord};` to `lib.rs`.)

- [ ] **Step 3: Write the failing tests for the platform readers**

Append to `crates/openvhost-proc/src/orphan/mod.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn start_time_serde_round_trip_both_variants() {
        for st in [
            ProcStartTime::Unix { sec: 1_700_000_000, usec: 123456 },
            ProcStartTime::Windows { creation_filetime: 133_000_000_000_000_000 },
        ] {
            let j = serde_json::to_string(&st).unwrap();
            assert_eq!(serde_json::from_str::<ProcStartTime>(&j).unwrap(), st);
        }
        // A record written as Unix must NOT silently deserialize as Windows.
        let unix = serde_json::to_string(&ProcStartTime::Unix { sec: 1, usec: 2 }).unwrap();
        assert!(unix.contains("\"os\":\"unix\""));
    }

    #[test]
    fn boot_id_tolerance() {
        let a = BootId::Unix { sec: 1000, usec: 0 };
        assert!(a.matches(&BootId::Unix { sec: 1003, usec: 999 })); // within 5s
        assert!(!a.matches(&BootId::Unix { sec: 1010, usec: 0 })); // beyond 5s
        assert!(!a.matches(&BootId::Windows { boot_ms: 1_000_000 })); // cross-os never
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_time_of_self_is_some_and_stable() {
        let me = std::process::id();
        let a = crate::platform::process_start_time(me).unwrap();
        assert!(a.is_some(), "our own pid must have a start time");
        let b = crate::platform::process_start_time(me).unwrap();
        assert_eq!(a, b, "start time is stable across reads");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_time_of_dead_pid_is_none() {
        // A pid that cannot be alive (max pid space is ~99999 on macOS).
        assert!(crate::platform::process_start_time(999_999).unwrap().is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn current_boot_id_reads_and_self_matches() {
        let b = crate::platform::current_boot_id().unwrap();
        assert!(b.matches(&b));
        // Our own process group id is readable and equals our pgid.
        let pg = crate::platform::getpgid(std::process::id()).unwrap();
        assert!(pg > 0);
    }
}
```

- [ ] **Step 4: Run to verify failure**

Run: `cargo test -p openvhost-proc orphan 2>&1 | tail -5`
Expected: compile errors (`platform::process_start_time` etc. undefined). Implement next.

- [ ] **Step 5: Implement the unix readers in `platform/unix.rs`**

Add to `crates/openvhost-proc/src/platform/unix.rs` (and change `fn signal_group` to `pub(crate) fn signal_group`):

```rust
use crate::orphan::{BootId, ProcStartTime};

/// Read a live process's fork-time start-time via one `sysctl(KERN_PROC_PID)`.
/// libc exposes no `kinfo_proc` on macOS, but `p_starttime` is a `timeval` at
/// byte offset 0 of the result buffer (offsetof-verified). Returns `Ok(None)`
/// for a dead/nonexistent pid (sysctl succeeds with len==0), `Err` on a real
/// error. (macOS consult; empirically verified.)
#[cfg(target_os = "macos")]
pub(crate) fn process_start_time(pid: u32) -> io::Result<Option<ProcStartTime>> {
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(None); // pid 0 is kernel_task; out-of-range can't be ours
    }
    let mut mib = [
        libc::CTL_KERN,
        libc::KERN_PROC,
        libc::KERN_PROC_PID,
        pid as libc::c_int,
    ];
    let mut buf = [0u8; 1024]; // >> sizeof(kinfo_proc) (~648B); over-sizing is harmless
    let mut len = buf.len();
    // SAFETY: mib has exactly 4 elements matching namelen(4); buf/len describe a
    // valid writable region; newp/newlen are null/0 (pure unprivileged read of
    // KERN_PROC_PID). No pointer is retained past the call.
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            buf.as_mut_ptr() as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    if len == 0 {
        return Ok(None); // dead/nonexistent pid (verified — not ESRCH)
    }
    if len < std::mem::size_of::<libc::timeval>() {
        return Err(io::Error::other("KERN_PROC_PID returned an undersized record"));
    }
    // SAFETY: p_starttime is at byte offset 0 of the kinfo_proc blob
    // (offsetof-verified against <sys/proc.h>); read_unaligned avoids any
    // alignment assumption on `buf`.
    let tv = unsafe { (buf.as_ptr() as *const libc::timeval).read_unaligned() };
    Ok(Some(ProcStartTime::Unix {
        sec: tv.tv_sec as i64,
        usec: tv.tv_usec as i64,
    }))
}

/// Current boot time via `sysctl(kern.boottime)` — the boot identity.
#[cfg(target_os = "macos")]
pub(crate) fn current_boot_id() -> io::Result<BootId> {
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let mut tv = libc::timeval { tv_sec: 0, tv_usec: 0 };
    let mut len = std::mem::size_of::<libc::timeval>();
    // SAFETY: mib has 2 elements matching namelen(2); tv is a valid writable
    // timeval of `len` bytes; newp/newlen null/0 (unprivileged read).
    let rc = unsafe {
        libc::sysctl(
            mib.as_mut_ptr(),
            mib.len() as libc::c_uint,
            &mut tv as *mut _ as *mut libc::c_void,
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(BootId::Unix { sec: tv.tv_sec as i64, usec: tv.tv_usec as i64 })
}

/// Process-group id of `pid` (reap re-verifies `getpgid(pid) == pid`).
#[cfg(unix)]
pub(crate) fn getpgid(pid: u32) -> io::Result<u32> {
    // SAFETY: plain syscall, no memory handed over.
    let pg = unsafe { libc::getpgid(pid as libc::pid_t) };
    if pg < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pg as u32)
    }
}
```

Note: `KERN_BOOTTIME` exists in libc 0.2 for macOS. `signal_group` is used by the driver already; only its visibility changes.

- [ ] **Step 6: Implement the cfg dispatch in `platform/mod.rs` + Windows stubs**

In `crates/openvhost-proc/src/platform/mod.rs` add (near the driver dispatch):

```rust
use crate::orphan::{BootId, ProcStartTime};

#[cfg(unix)]
pub(crate) fn process_start_time(pid: u32) -> std::io::Result<Option<ProcStartTime>> {
    unix::process_start_time(pid)
}
#[cfg(windows)]
pub(crate) fn process_start_time(pid: u32) -> std::io::Result<Option<ProcStartTime>> {
    windows::process_start_time(pid)
}

#[cfg(unix)]
pub(crate) fn current_boot_id() -> std::io::Result<BootId> {
    unix::current_boot_id()
}
#[cfg(windows)]
pub(crate) fn current_boot_id() -> std::io::Result<BootId> {
    windows::current_boot_id()
}

#[cfg(unix)]
pub(crate) fn getpgid(pid: u32) -> std::io::Result<u32> {
    unix::getpgid(pid)
}
```

In `crates/openvhost-proc/src/platform/windows.rs` add stubs (defined, runtime-deferred — they must compile for the msvc cross-check):

```rust
use crate::orphan::{BootId, ProcStartTime};

/// Windows start-time via OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) +
/// GetProcessTimes creation time. Deferred to the Windows-enablement phase.
#[cfg(windows)]
pub(crate) fn process_start_time(_pid: u32) -> io::Result<Option<ProcStartTime>> {
    Err(io::Error::other(
        "process_start_time is not implemented on Windows in v1 (macOS-first)",
    ))
}

#[cfg(windows)]
pub(crate) fn current_boot_id() -> io::Result<BootId> {
    Err(io::Error::other(
        "current_boot_id is not implemented on Windows in v1 (macOS-first)",
    ))
}
```

(macOS-only test note: `getpgid` is `#[cfg(unix)]` so it exists on macOS; the Windows reaper in Task 3 provides the Windows kill stub. `current_boot_id`/`process_start_time` returning `Err` on Windows means the app's reap simply finds nothing to do there safely — errors resolve to no-kill.)

- [ ] **Step 7: Run tests, gates, commit**

```bash
cargo test -p openvhost-proc orphan 2>&1 | tail -6
cargo fmt && cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-proc && git commit -s -m "feat(proc): orphan identity types + macOS start-time/boot-id readers"
```

Expected: the macOS-gated readers pass (self start-time stable, dead pid → None, boot id reads); serde round-trips pass.

---

### Task 2: FileRegistry (ProcessRegistry trait)

**Files:**
- Create: `crates/openvhost-proc/src/orphan/registry.rs`
- Modify: `crates/openvhost-proc/src/orphan/mod.rs` (`pub(crate) mod registry;` + `ProcessRegistry` trait)

**Interfaces:**
- Consumes (Task 1): `SupervisedRecord`, `RegistrySnapshot`, `BootId`, `platform::current_boot_id`.
- Produces:
  - `pub trait ProcessRegistry: Send + Sync { fn record(&self, &SupervisedRecord) -> io::Result<()>; fn remove(&self, &str) -> io::Result<()>; fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>>; }`
  - `pub struct FileRegistry { path: PathBuf }` + `FileRegistry::new(run_dir: &Path) -> Self` (file at `run_dir/supervised.json`).

- [ ] **Step 1: Add the trait to `orphan/mod.rs`**

```rust
use std::io;

/// Persistence only — no kill logic. state.db can implement this later.
pub trait ProcessRegistry: Send + Sync {
    fn record(&self, rec: &SupervisedRecord) -> io::Result<()>; // upsert by service_id
    fn remove(&self, service_id: &str) -> io::Result<()>;
    /// Records for the CURRENT boot only; a stale boot_id purges the file and
    /// returns empty. Never errors on a corrupt/oversized file — rotates it
    /// aside and returns empty.
    fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>>;
}

pub(crate) mod registry;
pub use registry::FileRegistry;
```

- [ ] **Step 2: Write the failing tests for `registry.rs`**

Bottom of `crates/openvhost-proc/src/orphan/registry.rs`:

```rust
#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::orphan::{ProcIdentity, ProcStartTime};

    fn rec(id: &str, pid: u32) -> SupervisedRecord {
        SupervisedRecord {
            service_id: id.to_string(),
            identity: ProcIdentity { pid, start_time: ProcStartTime::Unix { sec: 1, usec: pid as i64 } },
            recorded_at_ms: 0,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn record_upsert_remove_list() {
        let dir = tempfile::tempdir().unwrap();
        let r = FileRegistry::new(dir.path());
        r.record(&rec("nginx", 100)).unwrap();
        r.record(&rec("php-fpm", 200)).unwrap();
        r.record(&rec("nginx", 101)).unwrap(); // upsert by service_id
        let mut got = r.list_current_boot().unwrap();
        got.sort_by(|a, b| a.service_id.cmp(&b.service_id));
        assert_eq!(got.len(), 2);
        assert_eq!(got.iter().find(|x| x.service_id == "nginx").unwrap().identity.pid, 101);
        r.remove("nginx").unwrap();
        assert_eq!(r.list_current_boot().unwrap().len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_boot_purges_and_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        // Hand-write a snapshot with a wildly different boot time.
        let snap = RegistrySnapshot {
            boot_id: crate::orphan::BootId::Unix { sec: 1, usec: 0 },
            records: vec![rec("nginx", 100)],
        };
        std::fs::write(&path, serde_json::to_string(&snap).unwrap()).unwrap();
        let r = FileRegistry::new(dir.path());
        assert!(r.list_current_boot().unwrap().is_empty(), "stale boot -> empty");
        // File is purged (no records under the current boot).
        let after: RegistrySnapshot =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after.records.is_empty());
    }

    #[test]
    fn corrupt_file_rotates_and_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        let r = FileRegistry::new(dir.path());
        assert!(r.list_current_boot().unwrap().is_empty());
        assert!(dir.path().join("supervised.json.corrupt").exists(), "rotated aside");
    }

    #[test]
    fn oversized_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        std::fs::write(&path, vec![b'x'; 65 * 1024]).unwrap(); // > 64 KiB cap
        let r = FileRegistry::new(dir.path());
        assert!(r.list_current_boot().unwrap().is_empty());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-proc registry 2>&1 | tail -5`
Expected: compile error (`FileRegistry` undefined).

- [ ] **Step 4: Implement `registry.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! File-backed process registry: one atomic JSON file at
//! `<run>/supervised.json`, boot-gated on load, size/count capped.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::{ProcessRegistry, RegistrySnapshot, SupervisedRecord};
use crate::platform;

const MAX_BYTES: u64 = 64 * 1024;
const MAX_RECORDS: usize = 64;

pub struct FileRegistry {
    path: PathBuf,
}

impl FileRegistry {
    pub fn new(run_dir: &Path) -> Self {
        Self { path: run_dir.join("supervised.json") }
    }

    /// Load the snapshot, applying the boot gate + caps. Never errors on bad
    /// content — rotates aside and returns an empty (current-boot) snapshot.
    fn load(&self) -> io::Result<RegistrySnapshot> {
        let boot = platform::current_boot_id()?;
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(RegistrySnapshot { boot_id: boot, records: vec![] });
            }
            Err(e) => return Err(e),
        };
        if meta.len() > MAX_BYTES {
            self.rotate_corrupt();
            return Ok(RegistrySnapshot { boot_id: boot, records: vec![] });
        }
        let text = std::fs::read_to_string(&self.path)?;
        let snap: RegistrySnapshot = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(_) => {
                self.rotate_corrupt();
                return Ok(RegistrySnapshot { boot_id: boot, records: vec![] });
            }
        };
        if snap.records.len() > MAX_RECORDS || !snap.boot_id.matches(&boot) {
            // Too many records, or a different boot: nothing is actionable.
            // Purge to the current (empty) boot and persist.
            let empty = RegistrySnapshot { boot_id: boot, records: vec![] };
            let _ = self.store(&empty);
            return Ok(empty);
        }
        Ok(snap)
    }

    fn store(&self, snap: &RegistrySnapshot) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("registry path has no parent"))?;
        std::fs::create_dir_all(parent).map_err(|e| io_err("create_dir", parent, e))?;
        set_private_dir(parent);
        let json = serde_json::to_vec_pretty(snap).map_err(io::Error::other)?;
        // Atomic: temp in the same dir, then rename (repo golden rule 4).
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut f = create_private(&tmp)?;
            f.write_all(&json).map_err(|e| io_err("write", &tmp, e))?;
            f.sync_all().map_err(|e| io_err("sync", &tmp, e))?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| io_err("rename", &self.path, e))
    }

    fn rotate_corrupt(&self) {
        let _ = std::fs::rename(&self.path, self.path.with_extension("json.corrupt"));
    }
}

impl ProcessRegistry for FileRegistry {
    fn record(&self, rec: &SupervisedRecord) -> io::Result<()> {
        let mut snap = self.load()?;
        snap.records.retain(|r| r.service_id != rec.service_id); // upsert
        snap.records.push(rec.clone());
        self.store(&snap)
    }
    fn remove(&self, service_id: &str) -> io::Result<()> {
        let mut snap = self.load()?;
        let before = snap.records.len();
        snap.records.retain(|r| r.service_id != service_id);
        if snap.records.len() != before {
            self.store(&snap)?;
        }
        Ok(())
    }
    fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>> {
        Ok(self.load()?.records)
    }
}

fn io_err(op: &'static str, path: &Path, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{op} {}: {source}", path.display()))
}

#[cfg(unix)]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}
#[cfg(not(unix))]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::create(path)
}

#[cfg(unix)]
fn set_private_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
fn set_private_dir(_dir: &Path) {}
```

Add `tempfile = "3"` to `crates/openvhost-proc/Cargo.toml` `[dev-dependencies]` if absent (used by the tests).

- [ ] **Step 5: Run tests to green, gates, commit**

```bash
cargo test -p openvhost-proc registry 2>&1 | tail -6
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-proc Cargo.lock && git commit -s -m "feat(proc): atomic boot-gated file registry for supervised processes"
```

---

### Task 3: OrphanReaper + reap_orphans (the safety machinery)

**Files:**
- Create: `crates/openvhost-proc/src/orphan/reap.rs`
- Modify: `crates/openvhost-proc/src/orphan/mod.rs` (`OrphanReaper` trait, `ReapKind`, `ReapReport`, `reap_orphans`, `pub(crate) mod reap;`)
- Modify: `crates/openvhost-proc/src/platform/unix.rs` (`UnixReaper`)
- Modify: `crates/openvhost-proc/src/platform/mod.rs` (`default_reaper()`)
- Modify: `crates/openvhost-proc/src/platform/windows.rs` (`WindowsReaper` stub)

**Interfaces:**
- Consumes (Tasks 1–2): `ProcessRegistry`, `SupervisedRecord`, `platform::{process_start_time, getpgid}`, `platform::unix::signal_group`.
- Produces:
  - `pub enum ReapKind { Group, SinglePidFallback }`
  - `pub trait OrphanReaper: Send + Sync { fn reap(&self, pid: u32) -> io::Result<ReapKind>; }`
  - `pub fn default_reaper() -> std::sync::Arc<dyn OrphanReaper>`
  - `pub fn reap_orphans(registry: &dyn ProcessRegistry, reaper: &dyn OrphanReaper) -> ReapReport`
  - `pub struct ReapReport { pub killed_group, killed_single, killed_headless, skipped_dead, skipped_reused, rejected, errored: u32 }`

- [ ] **Step 1: Add the trait/types to `orphan/mod.rs`**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapKind {
    Group,
    SinglePidFallback,
}

/// Platform-specific KILL of an identity-verified orphan (a bare pid; an orphan
/// has no live `SpawnedChild`). Unix: getpgid re-check then group/single kill.
pub trait OrphanReaper: Send + Sync {
    fn reap(&self, pid: u32) -> std::io::Result<ReapKind>;
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReapReport {
    pub killed_group: u32,
    pub killed_single: u32,
    pub killed_headless: u32, // dead leader, surviving group members killed
    pub skipped_dead: u32,
    pub skipped_reused: u32,
    pub rejected: u32,
    pub errored: u32,
}

pub(crate) mod reap;
pub use reap::reap_orphans;
```

- [ ] **Step 2: Write the failing RISK tests for `reap.rs`**

Bottom of `crates/openvhost-proc/src/orphan/reap.rs` (these use real child processes — the whole point):

```rust
#[cfg(all(test, target_os = "macos"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::orphan::{FileRegistry, ProcIdentity, ProcessRegistry, SupervisedRecord};
    use crate::platform::{self, default_reaper};
    use std::process::{Command, Stdio};

    // Spawn a long-lived `sleep` as its OWN process-group leader (mirrors the
    // supervisor's process_group(0)); returns its pid.
    fn spawn_group_leader() -> u32 {
        use std::os::unix::process::CommandExt;
        let child = Command::new("/bin/sleep")
            .arg("120")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .unwrap();
        child.id()
    }

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 probes existence without delivering a signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    fn record(id: &str, pid: u32, st: crate::orphan::ProcStartTime) -> SupervisedRecord {
        SupervisedRecord { service_id: id.into(), identity: ProcIdentity { pid, start_time: st }, recorded_at_ms: 0 }
    }

    #[test]
    fn confirmed_orphan_is_group_killed_and_removed() {
        let pid = spawn_group_leader();
        let st = platform::process_start_time(pid).unwrap().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let reg = FileRegistry::new(dir.path());
        reg.record(&record("svc", pid, st)).unwrap();
        let rep = reap_orphans(&reg, &*default_reaper());
        assert_eq!(rep.killed_group, 1);
        // Give the kernel a beat to deliver SIGKILL.
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(!alive(pid), "confirmed orphan must be dead");
        assert!(reg.list_current_boot().unwrap().is_empty(), "record removed");
    }

    #[test]
    fn reused_pid_wrong_start_time_is_never_killed() {
        let pid = spawn_group_leader();
        // Record the LIVE pid but with a deliberately wrong start-time.
        let wrong = crate::orphan::ProcStartTime::Unix { sec: 1, usec: 1 };
        let dir = tempfile::tempdir().unwrap();
        let reg = FileRegistry::new(dir.path());
        reg.record(&record("svc", pid, wrong)).unwrap();
        let rep = reap_orphans(&reg, &*default_reaper());
        assert_eq!(rep.skipped_reused, 1);
        assert!(alive(pid), "an innocent reused pid must NOT be killed");
        assert!(reg.list_current_boot().unwrap().is_empty(), "stale record removed");
        // clean up
        unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
    }

    #[test]
    fn dead_pid_is_removed_not_killed() {
        let pid = spawn_group_leader();
        let st = platform::process_start_time(pid).unwrap().unwrap();
        // Kill it ourselves, then reap the now-dead record.
        unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL); }
        std::thread::sleep(std::time::Duration::from_millis(100));
        let dir = tempfile::tempdir().unwrap();
        let reg = FileRegistry::new(dir.path());
        reg.record(&record("svc", pid, st)).unwrap();
        let rep = reap_orphans(&reg, &*default_reaper());
        assert_eq!(rep.skipped_dead, 1);
        assert!(reg.list_current_boot().unwrap().is_empty());
    }

    #[test]
    fn validation_floor_rejects_dangerous_pids() {
        let dir = tempfile::tempdir().unwrap();
        let reg = FileRegistry::new(dir.path());
        let st = crate::orphan::ProcStartTime::Unix { sec: 1, usec: 1 };
        reg.record(&record("a", 1, st.clone())).unwrap(); // pid 1 -> kill(-1) = catastrophe
        reg.record(&record("b", std::process::id(), st)).unwrap(); // our own pid
        let rep = reap_orphans(&reg, &*default_reaper());
        assert_eq!(rep.rejected, 2, "pid 1 and own-pid must be rejected");
        assert!(reg.list_current_boot().unwrap().is_empty());
    }
}
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p openvhost-proc reap 2>&1 | tail -5`
Expected: compile error (`reap_orphans`/`default_reaper` undefined).

- [ ] **Step 4: Implement the reapers**

`crates/openvhost-proc/src/platform/unix.rs` (append):

```rust
use crate::orphan::{OrphanReaper, ReapKind};

pub(crate) struct UnixReaper;

impl OrphanReaper for UnixReaper {
    /// Re-verify group-leadership at reap time (never trust the spawn invariant
    /// alone): `getpgid(pid) == pid` → group-kill; else single-pid fallback.
    fn reap(&self, pid: u32) -> io::Result<ReapKind> {
        match getpgid(pid) {
            Ok(pg) if pg == pid => {
                signal_group(pid as i32, libc::SIGKILL)?;
                Ok(ReapKind::Group)
            }
            _ => {
                // SAFETY: plain kill syscall; identity already verified upstream.
                let rc = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
                if rc != 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(ReapKind::SinglePidFallback)
            }
        }
    }
}
```

`crates/openvhost-proc/src/platform/windows.rs` (append — stub, deferred):

```rust
use crate::orphan::{OrphanReaper, ReapKind};

pub(crate) struct WindowsReaper;

impl OrphanReaper for WindowsReaper {
    fn reap(&self, _pid: u32) -> io::Result<ReapKind> {
        Err(io::Error::other(
            "orphan reap is not implemented on Windows in v1 (macOS-first)",
        ))
    }
}
```

`crates/openvhost-proc/src/platform/mod.rs` (append):

```rust
use crate::orphan::OrphanReaper;
use std::sync::Arc;

pub fn default_reaper() -> Arc<dyn OrphanReaper> {
    #[cfg(unix)]
    { Arc::new(unix::UnixReaper) }
    #[cfg(windows)]
    { Arc::new(windows::WindowsReaper) }
}
```

- [ ] **Step 5: Implement `reap.rs` orchestration**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! The reap orchestration: for each recorded orphan, apply the validation
//! floor and the four-way decision table (spec §6). Every ambiguous outcome
//! resolves to NOT killing. `process_start_time` → `getpgid` (inside reaper) →
//! `kill` is contiguous (no `.await`/I/O between check and kill).

use super::{OrphanReaper, ProcessRegistry, ReapReport, SupervisedRecord};
use crate::platform;

/// Reject a record before any action. Returns Some(reason) if unsafe.
fn reject_reason(rec: &SupervisedRecord) -> Option<&'static str> {
    let pid = rec.identity.pid;
    if pid <= 1 {
        return Some("pid <= 1 (kill(-1) would signal every process the user can)");
    }
    if pid > i32::MAX as u32 {
        return Some("pid > i32::MAX (would flip kill(-pid) into kill(+pid))");
    }
    if pid == std::process::id() {
        return Some("pid is our own process");
    }
    #[cfg(unix)]
    if let Ok(our_pgid) = platform::getpgid(std::process::id()) {
        if pid == our_pgid {
            return Some("pid is our own process group");
        }
    }
    if rec.service_id.is_empty()
        || !rec.service_id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Some("service_id has an unsafe charset");
    }
    None
}

pub fn reap_orphans(registry: &dyn ProcessRegistry, reaper: &dyn OrphanReaper) -> ReapReport {
    let mut report = ReapReport::default();
    let records = match registry.list_current_boot() {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(error = %e, "orphan reap: could not read registry; skipping");
            return report;
        }
    };
    for rec in records {
        let pid = rec.identity.pid;
        if let Some(reason) = reject_reason(&rec) {
            tracing::warn!(service_id = %rec.service_id, pid, reason, "orphan reap: rejected record");
            report.rejected += 1;
            let _ = registry.remove(&rec.service_id);
            continue;
        }
        // Contiguous from here: read start-time, then (inside reaper) getpgid +
        // kill — no .await or I/O in between.
        match platform::process_start_time(pid) {
            Err(e) => {
                tracing::warn!(service_id = %rec.service_id, pid, error = %e,
                    "orphan reap: start-time read failed; NOT killing");
                report.errored += 1;
                // Leave the record: an error is not proof it's safe to drop.
            }
            Ok(None) => {
                // Leader gone. Probe the group: surviving members (leaked
                // workers) still hold the pgid — POSIX keeps it reserved, so
                // -pid still refers to OUR group and can't have been reused.
                // SAFETY: signal 0 to the group probes existence only.
                let group_alive = unsafe { libc::kill(-(pid as libc::pid_t), 0) == 0 };
                if group_alive {
                    // SAFETY: identity of the leader was our record; POSIX
                    // guarantees the pgid is not reused while members exist.
                    let _ = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
                    tracing::info!(service_id = %rec.service_id, pid, decision = "killed-group-headless",
                        "orphan reap: dead leader, killed surviving group members");
                    report.killed_headless += 1;
                } else {
                    tracing::info!(service_id = %rec.service_id, pid, decision = "dead-removed",
                        "orphan reap: process already gone");
                    report.skipped_dead += 1;
                }
                let _ = registry.remove(&rec.service_id);
            }
            Ok(Some(now)) if now != rec.identity.start_time => {
                tracing::info!(service_id = %rec.service_id, pid, decision = "reused-not-killed",
                    "orphan reap: pid reused by an unrelated process; NOT killing");
                report.skipped_reused += 1;
                let _ = registry.remove(&rec.service_id);
            }
            Ok(Some(_match)) => {
                match reaper.reap(pid) {
                    Ok(super::ReapKind::Group) => {
                        tracing::info!(service_id = %rec.service_id, pid, decision = "killed-group",
                            "orphan reap: confirmed orphan, group-killed");
                        report.killed_group += 1;
                    }
                    Ok(super::ReapKind::SinglePidFallback) => {
                        tracing::warn!(service_id = %rec.service_id, pid, decision = "killed-single-fallback",
                            "orphan reap: pgid != pid invariant violation; single-pid killed");
                        report.killed_single += 1;
                    }
                    Err(e) if e.raw_os_error() == Some(libc::EPERM) => {
                        // Identity gate passed on a process we cannot signal —
                        // the gate failed us. Canary; never retry.
                        tracing::warn!(service_id = %rec.service_id, pid, "orphan reap: EPERM on kill (invariant violation)");
                        report.errored += 1;
                    }
                    Err(e) => {
                        // ESRCH: already gone between check and kill — benign.
                        tracing::info!(service_id = %rec.service_id, pid, error = %e, "orphan reap: kill returned error");
                        report.errored += 1;
                    }
                }
                let _ = registry.remove(&rec.service_id);
            }
        }
    }
    report
}
```

**Note:** `openvhost-proc` needs `tracing` as a dependency. Add `tracing = "0.1"` to `crates/openvhost-proc/Cargo.toml` `[dependencies]` (MIT/Apache; name it in the commit body). If a `tracing` subscriber isn't installed, these events are no-ops at runtime — fine.

- [ ] **Step 6: Run the RISK tests to green**

Run: `cargo test -p openvhost-proc reap -- --nocapture 2>&1 | tail -15`
Expected: `confirmed_orphan_is_group_killed_and_removed`, `reused_pid_wrong_start_time_is_never_killed` (the safety-critical one — the innocent process stays alive), `dead_pid_is_removed_not_killed`, `validation_floor_rejects_dangerous_pids` all pass. No leaked `/bin/sleep` processes (each test cleans up).

- [ ] **Step 7: Gates + commit**

```bash
cargo fmt && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-proc Cargo.lock && git commit -s -m "feat(proc): reap_orphans safety machinery (validation floor, four-way table, group-kill)

Adds tracing (MIT OR Apache-2.0) for the per-record audit log."
```

---

### Task 4: Single-instance lock + supervisor/app wiring + exit-criterion proof

**Files:**
- Create: `crates/openvhost-proc/src/orphan/lock.rs`
- Modify: `crates/openvhost-proc/src/orphan/mod.rs` (`pub(crate) mod lock;` + `pub use lock::InstanceLock;`)
- Modify: `crates/openvhost-proc/src/supervisor.rs` (`Supervisor::new` takes registry + reaper, reaps; Inner holds the registry)
- Modify: `crates/openvhost-proc/src/service_task.rs` (record at spawn; remove on clean stop)
- Modify: `apps/desktop/src-tauri/src/lib.rs` (acquire lock, build registry/reaper, pass to supervisor; skip bootstrap if lock held)
- Create: `crates/openvhost-proc/tests/orphan_reap.rs` (headless exit-criterion + single-instance)

**Interfaces:**
- Consumes: everything from Tasks 1–3.
- Produces: `InstanceLock::acquire(run_dir: &Path) -> io::Result<Option<InstanceLock>>` (`Ok(None)` = already held by another instance).

- [ ] **Step 1: Write `lock.rs`**

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Single-instance advisory lock at `<run>/lock`, held for the process
//! lifetime. Reap MUST run only while this is held (spec §7): otherwise a
//! second live instance would reap the first's HEALTHY services (identity
//! matches — it really is their process — but the "orphan" premise is false).

use std::io;
use std::path::Path;

pub struct InstanceLock {
    _file: std::fs::File, // fd held for lifetime; flock releases on close
}

impl InstanceLock {
    /// `Ok(Some)` = acquired; `Ok(None)` = another instance holds it.
    #[cfg(unix)]
    pub fn acquire(run_dir: &Path) -> io::Result<Option<InstanceLock>> {
        use std::os::unix::io::AsRawFd;
        std::fs::create_dir_all(run_dir)?;
        let path = run_dir.join("lock");
        let file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&path)?;
        // SAFETY: plain flock on a valid fd; LOCK_NB returns EWOULDBLOCK instead
        // of blocking. The lock releases when `file`'s fd closes (on drop).
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            Ok(Some(InstanceLock { _file: file }))
        } else {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }

    /// Windows single-instance is deferred (LockFileEx / named mutex). Failing
    /// closed here would block the macOS-first app on non-unix; returning an
    /// error makes the caller's intent explicit. Not reached on macOS.
    #[cfg(not(unix))]
    pub fn acquire(_run_dir: &Path) -> io::Result<Option<InstanceLock>> {
        Err(io::Error::other(
            "InstanceLock is not implemented on Windows in v1 (macOS-first)",
        ))
    }
}
```

- [ ] **Step 2: Wire the supervisor (record at spawn, reap at new)**

In `crates/openvhost-proc/src/supervisor.rs`: add a `registry: Arc<dyn ProcessRegistry>` field to `Inner`; change `Supervisor::new`:

```rust
pub fn new(
    driver: Arc<dyn ProcessDriver>,
    registry: Arc<dyn crate::orphan::ProcessRegistry>,
    reaper: Arc<dyn crate::orphan::OrphanReaper>,
) -> Self {
    // Reap crash-orphans BEFORE anything is registered/started (safety
    // invariant: never see a record this run just wrote).
    let report = crate::orphan::reap_orphans(&*registry, &*reaper);
    tracing::info!(?report, "supervisor: orphan reap complete");
    let (tx, _) = broadcast::channel(256);
    Self {
        inner: Arc::new(Inner {
            driver,
            entries: Mutex::new(HashMap::new()),
            tx,
            registry,
        }),
    }
}
```

Add a helper `Inner::record_running(&Arc<Inner>, id, pid)` and call it from `service_task.rs` right after the pid is known (after `Inner::set_pid`, line ~74):

```rust
// in service_task.rs, after `Inner::set_pid(&inner, &id, child.id());`
if let Some(pid) = child.id() {
    Inner::record_running(&inner, &id, pid);
}
```

```rust
// in supervisor.rs, on Inner:
pub(crate) fn record_running(inner: &Arc<Inner>, id: &str, pid: u32) {
    // Record identity at SPAWN (start-time read immediately, same source as the
    // reap-time compare). Best-effort: a registry write failure only risks a
    // future leaked orphan, never a wrong kill.
    match crate::platform::process_start_time(pid) {
        Ok(Some(start_time)) => {
            let rec = crate::orphan::SupervisedRecord {
                service_id: id.to_string(),
                identity: crate::orphan::ProcIdentity { pid, start_time },
                recorded_at_ms: now_ms(),
            };
            if let Err(e) = inner.registry.record(&rec) {
                tracing::warn!(service_id = id, pid, error = %e, "failed to record supervised process");
            }
        }
        Ok(None) => { /* already dead; nothing to record */ }
        Err(e) => tracing::warn!(service_id = id, pid, error = %e, "could not read start-time to record"),
    }
}
```

On clean stop/failed (where the child is reaped by us — near the existing `set_state(Stopped/Failed)` + `set_pid(None)`), call `let _ = inner.registry.remove(id);`.

(`now_ms()` already exists in supervisor.rs — reuse it. If the desktop/CLI construct `Supervisor::new` with the old 1-arg signature, update those call sites: the app in Step 4; any test helper to pass a registry+reaper.)

- [ ] **Step 3: Wire the desktop app**

In `apps/desktop/src-tauri/src/lib.rs`'s `.setup(...)`, before building the supervisor:

```rust
use openvhost_proc::{FileRegistry, InstanceLock, default_reaper};

let home = openvhost_core::resolve_home().unwrap_or_else(|_| std::path::PathBuf::from("."));
let run_dir = home.join("run");
match InstanceLock::acquire(&run_dir) {
    Ok(Some(lock)) => {
        // Keep the lock alive for the app's lifetime.
        app.manage(lock);
        let registry = std::sync::Arc::new(FileRegistry::new(&run_dir));
        let supervisor = Arc::new(Supervisor::new(default_driver(), registry, default_reaper()));
        // ... existing register(demo_ticker_spec()) + macOS stack + event bridge ...
        app.manage(supervisor);
    }
    Ok(None) => {
        eprintln!("openvhost: another instance holds the run lock; not starting the supervisor");
    }
    Err(e) => {
        eprintln!("openvhost: failed to acquire the run lock: {e}");
    }
}
```

Re-export `FileRegistry`, `InstanceLock`, `OrphanReaper`, `default_reaper`, `ProcessRegistry` from `openvhost-proc`'s `lib.rs`. Adjust the existing supervisor-construction block to live inside the `Ok(Some(lock))` arm.

- [ ] **Step 4: Write the headless exit-criterion + single-instance tests**

`crates/openvhost-proc/tests/orphan_reap.rs`:

```rust
// SPDX-License-Identifier: GPL-3.0-or-later
//! Exit criterion (master plan P0-8): kill app hard → relaunch → orphan reaped.
//! Modeled headlessly: Supervisor A starts a service and is dropped WITHOUT
//! stopping it (the child outlives it — the real crash); Supervisor B on the
//! same home reaps it at construction.
#![cfg(target_os = "macos")]
#![allow(clippy::unwrap_used)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use openvhost_proc::{
    FileRegistry, InstanceLock, ServiceSpec, ServiceState, SpawnSpec, Supervisor, SupervisorEvent,
    default_driver, default_reaper,
};

fn alive(pid: u32) -> bool {
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

fn sleeper_spec() -> ServiceSpec {
    ServiceSpec {
        id: "orphan-svc".into(),
        display_name: "orphan svc".into(),
        endpoint: None,
        spawn: SpawnSpec {
            program: "/bin/sleep".into(),
            args: ["600"].iter().map(std::ffi::OsString::from).collect(),
            cwd: None,
            env: vec![],
        },
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_app_hard_relaunch_reaps_orphan() {
    let home = tempfile::Builder::new().prefix("ovh-orphan").tempdir_in("/tmp").unwrap();
    let run = home.path().join("run");
    let registry = Arc::new(FileRegistry::new(&run));

    // Supervisor A: start the service, wait for Running (pid recorded).
    let sup_a = Supervisor::new(default_driver(), registry.clone(), default_reaper());
    sup_a.register(sleeper_spec());
    let mut rx = sup_a.subscribe();
    sup_a.start("orphan-svc").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut pid = 0u32;
    while Instant::now() < deadline {
        if let Some(s) = sup_a.snapshot().into_iter().find(|s| s.id == "orphan-svc") {
            if matches!(s.state, ServiceState::Running) {
                pid = s.pid.unwrap();
                break;
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(pid > 0, "service should have reached Running");
    let _ = &mut rx;

    // Hard crash: drop Supervisor A WITHOUT stopping. The child outlives it.
    drop(sup_a);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(alive(pid), "the orphan should survive the supervisor drop");

    // Relaunch: Supervisor B on the same home reaps at construction.
    let _sup_b = Supervisor::new(default_driver(), registry.clone(), default_reaper());
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!alive(pid), "the orphan must be reaped on relaunch");
    assert!(registry.list_current_boot().unwrap().is_empty(), "registry cleared");
}

#[test]
fn second_instance_cannot_acquire_the_lock() {
    let home = tempfile::Builder::new().prefix("ovh-lock").tempdir_in("/tmp").unwrap();
    let run = home.path().join("run");
    let a = InstanceLock::acquire(&run).unwrap();
    assert!(a.is_some(), "first acquires");
    let b = InstanceLock::acquire(&run).unwrap();
    assert!(b.is_none(), "second is refused while the first is held");
    drop(a);
    let c = InstanceLock::acquire(&run).unwrap();
    assert!(c.is_some(), "acquirable again once released");
}
```

Ensure `openvhost-proc`'s `lib.rs` re-exports `FileRegistry`, `InstanceLock`, `default_reaper`, `ProcessRegistry`, `OrphanReaper` (plus the existing `Supervisor`, `default_driver`, etc.). Add `libc` to `[dev-dependencies]` if the integration test needs it directly (it's already a dep).

- [ ] **Step 5: Run the tests + the app builds**

```bash
cargo test -p openvhost-proc --test orphan_reap -- --nocapture 2>&1 | tail -12
cargo build -p openvhost-desktop 2>&1 | tail -3
cargo test -p openvhost-desktop export_bindings 2>&1 | tail -3
```

Expected: `kill_app_hard_relaunch_reaps_orphan` passes (orphan survives the drop, then is reaped by Supervisor B); `second_instance_cannot_acquire_the_lock` passes; the desktop app builds; bindings unchanged (no new commands/events). No leaked `/bin/sleep`.

- [ ] **Step 6: Gates + commit**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && bash scripts/check-spdx.sh
git add crates/openvhost-proc apps/desktop/src-tauri && git commit -s -m "feat(proc): single-instance lock + supervisor/app wiring + exit-criterion proof"
```

---

### Task 5: Windows cross-check, deny gate, PR

**Files:** no production code changes.

- [ ] **Step 1: Windows-seam compile cross-check (macOS-first stand-in)**

```bash
cargo check --target x86_64-pc-windows-msvc -p openvhost-proc 2>&1 | tail -8
cargo clippy --target x86_64-pc-windows-msvc -p openvhost-proc -- -D warnings 2>&1 | tail -8
```

Expected: clean — the Windows stubs (`process_start_time`/`current_boot_id`/`WindowsReaper`/`InstanceLock::acquire` returning errors) compile. If the msvc target is missing: `rustup target add x86_64-pc-windows-msvc`. If a `#[cfg(unix)]`-only symbol (e.g. `getpgid`, `signal_group`) leaks into a Windows build path, fix the cfg gating until clean.

- [ ] **Step 2: License gate**

```bash
cargo deny check licenses advisories 2>&1 | tail -12
```

Expected: exit 0 (`tracing` is MIT/Apache; no other new deps). Record for the PR body.

- [ ] **Step 3: Full local gate suite**

```bash
cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace && cargo deny check licenses advisories && bash scripts/check-spdx.sh && pnpm -C apps/desktop lint && pnpm -C apps/desktop check && pnpm -C apps/desktop test && pnpm -C apps/desktop build
```

Expected: all green; the `orphan_reap` integration test runs (not skips) on this Mac.

- [ ] **Step 4: Push + PR**

```bash
git push -u origin feat/p08-orphan-cleanup
gh pr create --title "feat: P0-8 — crash-orphan cleanup (openvhost-proc)" --body "Implements docs/superpowers/specs/2026-07-23-p08-orphan-cleanup-design.md: the supervisor persists each running service's (pid, start-time, boot-id) to a file registry and, on the next start, reaps crash-orphans — killing ONLY after an identity match and a safety gate (validation floor, boot-id gate, four-way decision table, getpgid re-check, single-instance lock). macOS-first; the Windows OrphanReaper / ProcStartTime::Windows / InstanceLock are defined and compile-checked (x86_64-pc-windows-msvc) but runtime-deferred.

Verification: the safety-critical reused-pid test proves an innocent process is NEVER killed; the headless exit-criterion test proves kill-app-hard → relaunch → orphan reaped; the single-instance lock test proves a second instance cannot reap the first's live services. Full local gates green; cargo deny green (tracing MIT/Apache). CI disabled (billing, P0-3 §2.3).

SECURITY: this is a SIGKILL-from-a-file path — MERGE-BLOCKED pending security-auditor APPROVE of this diff (spec §6/§7 are the audit checklist)."
```

- [ ] **Step 5: Hand back to controller** — final whole-branch review AND the **security-auditor diff audit** (merge gate; kill path). Then the owner-visible proof = the headless reap test + an instrumented app smoke. NOT the implementer's step.

---

## Self-review (controller: verify before dispatching Task 1)

- **Spec coverage:** §4 types (T1) + registry trait (T2) + OrphanReaper (T3); §5 boot gate (T1 BootId + T2 load); §6 safety machinery — validation floor, four-way table, getpgid re-check, contiguous, audit, EPERM canary (T3); §7 single-instance lock (T4); §8 macOS FFI (T1); §9 record-at-spawn + supervisor/app wiring (T4); §10 tests (T2/T3/T4) + gates + msvc cross-check + security-auditor gate (T5). No unmet requirement.
- **Type consistency:** `ProcStartTime`, `BootId`, `ProcIdentity`, `SupervisedRecord`, `RegistrySnapshot`, `ProcessRegistry`, `FileRegistry`, `OrphanReaper`, `ReapKind`, `ReapReport`, `reap_orphans`, `default_reaper`, `InstanceLock`, `process_start_time`, `current_boot_id`, `getpgid`, `signal_group` — consistent across tasks.
- **Known implementer hazards flagged in-plan:** `Supervisor::new` signature change ripples to every call site (desktop app in T4; any existing test helper — the implementer must update them or the workspace won't compile); the `#[cfg(unix)]`/`#[cfg(target_os="macos")]` split on the readers vs the `getpgid`/`signal_group` helpers must keep the Windows build green (T5 cross-check is the catch); `tracing` events are no-ops without a subscriber (fine); the integration tests spawn real `/bin/sleep` and must clean up (each does). The security-auditor audits this diff as the merge gate — the four-way table and validation floor must land exactly as specified.
