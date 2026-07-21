// SPDX-License-Identifier: GPL-3.0-or-later
//! CoreInfo — the payload of the first typed IPC command (spec §7.1).

use crate::error::CoreError;
use crate::home::resolve_home;

/// Basic environment facts, assembled by core (not by the Tauri command —
/// commands stay thin per master plan §5).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "specta", derive(specta::Type))]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "camelCase")]
pub struct CoreInfo {
    /// Version of the calling application (desktop app or CLI).
    pub app_version: String,
    /// Operating system, from `std::env::consts::OS` ("macos", "windows", …).
    pub os: String,
    /// CPU architecture, from `std::env::consts::ARCH` ("aarch64", "x86_64", …).
    pub arch: String,
    /// Resolved OpenServ home directory, for display.
    pub openserv_home: String,
}

/// Assemble [`CoreInfo`] for the given application version.
pub fn core_info(app_version: &str) -> Result<CoreInfo, CoreError> {
    let home = resolve_home()?;
    Ok(CoreInfo {
        app_version: app_version.to_owned(),
        os: std::env::consts::OS.to_owned(),
        arch: std::env::consts::ARCH.to_owned(),
        openserv_home: home.display().to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn core_info_reports_current_platform() {
        let info = core_info("9.9.9").unwrap();
        assert_eq!(info.app_version, "9.9.9");
        assert_eq!(info.os, std::env::consts::OS);
        assert_eq!(info.arch, std::env::consts::ARCH);
        assert!(!info.openserv_home.is_empty());
    }

    #[test]
    fn core_info_serializes_camel_case() {
        let info = core_info("1.0.0").unwrap();
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("\"appVersion\""));
        assert!(json.contains("\"openservHome\""));
    }
}
