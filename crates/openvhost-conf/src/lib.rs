// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-conf — config generator (stub).
//!
//! Responsibility (master plan §3.1): Tera templates → generated configs,
//! atomic writes, native-validator + diff-preview pipeline, WebServerAdapter
//! boundary. Implementation lands in the P0-7 slice.

/// Crate marker used until the config generator slice lands.
pub const CRATE_NAME: &str = "openvhost-conf";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::CRATE_NAME, "openvhost-conf");
    }
}
