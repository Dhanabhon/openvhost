// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows driver v0. Containment flags set at spawn; graceful stop is an
//! OPPORTUNISTIC CTRL_BREAK (works when a console exists — dev shells).
//! The packaged GUI app (windows_subsystem = "windows") has no console, so
//! v0/v1 graceful stop there is effectively hard-kill-only — documented
//! honestly (spec §5). From P0-5, kill() means TerminateJobObject on the
//! app-wide Job Object (ONE job per app); never simplify back to
//! per-process termination. FFI via windows-sys (already in-tree via tokio).

use std::io;
use std::os::windows::process::CommandExt;
use std::process::Stdio;

use windows_sys::Win32::System::Console::GenerateConsoleCtrlEvent;

use super::{PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild, assemble_env};

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
        // Opportunistic: reaches the child only when it shares a console
        // with us (dev). Failure here is expected in the GUI app; the
        // supervisor's 5s-deadline → kill() path is the real reclaimer.
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
