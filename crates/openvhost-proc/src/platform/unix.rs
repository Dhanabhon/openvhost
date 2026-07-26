// SPDX-License-Identifier: GPL-3.0-or-later
//! Unix driver: containment = own process group set atomically at spawn
//! (`process_group(0)` → posix_spawn attribute; closes the ESRCH race a
//! post-fork setpgid would leave). Signals target the SNAPSHOTTED -pgid.

use std::io;
use std::process::Stdio;

use super::{PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild, assemble_env};
use crate::orphan::{BootId, OrphanReaper, ProcStartTime, ReapKind};

pub(crate) struct UnixDriver;

pub(crate) fn signal_group(pgid: i32, sig: libc::c_int) -> io::Result<()> {
    // SAFETY: plain syscall; no memory handed over.
    let rc = unsafe { libc::kill(-pgid, sig) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

impl ProcessDriver for UnixDriver {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild> {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .env_clear()
            .envs(assemble_env(&spec.env))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let child = cmd.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
        Ok(SpawnedChild {
            child,
            // process_group(0) makes the child the leader: pgid == pid.
            handle: PlatformHandle { pgid: pid as i32 },
        })
    }

    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()> {
        signal_group(child.pgid(), libc::SIGTERM)
    }

    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()> {
        // NEVER tokio's Child::kill() — that signals the direct child only
        // and would orphan grandchildren (spec §5).
        signal_group(child.pgid(), libc::SIGKILL)
    }
}

/// Read a live process's fork-time start-time via one `sysctl(KERN_PROC_PID)`.
/// libc exposes no `kinfo_proc` on macOS, but `p_starttime` is a `timeval` at
/// byte offset 0 of the result buffer (offsetof-verified). Returns `Ok(None)`
/// for a dead/nonexistent pid (sysctl succeeds with len==0), `Err` on a real
/// error. (macOS consult; empirically verified.)
///
/// `#[allow(dead_code)]` dropped (P0-8 Task 3): called via
/// `platform::process_start_time` from the real `reap_orphans` production
/// path now, not just the macOS-gated tests in `orphan::tests`.
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
        return Err(io::Error::other(
            "KERN_PROC_PID returned an undersized record",
        ));
    }
    // SAFETY: p_starttime is at byte offset 0 of the kinfo_proc blob
    // (offsetof-verified against <sys/proc.h>); read_unaligned avoids any
    // alignment assumption on `buf`.
    let tv = unsafe { (buf.as_ptr() as *const libc::timeval).read_unaligned() };
    // NOTE: `tv_sec` (time_t) is already `i64` on 64-bit Darwin (libc 0.2.187:
    // unix/bsd/apple/mod.rs `pub type time_t = c_long;`), so no cast there —
    // clippy::unnecessary_cast flags `tv.tv_sec as i64` as a no-op. `tv_usec`
    // (suseconds_t = i32) genuinely needs the widening cast.
    Ok(Some(ProcStartTime::Unix {
        sec: tv.tv_sec,
        usec: tv.tv_usec as i64,
    }))
}

/// Non-macOS unix (Linux, BSD, ...) start-time read is deferred to the
/// Windows/Linux-enablement phase (spec: macOS-first). Mirrors the Windows
/// stub in `windows.rs` exactly, restoring compilation for `openvhost-proc`
/// on any `#[cfg(unix)]` target other than macOS — `platform::mod`'s
/// `#[cfg(unix)] pub fn process_start_time` dispatches here unconditionally
/// on every unix target, so without this arm the crate failed to compile at
/// all on e.g. Linux (P0-8 merge-gate fix wave C6).
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn process_start_time(_pid: u32) -> io::Result<Option<ProcStartTime>> {
    Err(io::Error::other(
        "process_start_time is not implemented on this platform in v1 (macOS-first)",
    ))
}

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
    // Kept even though it is unfalsifiable through this function's own tests
    // on this machine: `proc_pidinfo` happens to answer both excluded values
    // (pid 0 -> EPERM, the wrapped out-of-range value below -> ESRCH) with
    // `rc <= 0`, which the general fallback further down already turns into
    // `Ok(None)` — so deleting this guard leaves every test in `rss_tests`
    // green. That fallback is kernel behaviour we do not control, not a
    // contract. What we DO control is the cast two lines below: `pid as
    // libc::c_int` silently WRAPS any pid past `i32::MAX` into a negative
    // i32, which names something other than the pid the caller actually
    // asked about instead of reliably erroring — an undefined
    // reinterpretation, not a guaranteed rejection. This guard turns that
    // reinterpretation into an explicit "no" before the cast ever happens,
    // instead of leaning on kernel behaviour we don't control to reject it
    // for us. Green here is not evidence it's dead code.
    if pid == 0 || pid > i32::MAX as u32 {
        return Ok(None); // pid 0 is kernel_task; out-of-range can't be ours
    }
    // SAFETY: `proc_taskinfo` is a POD of `u64`/`i32` fields (libc 0.2.187,
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

/// Current boot time via `sysctl(kern.boottime)` — the boot identity.
///
/// `#[allow(dead_code)]` dropped (P0-8 Task 2): `registry::load()` calls this
/// via `platform::current_boot_id()` from production code now.
#[cfg(target_os = "macos")]
pub(crate) fn current_boot_id() -> io::Result<BootId> {
    let mut mib = [libc::CTL_KERN, libc::KERN_BOOTTIME];
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
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
    // See the NOTE in `process_start_time`: `tv_sec` needs no cast on Darwin.
    Ok(BootId::Unix {
        sec: tv.tv_sec,
        usec: tv.tv_usec as i64,
    })
}

/// Non-macOS unix (Linux, BSD, ...) boot-id read is deferred to the
/// Windows/Linux-enablement phase (spec: macOS-first). Mirrors the Windows
/// stub in `windows.rs` exactly — see `process_start_time`'s non-macOS-unix
/// stub above for why this arm must exist (P0-8 merge-gate fix wave C6).
#[cfg(all(unix, not(target_os = "macos")))]
pub(crate) fn current_boot_id() -> io::Result<BootId> {
    Err(io::Error::other(
        "current_boot_id is not implemented on this platform in v1 (macOS-first)",
    ))
}

/// Process-group id of `pid` (reap re-verifies `getpgid(pid) == pid`).
///
/// `#[allow(dead_code)]` dropped (P0-8 Task 3): called from production code
/// now, both via the `platform::getpgid` wrapper (`reap::reject_reason`) and
/// directly from `UnixReaper::reap` below.
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

/// PURE validation floor for `UnixReaper::reap`, independent of and
/// additional to `reap::reject_reason` in `orphan/reap.rs`. Defense-in-depth:
/// `reap` is a `pub` trait method on a type returned by the `pub fn
/// default_reaper()`, so it is publicly callable with an arbitrary `u32` by
/// any caller — not just the validated `reap_orphans` orchestration, an
/// assumption future callers (Task 4 wires in more of them) would otherwise
/// silently rely on. No syscalls — a plain predicate, so it is safe to
/// unit-test directly, unlike `reap()` itself, which must NEVER be called
/// with these values (see this crate's test-safety note: `reap(1)` resolves
/// to `kill(-1, SIGKILL)` on this platform).
///
/// Rejects `pid <= 1`: `pid == 0` means `kill(0, ...)` signals OUR OWN
/// process group, and `pid == 1` means `getpgid(1) == 1` on macOS, so the
/// group-kill arm below would resolve to `kill(-1, SIGKILL)` — every process
/// the caller can signal. Also rejects `pid > i32::MAX as u32`: such a value
/// goes negative when cast to `libc::pid_t` (`i32`), and `kill()` treats a
/// negative pid as "signal the process GROUP with abs(pid)" — flipping an
/// intended single-process kill into a group kill; `u32::MAX` in particular
/// becomes `-1`, the same all-process catastrophe as `pid == 1` above.
pub(crate) fn reap_pid_floor_violation(pid: u32) -> Option<&'static str> {
    if pid <= 1 {
        return Some("pid <= 1 (would self-signal our own group or flip into kill(-1, SIGKILL))");
    }
    if pid > i32::MAX as u32 {
        return Some(
            "pid > i32::MAX (u32->pid_t cast goes negative, flipping a single-pid kill into a group kill; u32::MAX becomes kill(-1, ...))",
        );
    }
    None
}

pub(crate) struct UnixReaper;

impl OrphanReaper for UnixReaper {
    /// Defense-in-depth floor FIRST (`reap_pid_floor_violation` above — read
    /// its doc comment): this method is reachable with an arbitrary `u32`
    /// from any caller, not only the validated `reap_orphans` orchestration.
    /// Then re-verify group-leadership at reap time (never trust the spawn
    /// invariant alone): `getpgid(pid) == pid` → group-kill; else single-pid
    /// fallback.
    fn reap(&self, pid: u32) -> io::Result<ReapKind> {
        if let Some(reason) = reap_pid_floor_violation(pid) {
            return Err(io::Error::other(format!(
                "refusing to reap pid {pid}: {reason}"
            )));
        }
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

#[cfg(test)]
mod floor_tests {
    use super::*;

    // F2: test ONLY the pure predicate. NEVER call `UnixReaper::reap` (or
    // `signal_group`/raw `libc::kill`) with 0, 1, or u32::MAX to "prove" this
    // — `reap(1)` resolves to `kill(-1, SIGKILL)` on this platform
    // (`getpgid(1) == 1`), which would signal EVERY process the developer
    // running this suite owns.

    #[test]
    fn floor_violation_rejects_0_1_and_u32_max() {
        assert!(reap_pid_floor_violation(0).is_some());
        assert!(reap_pid_floor_violation(1).is_some());
        assert!(reap_pid_floor_violation(u32::MAX).is_some());
    }

    #[test]
    fn floor_violation_allows_pids_in_the_safe_range() {
        assert!(reap_pid_floor_violation(2).is_none());
        assert!(reap_pid_floor_violation(std::process::id()).is_none());
        assert!(reap_pid_floor_violation(i32::MAX as u32).is_none());
    }
}

#[cfg(all(test, target_os = "macos"))]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod rss_tests {
    use super::*;

    /// A live process must report a non-zero resident size. This is the test that
    /// catches a units mistake or a wrong struct field: `/bin/sleep`'s RSS is a
    /// few hundred KB, so a pages-vs-bytes error would show up as an absurd value
    /// and a wrong-field error as zero.
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
        // `spawn()` returning does not mean the child has faulted in its
        // pages yet, so a single sample taken immediately flakes under CPU
        // contention (observed twice on correct code). Poll instead, up to a
        // bounded 2s deadline (40 attempts, 50ms apart); if the floor is
        // never cleared in that window something is genuinely wrong, not
        // merely slow.
        const MAX_ATTEMPTS: u32 = 40;
        const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);
        let mut rss = 0u64;
        for _ in 0..MAX_ATTEMPTS {
            rss = process_rss(pid)
                .unwrap()
                .expect("a live process has an RSS");
            if rss > 64 * 1024 {
                break;
            }
            std::thread::sleep(POLL_INTERVAL);
        }
        // Lower bound: any real process has more than 64 KB resident. Polled
        // above instead of sampled once — see the comment there.
        assert!(
            rss > 64 * 1024,
            "rss {rss} never exceeded the floor within {:?}",
            POLL_INTERVAL * MAX_ATTEMPTS
        );
        // Upper bound: chosen relative to the floor, not to `sleep`'s typical
        // size. The floor is 64 KB, so a 4096x pages-as-bytes error inflates
        // ANY value that clears the floor to at least 64 KB * 4096 = 256 MB.
        // Setting the ceiling below that threshold — 64 MB — makes the
        // mutation structurally impossible to miss, for every sampled value
        // that passes the floor, not merely for a typical/large one. 64 MB
        // still leaves ample headroom over `/bin/sleep`'s real resident size
        // (single-digit MB at most).
        assert!(
            rss < 64 * 1024 * 1024,
            "rss {rss} is implausibly large (>= 64 MB ceiling, chosen so a \
             4096x pages-as-bytes error cannot slip past the 64 KB floor)"
        );
        let _ = child.kill();
        let _ = child.wait();
    }

    /// A pid that no longer exists is `Ok(None)`, NOT `Err`. The caller samples
    /// pids the supervisor listed a moment earlier; a process exiting in that gap
    /// is normal and must not surface as a failure (spec §4.1).
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

    /// pid 0 (`kernel_task`) and any pid above `i32::MAX` are `Ok(None)`. The
    /// *behaviour* is worth pinning even though — see the comment inside —
    /// it does not by itself prove the guard in `process_rss` ran.
    #[test]
    fn rss_of_pid_zero_and_out_of_range_is_none() {
        // NOT proof the guard above fired: `proc_pidinfo` absorbs both pid 0
        // (rc == 0, errno == EPERM) and the wrapped out-of-range value
        // (rc == 0, errno == ESRCH) into its own `rc <= 0 => Ok(None)`
        // fallback, so this test is green whether or not the guard exists.
        assert_eq!(process_rss(0).unwrap(), None);
        assert_eq!(process_rss(i32::MAX as u32 + 1).unwrap(), None);
    }
}
