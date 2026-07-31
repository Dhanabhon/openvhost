// SPDX-License-Identifier: GPL-3.0-or-later
//! Diff two [`TrayModel`]s and apply the minimal set of native mutations
//! (P1 tray design `docs/superpowers/specs/2026-07-31-p1-tray-design.md`,
//! spec D2/D9).
//!
//! This module is deliberately as tauri-free as [`crate::tray::model`]: the
//! [`TraySink`] trait is the seam (spec D9) that lets [`apply`] be driven by
//! a plain recording fake in `cargo test` instead of a real `muda`/
//! `tray-icon` menu (no `NSStatusItem`/`NSMenu`, no main thread, no `NSApp`
//! in a test process). The real sink — the one that actually holds
//! `MenuItem`/`TrayIcon` handles and round-trips to the main thread — is
//! `crate::tray::TrayHandle`, built in `mod.rs` alongside the rest of the
//! real tray construction.
//!
//! Vacuity status: this whole module (the trait, `apply`, and its test
//! suite) is new in this commit, and every test below was passing against
//! the implementation as written. Rather than assert that in the abstract,
//! the trickiest invariants were each separately verified by
//! neuter-and-watch-it-fail — temporarily breaking exactly the guard a
//! test claims to protect, confirming the SPECIFIC test (and no unrelated
//! one) fails, then restoring it — recorded on each test's own doc
//! comment below, including one case where the first neuter attempted
//! did NOT fail the test (documented on
//! `identical_models_produce_no_calls_at_all`) and a second, more
//! precise neuter was needed to actually falsify the claim.

use crate::tray::model::{IconState, TrayModel};

/// Everything a diff between two [`TrayModel`]s can ask a real tray to do.
///
/// Every method takes `&self`: the real implementation's "mutation" is a
/// `MenuItem`/`TrayIcon` method call that is already interior-mutable (it
/// round-trips to the main thread internally — see `mod.rs`), so `apply`
/// itself never needs `&mut`.
pub trait TraySink {
    /// Update the disabled aggregate-summary row's text.
    fn set_summary(&self, text: &str);
    /// Update the tray icon's glyph.
    fn set_icon(&self, state: IconState);
    /// Enable or disable the "Start all" row.
    fn set_start_all_enabled(&self, enabled: bool);
    /// Enable or disable the "Stop all" row.
    fn set_stop_all_enabled(&self, enabled: bool);
    /// Update one per-service row's label, by service id.
    fn set_row_label(&self, id: &str, label: &str);
    /// Enable or disable one per-service row, by service id.
    fn set_row_enabled(&self, id: &str, enabled: bool);
    /// Throw away and rebuild the per-service row section of the menu from
    /// scratch, and resync EVERY other field too (summary/icon/bulk
    /// enablement) — see [`apply`]'s doc comment for why a rebuild does not
    /// also receive the granular calls above for those other fields.
    fn rebuild(&self, model: &TrayModel);
}

/// Diff `old` against `new` and call the minimal set of [`TraySink`]
/// methods needed to bring a previously-applied `old` state up to `new`.
///
/// Three rules, in order:
///
/// 1. **Unchanged is silent.** If `old == new` field-for-field, this calls
///    NOTHING — not even a redundant `set_summary` with the same text. This
///    is what makes repeated recomputation from a fresh
///    `Supervisor::snapshot()` (spec D2: never apply event deltas) cheap
///    enough to do on every state event: most events change one service's
///    state, not the whole model, and the rest of a busy stack's rows stay
///    silent.
/// 2. **A membership change is a [`TraySink::rebuild`], never a mutation.**
///    If the SET of row ids differs (a service was registered after
///    launch — spec D2's whole reason `SupervisorEvent::Registered` exists —
///    there is no removal path today, but this checks the set both ways
///    rather than assuming growth-only), every other diff is skipped: there
///    is no existing `MenuItem` handle for a brand-new row to mutate, so
///    the only correct move is to hand the whole fresh model to the real
///    sink and let it reconstruct the row section (and resync everything
///    else — see [`TraySink::rebuild`]'s own doc comment for why that
///    includes summary/icon/bulk enablement too).
/// 3. **Otherwise, one call per CHANGED field, and nothing per unchanged
///    one.** Summary, icon, both bulk rows, and each per-service row's
///    label/enabled are compared independently; a transition that only
///    flips one service from `Running` to `Failed` therefore emits exactly
///    that row's label + enabled calls, plus a summary/icon call (the
///    aggregate almost always changes too), and nothing else.
///
/// Callers (real: `mod.rs`'s event-subscriber task; test: this module's own
/// suite) are responsible for calling this at most once inside a single
/// `AppHandle::run_on_main_thread` closure (spec D2's "critical mechanic") —
/// `apply` itself has no opinion about threads; it only decides WHAT to
/// call, not WHEN or WHERE.
pub fn apply(old: &TrayModel, new: &TrayModel, sink: &dyn TraySink) {
    if old == new {
        return;
    }

    let membership_changed = {
        let mut old_ids: Vec<&str> = old.rows.iter().map(|r| r.id.as_str()).collect();
        let mut new_ids: Vec<&str> = new.rows.iter().map(|r| r.id.as_str()).collect();
        old_ids.sort_unstable();
        new_ids.sort_unstable();
        old_ids != new_ids
    };
    if membership_changed {
        sink.rebuild(new);
        return;
    }

    if old.summary != new.summary {
        sink.set_summary(&new.summary);
    }
    if old.icon != new.icon {
        sink.set_icon(new.icon);
    }
    if old.start_all_enabled != new.start_all_enabled {
        sink.set_start_all_enabled(new.start_all_enabled);
    }
    if old.stop_all_enabled != new.stop_all_enabled {
        sink.set_stop_all_enabled(new.stop_all_enabled);
    }

    // Same id SET on both sides (checked above) — every `new` row has a
    // matching `old` row, though not necessarily at the same index, so this
    // pairs them up by id rather than by position. `else { continue }`
    // rather than `.expect(...)` (no unwrap/expect outside tests, workspace
    // rule): the `membership_changed` check above already guarantees this
    // lookup succeeds, so the `else` arm is unreachable in practice, but an
    // unreachable arm that silently skips one row costs nothing while an
    // `.expect()` that somehow DID fire would take the whole tray down
    // over a single stale row.
    for new_row in &new.rows {
        let Some(old_row) = old.rows.iter().find(|r| r.id == new_row.id) else {
            continue;
        };
        if old_row.label != new_row.label {
            sink.set_row_label(&new_row.id, &new_row.label);
        }
        if old_row.enabled != new_row.enabled {
            sink.set_row_enabled(&new_row.id, new_row.enabled);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tray::model::ServiceRow;
    use std::cell::RefCell;

    /// One recorded call. Deliberately a flat enum rather than, say, a
    /// `HashMap<String, ...>` of "latest value per field": a test asserting
    /// against an ordered `Vec<Call>` catches both WHICH calls fired and
    /// that no UNEXPECTED extra call fired, which a map-shaped recorder
    /// would silently collapse (two calls to the same row would just
    /// overwrite each other in a map).
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Call {
        Summary(String),
        Icon(IconState),
        StartAllEnabled(bool),
        StopAllEnabled(bool),
        RowLabel(String, String),
        RowEnabled(String, bool),
        Rebuild(TrayModel),
    }

    #[derive(Default)]
    struct RecordingSink {
        calls: RefCell<Vec<Call>>,
    }

    impl RecordingSink {
        fn calls(&self) -> Vec<Call> {
            self.calls.borrow().clone()
        }
    }

    impl TraySink for RecordingSink {
        fn set_summary(&self, text: &str) {
            self.calls
                .borrow_mut()
                .push(Call::Summary(text.to_string()));
        }
        fn set_icon(&self, state: IconState) {
            self.calls.borrow_mut().push(Call::Icon(state));
        }
        fn set_start_all_enabled(&self, enabled: bool) {
            self.calls.borrow_mut().push(Call::StartAllEnabled(enabled));
        }
        fn set_stop_all_enabled(&self, enabled: bool) {
            self.calls.borrow_mut().push(Call::StopAllEnabled(enabled));
        }
        fn set_row_label(&self, id: &str, label: &str) {
            self.calls
                .borrow_mut()
                .push(Call::RowLabel(id.to_string(), label.to_string()));
        }
        fn set_row_enabled(&self, id: &str, enabled: bool) {
            self.calls
                .borrow_mut()
                .push(Call::RowEnabled(id.to_string(), enabled));
        }
        fn rebuild(&self, model: &TrayModel) {
            self.calls.borrow_mut().push(Call::Rebuild(model.clone()));
        }
    }

    fn row(id: &str, label: &str, enabled: bool) -> ServiceRow {
        ServiceRow {
            id: id.to_string(),
            label: label.to_string(),
            enabled,
        }
    }

    fn model(
        rows: Vec<ServiceRow>,
        summary: &str,
        icon: IconState,
        start: bool,
        stop: bool,
    ) -> TrayModel {
        TrayModel {
            summary: summary.to_string(),
            rows,
            start_all_enabled: start,
            stop_all_enabled: stop,
            icon,
        }
    }

    // -----------------------------------------------------------------
    // Rule 1: unchanged model -> zero calls.
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily made every field
    /// comparison in `apply` unconditional (dropped every `!=` guard,
    /// including the top-level `old == new` short-circuit) — this test
    /// failed, recording all four non-row calls against the asserted empty
    /// `Vec`. Restoring the guards made it pass again.
    ///
    /// Note this ALSO proves the top-level `old == new` short-circuit is
    /// NOT independently load-bearing for this specific property: removing
    /// only it (leaving every per-field `!=` guard intact) still produces
    /// zero calls for equal models, because each per-field guard already
    /// compares that field for equality on its own. The short-circuit's
    /// actual job is skipping the membership-diff computation (two
    /// sorted-vec allocations) and the row-pairing loop on every
    /// recompute, not correctness of "unchanged is silent" — that
    /// property comes from the per-field guards below.
    #[test]
    fn identical_models_produce_no_calls_at_all() {
        let m = model(
            vec![row("nginx", "Stop nginx — Running", true)],
            "All running",
            IconState::Running,
            false,
            true,
        );
        let sink = RecordingSink::default();
        apply(&m, &m.clone(), &sink);
        assert_eq!(sink.calls(), Vec::new());
    }

    // -----------------------------------------------------------------
    // Rule 3: a representative transition emits exactly the changed
    // fields' calls, and nothing for unchanged ones.
    // -----------------------------------------------------------------

    /// The brief's own worked example: one service flips Stopped -> Running.
    /// Its row's label AND enabled-state stay put (both states render
    /// `enabled: true`), so only the LABEL call should fire for that row —
    /// proving `apply` diffs label and enabled independently rather than
    /// emitting both whenever either differs.
    ///
    /// VACUITY (neuter-and-watch-it-fail): temporarily made
    /// `sink.set_row_enabled(&new_row.id, new_row.enabled)` unconditional
    /// (dropped the `old_row.enabled != new_row.enabled` guard) — this
    /// test failed, recording `RowEnabled("nginx", true)` even though
    /// enabled never changed for this transition. Restoring the guard
    /// made it pass again.
    #[test]
    fn a_representative_transition_emits_only_the_fields_that_changed() {
        let old = model(
            vec![row("nginx", "Start nginx — Stopped", true)],
            "All stopped",
            IconState::Stopped,
            true,
            false,
        );
        let new = model(
            vec![row("nginx", "Stop nginx — Running", true)],
            "All running",
            IconState::Running,
            false,
            true,
        );
        let sink = RecordingSink::default();
        apply(&old, &new, &sink);
        assert_eq!(
            sink.calls(),
            vec![
                Call::Summary("All running".to_string()),
                Call::Icon(IconState::Running),
                Call::StartAllEnabled(false),
                Call::StopAllEnabled(true),
                Call::RowLabel("nginx".to_string(), "Stop nginx — Running".to_string()),
            ]
        );
    }

    /// The mirror image of the test above: ONLY `enabled` changes (a
    /// `Starting` row's label still names `Stop` per spec D3, so a
    /// Running -> Starting transition changes enabled from true to false
    /// while the row STAYS labeled "Stop ...").
    #[test]
    fn enabled_only_change_emits_row_enabled_but_not_row_label() {
        let old = model(
            vec![row("mysql-8.4", "Stop mysql-8.4 — Running", true)],
            "All running",
            IconState::Running,
            false,
            true,
        );
        let new = model(
            vec![row("mysql-8.4", "Stop mysql-8.4 — Running", false)],
            "1 of 1 starting",
            IconState::Starting,
            false,
            true,
        );
        let sink = RecordingSink::default();
        apply(&old, &new, &sink);
        assert_eq!(
            sink.calls(),
            vec![
                Call::Summary("1 of 1 starting".to_string()),
                Call::Icon(IconState::Starting),
                Call::RowEnabled("mysql-8.4".to_string(), false),
            ]
        );
    }

    /// Multiple rows: only the row that actually changed emits calls; a
    /// silent second row must stay silent even though the OVERALL model
    /// differs (so the top-level `old == new` short-circuit does not fire).
    #[test]
    fn only_the_changed_row_among_several_emits_calls() {
        let old = model(
            vec![
                row("nginx", "Stop nginx — Running", true),
                row("php-fpm-8.4", "Stop php-fpm-8.4 — Running", true),
            ],
            "All running",
            IconState::Running,
            false,
            true,
        );
        let new = model(
            vec![
                row("nginx", "Stop nginx — Running", true),
                row("php-fpm-8.4", "Retry php-fpm-8.4 — Failed", true),
            ],
            "1 of 2 failed",
            IconState::Failed,
            true,
            true,
        );
        let sink = RecordingSink::default();
        apply(&old, &new, &sink);
        assert_eq!(
            sink.calls(),
            vec![
                Call::Summary("1 of 2 failed".to_string()),
                Call::Icon(IconState::Failed),
                Call::StartAllEnabled(true),
                Call::RowLabel(
                    "php-fpm-8.4".to_string(),
                    "Retry php-fpm-8.4 — Failed".to_string()
                ),
            ]
        );
    }

    // -----------------------------------------------------------------
    // Rule 2: a membership change is a rebuild, never a mutation.
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily hardcoded
    /// `membership_changed` to `false` (i.e. always ran the granular
    /// per-field diff, never `rebuild`) — this test failed, recording an
    /// EMPTY call list against the asserted single `Rebuild`: `old` and
    /// `new` share the same summary/icon/bulk flags in this fixture, and
    /// the granular path's per-row lookup silently skips `mysql-8.4` (no
    /// matching `old` row — see the `else { continue }` comment above),
    /// so nothing about this transition looks "changed" to the granular
    /// path at all. Restoring the membership check made it pass again.
    #[test]
    fn a_new_service_id_triggers_rebuild_and_nothing_else() {
        let old = model(
            vec![row("nginx", "Start nginx — Stopped", true)],
            "All stopped",
            IconState::Stopped,
            true,
            false,
        );
        let new = model(
            vec![
                row("nginx", "Start nginx — Stopped", true),
                row("mysql-8.4", "Start mysql-8.4 — Stopped", true),
            ],
            "All stopped",
            IconState::Stopped,
            true,
            false,
        );
        let sink = RecordingSink::default();
        apply(&old, &new, &sink);
        assert_eq!(sink.calls(), vec![Call::Rebuild(new)]);
    }

    /// The set-difference check must catch a REMOVAL too, not just growth —
    /// `Supervisor::register` never un-registers a service today, but
    /// `apply` is a general-purpose pure function and should not assume
    /// that invariant holds forever.
    #[test]
    fn a_removed_service_id_also_triggers_rebuild() {
        let old = model(
            vec![
                row("nginx", "Stop nginx — Running", true),
                row("mysql-8.4", "Stop mysql-8.4 — Running", true),
            ],
            "All running",
            IconState::Running,
            false,
            true,
        );
        let new = model(
            vec![row("nginx", "Stop nginx — Running", true)],
            "All running",
            IconState::Running,
            false,
            true,
        );
        let sink = RecordingSink::default();
        apply(&old, &new, &sink);
        assert_eq!(sink.calls(), vec![Call::Rebuild(new)]);
    }

    /// A membership change alongside OTHER changed fields (summary/icon)
    /// must still emit exactly the one `Rebuild` call: `rebuild` resyncs
    /// everything itself (see `TraySink::rebuild`'s doc comment), so a
    /// membership-changed diff emitting the granular summary/icon calls
    /// TOO would be redundant double-application, not a bug that changes
    /// the end state — but it would violate "one call per changed field"
    /// and silently hide a future regression that makes `rebuild` stop
    /// resyncing those fields itself.
    #[test]
    fn rebuild_is_the_only_call_even_when_other_fields_also_changed() {
        let old = model(
            vec![row("nginx", "Start nginx — Stopped", true)],
            "All stopped",
            IconState::Stopped,
            true,
            false,
        );
        let new = model(
            vec![
                row("nginx", "Stop nginx — Running", true),
                row("mysql-8.4", "Start mysql-8.4 — Stopped", true),
            ],
            "1 of 2 running",
            IconState::Running,
            true,
            true,
        );
        let sink = RecordingSink::default();
        apply(&old, &new, &sink);
        assert_eq!(sink.calls(), vec![Call::Rebuild(new)]);
    }
}
