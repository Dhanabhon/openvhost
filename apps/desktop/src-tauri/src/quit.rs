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
/// Must exceed `openvhost-proc`'s `DEFAULT_GRACE` (5s, formerly a private
/// `GRACE_DEADLINE` constant — now per-`ServiceSpec`, see `ServiceSpec::grace`):
/// `Supervisor::stop` only REQUESTS a graceful stop, and the service task waits
/// out that spec's grace period before escalating to a kill. A timeout at or
/// under 5s would abandon exactly the processes the kill was about to reap.
///
/// nginx/php-fpm both still use `DEFAULT_GRACE`, so 8s stays correct for them.
/// A future spec with a LONGER grace (the roadmap's MySQL lifecycle slice
/// proposes 15s, for a clean InnoDB shutdown) would need this timeout raised
/// too, or `stop_all_with` would report it as a straggler mid-shutdown even
/// though it was still within its own grace window — flagged here rather than
/// silently left for that slice to rediscover.
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
/// app's handler prevents — the confirmation would loop forever. `destroy`
/// "does not emit any events and force close the window instead"
/// (tauri 2.11.3, `Window::destroy`), so it is the only exit that terminates.
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
        lock.inner()
            .set_running("8.4".to_string(), install_task.abort_handle());

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
