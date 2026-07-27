// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-core — domain model and state for OpenVHost.
//!
//! Responsibility (master plan §3.1): domain model, SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and the
//! openvhost CLI. Current slice: home-directory resolution + CoreInfo.

pub mod db;
mod error;
mod home;
mod info;
pub mod php;
pub mod site;

pub use db::Db;
pub use error::CoreError;
pub use home::{home_disk_usage, resolve_home};
pub use info::{CoreInfo, core_info};
pub use php::{BREW_PREFIXES, discover_php_in};
pub use site::apply::{
    ApplyError, ApplyInput, ApplyOutcome, ApplyPlan, ChangeKind, ConfigValidator, FileChange,
    InstalledRuntimes, NginxValidator, PhpRuntime, RollbackReport, apply, commit, plan, render_set,
    rollback,
};
pub use site::repo::{SiteRepository, SqliteSiteRepository};
pub use site::{Docroot, Domain, NewSite, PhpVersion, Site, SiteId, SiteName, WebServer};

pub mod platform;
