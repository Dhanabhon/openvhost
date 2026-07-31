// SPDX-License-Identifier: GPL-3.0-or-later
//! The pure tray/menu-bar model (P1 tray design
//! `docs/superpowers/specs/2026-07-31-p1-tray-design.md`, spec
//! D2/D3/D5/D6/D9): given the supervisor's current [`ServiceStatus`] rows,
//! decide what the menu should say and which glyph the tray icon should
//! show. Nothing here touches a tray, a menu, or the filesystem — that is
//! deliberate (spec D9): it is the seam that lets the rest of the slice be
//! driven by `cargo test` instead of AppKit (no `NSStatusItem`/`NSMenu`, no
//! main thread, no `NSApp` in a test process). A later Phase B commit wires
//! a real `muda`/`tray-icon` menu to what this module computes.
//!
//! RED-first status: this module and its whole test suite are new in this
//! commit — every test below failed to compile/panicked against
//! `todo!()`-bodied stubs before the real implementation landed (see the
//! task report for the exhaustiveness-specific proof, which goes further
//! than "it didn't exist yet": a variant was temporarily added to
//! `openvhost_proc::ServiceState` and `cargo check` was confirmed to break
//! at `toggle_action`'s match with "non-exhaustive patterns" before being
//! reverted).

use std::collections::HashSet;

use openvhost_proc::{ServiceState, ServiceStatus};

/// A service id excluded from every bulk action regardless of its state
/// (spec D6/D8). `lib.rs` registers `demo-ticker` purely for local
/// development — it deliberately fails after 45 ticks — and a faithful
/// "Start all" must not try to bring up a fake service that exists only to
/// prove the `Failed` path works.
const DEMO_TICKER_ID: &str = "demo-ticker";

/// The tray icon's glyph. Four states, matching `docs/design/README.md`'s
/// aggregate contract — `failed > starting > running > stopped` — exactly
/// (spec D5): that document already names the tray as a future consumer of
/// this precedence, and inventing a smaller set here would let the titlebar
/// and the tray disagree about the same stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IconState {
    Stopped,
    Running,
    Starting,
    Failed,
}

/// What a per-service row's click should do. Carries the row's service id so
/// the caller (the Phase B click router) can dispatch without re-deriving
/// it.
///
/// `None` is not a wildcard escape hatch — it is the honest answer for a
/// `Starting` row (spec D3): `Supervisor::stop` on a service still coming up
/// is queued until its readiness probe finishes, so wiring a click there
/// would be a silent no-op. [`tray_model`] renders that row disabled for the
/// identical reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Start(String),
    Stop(String),
    Retry(String),
    None,
}

/// Whether a bulk action (Start all / Stop all) is currently in flight.
///
/// Spec D7: bulk actions take a lock and reject a second call rather than
/// queuing it, and the menu must render that — both rows disable while
/// [`BulkState::Busy`]. The real lock (a `BulkLock`, Phase B) is what
/// produces this value via a `try_lock` probe; this module stays ignorant
/// of `tokio::sync::Mutex` entirely and only renders the yes/no answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BulkState {
    #[default]
    Idle,
    Busy,
}

/// One per-service menu row.
///
/// `label` is always `"<Action> <name> — <State>"` (spec D2) — never a
/// checkmark: a `CheckMenuItem` is a boolean where this four-state enum
/// belongs, the exact collapse this codebase has hit three times already.
/// `enabled` is `false` only for a `Starting` row (spec D3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceRow {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

/// The whole tray menu's content, recomputed fresh from a
/// `Supervisor::snapshot()`-shaped slice every time (spec D2: this module
/// never applies event deltas — the Phase B apply step diffs two of these
/// against each other instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayModel {
    /// The disabled aggregate-summary row shown above "Start all"/"Stop
    /// all" (spec D2's menu layout).
    pub summary: String,
    /// One row per registered service, in `services`' own order.
    pub rows: Vec<ServiceRow>,
    /// Whether "Start all" should be enabled.
    pub start_all_enabled: bool,
    /// Whether "Stop all" should be enabled.
    pub stop_all_enabled: bool,
    /// The tray icon's glyph — [`aggregate_icon`] run over the same slice.
    pub icon: IconState,
}

/// Build the tray's whole content from the supervisor's current rows.
///
/// Pure: the same inputs always produce the same [`TrayModel`], field for
/// field — the Phase B apply step relies on that to diff against the
/// previously applied model rather than re-deriving change detection of its
/// own.
pub fn tray_model(services: &[ServiceStatus], bulk: BulkState) -> TrayModel {
    let icon = aggregate_icon(services);
    let rows: Vec<ServiceRow> = services.iter().map(service_row).collect();
    let idle = bulk == BulkState::Idle;
    TrayModel {
        summary: summary_line(services, icon),
        rows,
        // Disabled outright while a bulk action holds the lock (spec D7);
        // otherwise enabled only when there is something real for the
        // action to do. An always-clickable "Start all" that starts
        // nothing whenever the stack is already fully up would be a
        // second silent no-op alongside the one D3 already names for a
        // disabled `Starting` row.
        start_all_enabled: idle && !bulk_start_ids(services).is_empty(),
        stop_all_enabled: idle && services.iter().any(|s| is_pending(&s.state)),
        icon,
    }
}

/// One [`ServiceRow`] for a single [`ServiceStatus`].
fn service_row(status: &ServiceStatus) -> ServiceRow {
    let (verb, state_name) = verb_and_state_name(&status.state);
    let name = &status.display_name;
    ServiceRow {
        id: status.id.clone(),
        label: format!("{verb} {name} — {state_name}"),
        enabled: !matches!(status.state, ServiceState::Starting),
    }
}

/// The verb and human state name a row's label shows for `state` (spec D2's
/// three worked examples, plus `Starting`'s own honest-but-disabled row,
/// spec D3).
///
/// Deliberately a SEPARATE exhaustive match from [`toggle_action`]'s,
/// answering a different question — "what should this row's text say"
/// rather than "what should a click do right now". The two agree for three
/// of four states and diverge exactly at `Starting`, where the label still
/// names `Stop` (the action that will apply once the service becomes
/// cancel-safe) while [`toggle_action`] answers `Action::None` for that same
/// state. Folding the two into one function would either lose that honesty
/// or smuggle a state-dependent special case into `Action::None`, which is
/// meant to stay state-agnostic.
fn verb_and_state_name(state: &ServiceState) -> (&'static str, &'static str) {
    match state {
        ServiceState::Stopped => ("Start", "Stopped"),
        ServiceState::Starting => ("Stop", "Starting"),
        ServiceState::Running => ("Stop", "Running"),
        ServiceState::Failed { .. } => ("Retry", "Failed"),
    }
}

/// What a click on this service's row should do right now.
///
/// Exhaustive, no wildcard arm: a future `ServiceState` variant must fail to
/// compile here rather than silently falling into a catch-all that renders
/// the new state inert. Takes `id` separately from `state` because
/// `ServiceState` alone carries no identifier, and every non-`None` `Action`
/// needs one to be dispatchable.
pub fn toggle_action(id: &str, state: &ServiceState) -> Action {
    match state {
        ServiceState::Stopped => Action::Start(id.to_string()),
        ServiceState::Starting => Action::None,
        ServiceState::Running => Action::Stop(id.to_string()),
        ServiceState::Failed { .. } => Action::Retry(id.to_string()),
    }
}

/// Whether `state` has a live child a bulk action still needs to deal with —
/// i.e. NOT terminal. Mirrors `quit::is_pending`'s exact semantics but is
/// deliberately duplicated rather than imported: `quit` is lifecycle/Tauri
/// glue this pure model has no reason to depend on, and both are a one-line
/// check against the same four-variant enum, so any drift between them
/// would be a one-line, easy-to-spot diff in review.
fn is_pending(state: &ServiceState) -> bool {
    !matches!(state, ServiceState::Stopped | ServiceState::Failed { .. })
}

/// The ids "Start all" should start: every registered service that
///
/// - is NOT `demo-ticker` (spec D6/D8, regardless of its current state),
/// - is in a terminal state (`Stopped` or `Failed` — anything already
///   `Starting`/`Running` needs no help getting there), and
/// - is the FIRST service seen carrying its particular `endpoint` value.
///
/// The last rule exists because every `mysql-<major>` declares the literal
/// endpoint `127.0.0.1:3306` (verified: `stack.rs:171` says so explicitly)
/// — starting two of them at once guarantees an "Address already in use"
/// failure for the second. `php-fpm-<major>` rows do NOT collapse: each
/// pool's endpoint is its own per-major unix-socket path, so two majors are
/// two distinct endpoint values and both survive the filter. A `None`
/// endpoint never collides with another `None` — the absence of an
/// endpoint says nothing about address contention, so every no-endpoint
/// service is kept independently rather than only the first one seen.
pub fn bulk_start_ids(services: &[ServiceStatus]) -> Vec<String> {
    let mut seen_endpoints: HashSet<&str> = HashSet::new();
    services
        .iter()
        .filter(|s| s.id != DEMO_TICKER_ID)
        .filter(|s| !is_pending(&s.state))
        .filter_map(|s| match s.endpoint.as_deref() {
            Some(endpoint) => seen_endpoints.insert(endpoint).then_some(s.id.clone()),
            None => Some(s.id.clone()),
        })
        .collect()
}

/// The tray icon's glyph for the whole stack: the HIGHEST-precedence state
/// among every service, per `docs/design/README.md`'s aggregate contract —
/// `failed > starting > running > stopped` — exactly (spec D5). An empty
/// slice reports `Stopped`: with nothing registered there is nothing
/// running, starting, or failed to report either, and `Stopped` is the
/// least alarming default.
pub fn aggregate_icon(services: &[ServiceStatus]) -> IconState {
    services
        .iter()
        .map(|s| icon_for_state(&s.state))
        .max_by_key(severity)
        .unwrap_or(IconState::Stopped)
}

/// [`IconState`] for one [`ServiceState`] — the per-service half of
/// [`aggregate_icon`], kept separate so the aggregation itself
/// (`max_by_key(severity)`) stays a one-liner.
fn icon_for_state(state: &ServiceState) -> IconState {
    match state {
        ServiceState::Stopped => IconState::Stopped,
        ServiceState::Running => IconState::Running,
        ServiceState::Starting => IconState::Starting,
        ServiceState::Failed { .. } => IconState::Failed,
    }
}

/// `failed > starting > running > stopped`, encoded as an ordinal so
/// [`aggregate_icon`] can pick the maximum with one `Iterator::max_by_key`
/// call instead of a hand-rolled four-way comparison. Private: nothing
/// outside this module needs to compare `IconState`s by rank, only by
/// equality (`IconState` derives `Eq` for that).
fn severity(icon: &IconState) -> u8 {
    match icon {
        IconState::Stopped => 0,
        IconState::Running => 1,
        IconState::Starting => 2,
        IconState::Failed => 3,
    }
}

/// The disabled summary row's text (spec D2's menu layout names this row;
/// it does not dictate its exact wording). Not part of the task brief's
/// binding test list, but named as part of [`TrayModel`]'s own contract, so
/// it gets reasoned behavior rather than a placeholder: name the
/// failed/starting count while either is nonzero — the two states a user
/// most wants a number for — fall back to a running count once neither
/// applies, and say "All stopped" only in the one case `icon == Stopped`
/// can mean, since [`aggregate_icon`] only reports `Stopped` when every
/// single row is `Stopped` (anything else would outrank it).
fn summary_line(services: &[ServiceStatus], icon: IconState) -> String {
    if services.is_empty() {
        return "No services registered".to_string();
    }
    let total = services.len();
    match icon {
        IconState::Failed => {
            let failed = services
                .iter()
                .filter(|s| matches!(s.state, ServiceState::Failed { .. }))
                .count();
            format!("{failed} of {total} failed")
        }
        IconState::Starting => {
            let starting = services
                .iter()
                .filter(|s| matches!(s.state, ServiceState::Starting))
                .count();
            format!("{starting} of {total} starting")
        }
        IconState::Running => {
            let running = services
                .iter()
                .filter(|s| matches!(s.state, ServiceState::Running))
                .count();
            if running == total {
                "All running".to_string()
            } else {
                format!("{running} of {total} running")
            }
        }
        IconState::Stopped => "All stopped".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(id: &str, endpoint: Option<&str>, state: ServiceState) -> ServiceStatus {
        ServiceStatus {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: endpoint.map(str::to_string),
            pid: None,
            state,
        }
    }

    fn failed() -> ServiceState {
        ServiceState::Failed {
            exit: Some(1),
            stderr_tail: vec!["boom".to_string()],
        }
    }

    // -----------------------------------------------------------------
    // Every state's row label + enablement (tray_model / service_row).
    // -----------------------------------------------------------------

    #[test]
    fn stopped_row_is_enabled_and_offers_start() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Stopped,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(
            model.rows,
            vec![ServiceRow {
                id: "nginx".to_string(),
                label: "Start nginx — Stopped".to_string(),
                enabled: true,
            }]
        );
    }

    #[test]
    fn running_row_is_enabled_and_offers_stop() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Running,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.rows[0].label, "Stop nginx — Running");
        assert!(model.rows[0].enabled);
    }

    #[test]
    fn starting_row_is_disabled_but_still_names_the_pending_stop_action() {
        let services = [status(
            "mysql-8.4",
            Some("127.0.0.1:3306"),
            ServiceState::Starting,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.rows[0].label, "Stop mysql-8.4 — Starting");
        assert!(
            !model.rows[0].enabled,
            "a Starting row must render disabled — spec D3"
        );
    }

    #[test]
    fn failed_row_is_enabled_and_offers_retry() {
        let services = [status(
            "php-fpm-8.4",
            Some("run/php-fpm-8.4.sock"),
            failed(),
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.rows[0].label, "Retry php-fpm-8.4 — Failed");
        assert!(model.rows[0].enabled);
    }

    // -----------------------------------------------------------------
    // toggle_action: exhaustive, per-state mapping (no wildcard arm —
    // see the task report for the compile-break proof).
    // -----------------------------------------------------------------

    #[test]
    fn toggle_action_maps_every_state_to_its_own_action() {
        assert_eq!(
            toggle_action("nginx", &ServiceState::Stopped),
            Action::Start("nginx".to_string())
        );
        assert_eq!(
            toggle_action("nginx", &ServiceState::Running),
            Action::Stop("nginx".to_string())
        );
        assert_eq!(
            toggle_action("nginx", &ServiceState::Starting),
            Action::None
        );
        assert_eq!(
            toggle_action("nginx", &failed()),
            Action::Retry("nginx".to_string())
        );
    }

    // -----------------------------------------------------------------
    // bulk_start_ids: endpoint collapsing, non-terminal skip, demo-ticker.
    // -----------------------------------------------------------------

    #[test]
    fn two_mysql_majors_sharing_the_literal_3306_endpoint_collapse_to_one_id() {
        let services = [
            status("mysql-8.4", Some("127.0.0.1:3306"), ServiceState::Stopped),
            status("mysql-8.1", Some("127.0.0.1:3306"), ServiceState::Stopped),
        ];
        assert_eq!(bulk_start_ids(&services), vec!["mysql-8.4".to_string()]);
    }

    #[test]
    fn two_php_fpm_majors_have_distinct_endpoints_and_both_start() {
        let services = [
            status(
                "php-fpm-8.1",
                Some("run/php-fpm-8.1.sock"),
                ServiceState::Stopped,
            ),
            status(
                "php-fpm-8.3",
                Some("run/php-fpm-8.3.sock"),
                ServiceState::Stopped,
            ),
        ];
        assert_eq!(
            bulk_start_ids(&services),
            vec!["php-fpm-8.1".to_string(), "php-fpm-8.3".to_string()]
        );
    }

    #[test]
    fn demo_ticker_is_never_a_bulk_start_candidate() {
        let services = [status("demo-ticker", None, ServiceState::Stopped)];
        assert!(bulk_start_ids(&services).is_empty());
    }

    #[test]
    fn non_terminal_services_are_skipped() {
        let services = [
            status("nginx", Some("http://127.0.0.1:80"), ServiceState::Running),
            status(
                "php-fpm-8.4",
                Some("run/php-fpm-8.4.sock"),
                ServiceState::Starting,
            ),
        ];
        assert!(bulk_start_ids(&services).is_empty());
    }

    #[test]
    fn failed_services_are_terminal_and_are_bulk_start_candidates() {
        let services = [status("nginx", Some("http://127.0.0.1:80"), failed())];
        assert_eq!(bulk_start_ids(&services), vec!["nginx".to_string()]);
    }

    #[test]
    fn two_services_with_no_endpoint_do_not_collide_with_each_other() {
        let services = [
            status("a", None, ServiceState::Stopped),
            status("b", None, ServiceState::Stopped),
        ];
        assert_eq!(
            bulk_start_ids(&services),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    // -----------------------------------------------------------------
    // aggregate_icon: every precedence pair, and the empty-list edge case.
    // -----------------------------------------------------------------

    #[test]
    fn failed_outranks_starting() {
        let a = [
            status("a", None, ServiceState::Starting),
            status("b", None, failed()),
        ];
        assert_eq!(aggregate_icon(&a), IconState::Failed);
        // Order-independent: aggregation must not depend on list position.
        let b = [
            status("b", None, failed()),
            status("a", None, ServiceState::Starting),
        ];
        assert_eq!(aggregate_icon(&b), IconState::Failed);
    }

    #[test]
    fn failed_outranks_running() {
        let services = [
            status("a", None, ServiceState::Running),
            status("b", None, failed()),
        ];
        assert_eq!(aggregate_icon(&services), IconState::Failed);
    }

    #[test]
    fn failed_outranks_stopped() {
        let services = [
            status("a", None, ServiceState::Stopped),
            status("b", None, failed()),
        ];
        assert_eq!(aggregate_icon(&services), IconState::Failed);
    }

    #[test]
    fn starting_outranks_running() {
        let services = [
            status("a", None, ServiceState::Running),
            status("b", None, ServiceState::Starting),
        ];
        assert_eq!(aggregate_icon(&services), IconState::Starting);
    }

    #[test]
    fn starting_outranks_stopped() {
        let services = [
            status("a", None, ServiceState::Stopped),
            status("b", None, ServiceState::Starting),
        ];
        assert_eq!(aggregate_icon(&services), IconState::Starting);
    }

    #[test]
    fn running_outranks_stopped() {
        let services = [
            status("a", None, ServiceState::Stopped),
            status("b", None, ServiceState::Running),
        ];
        assert_eq!(aggregate_icon(&services), IconState::Running);
    }

    #[test]
    fn empty_service_list_reports_stopped_icon() {
        assert_eq!(aggregate_icon(&[]), IconState::Stopped);
    }

    // -----------------------------------------------------------------
    // tray_model: bulk enablement, summary line, empty list.
    // -----------------------------------------------------------------

    #[test]
    fn empty_service_list_produces_an_inert_model() {
        let model = tray_model(&[], BulkState::Idle);
        assert_eq!(model.summary, "No services registered");
        assert!(model.rows.is_empty());
        assert!(!model.start_all_enabled);
        assert!(!model.stop_all_enabled);
        assert_eq!(model.icon, IconState::Stopped);
    }

    #[test]
    fn busy_bulk_state_disables_both_bulk_rows_regardless_of_service_states() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Stopped,
        )];
        let model = tray_model(&services, BulkState::Busy);
        assert!(!model.start_all_enabled);
        assert!(!model.stop_all_enabled);
    }

    #[test]
    fn start_all_disabled_when_nothing_is_startable() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Running,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert!(
            !model.start_all_enabled,
            "nothing terminal to start — Start all would be a no-op"
        );
    }

    #[test]
    fn stop_all_disabled_when_nothing_is_pending() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Stopped,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert!(
            !model.stop_all_enabled,
            "nothing running/starting to stop — Stop all would be a no-op"
        );
    }

    #[test]
    fn start_all_enabled_when_something_terminal_exists() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Stopped,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert!(model.start_all_enabled);
    }

    #[test]
    fn stop_all_enabled_when_something_pending_exists() {
        let services = [status(
            "nginx",
            Some("http://127.0.0.1:80"),
            ServiceState::Running,
        )];
        let model = tray_model(&services, BulkState::Idle);
        assert!(model.stop_all_enabled);
    }

    #[test]
    fn summary_counts_failures_first() {
        let services = [
            status("a", None, failed()),
            status("b", None, ServiceState::Starting),
            status("c", None, ServiceState::Running),
        ];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.summary, "1 of 3 failed");
    }

    #[test]
    fn summary_counts_starting_when_nothing_failed() {
        let services = [
            status("a", None, ServiceState::Starting),
            status("b", None, ServiceState::Running),
        ];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.summary, "1 of 2 starting");
    }

    #[test]
    fn summary_says_all_running_when_every_row_is_running() {
        let services = [
            status("a", None, ServiceState::Running),
            status("b", None, ServiceState::Running),
        ];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.summary, "All running");
    }

    #[test]
    fn summary_names_a_partial_running_count() {
        let services = [
            status("a", None, ServiceState::Running),
            status("b", None, ServiceState::Stopped),
        ];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.summary, "1 of 2 running");
    }

    #[test]
    fn summary_says_all_stopped_when_every_row_is_stopped() {
        let services = [status("a", None, ServiceState::Stopped)];
        let model = tray_model(&services, BulkState::Idle);
        assert_eq!(model.summary, "All stopped");
    }
}
