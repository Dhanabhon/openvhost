// SPDX-License-Identifier: GPL-3.0-or-later
//! Staging, atomic install, stale-staging sweep, and `current`-link update
//! (spec §5 S20–S22).

use std::fs;
use std::io;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::error::PkgError;
use crate::platform;
use crate::request::PackagesRoot;

const STALE_AFTER: Duration = Duration::from_secs(24 * 60 * 60);

/// A staging directory holding an exclusive advisory lock on its `.lock`
/// file for the `Staging` value's entire lifetime, so the 24h sweeper never
/// deletes a live (possibly slept-mid-download) install (S20).
///
/// The lock is deliberately NOT modeled as a borrowed guard held alongside
/// the locked `File` (`fd_lock`'s `RwLock<File>::write()` guard borrows the
/// `RwLock`, which would make this struct self-referential and impossible
/// to construct safely). Instead this relies on `flock`'s fd-scoped
/// semantics directly: the lock is associated with the *open file
/// description*, not the path, so it is held for as long as `_lock_file`'s
/// descriptor stays open and releases automatically — with no explicit
/// unlock call — the moment that descriptor closes, i.e. when `Staging`
/// drops. No borrow, no self-reference, no leaked guard.
pub(crate) struct Staging {
    dir: tempfile::TempDir,
    _lock_file: fs::File,
}

impl Staging {
    /// Create a fresh, private (`0o700`), locked staging directory under
    /// `root.staging_root()` (creating that root if needed).
    pub(crate) fn create(root: &PackagesRoot) -> Result<Staging, PkgError> {
        let sroot = root.staging_root();
        fs::create_dir_all(&sroot).map_err(|e| PkgError::io("create_dir", &sroot, e))?;
        set_private(&sroot)?;

        let dir = tempfile::Builder::new()
            .prefix("ovh")
            .tempdir_in(&sroot)
            .map_err(|e| PkgError::io("tempdir", &sroot, e))?;
        set_private(dir.path())?;

        let lock_path = dir.path().join(".lock");
        let lock_file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&lock_path)
            .map_err(|e| PkgError::io("lockfile", &lock_path, e))?;
        // Exclusive, non-blocking: staging dirs are freshly minted with a
        // random name, so contention here would mean something else already
        // opened this exact path — treat that as a hard error rather than
        // waiting.
        try_lock_exclusive(&lock_file).map_err(|e| PkgError::io("flock", &lock_path, e))?;

        Ok(Staging {
            dir,
            _lock_file: lock_file,
        })
    }

    /// The staging directory's path.
    pub(crate) fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Test-only: release the lock without running the `TempDir`'s RAII
    /// cleanup, simulating a process that held the lock and exited without
    /// unwinding through `Drop` (the OS releases all of a dead process's
    /// `flock`s on exit; the directory and its `.lock` file remain on disk
    /// exactly as they would after a crash or a killed process).
    #[cfg(test)]
    fn leak_unlocked(self) -> tempfile::TempDir {
        let Staging { dir, _lock_file } = self;
        drop(_lock_file);
        dir
    }
}

/// Atomic install: rename the staged tree onto the final version directory.
/// Same volume by construction (S21). `EEXIST`/`ENOTEMPTY` (dest already
/// present, or a concurrent identical install won the race) maps to
/// [`PkgError::AlreadyInstalled`].
pub(crate) fn install_dir(
    staged_root: &Path,
    final_dir: &Path,
    name: &str,
    version: &str,
) -> Result<(), PkgError> {
    if final_dir.exists() {
        return Err(already_installed(name, version));
    }
    if let Some(parent) = final_dir.parent() {
        fs::create_dir_all(parent).map_err(|e| PkgError::io("create_dir", parent, e))?;
    }
    match fs::rename(staged_root, final_dir) {
        Ok(()) => Ok(()),
        Err(e) if matches!(e.raw_os_error(), Some(code) if is_exists(code)) => {
            Err(already_installed(name, version))
        }
        Err(e) => Err(PkgError::io("rename", final_dir, e)),
    }
}

/// Best-effort sweep of staging directories older than 24h whose lock is
/// currently free, i.e. no live [`Staging`] holds them (S20). Errors reading
/// or removing any single entry are swallowed: this is background hygiene,
/// never a hard failure path, and a directory this couldn't touch this time
/// is simply reconsidered on the next sweep.
pub(crate) fn sweep_stale(root: &PackagesRoot) {
    let sroot = root.staging_root();
    let Ok(read_dir) = fs::read_dir(&sroot) else {
        return;
    };
    let now = SystemTime::now();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_dir() || !is_stale(&entry, now) {
            continue;
        }
        remove_if_unlocked(&path);
    }
}

fn is_stale(entry: &fs::DirEntry, now: SystemTime) -> bool {
    entry
        .metadata()
        .and_then(|m| m.modified())
        .map(|modified| {
            now.duration_since(modified)
                .map(|age| age > STALE_AFTER)
                .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// Remove `path` ONLY if we can take its `.lock` file exclusively — i.e. no
/// live [`Staging`] holds it — OR the `.lock` file is genuinely absent
/// (`NotFound`: older format, or a partial staging dir that never got as far
/// as creating one), which is safe to remove since staleness was already
/// established by the caller. Any OTHER open error (`EMFILE`, `EACCES`, …)
/// is NOT evidence of an orphan — it may just mean this process is
/// transiently unable to check — so it is left alone; the next sweep
/// reconsiders it.
fn remove_if_unlocked(path: &Path) {
    let lock_path = path.join(".lock");
    match fs::OpenOptions::new().write(true).open(&lock_path) {
        Ok(lock_file) => {
            if try_lock_exclusive(&lock_file).is_ok() {
                let _ = fs::remove_dir_all(path);
            }
            // Else: a live Staging holds the lock (EWOULDBLOCK) -> skip.
            // `lock_file` drops here regardless, releasing any lock we took.
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let _ = fs::remove_dir_all(path);
        }
        Err(_) => {
            // Transient/permission error opening the lock file: NOT proof
            // of an orphan. Skip rather than risk deleting a live install.
        }
    }
}

/// Point `link` (…/current) at the sibling version directory
/// `version_dir_name`. Dispatches to the per-OS implementation (S22).
pub(crate) fn update_current(link: &Path, version_dir_name: &str) -> Result<(), PkgError> {
    platform::update_current(link, version_dir_name)
}

fn already_installed(name: &str, version: &str) -> PkgError {
    PkgError::AlreadyInstalled {
        name: name.to_string(),
        version: version.to_string(),
    }
}

/// `EEXIST` (17) / `ENOTEMPTY` (66 macOS, 39 Linux). Windows
/// `ERROR_ALREADY_EXISTS` (183).
fn is_exists(code: i32) -> bool {
    matches!(code, 17 | 66 | 39 | 183)
}

/// Take an exclusive, non-blocking advisory lock on `file`'s underlying fd.
/// The lock is scoped to the *open file description*: it is held for as
/// long as this fd (or a `dup`'d copy of it) stays open, and it releases
/// automatically the moment the last referencing fd closes — no borrowed
/// guard needs to be carried alongside the `File` for this to work (S20).
#[cfg(unix)]
fn try_lock_exclusive(file: &fs::File) -> io::Result<()> {
    use std::os::unix::io::AsRawFd;
    // SAFETY: `file` is a valid, open file descriptor for the duration of
    // this call. `flock(2)` has no other preconditions; on failure it
    // returns -1 and sets `errno`, which `io::Error::last_os_error` reads.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

/// Non-unix fallback: best-effort no-op. Windows staging-lock enforcement is
/// deferred to the Windows-enablement phase along with the rest of Windows
/// runtime support (macOS-first v1) — the crate must still compile, and
/// `sweep_stale` must still run (conservatively, without lock protection) on
/// that target.
#[cfg(not(unix))]
fn try_lock_exclusive(_file: &fs::File) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private(p: &Path) -> Result<(), PkgError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(p, fs::Permissions::from_mode(0o700))
        .map_err(|e| PkgError::io("chmod", p, e))
}
#[cfg(not(unix))]
fn set_private(_p: &Path) -> Result<(), PkgError> {
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::request::PackagesRoot;

    fn root() -> (tempfile::TempDir, PackagesRoot) {
        let home = tempfile::tempdir().unwrap();
        let root = PackagesRoot::from_home(home.path());
        std::fs::create_dir_all(root.as_path()).unwrap();
        (home, root)
    }

    #[cfg(unix)]
    fn set_mtime_past(path: &Path, age: Duration) {
        let past = SystemTime::now().checked_sub(age).unwrap();
        let times = fs::FileTimes::new().set_modified(past);
        let f = fs::File::open(path).unwrap();
        f.set_times(times).unwrap();
    }

    #[test]
    fn staging_is_locked_and_created() {
        let (_h, r) = root();
        let s = Staging::create(&r).unwrap();
        assert!(s.path().is_dir());
        assert!(s.path().starts_with(r.staging_root()));
    }

    #[test]
    fn install_dir_is_atomic_and_rejects_existing() {
        let (_h, r) = root();
        let staging = tempfile::tempdir_in(r.as_path()).unwrap();
        std::fs::write(staging.path().join("marker"), b"x").unwrap();
        let dest = r.package_dir("php", "8.4", "8.4.23");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        install_dir(staging.path(), &dest, "php", "8.4.23").unwrap();
        assert!(dest.join("marker").is_file());

        // second install to same dest -> AlreadyInstalled
        let staging2 = tempfile::tempdir_in(r.as_path()).unwrap();
        let err = install_dir(staging2.path(), &dest, "php", "8.4.23").unwrap_err();
        assert!(matches!(err, PkgError::AlreadyInstalled { .. }));
    }

    #[cfg(unix)]
    #[test]
    fn current_link_swaps_atomically() {
        let (_h, r) = root();
        let major = r.major_dir("php", "8.4");
        std::fs::create_dir_all(major.join("8.4.1")).unwrap();
        std::fs::create_dir_all(major.join("8.4.2")).unwrap();
        let link = r.current_link("php", "8.4");
        update_current(&link, "8.4.1").unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap().to_str().unwrap(),
            "8.4.1"
        );
        update_current(&link, "8.4.2").unwrap();
        assert_eq!(
            std::fs::read_link(&link).unwrap().to_str().unwrap(),
            "8.4.2"
        );
    }

    #[cfg(unix)]
    #[test]
    fn current_refuses_to_replace_real_dir() {
        let (_h, r) = root();
        let major = r.major_dir("php", "8.4");
        let link = r.current_link("php", "8.4");
        std::fs::create_dir_all(&link).unwrap(); // a real dir named "current"
        std::fs::create_dir_all(major.join("8.4.1")).unwrap();
        // Asserts the SPECIFIC variant (security audit D3): `UnsafeArchive`'s
        // Display ("archive rejected: ...") is nonsense for a current-link
        // precondition failure that has nothing to do with archive content.
        assert!(matches!(
            update_current(&link, "8.4.1"),
            Err(PkgError::Unsupported(_))
        ));
    }

    #[test]
    fn sweep_stale_ignores_a_fresh_staging() {
        let (_h, r) = root();
        let staging = Staging::create(&r).unwrap();
        let dir_path = staging.path().to_path_buf();
        sweep_stale(&r); // brand new, nowhere near 24h old -> must survive
        assert!(dir_path.is_dir());
    }

    // The KNOWN HAZARD this task exists to close: prove the lock is held for
    // the whole `Staging` lifetime (so a live install can never be swept),
    // and that a directory becomes sweepable again the instant the lock is
    // actually released — exactly the fd-close-releases model, exercised
    // end to end rather than asserted about in the abstract.
    #[cfg(unix)]
    #[test]
    fn sweep_stale_skips_a_locked_staging_and_removes_it_once_released() {
        let (_h, r) = root();
        let staging = Staging::create(&r).unwrap();
        let dir_path = staging.path().to_path_buf();
        set_mtime_past(&dir_path, STALE_AFTER + Duration::from_secs(60));

        // `staging` is still alive and holding the flock -> even though the
        // directory now looks stale by mtime, the sweeper must not touch it.
        sweep_stale(&r);
        assert!(
            dir_path.is_dir(),
            "sweep must not remove a dir whose lock is held"
        );

        // Simulate the owning process disappearing without a graceful
        // shutdown: the OS closes its fds (releasing the flock) but the
        // directory and `.lock` file remain on disk, same as after a crash.
        let leaked_dir = staging.leak_unlocked();
        sweep_stale(&r);
        assert!(
            !dir_path.exists(),
            "sweep must remove a stale dir once its lock is free"
        );
        std::mem::forget(leaked_dir); // already gone; nothing left to clean up
    }

    #[cfg(unix)]
    #[test]
    fn sweep_stale_removes_a_stale_dir_with_no_lock_file_at_all() {
        // NotFound branch: a stale staging dir with NO `.lock` file at all
        // (older format, or a partial staging dir that never got as far as
        // creating one) is a true orphan and must still be removed.
        let (_h, r) = root();
        std::fs::create_dir_all(r.staging_root()).unwrap();
        let staging = tempfile::tempdir_in(r.staging_root()).unwrap();
        let dir_path = staging.path().to_path_buf();
        set_mtime_past(&dir_path, STALE_AFTER + Duration::from_secs(60));

        sweep_stale(&r);
        assert!(
            !dir_path.exists(),
            "a stale dir with no .lock file at all must be removed as a true orphan"
        );
    }

    // GROUP C (security audit): the fix this test locks in. Previously ANY
    // `.lock` open error — including EMFILE/EACCES, not just a genuinely
    // missing file — was treated as proof of an orphan and deleted. Forcing
    // `.lock` itself to be a directory produces a deterministic, portable
    // "can't open as a plain file" error with no dependency on permission
    // bits (which a root-run process would simply bypass) — nothing else
    // reliably fails an `OpenOptions::write(true).open` call the same way
    // on every platform/user. This must now be SKIPPED, not deleted.
    #[cfg(unix)]
    #[test]
    fn sweep_stale_skips_a_stale_dir_when_lock_path_is_not_a_plain_file() {
        let (_h, r) = root();
        std::fs::create_dir_all(r.staging_root()).unwrap();
        let staging = tempfile::tempdir_in(r.staging_root()).unwrap();
        let dir_path = staging.path().to_path_buf();
        std::fs::create_dir(dir_path.join(".lock")).unwrap();
        set_mtime_past(&dir_path, STALE_AFTER + Duration::from_secs(60));

        sweep_stale(&r);
        assert!(
            dir_path.is_dir(),
            "a non-NotFound .lock open error must not be treated as proof of an orphan"
        );
    }
}
