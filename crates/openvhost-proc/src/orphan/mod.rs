// SPDX-License-Identifier: GPL-3.0-or-later
//! Crash-orphan cleanup: persist supervised (pid, start-time, boot-id) and, on
//! the next start, reap confirmed orphans — killing ONLY after an identity
//! match and the safety gate in `reap` (spec §6). This is a SIGKILL-from-a-file
//! path; every check here closes a specific false-kill scenario.

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
