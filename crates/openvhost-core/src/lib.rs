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
// MariaDB from OpenVHost's OWN build (upstream publishes no macOS binaries):
// the compiled-in pin and the wiring that installs it. Catalogue and install
// only — the service itself is the next slice.
pub mod mariadb;
pub mod mysql;
// nginx from OpenVHost's OWN build (upstream publishes no macOS binaries
// either): the compiled-in pin and the wiring that installs it. Catalogue and
// install only — discovery and replacing Homebrew at runtime are later
// slices.
pub mod nginx;
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
/// MariaDB's package and service surface. Nothing here is wired to a Tauri
/// command, a CLI subcommand or a UI control, and the install path may not be
/// until the owner publishes the release the catalogue pins — see
/// `mariadb::Availability`. Exported so the pin is inspectable (and testable)
/// without exposing an install a user could trigger into a 404.
///
/// `classify_mariadb_datadir` is the one function here whose verdict authorises
/// a destructive next step, and it is deliberately the only one: nothing else
/// in this list writes anything at all.
pub use mariadb::{
    Availability, MARIADB_PACKAGE_NAME, MARIADB_PACKAGES, MARIADB_SERIES, MARIADB_WARMUP_BINARY,
    MariadbDatadirState, MariadbPackage, MariadbPackageInstall, MariadbPaths, MariadbRuntime,
    classify_mariadb_datadir, discover_mariadb, install_mariadb_package, mariadb_data_root,
    mariadb_package_for_host, mariadb_package_for_target, mariadb_paths, packaged_mariadb_runtime,
};
pub use mysql::{
    DatadirState, InstallLedger, LedgerEntry, LedgerWrite, MYSQL_CATALOGUE, MYSQL_PACKAGE_NAME,
    MYSQL_PACKAGES, MYSQL_WARMUP_BINARY, MysqlInitOutcome, MysqlInitStep, MysqlInstance,
    MysqlInstanceRepo, MysqlMajor, MysqlPackage, MysqlPackageInstall, MysqlPaths, MysqlRuntime,
    MysqlRuntimeSource, PackageTarget, RootPassword, alter_user_sql, brew_mysql_runtime_for_major,
    classify_datadir, discover_mysql, finalize_staging, generate_root_password,
    install_mysql_package, mysql_brew_formula, mysql_brew_install_spec, mysql_brew_uninstall_spec,
    mysql_package_for_host, mysql_package_for_target, mysql_paths, packaged_mysql_runtime,
    remove_staging_dir, staging_dir_path, sweep_stale_staging,
};
/// nginx's package and discovery surface. Nothing here is wired to a Tauri
/// command, a CLI subcommand or a UI control, and the install path may not be
/// until the owner publishes the release the catalogue pins — see
/// `nginx::Availability`. Exported so the pin is inspectable (and testable)
/// without exposing an install a user could trigger into a 404.
///
/// `discover_nginx`/`packaged_nginx_runtime`/`NginxRuntime`/
/// `NginxRuntimeSource` (off-Homebrew slice 4B) need no such gate: they only
/// ever READ the package tree and a Homebrew prefix, so they are safe to call
/// — and are called, by `apps/desktop/src-tauri/src/stack.rs`'s startup
/// seam — whether or not a packaged nginx has ever been published.
///
/// **`Availability` is deliberately absent from this list.** nginx's copy and
/// [`mariadb::Availability`] above are two distinct types that happen to share
/// a name (design decision: duplicated per package, not shared), and Rust
/// cannot flatten two same-named items into one module's namespace. It stays
/// reachable at `nginx::Availability`, not `crate::Availability`.
pub use nginx::{
    NGINX_PACKAGE_NAME, NGINX_PACKAGES, NGINX_SERIES, NGINX_WARMUP_BINARY, NginxPackage,
    NginxPackageInstall, NginxRuntime, NginxRuntimeSource, discover_nginx, install_nginx_package,
    nginx_package_for_host, nginx_package_for_target, nginx_prefix_dir, nginx_spawn_argv,
    packaged_nginx_runtime,
};
/// The package-pipeline types that appear in this crate's own public
/// signatures, re-exported so the desktop app and the CLI need no direct
/// dependency on `openvhost-pkg`. Nothing here lets a caller choose a URL or a
/// hash: [`PackagesRoot`] is minted from a resolved home, and
/// [`install_mysql_package`] takes a [`MysqlMajor`].
pub use openvhost_pkg::{ArchiveFormat, InstalledPackage, PackagesRoot, PkgError, Progress};
/// PHP's package-tree install surface, alongside its existing Homebrew
/// surface. Nothing here is wired to a Tauri command, a CLI subcommand or a
/// UI control, and the install path may not be until the owner publishes the
/// release the catalogue pins — see `php::Availability`. Exported so the pin
/// is inspectable (and testable) without exposing an install a user could
/// trigger into a 404.
///
/// `discover_php`/`PhpRuntimeSource` (off-Homebrew slice 5B) need no such
/// gate, exactly like nginx's discovery surface above: they only ever READ the
/// package tree and a Homebrew prefix, so they are safe to call whether or not
/// a packaged PHP has ever been published.
///
/// **`Availability` is deliberately absent from this list**, the same as
/// [`nginx::Availability`]: [`mariadb::Availability`] already holds the
/// `Availability` name at the crate root, and PHP's copy is a distinct type
/// that happens to share the name (design decision: duplicated per package,
/// not shared). It stays reachable at `php::Availability`.
pub use php::{
    BREW_PREFIXES, CATALOGUE, PHP_PACKAGE_NAME, PHP_PACKAGES, PHP_WARMUP_BINARY, PhpMajor,
    PhpPackage, PhpPackageInstall, PhpRuntimeSource, brew_formula, brew_install_spec,
    brew_uninstall_spec, discover_php, discover_php_in, find_brew, install_php_package,
    php_package_for_host, php_package_for_target, php_runtime_for_major,
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
