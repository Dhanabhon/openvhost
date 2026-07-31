// SPDX-License-Identifier: GPL-3.0-or-later
//! Quit confirmation, and the clean service shutdown behind it.
//!
//! Before this module, quitting abandoned every running service. Nothing stopped
//! them: there is no `Drop for Supervisor`, services spawn as their own process
//! group leaders, and `kill_on_drop` is never set — so `nginx` and `php-fpm` kept
//! serving after the window went away and only the NEXT launch's orphan reaper
//! killed them. The confirmation exists to make that consequence visible, and
//! `stop_all_with` exists to remove it.
//!
//! ## Two entry points, only one of which is interceptable by default
//!
//! - The window's close button raises [`tauri::WindowEvent::CloseRequested`],
//!   which carries an `api.prevent_close()`. That is the interceptable one —
//!   `lib.rs`'s handler now feeds it through [`hide_instead_of_close`], which
//!   hides the window instead of asking anything (P1 tray design, spec D1):
//!   the app and its services keep running, and a Dock click or the tray's
//!   "Open OpenVHost" bring the window back via `RunEvent::Reopen`. This path
//!   no longer touches the webview at all — see that function's docs.
//! - macOS `Cmd+Q` / the app menu's Quit is NOT interceptable by the OS. Tauri
//!   builds a default macOS menu whose Quit is `muda::PredefinedMenuItem::quit`,
//!   wired to the native `sel!(terminate:)`, and `tao` implements no
//!   `applicationShouldTerminate:` — so the process dies before any Rust or JS
//!   handler runs. Verified by reading tauri 2.11.3, muda 0.19.3
//!   (`PredefinedMenuItemType::Quit => sel!(terminate:)`) and tao 0.35.3 (no such
//!   selector anywhere in the tree).
//!
//! [`app_menu`] therefore replaces the default menu with the same structure minus
//! the predefined Quit, substituting a plain [`MenuItem`] that carries the
//! `Cmd+Q` accelerator and routes through `on_menu_event` like any other item.
//! Built explicitly rather than by mutating `Menu::default`'s items in place:
//! locating the predefined Quit there means indexing "last child of the first
//! submenu", which no API guarantees and a Tauri upgrade could silently move.
//!
//! ## Known exposure: `prevent_close` makes the UI load-bearing — for Quit only
//!
//! Before the P1 tray design's hide-on-close change, BOTH entry points ended in
//! "prevent, then ask the webview", so a dead frontend meant an app quittable
//! only by Force Quit on either path. That is no longer true for the close
//! button: [`hide_instead_of_close`] hides in pure Rust and never consults the
//! webview, so the app's most-used exit no longer depends on the frontend being
//! alive at all. `Cmd+Q` / the app menu's Quit ([`request_quit`]) is unchanged —
//! it still ends in "prevent, then ask the webview", and its exposure is exactly
//! what it always was: if the webview never came up, [`UiReady`] is never
//! marked, `request_quit` reports "cannot ask", and `lib.rs`'s `on_menu_event`
//! falls through to [`perform_quit`] directly instead of waiting on a dialog
//! that would never render. That fallback predates this module's D1 changes;
//! hiding just no longer needs it.
//!
//! ## `request_quit` must reveal the window before it asks
//!
//! Hiding-on-close creates a state that could not exist before it: the app can
//! be frontmost (own the OS's notion of "active application") while its ONLY
//! window is hidden — right after a Dock click re-activates the app but before
//! `RunEvent::Reopen` has run, or simply because the user hid the window and
//! then triggered `Cmd+Q` without ever clicking anything else. Emitting
//! [`QuitRequestedEvent`] in that state, into a webview nobody can see, would
//! silently refuse to quit: the dialog never renders, but [`UiReady`] IS marked
//! (this webview is alive), so the `perform_quit` fallback above does not fire
//! either — the app just does nothing, the single sharpest bug this design
//! flagged. [`request_quit`] therefore shows and focuses the main window BEFORE
//! emitting, not after, via [`request_quit_with`]: the closures-based split
//! exists so a test pins the ORDER, not merely that both calls happened.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use openvhost_proc::{ServiceState, Supervisor};
use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
// `tauri_specta::Event`, not `tauri::Emitter`: the emit method on a
// `#[derive(tauri_specta::Event)]` type comes from this trait, and it is what
// keeps the event name in sync with the generated TS binding.
use tauri_specta::Event as _;

/// Menu-item id for the substituted Quit. Namespaced so it cannot collide with a
/// predefined item's generated id.
pub const QUIT_MENU_ITEM_ID: &str = "openvhost:quit";

/// Menu-item id for the app menu's **Install Command Line Tool…** row (P1
/// CLI-install design D6).
///
/// Distinct from every other id in this app, and that is a correctness
/// requirement rather than tidiness: muda's menu events are GLOBAL — there is
/// one listener list for the whole process, which is why `tray::handle_tray_menu_id`
/// receives this app-menu row's clicks at all — so a colliding id would run
/// two actions from one click. Pinned by
/// `the_install_row_id_collides_with_nothing_else_this_app_dispatches`.
pub const INSTALL_CLI_TOOL_MENU_ITEM_ID: &str = "openvhost:install-cli-tool";

/// How long [`stop_all_with`] waits for services to actually reach a stopped
/// state before giving up on the stragglers.
///
/// Must exceed the LONGEST `grace` among registered [`ServiceSpec`]s
/// (`openvhost-proc`'s `ServiceSpec::grace` — nginx/php-fpm use
/// `DEFAULT_GRACE`, 5s; MySQL, added by the P1 MySQL lifecycle design, uses
/// 15s for a clean InnoDB shutdown, spec D4): `Supervisor::stop` only
/// REQUESTS a graceful stop, and the service task waits out that spec's own
/// grace period before escalating to a kill. A timeout at or under the
/// longest registered grace would report a service still legitimately
/// inside its own shutdown window as an abandoned straggler.
///
/// This was 8s (`DEFAULT_GRACE` + a 3s buffer) before MySQL's 15s grace
/// landed — the doc comment on THIS constant already flagged that the day a
/// longer-grace spec arrived, this timeout would need raising too, rather
/// than leaving that slice to rediscover it. It is now MySQL's 15s grace plus
/// the SAME 3s buffer the old value used relative to `DEFAULT_GRACE` (5s + 3s
/// = 8s ⇒ 15s + 3s = 18s), so the ratio of "how much slack beyond the
/// longest grace" is unchanged, not just the number.
///
/// A per-call value DERIVED from whichever specs are actually registered
/// (`Supervisor` has no accessor for a spec's `grace` today) was considered
/// and passed over for this slice: it would need a new `openvhost-proc`
/// accessor purely to serve this one call site, for a workspace that — as of
/// this slice — has exactly two grace values in use, not an open-ended set.
/// A documented constant that already carries its own "next slice, check
/// this" flag (this comment) is the simpler fix for two known values; if a
/// THIRD, longer grace ever lands, apply the identical reasoning again rather
/// than let this drift silently.
pub const STOP_ALL_TIMEOUT: Duration = Duration::from_secs(18);

/// Snapshot cadence while waiting. The supervisor's snapshot is an in-memory
/// read, so this is cheap; it only bounds how long after the last service stops
/// the quit still takes.
pub const STOP_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Emitted to the webview when a quit has been requested and prevented. The UI
/// answers with the `confirm_quit` command, or does nothing if the user cancels.
#[derive(Debug, Clone, serde::Serialize, specta::Type, tauri_specta::Event)]
pub struct QuitRequestedEvent {}

/// Whether [`perform_quit`] has begun tearing this instance down.
///
/// Exists for the local control channel (P1 CLI design). The control socket
/// keeps accepting for as long as the process lives, and a connection already
/// accepted when the quit starts is still served — so an `openvhost start
/// nginx` landing after [`stop_all`] has returned but before the process exits
/// would spawn nginx, register it, and then lose its supervisor, leaving
/// something listening after the user believes the stack is down.
///
/// Set once, never cleared: a quit does not get called off. The gate it feeds
/// is in `control::DesktopHandler` — mutating verbs answer `Busy`, reads keep
/// working, since answering "what is running?" while quitting is both harmless
/// and the honest thing to do.
///
/// This is the second half of the fix; the first is that `perform_quit`
/// unlinks the control socket before anything else, which is what stops NEW
/// connections from arriving at all. The flag only closes the in-flight
/// window that unlink cannot.
#[derive(Debug, Default)]
pub struct Quitting(AtomicBool);

impl Quitting {
    /// Record that a quit has started. Idempotent.
    pub fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    /// Whether a quit has started.
    pub fn has_begun(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Whether the UI has told us it is listening for [`QuitRequestedEvent`].
///
/// This flag is the whole reason `confirm_quit` is not the only new command.
/// `Emitter::emit` returns `Ok` when the payload was handed to the webview — it
/// says NOTHING about whether a listener exists — so "did the emit succeed" is
/// not a usable signal. Without a real one, a frontend that failed to load (a
/// broken bundle, a JS error before `onMount`) would leave `prevent_close`
/// refusing every close with no dialog to answer it: an app quittable only by
/// Force Quit. The UI acks once it has registered its listener, and until then
/// closing behaves exactly as it did before this feature.
///
/// It never resets. A webview that dies AFTER acking is still the Force Quit
/// case documented in the module header — this closes the startup hole, not that
/// one, and claiming otherwise would be the kind of guarantee that reads true
/// and is not.
#[derive(Debug, Default)]
pub struct UiReady(AtomicBool);

impl UiReady {
    pub fn mark(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_ready(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Ask the UI to confirm a quit, after making sure the window it will ask in
/// is actually visible and focused (see [`request_quit_with`] and the module
/// docs' "`request_quit` must reveal the window before it asks"). Returns
/// whether the ask can be answered: false means the caller must quit directly
/// ([`perform_quit`]) rather than wait for a dialog that cannot appear.
pub fn request_quit<R: Runtime>(app: &AppHandle<R>) -> bool {
    match app.try_state::<UiReady>() {
        Some(ready) if ready.is_ready() => {
            // One lookup, shared by both closures below, rather than two: the
            // window cannot have changed identity between "show" and "focus"
            // a few instructions later, so a second `get_webview_window` call
            // would only be able to observe the exact same value (or a
            // spurious `None` if the window vanished mid-call, which would
            // then inconsistently skip focus after having shown it).
            let window = app.get_webview_window("main");
            request_quit_with(
                || {
                    if let Some(w) = &window {
                        let _ = w.show();
                    }
                },
                || {
                    if let Some(w) = &window {
                        let _ = w.set_focus();
                    }
                },
                || QuitRequestedEvent {}.emit(app).is_ok(),
            )
        }
        // Not acked (or the state was never managed): no dialog can appear, so
        // report "cannot ask" and let the caller quit directly.
        _ => false,
    }
}

/// [`request_quit`]'s decision, parameterized over the window-reveal and emit
/// actions so the ORDER — show and focus attempted before the confirmation is
/// even asked — is what a test pins, not merely that all three ran. Same
/// reasoning as [`stop_all_with`]/[`abort_and_wait_with`]: the part of this
/// that can actually regress is a sequence of calls, and that has nothing to
/// do with a real `Window` or webview.
///
/// `show`/`focus` are best-effort BY DESIGN, not merely tolerated: a `Window`
/// method failing here has nothing to abort into (there is no window to fall
/// back to), and the ask is still worth making even if revealing the window
/// did not fully succeed — Force Quit remains the documented fallback either
/// way (see the module docs). What matters is that both are attempted, and
/// have RUN TO COMPLETION, strictly before `emit` is even called.
///
/// NOTE for anyone tempted to test this by building a real window under
/// `tauri::test::mock_builder` and asserting its state afterwards instead:
/// `tauri::test`'s `MockWindowDispatcher::is_visible`/`is_focused` are
/// hardcoded stubs (`Ok(true)`/`Ok(false)` unconditionally, verified against
/// tauri 2.11.5) that do not reflect calls to `show`/`set_focus` at all — such
/// a test would pass or fail independent of whether this function, or even
/// `request_quit`, was ever called. Closures make the ORDER the thing under
/// test instead, which is both real and actually verifiable.
pub fn request_quit_with(
    show: impl FnOnce(),
    focus: impl FnOnce(),
    emit: impl FnOnce() -> bool,
) -> bool {
    show();
    focus();
    emit()
}

/// The `CloseRequested` decision (P1 tray design, spec D1): hide the window,
/// and prevent the close only if the hide actually SUCCEEDED. A window that
/// failed to hide must not be trapped open either — the alternative,
/// preventing unconditionally, would leave a user stuck in an app whose
/// ordinary close button no longer works, which is worse than losing the
/// hide-instead-of-quit behaviour for that one attempt.
///
/// Takes closures rather than a real `Window`/`CloseRequestApi`, the same
/// reasoning as [`stop_all_with`] and [`abort_and_wait_with`]: the decision —
/// hide, then prevent only on success — has nothing to do with a live window.
/// It is also the ONLY way to unit test this at all:
/// `tauri::WindowEvent::CloseRequested`'s `api` (`CloseRequestApi`) wraps a
/// private `Sender<bool>` with no public constructor, so a test cannot build
/// one to drive a closure written in terms of the real types.
///
/// Returns the `hide` call's own result so the caller can log a failure
/// without this function needing to know how to log.
pub fn hide_instead_of_close<E>(
    hide: impl FnOnce() -> Result<(), E>,
    prevent_close: impl FnOnce(),
) -> Result<(), E> {
    let hidden = hide();
    if hidden.is_ok() {
        prevent_close();
    }
    hidden
}

/// The ids of services that are not in a terminal state, i.e. the ones a quit
/// would abandon.
///
/// `Failed` and `Stopped` are terminal — nothing is running to lose. Everything
/// else (`Running`, and the transitional states on the way in or out) counts:
/// a service mid-`Starting` has a live child just as much as a running one.
pub fn pending_service_ids(sup: &Supervisor) -> Vec<String> {
    sup.snapshot()
        .into_iter()
        .filter(|s| is_pending(&s.state))
        .map(|s| s.id)
        .collect()
}

/// Whether a state means "there is still a live child here".
///
/// Split out from [`pending_service_ids`] so the classification — the part that
/// can actually be wrong — is unit-testable: the function above needs a live
/// `Supervisor` (and therefore a process driver) to exercise, while this needs
/// only a `ServiceState` value.
fn is_pending(state: &ServiceState) -> bool {
    !matches!(state, ServiceState::Stopped | ServiceState::Failed { .. })
}

/// Request a stop for everything pending, then wait until it has actually
/// happened. Returns the ids still pending at the deadline — an empty vec means
/// a clean shutdown.
///
/// Takes closures rather than a `&Supervisor` so the waiting logic is testable
/// without spawning real processes: the timeout and give-up behaviour are the
/// parts most likely to be wrong, and they have nothing to do with the
/// supervisor. [`stop_all`] is the real-supervisor binding.
///
/// **Re-sends `stop` on every poll, not just an initial sweep before the loop**
/// (security audit finding M2, 2026-07-31 — this used to send `stop` once, to
/// whatever [`pending`] returned before the loop started, then only ever
/// poll). `perform_quit` takes neither the tray's `BulkLock` nor the existing
/// `ApplyLock`, so a bulk "Start all" dispatched moments before quit can still
/// be mid-sweep when this function takes its very first [`pending`] read: a
/// service `dispatch_start_all` has not yet reached is `Stopped` (not
/// pending) at that instant and only transitions to `Starting` on a LATER
/// poll — after a single up-front sweep would have already stopped asking
/// anything to stop at all. Re-sending on every iteration catches such a
/// service the next time it is observed pending, instead of abandoning it as
/// a straggler for the full `timeout` with a live child behind it.
///
/// Safe to re-send unconditionally: the real binding's `Supervisor::stop`
/// returns early for a service already `Stopped`/`Failed` (see [`stop_all`]),
/// so re-flagging an id that has already stopped is a documented no-op, not a
/// duplicate action — and D7's own admission-check reasoning already covers a
/// service mid-grace receiving more than one stop signal ("the full control
/// channel discards the duplicate").
pub async fn stop_all_with<P, S>(
    pending: P,
    stop: S,
    timeout: Duration,
    poll: Duration,
) -> Vec<String>
where
    P: Fn() -> Vec<String>,
    S: Fn(&str),
{
    // `Instant` rather than a deadline computed from wall-clock time: a clock
    // adjustment mid-shutdown must not turn an 8s wait into an instant give-up
    // (or a hang).
    let started = std::time::Instant::now();
    loop {
        let still = pending();
        if still.is_empty() {
            return Vec::new();
        }
        if started.elapsed() >= timeout {
            return still;
        }
        // Idempotent re-flag (see the doc comment above): everything still
        // pending is asked again on EVERY iteration, not only once up front,
        // so a service that becomes pending strictly after an earlier read —
        // e.g. a bulk start that reaches it moments later — is never left
        // with nothing telling it to stop.
        for id in &still {
            stop(id);
        }
        tokio::time::sleep(poll).await;
    }
}

/// [`stop_all_with`] bound to a real supervisor.
pub async fn stop_all(sup: Arc<Supervisor>) -> Vec<String> {
    let for_pending = Arc::clone(&sup);
    stop_all_with(
        move || pending_service_ids(&for_pending),
        // A stop that errors is deliberately ignored HERE rather than aborting the
        // shutdown: `Supervisor::stop` errors for an unknown or already-stopped id,
        // neither of which should keep the app open. What matters is the state the
        // poll observes, not this call's return.
        |id| {
            let _ = sup.stop(id);
        },
        STOP_ALL_TIMEOUT,
        STOP_POLL_INTERVAL,
    )
    .await
}

/// How long [`abort_pending_install`] waits for an in-flight `install_php` run
/// to actually finish after it is aborted, before giving up on it. Same value
/// and same reasoning as [`STOP_ALL_TIMEOUT`]: long enough for the abort to be
/// observed, short enough that quitting is not itself indefinitely blocked by
/// a run that is somehow wedged.
pub const INSTALL_ABORT_TIMEOUT: Duration = STOP_ALL_TIMEOUT;

/// Abort an in-flight run, then wait until it has actually finished — i.e.
/// until its future was genuinely dropped and `openvhost_proc`'s
/// `KillOnDrop` ran.
///
/// Takes closures rather than a real `AbortHandle`, mirroring why
/// [`stop_all_with`] takes closures instead of a `&Supervisor`: the decision
/// under test here is "abort, then poll `is_finished` until it is true or the
/// timeout elapses", and that decision has nothing to do with tokio tasks —
/// it is reachable from a plain boolean flag a test flips by hand. Returns
/// whether the run finished before the deadline.
pub async fn abort_and_wait_with<A, F>(
    abort: A,
    is_finished: F,
    timeout: Duration,
    poll: Duration,
) -> bool
where
    A: FnOnce(),
    F: Fn() -> bool,
{
    abort();

    // `Instant`, not wall-clock time — same reasoning as `stop_all_with`: a
    // clock adjustment mid-shutdown must not turn a bounded wait into an
    // instant give-up or a hang.
    let started = std::time::Instant::now();
    loop {
        if is_finished() {
            return true;
        }
        if started.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(poll).await;
    }
}

/// [`abort_and_wait_with`] bound to whatever `install_php` may have left
/// running on `InstallLock`. Returns `true` when there was nothing to abort,
/// or the abort completed in time; `false` only when a run was in flight and
/// did not finish before [`INSTALL_ABORT_TIMEOUT`].
///
/// `try_state`: `InstallLock` is only managed once the setup bootstrap
/// reaches that point (see `lib.rs`), same reasoning as every other
/// `try_state` read in this module — a quit must still work with nothing
/// managed at all.
pub async fn abort_pending_install<R: Runtime>(app: &AppHandle<R>) -> bool {
    let Some(lock) = app.try_state::<crate::commands::InstallLock>() else {
        return true;
    };
    let Some(abort) = lock.running_abort_handle() else {
        return true;
    };
    let for_abort = abort.clone();
    let for_check = abort;
    abort_and_wait_with(
        move || for_abort.abort(),
        move || for_check.is_finished(),
        INSTALL_ABORT_TIMEOUT,
        STOP_POLL_INTERVAL,
    )
    .await
}

// ---------------------------------------------------------------------------
// **Install Command Line Tool…** (P1 CLI-install design, D5/D6).
//
// Lives here, next to [`app_menu`] which builds the row, for the same reason
// [`QUIT_MENU_ITEM_ID`] does: the app menu is this module's. The click reaches
// it through `tray::handle_tray_menu_id`, the app's ONE menu-event listener —
// see that function's doc comment for why a function named for the tray also
// routes app-menu rows.
//
// **No new Tauri command and no `capabilities/*.json` change.** The handler
// calls `crate::clitool` directly, exactly as the tray's handlers call
// `service_control`. The webview gets no new surface.
// ---------------------------------------------------------------------------

/// Whether an install started from the menu is still running.
///
/// The action is not instantaneous: the D4 login-shell probe alone is bounded
/// at 2s, so a user can comfortably click the row a second time before the
/// first click has produced a dialog. `install()` itself is safe to run
/// concurrently — the CLI-install slice proved eight racing placements
/// converge on one link with no residue — so this is not about the
/// filesystem. It is about not stacking a second modal alert, and not
/// spawning a second login shell, for one intent.
///
/// **Reject, never queue**, the same posture the tray's `BulkLock` takes for
/// Start-all/Stop-all. A process-wide `static` rather than managed state:
/// there is one app per process and one such row, and a `static` needs no
/// `lib.rs` registration, so it has no "not managed yet" arm to get wrong.
static INSTALL_CLI_TOOL_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// A claim on [`INSTALL_CLI_TOOL_IN_FLIGHT`], released on drop.
///
/// `Drop` and not an explicit release call, for the same reason
/// `clitool::install`'s staging link uses one: every exit path has to release
/// it — the task finishing, the task being ABORTED (which is what a quit
/// mid-install does: `perform_quit` drops the runtime's tasks), or a panic.
/// An explicit call could only claim to cover the first.
struct InFlight<'a>(&'a AtomicBool);

impl<'a> InFlight<'a> {
    /// Claim the slot, or `None` if something already holds it.
    ///
    /// Written as an `if`, and **not** as
    /// `….is_ok().then_some(InFlight(flag))`, which is what this was first:
    /// `bool::then_some` evaluates its argument EAGERLY, so a REJECTED claim
    /// still constructed an `InFlight` and immediately dropped it — and this
    /// guard releases the flag in `Drop`. The rejected second click would
    /// therefore have handed the slot away from the first click that
    /// legitimately held it, so a third click would be admitted and the
    /// rejection would work exactly once. Caught by
    /// `a_second_click_is_rejected_while_the_first_install_is_still_running`'s
    /// third-claim assertion. `bool::then` (lazy) would also be correct; the
    /// `if` is spelled out because the failure it avoids is invisible in the
    /// combinator.
    fn claim(flag: &'a AtomicBool) -> Option<InFlight<'a>> {
        if flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(InFlight(flag))
        } else {
            None
        }
    }
}

impl Drop for InFlight<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Route a menu id to the CLI-tool install action, and report whether `id`
/// was ours.
///
/// Takes an **id, not a `tauri::menu::MenuEvent`**, exactly like
/// `tray::handle_tray_menu_id`: a `MenuEvent`'s only field is a private
/// `MenuId` with no public constructor, so a handler shaped around one would
/// be unreachable under `tauri::test::mock_builder`.
///
/// Parameterized over the ACTION as well, which the tray's router does not
/// need to be, for a reason specific to this feature: the real action writes
/// a symlink into a directory on the PATH of whoever is running the tests. No
/// test may do that. The closure is what lets a test observe the dispatch
/// decision — including that an unknown id dispatches NOTHING — without the
/// filesystem ever being touched. [`install_cli_tool`] is the production
/// argument and is passed by name, so the seam is one function reference
/// wide.
pub fn handle_install_cli_tool_id<R: Runtime>(
    app: &AppHandle<R>,
    id: &str,
    install: impl FnOnce(&AppHandle<R>),
) -> bool {
    if id != INSTALL_CLI_TOOL_MENU_ITEM_ID {
        return false;
    }
    install(app);
    true
}

/// Install the CLI and report what happened in a native dialog (D6).
///
/// Returns immediately. `crate::clitool::install` is `async` while
/// `on_menu_event` fires on the native main thread — which is NOT a tokio
/// worker — so the work goes through `tauri::async_runtime::spawn`, which
/// `.enter()`s tauri's own runtime before spawning. That is the established
/// crossing in this app (see `tray::handle_tray_menu_id`'s doc comment for
/// the verified details), and it is also what keeps the window responsive
/// across the up-to-2s login-shell probe instead of freezing the main thread
/// on it.
///
/// A second click while the first is still running does nothing at all — see
/// [`INSTALL_CLI_TOOL_IN_FLIGHT`].
pub fn install_cli_tool<R: Runtime>(app: &AppHandle<R>) {
    let Some(in_flight) = InFlight::claim(&INSTALL_CLI_TOOL_IN_FLIGHT) else {
        return;
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Moved into the task, so the slot is held for exactly as long as the
        // work runs and is released however the task ends.
        let _in_flight = in_flight;
        let report = match crate::clitool::install().await {
            Ok(outcome) => crate::clitool::report_for_outcome(&outcome),
            Err(e) => crate::clitool::report_for_error(&e),
        };
        show_report_dialog(&app, report);
    });
}

/// The actual native dialog call — the one piece of this feature that cannot
/// be tested (no `NSAlert` in a test process), kept as small as that fact
/// demands. Every DECISION feeding it (the title, the body, the PATH verdict,
/// the export line, the kind) is `crate::clitool`'s pure rendering, which is
/// tested without this function ever running. Same split as
/// `tray::show_failure_dialog`.
///
/// `.show(..)`, never `.blocking_show(..)`: the latter's own documentation
/// says it must not run on the main thread, and this is reached from a tokio
/// worker, which it would then block for as long as the user leaves the
/// dialog up.
fn show_report_dialog<R: Runtime>(app: &AppHandle<R>, report: crate::clitool::Report) {
    // Exhaustive: `clitool` deliberately owns its own kind enum so that
    // module stays free of Tauri, and this is the one place the two meet.
    let kind = match report.kind {
        crate::clitool::ReportKind::Info => MessageDialogKind::Info,
        crate::clitool::ReportKind::Warning => MessageDialogKind::Warning,
        crate::clitool::ReportKind::Error => MessageDialogKind::Error,
    };
    app.dialog()
        .message(report.body)
        .title(report.title)
        .kind(kind)
        .buttons(MessageDialogButtons::Ok)
        .show(|_| {});
}

/// The app menu, with a Quit that this app can intercept.
///
/// Mirrors `tauri::menu::Menu::default` — same submenus in the same order — with
/// one substitution: `PredefinedMenuItem::quit` becomes a plain [`MenuItem`]
/// carrying `Cmd+Q`. See the module docs for why the predefined one cannot be
/// intercepted.
#[cfg(target_os = "macos")]
pub fn app_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let config = app.config();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        copyright: config.bundle.copyright.clone(),
        authors: config.bundle.publisher.clone().map(|p| vec![p]),
        ..Default::default()
    };

    let quit = MenuItem::with_id(
        app,
        QUIT_MENU_ITEM_ID,
        format!("Quit {}", pkg.name),
        true,
        Some("CmdOrCtrl+Q"),
    )?;

    // **Install Command Line Tool…**, after About with its own separator (D6):
    // the app menu is already ours, there is no Settings route to put a row in
    // today, and this is the idiomatic macOS home for an "install helper /
    // command line tool" action.
    //
    // The label reflects `detect()`, which is why that function is
    // synchronous and does NOT run the D4 login-shell probe: this call
    // happens while the menu is being built, and a 2s probe here would be a
    // 2s stall before the app has a menu bar. The filesystem questions it
    // does ask are a handful of `symlink_metadata` calls.
    //
    // Computed ONCE, at menu-build time, and never refreshed — deliberately.
    // The one state that changes the label (`Broken`) arises from the app
    // being moved or deleted, which `current_exe()` does not observe in a
    // running process anyway (see `clitool::detect::source_binary`'s "moved
    // while running" note), so a relaunch is already part of reaching it —
    // and the click-list checks it across exactly that relaunch.
    let install_cli_tool_item = MenuItem::with_id(
        app,
        INSTALL_CLI_TOOL_MENU_ITEM_ID,
        crate::clitool::menu_label(&crate::clitool::detect()),
        true,
        None::<&str>,
    )?;

    Menu::with_items(
        app,
        &[
            &Submenu::with_items(
                app,
                pkg.name.clone(),
                true,
                &[
                    &PredefinedMenuItem::about(app, None, Some(about))?,
                    &PredefinedMenuItem::separator(app)?,
                    &install_cli_tool_item,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::services(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::hide(app, None)?,
                    &PredefinedMenuItem::hide_others(app, None)?,
                    &PredefinedMenuItem::show_all(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &quit,
                ],
            )?,
            &Submenu::with_items(
                app,
                "Edit",
                true,
                &[
                    &PredefinedMenuItem::undo(app, None)?,
                    &PredefinedMenuItem::redo(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::cut(app, None)?,
                    &PredefinedMenuItem::copy(app, None)?,
                    &PredefinedMenuItem::paste(app, None)?,
                    &PredefinedMenuItem::select_all(app, None)?,
                ],
            )?,
            &Submenu::with_items(
                app,
                "View",
                true,
                &[&PredefinedMenuItem::fullscreen(app, None)?],
            )?,
            &Submenu::with_items(
                app,
                "Window",
                true,
                &[
                    &PredefinedMenuItem::minimize(app, None)?,
                    &PredefinedMenuItem::maximize(app, None)?,
                    &PredefinedMenuItem::separator(app)?,
                    &PredefinedMenuItem::close_window(app, None)?,
                ],
            )?,
        ],
    )
}

/// Tear the app down: stop every pending service, then destroy the window.
///
/// `destroy` and not `close`: `close` re-emits `CloseRequested`, which this
/// app's handler feeds to [`hide_instead_of_close`] — a `close()` here would
/// therefore hide the window instead of tearing it down, forever (the window
/// keeps successfully hiding, so the "close" this function needs never
/// actually happens). `destroy` "does not emit any events and force close the
/// window instead" (tauri 2.11.3, `Window::destroy`), so it is the only exit
/// that terminates.
pub async fn perform_quit<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    // FIRST — before the socket is even unlinked, so there is no instant in
    // which the control channel is still reachable AND still admits work
    // (A3 audit fix). Cheap, infallible, and it only refuses mutating verbs.
    if let Some(quitting) = app.try_state::<Quitting>() {
        quitting.mark();
    }

    // SECOND, and still before a single service is touched — this is the A1
    // audit fix, and the ordering IS the fix.
    //
    // The socket is unlinked here rather than by `control::serve`, whose own
    // unlink sits after a loop the app never lets break: it is handed
    // `std::future::pending()`, because there is no orderly-shutdown event in
    // this app, only a quit. A unix socket is not removed when its process
    // exits, so without this the path outlives every quit and the next
    // `openvhost status` connects to a socket nobody is listening on —
    // ECONNREFUSED, reported as "the app appears to be running but is not
    // accepting control connections" with exit 69, when the truthful answer
    // is "not running" with exit 0.
    //
    // Doing it BEFORE `stop_all` is what makes a control verb racing this
    // quit get "not running" instead of starting a service moments before its
    // supervisor disappears. Identity-checked (`ControlSocket::remove` only
    // unlinks the inode it bound), so this cannot unlink a newer instance's
    // socket, and is safe to run even if `serve` somehow got there first.
    if let Some(socket) = app.try_state::<openvhost_proc::control::ControlSocket>() {
        socket.remove();
    }

    // THEN, before touching services or the window at all — this is the C1
    // audit fix. `install_php`'s `run_task` is only contained by
    // `KillOnDrop`, which fires when its future is dropped; nothing else in
    // this app's shutdown path ever drops it. Aborting-and-waiting HERE makes
    // this the place that does: by the time `stop_all` and `window.destroy()`
    // below run, any `brew install` that was mid-flight has genuinely been
    // torn down, group-kill included, rather than left to `process::exit`
    // with no unwinding at all.
    if !abort_pending_install(app).await {
        eprintln!(
            "openvhost: quitting with a PHP install still not aborted within {INSTALL_ABORT_TIMEOUT:?}"
        );
    }

    // `try_state`, not `state`: the supervisor is only managed when the setup
    // bootstrap succeeded (it is skipped when OPENVHOST_HOME cannot be resolved
    // or the instance lock is held elsewhere). With no supervisor there is
    // nothing running to stop, and a quit must still work.
    if let Some(sup) = app.try_state::<Arc<Supervisor>>() {
        let stragglers = stop_all(Arc::clone(&sup)).await;
        if !stragglers.is_empty() {
            // Quit anyway. The user asked to. These are left for the next
            // launch's orphan reaper — the same safety net that catches a crash.
            eprintln!(
                "openvhost: quitting with {} service(s) still not stopped: {}",
                stragglers.len(),
                stragglers.join(", ")
            );
        }
    }

    let window = app
        .get_webview_window("main")
        .ok_or_else(|| "the main window is gone".to_string())?;
    window.destroy().map_err(|e| e.to_string())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    const FAST_POLL: Duration = Duration::from_millis(1);

    #[tokio::test]
    async fn nothing_pending_reports_clean_without_asking_anyone_to_stop() {
        let stops = AtomicUsize::new(0);
        let straggling = stop_all_with(
            Vec::new,
            |_| {
                stops.fetch_add(1, Ordering::SeqCst);
            },
            STOP_ALL_TIMEOUT,
            FAST_POLL,
        )
        .await;
        assert!(straggling.is_empty());
        // Quitting an idle app must not send a stop to anything — a stray stop on
        // an unknown id is how a "clean shutdown" starts logging errors.
        assert_eq!(stops.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stops_every_pending_service_exactly_once() {
        let asked: Mutex<Vec<String>> = Mutex::new(Vec::new());
        // Pending until asked, then empty — the shape of a supervisor that honours
        // the stop request.
        let straggling = stop_all_with(
            || {
                if asked.lock().unwrap().is_empty() {
                    vec!["nginx".to_string(), "php-fpm".to_string()]
                } else {
                    Vec::new()
                }
            },
            |id| asked.lock().unwrap().push(id.to_string()),
            STOP_ALL_TIMEOUT,
            FAST_POLL,
        )
        .await;
        assert!(straggling.is_empty());
        assert_eq!(asked.lock().unwrap().as_slice(), ["nginx", "php-fpm"]);
    }

    #[tokio::test]
    async fn waits_for_a_slow_stop_rather_than_reporting_it_as_a_straggler() {
        // Still pending on the first two polls: a `stop_all_with` that only
        // checked once would call this a straggler and abandon it.
        let polls = AtomicUsize::new(0);
        let straggling = stop_all_with(
            || {
                if polls.fetch_add(1, Ordering::SeqCst) < 3 {
                    vec!["nginx".to_string()]
                } else {
                    Vec::new()
                }
            },
            |_| {},
            STOP_ALL_TIMEOUT,
            FAST_POLL,
        )
        .await;
        assert!(straggling.is_empty());
        assert!(polls.load(Ordering::SeqCst) >= 3);
    }

    #[tokio::test]
    async fn gives_up_at_the_deadline_and_names_the_stragglers() {
        let straggling = stop_all_with(
            || vec!["nginx".to_string()], // never stops
            |_| {},
            Duration::from_millis(20),
            FAST_POLL,
        )
        .await;
        assert_eq!(straggling, ["nginx"]);
    }

    /// THE M2 REGRESSION TEST (security audit finding M2, 2026-07-31): a bulk
    /// "Start all" racing quit can reach a service AFTER `stop_all_with`'s
    /// first read of what is pending — "early" stands in for a service
    /// already pending when the read happens (keeping the loop alive across
    /// several polls, the way a real service takes a moment to actually
    /// stop), "late-riser" stands in for one `dispatch_start_all` had not yet
    /// reached: `Stopped` (not pending) on the very first read, only
    /// transitioning to `Starting` (pending) from the second read onward,
    /// and staying pending until it has actually been asked to stop.
    ///
    /// VACUITY (neuter-and-watch-it-fail): reverting `stop_all_with` to a
    /// single up-front sweep (send `stop` once, to whatever `pending()`
    /// returns at that instant, before the loop, then only ever poll
    /// afterwards) makes this test fail exactly as the audit described:
    /// "late-riser" is never in the ids handed to `stop` at all (the
    /// up-front sweep only ever saw "early"), so it stays pending forever in
    /// this fake model and `straggling` comes back `["late-riser"]` instead
    /// of empty. The test's own timeout is far shorter than
    /// [`STOP_ALL_TIMEOUT`] so that failure is fast rather than merely
    /// "eventually true after 18s". Restoring the per-iteration re-send
    /// fixes it.
    #[tokio::test]
    async fn a_service_that_becomes_pending_after_the_first_read_is_still_stopped() {
        const EARLY_GONE_AFTER: usize = 3;
        let reads = AtomicUsize::new(0);
        let late_riser_stopped = std::sync::atomic::AtomicBool::new(false);
        let asked: Mutex<Vec<String>> = Mutex::new(Vec::new());

        let straggling = stop_all_with(
            || {
                let n = reads.fetch_add(1, Ordering::SeqCst);
                let mut still = Vec::new();
                // "early": pending for the first few reads, then finishes on
                // its own — an ordinary service completing its own stop —
                // which is what keeps the loop alive long enough for
                // "late-riser" to ever be observed at all.
                if n < EARLY_GONE_AFTER {
                    still.push("early".to_string());
                }
                // "late-riser": NOT pending on the very first read (n == 0,
                // still `Stopped` at that instant), but pending from the
                // SECOND read onward — the concurrent bulk-start reached it
                // moments later — until it has actually been asked to stop.
                if n > 0 && !late_riser_stopped.load(Ordering::SeqCst) {
                    still.push("late-riser".to_string());
                }
                still
            },
            |id| {
                asked.lock().unwrap().push(id.to_string());
                if id == "late-riser" {
                    late_riser_stopped.store(true, Ordering::SeqCst);
                }
            },
            STOP_ALL_TIMEOUT,
            FAST_POLL,
        )
        .await;

        assert!(
            straggling.is_empty(),
            "late-riser became pending only after the first read and must still be \
             stopped, not abandoned as a straggler: {straggling:?}"
        );
        assert!(
            asked.lock().unwrap().contains(&"late-riser".to_string()),
            "a service that only became pending on a LATER poll never received a stop \
             under the single-sweep implementation this test guards against"
        );
    }

    #[test]
    fn only_stopped_and_failed_count_as_finished() {
        // `Starting` is the one that matters: a service coming up has a live child,
        // and treating it as finished would abandon exactly the process a quit
        // during startup needs to clean up.
        assert!(is_pending(&ServiceState::Starting));
        assert!(is_pending(&ServiceState::Running));
        assert!(!is_pending(&ServiceState::Stopped));
        assert!(!is_pending(&ServiceState::Failed {
            exit: Some(1),
            stderr_tail: Vec::new(),
        }));
    }

    #[test]
    fn ui_ready_starts_false_and_latches() {
        let ready = UiReady::default();
        // False by default is the load-bearing half: it is what makes
        // `request_quit` report "cannot ask" — so `Cmd+Q` falls through to
        // `perform_quit` directly instead of waiting on a dialog that cannot
        // appear — before the UI has ever mounted. (Hiding on the close
        // button no longer consults this flag at all; see `quit.rs`'s module
        // docs.)
        assert!(!ready.is_ready());
        ready.mark();
        assert!(ready.is_ready());
    }

    // -----------------------------------------------------------------------
    // P1 tray design, spec D1: hide-on-close, and the `request_quit`
    // show-first fix.
    // -----------------------------------------------------------------------

    #[test]
    fn hide_instead_of_close_prevents_the_close_when_hiding_succeeds() {
        let prevented = AtomicUsize::new(0);
        let result: Result<(), &'static str> = hide_instead_of_close(
            || Ok(()),
            || {
                prevented.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(result.is_ok());
        assert_eq!(prevented.load(Ordering::SeqCst), 1);
    }

    /// VACUITY (neuter-and-watch-it-fail): temporarily made `prevent_close`
    /// unconditional in `hide_instead_of_close` (dropped the `if
    /// hidden.is_ok()` guard) — this test failed, `prevented` was 1 instead
    /// of the asserted 0, because an unconditional implementation cannot
    /// distinguish "hide succeeded" from "hide failed". Restoring the guard
    /// made it pass again. The PRECEDING test alone could not have caught
    /// this: an unconditionally-preventing implementation still passes it.
    #[test]
    fn a_failed_hide_does_not_prevent_the_close() {
        let prevented = AtomicUsize::new(0);
        let result = hide_instead_of_close(
            || Err("window is gone"),
            || {
                prevented.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert_eq!(result, Err("window is gone"));
        // The close must proceed — trapping the user in an app whose close
        // button no longer works is worse than losing the hide this once.
        assert_eq!(prevented.load(Ordering::SeqCst), 0);
    }

    /// VACUITY (neuter-and-watch-it-fail): temporarily reordered
    /// `request_quit_with`'s body to `emit(); show(); focus();` (emit
    /// first, the exact bug this function exists to prevent) — this test
    /// failed, recording `["emit", "show", "focus"]` against the asserted
    /// `["show", "focus", "emit"]`. A test that only checked "all three were
    /// called" (e.g. three separate booleans) would have kept passing;
    /// asserting the single ordered `Vec` is what makes order the thing
    /// under test. Restoring the original order made it pass again.
    #[test]
    fn request_quit_with_shows_and_focuses_before_emitting() {
        let order: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        let emitted = request_quit_with(
            || order.lock().unwrap().push("show"),
            || order.lock().unwrap().push("focus"),
            || {
                order.lock().unwrap().push("emit");
                true
            },
        );
        assert!(emitted);
        assert_eq!(order.lock().unwrap().as_slice(), ["show", "focus", "emit"]);
    }

    #[test]
    fn request_quit_with_reports_the_emit_outcome() {
        assert!(request_quit_with(|| {}, || {}, || true));
        assert!(!request_quit_with(|| {}, || {}, || false));
    }

    #[test]
    fn request_quit_reports_false_when_the_ui_never_acked() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        // No `UiReady` managed at all — the "never got that far" case
        // `abort_pending_install_reports_finished_when_nothing_is_running`
        // exercises for install state, applied to this module's own gate.
        assert!(!request_quit(app.handle()));

        // Managed, but not yet marked: the UI exists in principle but has
        // not acked its listener.
        app.manage(UiReady::default());
        assert!(!request_quit(app.handle()));
    }

    #[test]
    fn request_quit_asks_once_the_ui_has_acked_even_with_no_window() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        // `QuitRequestedEvent::emit` goes through `tauri_specta`, which
        // requires an `EventRegistry` to be managed — normally done once,
        // for every event, by `lib.rs`'s real `specta_builder().mount_events`
        // call inside `setup()`. `mock_builder()` never runs that, and
        // skipping this does not fail the assertion below — it PANICS deep
        // inside `tauri_specta::Event::emit` ("EventRegistry not found in
        // Tauri state") before this test gets anywhere near `request_quit`.
        // Mounting just this one event is enough to exercise the real path.
        tauri_specta::Builder::<tauri::test::MockRuntime>::new()
            .events(tauri_specta::collect_events![QuitRequestedEvent])
            .mount_events(&app);
        let ready = UiReady::default();
        ready.mark();
        app.manage(ready);
        // No "main" window exists in this mock context. `request_quit` must
        // not panic, and must not let a missing window suppress the ask —
        // the window is a best-effort reveal, not a precondition for asking.
        assert!(request_quit(app.handle()));
    }

    // -----------------------------------------------------------------------
    // P1 CLI-install design, D5/D6: the **Install Command Line Tool…** row.
    //
    // These drive `handle_install_cli_tool_id` under `mock_builder` with a
    // recording closure in place of `install_cli_tool`. That substitution is
    // not a convenience: the real action symlinks into `/usr/local/bin` or
    // `~/.local/bin`, i.e. the PATH of whoever runs `cargo test`. A test that
    // called it would modify the developer's machine. The rendering it feeds
    // is proven separately and fully in `clitool`'s own test module.
    // -----------------------------------------------------------------------

    /// A recorder for "was the action dispatched, and with which app?".
    fn dispatch_count(id: &str) -> usize {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let calls = AtomicUsize::new(0);
        let handled = handle_install_cli_tool_id(app.handle(), id, |_app| {
            calls.fetch_add(1, Ordering::SeqCst);
        });
        let calls = calls.load(Ordering::SeqCst);
        // The return value and the side effect must agree, always: a router
        // that claimed an id without acting on it would silently swallow the
        // click (this `return`s in `handle_tray_menu_id`), and one that acted
        // without claiming it would fall through to the service lookup below
        // it and act twice.
        assert_eq!(handled, calls == 1, "handled={handled} but calls={calls}");
        calls
    }

    /// VACUITY (neuter-and-watch-it-fail): dropped the `if id != …` guard from
    /// `handle_install_cli_tool_id` so it dispatched unconditionally — THIS
    /// test kept passing, and `an_unknown_id_never_dispatches_the_install_action`
    /// failed for every id it tries. The pair is the test; neither half alone
    /// is.
    #[test]
    fn the_install_row_id_dispatches_the_install_action() {
        assert_eq!(dispatch_count(INSTALL_CLI_TOOL_MENU_ITEM_ID), 1);
    }

    /// VACUITY (neuter-and-watch-it-fail): replaced the exact-match guard with
    /// `id.starts_with("openvhost:install")` — a plausible "helpful" loosening
    /// — and this failed on the `openvhost:install-cli-tool-uninstall`
    /// near-miss below, which is exactly the shape a later id would take.
    #[test]
    fn an_unknown_id_never_dispatches_the_install_action() {
        for id in [
            "",
            "does-not-exist",
            QUIT_MENU_ITEM_ID,
            crate::tray::OPEN_MENU_ITEM_ID,
            crate::tray::START_ALL_MENU_ITEM_ID,
            crate::tray::STOP_ALL_MENU_ITEM_ID,
            crate::tray::SUMMARY_MENU_ITEM_ID,
            "nginx",
            // Near misses in both directions: a prefix, and an id this one is
            // a prefix of.
            "openvhost:install",
            "openvhost:install-cli-tool-uninstall",
            " openvhost:install-cli-tool",
        ] {
            assert_eq!(dispatch_count(id), 0, "{id:?} must not install anything");
        }
    }

    /// muda's menu events are global — one listener list for the app menu and
    /// the tray together — so two rows sharing an id would run both actions
    /// from one click.
    #[test]
    fn the_install_row_id_collides_with_nothing_else_this_app_dispatches() {
        let ids = [
            INSTALL_CLI_TOOL_MENU_ITEM_ID,
            QUIT_MENU_ITEM_ID,
            crate::tray::OPEN_MENU_ITEM_ID,
            crate::tray::SUMMARY_MENU_ITEM_ID,
            crate::tray::START_ALL_MENU_ITEM_ID,
            crate::tray::STOP_ALL_MENU_ITEM_ID,
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in ids.iter().skip(i + 1) {
                assert_ne!(a, b, "two menu rows share an id");
            }
        }
    }

    /// Reentrancy (D6): a second click landing while the first install is
    /// still probing the login shell — up to 2s — must do nothing at all
    /// rather than stack a second dialog.
    ///
    /// Driven against a local `AtomicBool` rather than the real
    /// `INSTALL_CLI_TOOL_IN_FLIGHT` static, deliberately: `cargo test` runs
    /// this file's tests in parallel threads of ONE process, so a test that
    /// claimed the process-wide slot would be visible to every other test
    /// that touched it. The static is only ever claimed by
    /// `install_cli_tool`, which no test may call.
    ///
    /// VACUITY: this one did not need a neuter — it went RED against the
    /// first real implementation and found a live bug. `InFlight::claim` was
    /// `….is_ok().then_some(InFlight(flag))`, and `bool::then_some` evaluates
    /// its argument EAGERLY: a rejected claim still built an `InFlight` and
    /// dropped it on the spot, and `Drop` releases the flag. So the SECOND
    /// click, while correctly reporting itself rejected, handed the slot away
    /// from the first click that held it, and a THIRD click was admitted. The
    /// third-claim assertion below is the one that caught it; a test that
    /// stopped after the second would have passed against that bug.
    ///
    /// Confirmed as a vacuity check afterwards too: replacing
    /// `compare_exchange` with an unconditional `store(true)` +
    /// `Some(InFlight(flag))` fails on the second claim.
    #[test]
    fn a_second_click_is_rejected_while_the_first_install_is_still_running() {
        let flag = AtomicBool::new(false);
        let first = InFlight::claim(&flag).expect("the slot must start free");
        assert!(
            InFlight::claim(&flag).is_none(),
            "a second click must be rejected, not queued"
        );
        assert!(
            InFlight::claim(&flag).is_none(),
            "and so must a third — rejection is not a one-shot"
        );
        drop(first);
    }

    /// The other half: the slot must come back. A gate that latched would
    /// disable the menu row for the rest of the session after one use — and
    /// it must come back however the run ended, which is why `InFlight`
    /// releases in `Drop` rather than on a success path.
    ///
    /// VACUITY (neuter-and-watch-it-fail): emptied `InFlight`'s `Drop` body —
    /// this failed on the claim after the drop. It also failed after the
    /// panic-unwind half below, which is the case an explicit release call
    /// could not have covered.
    #[test]
    fn the_slot_is_released_however_the_run_ended() {
        let flag = AtomicBool::new(false);
        drop(InFlight::claim(&flag).expect("free"));
        let again = InFlight::claim(&flag);
        assert!(again.is_some(), "the slot never came back");
        drop(again);

        // An aborted or panicking run still drops the guard.
        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _held = InFlight::claim(&flag).expect("free again");
            panic!("the install task blew up");
        }));
        assert!(unwound.is_err(), "the closure was supposed to panic");
        assert!(
            InFlight::claim(&flag).is_some(),
            "a panicking run left the row permanently disabled"
        );
    }

    /// The timeout must be longer than the grace period the service task waits
    /// out before killing, or the wait abandons processes that were about to die.
    #[test]
    fn stop_all_timeout_outlives_the_supervisor_grace_deadline() {
        assert!(STOP_ALL_TIMEOUT > Duration::from_secs(5));
    }

    /// THE QUIT-BUDGET TEST (P1 MySQL lifecycle design decision 1): MySQL's
    /// `ServiceSpec::grace` is 15s (`crate::stack::MYSQL_GRACE`) — longer than
    /// the 5s `DEFAULT_GRACE` nginx/php-fpm use, which is what the OLD
    /// `STOP_ALL_TIMEOUT` (8s) was sized against. Pinned against the REAL
    /// constant `mysql_spec` builds its `ServiceSpec` with, not a bare
    /// literal `15`, so this test cannot silently pass after `MYSQL_GRACE`
    /// changes without `STOP_ALL_TIMEOUT` changing to match.
    ///
    /// VACUITY: this is exactly the pre-fix value's own shape
    /// (`stop_all_timeout_outlives_the_supervisor_grace_deadline` above,
    /// asserting `> 5s` against `DEFAULT_GRACE`) — reverting `STOP_ALL_TIMEOUT`
    /// to 8s makes THIS test fail while that one keeps passing, which is the
    /// point: the old test alone was not enough to catch a quit budget sized
    /// for nginx/php-fpm but not MySQL.
    #[test]
    fn stop_all_timeout_outlives_mysqls_longer_grace() {
        assert!(STOP_ALL_TIMEOUT > crate::stack::MYSQL_GRACE);
    }

    // -----------------------------------------------------------------------
    // C1: `abort_and_wait_with` / `abort_pending_install`.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn aborts_and_reports_finished_once_is_finished_becomes_true() {
        let aborted = AtomicUsize::new(0);
        // Finished only after a couple of polls — the shape of a real task
        // that takes a moment to actually tear down after being asked to stop.
        let polls = AtomicUsize::new(0);
        let finished = abort_and_wait_with(
            || {
                aborted.fetch_add(1, Ordering::SeqCst);
            },
            || polls.fetch_add(1, Ordering::SeqCst) >= 2,
            STOP_ALL_TIMEOUT,
            FAST_POLL,
        )
        .await;
        assert!(finished);
        assert_eq!(
            aborted.load(Ordering::SeqCst),
            1,
            "abort must fire exactly once"
        );
    }

    #[tokio::test]
    async fn gives_up_at_the_deadline_when_the_run_never_finishes() {
        let finished = abort_and_wait_with(
            || {},
            || false, // never finishes
            Duration::from_millis(20),
            FAST_POLL,
        )
        .await;
        assert!(!finished);
    }

    #[tokio::test]
    async fn abort_pending_install_reports_finished_when_nothing_is_running() {
        // No `InstallLock` managed at all — same "nothing to do" shape
        // `perform_quit`'s own `try_state` reads handle everywhere else in
        // this module.
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        assert!(abort_pending_install(app.handle()).await);

        // Managed, but idle: still nothing to abort.
        app.manage(crate::commands::InstallLock::default());
        assert!(abort_pending_install(app.handle()).await);
    }

    /// THE C1 REGRESSION TEST. Reproduces the audit's own repro shape — a
    /// `run_task` whose child is still alive when the app quits mid-install —
    /// but drives the abort through `abort_pending_install`, the exact
    /// function `perform_quit` calls before `window.destroy()`, rather than a
    /// bare `run.abort()` the way `openvhost-proc`'s own `tests/task_group.rs`
    /// does. `perform_quit` itself cannot be driven from a test (it also
    /// requires a real webview window to destroy), so this pins the
    /// abort-and-wait DECISION instead — same reasoning `stop_all_with` is
    /// unit-tested separately from `stop_all`.
    ///
    /// VACUITY CHECK (see the audit report): with the `abort()` call removed
    /// from `abort_and_wait_with`, this test fails — the child is still alive
    /// when the deadline is asserted. Restoring it makes the test pass again.
    #[cfg(unix)]
    #[tokio::test]
    async fn quitting_mid_install_actually_kills_the_still_running_child() {
        use std::ffi::OsString;
        use std::path::PathBuf;

        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(crate::commands::InstallLock::default());

        // A real child, standing in for the tree `brew install` would leave
        // running: prints its own pid (so the test can check on it with
        // `kill -0`/`SIGKILL`, the same technique `task_group.rs` uses for its
        // forked grandchild), then sleeps far longer than this test's
        // deadline. `exec` replaces the shell with `sleep` so there is exactly
        // one pid to track, and it stays the process-group leader `run_task`'s
        // `KillOnDrop` targets.
        let spec = openvhost_proc::SpawnSpec {
            program: PathBuf::from("/bin/sh"),
            args: vec![
                OsString::from("-c"),
                OsString::from("echo pid: $$; exec sleep 100"),
            ],
            cwd: None,
            env: vec![],
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let install_task = tokio::spawn(openvhost_proc::run_task(
            openvhost_proc::default_driver(),
            spec,
            tx,
        ));

        let lock = app.state::<crate::commands::InstallLock>();
        lock.inner().set_running(
            crate::commands::InstallKind::Php,
            "8.4".to_string(),
            install_task.abort_handle(),
        );

        // Wait for proof the child is actually running before quitting on it.
        let pid: i32 = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match rx
                    .recv()
                    .await
                    .expect("channel closed before a pid line appeared")
                {
                    openvhost_proc::TaskEvent::Line { text, .. } => {
                        if let Some(rest) = text.strip_prefix("pid: ") {
                            return rest
                                .trim()
                                .parse::<i32>()
                                .expect("pid line was not a number");
                        }
                    }
                    openvhost_proc::TaskEvent::Finished { .. } => {
                        panic!("the child exited before printing its pid")
                    }
                }
            }
        })
        .await
        .expect("no pid line within the deadline");

        // SAFETY: signal 0 performs no action; it only checks existence/permission.
        assert_eq!(
            unsafe { libc::kill(pid, 0) },
            0,
            "child {pid} was not alive before quitting on it"
        );

        // THE CLAIM: this call — not `install_task.abort()` on its own — is
        // what `perform_quit` makes before `window.destroy()`.
        let finished = abort_pending_install(app.handle()).await;
        assert!(
            finished,
            "abort_pending_install did not finish within its timeout"
        );

        // Poll for actual death rather than asserting instantly: abort is
        // requested, not synchronous with the child's exit.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut still_alive = true;
        while std::time::Instant::now() < deadline {
            // SAFETY: signal 0 performs no action; it only checks existence/permission.
            if unsafe { libc::kill(pid, 0) } != 0 {
                still_alive = false;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        // Defensive cleanup on both the pass and fail path, same as
        // `task_group.rs`: a failing run must not leave a stray process on
        // the developer's machine.
        if still_alive {
            // SAFETY: plain kill syscall, cleaning up a leaked descendant.
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }

        assert!(
            !still_alive,
            "child {pid} was still alive after abort_pending_install — the future was \
             never genuinely dropped, so KillOnDrop never ran"
        );

        let _ = install_task.await;
    }

    // -----------------------------------------------------------------------
    // A1/A3: the control socket must not outlive the quit.
    // -----------------------------------------------------------------------

    /// A handler that must never be reached: these tests drive the QUIT path,
    /// and the only control request they make happens after the socket is
    /// gone, so it never gets as far as a handler.
    #[cfg(unix)]
    struct UnreachableHandler;

    #[cfg(unix)]
    #[openvhost_proc::control::async_trait]
    impl openvhost_proc::control::ControlHandler for UnreachableHandler {
        async fn execute(
            &self,
            req: openvhost_proc::control::Request,
        ) -> openvhost_proc::control::Response {
            panic!("no request should have reached the handler, got {req:?}")
        }
    }

    /// THE A1 REGRESSION TEST — driven through the PRODUCTION WIRING SHAPE.
    ///
    /// `serve` is handed `std::future::pending::<()>()`, exactly what `lib.rs`
    /// passes, so its loop never breaks and its own `socket.remove()` is
    /// unreachable — which is the whole defect. Three existing tests missed
    /// this by passing a real shutdown future the app never passes
    /// (`openvhost-proc/tests/control.rs`, `control.rs`'s live-supervisor
    /// test, and `apps/cli/tests/two_process.rs`'s `Drop`): the mechanism was
    /// tested, the wiring was not.
    ///
    /// The assertion is deliberately not "the file is gone" alone but what
    /// that costs a user: `control::request` against this home must answer
    /// `NotRunning` (which the CLI reports as "not running", exit 0) and not
    /// `Unreachable` (exit 69, the thing spec D3 rejects and click-list item 6
    /// checks).
    ///
    /// VACUITY: remove the `ControlSocket` block from `perform_quit` and this
    /// fails on the `NotRunning` assertion with `Unreachable` — the socket is
    /// still there, and nothing is listening.
    #[cfg(unix)]
    #[tokio::test]
    async fn quitting_removes_the_control_socket_although_serve_never_stops() {
        use openvhost_proc::control::{ControlError, ControlHandler, Request};

        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Quitting::default());

        let listener = openvhost_proc::control::bind(home.path()).unwrap();
        let socket_path = listener.path().to_path_buf();
        // The one line `lib.rs` also runs, before `serve` consumes the
        // listener.
        app.manage(listener.socket());
        let handler: Arc<dyn ControlHandler> = Arc::new(UnreachableHandler);
        let server = tokio::spawn(openvhost_proc::control::serve(
            listener,
            handler,
            std::future::pending::<()>(),
        ));
        assert!(socket_path.exists(), "the socket must exist to begin with");

        // `perform_quit` cannot reach `window.destroy()` under `mock_builder`
        // (there is no "main" window), and that is fine: everything under
        // test happens strictly before it.
        let outcome = perform_quit(app.handle()).await;
        assert_eq!(outcome, Err("the main window is gone".to_string()));

        assert!(
            !socket_path.exists(),
            "the socket outlived the quit: {}",
            socket_path.display()
        );
        match openvhost_proc::control::request(home.path(), &Request::List) {
            Err(ControlError::NotRunning { .. }) => {}
            other => panic!(
                "a CLI meeting this home must be told the app is not running (exit 0), got {other:?}"
            ),
        }
        // The proof that this did NOT come from `serve` shutting down: it is
        // still sitting in its accept loop, exactly as in production.
        assert!(
            !server.is_finished(),
            "serve returned — then this test proved the wrong mechanism"
        );
        server.abort();
    }

    /// A quit marks [`Quitting`] before it does anything else, which is what
    /// lets `control::DesktopHandler` refuse a verb that raced the teardown.
    ///
    /// VACUITY: delete the `quitting.mark()` block from `perform_quit` and
    /// this fails.
    #[tokio::test]
    async fn quitting_is_marked_before_the_teardown_runs() {
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Quitting::default());
        assert!(!app.state::<Quitting>().has_begun());

        let _ = perform_quit(app.handle()).await;

        assert!(
            app.state::<Quitting>().has_begun(),
            "a quit in flight must be observable to the control channel"
        );
    }

    /// ORDERING, which is the half of the A1 fix that also closes A3's common
    /// case: the socket is gone BEFORE services start being stopped, so a
    /// control verb racing the quit meets "not running" rather than starting
    /// something whose supervisor is about to disappear.
    ///
    /// The service ignores SIGTERM, so `stop_all` is stuck for the whole
    /// grace period — a window this test observes from the outside while
    /// `perform_quit` is still inside it. A short per-spec grace keeps it to
    /// about a second.
    ///
    /// VACUITY: move the `ControlSocket` block below the `stop_all` block in
    /// `perform_quit` and this fails — the socket is still present throughout
    /// the stop window.
    #[cfg(unix)]
    #[tokio::test]
    async fn the_control_socket_is_gone_before_services_are_stopped() {
        use std::ffi::OsString;
        use std::path::PathBuf;

        use openvhost_proc::control::ControlHandler;
        use openvhost_proc::{ReadinessProbe, ServiceSpec, SpawnSpec, Supervisor, default_driver};

        const GRACE: Duration = Duration::from_millis(800);

        let home = tempfile::tempdir().unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        app.manage(Quitting::default());

        let sup = Arc::new(Supervisor::new(default_driver()));
        sup.register(ServiceSpec {
            id: "stubborn".to_string(),
            display_name: "stubborn".to_string(),
            endpoint: None,
            spawn: SpawnSpec {
                // Ignores the polite stop, so the supervisor has to wait out
                // `GRACE` and then kill it — a stop that provably takes time.
                program: PathBuf::from("/bin/sh"),
                args: vec![
                    OsString::from("-c"),
                    OsString::from("trap '' TERM; while true; do sleep 0.1; done"),
                ],
                cwd: None,
                env: vec![],
            },
            readiness: ReadinessProbe::default(),
            grace: GRACE,
        });
        sup.start("stubborn").unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !matches!(
            sup.snapshot().first().map(|s| s.state.clone()),
            Some(ServiceState::Running)
        ) {
            assert!(std::time::Instant::now() < deadline, "service never ran");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        app.manage(Arc::clone(&sup));

        let listener = openvhost_proc::control::bind(home.path()).unwrap();
        let socket_path = listener.path().to_path_buf();
        app.manage(listener.socket());
        let handler: Arc<dyn ControlHandler> = Arc::new(UnreachableHandler);
        let server = tokio::spawn(openvhost_proc::control::serve(
            listener,
            handler,
            std::future::pending::<()>(),
        ));

        let handle = app.handle().clone();
        let quit = tokio::spawn(async move { perform_quit(&handle).await });

        // Observe from OUTSIDE, while the quit is still inside `stop_all`:
        // the socket must already be gone, and the service must still be
        // pending. Both halves matter — "gone" after everything has stopped
        // would prove nothing about the order.
        let mut observed_gone_while_stopping = false;
        let deadline = std::time::Instant::now() + GRACE;
        while std::time::Instant::now() < deadline {
            let still_pending = !pending_service_ids(&sup).is_empty();
            if still_pending && !socket_path.exists() {
                observed_gone_while_stopping = true;
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let _ = quit.await;
        server.abort();

        assert!(
            observed_gone_while_stopping,
            "the socket was still present for the whole stop window — it is being removed after \
             services are stopped, not before"
        );
        assert!(!socket_path.exists());
    }
}
