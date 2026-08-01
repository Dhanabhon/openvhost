// SPDX-License-Identifier: GPL-3.0-or-later
//! The tray / menu-bar quick-controls slice (P1 tray design
//! `docs/superpowers/specs/2026-07-31-p1-tray-design.md`).
//!
//! - `model` (Phase A): the pure menu model — labels, enablement, aggregate
//!   icon state. No tray, no menu, no AppKit (spec D9).
//! - `apply` (this phase): diff two [`model::TrayModel`]s and call the
//!   minimal [`apply::TraySink`] methods needed, against either a recording
//!   test fake or [`TrayHandle`] below.
//! - This file (Phase B): the real `muda`/`tray-icon` construction, the
//!   supervisor-event subscriber that keeps it live, and
//!   [`handle_tray_menu_id`], the click router.
//!
//! **Scope note (spec D10 — Windows is out for this slice).** Nothing in
//! this file is `#[cfg(target_os = "macos")]`-gated at its DEFINITION: the
//! `tray-icon`/`muda` APIs used here are already cross-platform (the
//! `tray-icon` Cargo feature turns on `tauri::tray` for every desktop
//! target, not just macOS), and gating the code itself would make it
//! unusable as a seam for a future Windows-enablement slice. What IS gated
//! is the one CALL SITE in `lib.rs` that invokes [`build`] — matching
//! `quit::app_menu`'s existing precedent — because the four shipped icons
//! are macOS `setTemplate` assets (spec D5) and D10 explicitly defers
//! Windows's own (coloured, non-template) icon set to a later slice. A
//! Windows-enablement slice needs to add Windows icons and un-gate that one
//! call site; it does not need to touch this module's logic.
//!
//! **Addendum (security audit finding H1, 2026-07-31): that checklist is
//! incomplete on its own.** `lib.rs`'s `on_window_event` hide-on-close branch
//! is ALSO `#[cfg(target_os = "macos")]`-gated now, precisely because hiding
//! the only window is only safe once something in this module (or the app
//! menu, or `RunEvent::Reopen`) can bring it back — and today none of those
//! exist off macOS. A Windows-enablement slice must un-gate that `lib.rs`
//! branch in the SAME change that adds Windows icons and un-gates this
//! module's `build` call, or Windows would grow a tray with no window to
//! show for it while the close button still just closes. Un-gating the two
//! independently would each look complete on its own and still leave the
//! app unreachable in between.

pub mod apply;
pub mod model;
// Bulk start/stop primitives and the failure-dialog's pure decision logic
// (Task 5 of the tray slice, spec D4/D6/D7).
//
// `pub(crate)`, widened from private by the P1 CLI slice: the control
// channel's `stop-all` must take the SAME two locks this module's Stop-all
// takes, through the SAME `try_acquire_bulk` — a second copy of that
// admission check is exactly how the tray and the CLI would drift into
// disagreeing about what "busy" means. Only that one function is reachable
// from outside; everything else here stays `pub(super)`.
pub(crate) mod service_control;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use apply::{TraySink, apply};
use model::{Action, BulkState, IconState, TrayModel, toggle_action, tray_model};
use openvhost_proc::{ServiceState, ServiceStatus, Supervisor, SupervisorEvent};
use tauri::image::Image;
use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{TrayIcon, TrayIconBuilder};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};

/// "Open OpenVHost" row's menu-item id (spec D2's menu layout; also the
/// action `RunEvent::Reopen`/a Dock click perform — see `lib.rs`'s
/// `reopen_window`, which this dispatches the same three calls as).
pub const OPEN_MENU_ITEM_ID: &str = "openvhost:tray:open";
/// The disabled aggregate-summary row's id. Never dispatched (it is
/// rendered `enabled: false`) — reserved so `MenuItem::with_id` has
/// something namespaced to assign rather than an auto-generated id.
pub const SUMMARY_MENU_ITEM_ID: &str = "openvhost:tray:summary";
/// "Start all" row's id. Recognized by [`build`] (so the row exists and its
/// enabled state tracks [`TrayModel::start_all_enabled`]) and dispatched by
/// [`handle_tray_menu_id`] via [`dispatch_start_all`] (spec D6/D7).
pub const START_ALL_MENU_ITEM_ID: &str = "openvhost:tray:start-all";
/// "Stop all" row's id. Dispatched via [`dispatch_stop_all`] — otherwise
/// the same status as [`START_ALL_MENU_ITEM_ID`].
pub const STOP_ALL_MENU_ITEM_ID: &str = "openvhost:tray:stop-all";

/// Index the per-service rows are inserted/removed at: Open (0), summary
/// (1), Start all (2), Stop all (3), a leading separator (4) — rows begin
/// at 5, followed by a trailing separator and Quit.
const ROWS_START_INDEX: usize = 5;

/// Guards a Start-all/Stop-all bulk action end to end (spec D7): reject a
/// SECOND bulk action while one is in flight — never queue it. `try_lock`
/// only; nothing in this app ever calls `.lock().await` on this mutex, which
/// is what makes "reject" possible at all — a blocking acquire would wait
/// instead.
///
/// Managed as its own app state (`lib.rs`), NOT a field on the real
/// [`TrayHandle`]: spec D9 forbids constructing a real tray/menu in a test,
/// so the reject-not-queue admission check ([`service_control::try_acquire_bulk`],
/// dispatched from [`handle_tray_menu_id`]) has to be reachable under
/// `tauri::test::mock_builder` WITHOUT one — see this module's own test
/// module for exactly that.
///
/// The inner mutex is `pub(crate)` (widened by the P1 CLI slice, mirroring
/// `commands::ApplyLock`'s own shape): `control.rs`'s `stop-all` needs to
/// hand it to the same [`service_control::try_acquire_bulk`] this module
/// does, and there is no meaningful invariant to protect by keeping the field
/// private — `try_lock` is the only operation anything in this app ever
/// performs on it.
#[derive(Default)]
pub(crate) struct BulkLock(pub(crate) tokio::sync::Mutex<()>);

/// The ids currently in a start attempt DISPATCHED FROM THE TRAY (spec D4):
/// a single row's `Start`/`Retry` click, or a [`dispatch_start_all`] sweep.
/// An id is [`mark`](TrayInitiated::mark)ed right before the corresponding
/// `Supervisor::start` call and [`resolve`](TrayInitiated::resolve)d the
/// moment that attempt's outcome is known (`Running`, `Stopped`, or
/// `Failed` — see [`service_control::dialog_for_transition`]), so a MUCH
/// LATER failure of a long-since-`Running` service (e.g. killed externally,
/// or crashed after running fine for an hour) is never mistaken for the
/// failure of the start attempt itself.
///
/// Bounded by construction, not by any eviction policy: the only way an id
/// enters this set is a tray dispatch, and every entry is removed on that
/// SAME service's very next `StateChanged` — an id can therefore only ever
/// be a member between being marked and the next event for it, never
/// longer, so this cannot accumulate faster than the (small, fixed) number
/// of services a machine has registered.
///
/// Same "own app state, not a `TrayHandle` field" reasoning as [`BulkLock`]
/// — see its doc comment.
#[derive(Default)]
pub(crate) struct TrayInitiated(Mutex<HashSet<String>>);

impl TrayInitiated {
    fn mark(&self, id: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string());
    }

    /// Remove `id` unconditionally (its start attempt has resolved, one way
    /// or another) and report whether it had actually been tracked — the
    /// caller uses that to decide whether a `Failed` resolution owes a
    /// dialog.
    fn resolve(&self, id: &str) -> bool {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(id)
    }
}

/// The tray icon's glyph for `state`, embedded at compile time (spec D5).
///
/// 36x36 PNGs (`tray-icon` scales to 18pt, so 36px is exactly 2x for
/// Retina), template images (mask-only — see
/// `icons/tray/generate_tray_icons.py`'s module docs for the full
/// geometry/legibility reasoning), generated by and committed alongside
/// that script rather than hand-drawn, so they are reproducible from
/// source. Deliberately NOT added to `tauri.conf.json`'s `bundle.icon`
/// (spec D2/Task 4 brief): those are the app/dock icon set, a completely
/// different asset with a completely different design (brand guidelines
/// Sec 7.2 vs 7.3).
fn icon_for_state(state: IconState) -> Image<'static> {
    match state {
        IconState::Stopped => tauri::include_image!("icons/tray/stopped.png"),
        IconState::Running => tauri::include_image!("icons/tray/running.png"),
        IconState::Starting => tauri::include_image!("icons/tray/starting.png"),
        IconState::Failed => tauri::include_image!("icons/tray/failed.png"),
    }
}

/// Route a menu-item id to its action (spec D2/D6/D9).
///
/// Despite living in `tray`, this is the ONE router for every menu event
/// the whole app receives, not only tray-originated ones: tauri's
/// `on_menu_event` fires "for any menu event, whether it is coming from
/// this window, another window or from the tray icon menu" (verified
/// against the resolved tauri 2.11.5, `tray/mod.rs`'s own doc comment on
/// `TrayIconBuilder::on_menu_event` — there is exactly one global listener
/// list, so a second, tray-specific registration would double-dispatch
/// every id, not add coverage). `lib.rs` wires its single
/// `Builder::on_menu_event` closure straight to this function, which is
/// why the app's macOS menu-bar Quit and the tray's own rows both arrive
/// here.
///
/// Takes a raw `&str` id, not a `tauri::menu::MenuEvent` (spec D9): a
/// `MenuEvent` cannot be constructed in a test (its only field is a
/// private `MenuId`), so a router shaped around one would be unreachable
/// under `mock_builder`. The caller passes `event.id().as_ref()`.
///
/// Re-reads `Supervisor::snapshot()` and derives the action from the
/// CURRENT state via [`toggle_action`] — it never trusts the label the row
/// was rendered with (spec D2's "staleness is closed by construction"):
/// the menu can be showing a state that is already stale by the time a
/// click lands, since recomputation only happens on the next event.
///
/// Dispatch to the supervisor is wrapped in [`tauri::async_runtime::spawn`]
/// rather than called inline: `on_menu_event` fires on the native main
/// thread, which is NOT a tokio worker thread, and `Supervisor::start`
/// documents that it "must be called from within a tokio runtime context"
/// (it does a bare `tokio::spawn` internally). `tauri::async_runtime::spawn`
/// explicitly `.enter()`s tauri's own runtime before spawning (verified
/// against the resolved tauri 2.11.5, `async_runtime.rs:110`), which is
/// what makes this safe from a bare native callback; calling `sup.start`
/// directly here would panic off the main thread with "there is no
/// reactor running" the first time a tray click landed outside of any
/// already-async context.
///
/// [`START_ALL_MENU_ITEM_ID`]/[`STOP_ALL_MENU_ITEM_ID`] are handled next
/// (spec D6/D7), each delegating to its own dispatch function — see
/// [`dispatch_start_all`]/[`dispatch_stop_all`]'s own doc comments for the
/// admission check (both the tray's own [`BulkLock`] and the existing
/// `ApplyLock` must be free) and the failure-tracking side effect.
/// [`SUMMARY_MENU_ITEM_ID`] and an id that matches no known service are both
/// deliberate no-ops: an unrecognized id must never panic or guess at an
/// action.
pub fn handle_tray_menu_id<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if id == crate::quit::QUIT_MENU_ITEM_ID {
        if !crate::quit::request_quit(app) {
            let handle = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = crate::quit::perform_quit(&handle).await {
                    eprintln!("openvhost: quit failed: {e}");
                }
            });
        }
        return;
    }

    // The app menu's **Install Command Line Tool…** row (P1 CLI-install
    // design D6). Same shape as the Quit row above — the id, the action and
    // the `app_menu` that builds the row all live together in `quit`, and
    // this router, being the app's ONE menu-event listener, is what reaches
    // them. Returns whether it was ours, so the dispatch decision itself is
    // testable without performing a real install; see its doc comment.
    if crate::quit::handle_install_cli_tool_id(app, id, crate::quit::install_cli_tool) {
        return;
    }

    if id == OPEN_MENU_ITEM_ID {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.show();
            let _ = window.unminimize();
            let _ = window.set_focus();
        }
        return;
    }

    if id == START_ALL_MENU_ITEM_ID {
        dispatch_start_all(app);
        return;
    }

    if id == STOP_ALL_MENU_ITEM_ID {
        dispatch_stop_all(app);
        return;
    }

    let Some(sup) = app.try_state::<Arc<Supervisor>>().map(|s| Arc::clone(&s)) else {
        return;
    };
    let Some(status) = sup.snapshot().into_iter().find(|s| s.id == id) else {
        return;
    };
    match toggle_action(&status.id, &status.state) {
        Action::Start(sid) | Action::Retry(sid) => {
            // Mark BEFORE dispatch (spec D4): `sid` must already be tracked
            // by the time `sup.start` could possibly broadcast a `Failed`
            // for it, however fast — see `TrayInitiated`'s own doc comment.
            if let Some(tracked) = app.try_state::<TrayInitiated>() {
                tracked.mark(&sid);
            }
            tauri::async_runtime::spawn(async move {
                let _ = sup.start(&sid);
            });
        }
        Action::Stop(sid) => {
            tauri::async_runtime::spawn(async move {
                let _ = sup.stop(&sid);
            });
        }
        Action::None => {}
    }
}

/// Dispatch "Start all" (spec D6/D7/D8): rejects — does nothing at all — if
/// EITHER the tray's own [`BulkLock`] or the existing
/// `crate::commands::ApplyLock` is already held (spec D7's "reject, never
/// queue"; see [`service_control::try_acquire_bulk`]'s own doc comment for
/// why the SAME mutex `apply_config` uses is part of this check).
/// Otherwise marks every id [`model::bulk_start_ids`] selects as
/// tray-initiated (spec D4) strictly BEFORE calling [`Supervisor::start`] on
/// it, so even a start that fails instantly is still caught by the failure
/// dialog.
///
/// Forces a menu recompute the INSTANT the locks are acquired (the fresh
/// probe inside that recompute reports `Busy`, since the locks are still
/// held at that point) and again the instant they are released (now
/// reporting `Idle`), rather than waiting for the next unrelated
/// `StateChanged` to happen to recompute anyway — see
/// [`refresh_tray_if_built`]'s own doc comment for why that matters.
///
/// Requires `Arc<Supervisor>`/[`BulkLock`]/`ApplyLock`/[`TrayInitiated`] all
/// to be managed; missing ANY of them is a silent no-op (mirrors every other
/// `try_state` read in this app) rather than a partial action — there is no
/// safe partial version of "start some services but track none of them".
fn dispatch_start_all<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(sup) = app.try_state::<Arc<Supervisor>>().map(|s| Arc::clone(&s)) else {
            return;
        };
        let Some(bulk_lock) = app.try_state::<BulkLock>() else {
            return;
        };
        let Some(apply_lock) = app.try_state::<crate::commands::ApplyLock>() else {
            return;
        };
        let Some(tracked) = app.try_state::<TrayInitiated>() else {
            return;
        };

        let bulk_mutex = &bulk_lock.inner().0;
        let apply_mutex = &apply_lock.inner().0;
        let Some(_guards) = service_control::try_acquire_bulk(bulk_mutex, apply_mutex) else {
            return;
        };

        let snapshot = sup.snapshot();
        refresh_tray_if_built(&app, snapshot.clone());
        service_control::start_all_with(&snapshot, |id| {
            tracked.mark(id);
            let _ = sup.start(id);
        });

        drop(_guards);
        refresh_tray_if_built(&app, sup.snapshot());
    });
}

/// Dispatch "Stop all" (spec D6/D7): the same reject-not-queue admission
/// check as [`dispatch_start_all`], then delegates the actual stop to
/// [`crate::quit::stop_all`] — the LITERAL function [`crate::quit::perform_quit`]
/// uses, not a copy (spec D6: forking it is how Quit and tray Stop-all
/// would silently drift apart), including its 18s budget already covering
/// MySQL's own 15s shutdown grace.
///
/// Does not touch [`TrayInitiated`] — stopping is never itself a "start
/// failed" event. If a service the tray started is still `Starting` when
/// this runs (spec D7: "Stop-all during MySQL's 15s grace needs no
/// change") and its stop resolves to `Failed` rather than `Stopped` (a
/// real, if rare, classification — see `openvhost_proc`'s
/// `service_task::finish_never_ready`), the EXISTING event-subscriber loop
/// in [`build`] still raises the dialog for it via
/// [`maybe_show_failure_dialog`]; nothing here needs to special-case that.
fn dispatch_stop_all<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let Some(sup) = app.try_state::<Arc<Supervisor>>().map(|s| Arc::clone(&s)) else {
            return;
        };
        let Some(bulk_lock) = app.try_state::<BulkLock>() else {
            return;
        };
        let Some(apply_lock) = app.try_state::<crate::commands::ApplyLock>() else {
            return;
        };

        let bulk_mutex = &bulk_lock.inner().0;
        let apply_mutex = &apply_lock.inner().0;
        let Some(_guards) = service_control::try_acquire_bulk(bulk_mutex, apply_mutex) else {
            return;
        };

        refresh_tray_if_built(&app, sup.snapshot());
        let stragglers = crate::quit::stop_all(Arc::clone(&sup)).await;
        if !stragglers.is_empty() {
            eprintln!(
                "openvhost: tray Stop all left {} service(s) still not stopped: {}",
                stragglers.len(),
                stragglers.join(", ")
            );
        }

        drop(_guards);
        refresh_tray_if_built(&app, sup.snapshot());
    });
}

/// Recompute a [`TrayModel`] from `snapshot`/a freshly-probed [`BulkState`],
/// diff it against the last-applied model, and apply the result — all
/// inside ONE `run_on_main_thread` dispatch (see [`build`]'s own doc
/// comment, "critical mechanic"). A no-op, not a panic, when no real tray
/// was built (non-macOS, spec D10 — or `build` itself failed and logged;
/// see `lib.rs`'s `#[cfg(target_os = "macos")]` call site) — mirrors every
/// other `try_state` read in this app.
///
/// Shared by the live event-subscriber loop in [`build`] and the bulk-action
/// dispatch functions above (spec D7): a bulk action must show
/// `Busy`/`Idle` the INSTANT it acquires/releases [`BulkLock`], not only
/// whenever the next unrelated `StateChanged` happens to recompute anyway —
/// MySQL's own stop sequence, for one, emits no intermediate event across
/// its whole 15s grace, so without this a Stop-all click would leave the
/// menu looking idle-and-clickable for up to 15s before anything visibly
/// changed.
///
/// **`BulkState` is probed INSIDE the `run_on_main_thread` closure — at
/// APPLY time, not at enqueue time.** It used to be probed by the caller and
/// passed in, which left an enqueue-order race: a bulk-dispatch function
/// (releasing its guards, probing `Idle`, then enqueueing) and this same
/// event-subscriber loop (reacting to the state change the release itself
/// produced, probing — typically `Busy`, since the release may not have
/// happened yet — then enqueueing) can each enqueue their own closure around
/// the same instant, and `run_on_main_thread`'s delivery order is not
/// guaranteed to match either caller's probe-then-enqueue order. Whichever
/// closure happened to carry the STALER pre-probed value could therefore
/// apply LAST — and because a Stop-all's own final event is often the last
/// event there is (nothing else necessarily follows to trigger a healing
/// recompute), a stale `Busy` applying last could wedge both bulk rows
/// disabled with nothing actually in flight. Probing fresh from INSIDE the
/// closure means whichever one genuinely runs last reads the CURRENT lock
/// state at that moment, so enqueue order stops mattering.
///
/// The `snapshot` parameter is NOT given the same treatment, deliberately:
/// it is still captured once at the caller's enqueue time, not re-read
/// inside the closure. That is fine as-is — an out-of-date `snapshot` only
/// means the applied model is briefly behind current reality, and any
/// service state that changed in the meantime is, by definition, a state
/// change the supervisor separately broadcasts, which drives its own later
/// call to this same function with a fresh snapshot that heals the
/// discrepancy. `BulkState` has no equivalent follow-up event once a bulk
/// action's own two calls (acquire, then release) are both enqueued — which
/// is exactly why it, unlike `snapshot`, needed the fix above. Do not
/// "fix" this half too.
fn refresh_tray_if_built<R: Runtime>(app: &AppHandle<R>, snapshot: Vec<ServiceStatus>) {
    let Some(tray) = app
        .try_state::<Arc<TrayHandle<R>>>()
        .map(|t| Arc::clone(&t))
    else {
        return;
    };
    let app_for_probe = app.clone();
    let _ = app.run_on_main_thread(move || {
        let bulk = probe_bulk_state(&app_for_probe);
        let new_model = tray_model(&snapshot, bulk);
        let mut last = tray.last_model.lock().unwrap_or_else(|e| e.into_inner());
        apply(&last, &new_model, tray.as_ref());
        *last = new_model;
    });
}

/// Whether a bulk action currently holds [`BulkLock`] — a non-blocking
/// PROBE (`try_lock`, immediately dropped on success), never an actual
/// acquisition: this is ONLY for deciding what the menu should currently
/// render. The real admission check bulk actions gate on is
/// [`service_control::try_acquire_bulk`], called separately by
/// [`dispatch_start_all`]/[`dispatch_stop_all`] — this function has no
/// bearing on it either way.
///
/// `Idle` when [`BulkLock`] is not even managed — mirrors every other
/// `try_state` absent-state fallback in this app; there is nothing that
/// could be holding a lock that was never created.
///
/// Called from exactly one place: INSIDE [`refresh_tray_if_built`]'s
/// `run_on_main_thread` closure, never by that function's own callers — see
/// its doc comment for why probing at apply time rather than enqueue time
/// matters.
fn probe_bulk_state<R: Runtime>(app: &AppHandle<R>) -> BulkState {
    match app.try_state::<BulkLock>() {
        Some(lock) if lock.inner().0.try_lock().is_ok() => BulkState::Idle,
        Some(_) => BulkState::Busy,
        None => BulkState::Idle,
    }
}

/// If `state` is a `Failed` transition for an id [`TrayInitiated`] is
/// tracking (spec D4 — dispatched from THIS tray, never e.g. the Services
/// page's own `start_service` command), show the native error dialog with
/// the exit status and `stderr_tail` VERBATIM; any other transition just
/// resolves the tracking without a dialog — see
/// [`service_control::dialog_for_transition`]'s own doc comment for exactly
/// which transitions count.
fn maybe_show_failure_dialog<R: Runtime>(
    app: &AppHandle<R>,
    supervisor: &Supervisor,
    id: &str,
    state: &ServiceState,
) {
    maybe_show_failure_dialog_with(app, supervisor, id, state, show_failure_dialog);
}

/// [`maybe_show_failure_dialog`], parameterized over the actual
/// dialog-showing call so the DECISION (fetch [`TrayInitiated`], look up a
/// display name, defer to [`service_control::dialog_for_transition`]) is
/// testable under `tauri::test::mock_builder` with a recording closure
/// instead of a real `rfd` alert (spec D9 rules out the latter, not the
/// former) — same closure-injection reasoning as `quit::stop_all`/
/// `stop_all_with`.
///
/// No-op, not a panic, when [`TrayInitiated`] is not managed (mirrors every
/// other `try_state` read in this app) or `id` no longer names a registered
/// service (defensive; nothing in this app unregisters one today — falls
/// back to `id` itself as the display name rather than skipping the dialog
/// outright).
fn maybe_show_failure_dialog_with<R: Runtime>(
    app: &AppHandle<R>,
    supervisor: &Supervisor,
    id: &str,
    state: &ServiceState,
    show: impl FnOnce(&AppHandle<R>, String, String),
) {
    let Some(tracked) = app.try_state::<TrayInitiated>() else {
        return;
    };
    let display_name = supervisor
        .snapshot()
        .into_iter()
        .find(|s| s.id == id)
        .map(|s| s.display_name)
        .unwrap_or_else(|| id.to_string());
    let Some((title, body)) =
        service_control::dialog_for_transition(&tracked, id, &display_name, state)
    else {
        return;
    };
    show(app, title, body);
}

/// The actual native dialog call (spec D4) — the one piece of this feature
/// spec D9 rules out testing directly (no `NSAlert` in a test process).
/// Kept as small and untested-by-necessity as possible: every DECISION that
/// feeds it (whether to show one at all, and its exact text) is
/// [`maybe_show_failure_dialog_with`] and
/// [`service_control::dialog_for_transition`]/[`service_control::failure_dialog_text`],
/// all of which have full test coverage without this function ever running.
///
/// Calling the Rust `tauri_plugin_dialog::DialogExt` API directly, rather
/// than a new Tauri command the frontend would invoke, means this never
/// touches the IPC/ACL layer at all (spec D6's "zero new Tauri commands")
/// — the same reasoning that already applies to every other tray action in
/// this module.
///
/// *Open OpenVHost* shows, un-minimizes, and focuses the main window — the
/// SAME three calls [`OPEN_MENU_ITEM_ID`]'s own handler performs, so the
/// button and the tray's "Open OpenVHost" row are indistinguishable in
/// effect. *Dismiss* does nothing further: the tray icon and the row
/// already stayed in the `Failed` state on their own (spec D4 — the dialog
/// is transient, the tray state is durable), so there is nothing left for
/// this button to do. Deliberately does NOT show/focus the window on a bare
/// dismiss, and does NOT show it before the dialog appears either — spec
/// D4, "auto-opening the window is rejected": the dialog already makes the
/// failure unmissable, so raising the window on top of it would only be
/// focus-stealing.
///
/// `.show(..)`, never `.blocking_show(..)`: the latter's own documentation
/// says it must not run on the main thread, and blocking a tokio worker
/// thread (this runs from inside the event-subscriber task) until the user
/// gets around to dismissing a dialog would starve that worker of every
/// other pending task for as long as the dialog stays open.
fn show_failure_dialog<R: Runtime>(app: &AppHandle<R>, title: String, body: String) {
    let app_for_open = app.clone();
    app.dialog()
        .message(body)
        .title(title)
        .kind(MessageDialogKind::Error)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Open OpenVHost".to_string(),
            "Dismiss".to_string(),
        ))
        .show(move |opened| {
            if opened && let Some(window) = app_for_open.get_webview_window("main") {
                let _ = window.show();
                let _ = window.unminimize();
                let _ = window.set_focus();
            }
        });
}

/// The real [`TraySink`]: holds every native handle a diff might need to
/// mutate, plus the last-applied model to diff the next snapshot against.
struct TrayHandle<R: Runtime> {
    tray_icon: TrayIcon<R>,
    summary_item: MenuItem<R>,
    start_all_item: MenuItem<R>,
    stop_all_item: MenuItem<R>,
    menu: Menu<R>,
    /// Current per-service rows, in display order, alongside their
    /// `MenuItem` handles — the thing [`TraySink::rebuild`] tears down and
    /// replaces on a membership change.
    rows: Mutex<Vec<(String, MenuItem<R>)>>,
    /// The model [`apply`] last diffed against, updated after every apply
    /// call. Read/written only from inside the single
    /// `run_on_main_thread` closure in [`build`]'s subscriber task, so a
    /// plain `Mutex` (not swapped concurrently) is enough.
    last_model: Mutex<TrayModel>,
}

impl<R: Runtime> TraySink for TrayHandle<R> {
    fn set_summary(&self, text: &str) {
        if let Err(e) = self.summary_item.set_text(text) {
            eprintln!("openvhost: failed to update the tray summary: {e}");
        }
    }

    fn set_icon(&self, state: IconState) {
        if let Err(e) = self
            .tray_icon
            .set_icon_with_as_template(Some(icon_for_state(state)), true)
        {
            eprintln!("openvhost: failed to update the tray icon: {e}");
        }
    }

    fn set_start_all_enabled(&self, enabled: bool) {
        if let Err(e) = self.start_all_item.set_enabled(enabled) {
            eprintln!("openvhost: failed to update Start all: {e}");
        }
    }

    fn set_stop_all_enabled(&self, enabled: bool) {
        if let Err(e) = self.stop_all_item.set_enabled(enabled) {
            eprintln!("openvhost: failed to update Stop all: {e}");
        }
    }

    fn set_row_label(&self, id: &str, label: &str) {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, item)) = rows.iter().find(|(row_id, _)| row_id == id)
            && let Err(e) = item.set_text(label)
        {
            eprintln!("openvhost: failed to update tray row {id}: {e}");
        }
    }

    fn set_row_enabled(&self, id: &str, enabled: bool) {
        let rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((_, item)) = rows.iter().find(|(row_id, _)| row_id == id)
            && let Err(e) = item.set_enabled(enabled)
        {
            eprintln!("openvhost: failed to enable/disable tray row {id}: {e}");
        }
    }

    /// Tear down every current per-service `MenuItem` and rebuild the row
    /// section from `model.rows`, THEN resync summary/icon/bulk enablement
    /// too. The trailing resync is deliberate, not an afterthought:
    /// [`apply`] calls `rebuild` INSTEAD of the granular per-field diffs
    /// when membership changes (see its doc comment), so if this method
    /// only touched rows, a membership change that ALSO flips the
    /// aggregate icon (almost always true — a newly `Registered` service
    /// is `Stopped`, which can only raise or hold the aggregate, never by
    /// itself lower it before the caller's next diff) would leave the old
    /// icon/summary stale until some LATER event happened to change them
    /// again.
    fn rebuild(&self, model: &TrayModel) {
        let app = self.menu.app_handle().clone();
        let mut rows = self.rows.lock().unwrap_or_else(|e| e.into_inner());
        for (_, item) in rows.drain(..) {
            if let Err(e) = self.menu.remove(&item) {
                eprintln!("openvhost: failed to remove a stale tray row: {e}");
            }
        }

        // `(id, item)` pairs built in the SAME loop/push as each other,
        // deliberately, rather than collecting ids and items into two
        // parallel `Vec`s and `zip`-ing them back together afterwards: if
        // one `MenuItem::with_id` call in the middle fails and is skipped,
        // a separately-collected id list stays the full length of
        // `model.rows` while the item list comes up one short, so zipping
        // them shifts every id/item pairing AFTER the failure by one
        // position — a later row's label/enabled updates would then
        // silently land on an earlier row's `MenuItem`. Pushing the pair
        // together means a skipped item skips its own id too, so nothing
        // downstream ever shifts.
        let mut new_items: Vec<(String, MenuItem<R>)> = Vec::with_capacity(model.rows.len());
        for row in &model.rows {
            match MenuItem::with_id(&app, row.id.clone(), &row.label, row.enabled, None::<&str>) {
                Ok(item) => new_items.push((row.id.clone(), item)),
                Err(e) => eprintln!("openvhost: failed to build tray row {}: {e}", row.id),
            }
        }
        let refs: Vec<&dyn IsMenuItem<R>> = new_items
            .iter()
            .map(|(_, item)| item as &dyn IsMenuItem<R>)
            .collect();
        if let Err(e) = self.menu.insert_items(&refs, ROWS_START_INDEX) {
            eprintln!("openvhost: failed to insert refreshed tray rows: {e}");
        }

        *rows = new_items;
        drop(rows);

        self.set_summary(&model.summary);
        self.set_icon(model.icon);
        self.set_start_all_enabled(model.start_all_enabled);
        self.set_stop_all_enabled(model.stop_all_enabled);
    }
}

/// Build the tray: the initial menu from `supervisor.snapshot()`, and a
/// background task that keeps it live (spec D2).
///
/// On every `StateChanged`/`Registered` event (and, conservatively, on a
/// `Lagged` notification — see the inline comment at the subscriber loop),
/// recomputes a fresh [`TrayModel`] and applies it. Never applies event
/// deltas: the model is always rebuilt from a fresh `snapshot()`, which is
/// what makes this immune to the broadcast channel's `Lagged` arm (spec
/// D2) — a dropped batch of events just means the next recompute catches
/// up to current reality, rather than needing to replay what was missed.
/// [`BulkState`] is likewise re-PROBED (`probe_bulk_state`) on every
/// recompute rather than cached, and every `StateChanged` is separately
/// checked against [`TrayInitiated`] for a spec D4 failure dialog
/// ([`maybe_show_failure_dialog`]) before the recompute even runs.
///
/// **Critical mechanic:** `MenuItem::set_text`/`set_enabled` (and
/// `Menu::insert`/`remove`) are each their own blocking main-thread
/// round-trip (verified against the resolved tauri 2.11.5,
/// `menu/mod.rs`'s `run_item_main_thread!` macro: a channel send, a
/// `run_on_main_thread` dispatch, and a blocking `recv` on the calling
/// thread). Called one-by-one from this event task, N changed fields would
/// mean N such round-trips, each one blocking a tokio worker for as long
/// as the main thread takes to service it — which can be a while if the
/// user is holding the menu open (a native menu runs its own nested run
/// loop). Wrapping the ENTIRE diff-and-apply in one
/// `AppHandle::run_on_main_thread` closure collapses that to a single
/// dispatch: once code is executing INSIDE that closure, it is genuinely
/// on the main thread, so every nested `MenuItem`/`Menu` call's own
/// internal `run_on_main_thread` detects "already on this thread" and
/// runs inline instead of posting-and-waiting (verified against the
/// resolved tauri-runtime-wry 2.11.4, `send_user_message`'s
/// `current_thread().id() == context.main_thread_id` check) — so batching
/// is not just fewer trips in theory, it is genuinely zero EXTRA
/// cross-thread trips beyond the one this function issues itself.
///
/// Rebuilds the row section only when the SET of service ids changes
/// (spec D2) — see [`apply`]'s own doc comment for the exact rule; this
/// function only has to call it.
pub fn build<R: Runtime>(app: &AppHandle<R>, supervisor: Arc<Supervisor>) -> tauri::Result<()> {
    let initial = tray_model(&supervisor.snapshot(), BulkState::Idle);

    let open_item =
        MenuItem::with_id(app, OPEN_MENU_ITEM_ID, "Open OpenVHost", true, None::<&str>)?;
    let summary_item = MenuItem::with_id(
        app,
        SUMMARY_MENU_ITEM_ID,
        &initial.summary,
        false,
        None::<&str>,
    )?;
    let start_all_item = MenuItem::with_id(
        app,
        START_ALL_MENU_ITEM_ID,
        "Start all",
        initial.start_all_enabled,
        None::<&str>,
    )?;
    let stop_all_item = MenuItem::with_id(
        app,
        STOP_ALL_MENU_ITEM_ID,
        "Stop all",
        initial.stop_all_enabled,
        None::<&str>,
    )?;
    let leading_separator = PredefinedMenuItem::separator(app)?;
    let trailing_separator = PredefinedMenuItem::separator(app)?;
    let quit_item = MenuItem::with_id(
        app,
        crate::quit::QUIT_MENU_ITEM_ID,
        "Quit OpenVHost",
        true,
        None::<&str>,
    )?;

    let mut row_items: Vec<MenuItem<R>> = Vec::with_capacity(initial.rows.len());
    for row in &initial.rows {
        row_items.push(MenuItem::with_id(
            app,
            row.id.clone(),
            &row.label,
            row.enabled,
            None::<&str>,
        )?);
    }

    let menu = Menu::new(app)?;
    menu.append(&open_item)?;
    menu.append(&summary_item)?;
    menu.append(&start_all_item)?;
    menu.append(&stop_all_item)?;
    menu.append(&leading_separator)?;
    for item in &row_items {
        menu.append(item)?;
    }
    menu.append(&trailing_separator)?;
    menu.append(&quit_item)?;

    let tray_icon = TrayIconBuilder::new()
        .icon(icon_for_state(initial.icon))
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .tooltip("OpenVHost")
        .build(app)?;

    let rows: Vec<(String, MenuItem<R>)> = initial
        .rows
        .iter()
        .map(|r| r.id.clone())
        .zip(row_items)
        .collect();

    let handle = Arc::new(TrayHandle {
        tray_icon,
        summary_item,
        start_all_item,
        stop_all_item,
        menu,
        rows: Mutex::new(rows),
        last_model: Mutex::new(initial),
    });
    // The SOLE strong reference kept beyond this function: both the
    // subscriber loop below and the bulk-action dispatch functions
    // (`dispatch_start_all`/`dispatch_stop_all` in this module) reach the
    // tray only via `refresh_tray_if_built`'s own `app.try_state::<Arc<TrayHandle<R>>>()`
    // lookup, never a directly captured clone — so managed state is what
    // keeps this alive for the app's whole lifetime, not a loop closure.
    app.manage(Arc::clone(&handle));

    let mut rx = supervisor.subscribe();
    let app_for_task = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            let refresh = match rx.recv().await {
                Ok(SupervisorEvent::StateChanged { id, state, .. }) => {
                    // Spec D4: check this BEFORE the (possibly skipped, see
                    // below) menu recompute — a failure dialog is owed
                    // regardless of whether the aggregate model actually
                    // changed shape.
                    maybe_show_failure_dialog(&app_for_task, &supervisor, &id, &state);
                    true
                }
                Ok(SupervisorEvent::Registered { .. }) => true,
                // A row DISAPPEARED (package-uninstall design D4). Same
                // answer as `Registered` for the same reason, and it costs
                // nothing extra here: the recompute below re-reads
                // `snapshot()` wholesale, so a shrunken service set flows
                // through `tray_model` → `apply` exactly like a grown one,
                // and `apply`'s membership check (which compares the id set
                // BOTH ways) turns it into a full `rebuild` — the only
                // correct move, since the vanished row's `MenuItem` handle
                // has to be removed from the native menu, not mutated.
                Ok(SupervisorEvent::Unregistered { .. }) => true,
                // A log line never changes a ServiceStatus's id/display_name/
                // endpoint/pid/state — the only fields `tray_model` reads —
                // so recomputing here would be a guaranteed-no-op diff at
                // whatever rate the busiest service logs. This is NOT
                // "applying an event delta" (spec D2's actual rule): the
                // recompute below always re-reads a fresh `snapshot()`
                // regardless of which event triggered it; skipping Log is
                // only a decision about WHEN to bother, never about what
                // to apply.
                Ok(SupervisorEvent::Log { .. }) => false,
                // Unlike a plain skip, a lagged receiver may have missed a
                // StateChanged/Registered entirely — recompute defensively
                // rather than risk staying stale until the next real event.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            };
            if !refresh {
                continue;
            }
            let snapshot = supervisor.snapshot();
            refresh_tray_if_built(&app_for_task, snapshot);
        }
    });

    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use openvhost_proc::{
        DEFAULT_GRACE, ReadinessProbe, ServiceSpec, ServiceState, SpawnSpec, default_driver,
    };
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn app_with_supervisor() -> (tauri::App<tauri::test::MockRuntime>, Arc<Supervisor>) {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let sup = Arc::new(Supervisor::new(default_driver()));
        app.manage(Arc::clone(&sup));
        (app, sup)
    }

    /// A real, harmless, long-lived child (mirrors `quit.rs`'s own
    /// `#[cfg(unix)]` precedent for exercising a REAL `Supervisor` rather
    /// than a spec pointing at a nonexistent binary): sleeps far longer
    /// than this test's deadline, so it is still alive for the STOP half
    /// of the test to act on.
    #[cfg(unix)]
    fn sleepy_spec(id: &str) -> ServiceSpec {
        ServiceSpec {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: None,
            spawn: SpawnSpec {
                program: PathBuf::from("/bin/sh"),
                args: vec![OsString::from("-c"), OsString::from("exec sleep 30")],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        }
    }

    /// A [`ServiceSpec`] that is never actually spawned — the bulk-dispatch
    /// rejection tests below only `register` it (so `sup.snapshot()` has a
    /// real row to leave untouched) and never `start` it, so unlike
    /// `sleepy_spec` above this needs no real shell and is not
    /// `#[cfg(unix)]`-gated.
    fn registered_only_spec(id: &str) -> ServiceSpec {
        ServiceSpec {
            id: id.to_string(),
            display_name: id.to_string(),
            endpoint: Some(format!("endpoint-{id}")),
            spawn: SpawnSpec {
                program: PathBuf::from("/does/not/exist"),
                args: vec![],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: DEFAULT_GRACE,
        }
    }

    /// Polls `condition` until it is true or `deadline` elapses, sleeping
    /// briefly between attempts. Panics with `msg` on timeout — mirrors the
    /// polling style already established in `quit.rs`'s own tests, adapted
    /// to a plain synchronous `#[test]` (no `#[tokio::test]` needed here:
    /// `handle_tray_menu_id`'s dispatch runs on tauri's OWN internal
    /// runtime via `tauri::async_runtime::spawn`, not on whatever runtime —
    /// if any — the calling test happens to run under).
    fn wait_until(mut condition: impl FnMut() -> bool, deadline: Duration, msg: &str) {
        let start = Instant::now();
        while !condition() {
            if start.elapsed() >= deadline {
                panic!("{msg}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // -----------------------------------------------------------------
    // handle_tray_menu_id: unknown id is a no-op.
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily replaced the
    /// exact-id lookup (`.find(|s| s.id == id)`) with `.next()` — i.e. "the
    /// router matches WHATEVER service happens to be first, not
    /// necessarily the one that was clicked". This test failed: the
    /// reserved `START_ALL_MENU_ITEM_ID`/`STOP_ALL_MENU_ITEM_ID`/
    /// `SUMMARY_MENU_ITEM_ID` ids (and the bogus `"does-not-exist"`) all
    /// incorrectly matched the one registered service (`nginx`, the only
    /// row) and started it — `sup.snapshot()` after no longer equaled
    /// `before`. Restoring the exact-id `find` made it pass again.
    #[test]
    fn unknown_id_changes_nothing_and_does_not_panic() {
        let (app, sup) = app_with_supervisor();
        sup.register(sleepy_spec("nginx"));
        let before = sup.snapshot();

        handle_tray_menu_id(app.handle(), "does-not-exist");
        handle_tray_menu_id(app.handle(), START_ALL_MENU_ITEM_ID);
        handle_tray_menu_id(app.handle(), STOP_ALL_MENU_ITEM_ID);
        handle_tray_menu_id(app.handle(), SUMMARY_MENU_ITEM_ID);

        // Give a wrongly-dispatching implementation a real chance to have
        // acted before asserting nothing changed.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(sup.snapshot(), before);
    }

    /// No managed `Supervisor` at all must also be a no-op, not a panic —
    /// mirrors `quit.rs`'s own "nothing managed yet" posture for every
    /// other `try_state` read in this app.
    #[test]
    fn unknown_id_with_no_managed_supervisor_does_not_panic() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        handle_tray_menu_id(app.handle(), "nginx");
    }

    // -----------------------------------------------------------------
    // handle_tray_menu_id: per-service start/stop dispatch, under a real
    // managed Supervisor with a real spawned child.
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily replaced the
    /// `toggle_action` match with an unconditional `sup.start(&sid)` (i.e.
    /// dispatch Start regardless of the service's current state). The
    /// START half of this test still passed against that neuter, but the
    /// STOP half failed: the service never left `Running` because a
    /// second `start` is a documented no-op on an already-`Starting`/
    /// `Running` service (`Supervisor::start`'s own doc comment), so
    /// `wait_until` timed out waiting for `Stopped`. This is exactly why
    /// the test drives BOTH halves through the same call site rather than
    /// asserting Start and Stop separately — a stub that special-cased
    /// "first call" would still have passed a Start-only test. Restoring
    /// the real `toggle_action` match made both halves pass again.
    #[cfg(unix)]
    #[test]
    fn dispatches_start_then_stop_through_the_router() {
        let (app, sup) = app_with_supervisor();
        sup.register(sleepy_spec("nginx"));
        assert_eq!(
            sup.snapshot()[0].state,
            ServiceState::Stopped,
            "precondition: freshly registered services start Stopped"
        );

        // toggle_action(Stopped) => Start — the router must bring it up.
        handle_tray_menu_id(app.handle(), "nginx");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "router did not start nginx (never reached Running)",
        );

        // toggle_action(Running) => Stop — the SAME router call, now with
        // the service in a different state, must stop it instead. Nothing
        // about the call site changed; only `Supervisor::snapshot()`'s
        // answer did — this is the "never trusts the rendered label"
        // property the whole router is built around.
        handle_tray_menu_id(app.handle(), "nginx");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Stopped)
            },
            Duration::from_secs(5),
            "router did not stop nginx (never reached Stopped)",
        );
    }

    /// `toggle_action` answers `Action::None` for a `Starting` row (spec
    /// D3) — the router must honour that and NOT call `stop` on a service
    /// still coming up. Exercised by racing a click immediately after
    /// `start`, before the 500ms `AliveAfter` readiness window elapses.
    ///
    /// `#[tokio::test]`, unlike this module's other router tests: this one
    /// calls `Supervisor::start` DIRECTLY (as test setup, to reach the
    /// `Starting` precondition deterministically — see the comment below)
    /// rather than only through `handle_tray_menu_id`, and
    /// `Supervisor::start` documents that it "must be called from within a
    /// tokio runtime context" (a bare internal `tokio::spawn`). The
    /// router's OWN dispatch is separately wrapped in
    /// `tauri::async_runtime::spawn` (see `handle_tray_menu_id`'s doc
    /// comment) precisely so it does not share this requirement with its
    /// caller — but a direct test setup call is not the router, and does.
    ///
    /// `flavor = "multi_thread"`, not the default single-threaded flavor:
    /// the spawned service task (which must independently progress
    /// `Starting` -> `Running` on ITS OWN 500ms timer) and this test's own
    /// `wait_until` polling loop (a blocking `std::thread::sleep`, not an
    /// `.await`) both live on this same runtime once `sup.start` is called
    /// from inside it. On a single worker thread the poll loop never
    /// yields the thread back to the scheduler, so the service task would
    /// starve and the test would time out regardless of whether the
    /// router behaved correctly — a multi-threaded runtime gives the two
    /// tasks separate OS threads so they do not compete.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_starting_service_is_not_stopped_by_the_router() {
        let (app, sup) = app_with_supervisor();
        sup.register(sleepy_spec("nginx"));
        sup.start("nginx").expect("start failed");
        assert_eq!(
            sup.snapshot()[0].state,
            ServiceState::Starting,
            "precondition: still inside the AliveAfter(500ms) window"
        );

        handle_tray_menu_id(app.handle(), "nginx");

        // Give a wrongly-dispatching implementation time to act, then
        // assert the service reached Running on its OWN (the readiness
        // window elapsing), not because it was stopped-and-restarted.
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "nginx never reached Running on its own",
        );
    }

    // -----------------------------------------------------------------
    // handle_tray_menu_id: the quit id still delegates correctly (a
    // regression net for moving this logic out of lib.rs's on_menu_event
    // closure and into this shared router).
    // -----------------------------------------------------------------

    #[test]
    fn quit_id_falls_through_to_perform_quit_when_ui_never_acked() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        // No `UiReady` managed — `request_quit` reports "cannot ask", so
        // this must take the `perform_quit` fallback branch rather than
        // silently doing nothing. `perform_quit` itself will fail (no
        // "main" window in this mock context) and log to stderr; the
        // claim under test is that it was REACHED at all and did not
        // panic, exactly like `perform_quit`'s own direct callers in
        // `lib.rs`.
        handle_tray_menu_id(app.handle(), crate::quit::QUIT_MENU_ITEM_ID);
        // The dispatch is asynchronous (spawned) — briefly yield so a
        // panicking implementation would have had a chance to surface
        // before this test process exits.
        std::thread::sleep(Duration::from_millis(50));
    }

    // -----------------------------------------------------------------
    // handle_tray_menu_id: "Open OpenVHost" is best-effort with no window.
    // -----------------------------------------------------------------

    #[test]
    fn open_id_does_not_panic_when_no_main_window_exists() {
        let (app, _sup) = app_with_supervisor();
        // `mock_context(noop_assets())` creates no "main" window — this
        // must be a silent best-effort no-op, not a panic, mirroring
        // `request_quit`'s own "no window" handling.
        handle_tray_menu_id(app.handle(), OPEN_MENU_ITEM_ID);
    }

    // -----------------------------------------------------------------
    // handle_tray_menu_id: Start all / Stop all (spec D6/D7) — happy path.
    // -----------------------------------------------------------------

    /// Two `Stopped` services, both with NO endpoint (so `bulk_start_ids`'s
    /// one-per-endpoint rule never excludes either — see `model.rs`'s own
    /// `two_services_with_no_endpoint_do_not_collide_with_each_other`):
    /// Start-all through the router must bring BOTH to `Running`.
    ///
    /// VACUITY (neuter-and-watch-it-fail): temporarily made
    /// `dispatch_start_all` call `sup.start` on every registered id directly
    /// (skipping `service_control::start_all_with`/`bulk_start_ids`
    /// entirely) — this test still passed (nothing here distinguishes that
    /// from the real selection with only terminal, endpoint-distinct rows),
    /// so it does NOT by itself prove the real selection rule is wired in —
    /// that proof lives in `service_control.rs`'s own
    /// `starts_only_the_ids_bulk_start_ids_selects_in_order`. This test's
    /// actual job, confirmed separately, is that clicking Start-all reaches
    /// a REAL `Supervisor::start` through the REAL managed `BulkLock`/
    /// `ApplyLock`/`TrayInitiated` chain at all — reverting
    /// `service_control::start_all_with`'s own `bulk_start_ids` call to
    /// `Vec::new()` (start nothing) DOES fail this test (both services stay
    /// `Stopped`), which is the failure mode this test exists to catch.
    #[cfg(unix)]
    #[test]
    fn start_all_through_the_router_starts_every_stopped_service() {
        let (app, sup) = app_with_supervisor();
        app.manage(BulkLock::default());
        app.manage(crate::commands::ApplyLock::default());
        app.manage(TrayInitiated::default());
        sup.register(sleepy_spec("nginx"));
        sup.register(sleepy_spec("php-fpm-8.4"));

        handle_tray_menu_id(app.handle(), START_ALL_MENU_ITEM_ID);

        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .all(|s| s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "start-all did not bring every service to Running",
        );
    }

    /// Mirrors the Start-all test above for Stop all: a `Running` service
    /// must reach `Stopped`. `#[tokio::test(flavor = "multi_thread", ...)]`
    /// for the SAME reason as `a_starting_service_is_not_stopped_by_the_router`
    /// above — the setup's own direct `sup.start` call needs an ambient
    /// tokio runtime, and the service task's readiness timer plus this
    /// test's blocking `wait_until` polls both need to make progress on the
    /// same runtime without starving each other.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_all_through_the_router_stops_a_running_service() {
        let (app, sup) = app_with_supervisor();
        app.manage(BulkLock::default());
        app.manage(crate::commands::ApplyLock::default());
        app.manage(TrayInitiated::default());
        sup.register(sleepy_spec("nginx"));
        sup.start("nginx").expect("start failed");
        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Running)
            },
            Duration::from_secs(5),
            "setup: nginx never reached Running",
        );

        handle_tray_menu_id(app.handle(), STOP_ALL_MENU_ITEM_ID);

        wait_until(
            || {
                sup.snapshot()
                    .iter()
                    .any(|s| s.id == "nginx" && s.state == ServiceState::Stopped)
            },
            Duration::from_secs(5),
            "stop-all did not bring nginx to Stopped",
        );
    }

    // -----------------------------------------------------------------
    // handle_tray_menu_id: Start all / Stop all — reject, never queue
    // (spec D7), proven against the REAL managed BulkLock/ApplyLock (the
    // pure admission-check primitive itself is proven independently in
    // `service_control.rs`'s own `try_acquire_bulk` tests).
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily replaced
    /// `dispatch_start_all`'s `service_control::try_acquire_bulk(...)` call
    /// with an unconditional `Some((..))` stand-in (i.e. never actually
    /// checked either lock) — this test failed, `nginx` reached `Running`
    /// even with `BulkLock` pre-held. Restoring the real acquisition check
    /// made it pass again.
    #[test]
    fn start_all_through_the_router_is_rejected_while_the_bulk_lock_is_already_held() {
        let (app, sup) = app_with_supervisor();
        app.manage(BulkLock::default());
        app.manage(crate::commands::ApplyLock::default());
        app.manage(TrayInitiated::default());
        sup.register(registered_only_spec("nginx"));
        let before = sup.snapshot();

        let bulk_lock = app.state::<BulkLock>();
        let _held = bulk_lock
            .inner()
            .0
            .try_lock()
            .expect("test setup: lock must be free");

        handle_tray_menu_id(app.handle(), START_ALL_MENU_ITEM_ID);
        // Give a wrongly-dispatching implementation a real chance to have
        // acted before asserting nothing changed — same style as
        // `unknown_id_changes_nothing_and_does_not_panic`.
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            sup.snapshot(),
            before,
            "a bulk dispatch must not touch anything while BulkLock is already held"
        );
    }

    /// The OTHER half of spec D7's admission check: a bulk dispatch must
    /// ALSO reject while the EXISTING `crate::commands::ApplyLock` is held
    /// — not a second, unrelated lock, but the literal mutex `apply_config`
    /// itself locks.
    #[test]
    fn start_all_through_the_router_is_rejected_while_the_apply_lock_is_already_held() {
        let (app, sup) = app_with_supervisor();
        app.manage(BulkLock::default());
        app.manage(crate::commands::ApplyLock::default());
        app.manage(TrayInitiated::default());
        sup.register(registered_only_spec("nginx"));
        let before = sup.snapshot();

        let apply_lock = app.state::<crate::commands::ApplyLock>();
        let _held = apply_lock
            .inner()
            .0
            .try_lock()
            .expect("test setup: lock must be free");

        handle_tray_menu_id(app.handle(), START_ALL_MENU_ITEM_ID);
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(
            sup.snapshot(),
            before,
            "a bulk dispatch must not touch anything while ApplyLock is already held"
        );
    }

    // -----------------------------------------------------------------
    // maybe_show_failure_dialog_with (spec D4) — the WIRING half; the pure
    // DECISION (which transitions/ids owe a dialog) is proven independently
    // in `service_control.rs`'s own `dialog_for_transition` tests.
    // -----------------------------------------------------------------

    /// VACUITY (neuter-and-watch-it-fail): temporarily hardcoded
    /// `display_name` to `String::new()` instead of looking it up from
    /// `supervisor.snapshot()` — this test still passed (it only asserts
    /// the STDERR TAIL appears verbatim, not the display name), which is
    /// why the second assertion below checks the title contains the real
    /// registered display name too: reverting the lookup to a hardcoded
    /// value fails THAT assertion specifically.
    #[test]
    fn a_tracked_failure_shows_the_dialog_with_the_stderr_tail_verbatim() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let tracked = TrayInitiated::default();
        tracked.mark("nginx");
        app.manage(tracked);

        let sup = Supervisor::new(default_driver());
        sup.register(registered_only_spec("nginx"));

        let shown: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
        maybe_show_failure_dialog_with(
            app.handle(),
            &sup,
            "nginx",
            &ServiceState::Failed {
                exit: Some(1),
                stderr_tail: vec!["boom: could not bind port".to_string()],
            },
            |_app, title, body| {
                shown.lock().unwrap().push((title, body));
            },
        );

        let recorded = shown.into_inner().unwrap();
        assert_eq!(recorded.len(), 1, "expected exactly one dialog call");
        assert!(recorded[0].0.contains("nginx"));
        assert!(recorded[0].1.contains("boom: could not bind port"));
    }

    #[test]
    fn an_untracked_failure_never_calls_show() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        // Managed, but nothing marked — the id below was never dispatched
        // from the tray.
        app.manage(TrayInitiated::default());

        let sup = Supervisor::new(default_driver());
        sup.register(registered_only_spec("nginx"));

        let shown: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());
        maybe_show_failure_dialog_with(
            app.handle(),
            &sup,
            "nginx",
            &ServiceState::Failed {
                exit: Some(1),
                stderr_tail: vec!["boom".to_string()],
            },
            |_app, title, body| {
                shown.lock().unwrap().push((title, body));
            },
        );

        assert!(shown.into_inner().unwrap().is_empty());
    }

    #[test]
    fn maybe_show_failure_dialog_with_does_not_panic_when_tray_initiated_is_not_managed() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let sup = Supervisor::new(default_driver());
        sup.register(registered_only_spec("nginx"));

        maybe_show_failure_dialog_with(
            app.handle(),
            &sup,
            "nginx",
            &ServiceState::Failed {
                exit: Some(1),
                stderr_tail: vec!["boom".to_string()],
            },
            |_app, _title, _body| {
                panic!("must not be called with no TrayInitiated managed");
            },
        );
    }
}
