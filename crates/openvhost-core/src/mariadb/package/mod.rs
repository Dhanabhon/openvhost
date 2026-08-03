// SPDX-License-Identifier: GPL-3.0-or-later
//! MariaDB from OpenVHost's own package tree — and, unlike MySQL, from
//! OpenVHost's own **build**: MariaDB publishes no macOS binaries at all, so
//! the artifact this module installs is one `build/recipes/mariadb.sh` produced
//! and `build/audit.sh` accepted (build-pipeline design D5).
//!
//! Two pieces, in the order they run:
//!
//! - [`catalogue`] — the compiled-in pin: which build we install for which
//!   host, from where, what its bytes must hash to, whether the release exists
//!   yet, and the two dates spec §14's security obligation fires on. Nothing a
//!   user supplies can reach any of those values.
//! - [`install`] — the wiring to `openvhost-pkg`'s download → verify → extract
//!   → atomic-install pipeline. This module adds no install machinery; it is
//!   that pipeline's second consumer and reuses it unchanged.
//!
//! There is no `ledger` here: [`crate::mysql::InstallLedger`] is keyed by
//! package name and already records any package, so a second one would be a
//! second shape for one fact. Its home under `mysql/` is now the wrong place
//! for it — a mechanical move worth doing when a third package joins, not part
//! of this slice.

mod catalogue;
mod install;

pub use catalogue::{
    Availability, MARIADB_PACKAGE_NAME, MARIADB_PACKAGES, MARIADB_SERIES, MARIADB_WARMUP_BINARY,
    MariadbPackage, mariadb_package_for_host, mariadb_package_for_target,
};
pub use install::{MariadbPackageInstall, install_mariadb_package};
