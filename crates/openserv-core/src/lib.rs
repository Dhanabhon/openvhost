// SPDX-License-Identifier: GPL-3.0-or-later
//! openserv-core — domain model and state (stub until Task 3).
//!
//! Responsibility (master plan §3.1): domain model (Site, ServicePackage,
//! ServiceInstance, Certificate, HostsEntry), SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and
//! the openservctl CLI.

/// Crate marker; replaced by real API in Task 3.
pub const CRATE_NAME: &str = "openserv-core";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::CRATE_NAME, "openserv-core");
    }
}
