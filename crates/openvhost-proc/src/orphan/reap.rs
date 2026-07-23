// SPDX-License-Identifier: GPL-3.0-or-later
//! The reap orchestration: for each recorded orphan, apply the validation
//! floor and the four-way decision table (spec §6). Every ambiguous outcome
//! resolves to NOT killing. `process_start_time` → `getpgid` (inside reaper) →
//! `kill` is contiguous (no `.await`/I/O between check and kill).

// reap_orphans is POSIX process-group semantics end to end; the Windows reaping
// design (Job Objects) is a separate, deferred surface (spec: macOS-first). The
// unix orchestration is cfg-gated with a compiling Windows stub so
// `Supervisor::new` builds on both — the msvc cross-check (Task 5) proves it.
#[cfg(unix)]
use super::SupervisedRecord;
use super::{OrphanReaper, ProcessRegistry, ReapReport};
#[cfg(unix)]
use crate::platform;

/// Reject a record before any action. Returns Some(reason) if unsafe.
///
/// `#[allow(dead_code)]`: only called from `reap_orphans` below, which itself
/// has no production caller until Task 4 (`Supervisor::new`) — see
/// `ReapReport`'s dead_code note in `orphan/mod.rs`.
#[cfg(unix)]
#[allow(dead_code)]
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
    // Collapsed from the brief's nested `if let { if { .. } }` — behavior-
    // identical (same short-circuit: an `Err` from `getpgid` still falls
    // through without rejecting on this check), forced by
    // `clippy::collapsible_if` under `-D warnings`.
    if let Ok(our_pgid) = platform::getpgid(std::process::id())
        && pid == our_pgid
    {
        return Some("pid is our own process group");
    }
    if rec.service_id.is_empty()
        || !rec
            .service_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        return Some("service_id has an unsafe charset");
    }
    None
}

/// `#[allow(dead_code)]`: no production caller until Task 4 wires this into
/// `Supervisor::new` (spec §6/§9) — see `ReapReport`'s dead_code note in
/// `orphan/mod.rs`. The only caller today is the macOS-gated `tests` module
/// at the bottom of this file.
#[cfg(unix)]
#[allow(dead_code)]
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

/// `#[allow(dead_code)]`: see the `#[cfg(unix)]` `reap_orphans` above — no
/// production caller until Task 4.
#[cfg(not(unix))]
#[allow(dead_code)]
pub fn reap_orphans(_registry: &dyn ProcessRegistry, _reaper: &dyn OrphanReaper) -> ReapReport {
    // Windows orphan reaping (Job Objects) is deferred to the Windows-enablement
    // phase (spec: macOS-first). An empty report keeps Supervisor::new compiling
    // on Windows and reaps nothing — the safe default.
    ReapReport::default()
}

#[cfg(all(test, target_os = "macos"))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::orphan::{FileRegistry, ProcIdentity, ProcessRegistry, SupervisedRecord};
    use crate::platform::{self, default_reaper};
    use std::process::{Command, Stdio};

    // Spawn a long-lived `sleep` as its OWN process-group leader (mirrors the
    // supervisor's process_group(0)); returns its pid.
    //
    // `#[allow(clippy::zombie_processes)]`: the `Child` handle is deliberately
    // dropped here — every caller reaps the returned pid itself via
    // `reap_test_child` (raw `libc::waitpid`, not `Child::wait()`), which
    // this lint's local dataflow analysis can't see across the function
    // boundary. Empirically confirmed necessary: on this macOS box, a killed
    // child that is never `waitpid`-ed lingers as a zombie (`ps` shows
    // `STAT=Z`), and a zombie's `kinfo_proc` (and hence
    // `platform::process_start_time`) and `kill(pid, 0)` both keep reporting
    // it as present — which would corrupt the tests below.
    #[allow(clippy::zombie_processes)]
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

    /// Reap the OS-level zombie a kill leaves behind. THIS TEST PROCESS is
    /// the real parent of every child `spawn_group_leader` creates — unlike
    /// production, where the crashed supervisor is gone and the orphan has
    /// already been re-parented to `launchd`, which reaps it for real.
    /// Without this, a killed-but-unwaited child lingers as a zombie, and
    /// BOTH `kill(pid, 0)` (`alive()` above) and the `sysctl(KERN_PROC_PID)`
    /// read backing `process_start_time` keep reporting it as present
    /// (empirically confirmed: `ps` shows `STAT=Z`, `COMMAND=<defunct>`,
    /// zombie's `kinfo_proc`/start-time still readable) — corrupting the
    /// very assertions these tests exist to make. Blocking `waitpid` (no
    /// `WNOHANG`) is safe here: the target has already been sent SIGKILL
    /// (unblockable/unignorable), so this returns as soon as the kernel
    /// finishes the exit; there is no path that hangs.
    fn reap_test_child(pid: u32) {
        // SAFETY: pid is a plain integer parameter; the status pointer is
        // null (we don't care about the exit status); we are the real
        // OS-level parent of this pid (spawned directly by this test via
        // `spawn_group_leader`, never waited on since).
        unsafe {
            libc::waitpid(pid as libc::pid_t, std::ptr::null_mut(), 0);
        }
    }

    fn record(id: &str, pid: u32, st: crate::orphan::ProcStartTime) -> SupervisedRecord {
        SupervisedRecord {
            service_id: id.into(),
            identity: ProcIdentity {
                pid,
                start_time: st,
            },
            recorded_at_ms: 0,
        }
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
        // Reap the zombie (see `reap_test_child`) — this blocks until the
        // kernel finishes the exit, which is what the brief's heuristic
        // "give the kernel a beat" sleep was approximating, and additionally
        // clears the zombie so the liveness check below is trustworthy.
        reap_test_child(pid);
        assert!(!alive(pid), "confirmed orphan must be dead");
        assert!(
            reg.list_current_boot().unwrap().is_empty(),
            "record removed"
        );
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
        assert!(
            reg.list_current_boot().unwrap().is_empty(),
            "stale record removed"
        );
        // clean up
        // SAFETY: pid is our own test fixture's group leader (this test's
        // whole point is that reap_orphans must NOT have killed it), still
        // alive per the `alive(pid)` assert just above; plain kill syscall.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        reap_test_child(pid);
    }

    #[test]
    fn dead_pid_is_removed_not_killed() {
        let pid = spawn_group_leader();
        let st = platform::process_start_time(pid).unwrap().unwrap();
        // Kill it ourselves, then reap the now-dead record.
        // SAFETY: pid is our own test fixture's group leader, spawned moments
        // ago by this same test; plain kill syscall, no memory handed over.
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
        // Reap the zombie so `process_start_time` genuinely observes `None`
        // below (see `reap_test_child`) — a blocking wait is strictly
        // stronger than the brief's heuristic sleep it replaces: without it,
        // the still-zombied pid's kinfo_proc keeps reporting the OLD
        // start-time as present, which would misroute this case into the
        // "confirmed match" branch instead of the "already gone" branch this
        // test exists to exercise.
        reap_test_child(pid);
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
