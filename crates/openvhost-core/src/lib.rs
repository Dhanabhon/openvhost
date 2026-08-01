// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-core — domain model and state for OpenVHost.
//!
//! Responsibility (master plan §3.1): domain model, SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and the
//! openvhost CLI. Current slice: home-directory resolution + CoreInfo.

mod atomicfile;
// The single chokepoint every `brew` argv/env in this crate is composed
// through (`php::brew` and `mysql::brew` are thin formula-naming wrappers over
// it). Private: nothing outside this crate may name a verb or a formula.
mod brew_cmd;
pub mod db;
pub mod discovery;
mod error;
mod home;
mod info;
// What a Homebrew `opt/<formula>` link actually resolves to: the version,
// without executing anything, and WHOSE keg it is — which is what stops
// `brew uninstall php@8.5` from removing an aliased, unversioned `php`.
pub mod keg;
pub mod logs;
pub mod mysql;
pub mod php;
pub mod settings_repo;
pub mod site;

pub use db::Db;
pub use discovery::Discovery;
pub use error::CoreError;
pub use home::{home_disk_usage, resolve_home};
pub use info::{CoreInfo, core_info};
pub use keg::{KegProvenance, ResolvedKeg, keg_provenance, resolve_keg};
pub use logs::{
    LogCursor, LogLevel, LogLimits, LogPaths, LogQuery, LogReset, LogRow, LogWindow,
    classify_level, ensure_log_dir, read_window,
};
pub use mysql::{
    DatadirState, MYSQL_CATALOGUE, MysqlInitOutcome, MysqlInitStep, MysqlInstance,
    MysqlInstanceRepo, MysqlMajor, MysqlPaths, MysqlRuntime, RootPassword, alter_user_sql,
    classify_datadir, discover_mysql, finalize_staging, generate_root_password, mysql_brew_formula,
    mysql_brew_install_spec, mysql_brew_uninstall_spec, mysql_paths, mysql_runtime_for_major,
    remove_staging_dir, staging_dir_path, sweep_stale_staging,
};
pub use php::{
    BREW_PREFIXES, CATALOGUE, PhpMajor, brew_formula, brew_install_spec, brew_uninstall_spec,
    discover_php_in, find_brew, php_runtime_for_major,
};
pub use settings_repo::{SqliteWebServerSettings, WebServerSettingsRepository};
pub use site::apply::{
    ApplyError, ApplyInput, ApplyOutcome, ApplyPlan, ChangeKind, ConfigValidator, FileChange,
    InstalledRuntimes, NginxValidator, PhpRuntime, RollbackReport, apply, commit, plan, render_set,
    rollback,
};
pub use site::repo::{SiteRepository, SqliteSiteRepository};
pub use site::{Docroot, Domain, NewSite, PhpVersion, Site, SiteId, SiteName, WebServer};

pub mod platform;
