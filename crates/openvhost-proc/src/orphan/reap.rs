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
#[cfg(unix)]
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

/// P0-8 Task 4's real caller: `Supervisor::with_orphan_cleanup` (and
/// `Supervisor::new`, which delegates to it).
#[cfg(unix)]
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
            if let Err(e) = registry.remove(&rec.service_id) {
                tracing::warn!(service_id = %rec.service_id, error = %e,
                    "orphan reap: failed to remove rejected record");
            }
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
                // Leader gone. Probe the group: while it is non-empty, POSIX
                // keeps the pgid reserved — no OTHER group can be freshly
                // assigned this exact number while current members hold it.
                // That is narrower than "these members are ours"; see the
                // honest residual-risk note on the kill below.
                // SAFETY: signal 0 to the group probes existence only.
                let group_alive = unsafe { libc::kill(-(pid as libc::pid_t), 0) == 0 };
                if group_alive {
                    // SAFETY: plain kill syscall targeting a pgid just
                    // confirmed to exist by the probe above; no memory
                    // handed over.
                    //
                    // RESIDUAL RISK (stated honestly, not a guarantee): POSIX
                    // guarantees only that a LIVE group's id is not
                    // concurrently reassigned to a NEW, unrelated group — it
                    // does NOT establish that the surviving members belong to
                    // OUR service. We reach this branch because the recorded
                    // leader pid no longer exists, so — unlike every other
                    // kill in this file — we have NO start-time identity
                    // evidence here: the leader pid could have been reused by
                    // an unrelated process that itself became a group
                    // leader, spawned children, and then also died, leaving a
                    // headless group indistinguishable from ours by pid
                    // alone. This branch accepts that residual risk in order
                    // to reclaim genuinely leaked workers; tightening it
                    // (e.g. requiring corroborating evidence on survivors) is
                    // a pending policy decision — not something this comment
                    // fix changes.
                    let _ = unsafe { libc::kill(-(pid as libc::pid_t), libc::SIGKILL) };
                    tracing::info!(service_id = %rec.service_id, pid, decision = "killed-group-headless",
                        "orphan reap: dead leader, killed surviving group members");
                    report.killed_headless += 1;
                } else {
                    tracing::info!(service_id = %rec.service_id, pid, decision = "dead-removed",
                        "orphan reap: process already gone");
                    report.skipped_dead += 1;
                }
                if let Err(e) = registry.remove(&rec.service_id) {
                    tracing::warn!(service_id = %rec.service_id, error = %e,
                        "orphan reap: failed to remove dead-leader record");
                }
            }
            Ok(Some(now)) if now != rec.identity.start_time => {
                tracing::info!(service_id = %rec.service_id, pid, decision = "reused-not-killed",
                    "orphan reap: pid reused by an unrelated process; NOT killing");
                report.skipped_reused += 1;
                if let Err(e) = registry.remove(&rec.service_id) {
                    tracing::warn!(service_id = %rec.service_id, error = %e,
                        "orphan reap: failed to remove stale reused-pid record");
                }
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
                if let Err(e) = registry.remove(&rec.service_id) {
                    tracing::warn!(service_id = %rec.service_id, error = %e,
                        "orphan reap: failed to remove record after reap attempt");
                }
            }
        }
    }
    report
}

#[cfg(not(unix))]
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

    /// Spawn a second long-lived process that explicitly JOINS an existing
    /// group (`leader_pid`) rather than starting a new one (F6, for the
    /// headless-group-kill test): the deterministic equivalent of a second
    /// job backgrounded into the same pgid, without any shell-parsing or
    /// synchronization. `process_group(pgid)` with a non-zero, already-live
    /// pgid is a pre-exec `setpgid(0, pgid)` (join), vs. `process_group(0)`'s
    /// "become your own leader" used by `spawn_group_leader` above.
    #[allow(clippy::zombie_processes)] // see spawn_group_leader's note; reaped by raw pid via KillOnDrop below
    fn spawn_group_member(leader_pid: u32) -> u32 {
        use std::os::unix::process::CommandExt;
        let child = Command::new("/bin/sleep")
            .arg("300")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(leader_pid as i32) // join the LEADER's existing group
            .spawn()
            .unwrap();
        child.id()
    }

    /// RAII guard for tests that spawn a multi-process group (F6): drop
    /// unconditionally SIGKILLs the whole group and best-effort reaps every
    /// listed pid, even if an assertion panics partway through the test — a
    /// failing assertion must never leak a live fixture process.
    struct KillOnDrop {
        pgid: libc::pid_t,
        pids: Vec<u32>,
    }

    impl Drop for KillOnDrop {
        fn drop(&mut self) {
            // SAFETY: `pgid` is our own test fixture's group id; plain kill
            // syscall; ESRCH (group already empty/gone) is expected and fine.
            unsafe {
                libc::kill(-self.pgid, libc::SIGKILL);
            }
            for pid in &self.pids {
                // SAFETY: pid is our own direct child spawned by this test.
                // Blocking waitpid is safe: either the target was just sent
                // SIGKILL (unblockable/unignorable, so this returns as soon
                // as the kernel finishes the exit) or it's already reaped
                // (returns immediately with ECHILD) — no hang path either way.
                unsafe {
                    libc::waitpid(*pid as libc::pid_t, std::ptr::null_mut(), 0);
                }
            }
        }
    }

    fn alive(pid: u32) -> bool {
        // SAFETY: signal 0 probes existence without delivering a signal.
        unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
    }

    /// Distinguishes a genuinely RUNNING process from a ZOMBIE (F1) — unlike
    /// `alive()` above (`kill(pid, 0) == 0`), which returns `true` for a
    /// zombie too: a zombie's `kinfo_proc` entry (and hence `kill(pid, 0)`)
    /// persists until something reaps it. Empirically confirmed on this
    /// macOS box (spawn `/bin/sleep 120` as a group leader, `kill(-pid,
    /// SIGKILL)`, sleep 300ms): `alive(pid)` is STILL true and `ps` shows
    /// `STAT=Z <defunct>`. `waitpid(pid, WNOHANG)` returns `0` only while the
    /// child is genuinely still running (no state change to report yet), and
    /// returns `pid` the moment it has exited and become a zombie — even one
    /// we have not reaped yet. Use this, never `alive()`, whenever "is it
    /// really still alive" is the SAFETY assertion itself (i.e. whenever the
    /// process could plausibly have just been killed and not yet
    /// waited-on) — `alive()` remains correct only for checks performed
    /// after an explicit `waitpid`/reap has already run (see
    /// `confirmed_orphan_is_group_killed_and_removed` below).
    fn still_running(pid: u32) -> bool {
        let mut status: libc::c_int = 0;
        // SAFETY: pid is a plain integer parameter; `status` is a valid,
        // writable local; WNOHANG never blocks; we are the real OS-level
        // parent of this pid (spawned directly by this test via
        // `spawn_group_leader`). If the child has in fact exited, this call
        // also reaps it — a side effect we want, since it clears the zombie.
        let ret = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
        ret == 0
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

    /// Poll-reap with a short bounded deadline (F4), instead of the blocking
    /// `waitpid` above. A blocking wait cannot distinguish "confirmed dead,
    /// the kernel just needs a moment to finish the exit" from "the reaper
    /// silently failed and the process is still running": it simply hangs
    /// until the child eventually exits on its own (here, the fixture's full
    /// `sleep` duration) and then reports a spurious PASS. Use this whenever
    /// the wait is standing in for "prove the reaper's kill really landed",
    /// i.e. whenever the process's death is not already guaranteed by a kill
    /// this same test just issued directly (contrast `reap_test_child`
    /// above, which follows a kill WE issued and so cannot hang).
    fn reap_test_child_bounded(pid: u32, deadline: std::time::Duration) {
        let start = std::time::Instant::now();
        loop {
            let mut status: libc::c_int = 0;
            // SAFETY: pid is a plain integer parameter; `status` is a valid,
            // writable local; WNOHANG never blocks; we are the real OS-level
            // parent of this pid.
            let ret = unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
            if ret != 0 {
                // Reaped (`ret == pid`), or already gone (`ret < 0`, ECHILD).
                return;
            }
            if start.elapsed() >= deadline {
                panic!(
                    "reap_test_child_bounded: pid {pid} did not exit within \
                     {deadline:?} — the reaper likely failed to kill it (a \
                     blocking wait here would otherwise hang for the \
                     fixture's full sleep duration and then PASS spuriously)"
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
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
        // Bounded poll-reap (F4, `reap_test_child_bounded`) rather than a
        // blocking wait: if the reaper had silently failed to actually kill
        // the process, a blocking wait would hang for the fixture's full
        // `sleep 120` and then report a spurious PASS. This also clears the
        // zombie so the liveness check below is trustworthy.
        reap_test_child_bounded(pid, std::time::Duration::from_secs(2));
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
        // F1: `alive()` (`kill(pid, 0) == 0`) also returns true for a
        // ZOMBIE, so it cannot tell "never killed" apart from "wrongly
        // killed, and now a zombie of THIS test process" — this test
        // process never reaps `pid` before this assert, and is the real
        // parent of everything `spawn_group_leader` creates. `still_running`
        // (`waitpid(..., WNOHANG)`) is the predicate that can actually catch
        // that: see its doc comment. This is the single most important
        // safety property in this slice, so it gets the predicate that can
        // actually fail.
        assert!(
            still_running(pid),
            "an innocent reused pid must NOT be killed"
        );
        assert!(
            reg.list_current_boot().unwrap().is_empty(),
            "stale record removed"
        );
        // clean up
        // SAFETY: pid is our own test fixture's group leader (this test's
        // whole point is that reap_orphans must NOT have killed it), still
        // running per the `still_running(pid)` assert just above; plain kill
        // syscall.
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

    /// F6: the headless-group branch (`Ok(None)` + surviving members) had NO
    /// test before this. Construct it directly: a leader plus a SEPARATE
    /// process that joins the leader's group (`spawn_group_member`); kill
    /// ONLY the leader and reap it for real (so `process_start_time` on the
    /// leader genuinely observes `None`, and the member is untouched);
    /// confirm the surviving member gets swept up by the headless-group kill.
    #[test]
    fn killed_headless_group_member_is_reaped() {
        let leader_pid = spawn_group_leader();
        let member_pid = spawn_group_member(leader_pid);
        // Cleans up unconditionally, even if an assertion below panics.
        let _guard = KillOnDrop {
            pgid: leader_pid as libc::pid_t,
            pids: vec![leader_pid, member_pid],
        };

        let st = platform::process_start_time(leader_pid).unwrap().unwrap();

        // Kill ONLY the leader (no `-`, so the rest of the group is
        // untouched) and reap it for real, so it is genuinely gone before
        // `reap_orphans` runs.
        // SAFETY: leader_pid is our own test fixture, a direct child of this
        // process; plain kill syscall targeting the single pid, not the
        // group.
        unsafe {
            libc::kill(leader_pid as libc::pid_t, libc::SIGKILL);
        }
        reap_test_child(leader_pid);
        assert!(
            !alive(leader_pid),
            "leader must be genuinely gone (reaped) before reap_orphans runs"
        );
        assert!(
            alive(member_pid),
            "sanity check on the fixture: member must still be alive"
        );

        let dir = tempfile::tempdir().unwrap();
        let reg = FileRegistry::new(dir.path());
        reg.record(&record("svc", leader_pid, st)).unwrap();
        let rep = reap_orphans(&reg, &*default_reaper());

        assert_eq!(
            rep.killed_headless, 1,
            "dead leader + surviving member must route to killed_headless"
        );
        // Bounded poll-reap (F4) — see reap_test_child_bounded's doc comment.
        reap_test_child_bounded(member_pid, std::time::Duration::from_secs(2));
        assert!(
            !alive(member_pid),
            "surviving group member must be dead after the headless-group kill"
        );
        assert!(
            reg.list_current_boot().unwrap().is_empty(),
            "record removed"
        );
    }

    /// F6: pure-function tests for `reject_reason` — no processes involved.
    #[test]
    fn reject_reason_flags_unsafe_service_id_charset() {
        let st = crate::orphan::ProcStartTime::Unix { sec: 1, usec: 1 };
        // A pid that safely clears every numeric check (not <= 1, not
        // > i32::MAX, and — bounded by macOS's ~99999 pid space — never our
        // own pid or pgid) so only the charset check can fire.
        let rec = record("svc; rm -rf /", 999_999, st);
        assert_eq!(
            reject_reason(&rec),
            Some("service_id has an unsafe charset")
        );
    }

    #[test]
    fn reject_reason_flags_pid_above_i32_max() {
        let st = crate::orphan::ProcStartTime::Unix { sec: 1, usec: 1 };
        let rec = record("svc", u32::MAX, st);
        assert_eq!(
            reject_reason(&rec),
            Some("pid > i32::MAX (would flip kill(-pid) into kill(+pid))")
        );
    }
}
