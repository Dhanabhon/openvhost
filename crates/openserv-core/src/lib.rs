// SPDX-License-Identifier: GPL-3.0-or-later
//! openserv-core — domain model and state for OpenServ.
//!
//! Responsibility (master plan §3.1): domain model, SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and the
//! openservctl CLI. Current slice: home-directory resolution + CoreInfo.

mod error;
mod home;
mod info;

pub use error::CoreError;
pub use home::resolve_home;
pub use info::{CoreInfo, core_info};
