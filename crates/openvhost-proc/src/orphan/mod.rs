// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash-orphan cleanup: persist supervised (pid, start-time, boot-id) and, on
//! the next start, reap confirmed orphans — killing ONLY after an identity
//! match and the safety gate in `reap` (spec §6). This is a SIGKILL-from-a-file
//! path; every check here closes a specific false-kill scenario.

use std::io;

use serde::{Deserialize, Serialize};

/// Start-time identity token that defeats PID reuse. Tagged so a registry
/// written under one OS cannot be misread as the other's numeric shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "camelCase")]
pub enum ProcStartTime {
    Unix { sec: i64, usec: i64 },
    Windows { creation_filetime: u64 },
}

/// Boot identity: registry records are only actionable within the same boot
/// (after a reboot no orphan can exist). macOS uses kern.boottime.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "os", rename_all = "camelCase")]
pub enum BootId {
    Unix { sec: i64, usec: i64 },
    Windows { boot_ms: u64 },
}

impl BootId {
    /// Same boot within a small tolerance (kern.boottime shifts on clock steps).
    pub fn matches(&self, other: &BootId) -> bool {
        match (self, other) {
            (BootId::Unix { sec: a, .. }, BootId::Unix { sec: b, .. }) => (a - b).abs() <= 5,
            (BootId::Windows { boot_ms: a }, BootId::Windows { boot_ms: b }) => {
                a.abs_diff(*b) <= 5000
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcIdentity {
    /// INVARIANT: services spawn as group leaders (`process_group(0)`), so
    /// `pgid == pid` and `kill(-pid)` hits the whole tree. Re-verified via
    /// `getpgid` at reap — never trusted on the spawn invariant alone.
    pub pid: u32,
    pub start_time: ProcStartTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SupervisedRecord {
    pub service_id: String,
    pub identity: ProcIdentity,
    pub recorded_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegistrySnapshot {
    pub boot_id: BootId,
    pub records: Vec<SupervisedRecord>,
}

/// Persistence only — no kill logic. state.db can implement this later.
///
/// `#[allow(dead_code)]` dropped at the trait level (P0-8 Task 3):
/// `reap_orphans` (the `#[cfg(unix)]` body) now calls
/// `remove`/`list_current_boot` through a `&dyn ProcessRegistry` — real,
/// dynamically-dispatched production call sites, on unix only. On
/// `#[cfg(not(unix))]` (Windows), `reap_orphans` is the stub that ignores its
/// `_registry` parameter entirely, so `remove`/`list_current_boot` have no
/// real caller THERE either — hence the `cfg_attr` below, empirically
/// verified against both the native macOS and the msvc `--lib` builds.
/// `record` keeps an unconditional allow: nothing calls it through the trait
/// object on EITHER platform yet (Task 4 adds spawn-time recording, spec §9).
/// `FileRegistry` (the concrete impl) keeps its own separate allow — its
/// only production caller (any method) arrives in Task 4.
pub trait ProcessRegistry: Send + Sync {
    // `#[allow(dead_code)]`: no call site through `&dyn ProcessRegistry` on
    // either platform yet — Task 4 wires in record-at-spawn (spec §9), the
    // real caller.
    #[allow(dead_code)]
    fn record(&self, rec: &SupervisedRecord) -> io::Result<()>; // upsert by service_id
    // `#[cfg_attr(not(unix), allow(dead_code))]`: real caller on unix
    // (`reap_orphans`'s `#[cfg(unix)]` body); the `#[cfg(not(unix))]` stub
    // ignores its registry parameter, so this has no real caller on Windows.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn remove(&self, service_id: &str) -> io::Result<()>;
    /// Records for the CURRENT boot only; a stale boot_id purges the file and
    /// returns empty. Never errors on a corrupt/oversized file — rotates it
    /// aside and returns empty.
    #[cfg_attr(not(unix), allow(dead_code))]
    fn list_current_boot(&self) -> io::Result<Vec<SupervisedRecord>>;
}

pub(crate) mod registry;
#[allow(unused_imports)] // see ProcessRegistry's dead_code note above
pub use registry::FileRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReapKind {
    Group,
    SinglePidFallback,
}

/// Platform-specific KILL of an identity-verified orphan (a bare pid; an orphan
/// has no live `SpawnedChild`). Unix: getpgid re-check then group/single kill.
pub trait OrphanReaper: Send + Sync {
    fn reap(&self, pid: u32) -> std::io::Result<ReapKind>;
}

/// `#[allow(dead_code)]`: never constructed outside `reap_orphans` (below),
/// which itself has no production caller yet — `Supervisor::new` (Task 4 of
/// P0-8) is the real caller. Until it lands, the only user is
/// `reap::tests`, a `#[cfg(test)]` module invisible to the dead-code pass on
/// the plain (non-`--test`) build of this crate, same mechanism as the
/// `ProcessRegistry` trait above. Drop this allow once Task 4 wires in the
/// real caller.
#[allow(dead_code)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ReapReport {
    pub killed_group: u32,
    pub killed_single: u32,
    pub killed_headless: u32, // dead leader, surviving group members killed
    pub skipped_dead: u32,
    pub skipped_reused: u32,
    pub rejected: u32,
    pub errored: u32,
}

pub(crate) mod reap;
// `#[allow(unused_imports)]`: see `ReapReport`'s dead_code note above —
// `reap_orphans` has no production caller until Task 4.
#[allow(unused_imports)]
pub use reap::reap_orphans;

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn start_time_serde_round_trip_both_variants() {
        for st in [
            ProcStartTime::Unix {
                sec: 1_700_000_000,
                usec: 123456,
            },
            ProcStartTime::Windows {
                creation_filetime: 133_000_000_000_000_000,
            },
        ] {
            let j = serde_json::to_string(&st).unwrap();
            assert_eq!(serde_json::from_str::<ProcStartTime>(&j).unwrap(), st);
        }
        // A record written as Unix must NOT silently deserialize as Windows.
        let unix = serde_json::to_string(&ProcStartTime::Unix { sec: 1, usec: 2 }).unwrap();
        assert!(unix.contains("\"os\":\"unix\""));
    }

    #[test]
    fn boot_id_tolerance() {
        let a = BootId::Unix { sec: 1000, usec: 0 };
        assert!(a.matches(&BootId::Unix {
            sec: 1003,
            usec: 999
        })); // within 5s
        assert!(!a.matches(&BootId::Unix { sec: 1010, usec: 0 })); // beyond 5s
        assert!(!a.matches(&BootId::Windows { boot_ms: 1_000_000 })); // cross-os never
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_time_of_self_is_some_and_stable() {
        let me = std::process::id();
        let a = crate::platform::process_start_time(me).unwrap();
        assert!(a.is_some(), "our own pid must have a start time");
        let b = crate::platform::process_start_time(me).unwrap();
        assert_eq!(a, b, "start time is stable across reads");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn start_time_of_dead_pid_is_none() {
        // A pid that cannot be alive (max pid space is ~99999 on macOS).
        assert!(
            crate::platform::process_start_time(999_999)
                .unwrap()
                .is_none()
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn current_boot_id_reads_and_self_matches() {
        let b = crate::platform::current_boot_id().unwrap();
        assert!(b.matches(&b));
        // Our own process group id is readable and equals our pgid.
        let pg = crate::platform::getpgid(std::process::id()).unwrap();
        assert!(pg > 0);
    }
}
