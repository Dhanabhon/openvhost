// SPDX-License-Identifier: GPL-3.0-or-later
//! Pure exit classification (spec §4). The stop-requested flag is recorded
//! BEFORE inspecting the exit status so a timeout-kill during a requested
//! stop lands as Stopped, never Failed.

use std::process::ExitStatus;

use crate::events::ServiceState;

pub(crate) fn classify_exit(
    stop_requested: bool,
    status: Option<&ExitStatus>,
    stderr_tail: Vec<String>,
) -> ServiceState {
    if stop_requested {
        return ServiceState::Stopped;
    }
    match status {
        Some(s) if s.success() => ServiceState::Stopped,
        Some(s) => ServiceState::Failed {
            exit: s.code(),
            stderr_tail,
        },
        None => ServiceState::Failed {
            exit: None,
            stderr_tail,
        },
    }
}

/// Exit classification for a service that has not yet confirmed readiness
/// under a [`crate::ReadinessProbe::Command`] probe (spec D4: either the
/// child exited while a probe attempt was still outstanding, or the probe's
/// own deadline elapsed and the supervisor killed the child itself).
///
/// Unlike [`classify_exit`], a clean (exit-0) death is NOT `Stopped` here:
/// "ready" was never confirmed, so any exit — including one this same
/// supervisor caused by killing an unresponsive child after a deadline — is
/// `Failed`. A stop the USER requested still wins as `Stopped`, exactly as
/// it does for [`classify_exit`]: `stop_requested` is set synchronously by
/// `Supervisor::stop` regardless of which readiness path is in flight, so
/// this is never ambiguous with an internally-triggered teardown.
pub(crate) fn classify_exit_during_probe(
    stop_requested: bool,
    status: Option<&ExitStatus>,
    stderr_tail: Vec<String>,
) -> ServiceState {
    if stop_requested {
        return ServiceState::Stopped;
    }
    ServiceState::Failed {
        exit: status.and_then(|s| s.code()),
        stderr_tail,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::process::Command;

    fn exit_status(code: i32) -> ExitStatus {
        #[cfg(unix)]
        let out = Command::new("/bin/sh")
            .args(["-c", &format!("exit {code}")])
            .status()
            .unwrap();
        #[cfg(windows)]
        let out = Command::new("cmd")
            .args(["/C", &format!("exit {code}")])
            .status()
            .unwrap();
        out
    }

    #[test]
    fn requested_stop_wins_even_after_kill() {
        let st = exit_status(137); // looks like a crash
        let s = classify_exit(true, Some(&st), vec![]);
        assert!(matches!(s, ServiceState::Stopped));
    }

    #[test]
    fn clean_exit_is_stopped() {
        let st = exit_status(0);
        assert!(matches!(
            classify_exit(false, Some(&st), vec![]),
            ServiceState::Stopped
        ));
    }

    #[test]
    fn nonzero_is_failed_with_tail() {
        let st = exit_status(2);
        let s = classify_exit(false, Some(&st), vec!["ERROR boom".into()]);
        match s {
            ServiceState::Failed { exit, stderr_tail } => {
                assert_eq!(exit, Some(2));
                assert_eq!(stderr_tail, vec!["ERROR boom".to_string()]);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // classify_exit_during_probe (spec D4): the one behavior that must
    // DIFFER from classify_exit — a clean exit code is still Failed,
    // because readiness was never confirmed.
    // -----------------------------------------------------------------

    #[test]
    fn requested_stop_wins_during_probe_too() {
        let st = exit_status(137);
        let s = classify_exit_during_probe(true, Some(&st), vec![]);
        assert!(matches!(s, ServiceState::Stopped));
    }

    #[test]
    fn clean_exit_during_probe_is_failed_not_stopped() {
        // THE divergence from `classify_exit`: `clean_exit_is_stopped` above
        // proves a code-0 exit is `Stopped` under the standard rule. Here,
        // the identical exit status must be `Failed` instead — plain
        // `classify_exit` would wrongly call this a clean stop and hide a
        // service that never became ready.
        let st = exit_status(0);
        let s = classify_exit_during_probe(false, Some(&st), vec![]);
        match s {
            ServiceState::Failed { exit, .. } => assert_eq!(exit, Some(0)),
            other => panic!("expected Failed even for a clean exit code, got {other:?}"),
        }
    }

    #[test]
    fn nonzero_during_probe_is_failed_with_tail() {
        let st = exit_status(2);
        let s = classify_exit_during_probe(false, Some(&st), vec!["probe: not ready".into()]);
        match s {
            ServiceState::Failed { exit, stderr_tail } => {
                assert_eq!(exit, Some(2));
                assert_eq!(stderr_tail, vec!["probe: not ready".to_string()]);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

    #[test]
    fn no_status_during_probe_is_failed() {
        // The deadline-elapsed path: the supervisor killed the child itself
        // and the wait may not yield a status at all.
        let s = classify_exit_during_probe(false, None, vec!["probe: timed out".into()]);
        match s {
            ServiceState::Failed { exit, stderr_tail } => {
                assert_eq!(exit, None);
                assert_eq!(stderr_tail, vec!["probe: timed out".to_string()]);
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }
}
