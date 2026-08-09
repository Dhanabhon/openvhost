// SPDX-License-Identifier: GPL-3.0-or-later
//! MySQL from OpenVHost's own package tree, instead of Homebrew.
//!
//! Three pieces, in the order they run:
//!
//! - [`catalogue`] — the compiled-in pins: which upstream build we install for
//!   which host, from where, and what its bytes must hash to. Nothing a user
//!   supplies can reach any of those values.
//! - [`install`] — the wiring to `openvhost-pkg`'s download → verify → extract
//!   → atomic-install pipeline. This module adds no install machinery; it is
//!   that pipeline's first production consumer.
//! - [`ledger`] — what we installed and when, recorded because we asked for a
//!   specific version rather than discovered one (design D4).
//!
//! Homebrew remains a parallel, untouched source (design D7): nothing here
//! uninstalls, relinks or migrates a keg. Two install sources coexisting is
//! the intended state during the migration.

mod catalogue;
mod install;
mod ledger;

pub use catalogue::{
    MYSQL_PACKAGE_NAME, MYSQL_PACKAGES, MYSQL_WARMUP_BINARY, MysqlPackage, PackageTarget,
    mysql_package_for_host, mysql_package_for_target,
};
/// Crate-internal: the `reason` a [`LedgerWrite::Failed`] carries when there was
/// no store to write to. Shared with [`crate::mariadb`] and [`crate::php`] so
/// the three engines' `None` arms say one sentence rather than three.
pub(crate) use install::NO_LEDGER_REASON;
pub use install::{LedgerWrite, MysqlPackageInstall, install_mysql_package};
pub use ledger::{InstallLedger, LedgerEntry};
