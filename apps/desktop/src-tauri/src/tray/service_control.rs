// SPDX-License-Identifier: GPL-3.0-or-later
//! Bulk start/stop primitives (P1 tray design
//! `docs/superpowers/specs/2026-07-31-p1-tray-design.md`, spec D6/D7) and the
//! tray-initiated-failure dialog's pure decision logic (spec D4) — split out
//! from `mod.rs` so the closure-injected, testable pieces (which ids a bulk
//! action starts, the reject-vs-queue lock probe, the dialog's own text,
//! which transitions owe one) do not have to live alongside the real
//! `muda`/`tray-icon` construction and native dialog call that spec D9 rules
//! out testing at all.
//!
//! Vacuity status: every function and test below is new in this commit.
//! Each test's own doc comment records its neuter-and-watch-it-fail proof
//! where one was performed; the rest are new-code RED-then-GREEN (they
//! failed to compile against `todo!()`-bodied stubs before the real
//! implementation landed, since there was no prior implementation to
//! regress against).

use openvhost_proc::{ServiceState, ServiceStatus};

use super::TrayInitiated;
use super::model::bulk_start_ids;

/// Attempt to acquire BOTH `bulk` and `apply`'s locks via `try_lock` (spec
/// D7 — "reject, never queue", never a blocking `.lock().await`: bulk
/// actions are long-running — `stop_all` alone can take up to 18s covering
/// MySQL's own 15s grace — so queuing behind one would flap the stack with
/// no user intent behind it). Returns both guards on success; returns
/// `None` — having taken NEITHER lock — the moment either is already held.
///
/// `apply` is the EXISTING `crate::commands::ApplyLock`'s own mutex, not a
/// second lock invented for this slice: `apply_config` stops and restarts
/// the same nginx/php-fpm services a bulk action does, so a bulk action
/// racing an in-flight Apply is exactly the interleaving this guards
/// against — not merely two bulk actions racing each other, which `bulk`
/// alone already prevents.
/// `pub(crate)`, widened from `pub(super)` by the P1 CLI slice: the control
/// channel's `stop-all` (`control.rs`) is the third caller of this admission
/// check, alongside [`super::dispatch_start_all`] and
/// [`super::dispatch_stop_all`]. It calls THIS function rather than repeating
/// the two `try_lock`s, for the same reason `dispatch_stop_all` calls
/// `quit::stop_all` rather than reimplementing it.
pub(crate) fn try_acquire_bulk<'a>(
    bulk: &'a tokio::sync::Mutex<()>,
    apply: &'a tokio::sync::Mutex<()>,
) -> Option<(
    tokio::sync::MutexGuard<'a, ()>,
    tokio::sync::MutexGuard<'a, ()>,
)> {
    let bulk_guard = bulk.try_lock().ok()?;
    let apply_guard = apply.try_lock().ok()?;
    Some((bulk_guard, apply_guard))
}

/// The ids [`bulk_start_ids`] selects, started through `start`, in
/// `bulk_start_ids`'s own order. Closure-injected (mirrors
/// `quit::stop_all_with`'s own shape) so the DISPATCH is testable without a
/// real `Supervisor` — a test passes a closure that just records ids into a
/// `Vec` instead of spawning a child process.
///
/// `bulk_start_ids` itself already carries the full spec D6/D8 test suite
/// (endpoint collapsing, `demo-ticker` exclusion, non-terminal skip — see
/// `model.rs`) — this function's own tests exist only to prove "calling
/// this actually invokes `start` for exactly that list, in order", not to
/// re-prove the selection rule a second time.
pub(super) fn start_all_with(services: &[ServiceStatus], start: impl Fn(&str)) {
    for id in bulk_start_ids(services) {
        start(&id);
    }
}

/// The title/body a tray-started service's `Failed` transition shows in the
/// native error dialog (spec D4) — split out as a pure function so the
/// VERBATIM stderr-tail claim is testable without a real dialog (spec D9: a
/// `muda`/`tray-icon` menu cannot be constructed in a test, and neither can
/// `rfd`'s native alert).
pub(super) fn failure_dialog_text(
    display_name: &str,
    exit: Option<i32>,
    stderr_tail: &[String],
) -> (String, String) {
    let title = format!("{display_name} failed to start");
    let exit_line = match exit {
        Some(code) => format!("Exit code: {code}"),
        None => "Exit code: unknown (killed by signal, or never launched)".to_string(),
    };
    let tail = if stderr_tail.is_empty() {
        "(no stderr captured)".to_string()
    } else {
        // `.join("\n")`, not a debug-formatted `Vec` or any other
        // transformation — spec D4 says VERBATIM, and a `{stderr_tail:?}`
        // here would wrap every line in escaped quotes and commas instead
        // of showing the operator what the process actually printed.
        stderr_tail.join("\n")
    };
    (title, format!("{exit_line}\n\n{tail}"))
}

/// Decide whether a `StateChanged` transition owes the tray a failure
/// dialog, and update `tracked` to reflect the transition resolving the
/// service's tray-dispatched start attempt (spec D4).
///
/// `Some((title, body))` only when BOTH: `id` was marked tray-initiated (a
/// row click or a bulk start dispatched THIS attempt — see
/// [`TrayInitiated`]'s own doc comment), AND `state` is `Failed`. Every
/// other case resolves the tracked attempt WITHOUT a dialog: `Running` is
/// the attempt succeeding; `Stopped` is some other actor (bulk Stop-all, the
/// Services page's own `stop_service`) intervening before it could fail.
/// `Starting` is not a resolution at all, so `tracked` is left untouched —
/// the caller never even reaches this function on `Starting` today (`Log`
/// is filtered upstream and only `StateChanged`/`Registered` reach the
/// subscriber loop's dispatch at all, and `Registered` never carries
/// `Starting` either — a freshly registered service is always `Stopped`),
/// but this stays exhaustive and correct even if that changes.
///
/// This IS the id-lifecycle half of spec D4's "a non-tray-initiated failure
/// does not raise a dialog": [`TrayInitiated::resolve`] removes `id` the
/// moment its attempt resolves ANY way, which is also what keeps
/// [`TrayInitiated`] from growing without bound (see its own doc comment) —
/// an id can only ever be "in flight" between being marked and its very
/// next `StateChanged`.
pub(super) fn dialog_for_transition(
    tracked: &TrayInitiated,
    id: &str,
    display_name: &str,
    state: &ServiceState,
) -> Option<(String, String)> {
    match state {
        ServiceState::Starting => None,
        ServiceState::Failed { exit, stderr_tail } => tracked
            .resolve(id)
            .then(|| failure_dialog_text(display_name, *exit, stderr_tail)),
        ServiceState::Running | ServiceState::Stopped => {
            tracked.resolve(id);
            None
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn failed(stderr_tail: Vec<&str>) -> ServiceState {
        ServiceState::Failed {
            exit: Some(1),
            stderr_tail: stderr_tail.into_iter().map(str::to_string).collect(),
        }
    }

    fn status(id: &str, endpoint: Option<&str>, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: endpoint.map(str::to_string),
            pid: None,
            state,
        }
    }

    // -----------------------------------------------------------------
    // try_acquire_bulk
    // -----------------------------------------------------------------

    #[test]
    fn succeeds_and_holds_both_locks_when_neither_is_taken() {
        let bulk = tokio::sync::Mutex::new(());
        let apply = tokio::sync::Mutex::new(());
        let guards = try_acquire_bulk(&bulk, &apply);
        assert!(guards.is_some());
        // Holding both: a further probe (standing in for a SECOND dispatch
        // racing this one) must fail while `guards` is alive.
        assert!(bulk.try_lock().is_err());
        assert!(apply.try_lock().is_err());
    }

    /// VACUITY (neuter-and-watch-it-fail): temporarily dropped the `bulk`
    /// check (`let bulk_guard = bulk.try_lock().ok()?;` replaced with
    /// `apply.try_lock().ok()?` reused for both slots of the returned
    /// tuple) — this test failed: a pre-held `bulk` guard no longer blocked
    /// anything, since the function never actually looked at `bulk` at all.
    /// Restoring the real `bulk.try_lock()` call made it pass again.
    #[test]
    fn rejects_when_the_bulk_lock_is_already_held() {
        let bulk = tokio::sync::Mutex::new(());
        let apply = tokio::sync::Mutex::new(());
        let _held = bulk.try_lock().expect("test setup: lock must be free");
        assert!(try_acquire_bulk(&bulk, &apply).is_none());
    }

    /// The mirror image of the test above: THIS is the claim specific to
    /// spec D7's "a try_lock on the EXISTING apply lock" — a bulk action
    /// must also reject while `apply_config` (or another bulk action) holds
    /// the SAME `ApplyLock` a real caller passes here.
    #[test]
    fn rejects_when_the_apply_lock_is_already_held() {
        let bulk = tokio::sync::Mutex::new(());
        let apply = tokio::sync::Mutex::new(());
        let _held = apply.try_lock().expect("test setup: lock must be free");
        assert!(try_acquire_bulk(&bulk, &apply).is_none());
    }

    /// A rejection caused by `apply` must not leave `bulk` held: `bulk` is
    /// acquired FIRST inside `try_acquire_bulk`, before `apply` is even
    /// probed, so a naive implementation could plausibly forget to release
    /// it on the later failure. This proves the failed attempt released it
    /// again rather than leaking a guard nothing will ever drop — the exact
    /// property that keeps a REJECTED bulk dispatch from permanently
    /// wedging the REAL `BulkLock` for every later click.
    #[test]
    fn a_rejected_attempt_releases_the_bulk_lock_it_had_already_taken() {
        let bulk = tokio::sync::Mutex::new(());
        let apply = tokio::sync::Mutex::new(());
        let held_apply = apply.try_lock().expect("test setup: lock must be free");
        assert!(try_acquire_bulk(&bulk, &apply).is_none());
        drop(held_apply);
        assert!(
            bulk.try_lock().is_ok(),
            "a failed apply-lock probe must not leave the bulk lock held"
        );
    }

    // -----------------------------------------------------------------
    // start_all_with
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily replaced the
    /// `bulk_start_ids(services)` call with
    /// `services.iter().map(|s| s.id.clone()).collect::<Vec<_>>()` (i.e.
    /// start EVERYTHING, ignoring terminal-state/endpoint filtering) — this
    /// test failed, recording the already-`Running` php-fpm id and BOTH
    /// colliding mysql ids too. Restoring the real `bulk_start_ids` call
    /// made it pass again.
    #[test]
    fn starts_only_the_ids_bulk_start_ids_selects_in_order() {
        let services = [
            status("nginx", Some("http://127.0.0.1:80"), ServiceState::Stopped),
            status("php-fpm-8.4", Some("run/a.sock"), ServiceState::Running),
            status("mysql-8.4", Some("127.0.0.1:3306"), ServiceState::Stopped),
            status("mysql-8.1", Some("127.0.0.1:3306"), ServiceState::Stopped),
        ];
        let started: Mutex<Vec<String>> = Mutex::new(Vec::new());
        start_all_with(&services, |id| {
            started
                .lock()
                .expect("test mutex poisoned")
                .push(id.to_string())
        });
        // `bulk_start_ids` preserves `services`' own input order (it only
        // filters, never sorts) — `nginx` precedes `mysql-8.4` here because
        // it does in `services` above; `php-fpm-8.4` (not terminal) and
        // `mysql-8.1` (endpoint already claimed by `mysql-8.4`) are both
        // absent.
        assert_eq!(
            started.into_inner().expect("test mutex poisoned"),
            vec!["nginx".to_string(), "mysql-8.4".to_string()],
        );
    }

    #[test]
    fn starting_nothing_calls_start_zero_times() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Running,
        )];
        let calls: Mutex<u32> = Mutex::new(0);
        start_all_with(&services, |_| {
            *calls.lock().expect("test mutex poisoned") += 1;
        });
        assert_eq!(*calls.lock().expect("test mutex poisoned"), 0);
    }

    // -----------------------------------------------------------------
    // failure_dialog_text: the stderr tail must appear VERBATIM.
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily formatted the tail
    /// with `format!("{stderr_tail:?}")` (a `Debug` dump of the `Vec`)
    /// instead of `.join("\n")` — this test failed: the asserted exact
    /// nginx error line no longer appeared unescaped/unquoted in `body`.
    /// Restoring `.join("\n")` made it pass again.
    #[test]
    fn dialog_text_contains_the_stderr_tail_verbatim() {
        let (_, body) = failure_dialog_text(
            "nginx",
            Some(1),
            &[
                "nginx: [emerg] bind() to 0.0.0.0:80 failed (48: Address already in use)"
                    .to_string(),
                "nginx: still could not bind()".to_string(),
            ],
        );
        assert!(
            body.contains(
                "nginx: [emerg] bind() to 0.0.0.0:80 failed (48: Address already in use)"
            )
        );
        assert!(body.contains("nginx: still could not bind()"));
    }

    #[test]
    fn dialog_text_names_the_exit_code_and_display_name() {
        let (title, body) = failure_dialog_text("nginx", Some(13), &[]);
        assert_eq!(title, "nginx failed to start");
        assert!(body.contains("Exit code: 13"));
    }

    #[test]
    fn dialog_text_is_honest_about_a_missing_exit_code() {
        let (_, body) = failure_dialog_text("nginx", None, &[]);
        assert!(body.contains("Exit code: unknown"));
    }

    // -----------------------------------------------------------------
    // dialog_for_transition: the id-tracking half of spec D4.
    // -----------------------------------------------------------------

    #[test]
    fn a_tracked_id_reaching_failed_returns_dialog_text_and_stops_tracking_it() {
        let tracked = TrayInitiated::default();
        tracked.mark("nginx");

        let result = dialog_for_transition(&tracked, "nginx", "nginx", &failed(vec!["boom"]));
        assert!(result.is_some());
        assert!(result.expect("checked above").1.contains("boom"));

        // Resolved: a SECOND Failed for the same id (e.g. a much later,
        // unrelated crash) must not dialog again — see this module's own
        // doc comment on `dialog_for_transition` for why that is the
        // correct behaviour, not a bug.
        assert!(dialog_for_transition(&tracked, "nginx", "nginx", &failed(vec!["boom"])).is_none());
    }

    /// THE non-tray-initiated claim (spec D4): an id nobody marked must
    /// never dialog, no matter how it fails.
    ///
    /// VACUITY (neuter-and-watch-it-fail): temporarily replaced
    /// `tracked.resolve(id).then(...)` with an unconditional
    /// `Some(failure_dialog_text(...))` — this test failed (it got a dialog
    /// for an untracked id). Restoring the `tracked.resolve(id)` gate made
    /// it pass again.
    #[test]
    fn an_untracked_id_reaching_failed_raises_no_dialog() {
        let tracked = TrayInitiated::default();
        assert!(dialog_for_transition(&tracked, "nginx", "nginx", &failed(vec!["boom"])).is_none());
    }

    #[test]
    fn reaching_running_resolves_tracking_without_a_dialog() {
        let tracked = TrayInitiated::default();
        tracked.mark("nginx");
        assert!(
            dialog_for_transition(&tracked, "nginx", "nginx", &ServiceState::Running).is_none()
        );
        // Resolved: a LATER Failed for the same id is a DIFFERENT attempt
        // this function never saw start (this one already succeeded), so
        // it must not dialog either.
        assert!(dialog_for_transition(&tracked, "nginx", "nginx", &failed(vec!["boom"])).is_none());
    }

    #[test]
    fn reaching_stopped_resolves_tracking_without_a_dialog() {
        let tracked = TrayInitiated::default();
        tracked.mark("nginx");
        assert!(
            dialog_for_transition(&tracked, "nginx", "nginx", &ServiceState::Stopped).is_none()
        );
    }

    #[test]
    fn starting_never_dialogs_and_never_resolves_tracking() {
        let tracked = TrayInitiated::default();
        tracked.mark("nginx");
        assert!(
            dialog_for_transition(&tracked, "nginx", "nginx", &ServiceState::Starting).is_none()
        );
        // Still tracked: a Failed immediately afterward is the SAME attempt
        // and must still dialog.
        assert!(dialog_for_transition(&tracked, "nginx", "nginx", &failed(vec!["boom"])).is_some());
    }
}
