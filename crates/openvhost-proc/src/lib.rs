// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-proc — process supervisor (stub).
//!
//! Responsibility (master plan §3.1): spawn/stop/restart/status for every
//! managed service; state machine Stopped → Starting → Running → Failed;
//! graceful shutdown; orphan cleanup; Windows Job Objects; log capture;
//! health checks. Every child process in the codebase is spawned through
//! this crate — implementation lands in the P0-3 slice.

/// Crate marker used until the supervisor slice lands.
pub const CRATE_NAME: &str = "openvhost-proc";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::CRATE_NAME, "openvhost-proc");
    }
}
