// SPDX-License-Identifier: GPL-3.0-or-later
//! Unix driver: containment = own process group set atomically at spawn
//! (`process_group(0)` → posix_spawn attribute; closes the ESRCH race a
//! post-fork setpgid would leave). Signals target the SNAPSHOTTED -pgid.

use std::io;
use std::process::Stdio;

use super::{PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild, assemble_env};
use crate::orphan::{BootId, ProcStartTime};

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
/// `#[allow(dead_code)]`: only called via `platform::process_start_time` from
/// the macOS-gated tests in `orphan::tests` until Task 2/3 wires in the
/// registry/reaper callers.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
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

/// Current boot time via `sysctl(kern.boottime)` — the boot identity.
///
/// `#[allow(dead_code)]`: see `process_start_time` above.
#[cfg(target_os = "macos")]
#[allow(dead_code)]
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

/// Process-group id of `pid` (reap re-verifies `getpgid(pid) == pid`).
///
/// `#[allow(dead_code)]`: see `process_start_time` above.
#[cfg(unix)]
#[allow(dead_code)]
pub(crate) fn getpgid(pid: u32) -> io::Result<u32> {
    // SAFETY: plain syscall, no memory handed over.
    let pg = unsafe { libc::getpgid(pid as libc::pid_t) };
    if pg < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(pg as u32)
    }
}
