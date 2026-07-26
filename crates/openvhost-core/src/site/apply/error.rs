// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors for the site-apply pipeline.

use std::path::PathBuf;

use openvhost_conf::ConfError;

use crate::CoreError;

/// Two different true statements depending on whether ANY PHP runtime was
/// detected at all.
///
/// `available` is empty whenever the startup probe failed or timed out — in
/// which case the version-specific "which is not installed (installed: )"
/// phrasing is actively misleading: it tells the user their installed PHP is
/// not installed, with nothing after "installed:" to back that up. The empty
/// case also carries a different, actionable next step, and notes WHY it is
/// a restart and not a retry: the runtime probe runs once, at startup, so
/// installing PHP while OpenVHost is already open does not take effect
/// until the app restarts.
fn missing_runtime_message(site: &str, requested: &str, available: &[String]) -> String {
    if available.is_empty() {
        format!(
            "site {site} needs PHP {requested}, but no PHP runtime was detected — install PHP \
             and restart OpenVHost (the runtime probe only runs once, at startup, so an install \
             made while the app is open will not be picked up until then)"
        )
    } else {
        format!(
            "site {site} needs PHP {requested}, which is not installed (installed: {})",
            available.join(", ")
        )
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// An enabled site asks for a PHP major that is not installed. Raised
    /// before any file is touched: a config claiming 8.3 while served by 8.4
    /// is a lie the user debugs the hard way.
    #[error("{}", missing_runtime_message(site, requested, available))]
    MissingRuntime {
        site: String,
        requested: String,
        available: Vec<String>,
    },
    /// Generation or validator launch failed.
    #[error("config: {0}")]
    Conf(#[from] ConfError),
    #[error("io error {op} {}: {source}", path.display())]
    Io {
        op: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A path apply needed is occupied by something of the wrong kind: a
    /// generated-config path that is not a plain file (a directory, or a
    /// symlink pointing who-knows-where), or a scan root (`config/generated/
    /// nginx/sites`, `config/generated/php`) that is not a real directory (a
    /// symlink, or a plain file).
    ///
    /// Refused rather than followed or ignored. For a config path, following
    /// it would read a file outside the generated tree into the diff the
    /// user is shown, and ignoring it would hide the fact that apply cannot
    /// write there. For a scan root, following a symlinked directory would
    /// let `read_dir` walk into whatever it points at, classify every
    /// `.conf` file it finds `Removed`, and delete it — a confinement break
    /// (A4) that per-entry `DirEntry::file_type()` checks cannot catch,
    /// because they only ever see entries inside whatever `read_dir` already
    /// opened.
    #[error("{} is not {expected} (found {found}); refusing to touch it", path.display())]
    NotAPlainFile {
        path: PathBuf,
        /// What apply needed there — `"a plain file"` for a generated-config
        /// path, `"a plain directory"` for a scan root.
        expected: &'static str,
        found: &'static str,
    },
    /// `nginx -t` rejected the generated set. The tree has been rolled back.
    #[error("the generated config was rejected by the web server:\n{stderr}")]
    ValidationFailed { stderr: String },
    /// Both the apply and its rollback failed. The generated tree now matches
    /// NEITHER the old nor the new configuration; `stranded` names the files
    /// that could not be restored. Never collapse this into a generic
    /// failure — it is the only signal the user gets that the tree is mixed.
    #[error("apply failed ({original}) AND rollback failed ({rollback}); these files were not restored: {}",
        stranded.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", "))]
    RollbackFailed {
        original: Box<ApplyError>,
        rollback: Box<ApplyError>,
        stranded: Vec<PathBuf>,
    },
    #[error(transparent)]
    Core(#[from] CoreError),
}

/// What a rollback managed to do. Rollback continues past a failure — restoring
/// four of five files beats abandoning at the first error — so it reports the
/// first error together with every path it could not restore.
#[derive(Debug)]
pub struct RollbackReport {
    pub first_error: ApplyError,
    pub stranded: Vec<PathBuf>,
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Reachable whenever the startup PHP probe fails or times out: `available`
    /// is empty, and the plain "which is not installed (installed: {joined})"
    /// phrasing would render as "...(installed: )" — telling the user their
    /// installed PHP is not installed, with nothing after the colon. This
    /// pins the honest replacement instead: no runtime detected at all, plus
    /// the actionable next step and the restart caveat.
    #[test]
    fn missing_runtime_with_no_detected_php_says_so_instead_of_a_trailing_colon() {
        let err = ApplyError::MissingRuntime {
            site: "blog".to_string(),
            requested: "8.4".to_string(),
            available: vec![],
        };
        let msg = err.to_string();
        assert!(
            !msg.contains("installed: )"),
            "must not render the empty-list artifact \"installed: )\": {msg}"
        );
        assert!(msg.contains("no PHP runtime was detected"), "got {msg:?}");
        assert!(msg.contains("blog"), "must still name the site: {msg:?}");
        assert!(
            msg.contains("8.4"),
            "must still name the requested version: {msg:?}"
        );
        assert!(
            msg.contains("restart"),
            "must say the probe runs once at startup, so installing PHP now needs a restart: {msg:?}"
        );
    }

    /// The non-empty case is unchanged: still names what IS installed.
    #[test]
    fn missing_runtime_with_detected_php_lists_what_is_installed() {
        let err = ApplyError::MissingRuntime {
            site: "blog".to_string(),
            requested: "7.4".to_string(),
            available: vec!["8.3".to_string(), "8.4".to_string()],
        };
        let msg = err.to_string();
        assert!(msg.contains("installed: 8.3, 8.4"), "got {msg:?}");
    }
}
