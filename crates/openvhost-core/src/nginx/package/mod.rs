// SPDX-License-Identifier: GPL-3.0-or-later
//! nginx from OpenVHost's own package tree.
//!
//! Two pieces, in the order they run:
//!
//! - [`catalogue`] — the compiled-in pin: which build we install for which
//!   host, from where, what its bytes must hash to, whether the release exists
//!   yet, and the two dates spec §14's security obligation fires on. Nothing a
//!   user supplies can reach any of those values.
//! - [`install`] — the wiring to `openvhost-pkg`'s download → verify → extract
//!   → atomic-install pipeline. This module adds no install machinery; it is
//!   that pipeline's third consumer and reuses it unchanged.
//!
//! There is no `ledger` here: [`crate::mysql::InstallLedger`] is keyed by
//! package name and already records any package, so a second one would be a
//! second shape for one fact — the same reasoning [`crate::mariadb`] already
//! gives for reusing it rather than growing a copy.

mod catalogue;
mod install;

pub use catalogue::{
    Availability, NGINX_PACKAGE_NAME, NGINX_PACKAGES, NGINX_SERIES, NGINX_WARMUP_BINARY,
    NginxPackage, nginx_package_for_host, nginx_package_for_target,
};
pub use install::{NginxPackageInstall, install_nginx_package};
