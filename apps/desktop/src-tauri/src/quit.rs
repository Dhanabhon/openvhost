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
//!   which carries an `api.prevent_close()`. That is the interceptable one.
//! - macOS `Cmd+Q` / the app menu's Quit is NOT. Tauri builds a default macOS
//!   menu whose Quit is `muda::PredefinedMenuItem::quit`, wired to the native
//!   `sel!(terminate:)`, and `tao` implements no `applicationShouldTerminate:` —
//!   so the process dies before any Rust or JS handler runs. Verified by reading
//!   tauri 2.11.3, muda 0.19.3 (`PredefinedMenuItemType::Quit => sel!(terminate:)`)
//!   and tao 0.35.3 (no such selector anywhere in the tree).
//!
//! [`app_menu`] therefore replaces the default menu with the same structure minus
//! the predefined Quit, substituting a plain [`MenuItem`] that carries the
//! `Cmd+Q` accelerator and routes through `on_menu_event` like any other item.
//! Built explicitly rather than by mutating `Menu::default`'s items in place:
//! locating the predefined Quit there means indexing "last child of the first
//! submenu", which no API guarantees and a Tauri upgrade could silently move.
//!
//! ## Known exposure: `prevent_close` makes the UI load-bearing
//!
//! Both paths end in "prevent, then ask the webview". If the webview is dead the
//! dialog never renders and the app can only be Force Quit. That is the standard
//! exposure of any unsaved-changes prompt, and the alternative — a Rust-side
//! timeout that quits on its own — would quit WITHOUT the confirmation the user
//! asked for, which is worse. Left as-is, deliberately.

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
/// Must exceed `openvhost-proc`'s `GRACE_DEADLINE` (5s): `Supervisor::stop` only
/// REQUESTS a graceful stop, and the service task waits out that grace period
/// before escalating to a kill. A timeout at or under 5s would abandon exactly
/// the processes the kill was about to reap.
pub const STOP_ALL_TIMEOUT: Duration = Duration::from_secs(8);

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

/// Ask the UI to confirm a quit. Returns whether the ask can be answered: false
/// means the caller must let the close proceed rather than prevent it.
pub fn request_quit<R: Runtime>(app: &AppHandle<R>) -> bool {
    match app.try_state::<UiReady>() {
        Some(ready) if ready.is_ready() => QuitRequestedEvent {}.emit(app).is_ok(),
        // Not acked (or the state was never managed): no dialog can appear, so
        // report "cannot ask" and let the window close.
        _ => false,
    }
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
    // No early return for "nothing pending": the loop below already returns
    // immediately in that case, without sleeping, so a guard here would only
    // duplicate it.
    for id in &pending() {
        stop(id);
    }

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
/// app's handler prevents — the confirmation would loop forever. `destroy`
/// "does not emit any events and force close the window instead"
/// (tauri 2.11.3, `Window::destroy`), so it is the only exit that terminates.
pub async fn perform_quit<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
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
#[allow(clippy::unwrap_used)]
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
        // False by default is the load-bearing half: it is what makes a close
        // proceed normally before the UI has ever mounted.
        assert!(!ready.is_ready());
        ready.mark();
        assert!(ready.is_ready());
    }

    /// The timeout must be longer than the grace period the service task waits
    /// out before killing, or the wait abandons processes that were about to die.
    #[test]
    fn stop_all_timeout_outlives_the_supervisor_grace_deadline() {
        assert!(STOP_ALL_TIMEOUT > Duration::from_secs(5));
    }
}
