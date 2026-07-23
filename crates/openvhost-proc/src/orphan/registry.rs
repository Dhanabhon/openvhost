// SPDX-License-Identifier: GPL-3.0-or-later
//! File-backed process registry: one atomic JSON file at
//! `<run>/supervised.json`, boot-gated on load, size/count capped.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use super::{BootId, ProcessRegistry, RegistrySnapshot, SupervisedRecord};
use crate::platform;

// `#[allow(dead_code)]` on the items below: no caller yet — Task 3's
// `OrphanReaper` is the real caller of `FileRegistry`/`ProcessRegistry` in
// production code. Until it lands, the only user is `registry::tests`
// (a `#[cfg(test)]` module invisible to the dead-code pass on the plain,
// non-`--test` build of this crate — the same mechanism documented on the
// Task 1 platform readers in `platform/mod.rs`). Drop these once Task 3
// wires in the real caller.
#[allow(dead_code)]
const MAX_BYTES: u64 = 64 * 1024;
#[allow(dead_code)]
const MAX_RECORDS: usize = 64;

#[allow(dead_code)]
pub struct FileRegistry {
    path: PathBuf,
    // Serializes each `record`/`remove` load-mutate-store critical section
    // WITHIN THIS PROCESS: Task 4's supervisor calls `record()` from
    // concurrent per-service tasks, and without this, two concurrent
    // load-mutate-store sequences could each load the same snapshot, push
    // their own record, and have the second `store()` silently clobber the
    // first — a lost record means an orphan that never gets reaped. This
    // does NOT provide cross-process safety; that comes from the
    // single-instance advisory file lock (design spec §7, Task 4), which
    // ensures only one supervisor process ever touches this file at all.
    write_lock: Mutex<()>,
}

#[allow(dead_code)]
impl FileRegistry {
    pub fn new(run_dir: &Path) -> Self {
        Self {
            path: run_dir.join("supervised.json"),
            write_lock: Mutex::new(()),
        }
    }

    /// Acquire the intra-process write lock, recovering from poisoning. The
    /// guarded data is `()`, so a poisoned lock carries no invalid state —
    /// a prior panic mid-critical-section leaves nothing to roll back; the
    /// next `load()` simply re-reads whatever is on disk.
    fn lock_for_write(&self) -> MutexGuard<'_, ()> {
        match self.write_lock.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// Load the snapshot, applying the boot gate + caps. Never errors on bad
    /// CONTENT — corrupt/non-UTF8/oversized/over-cap content is rotated
    /// aside and read back as an empty (current-boot) snapshot. A genuine
    /// I/O error (permission denied, `current_boot_id()` failing, etc.)
    /// still propagates via `?`.
    fn load(&self) -> io::Result<RegistrySnapshot> {
        let boot = platform::current_boot_id()?;
        let meta = match std::fs::metadata(&self.path) {
            Ok(m) => m,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(RegistrySnapshot {
                    boot_id: boot,
                    records: vec![],
                });
            }
            Err(e) => return Err(e),
        };
        if meta.len() > MAX_BYTES {
            return Ok(self.rotate_and_empty(boot));
        }
        // Read raw bytes first: a failure here (permission denied, etc.) is
        // a genuine I/O error and propagates via `?`. Non-UTF8 bytes, in
        // contrast, are bad file CONTENT — a plausible corruption mode, same
        // as truncated/garbled JSON — so a decode failure routes into the
        // SAME rotate-and-empty path as a JSON parse failure below, rather
        // than escaping as a raw `io::Error` out of a lossy `read_to_string`.
        let bytes = std::fs::read(&self.path)?;
        let text = match std::str::from_utf8(&bytes) {
            Ok(t) => t,
            Err(_) => return Ok(self.rotate_and_empty(boot)),
        };
        let snap: RegistrySnapshot = match serde_json::from_str(text) {
            Ok(s) => s,
            Err(_) => return Ok(self.rotate_and_empty(boot)),
        };
        if snap.records.len() > MAX_RECORDS {
            // `record()` enforces the cap at write time, so this is only
            // reachable via tampering/external corruption now — treat it
            // exactly like corrupt content: rotate aside for forensics and
            // do NOT `store()` an empty snapshot over it (that would destroy
            // the evidence with no trace it ever existed).
            return Ok(self.rotate_and_empty(boot));
        }
        if !snap.boot_id.matches(&boot) {
            // A different boot legitimately invalidates every record (no
            // orphan can exist across a reboot) — purge to an empty
            // snapshot under the current boot AND persist it. Unlike the
            // over-cap case above, this is routine, expected behavior, not
            // tampering, so overwriting is correct here.
            let empty = RegistrySnapshot {
                boot_id: boot,
                records: vec![],
            };
            let _ = self.store(&empty);
            return Ok(empty);
        }
        Ok(snap)
    }

    /// Rotate the current file aside as corrupt and return an empty
    /// snapshot tagged with `boot`. Deliberately does NOT call `store()` —
    /// callers use this for content that must be preserved out-of-band
    /// rather than overwritten (see call sites in `load()`).
    fn rotate_and_empty(&self, boot: BootId) -> RegistrySnapshot {
        self.rotate_corrupt();
        RegistrySnapshot {
            boot_id: boot,
            records: vec![],
        }
    }

    fn store(&self, snap: &RegistrySnapshot) -> io::Result<()> {
        let parent = self
            .path
            .parent()
            .ok_or_else(|| io::Error::other("registry path has no parent"))?;
        std::fs::create_dir_all(parent).map_err(|e| io_err("create_dir", parent, e))?;
        set_private_dir(parent);
        let json = serde_json::to_vec_pretty(snap).map_err(io::Error::other)?;
        // Atomic: temp in the same dir, then rename (repo golden rule 4).
        let tmp = self.path.with_extension("json.tmp");
        {
            let mut f = create_private(&tmp)?;
            f.write_all(&json).map_err(|e| io_err("write", &tmp, e))?;
            f.sync_all().map_err(|e| io_err("sync", &tmp, e))?;
        }
        std::fs::rename(&tmp, &self.path).map_err(|e| io_err("rename", &self.path, e))
    }

    fn rotate_corrupt(&self) {
        let _ = std::fs::rename(&self.path, self.path.with_extension("json.corrupt"));
    }
}

impl ProcessRegistry for FileRegistry {
    fn record(&self, rec: &SupervisedRecord) -> io::Result<()> {
        // Held across the whole load-mutate-store section below: see the
        // `write_lock` field doc for why (Task 4's concurrent per-service
        // spawns each call `record()`).
        let _guard = self.lock_for_write();
        let mut snap = self.load()?;
        let is_new = !snap.records.iter().any(|r| r.service_id == rec.service_id);
        if is_new && snap.records.len() >= MAX_RECORDS {
            // Preventive, not destructive: reject BEFORE writing anything.
            // An upsert of an already-present service_id is always allowed,
            // even at capacity — it replaces rather than grows.
            return Err(io::Error::other(format!(
                "registry at capacity ({MAX_RECORDS} records max); refusing new service_id {:?}",
                rec.service_id
            )));
        }
        snap.records.retain(|r| r.service_id != rec.service_id); // upsert
        snap.records.push(rec.clone());
        self.store(&snap)
    }
    fn remove(&self, service_id: &str) -> io::Result<()> {
        // Held across the whole load-mutate-store section below (same
        // rationale as `record()` above).
        let _guard = self.lock_for_write();
        let mut snap = self.load()?;
        let before = snap.records.len();
        snap.records.retain(|r| r.service_id != service_id);
        if snap.records.len() != before {
            self.store(&snap)?;
        }
        Ok(())
    }
    fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>> {
        // Read-only: no lock needed (see the `write_lock` field doc).
        Ok(self.load()?.records)
    }
}

// `#[allow(dead_code)]`: see the note above `MAX_BYTES` — no caller until
// Task 3.
#[allow(dead_code)]
fn io_err(op: &'static str, path: &Path, source: io::Error) -> io::Error {
    io::Error::new(source.kind(), format!("{op} {}: {source}", path.display()))
}

#[cfg(unix)]
#[allow(dead_code)]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}
#[cfg(not(unix))]
#[allow(dead_code)]
fn create_private(path: &Path) -> io::Result<std::fs::File> {
    std::fs::File::create(path)
}

#[cfg(unix)]
#[allow(dead_code)]
fn set_private_dir(dir: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
}
#[cfg(not(unix))]
#[allow(dead_code)]
fn set_private_dir(_dir: &Path) {}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::orphan::{ProcIdentity, ProcStartTime};

    fn rec(id: &str, pid: u32) -> SupervisedRecord {
        SupervisedRecord {
            service_id: id.to_string(),
            identity: ProcIdentity {
                pid,
                start_time: ProcStartTime::Unix {
                    sec: 1,
                    usec: pid as i64,
                },
            },
            recorded_at_ms: 0,
        }
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn record_upsert_remove_list() {
        let dir = tempfile::tempdir().unwrap();
        let r = FileRegistry::new(dir.path());
        r.record(&rec("nginx", 100)).unwrap();
        r.record(&rec("php-fpm", 200)).unwrap();
        r.record(&rec("nginx", 101)).unwrap(); // upsert by service_id
        let mut got = r.list_current_boot().unwrap();
        got.sort_by(|a, b| a.service_id.cmp(&b.service_id));
        assert_eq!(got.len(), 2);
        assert_eq!(
            got.iter()
                .find(|x| x.service_id == "nginx")
                .unwrap()
                .identity
                .pid,
            101
        );
        r.remove("nginx").unwrap();
        assert_eq!(r.list_current_boot().unwrap().len(), 1);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn stale_boot_purges_and_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        // Hand-write a snapshot with a wildly different boot time.
        let snap = RegistrySnapshot {
            boot_id: crate::orphan::BootId::Unix { sec: 1, usec: 0 },
            records: vec![rec("nginx", 100)],
        };
        std::fs::write(&path, serde_json::to_string(&snap).unwrap()).unwrap();
        let r = FileRegistry::new(dir.path());
        assert!(
            r.list_current_boot().unwrap().is_empty(),
            "stale boot -> empty"
        );
        // File is purged (no records under the current boot).
        let after: RegistrySnapshot =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after.records.is_empty());
    }

    #[test]
    fn corrupt_file_rotates_and_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        std::fs::write(&path, b"{ this is not json").unwrap();
        let r = FileRegistry::new(dir.path());
        assert!(r.list_current_boot().unwrap().is_empty());
        assert!(
            dir.path().join("supervised.json.corrupt").exists(),
            "rotated aside"
        );
    }

    #[test]
    fn oversized_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        std::fs::write(&path, vec![b'x'; 65 * 1024]).unwrap(); // > 64 KiB cap
        let r = FileRegistry::new(dir.path());
        assert!(r.list_current_boot().unwrap().is_empty());
        assert!(
            dir.path().join("supervised.json.corrupt").exists(),
            "rotated aside"
        );
    }

    #[test]
    fn non_utf8_file_rotates_and_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        std::fs::write(&path, [0xff, 0xfe, 0xff]).unwrap(); // invalid UTF-8
        let r = FileRegistry::new(dir.path());
        assert!(r.list_current_boot().unwrap().is_empty());
        assert!(
            dir.path().join("supervised.json.corrupt").exists(),
            "rotated aside"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn record_rejects_new_service_beyond_cap() {
        let dir = tempfile::tempdir().unwrap();
        let r = FileRegistry::new(dir.path());
        for i in 0..MAX_RECORDS {
            r.record(&rec(&format!("svc-{i}"), 100 + i as u32)).unwrap();
        }
        assert_eq!(r.list_current_boot().unwrap().len(), MAX_RECORDS);

        // A 65th DISTINCT service_id is rejected, and nothing is written.
        assert!(
            r.record(&rec("svc-overflow", 9_999)).is_err(),
            "65th distinct service_id must be rejected"
        );
        assert_eq!(
            r.list_current_boot().unwrap().len(),
            MAX_RECORDS,
            "a rejected record must not be persisted"
        );

        // An upsert of an ALREADY-PRESENT id must still succeed at capacity.
        r.record(&rec("svc-0", 12_345)).unwrap();
        let got = r.list_current_boot().unwrap();
        assert_eq!(
            got.len(),
            MAX_RECORDS,
            "upsert at capacity must not grow the registry"
        );
        assert_eq!(
            got.iter()
                .find(|x| x.service_id == "svc-0")
                .unwrap()
                .identity
                .pid,
            12_345,
            "upsert at capacity must replace, not append"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn over_cap_file_rotates_and_reads_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("supervised.json");
        // 65 records under a boot_id that MATCHES the current boot: proves
        // the over-cap branch fires independent of the boot-mismatch branch,
        // and that it rotates aside rather than purging-and-persisting.
        let boot = crate::platform::current_boot_id().unwrap();
        let records: Vec<SupervisedRecord> = (0..=MAX_RECORDS)
            .map(|i| rec(&format!("svc-{i}"), 100 + i as u32))
            .collect();
        let snap = RegistrySnapshot {
            boot_id: boot,
            records,
        };
        std::fs::write(&path, serde_json::to_string(&snap).unwrap()).unwrap();
        let r = FileRegistry::new(dir.path());
        assert!(
            r.list_current_boot().unwrap().is_empty(),
            "over-cap file -> empty read"
        );
        assert!(
            dir.path().join("supervised.json.corrupt").exists(),
            "rotated aside"
        );
        assert!(
            !path.exists(),
            "must not store() a fresh file over the rotated original"
        );
    }

    #[cfg(unix)]
    #[test]
    fn store_sets_private_file_and_dir_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let run_dir = dir.path().join("run"); // does not exist yet
        let r = FileRegistry::new(&run_dir);
        r.record(&rec("nginx", 100)).unwrap();

        let file_mode = std::fs::metadata(run_dir.join("supervised.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(file_mode, 0o600, "registry file must be 0600");

        let dir_mode = std::fs::metadata(&run_dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "registry dir must be 0700");
    }
}
