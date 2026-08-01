// SPDX-License-Identifier: GPL-3.0-or-later
//! MySQL runtimes: which are installed and how to install another
//! (`brew`/`discover`); where MySQL's on-disk state lives, what state a
//! datadir is actually in, and cleanup of abandoned staged-init directories
//! (`datadir`); the staged-init sequencing primitives and generated root
//! credential (`init`); and that credential's persistence in `state.db`
//! (`repo`). See docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md
//! (D1, D2, D3).

mod brew;
mod datadir;
mod discover;
mod init;
// MySQL from OpenVHost's own package tree — the compiled-in catalogue, the
// wiring to `openvhost-pkg`'s verified download pipeline, and the install
// ledger. Homebrew (`brew`, above) stays as a parallel source during the
// migration; see `package`'s own header and design D3/D7.
mod package;
mod repo;

pub use brew::{
    MYSQL_CATALOGUE, MysqlMajor, mysql_brew_formula, mysql_brew_install_spec,
    mysql_brew_uninstall_spec,
};
pub use datadir::{
    DatadirState, MysqlPaths, classify_datadir, mysql_data_root, mysql_paths, sweep_stale_staging,
};
pub use discover::{
    MysqlRuntime, MysqlRuntimeSource, brew_mysql_runtime_for_major, discover_mysql,
    packaged_mysql_runtime,
};
pub use init::{
    MysqlInitOutcome, MysqlInitStep, RootPassword, alter_user_sql, finalize_staging,
    generate_root_password, remove_staging_dir, staging_dir_path, write_generated_config,
};
pub use package::{
    InstallLedger, LedgerEntry, LedgerWrite, MYSQL_PACKAGE_NAME, MYSQL_PACKAGES,
    MYSQL_WARMUP_BINARY, MysqlPackage, MysqlPackageInstall, PackageTarget, install_mysql_package,
    mysql_package_for_host, mysql_package_for_target,
};
pub use repo::{MysqlInstance, MysqlInstanceRepo};
