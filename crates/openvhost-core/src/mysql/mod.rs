// SPDX-License-Identifier: GPL-3.0-or-later
//! MySQL runtimes: which are installed and how to install another
//! (`brew`/`discover`); where MySQL's on-disk state lives, what state a
//! datadir is actually in, and cleanup of abandoned staged-init directories
//! (`datadir`). See docs/superpowers/specs/2026-07-29-p1-db-mysql-design.md
//! (D1, D2).

mod brew;
mod datadir;
mod discover;

pub use brew::{MYSQL_CATALOGUE, MysqlMajor, mysql_brew_install_spec};
pub use datadir::{DatadirState, MysqlPaths, classify_datadir, mysql_paths, sweep_stale_staging};
pub use discover::{MysqlRuntime, discover_mysql};
