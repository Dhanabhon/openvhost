// SPDX-License-Identifier: GPL-3.0-or-later
//! openvhost-proc — process supervisor for OpenVHost.
//!
//! Responsibility (master plan §3.1): spawn/stop/status for every managed
//! service with the state machine Stopped → Starting → Running → Failed,
//! log capture, and a broadcast event stream. MUST stay tauri-free.
//! v0 scope per spec 2026-07-21-p03-supervisor-design.md.

mod error;
pub mod events;
mod log;
mod orphan;
pub mod platform;
mod service_task;
mod state;
mod supervisor;
pub mod task;
pub mod testchild;

pub use error::ProcError;
pub use events::{LogLevel, LogLine, ServiceState, ServiceStatus, StreamSource, SupervisorEvent};
pub use orphan::{
    BootId, FileRegistry, InstanceLock, OrphanReaper, ProcIdentity, ProcStartTime, ProcessRegistry,
    RegistrySnapshot, SupervisedRecord,
};
pub use platform::{
    OutputStream, ProcessDriver, SpawnSpec, SpawnedChild, default_driver, default_reaper,
};
pub use supervisor::{DEFAULT_GRACE, DEFAULT_READY_AFTER, ReadinessProbe, ServiceSpec, Supervisor};
// `Stream` is re-exported as `TaskStream`: this crate already exports
// `StreamSource` from `events`, and two similarly-named types in one
// namespace is how call sites end up importing the wrong one.
pub use task::{Stream as TaskStream, TaskEvent, run_task};
