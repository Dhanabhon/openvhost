// SPDX-License-Identifier: GPL-3.0-or-later
//! The one managed handle to `state.db` (optional-state.db design, D1).
//!
//! `Db::open` is best-effort at startup — a missing or unreadable store must
//! never stop the supervisor — so for as long as commands took
//! `State<'_, Db>`, the degraded machine got Tauri's own refusal instead of an
//! answer: *"state not managed for field `db` on command `php_environment`.
//! You must call `.manage()` before using this command."* That sentence
//! reached a **user**, in a page that had lost all its rows and controls.
//!
//! [`DbHandle`] is what replaces it. **Both** arms of the open manage one, so
//! extraction always succeeds and each command answers for itself — a typed,
//! renderable refusal ([`DbHandle::require`]) or a degraded but real result
//! ([`DbHandle::optional`]).
//!
//! Two properties are load-bearing rather than incidental:
//!
//! 1. **There is no `inner()`-shaped escape hatch.** Neither accessor hands
//!    out a `&Db` without the caller acknowledging absence, so the worst a new
//!    command can do is refuse.
//! 2. **`Db` itself is never managed again.** That turns "a new
//!    `State<'_, Db>` parameter" from a bug that only fires on a machine whose
//!    store is broken into one that fires on *every* machine, including the
//!    developer's, on the first invocation — it cannot reach a user (D6).

use openvhost_core::Db;

use crate::commands::IpcError;

/// The sentence every refusal and the startup log line open with.
///
/// Shared so the developer reading a terminal and the user reading a banner
/// are told the same thing about the same condition.
pub const STORE_UNAVAILABLE: &str = "OpenVHost's data store (state.db) is unavailable this run";

/// `STORE_UNAVAILABLE`, with the reason the open actually failed for.
///
/// **Carrying the reason is the point** — startup already has the `CoreError`
/// and used to only `eprintln!` it, so a refusal could say no more than
/// "unavailable". It can now say *permission denied*.
pub fn unavailable_message(reason: &str) -> String {
    format!("{STORE_UNAVAILABLE}: {reason}")
}

/// The managed store: open, or absent with the reason it is absent.
///
/// Managed **unconditionally and exactly once** — `Manager::manage` does not
/// overwrite an existing value (its own doc example asserts
/// `assert!(!app.manage(MyInt(1)))`), so a "manage `Unavailable` early, the
/// real one later" split would silently pin every user to `Unavailable`.
/// `Manager::unmanage` exists; it is deliberately not used to fake a
/// re-manage.
pub enum DbHandle {
    /// `Db::open` succeeded at startup.
    Ready(Db),
    /// It did not. `reason` is that failure's own `Display`.
    Unavailable { reason: String },
}

impl DbHandle {
    /// REFUSE: the store, or a typed error naming why there isn't one.
    ///
    /// `IpcError::Core` and **no new variant**: the error genuinely came from
    /// openvhost-core, nothing branches on `kind`, and every affected page
    /// already renders `.message` — a variant earns nothing until some UI
    /// switches on it.
    pub fn require(&self) -> Result<&Db, IpcError> {
        match self {
            DbHandle::Ready(db) => Ok(db),
            DbHandle::Unavailable { reason } => Err(IpcError::Core {
                message: unavailable_message(reason),
            }),
        }
    }

    /// DEGRADE: the store if there is one, and the caller handles `None`.
    ///
    /// For the commands whose real work does not need state.db — only their
    /// bookkeeping does. Returning `Option` rather than a `&Db` is what forces
    /// that handling to be written down.
    pub fn optional(&self) -> Option<&Db> {
        match self {
            DbHandle::Ready(db) => Some(db),
            DbHandle::Unavailable { .. } => None,
        }
    }

    /// Why the store is unavailable, for a DEGRADE path that wants to say so.
    ///
    /// `None` means it is available — this is not an error channel.
    ///
    /// No caller in the tree yet: D5's zero-arg `state_store_status` command
    /// (the app-level banner) is the one that needs a reason rather than a
    /// bare `None`, and it lands with the banner. It ships here because D1
    /// specifies the three accessors as one API, and because splitting the
    /// type across two commits would be churn, not safety.
    #[allow(dead_code)]
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            DbHandle::Ready(_) => None,
            DbHandle::Unavailable { reason } => Some(reason),
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// The whole point of the slice, pinned at the source: a refusal names the
    /// reason the store is missing, and says nothing about `.manage()`.
    #[test]
    fn require_refuses_with_the_reason_and_never_mentions_manage() {
        let handle = DbHandle::Unavailable {
            reason: "Permission denied (os error 13)".into(),
        };

        // `Db` has no `Debug`, so the `Ok` arm is named rather than unwrapped.
        let Err(err) = handle.require() else {
            panic!("an unavailable store must refuse");
        };
        let IpcError::Core { message } = &err else {
            panic!("expected IpcError::Core, got {err:?}");
        };
        assert!(
            message.contains("Permission denied (os error 13)"),
            "the refusal must carry the reason: {message:?}"
        );
        assert!(
            message.contains("state.db"),
            "the refusal must name what is unavailable: {message:?}"
        );
        assert!(
            !message.contains(".manage()"),
            "the user must never be told to call a Rust API: {message:?}"
        );
    }

    #[tokio::test]
    async fn a_ready_handle_hands_out_the_store_through_both_accessors() {
        let handle = DbHandle::Ready(Db::open_in_memory().await.expect("in-memory db"));

        assert!(handle.require().is_ok());
        assert!(handle.optional().is_some());
        assert_eq!(
            handle.unavailable_reason(),
            None,
            "a Ready handle has no reason to report"
        );
    }

    #[test]
    fn an_unavailable_handle_degrades_to_none_and_reports_why() {
        let handle = DbHandle::Unavailable {
            reason: "disk I/O error".into(),
        };

        assert!(handle.optional().is_none());
        assert_eq!(handle.unavailable_reason(), Some("disk I/O error"));
    }
}
