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
        // A transition is judged by where the service *landed*, not by whether
        // it moved: `Unchanged` is an explicit success (spec D3), but a run
        // that ends on `Failed` is a failure even if the handler chose to
        // report it as a transition rather than an error.
        Response::Transition {
            service,
            disposition,
        } => match disposition {
            Disposition::Changed | Disposition::Unchanged => match &service.state {
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

    /// A transition that lands on `Failed` is not a success, whatever the
    /// disposition says. Without this a handler that reported a failed start
    /// as a `Transition` rather than an `Error` would exit 0.
    #[test]
    fn a_transition_onto_failed_is_a_failure_even_when_unchanged() {
        let r = Response::Transition {
            service: status(ServiceState::Failed {
                exit: Some(78),
                stderr_tail: vec![],
            }),
            disposition: Disposition::Unchanged,
        };
        assert_eq!(exit_for(&r), Exit::Failure);
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
