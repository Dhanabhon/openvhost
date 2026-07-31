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
    /// advisory, mildly racy answer. The connect result stays authoritative
    /// about *whether contact was made*; the probe only ever answers the
    /// separate question "is an app alive at all", and it is consulted for
    /// exactly two variants:
    ///
    /// - [`ControlError::NotRunning`] — wording only. There is no socket, so
    ///   there is no supervisor whatever the lock says.
    /// - [`ControlError::Unreachable`] — wording **and** the answer. A socket
    ///   that refuses connections while nothing holds the run lock is a force
    ///   quit's leftover, not an ambiguity: the app is definitively down, and
    ///   saying "I could not answer" there is the state-as-error collapse
    ///   spec D3 exists to prevent (it made `status` exit 69 for an indefinite
    ///   window after any force quit). `Present` and `Indeterminate` stay
    ///   [`ErrorCode::ControlChannelUnavailable`], because those genuinely are
    ///   "I could not answer".
    ///
    /// A script still never branches on a race: both probe answers for
    /// `Unreachable` map onto exit 69 for control verbs, and the only verbs
    /// whose exit code moves are `status`/`list`, whose entire job is to
    /// report whether the app is up.
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
            // A socket file that refuses connections. Whether that is an
            // answer or a failure depends entirely on the probe, which is the
            // one place it is allowed to decide more than the wording — see
            // this function's own doc comment.
            ControlError::Unreachable { path, source } => match presence() {
                // Force quit, crash, `pkill`: the socket file outlived the
                // app that bound it. Nothing holds the run lock, so there is
                // definitively no supervisor — which is not "I could not
                // answer", it is the answer. Reported exactly as a missing
                // socket is, so `status`/`list` relax it into an empty list
                // and exit 0 (spec D3) while control verbs still exit 69.
                SupervisorPresence::Absent => (
                    SupervisorReport::NotRunning,
                    ErrorCode::SupervisorUnavailable,
                    format!(
                        "the OpenVHost app is not running, so the control socket at {} is a \
                         leftover from a force quit ({source}). Start the app, then run this \
                         again; relaunching clears the stale socket.",
                        path.display()
                    ),
                ),
                // Something holds the lock but will not talk: genuinely
                // ambiguous, most likely an app still starting up. This is
                // D3's `controlChannelUnavailable`, and it stays a failure
                // for every verb including `status`.
                SupervisorPresence::Present => (
                    SupervisorReport::Unknown,
                    ErrorCode::ControlChannelUnavailable,
                    format!(
                        "the control socket at {} is not accepting connections ({source}). The \
                         OpenVHost app appears to be running; it may still be starting.",
                        path.display()
                    ),
                ),
                // "I could not find out" is not "no" — the reason travels so
                // the third state is worth having.
                SupervisorPresence::Indeterminate { reason } => (
                    SupervisorReport::Unknown,
                    ErrorCode::ControlChannelUnavailable,
                    format!(
                        "the control socket at {} is not accepting connections ({source}), and \
                         whether an app is live could not be checked ({reason}).",
                        path.display()
                    ),
                ),
            },
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
    /// Deliberately narrow: only [`SupervisorReport::NotRunning`] is relaxed,
    /// and only [`from_client_error`](Self::from_client_error) decides who
    /// gets it — a missing socket, or a socket that refuses connections while
    /// the lock probe says nothing is alive (a force quit's leftover). Both
    /// mean "there is no supervisor", which is an answer.
    ///
    /// Everything else stays a failure, including for `status`: a socket that
    /// will not answer while an app *is* alive, or while the probe could not
    /// tell, is "I could not answer", not "the answer is no".
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

    /// EVERY `ControlError` variant, as answered under an **absent** probe.
    /// `ControlError` is deliberately not `#[non_exhaustive]`, so
    /// `from_client_error`'s match is a compile error when a variant is added;
    /// this table is the behavioural half of that guarantee.
    ///
    /// `Unreachable` is the one entry that depends on the probe — see
    /// `an_unreachable_socket_is_a_failure_unless_the_probe_says_nothing_is_alive`.
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
            // Under an absent probe this is a force quit's leftover socket:
            // no supervisor, reported exactly as a missing socket is.
            (
                ControlError::Unreachable {
                    path: sock(),
                    source: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
                },
                SupervisorReport::NotRunning,
                ErrorCode::SupervisorUnavailable,
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

    /// "I could not answer" must NOT be relaxed into "the answer is no".
    ///
    /// The probe says `Absent` throughout, so these are refused on the shape
    /// of the failure alone: something that is not a socket sitting at the
    /// path is a broken home directory, not an absent app, and a peer talking
    /// gibberish answered — badly.
    #[test]
    fn a_channel_that_will_not_answer_is_not_relaxed_into_an_answer() {
        for err in [
            ControlError::NotASocket { path: sock() },
            ControlError::Protocol("garbage".into()),
            ControlError::Io(std::io::Error::from(std::io::ErrorKind::BrokenPipe)),
        ] {
            let ex = Exchange::from_client_error(&err, absent).absent_supervisor_is_an_answer();
            match ex.response {
                Response::Error { .. } => {}
                other => panic!("{err:?} was relaxed into {other:?}"),
            }
        }
    }

    /// A stale socket left by a force quit is **not** an ambiguity: nothing
    /// holds the run lock, so the app is definitively down and `status` must
    /// say so and exit 0 like any other "no app" answer.
    ///
    /// The alternative — 69, `ok:false`, `supervisor:"unknown"` — persisted
    /// for as long as the leftover file did (indefinitely, until the next
    /// launch), which is D3's state-as-error collapse in the exact scenario
    /// the spec's click-list item 7 anticipates.
    ///
    /// It stays a failure when the probe says an app IS alive, or could not
    /// tell: those are genuinely "I could not answer".
    #[test]
    fn an_unreachable_socket_is_a_failure_unless_the_probe_says_nothing_is_alive() {
        let refused = || ControlError::Unreachable {
            path: sock(),
            source: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
        };

        let ex = Exchange::from_client_error(&refused(), absent);
        assert_eq!(ex.supervisor, SupervisorReport::NotRunning);
        let relaxed = ex.absent_supervisor_is_an_answer();
        assert_eq!(relaxed.response, Response::Services { services: vec![] });
        let note = relaxed.note.expect("the force-quit wording must survive");
        assert!(note.contains("not running"), "{note}");
        assert!(note.contains("force quit"), "{note}");

        for presence in [
            SupervisorPresence::Present,
            SupervisorPresence::Indeterminate {
                reason: "cannot stat /home/run: permission denied".into(),
            },
        ] {
            let ex = Exchange::from_client_error(&refused(), || clone_presence(&presence))
                .absent_supervisor_is_an_answer();
            assert_eq!(ex.supervisor, SupervisorReport::Unknown, "{presence:?}");
            match ex.response {
                Response::Error { code, .. } => {
                    assert_eq!(code, ErrorCode::ControlChannelUnavailable, "{presence:?}");
                }
                other => panic!("{presence:?} was relaxed into {other:?}"),
            }
        }
    }

    /// Whatever the probe says, an unreachable socket is exit 69 for a
    /// control verb — there is nothing to start or stop through. Only the
    /// two reporting verbs' exit code moves, and only because reporting
    /// whether the app is up is their entire job.
    #[test]
    fn an_unreachable_socket_never_becomes_an_answer_for_a_control_verb() {
        for presence in [
            SupervisorPresence::Absent,
            SupervisorPresence::Present,
            SupervisorPresence::Indeterminate {
                reason: "unreadable".into(),
            },
        ] {
            // No `absent_supervisor_is_an_answer`: that is exactly what
            // `main::run` withholds from a control verb.
            let ex = Exchange::from_client_error(
                &ControlError::Unreachable {
                    path: sock(),
                    source: std::io::Error::from(std::io::ErrorKind::ConnectionRefused),
                },
                || clone_presence(&presence),
            );
            assert_eq!(
                crate::exit::exit_for(&ex.response).code(),
                69,
                "{presence:?}"
            );
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
