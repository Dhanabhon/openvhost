// SPDX-License-Identifier: GPL-3.0-or-later
//! OPENVHOST_HOME resolution (master plan §3.2; spec §7.1).

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use crate::error::CoreError;

/// Resolve the OpenVHost home directory: `OPENVHOST_HOME` env override wins,
/// otherwise `<user home>/.openvhost`. The override is what makes tests and
/// the future integration harness hermetic.
pub fn resolve_home() -> Result<PathBuf, CoreError> {
    resolve_home_from(
        std::env::var_os("OPENVHOST_HOME").as_deref(),
        dirs::home_dir().as_deref(),
    )
}

/// Pure core of [`resolve_home`], testable without touching process env.
pub(crate) fn resolve_home_from(
    override_val: Option<&OsStr>,
    home_dir: Option<&Path>,
) -> Result<PathBuf, CoreError> {
    if let Some(v) = override_val.filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(v));
    }
    home_dir
        .map(|h| h.join(".openvhost"))
        .ok_or(CoreError::HomeDirUnavailable)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins() {
        let p = resolve_home_from(
            Some(OsStr::new("/custom/openvhost-home")),
            Some(Path::new("/Users/x")),
        )
        .unwrap();
        assert_eq!(p, PathBuf::from("/custom/openvhost-home"));
    }

    #[test]
    fn defaults_to_dot_openvhost_under_home() {
        let p = resolve_home_from(None, Some(Path::new("/Users/x"))).unwrap();
        // Build expected via join so the separator is right on Windows too.
        assert_eq!(p, Path::new("/Users/x").join(".openvhost"));
    }

    #[test]
    fn empty_override_falls_back_to_default() {
        let p = resolve_home_from(Some(OsStr::new("")), Some(Path::new("/Users/x"))).unwrap();
        assert_eq!(p, Path::new("/Users/x").join(".openvhost"));
    }

    #[test]
    fn no_home_and_no_override_errors() {
        assert!(matches!(
            resolve_home_from(None, None),
            Err(CoreError::HomeDirUnavailable)
        ));
    }
}
