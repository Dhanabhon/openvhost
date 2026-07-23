// SPDX-License-Identifier: GPL-3.0-or-later
//! Single-instance advisory lock at `<run>/lock`, held for the process
//! lifetime. Reap MUST run only while this is held (spec §7): otherwise a
//! second live instance would reap the first's HEALTHY services (identity
//! matches — it really is their process — but the "orphan" premise is false).

use std::io;
use std::path::Path;

/// Holds the lock for as long as this value is alive; the `flock` is scoped
/// to the *open file description*, so it releases automatically the moment
/// `_file`'s descriptor closes (on `Drop`) — same fd-scoped model as
/// `openvhost-pkg`'s staging lock (`crates/openvhost-pkg/src/layout.rs`).
pub struct InstanceLock {
    _file: std::fs::File, // fd held for lifetime; flock releases on close
}

impl InstanceLock {
    /// `Ok(Some)` = acquired; `Ok(None)` = another instance holds it.
    #[cfg(unix)]
    pub fn acquire(run_dir: &Path) -> io::Result<Option<InstanceLock>> {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        use std::os::unix::io::AsRawFd;
        std::fs::create_dir_all(run_dir)?;
        // Spec §4: the run dir holds the lock file and the process registry
        // (pid/start-time identities) — tighten to 0700 at create time rather
        // than trusting the ambient umask. `set_permissions` sets the exact
        // bits regardless of umask (same approach as
        // `registry::set_private_dir`), instead of `DirBuilder::mode`, which
        // would still be masked by umask on creation.
        std::fs::set_permissions(run_dir, std::fs::Permissions::from_mode(0o700))?;
        let path = run_dir.join("lock");
        let file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .mode(0o600)
            .open(&path)?;
        // SAFETY: `file` is a valid, open file descriptor for the duration of
        // this call; `flock(2)` has no other preconditions. `LOCK_NB` makes it
        // return immediately with `EWOULDBLOCK` instead of blocking when
        // another instance already holds the lock. The lock releases when
        // `file`'s fd closes (on drop) — no explicit unlock call needed.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            Ok(Some(InstanceLock { _file: file }))
        } else {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EWOULDBLOCK) {
                Ok(None)
            } else {
                Err(e)
            }
        }
    }

    /// Windows single-instance is deferred (`LockFileEx` / named mutex —
    /// spec §7). Failing closed here would block the macOS-first app on
    /// non-unix; returning an explicit error makes the caller's intent
    /// unambiguous instead of silently pretending success. Not reached on
    /// macOS.
    #[cfg(not(unix))]
    pub fn acquire(_run_dir: &Path) -> io::Result<Option<InstanceLock>> {
        Err(io::Error::other(
            "InstanceLock is not implemented on Windows in v1 (macOS-first)",
        ))
    }
}
