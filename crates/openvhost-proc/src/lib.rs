// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-proc — process supervisor for OpenVHost.
//!
//! Responsibility (master plan §3.1): spawn/stop/status for every managed
//! service with the state machine Stopped → Starting → Running → Failed,
//! log capture, and a broadcast event stream. MUST stay tauri-free.
//! v0 scope per spec 2026-07-21-p03-supervisor-design.md.

mod error;
pub mod platform;

pub use error::ProcError;
pub use platform::{OutputStream, ProcessDriver, SpawnSpec, SpawnedChild, default_driver};
