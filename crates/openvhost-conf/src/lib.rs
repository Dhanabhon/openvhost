// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-conf — generated-config engine (Tera templates → nginx + php-fpm
//! configs) with a native-validator pass. Pure generation: same input ⇒
//! byte-identical output; never reads prior generated output. See
//! docs/superpowers/specs/2026-07-23-p07-tera-templates-design.md.

mod ctx;
mod engine;
mod error;
mod inspect;
mod mariadb;
mod mysql;
mod phpruntime;
pub mod settings;
#[cfg(all(test, unix))]
mod tests_support;
mod validate;
mod webserver;

pub use ctx::{GeneratedFile, PhpUpstream, RenderCtx, ValidationReport};
pub use error::ConfError;
pub use inspect::{
    PROBE_TIMEOUT, probe_mysqld_version, probe_nginx_version, probe_php_fpm_version, run_bounded,
    validate_live,
};
pub use mariadb::{MariadbCtx, generate_mariadb_my_cnf};
pub use mysql::{MysqlCtx, MysqlValidator, generate_my_cnf};
pub use phpruntime::{PhpFpmRuntime, PhpRuntimeAdapter};
pub use settings::{
    BodySize, GzipLevel, GzipTypes, OnOff, Seconds, SettingsCheck, WebServerSettings,
    WorkerConnections, check_settings,
};
pub use validate::{BrewStack, find_brew_binaries};
pub use webserver::{NginxAdapter, WebServerAdapter};
