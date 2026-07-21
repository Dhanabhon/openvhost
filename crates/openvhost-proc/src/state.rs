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
}
