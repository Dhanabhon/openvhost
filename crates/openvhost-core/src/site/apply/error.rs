// SPDX-License-Identifier: GPL-3.0-or-later
//! Errors for the site-apply pipeline.

use std::path::PathBuf;

use openvhost_conf::ConfError;

use crate::CoreError;

#[derive(Debug, thiserror::Error)]
pub enum ApplyError {
    /// An enabled site asks for a PHP major that is not installed. Raised
    /// before any file is touched: a config claiming 8.3 while served by 8.4
    /// is a lie the user debugs the hard way.
    #[error("site {site} needs PHP {requested}, which is not installed (installed: {})", available.join(", "))]
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
    /// A generated-config path is occupied by something that is not a plain
    /// file — a directory, or a symlink pointing who-knows-where.
    ///
    /// Refused rather than followed or ignored. Following it would read a file
    /// outside the generated tree into the diff the user is shown, and ignoring
    /// it would hide the fact that apply cannot write there.
    #[error("{} is not a plain file (found {found}); refusing to read or replace it", path.display())]
    NotAPlainFile { path: PathBuf, found: &'static str },
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
