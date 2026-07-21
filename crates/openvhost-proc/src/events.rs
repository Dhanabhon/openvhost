// SPDX-License-Identifier: GPL-3.0-or-later
//! Event and status DTOs — the shapes the UI contract demands
//! (docs/design/README.md). serde camelCase; optional specta derive.

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum ServiceState {
    Stopped,
    Starting,
    Running,
    #[serde(rename_all = "camelCase")]
    Failed {
        exit: Option<i32>,
        stderr_tail: Vec<String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct LogLine {
    pub ts_ms: u64,
    pub level: LogLevel,
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[serde(rename_all = "camelCase")]
pub struct ServiceStatus {
    pub id: String,
    pub display_name: String,
    pub endpoint: Option<String>,
    pub pid: Option<u32>,
    pub state: ServiceState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SupervisorEvent {
    StateChanged {
        id: String,
        state: ServiceState,
        detail: Option<String>,
    },
    Log {
        id: String,
        ts_ms: u64,
        level: LogLevel,
        line: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSource {
    Stdout,
    Stderr,
}
