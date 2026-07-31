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
// `tauri_specta::Event`, not `tauri::Emitter`: the emit method on a
// `#[derive(tauri_specta::Event)]` type comes from this trait, and it is what
// keeps the event name in sync with the generated TS binding.
use tauri_specta::Event as _;

/// Menu-item id for the substituted Quit. Namespaced so it cannot collide with a
/// predefined item's generated id.
pub const QUIT_MENU_ITEM_ID: &str = "openvhost:quit";

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
/// (security audit finding M3, 2026-07-31 — this used to send `stop` once, to
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
    // FIRST, before touching services or the window at all — this is the C1
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

    /// THE M3 REGRESSION TEST (security audit finding M3, 2026-07-31): a bulk
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
}
