# P0-8 — Crash-Orphan Cleanup (openvhost-proc) — Design

- **Date:** 2026-07-23
- **Status:** Approved in brainstorming (3 sections). Three consultations folded in verbatim-by-requirement: **platform-macos-specialist** APPROVE-WITH-CHANGES (empirical — ran `sysctl` + proved group-kill end-to-end on this macOS build), **platform-windows-specialist** APPROVE-WITH-CHANGES (seam shape for the future Job-Object reap), **security-auditor** APPROVE-DESIGN-WITH-REQUIRED-CHANGES (this is a process-KILL path; the auditor found three false-kill paths the identity gate alone does not close). This slice introduces a new cross-platform abstraction (`process_start_time` + `OrphanReaper`), so golden rule 3 required both platform specialists; and because it sends `SIGKILL` based on a file's contents, the security-auditor gate applies at merge.
- **Source of truth:** `docs/OPENVHOST_MASTER_PLAN.md` v1.2 — row **P0-8**: "Orphan cleanup: PID persistence + stale-process kill on restart", owner rust-core-engineer, exit criterion "Kill app hard → relaunch → old services detected & reaped". Plan §304-305: persist PID + start-time; on boot reap orphans ONLY after verifying PID identity via start-time (PIDs get reused).
- **Owner decisions (2026-07-23):** persistence = a **file registry** (`~/.openvhost/run/supervised.json`) behind a `ProcessRegistry` trait — NOT sqlx/state.db (deferred to its own slice; the trait lets state.db swap in later without touching reap logic; divergence from the plan's "state.db" phrasing recorded here per the security consult). macOS-first. Dead-leader-with-surviving-workers → **probe + group-kill** (below).

## 1. Context

The supervisor (P0-3) spawns every service as a process-group leader (`process_group(0)` → `pgid == pid`) and group-signals via `signal_group(pgid, sig) = libc::kill(-pgid, sig)`. If the app is hard-killed, its children (nginx, php-fpm) outlive it (tokio `Child` has no kill-on-drop, per P0-3) and re-parent to launchd — a crash orphan. P0-8 records each running service's identity, and on the next app start reaps confirmed orphans **before** starting anything. The hard part — and the entire risk this slice burns down — is killing ONLY our own orphans: a bare pid is not an identity (PIDs get reused), so every kill is gated on `(pid, start_time)` equality plus the safety machinery the security consult requires. **macOS-first**: the unix path is implemented + validated; the Windows `OrphanReaper` (Job-Object / `TerminateProcess`) and `ProcStartTime::Windows` branch are defined in the seam and unit-shaped, not runtime-tested.

## 2. Goals

1. Persist `(service_id, pid, start_time, boot_id)` when a service is spawned; remove it on clean stop; store in an atomic file registry behind a `ProcessRegistry` trait.
2. On app start, hold a single-instance lock, then reap confirmed crash-orphans — kill ONLY after `(pid, start_time)` identity match, boot-id match, and the validation/safety machinery below; never kill an innocent process.
3. A cross-platform `process_start_time(pid)` (macOS impl) + a platform `OrphanReaper` (macOS group-kill impl); the Windows shapes defined but deferred.
4. Prove the exit criterion headlessly (kill-app-hard → relaunch → orphan reaped, deterministic) and via an instrumented app smoke; prove the safety gate (reused pid → NOT killed).

## 3. Non-goals

sqlx / state.db (own slice) · Site/Service domain model · health checks / restart policy · Windows reap *runtime* (`OrphanReaper` Windows impl + `ProcStartTime::Windows` are defined, unit-shaped, deferred to the Windows-enablement phase) · reaping processes not spawned by this supervisor · graceful SIGTERM for orphans (SIGKILL — an orphan's supervisor is dead, graceful shutdown is meaningless) · per-service graceful DB shutdown paths (later; `service_id` durability is confirmed here so it composes) · the app single-instance UX (a second launch simply skips reap + registration; focusing an existing window is Phase 1).

## 4. Types, registry, reaper

New in `openvhost-proc` (owner rust-core-engineer; platform impls from the specialists' folded findings):

```rust
/// Start-time identity token that defeats PID reuse. TAGGED (not opaque) so a
/// registry written under one OS can never be misread as the other's numeric
/// shape — an unknown/mismatched tag is a hard deserialize error, not a silent
/// wrong-shape compare (Windows consult). Mirrors the crate's existing
/// `#[serde(tag=...)]` idiom (events.rs::ServiceState).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "camelCase")]
pub enum ProcStartTime {
    Unix { sec: i64, usec: i64 },        // macOS kp_proc.p_starttime (fork-time, stable across exec)
    Windows { creation_filetime: u64 },  // GetProcessTimes creation time — DEFINED, deferred
}

/// (pid, start_time) — the reuse-defeating identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcIdentity {
    /// INVARIANT: services are spawned as group leaders (`process_group(0)`),
    /// so `pgid == pid` and `kill(-pid)` targets the whole tree. Re-verified at
    /// reap via `getpgid` — never trusted on the spawn invariant alone.
    pub pid: u32,
    pub start_time: ProcStartTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedRecord {
    pub service_id: String,   // durable service key (NOT an ephemeral runtime id)
    pub identity: ProcIdentity,
    pub recorded_at_ms: u64,  // diagnostics
}

/// Whole-registry snapshot: a boot identity header + records. Boot mismatch on
/// load ⇒ purge all, reap nothing (§5).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistrySnapshot { pub boot_id: BootId, pub records: Vec<SupervisedRecord> }

/// Persistence only — no kill logic. state.db can implement this later.
pub trait ProcessRegistry: Send + Sync {
    fn record(&self, rec: &SupervisedRecord) -> io::Result<()>;   // upsert by service_id
    fn remove(&self, service_id: &str) -> io::Result<()>;
    /// Returns records ONLY if the stored boot_id matches the current boot;
    /// otherwise purges the file and returns empty (§5). Never errors on a
    /// corrupt/oversized file — logs, rotates aside, returns empty.
    fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>>;
}

/// Platform-specific KILL — separate from ProcessRegistry (persistence) and
/// from ProcessDriver::kill (which needs a live SpawnedChild an orphan lacks).
/// Windows consult: an orphan is a bare pid, so the kill can't reuse the driver.
pub trait OrphanReaper: Send + Sync {
    /// Kill the process group of an already-identity-verified orphan. Unix:
    /// getpgid re-check then kill(-pid|pid, SIGKILL). Windows: TerminateProcess
    /// / TerminateJobObject (deferred).
    fn reap(&self, pid: u32) -> io::Result<ReapKind>;   // ReapKind: Group | SinglePidFallback
}
```

**`FileRegistry`** — `~/.openvhost/run/supervised.json`, dir created 0700, file created 0600 (pids/timestamps are not secrets but stay private; some homes are world-readable). Every `record`/`remove` rewrites the whole file **atomically** (temp + rename in the same dir — repo golden rule 4; reuse the `demo_stack.rs` atomic-write pattern, do not invent a new one). Parse caps: file ≤ 64 KiB, ≤ 64 records, flat serde schema (the size cap + serde's recursion limit end the malformed/huge/deeply-nested DoS). Unparseable → rotate aside (`supervised.json.corrupt`) + treat as empty, never abort startup, never act on partially-parsed data.

**Cross-platform split (Windows consult):** the identity CHECK (`process_start_time` equality) is shared; the KILL (`OrphanReaper::reap`) is platform-specific. `process_start_time(pid) -> io::Result<Option<ProcStartTime>>` (`Ok(Some)` = live with start-time, `Ok(None)` = no live process, `Err` = a real error → never treated as "safe to reap").

## 5. Boot-identity gate (security REQUIRED 1a)

The registry stores a `BootId` captured at write time. On `list_current_boot`, if the stored `boot_id` != the current boot → **purge every record and reap nothing** (after a reboot no orphan can exist by definition — every process from the prior boot is gone). This deletes the entire cross-boot false-kill class (macOS `p_starttime` is wall-clock and could alias across a clock step; Linux `starttime` is ticks-since-boot and resets outright). macOS `BootId` = `kern.boottime` (a `timeval`); compare with a small **±5 s tolerance** (boottime shifts slightly on clock steps). Indeterminate boot id → purge, reap nothing. (A `BootId` newtype keeps the platform detail contained; macOS reads `kern.boottime` via sysctl alongside the start-time reader.)

## 6. Reap orchestration — the safety machinery (security REQUIRED)

`reap_orphans(registry, reaper) -> ReapReport`, run synchronously at `Supervisor::new` **before any service is registered or started** (REQUIRED 4 — a tested invariant: if reap could see a record this run just wrote, the start-time would match and it would kill our own fresh child). For each record from `list_current_boot()`:

**Validation floor (REQUIRED 2) — reject before any action, log, drop the record:**
- `pid > 1` — `pid == 0` → `kill(0,…)` signals our OWN group; `pid == 1` → `kill(-1, SIGKILL)` kills *every process the user can signal* (the single worst syscall reachable here, via file corruption or a serde bug, not only tampering).
- `pid <= i32::MAX` — a value that overflows the `u32 → pid_t` cast silently flips `kill(-pid)` into `kill(+pid)`.
- `pid != std::process::id()` and `pid != getpgrp()` — never signal our own pid or our own group (process-suicide / killing our own terminal session), defense-in-depth beyond the start-time gate.
- `start_time` present.
- `service_id` matches the service-id charset (log-injection hygiene).

**The four-way decision table (REQUIRED 6) — an explicit, documented MUST, not implicit:**
1. `process_start_time(pid)` → **`Err`** → do NOT kill, leave/remove per policy, log `error-no-kill`. Every ambiguous outcome resolves to "leak an orphan" (recoverable — port-in-use surfaces to the user), never "kill on doubt".
2. → **`Ok(None)`** (no live process at pid): the leader is gone. **Probe `kill(-pid, 0)`** (owner-approved): if it succeeds, the process group still has surviving members (POSIX keeps the pgid reserved while members exist, so `-pid` still refers to OUR group — the pid cannot have been reused) → `kill(-pid, SIGKILL)` to reap the leaked workers → remove; if `ESRCH`, the group is empty → just remove the record. Log `killed-group-headless` or `dead-removed`.
3. → **`Ok(Some(t))`, `t != recorded`**: pid reused by an unrelated process → do NOT kill → remove the stale record, log `reused-not-killed`.
4. → **`Ok(Some(t))`, `t == recorded`**: confirmed our orphan → **re-verify `getpgid(pid) == pid`** (REQUIRED 3 — don't trust the spawn invariant): if yes → `reaper.reap(pid)` group-SIGKILL (`ReapKind::Group`); if it errs or differs (a service that violated the no-setpgid rule) → single-pid `kill(pid, SIGKILL)` (`ReapKind::SinglePidFallback`) + log the invariant violation → remove. Log `killed-group` / `killed-single-fallback`.

**Contiguity (REQUIRED 5):** the `process_start_time` → `getpgid` → `kill` sequence is synchronous and contiguous — no `.await`, no I/O, no logging flush between the check and the kill (shrinks the TOCTOU window to two syscalls of pure CPU; the residual reuse-in-that-window race is accepted per the P0-6 §8 threat model and is no worse than a bare `kill -9 $PID`).

**Audit + canary (REQUIRED 6):** one structured `tracing` line per record — `service_id, pid, recorded vs observed start-time, decision` (one of the labels above). A `kill` returning **EPERM** means the identity gate passed on a process we cannot signal (the gate failed us) → log at `warn` as an invariant violation, never retry. `ESRCH` from the kill is benign (already gone). Best-effort: one record's failure never aborts the sweep. `ReapReport { killed_group, killed_single, killed_headless, skipped_dead, skipped_reused, rejected, errored }` is returned + logged.

## 7. Single-instance lock (security + Windows REQUIRED 1c) — companion

The one sequence the identity gate CANNOT catch: the user launches a **second app instance while the first runs**. Instance B reads instance A's LIVE record — pid, start-time, and boot-id all match (it is genuinely A's process) — and SIGKILLs A's healthy service group. The premise ("this is an orphan") is false, but every gate passes. Mitigation: an **exclusive advisory lock** on `~/.openvhost/run/lock`, acquired at app start and held for the whole process lifetime; **reap runs only after the lock is acquired**. A second instance fails to acquire → skips reap AND registration entirely (it must not touch the registry either). Unix: `libc::flock(LOCK_EX | LOCK_NB)` on a held fd (the fd-close-releases model already used in `openvhost-pkg/src/layout.rs`; reuse it). Windows (deferred, but the seam is real): `LockFileEx(LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY)` or a named mutex checked for `ERROR_ALREADY_EXISTS` — NOT the no-op stub `layout.rs` uses for staging (there the failure is a swept temp dir; here it is a wrong-process kill). The lock is owned by the app/supervisor bootstrap, passed into or acquired around `Supervisor::new`.

## 8. macOS platform implementation (folded from the empirical consult)

`process_start_time(pid)` via one `sysctl(KERN_PROC_PID)` call — **libc has no `kinfo_proc` for macOS**, so read a `libc::timeval` from **byte offset 0** of the raw result buffer (`p_starttime` is offset 0 of `kinfo_proc`, offsetof-verified against the real headers):
- `mib = [CTL_KERN, KERN_PROC, KERN_PROC_PID, pid as pid_t]` (constants exist in libc 0.2, no arch cfg); fixed `[0u8; 1024]` buffer (> the measured 648-byte struct); **skip the two-call "query size first" idiom** — its size-only response returns nonzero even for dead pids and is not an existence signal.
- Map: `rc != 0` → `Err(last_os_error())`; `rc == 0 && len == 0` → `Ok(None)` (dead/nonexistent — verified, NOT `ESRCH`); `rc == 0 && len >= size_of::<timeval>()` → `Ok(Some)`; any other `len` → defensive `Err`.
- `read_unaligned` the `timeval` from the buffer front; `// SAFETY:` documents the 4-element mib, the `len`-byte writable region, null newp (unprivileged read), and the offset-0 field.
- Empirically confirmed: `p_starttime` is fork-time state, unchanged by `exec` or by becoming a zombie; cross-user reads succeed (a reused pid owned by another user just returns a different `t`, and the equality gate handles it); `kern.boottime` reads the same way for the `BootId`. Reject `pid == 0` explicitly (Darwin's `kernel_task` returns `Ok(Some)`, not `None`) — covered by the `pid > 1` floor.

macOS `OrphanReaper::reap(pid)`: `getpgid(pid)`; if `== pid` → `signal_group(pid as i32, SIGKILL)` (promote the P0-3 private `signal_group` to `pub(crate)`); else `kill(pid, SIGKILL)` single. Group-kill safety proven end-to-end by the consult (crash-orphaned leader stays a group leader; re-parenting to launchd changes only ppid, never pgid).

## 9. Supervisor & app wiring

- `Supervisor::new(driver, registry, reaper)` gains the registry + reaper; after the single-instance lock is held, it calls `reap_orphans` once, before anything else.
- **Record at spawn, not at Running** (security RECOMMENDED, adopted): the moment a service is spawned and its pid is known, read that pid's start-time immediately (same-source, raw, per REQUIRED 1b) and `registry.record(...)`. This shrinks the unrecorded-orphan window to ~0 (the P0-3 record-at-Running design left the 500 ms Starting window unrecorded); harmless if the child dies pre-Running (a dead pid reaps to `None`).
- On clean **Stopped/Failed** (we reaped the child), `registry.remove(service_id)`.
- **Desktop app** (`lib.rs`): acquire the single-instance lock at `~/.openvhost/run/lock`; build a `FileRegistry` + `default_reaper()` and pass them to `Supervisor::new` → the real app reaps on startup, satisfying "kill app hard → relaunch → old services detected & reaped". If the lock is already held (second instance), skip supervisor bootstrap.

## 10. Testing

- **Unit (hermetic):** `FileRegistry` round-trip (upsert-by-service_id, atomic temp+rename, corrupt→rotate+empty, >64 KiB / >64 records rejected, 0600/0700), boot-id purge (stale boot_id → empty + file purged), `ProcStartTime`/`ProcIdentity` serde round-trip (both enum variants), the validation floor (pid 0/1/`i32::MAX+1`/own-pid rejected).
- **Reap-logic tests (the RISK — real processes):**
  - *confirmed orphan reaped:* spawn a testchild (group leader) → read start-time → write a record with that identity + current boot_id → `reap_orphans` → assert the group is dead (`kill(pid,0)`→ESRCH) + record removed.
  - *pid reused → NOT killed (safety-critical):* spawn a live process, record its pid with a **deliberately wrong start-time** → reap → assert the process is **still alive** + record removed.
  - *dead pid, no group → removed; dead leader + surviving member → group-killed* (the §6 case-2 probe): assert the surviving member dies.
  - *getpgid mismatch → single-pid fallback* (a process that `setpgid`'d away): assert single-pid kill, not group.
- **Exit-criterion proof (headless, deterministic):** Supervisor A (FileRegistry at a temp home) starts a long-lived testchild → recorded; drop A **without stopping** (child outlives it — the real crash) → construct Supervisor B on the same home → its startup reap kills the orphan → assert dead + registry cleared. Plus a **single-instance test**: hold the lock, construct a second supervisor bootstrap → it does NOT reap and does NOT register.
- **Instrumented app smoke:** real app running the demo service → `pkill -9` the app (registry intact) → relaunch → assert the orphaned service is reaped (`pgrep` clean).
- **Gates:** full local suite (fmt, clippy `-D warnings`, `cargo test --workspace`, `cargo deny`, SPDX, pnpm untouched-but-green) + `cargo check/clippy --target x86_64-pc-windows-msvc -p openvhost-proc` (the Windows seam compiles) + **security-auditor final audit of the diff → written APPROVE (merge gate)**. CI disabled (billing, P0-3 §2.3).

## 11. Delivery

Branch `feat/p08-orphan-cleanup` → SDD per-task → final whole-branch review → **security-auditor diff audit APPROVE** (kill path) → PR with the headless reap proof + app smoke evidence → local gates → merge. No new heavy deps (serde/libc/windows-sys present). Conventional Commits + DCO; SPDX on new source. Divergence recorded: the plan says "state.db"; this slice uses a file registry behind `ProcessRegistry` — state.db implements the trait later.
