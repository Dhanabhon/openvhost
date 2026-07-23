// SPDX-License-Identifier: GPL-3.0-or-later
//! File-backed process registry: one atomic JSON file at
//! `<run>/supervised.json`, boot-gated on load, size/count capped.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::{ProcessRegistry, RegistrySnapshot, SupervisedRecord};
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
}

#[allow(dead_code)]
impl FileRegistry {
    pub fn new(run_dir: &Path) -> Self {
        Self {
            path: run_dir.join("supervised.json"),
        }
    }

    /// Load the snapshot, applying the boot gate + caps. Never errors on bad
    /// content — rotates aside and returns an empty (current-boot) snapshot.
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
            self.rotate_corrupt();
            return Ok(RegistrySnapshot {
                boot_id: boot,
                records: vec![],
            });
        }
        let text = std::fs::read_to_string(&self.path)?;
        let snap: RegistrySnapshot = match serde_json::from_str(&text) {
            Ok(s) => s,
            Err(_) => {
                self.rotate_corrupt();
                return Ok(RegistrySnapshot {
                    boot_id: boot,
                    records: vec![],
                });
            }
        };
        if snap.records.len() > MAX_RECORDS || !snap.boot_id.matches(&boot) {
            // Too many records, or a different boot: nothing is actionable.
            // Purge to the current (empty) boot and persist.
            let empty = RegistrySnapshot {
                boot_id: boot,
                records: vec![],
            };
            let _ = self.store(&empty);
            return Ok(empty);
        }
        Ok(snap)
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
        let mut snap = self.load()?;
        snap.records.retain(|r| r.service_id != rec.service_id); // upsert
        snap.records.push(rec.clone());
        self.store(&snap)
    }
    fn remove(&self, service_id: &str) -> io::Result<()> {
        let mut snap = self.load()?;
        let before = snap.records.len();
        snap.records.retain(|r| r.service_id != service_id);
        if snap.records.len() != before {
            self.store(&snap)?;
        }
        Ok(())
    }
    fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>> {
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
    }
}
