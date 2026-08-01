// SPDX-License-Identifier: GPL-3.0-or-later
//! Crate error type (thiserror in lib crates — master plan §5).

/// Errors produced by openvhost-proc.
#[derive(Debug, thiserror::Error)]
pub enum ProcError {
    /// No service registered under this id.
    #[error("unknown service '{0}'")]
    NotFound(String),
    /// [`crate::Supervisor::unregister`] was asked to forget a service that is
    /// not in a terminal state (`Stopped`/`Failed`).
    ///
    /// Load-bearing rather than merely polite (package-uninstall design D4):
    /// this crate's orphan registry is keyed by the services it is supervising,
    /// and the next launch's reaper is identity-matched against exactly those
    /// records — so forgetting a service whose child is still alive would drop
    /// our only record of that child.
    ///
    /// `state` is a `&'static str` on purpose: the names come from
    /// [`crate::ServiceState`]'s closed set (see `supervisor::check_terminal`,
    /// the single exhaustive match that produces them), never from a caller.
    #[error("service '{id}' is {state}; it must be stopped before it can be removed")]
    NotTerminal {
        /// The service that was asked to be forgotten.
        id: String,
        /// Its state at the moment of the refusal — `"starting"` or
        /// `"running"`.
        state: &'static str,
    },
    /// Underlying process/system operation failed.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
