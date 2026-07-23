// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows driver v0. Containment flags set at spawn; graceful stop is an
//! OPPORTUNISTIC CTRL_BREAK, but it never actually lands: CREATE_NO_WINDOW
//! gives every spawned child its own hidden console, so CTRL_BREAK from us
//! cannot reach it — that's true whether we're a dev console app or the
//! packaged GUI (windows_subsystem = "windows") with no console at all. v0
//! Windows stop is therefore always deadline+kill; real graceful shutdown
//! arrives with P0-5 (Job Objects / per-service shutdown protocol) —
//! documented honestly (spec §5). From P0-5, kill() means TerminateJobObject
//! on the app-wide Job Object (ONE job per app); never simplify back to
//! per-process termination. FFI via windows-sys (already in-tree via tokio).

use std::io;
use std::process::Stdio;

use windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent;

use super::{PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild, assemble_env};
use crate::orphan::{BootId, OrphanReaper, ProcStartTime, ReapKind};

const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const CTRL_BREAK_EVENT: u32 = 1;

pub(crate) struct WindowsDriver;

impl ProcessDriver for WindowsDriver {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild> {
        let mut cmd = tokio::process::Command::new(&spec.program);
        cmd.args(&spec.args)
            .env_clear()
            .envs(assemble_env(&spec.env))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
        if let Some(cwd) = &spec.cwd {
            cmd.current_dir(cwd);
        }
        let child = cmd.spawn()?;
        let pid = child
            .id()
            .ok_or_else(|| io::Error::other("spawned child has no pid"))?;
        Ok(SpawnedChild {
            child,
            handle: PlatformHandle { pid },
        })
    }

    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()> {
        // Opportunistic only, and it always fails in practice: CREATE_NO_WINDOW
        // means this child owns its own hidden console, so CTRL_BREAK from us
        // never reaches it. v0 Windows stop is always deadline+kill via the
        // supervisor's 5s grace deadline → kill() path; that's the real
        // reclaimer. Real graceful arrives with P0-5 (Job Objects /
        // per-service shutdown protocol). The call is kept anyway because
        // it's harmless: it returns Err and the caller ignores it.
        // SAFETY: plain Win32 call, no pointers.
        let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, child.pid_snapshot()) };
        if ok != 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()> {
        // v0: direct TerminateProcess via tokio (single process, no tree).
        // P0-5 replaces this with TerminateJobObject on the app-wide job.
        child.child.start_kill()
    }
}

/// Windows start-time via OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION) +
/// GetProcessTimes creation time. Deferred to the Windows-enablement phase.
///
/// `#[allow(dead_code)]`: only called via `platform::process_start_time`,
/// which itself has no caller on Windows until Task 2/3 wires in the
/// registry/reaper (the macOS-gated tests that exercise the dispatch layer
/// today are `cfg(target_os = "macos")` and don't exist on this target).
#[cfg(windows)]
#[allow(dead_code)]
pub(crate) fn process_start_time(_pid: u32) -> io::Result<Option<ProcStartTime>> {
    Err(io::Error::other(
        "process_start_time is not implemented on Windows in v1 (macOS-first)",
    ))
}

/// `#[allow(dead_code)]` dropped (P0-8 Task 2): `registry::load()` calls this
/// via `platform::current_boot_id()` from production code now, on this
/// target too (the stub still returns an error — Windows-enablement phase).
#[cfg(windows)]
pub(crate) fn current_boot_id() -> io::Result<BootId> {
    Err(io::Error::other(
        "current_boot_id is not implemented on Windows in v1 (macOS-first)",
    ))
}

pub(crate) struct WindowsReaper;

impl OrphanReaper for WindowsReaper {
    fn reap(&self, _pid: u32) -> io::Result<ReapKind> {
        Err(io::Error::other(
            "orphan reap is not implemented on Windows in v1 (macOS-first)",
        ))
    }
}
