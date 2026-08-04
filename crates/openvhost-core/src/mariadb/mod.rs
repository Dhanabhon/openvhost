// SPDX-License-Identifier: GPL-3.0-or-later
//! MariaDB: the compiled-in package pin and the wiring that installs it into
//! `<home>/packages/mariadb/11.4/<version>/`. See
//! docs/superpowers/specs/2026-08-02-p2-build-pipeline-design.md (D5, §13,
//! §14).
//!
//! **Installing a runtime is not running one.** `paths`, `datadir` and
//! `discover` (the service slice's first task) answer *where* MariaDB's state
//! lives, *what state a datadir is actually in*, and *which binaries to drive*
//! — and nothing more: initialization, credentials and supervision follow. As
//! the note above predicted, they sit beside `package` rather than reshuffling
//! it.
//!
//! **Nothing in this module touches a datadir, a credential or `<home>/logs/`
//! on any path, including error paths.** `datadir` reads; it never writes,
//! renames or removes, and its test suite asserts that by inode as well as by
//! content.
//!
//! [`crate::PackageTarget`] is reused rather than redefined. It answers "which
//! OS/architecture pair can a prebuilt package be published for", which is a
//! question about packages and not about MySQL — and a second copy of the enum
//! would mean a new variant broke compilation at only half the sites that have
//! to decide about it. Its declaration living under `mysql/` is the wrong home
//! for the same reason [`crate::mysql::InstallLedger`]'s is; both are worth
//! moving together, and neither is this slice's business.

mod datadir;
mod discover;
mod init;
mod package;
mod paths;
mod repo;

pub use datadir::{MariadbDatadirState, classify_mariadb_datadir};
pub use discover::{MariadbRuntime, discover_mariadb, packaged_mariadb_runtime};
pub use init::{
    MariadbInitCtx, MariadbInitOutcome, MariadbInitStep, MariadbRuntimeDirs,
    finalize_mariadb_staging, initialize_mariadb, mariadb_install_db_path, mariadb_runtime_dirs,
    mariadb_staging_dir_path, root_password_sql,
};
pub use package::{
    Availability, MARIADB_PACKAGE_NAME, MARIADB_PACKAGES, MARIADB_SERIES, MARIADB_WARMUP_BINARY,
    MariadbPackage, MariadbPackageInstall, install_mariadb_package, mariadb_package_for_host,
    mariadb_package_for_target,
};
pub use paths::{MariadbPaths, mariadb_data_root, mariadb_paths};
pub use repo::{MariadbInstance, MariadbInstanceRepo};
