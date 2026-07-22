// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-conf — generated-config engine (Tera templates → nginx + php-fpm
//! configs) with a native-validator pass. Pure generation: same input ⇒
//! byte-identical output; never reads prior generated output. See
//! docs/superpowers/specs/2026-07-23-p07-tera-templates-design.md.

mod ctx;
mod engine;
mod error;
mod phpruntime;
mod webserver;

pub use ctx::{GeneratedFile, PhpUpstream, RenderCtx, ValidationReport};
pub use error::ConfError;
pub use phpruntime::{PhpFpmRuntime, PhpRuntimeAdapter};
pub use webserver::{NginxAdapter, WebServerAdapter};
