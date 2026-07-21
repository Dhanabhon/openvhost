// SPDX-License-Identifier: GPL-3.0-or-later
//! Unix driver: containment = own process group set atomically at spawn
//! (`process_group(0)` → posix_spawn attribute; closes the ESRCH race a
//! post-fork setpgid would leave). Signals target the SNAPSHOTTED -pgid.

use std::io;
use std::process::Stdio;

use super::{PlatformHandle, ProcessDriver, SpawnSpec, SpawnedChild, assemble_env};

pub(crate) struct UnixDriver;

fn signal_group(pgid: i32, sig: libc::c_int) -> io::Result<()> {
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
