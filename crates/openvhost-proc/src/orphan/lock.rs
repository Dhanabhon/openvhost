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

    /// Is a supervisor holding the run lock right now? Acquire-then-drop.
    ///
    /// Side-effect-free in the way that matters: the orphan reap lives in
    /// `Supervisor::with_orphan_cleanup`, **not** in the lock, so probing
    /// never kills anything. It deliberately does not create the run
    /// directory either — a probe from a CLI on a machine that has never run
    /// the app should answer, not provision. It *may* create the empty lock
    /// file (and re-assert `0700` on an existing run dir) when the directory
    /// is already there, because it reuses [`InstanceLock::acquire`] verbatim
    /// rather than forking a second flock code path.
    ///
    /// Three variants, not a bool (spec D3): an I/O error on the lock file is
    /// a genuine third answer, and collapsing it into "absent" would make
    /// `openvhost status` report "not running" when the truth is "could not
    /// tell" — the boolean-collapse-where-a-state-belongs mistake this
    /// codebase has already made three times.
    ///
    /// Advisory only, and racy by nature: the app can quit a microsecond
    /// after this returns [`SupervisorPresence::Present`]. Callers use it to
    /// *improve a message*; whether the control channel actually answers is
    /// the connect result's job.
    pub fn probe(run_dir: &Path) -> SupervisorPresence {
        match std::fs::symlink_metadata(run_dir) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => return SupervisorPresence::Absent,
            Err(e) => {
                return SupervisorPresence::Indeterminate {
                    reason: format!("cannot stat {}: {e}", run_dir.display()),
                };
            }
            Ok(md) if !md.is_dir() => {
                return SupervisorPresence::Indeterminate {
                    reason: format!("{} is not a directory", run_dir.display()),
                };
            }
            Ok(_) => {}
        }
        match InstanceLock::acquire(run_dir) {
            // We took it, so nobody was holding it. Drop immediately — the
            // probe must not leave the lock held and lock out a real launch.
            Ok(Some(lock)) => {
                drop(lock);
                SupervisorPresence::Absent
            }
            Ok(None) => SupervisorPresence::Present,
            Err(e) => SupervisorPresence::Indeterminate {
                reason: e.to_string(),
            },
        }
    }
}

/// Answer to [`InstanceLock::probe`].
///
/// `Indeterminate` exists because "I could not find out" is not the same
/// answer as "no" — see [`InstanceLock::probe`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorPresence {
    /// Something holds the run lock: an app instance is live.
    Present,
    /// Nothing holds the run lock: no app instance is live.
    Absent,
    /// The lock could not be evaluated. `reason` is for humans; never parse
    /// it.
    Indeterminate {
        /// What went wrong, in the words of the failing syscall.
        reason: String,
    },
}

#[cfg(all(test, unix))]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Poll until the lock reads [`SupervisorPresence::Absent`], bounded.
    ///
    /// Closing the fd releases the `flock` synchronously *in this process* —
    /// but this test binary runs other tests that spawn children in
    /// parallel, and between the fork and the `exec` a child transiently
    /// holds a duplicate of every open descriptor, this lock file's
    /// included. `O_CLOEXEC` clears it at `exec`, so the window is
    /// milliseconds; within it `flock` correctly reports the lock still
    /// held. Measured: 0 failures in 5 runs of the lock tests alone, 2 in 12
    /// runs alongside the spawning reap tests.
    ///
    /// This does not weaken the assertion. A `probe` that genuinely left the
    /// lock held never becomes `Absent`, so the bound expires and the test
    /// fails — proved by the neuter in the module's own history (removing
    /// the `drop(lock)` from `probe` fails this in every run).
    ///
    /// Not a production concern: the CLI process that probes spawns nothing.
    fn wait_for_absent(run: &Path) -> SupervisorPresence {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let presence = InstanceLock::probe(run);
            if presence == SupervisorPresence::Absent || std::time::Instant::now() >= deadline {
                return presence;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[test]
    fn probe_is_absent_when_the_run_dir_does_not_exist() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        assert_eq!(InstanceLock::probe(&run), SupervisorPresence::Absent);
        assert!(
            !run.exists(),
            "probing must not provision the run directory"
        );
    }

    #[test]
    fn probe_is_absent_when_an_unheld_lock_file_exists() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        // Acquire and release, leaving the lock FILE behind but unheld —
        // exactly the state a clean quit leaves.
        drop(InstanceLock::acquire(&run).unwrap());
        assert!(run.join("lock").exists());
        assert_eq!(wait_for_absent(&run), SupervisorPresence::Absent);
    }

    #[test]
    fn probe_is_present_while_the_lock_is_held() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        // flock is scoped to the open file description, so a second open in
        // this same process contends exactly as another process would.
        let held = InstanceLock::acquire(&run).unwrap();
        assert!(held.is_some());
        assert_eq!(InstanceLock::probe(&run), SupervisorPresence::Present);
        drop(held);
        assert_eq!(wait_for_absent(&run), SupervisorPresence::Absent);
    }

    #[test]
    fn probe_is_indeterminate_when_the_run_path_is_not_a_directory() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::write(&run, b"not a directory").unwrap();
        match InstanceLock::probe(&run) {
            SupervisorPresence::Indeterminate { reason } => {
                assert!(reason.contains("not a directory"), "{reason}");
            }
            other => panic!("expected Indeterminate, got {other:?}"),
        }
    }

    #[test]
    fn probe_does_not_leave_the_lock_held() {
        let home = tempfile::tempdir().unwrap();
        let run = home.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
        assert_eq!(wait_for_absent(&run), SupervisorPresence::Absent);
        // A real launch right after a probe must still be able to acquire.
        // Polled for the same fork-window reason `wait_for_absent` documents.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if InstanceLock::acquire(&run).unwrap().is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "probe left the run lock held"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}
