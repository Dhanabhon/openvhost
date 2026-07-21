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

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn failed_state_serializes_with_kind_and_camel_case_tail() {
        let s = ServiceState::Failed {
            exit: Some(1),
            stderr_tail: vec!["ERROR boom".into()],
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["kind"], "failed");
        assert_eq!(v["exit"], 1);
        assert_eq!(v["stderrTail"][0], "ERROR boom");
    }

    #[test]
    fn running_state_serializes_kind_only() {
        let v = serde_json::to_value(ServiceState::Running).unwrap();
        assert_eq!(v, serde_json::json!({ "kind": "running" }));
    }

    #[test]
    fn log_line_and_status_use_camel_case_fields() {
        let line = LogLine {
            ts_ms: 7,
            level: LogLevel::Warn,
            line: "w".into(),
        };
        let v = serde_json::to_value(&line).unwrap();
        assert_eq!(v["tsMs"], 7);
        assert_eq!(v["level"], "warn");
        let status = ServiceStatus {
            id: "a".into(),
            display_name: "A".into(),
            endpoint: None,
            pid: None,
            state: ServiceState::Stopped,
        };
        let v = serde_json::to_value(&status).unwrap();
        assert_eq!(v["displayName"], "A");
        assert_eq!(v["state"]["kind"], "stopped");
    }
}
