// SPDX-License-Identifier: GPL-3.0-or-later
//! One request/answer exchange, however it turned out.
//!
//! Client-side failures are folded into the same [`Response`] the server would
//! have sent, so there is exactly one thing to render and exactly one exit
//! table to consult (spec D3/D5).

use openvhost_proc::SupervisorPresence;
use openvhost_proc::control::{ControlError, ErrorCode, Response};

/// What the CLI can honestly say about the supervisor after one exchange.
///
/// Three values, not a bool: "I could not tell" is a genuine third answer, and
/// collapsing it into "not running" is the state-as-boolean mistake the design
/// rejects (spec D3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorReport {
    /// The control channel answered, so a supervisor exists.
    Running,
    /// There is no control socket at all: no supervisor.
    NotRunning,
    /// Contact could not be established or evaluated.
    Unknown,
}

impl SupervisorReport {
    /// The wire spelling used in the `supervisor` envelope field.
    pub fn as_str(self) -> &'static str {
        match self {
            SupervisorReport::Running => "running",
            SupervisorReport::NotRunning => "notRunning",
            SupervisorReport::Unknown => "unknown",
        }
    }

    /// The human spelling.
    pub fn describe(self) -> &'static str {
        match self {
            SupervisorReport::Running => "running",
            SupervisorReport::NotRunning => "not running",
            SupervisorReport::Unknown => "unknown",
        }
    }
}

/// The outcome of one exchange: what happened, and what it says about the app.
#[derive(Debug, Clone, PartialEq)]
pub struct Exchange {
    /// What can be said about the supervisor.
    pub supervisor: SupervisorReport,
    /// The answer to render and to derive an exit code from.
    pub response: Response,
    /// Human-only prose that belongs with a *successful* answer, so it has
    /// nowhere to live inside [`Response`].
    ///
    /// Set by [`Exchange::absent_supervisor_is_an_answer`], which turns "there
    /// is no app" from an error into an empty service list and would otherwise
    /// throw away the wording the lock probe just improved. Never serialized:
    /// `supervisor` is the machine-readable signal, and the JSON contract says
    /// not to rely on human messages (spec D5).
    pub note: Option<String>,
}

impl Exchange {
    /// The server answered.
    pub fn answered(response: Response) -> Self {
        Exchange {
            supervisor: SupervisorReport::Running,
            response,
            note: None,
        }
    }

    /// A failure raised before anything was ever sent — a usage error, or a
    /// home directory that could not be resolved.
    ///
    /// `supervisor` is [`SupervisorReport::Unknown`] because nothing was
    /// asked: claiming either "running" or "not running" here would be a
    /// guess dressed up as an observation.
    pub fn refused(code: ErrorCode, message: impl Into<String>) -> Self {
        Exchange {
            supervisor: SupervisorReport::Unknown,
            response: Response::error(code, message),
            note: None,
        }
    }

    /// Fold a client-side failure into an answer.
    ///
    /// Exhaustive over [`ControlError`], which is deliberately **not**
    /// `#[non_exhaustive]`: a variant added upstream must fail to compile here
    /// rather than fall through a wildcard into some plausible-looking exit
    /// code.
    ///
    /// `presence` is [`InstanceLock::probe`](openvhost_proc::InstanceLock::probe)'s
    /// advisory, mildly racy answer, and it may influence **the wording and
    /// nothing else** (spec D3). The connect result is authoritative: neither
    /// the [`ErrorCode`] nor the [`SupervisorReport`] is derived from it, so a
    /// script can never end up branching on a race.
    ///
    /// Taken as a closure so it is only evaluated by the two variants that
    /// use it: probing takes the run lock and may create `<home>/run/lock`, and
    /// a mistyped service id is no reason to touch a user's disk at all.
    pub fn from_client_error(
        err: &ControlError,
        presence: impl FnOnce() -> SupervisorPresence,
    ) -> Self {
        let (supervisor, code, message) = match err {
            ControlError::SocketPathTooLong { path, len, max } => (
                SupervisorReport::Unknown,
                ErrorCode::BadRequest,
                format!(
                    "the control socket path is {len} bytes, over the {max}-byte limit: {}. \
                     Set OPENVHOST_HOME to a shorter path.",
                    path.display()
                ),
            ),
            // No socket at all. The one case `status`/`list` turn back into a
            // successful answer — see `absent_supervisor_is_an_answer`.
            ControlError::NotRunning { .. } => (
                SupervisorReport::NotRunning,
                ErrorCode::SupervisorUnavailable,
                match presence() {
                    SupervisorPresence::Absent => "the OpenVHost app is not running. \
                         Start it, then run this again."
                        .to_owned(),
                    SupervisorPresence::Present => "the OpenVHost app appears to be running but \
                         has not opened its control socket yet; it may still be starting. \
                         Try again in a moment."
                        .to_owned(),
                    SupervisorPresence::Indeterminate { reason } => format!(
                        "the OpenVHost app is not running, and whether one is live could not be \
                         checked ({reason}). Start the app, then run this again."
                    ),
                },
            ),
            ControlError::NotASocket { path } => (
                SupervisorReport::Unknown,
                ErrorCode::ControlChannelUnavailable,
                format!(
                    "{} exists but is not a socket, so it will not be used or removed. \
                     Move it aside and relaunch the OpenVHost app.",
                    path.display()
                ),
            ),
            ControlError::Unreachable { path, source } => (
                SupervisorReport::Unknown,
                ErrorCode::ControlChannelUnavailable,
                format!(
                    "the control socket at {} is not accepting connections ({source}). {}",
                    path.display(),
                    match presence() {
                        SupervisorPresence::Present =>
                            "The OpenVHost app appears to be running; it may still be starting.",
                        SupervisorPresence::Absent =>
                            "No app is running, so this looks like a leftover from a force quit; \
                             relaunching the OpenVHost app clears it.",
                        SupervisorPresence::Indeterminate { .. } =>
                            "Whether an app is live could not be checked.",
                    }
                ),
            ),
            ControlError::Io(e) => (
                SupervisorReport::Unknown,
                ErrorCode::OperationFailed,
                format!("the control channel failed mid-exchange: {e}"),
            ),
            ControlError::Protocol(detail) => (
                SupervisorReport::Unknown,
                ErrorCode::OperationFailed,
                format!(
                    "the app spoke something this build does not understand: {detail}. \
                     The app and this command line tool may be different versions."
                ),
            ),
            ControlError::InvalidServiceId(detail) => (
                SupervisorReport::Unknown,
                ErrorCode::BadRequest,
                format!("{detail}. Run `openvhost list` to see the registered service ids."),
            ),
            ControlError::UnsupportedPlatform => (
                SupervisorReport::Unknown,
                ErrorCode::ControlChannelUnavailable,
                "the control channel is not implemented on this platform yet (OpenVHost v1 is \
                 macOS-first)"
                    .to_owned(),
            ),
        };
        Exchange {
            supervisor,
            response: Response::error(code, message),
            note: None,
        }
    }

    /// For `status` and `list` only: a definitively absent supervisor is *the
    /// answer* — an empty service list and exit 0 — not a failure (spec D3).
    ///
    /// Deliberately narrow. Only [`SupervisorReport::NotRunning`], which only
    /// [`ControlError::NotRunning`] produces, is relaxed. A socket that exists
    /// but will not answer is "I could not answer", not "the answer is no",
    /// and stays a failure even for `status`.
    pub fn absent_supervisor_is_an_answer(self) -> Self {
        match self.supervisor {
            SupervisorReport::NotRunning => Exchange {
                supervisor: SupervisorReport::NotRunning,
                // The error message becomes a human note rather than being
                // dropped: it is the one place the lock probe's improved
                // wording ("…may still be starting") is visible.
                note: match &self.response {
                    Response::Error { message, .. } => Some(message.clone()),
                    Response::Services { .. }
                    | Response::Transition { .. }
                    | Response::StopAll { .. } => self.note,
                },
                response: Response::Services { services: vec![] },
            },
            SupervisorReport::Running | SupervisorReport::Unknown => self,
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn absent() -> SupervisorPresence {
        SupervisorPresence::Absent
    }

    /// `SupervisorPresence` is not `Clone`; the loop below needs each value
    /// twice (once to probe with, once to name the case in a failure).
    fn clone_presence(p: &SupervisorPresence) -> SupervisorPresence {
        match p {
            SupervisorPresence::Present => SupervisorPresence::Present,
            SupervisorPresence::Absent => SupervisorPresence::Absent,
            SupervisorPresence::Indeterminate { reason } => SupervisorPresence::Indeterminate {
                reason: reason.clone(),
            },
        }
    }

    fn sock() -> PathBuf {
        PathBuf::from("/home/run/control.sock")
    }

    /// EVERY `ControlError` variant. `ControlError` is deliberately not
    /// `#[non_exhaustive]`, so `from_client_error`'s match is a compile error
    /// when a variant is added; this table is the behavioural half of that
    /// guarantee.
    fn every_client_error() -> Vec<(ControlError, SupervisorReport, ErrorCode)> {
        vec![
            (
                ControlError::SocketPathTooLong {
                    path: sock(),
                    len: 200,
                    max: 103,
                },
                SupervisorReport::Unknown,
                ErrorCode::BadRequest,
            ),
            (
                ControlError::NotRunning { path: sock() },
                SupervisorReport::NotRunning,
                ErrorCode::SupervisorUnavailable,
            ),
            (
                ControlError::NotASocket { path: sock() },
                SupervisorReport::Unknown,
                ErrorCode::ControlChannelUnavailable,
            ),
            (
                ControlError::Unreachable {
                    path: sock(),
                    source: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
                },
                SupervisorReport::Unknown,
                ErrorCode::ControlChannelUnavailable,
            ),
            (
                ControlError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
                SupervisorReport::Unknown,
                ErrorCode::OperationFailed,
            ),
            (
                ControlError::Protocol("not our JSON".into()),
                SupervisorReport::Unknown,
                ErrorCode::OperationFailed,
            ),
            (
                ControlError::InvalidServiceId("bad id".into()),
                SupervisorReport::Unknown,
                ErrorCode::BadRequest,
            ),
            (
                ControlError::UnsupportedPlatform,
                SupervisorReport::Unknown,
                ErrorCode::ControlChannelUnavailable,
            ),
        ]
    }

    #[test]
    fn every_client_error_maps_to_its_documented_report_and_code() {
        for (err, want_report, want_code) in every_client_error() {
            let ex = Exchange::from_client_error(&err, absent);
            assert_eq!(ex.supervisor, want_report, "report for {err:?}");
            match ex.response {
                Response::Error { code, ref message } => {
                    assert_eq!(code, want_code, "code for {err:?}");
                    assert!(!message.is_empty(), "empty message for {err:?}");
                }
                other => panic!("{err:?} produced {other:?}"),
            }
        }
    }

    /// The probe is advisory and mildly racy, so it may change the *wording*
    /// and nothing else. If it ever moved the code, a script would start
    /// branching on a race.
    #[test]
    fn the_probe_changes_the_message_but_never_the_code_or_the_report() {
        let err = ControlError::NotRunning { path: sock() };
        let mut messages = Vec::new();
        for presence in [
            SupervisorPresence::Absent,
            SupervisorPresence::Present,
            SupervisorPresence::Indeterminate {
                reason: "permission denied".into(),
            },
        ] {
            let ex = Exchange::from_client_error(&err, || clone_presence(&presence));
            assert_eq!(ex.supervisor, SupervisorReport::NotRunning, "{presence:?}");
            match ex.response {
                Response::Error { code, message } => {
                    assert_eq!(code, ErrorCode::SupervisorUnavailable, "{presence:?}");
                    messages.push(message);
                }
                other => panic!("{presence:?} produced {other:?}"),
            }
        }
        messages.sort();
        messages.dedup();
        assert_eq!(messages.len(), 3, "each presence deserves its own wording");
    }

    /// `Indeterminate`'s reason has to survive into the message, or the third
    /// state is decorative.
    #[test]
    fn an_indeterminate_probe_carries_its_reason_into_the_message() {
        let ex = Exchange::from_client_error(&ControlError::NotRunning { path: sock() }, || {
            SupervisorPresence::Indeterminate {
                reason: "cannot stat /home/run: permission denied".into(),
            }
        });
        match ex.response {
            Response::Error { message, .. } => {
                assert!(message.contains("permission denied"), "{message}")
            }
            other => panic!("{other:?}"),
        }
    }

    /// THE rule of the slice: for `status`/`list`, "the app is not running" is
    /// an answer with an empty service list, not an error.
    #[test]
    fn an_absent_supervisor_becomes_an_empty_service_list() {
        let ex = Exchange::from_client_error(&ControlError::NotRunning { path: sock() }, absent)
            .absent_supervisor_is_an_answer();
        assert_eq!(ex.supervisor, SupervisorReport::NotRunning);
        assert_eq!(ex.response, Response::Services { services: vec![] });
    }

    /// Turning the error into an answer must not throw away the wording the
    /// probe just improved — otherwise the third `SupervisorPresence` state
    /// buys nothing on the one path that most needs it.
    #[test]
    fn the_relaxation_keeps_the_probes_wording_as_a_human_note() {
        let ex = Exchange::from_client_error(&ControlError::NotRunning { path: sock() }, || {
            SupervisorPresence::Present
        })
        .absent_supervisor_is_an_answer();
        let note = ex.note.expect("the wording must survive as a note");
        assert!(note.contains("may still be starting"), "{note}");
    }

    /// …and it stays an answer even when the lock probe disagrees, because the
    /// connect result is authoritative.
    #[test]
    fn an_absent_supervisor_is_still_an_answer_when_the_probe_says_present() {
        let ex = Exchange::from_client_error(&ControlError::NotRunning { path: sock() }, || {
            SupervisorPresence::Present
        })
        .absent_supervisor_is_an_answer();
        assert_eq!(ex.response, Response::Services { services: vec![] });
    }

    /// "I could not answer" must NOT be relaxed into "the answer is no" — a
    /// stale socket after a force quit is a failure, including for `status`.
    #[test]
    fn a_channel_that_will_not_answer_is_not_relaxed_into_an_answer() {
        for err in [
            ControlError::NotASocket { path: sock() },
            ControlError::Unreachable {
                path: sock(),
                source: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
            },
            ControlError::Protocol("garbage".into()),
        ] {
            let ex = Exchange::from_client_error(&err, absent).absent_supervisor_is_an_answer();
            match ex.response {
                Response::Error { .. } => {}
                other => panic!("{err:?} was relaxed into {other:?}"),
            }
        }
    }

    /// A real answer is never rewritten by the relaxation.
    #[test]
    fn an_answered_exchange_passes_through_the_relaxation_untouched() {
        let ex = Exchange::answered(Response::StopAll { stragglers: vec![] });
        assert_eq!(ex.clone().absent_supervisor_is_an_answer(), ex);
        assert_eq!(ex.supervisor, SupervisorReport::Running);
    }

    #[test]
    fn the_wire_and_human_spellings_are_the_documented_ones() {
        assert_eq!(SupervisorReport::Running.as_str(), "running");
        assert_eq!(SupervisorReport::NotRunning.as_str(), "notRunning");
        assert_eq!(SupervisorReport::Unknown.as_str(), "unknown");
        assert_eq!(SupervisorReport::NotRunning.describe(), "not running");
    }
}
