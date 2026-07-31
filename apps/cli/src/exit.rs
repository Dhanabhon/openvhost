// SPDX-License-Identifier: GPL-3.0-or-later
//! The one exit-code table (spec `D3`).

use openvhost_proc::control::{Disposition, ErrorCode, Response};
use openvhost_proc::events::ServiceState;

/// Every process exit status this CLI can produce.
///
/// One enum, matched exhaustively everywhere, so the documented table and the
/// behaviour cannot drift. Values follow `sysexits.h` where it has an opinion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// The verb succeeded — including an explicit "already in that state".
    Ok,
    /// The arguments could not be understood, or the request was malformed.
    Usage,
    /// No service is registered under that id.
    UnknownService,
    /// There is no supervisor, or its control channel would not answer.
    Unavailable,
    /// The verb ran and failed, or the peer broke the protocol.
    Failure,
    /// A conflicting operation is in flight, or the transition timed out.
    Busy,
    /// Authorization was denied.
    Unauthorized,
}

impl Exit {
    /// The numeric status handed to the shell.
    pub fn code(self) -> u8 {
        match self {
            Exit::Ok => 0,
            Exit::Usage => 64,
            Exit::UnknownService => 66,
            Exit::Unavailable => 69,
            Exit::Failure => 70,
            Exit::Busy => 75,
            Exit::Unauthorized => 77,
        }
    }
}

/// Map one control answer onto an exit status.
///
/// Exhaustive over [`Response`], [`ErrorCode`], [`Disposition`] and
/// [`ServiceState`] on purpose — no wildcard arm anywhere, so adding a variant
/// to any of them is a compile error here rather than a silent fall-through
/// into the wrong exit code.
pub fn exit_for(response: &Response) -> Exit {
    match response {
        Response::Services { .. } => Exit::Ok,
        Response::Transition {
            service,
            disposition,
        } => match disposition {
            // Spec D3, literally: "0 = success, including an explicit
            // unchanged result". `Unchanged` means the verb found the service
            // already where it was asked to put it and touched nothing, so
            // there is no run that could have failed.
            //
            // The row can still be `Failed`, and exactly one producer does
            // that: `settled(Target::Stopped, ServiceState::Failed) => true`
            // in the desktop handler — `openvhost stop nginx` on an nginx
            // that had already crashed. That is the user's request already
            // satisfied, so it is exit 0, and `render::render_transition`
            // says so on stdout while still showing the stale failure.
            //
            // No other verb can reach here with a `Failed` row:
            // `(Target::Running, Failed) => false` always, so a `start` never
            // takes the shortcut, and `restart` overwrites its answer with
            // `Disposition::Changed` unconditionally.
            Disposition::Unchanged => Exit::Ok,
            // A run that actually happened is judged by where the service
            // *landed*, so a transition that ends on `Failed` is a failure
            // even though the handler chose to report it as a transition
            // rather than an error.
            Disposition::Changed => match &service.state {
                ServiceState::Stopped | ServiceState::Starting | ServiceState::Running => Exit::Ok,
                ServiceState::Failed { .. } => Exit::Failure,
            },
        },
        // Stragglers are exactly what `ErrorCode::OperationFailed` documents
        // as a bulk-stop failure: the verb ran and did not finish the job.
        Response::StopAll { stragglers } => {
            if stragglers.is_empty() {
                Exit::Ok
            } else {
                Exit::Failure
            }
        }
        Response::Error { code, .. } => match code {
            ErrorCode::UnknownService => Exit::UnknownService,
            ErrorCode::SupervisorUnavailable | ErrorCode::ControlChannelUnavailable => {
                Exit::Unavailable
            }
            ErrorCode::OperationFailed => Exit::Failure,
            ErrorCode::Busy | ErrorCode::Timeout => Exit::Busy,
            ErrorCode::Unauthorized => Exit::Unauthorized,
            ErrorCode::BadRequest => Exit::Usage,
            // Not "unsupported platform" but "we do not speak each other's
            // protocol" — the spec's 70 covers protocol failure as well as
            // operation failure.
            ErrorCode::UnsupportedVersion => Exit::Failure,
        },
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use openvhost_proc::events::ServiceStatus;

    fn status(state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: "nginx".into(),
            display_name: "Nginx".into(),
            endpoint: Some("http://127.0.0.1:80".into()),
            pid: Some(4242),
            state,
        }
    }

    /// Every `Response` variant, so a new one cannot be added without turning
    /// up here.
    fn every_response() -> Vec<(Response, Exit)> {
        vec![
            (Response::Services { services: vec![] }, Exit::Ok),
            (
                Response::Transition {
                    service: status(ServiceState::Running),
                    disposition: Disposition::Changed,
                },
                Exit::Ok,
            ),
            (
                Response::Transition {
                    service: status(ServiceState::Running),
                    disposition: Disposition::Unchanged,
                },
                Exit::Ok,
            ),
            (
                Response::Transition {
                    service: status(ServiceState::Starting),
                    disposition: Disposition::Changed,
                },
                Exit::Ok,
            ),
            (
                Response::Transition {
                    service: status(ServiceState::Stopped),
                    disposition: Disposition::Changed,
                },
                Exit::Ok,
            ),
            (
                Response::Transition {
                    service: status(ServiceState::Failed {
                        exit: Some(1),
                        stderr_tail: vec!["boom".into()],
                    }),
                    disposition: Disposition::Changed,
                },
                Exit::Failure,
            ),
            // `stop` on a service that had already crashed — the one shape
            // that pairs a `Failed` row with a successful verb.
            (
                Response::Transition {
                    service: status(ServiceState::Failed {
                        exit: Some(1),
                        stderr_tail: vec!["boom".into()],
                    }),
                    disposition: Disposition::Unchanged,
                },
                Exit::Ok,
            ),
            (Response::StopAll { stragglers: vec![] }, Exit::Ok),
            (
                Response::StopAll {
                    stragglers: vec!["mysql-8.4".into()],
                },
                Exit::Failure,
            ),
            (
                Response::error(ErrorCode::UnknownService, "no such service"),
                Exit::UnknownService,
            ),
            (
                Response::error(ErrorCode::SupervisorUnavailable, "not running"),
                Exit::Unavailable,
            ),
            (
                Response::error(ErrorCode::ControlChannelUnavailable, "not answering"),
                Exit::Unavailable,
            ),
            (
                Response::error(ErrorCode::OperationFailed, "it failed"),
                Exit::Failure,
            ),
            (Response::error(ErrorCode::Busy, "in flight"), Exit::Busy),
            (Response::error(ErrorCode::Timeout, "too slow"), Exit::Busy),
            (
                Response::error(ErrorCode::Unauthorized, "not you"),
                Exit::Unauthorized,
            ),
            (
                Response::error(ErrorCode::BadRequest, "malformed"),
                Exit::Usage,
            ),
            (
                Response::error(ErrorCode::UnsupportedVersion, "newer server"),
                Exit::Failure,
            ),
        ]
    }

    #[test]
    fn every_response_variant_maps_to_its_documented_exit() {
        for (response, want) in every_response() {
            assert_eq!(exit_for(&response), want, "for {response:?}");
        }
    }

    /// The whole point of the table: the numbers are a public contract, so
    /// they are asserted literally rather than via the enum.
    #[test]
    fn the_numeric_codes_are_the_documented_ones() {
        assert_eq!(Exit::Ok.code(), 0);
        assert_eq!(Exit::Usage.code(), 64);
        assert_eq!(Exit::UnknownService.code(), 66);
        assert_eq!(Exit::Unavailable.code(), 69);
        assert_eq!(Exit::Failure.code(), 70);
        assert_eq!(Exit::Busy.code(), 75);
        assert_eq!(Exit::Unauthorized.code(), 77);
    }

    /// `Unchanged` is an explicit success (spec D3) **even when the row is
    /// `Failed`**, because nothing ran that could have failed.
    ///
    /// The producer is `stop` on a service that had already crashed: the
    /// desktop handler's `settled(Target::Stopped, Failed)` is `true`, and it
    /// answers with the crashed row. Judging that by the row rather than by
    /// the disposition made `openvhost stop nginx` exit 70 for doing exactly
    /// what it was asked. A `start` cannot reach here — `settled` is `false`
    /// for every `(Target::Running, _)` but `Running` — and `restart` always
    /// overwrites its disposition with `Changed`.
    ///
    /// The `Changed` half is asserted alongside so the fix cannot be
    /// over-applied into "any transition is a success".
    #[test]
    fn an_unchanged_transition_is_a_success_even_when_the_row_is_failed() {
        let failed = || ServiceState::Failed {
            exit: Some(78),
            stderr_tail: vec![],
        };
        assert_eq!(
            exit_for(&Response::Transition {
                service: status(failed()),
                disposition: Disposition::Unchanged,
            }),
            Exit::Ok
        );
        assert_eq!(
            exit_for(&Response::Transition {
                service: status(failed()),
                disposition: Disposition::Changed,
            }),
            Exit::Failure
        );
    }

    /// `stop-all` that leaves something running has not done its job — the
    /// `OperationFailed` doc names stragglers explicitly.
    #[test]
    fn stop_all_with_stragglers_is_a_failure() {
        assert_eq!(
            exit_for(&Response::StopAll {
                stragglers: vec!["nginx".into()]
            }),
            Exit::Failure
        );
        assert_eq!(
            exit_for(&Response::StopAll { stragglers: vec![] }),
            Exit::Ok
        );
    }
}
