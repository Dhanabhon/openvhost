// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-core — domain model and state for OpenVHost.
//!
//! Responsibility (master plan §3.1): domain model, SQLite state, event bus.
//! MUST NEVER depend on tauri: consumed by both the desktop app and the
//! openvhost CLI. Current slice: home-directory resolution + CoreInfo.

mod error;
mod home;
mod info;

pub use error::CoreError;
pub use home::resolve_home;
pub use info::{CoreInfo, core_info};

pub mod platform;
