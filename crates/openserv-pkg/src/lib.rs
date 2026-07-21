// SPDX-License-Identifier: GPL-3.0-or-later
//! openserv-pkg — package manager (stub).
//!
//! Responsibility (master plan §3.1): download → SHA-256 verify → extract;
//! packages/<name>/<major>/<full>/ layout with a current link per major;
//! install/uninstall/upgrade/disable. Implementation lands in the P0-6 slice.

/// Crate marker used until the package manager slice lands.
pub const CRATE_NAME: &str = "openserv-pkg";

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[test]
    fn crate_name_is_stable() {
        assert_eq!(super::CRATE_NAME, "openserv-pkg");
    }
}
