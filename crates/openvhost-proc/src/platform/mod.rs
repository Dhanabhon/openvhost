// SPDX-License-Identifier: GPL-3.0-or-later
//! Platform seam for process operations (spec §5). Core code never branches
//! on OS inline; the two driver impls live in `unix.rs` / `windows.rs`.

use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::Arc;

use crate::orphan::{BootId, OrphanReaper, ProcStartTime};

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

/// What to spawn. `program` MUST be a fully-resolved absolute path — the
/// drivers never consult $PATH (deterministic versioned installs).
/// Managed services must run in the FOREGROUND (no self-daemonize/setsid):
/// a daemonizing child escapes the containment group and stop would lie.
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Applied ON TOP of the allow-list base env (see [`assemble_env`]).
    pub env: Vec<(OsString, OsString)>,
}

/// Base environment allow-list + `extra` on top. The child NEVER inherits
/// the supervisor's full ambient environment (reproducible-env principle).
pub(crate) fn assemble_env(extra: &[(OsString, OsString)]) -> Vec<(OsString, OsString)> {
    let mut out: Vec<(OsString, OsString)> = Vec::new();
    let mut push_from_parent = |key: &str| {
        if let Some(v) = std::env::var_os(key) {
            out.push((OsString::from(key), v));
        }
    };
    for key in ["PATH", "HOME", "TMPDIR", "LANG"] {
        push_from_parent(key);
    }
    #[cfg(windows)]
    {
        for key in ["SystemRoot", "windir", "TEMP", "TMP"] {
            push_from_parent(key);
        }
        // CRT startup needs System32 resolvable even with a cleared env.
        if let Some(root) = std::env::var_os("SystemRoot") {
            let mut p = root.clone();
            p.push("\\System32");
            let path_entry = match out.iter_mut().find(|(k, _)| k == "PATH") {
                Some((_, existing)) => {
                    existing.push(";");
                    existing.push(&p);
                    None
                }
                None => Some((OsString::from("PATH"), p)),
            };
            if let Some(e) = path_entry {
                out.push(e);
            }
        }
    }
    for (k, v) in extra {
        match out.iter_mut().find(|(key, _)| key == k) {
            Some((_, existing)) => *existing = v.clone(),
            None => out.push((k.clone(), v.clone())),
        }
    }
    out
}

/// Opaque per-OS identity captured ONCE at spawn (never re-derived from the
/// child, whose id() becomes None after reaping).
pub struct PlatformHandle {
    #[cfg(unix)]
    pub(crate) pgid: i32,
    #[cfg(windows)]
    pub(crate) pid: u32,
}

/// Opaque spawned child. All fields private ON PURPOSE (Windows P0-5 may
/// swap the internals for a raw CreateProcessW route without breaking this
/// API — dual-specialist consultation, spec §5).
pub struct SpawnedChild {
    pub(crate) child: tokio::process::Child,
    pub(crate) handle: PlatformHandle,
}

/// Opaque async-readable pipe (keeps tokio types out of the public API).
pub struct OutputStream(pub(crate) OutputInner);
pub(crate) enum OutputInner {
    Out(tokio::process::ChildStdout),
    Err(tokio::process::ChildStderr),
}

impl tokio::io::AsyncRead for OutputStream {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        match &mut self.get_mut().0 {
            OutputInner::Out(s) => std::pin::Pin::new(s).poll_read(cx, buf),
            OutputInner::Err(s) => std::pin::Pin::new(s).poll_read(cx, buf),
        }
    }
}

impl SpawnedChild {
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub fn take_stdout(&mut self) -> Option<OutputStream> {
        self.child
            .stdout
            .take()
            .map(|s| OutputStream(OutputInner::Out(s)))
    }
    pub fn take_stderr(&mut self) -> Option<OutputStream> {
        self.child
            .stderr
            .take()
            .map(|s| OutputStream(OutputInner::Err(s)))
    }
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        self.child.wait().await
    }
    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }
    #[cfg(unix)]
    pub(crate) fn pgid(&self) -> i32 {
        self.handle.pgid
    }
    #[cfg(windows)]
    pub(crate) fn pid_snapshot(&self) -> u32 {
        self.handle.pid
    }
}

/// Process operations, one impl per OS. This trait is the LCD fallback:
/// real services get protocol shutdown (mysql admin cmd, `nginx -s quit`)
/// at their per-service adapter layer in later slices. A signal-delivery
/// error meaning "no such process/group" is NOT a failure — callers should
/// `try_wait()` (the target may have exited on its own). Reload (e.g.
/// SIGUSR2) is deliberately NOT a trait method — it lands as a
/// platform/macos capability in P0-4 against the snapshotted pid (spec §5).
pub trait ProcessDriver: Send + Sync {
    fn spawn(&self, spec: &SpawnSpec) -> io::Result<SpawnedChild>;
    fn request_graceful_stop(&self, child: &SpawnedChild) -> io::Result<()>;
    fn kill(&self, child: &mut SpawnedChild) -> io::Result<()>;
}

pub fn default_driver() -> Arc<dyn ProcessDriver> {
    #[cfg(unix)]
    {
        Arc::new(unix::UnixDriver)
    }
    #[cfg(windows)]
    {
        Arc::new(windows::WindowsDriver)
    }
}

pub fn default_reaper() -> Arc<dyn OrphanReaper> {
    #[cfg(unix)]
    {
        Arc::new(unix::UnixReaper)
    }
    #[cfg(windows)]
    {
        Arc::new(windows::WindowsReaper)
    }
}

// Widened `pub(crate)` -> `pub` (P0-8 Task 4): `Inner::record_running`
// (in-crate) calls this unconditionally on both platforms, and the
// macOS-gated exit-criterion integration test (`tests/orphan_reap.rs`) needs
// to read a process's start-time from OUTSIDE the crate to construct a
// `SupervisedRecord` for a process spawned directly by the test (never
// through a `Supervisor`) — that test only has the crate's public API.
#[cfg(unix)]
pub fn process_start_time(pid: u32) -> std::io::Result<Option<ProcStartTime>> {
    unix::process_start_time(pid)
}
#[cfg(windows)]
pub fn process_start_time(pid: u32) -> std::io::Result<Option<ProcStartTime>> {
    windows::process_start_time(pid)
}

/// Resident set size in bytes for a live pid. See the platform impls for the
/// `Ok(None)` vs `Err` contract.
#[cfg(unix)]
pub fn process_rss(pid: u32) -> std::io::Result<Option<u64>> {
    unix::process_rss(pid)
}
#[cfg(windows)]
pub fn process_rss(pid: u32) -> std::io::Result<Option<u64>> {
    windows::process_rss(pid)
}

// `current_boot_id`'s `#[allow(dead_code)]` was dropped here (P0-8 Task 2):
// `registry::load()` is now a real, non-test caller on both dispatch arms.
#[cfg(unix)]
pub(crate) fn current_boot_id() -> std::io::Result<BootId> {
    unix::current_boot_id()
}
#[cfg(windows)]
pub(crate) fn current_boot_id() -> std::io::Result<BootId> {
    windows::current_boot_id()
}

// `#[allow(dead_code)]` dropped (P0-8 Task 3): `reap::reject_reason` now
// calls this via `platform::getpgid(std::process::id())` from production
// code (the `#[cfg(unix)]` `reap_orphans` body).
#[cfg(unix)]
pub(crate) fn getpgid(pid: u32) -> std::io::Result<u32> {
    unix::getpgid(pid)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn env_is_allowlist_not_inherit() {
        // SAFETY: test-only env mutation; the two env tests touch disjoint keys.
        unsafe {
            std::env::set_var("OPENVHOST_TEST_SHOULD_NOT_LEAK", "1");
        }
        let env = assemble_env(&[]);
        assert!(env.iter().any(|(k, _)| k == "PATH"));
        assert!(
            !env.iter()
                .any(|(k, _)| k == "OPENVHOST_TEST_SHOULD_NOT_LEAK")
        );
        // SAFETY: test-only env mutation; the two env tests touch disjoint keys.
        unsafe {
            std::env::remove_var("OPENVHOST_TEST_SHOULD_NOT_LEAK");
        }
    }

    #[test]
    fn extra_env_overrides_base() {
        let extra = vec![(OsString::from("PATH"), OsString::from("/only/this"))];
        let env = assemble_env(&extra);
        let path = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone())
            .unwrap();
        assert_eq!(path, OsString::from("/only/this"));
        assert_eq!(env.iter().filter(|(k, _)| k == "PATH").count(), 1);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_true_exits_zero() {
        let driver = default_driver();
        let spec = SpawnSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec![OsString::from("-c"), OsString::from("exit 0")],
            cwd: None,
            env: vec![],
        };
        let mut child = driver.spawn(&spec).unwrap();
        let status = child.wait().await.unwrap();
        assert!(status.success());
    }
}
