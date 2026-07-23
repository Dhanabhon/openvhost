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
