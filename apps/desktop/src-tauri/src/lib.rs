// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenVHost desktop — Tauri entry point with typed (tauri-specta) commands
//! and events. The supervisor lives here as managed state; openvhost-proc
//! stays tauri-free and this crate owns the bridge.

// Putting the `openvhost` CLI on the user's PATH (P1 CLI-install design):
// resolve the sibling binary, classify what is already at
// `<candidate>/openvhost`, and install a symlink atomically. `pub` because
// nothing calls it yet — the menu item that will is a later task, and a
// private module of unreachable `pub fn`s would be dead code under
// `-D warnings`. No Tauri command, no capability change: the menu handler
// calls it directly, exactly as the tray's handlers do.
pub mod clitool;
mod commands;
// Desktop-side policy for the local control socket the `openvhost` CLI talks
// to (P1 CLI design). Transport/authorization live in
// `openvhost_proc::control`; this module is only the `ControlHandler` impl
// over the supervisor and the two bulk locks. Ungated: the handler is
// portable, and `openvhost_proc::control::bind` is what refuses off-unix.
mod control;
// The MySQL admin-CLI spawns (`mysqladmin`/`mysql` — ping, ALTER USER,
// shutdown): orchestration-layer child processes, not config generation —
// see this module's own doc comment for why they live here rather than in
// openvhost-conf (review fix wave finding 4).
mod mysql_admin;
mod quit;

// Ungated: `stack::StackPaths` is a portable type named by `commands.rs` on
// every target. Only the macOS stack BUILDER inside is `#[cfg]`-gated.
mod stack;

// The tray/menu-bar quick-controls slice (P1 tray design): the pure menu
// model, the diff/apply logic, the real tray construction, and the shared
// menu-event router — see its own module docs for the breakdown.
mod tray;

// Demo-ticker only (spec D8) — see `demo_ticker_spec`'s own `#[cfg(debug_assertions)]`
// gate for why this whole import is gated identically: a release build never
// constructs a `ServiceSpec` for it at all, so these would otherwise be
// unused-import errors under `-D warnings` in exactly the profile this gate
// targets.
#[cfg(debug_assertions)]
use std::ffi::OsString;
use std::sync::Arc;

#[cfg(debug_assertions)]
use openvhost_proc::{DEFAULT_GRACE, ReadinessProbe, ServiceSpec, SpawnSpec};
use openvhost_proc::{
    FileRegistry, InstanceLock, Supervisor, SupervisorEvent, default_driver, default_reaper,
};
use tauri::Manager;
use tauri_specta::{Builder, Event, collect_commands, collect_events};

/// Build the specta command collection shared by `run()` (dev-time export)
/// and the `export_bindings` test (committed-bindings regeneration) — kept
/// in one place so the two never drift apart.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new()
        .commands(collect_commands![
            commands::core_info,
            commands::list_services,
            commands::start_service,
            commands::stop_service,
            commands::service_log_tail,
            commands::list_sites,
            commands::create_site,
            commands::update_site,
            commands::delete_site,
            commands::list_web_servers,
            commands::read_web_server_config,
            commands::validate_web_server_config,
            commands::confirm_quit,
            commands::quit_dialog_ready,
            commands::services_memory,
            commands::home_disk_usage,
            commands::open_site,
            commands::open_homebrew_site,
            commands::plan_config_apply,
            commands::apply_config,
            commands::web_server_settings,
            commands::save_web_server_settings,
            commands::php_environment,
            commands::rescan_php_runtimes,
            commands::install_php,
            commands::pending_install,
            commands::mysql_environment,
            commands::rescan_mysql,
            commands::install_mysql,
            commands::initialize_mysql,
            commands::mysql_root_password,
            commands::reset_mysql_root_password,
            commands::verify_mysql_connection,
            commands::list_log_sources,
            commands::read_log_window,
            commands::reveal_log_folder,
        ])
        .events(collect_events![
            commands::ServiceStateEvent,
            commands::ServiceLogEvent,
            commands::ServiceRegisteredEvent,
            commands::PhpInstallLogEvent,
            commands::MysqlInstallLogEvent,
            commands::MysqlInitLogEvent,
            quit::QuitRequestedEvent
        ])
        // `LogLine`/`ServiceLogEvent` carry `ts_ms: u64` (millisecond epoch
        // timestamps). Specta forbids exporting BigInt-style Rust types to
        // TS by default (precision loss for arbitrary u64), but Tauri IPC
        // is JSON underneath: the wire value is already a JS `number`, never
        // a native `bigint`. Casting the TS type to `number` matches runtime
        // reality; ts_ms is milliseconds-since-epoch, far below
        // Number.MAX_SAFE_INTEGER for a very long time.
        // WARNING: this flag is BUILDER-GLOBAL — it remaps every u64/i64/
        // usize/isize across the whole exported command/event surface, not
        // just ts_ms. Any future large-integer field (byte totals, counters)
        // must be consciously checked to stay < 2^53, or use a lossless
        // encoding (string) instead of relying on this cast.
        .dangerously_cast_bigints_to_number()
}

/// Dev convenience: the demo ticker runs the openvhost CLI sitting next to
/// this executable in target/. A missing binary is an HONEST Failed state
/// in the UI (the spawn-failure log names the path), not a crash.
///
/// `#[cfg(debug_assertions)]` (P1 tray design, spec D8): this service
/// deliberately fails after 45 ticks to exercise the `Failed` UI path in
/// development. Registering it unconditionally — as this crate did before
/// this gate — meant a RELEASE build shipped it too, so every real user's
/// tray (and Services page) would show "demo-ticker — Failed" once the
/// counter ran out. `debug_assertions`, not `cfg(test)`: `cargo test`
/// itself compiles with `debug_assertions` on (dev/test profiles share it),
/// so this stays registered for tests and ordinary `cargo run`/`tauri dev`
/// and disappears ONLY from a `--release` build — exactly the one profile
/// real users run.
#[cfg(debug_assertions)]
fn demo_ticker_spec() -> ServiceSpec {
    let cli = std::env::current_exe()
        .ok()
        .and_then(|p| {
            p.parent().map(|d| {
                d.join(if cfg!(windows) {
                    "openvhost.exe"
                } else {
                    "openvhost"
                })
            })
        })
        .unwrap_or_else(|| std::path::PathBuf::from("openvhost"));
    ServiceSpec {
        id: "demo-ticker".into(),
        display_name: "demo ticker".into(),
        endpoint: Some("__testchild · 1s interval · fails after 45 ticks".into()),
        spawn: SpawnSpec {
            program: cli,
            args: [
                "__testchild",
                "--lines",
                "100000",
                "--interval-ms",
                "1000",
                "--fail-after",
                "45",
            ]
            .iter()
            .map(OsString::from)
            .collect(),
            cwd: None,
            env: vec![],
        },
        // Defaults only — see `stack::php_fpm_spec`'s matching comment.
        readiness: ReadinessProbe::default(),
        grace: DEFAULT_GRACE,
    }
}

pub fn run() {
    let specta_builder = specta_builder();

    #[cfg(debug_assertions)]
    if let Err(e) = specta_builder.export(
        specta_typescript::Typescript::default(),
        "../src/lib/ipc/bindings.ts",
    ) {
        eprintln!("fatal: failed to export TS bindings: {e}");
        std::process::exit(1);
    }

    let mut builder = tauri::Builder::default()
        .invoke_handler(specta_builder.invoke_handler())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init());

    // Substitute the app menu so macOS Cmd+Q is interceptable at all — see
    // `quit`'s module docs: the DEFAULT menu's Quit is wired to the native
    // `terminate:` selector and kills the process before any handler runs.
    // Non-macOS gets no default menu from Tauri, so there is nothing to replace.
    #[cfg(target_os = "macos")]
    {
        builder = builder.menu(quit::app_menu);
    }

    let app = builder
        // `tray::handle_tray_menu_id` is the ONE router for every menu
        // event this app receives (see its own doc comment for why a
        // function named for the tray also handles the app-menu Quit
        // click): it still special-cases `quit::QUIT_MENU_ITEM_ID` with
        // exactly the ask-unless-never-acked logic this closure used to
        // inline directly, and P1 tray design's real tray menu (built in
        // `setup()` below) shares that same id for its own Quit row.
        .on_menu_event(|app, event| tray::handle_tray_menu_id(app, event.id().as_ref()))
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Hide rather than quit (P1 tray design, spec D1): the app and
                // its services keep running; a Dock click or the tray's "Open
                // OpenVHost" bring the window back via `RunEvent::Reopen`
                // below. This never touches the webview — see
                // `quit::hide_instead_of_close`'s docs for why that is the
                // point. Only prevent the close if the hide actually
                // succeeded: a failed hide must not trap the user in an app
                // whose close button no longer works.
                //
                // macOS-ONLY (security audit finding H1, 2026-07-31). Every
                // way back from a hidden window is itself macOS-only this
                // slice: `builder.menu(quit::app_menu)` above, the real tray
                // built in `setup()` below, and `RunEvent::Reopen` further
                // down (spec D10 — Windows tray icons are a later slice; see
                // `tray`'s own module docs). Tauri also installs its default
                // menu only on macOS. Hiding the ONLY window on a platform
                // with none of those would make the app simultaneously
                // unreachable (no Dock/tray/Reopen to bring it back, and a
                // hidden window has no taskbar entry either) and unquittable
                // (the webview `confirm_quit` needs is itself hidden) — worse,
                // the zombie process keeps holding the single-instance run
                // lock forever, so every later launch attempt boots
                // permanently degraded instead of replacing it.
                #[cfg(target_os = "macos")]
                if let Err(e) =
                    quit::hide_instead_of_close(|| window.hide(), || api.prevent_close())
                {
                    eprintln!(
                        "openvhost: failed to hide the window ({e}); letting the close proceed"
                    );
                }
                // Everywhere else: the pre-tray behaviour, unchanged since it
                // shipped in PR #19 — this is a straight re-scoping, not new
                // logic. Ask exactly like the app-menu Quit does (`request_quit`
                // shows and focuses the window, then emits `QuitRequestedEvent`
                // for the webview to answer via `confirm_quit`); prevent the
                // close only if the UI actually acked and can answer. If the UI
                // never came up, let the close proceed exactly as it did before
                // the confirm-quit feature existed, rather than wedging on a
                // dialog that can never render — this deliberately does NOT
                // fall through to `perform_quit` the way the menu handler does,
                // matching this path's own historical behaviour on every
                // platform before the tray slice introduced hiding.
                #[cfg(not(target_os = "macos"))]
                if quit::request_quit(window.app_handle()) {
                    api.prevent_close();
                }
            }
        })
        .setup(move |app| {
            specta_builder.mount_events(app);

            // Managed unconditionally, BEFORE the bootstrap below that can bail
            // out early: `request_quit` treats a missing `UiReady` as "cannot
            // ask", so failing to manage it on some path would silently disable
            // the confirmation instead of failing loudly.
            app.manage(quit::UiReady::default());

            // A2: serializes `apply_config` end to end (plan -> commit -> validate
            // -> restart) so two overlapping Apply calls cannot interleave their
            // commit/rollback or their stop/start of the same services. Managed
            // unconditionally and up front, same reasoning as `UiReady` above —
            // `apply_config` also requires `Db`/`Arc<Supervisor>`/`Option<StackPaths>`
            // to be managed before it is reachable at all, so this is never
            // observed absent by a caller that could actually invoke the command.
            app.manage(commands::ApplyLock::default());

            // Serializes `install_php`: only one brew install can run at a
            // time. Same unconditional-and-up-front reasoning as `ApplyLock`
            // above — `install_php` also requires `Db`-adjacent state
            // (`Arc<Supervisor>`, `Option<StackPaths>`, the runtimes
            // `RwLock`) to be managed before it is reachable at all, so this
            // is never observed absent by a caller that could actually
            // invoke the command.
            app.manage(commands::InstallLock::default());

            // Bulk Start-all/Stop-all admission guard (P1 tray design, spec
            // D7) and the tray-initiated-failure tracking set (spec D4).
            // Managed unconditionally and up front, same reasoning as
            // `UiReady`/`ApplyLock`/`InstallLock` above: both are only ever
            // READ by `tray::handle_tray_menu_id`'s Start-all/Stop-all/
            // per-service branches, which in practice are only ever reached
            // via a real tray click (macOS-only, built further down) — but
            // managing them here rather than inside that `#[cfg(target_os =
            // "macos")]` block keeps every `try_state` read in this app
            // failing closed the same way, and keeps them reachable from a
            // test's own `mock_builder` setup without needing a real tray.
            app.manage(tray::BulkLock::default());
            app.manage(tray::TrayInitiated::default());

            // Set by `perform_quit` before it removes the control socket, and
            // read by `control::DesktopHandler` to refuse mutating verbs from
            // an `openvhost` invocation that raced the quit (P1 CLI design,
            // A3 audit fix). Managed unconditionally and up front for the same
            // reason as everything above: every `try_state` read in this app
            // fails closed the same way, and a test's own `mock_builder` setup
            // can reach it.
            app.manage(quit::Quitting::default());

            // Single-instance lock (design spec §7): reap MUST run only
            // while this is held, otherwise a second live instance would
            // reap the first's HEALTHY services (identity matches — it
            // really is their process — but the "orphan" premise is false).
            match openvhost_core::resolve_home() {
                Ok(home) => {
                    let run_dir = home.join("run");
                    match InstanceLock::acquire(&run_dir) {
                        Ok(Some(lock)) => {
                            // Keep the lock alive for the app's lifetime — dropping
                            // it releases the flock and lets a later instance
                            // acquire it.
                            app.manage(lock);
                            // Open the persistent state store best-effort: a
                            // missing/unreadable state.db must never stop the
                            // supervisor from starting. Sites features are
                            // simply unavailable this run (no IPC command
                            // reads `Db` yet — that lands with the Sites UI).
                            match tauri::async_runtime::block_on(openvhost_core::Db::open(&home)) {
                                Ok(db) => {
                                    app.manage(db);
                                }
                                Err(e) => {
                                    eprintln!(
                                        "openvhost: state.db unavailable ({e}); Sites features disabled this run"
                                    );
                                }
                            }
                            let registry = Arc::new(FileRegistry::new(&run_dir));
                            let supervisor = Arc::new(Supervisor::with_orphan_cleanup(
                                default_driver(),
                                registry,
                                default_reaper(),
                            ));
                            #[cfg(debug_assertions)]
                            supervisor.register(demo_ticker_spec());
                            #[cfg(target_os = "macos")]
                            let (stack_paths, stack_runtimes, mysql_runtimes) = {
                                let stack = stack::macos_stack();
                                for spec in stack.specs {
                                    supervisor.register(spec);
                                }
                                (stack.paths, stack.runtimes, stack.mysql_runtimes)
                            };
                            // No stack builder for this target yet, so `None` is the
                            // NORMAL state here — the home resolved fine, there is
                            // simply nothing to point the Web Server page at. See
                            // `commands::stack_paths` for the message that renders.
                            #[cfg(not(target_os = "macos"))]
                            let (stack_paths, stack_runtimes, mysql_runtimes): (
                                Option<stack::StackPaths>,
                                Option<openvhost_core::InstalledRuntimes>,
                                Option<Vec<openvhost_core::mysql::MysqlRuntime>>,
                            ) = (None, None, None);
                            // Manage the Option ITSELF, unconditionally. Tauri implements
                            // `CommandArg` only for `State<'r, T>` — there is no impl for
                            // `Option<State<'r, T>>` — so a command cannot take an
                            // optionally-managed state. Making `Option<StackPaths>` the
                            // managed type is what lets a command distinguish "no stack on
                            // this platform" from "not wired up", while always having
                            // something to extract.
                            //
                            // Exactly ONE `manage` call per state type: `Manager::manage`
                            // does NOT overwrite an existing value (its own doc example
                            // asserts `assert!(!app.manage(MyInt(1)))`), so a "manage None
                            // early, the real value later" split would silently pin every
                            // user to `None`.
                            app.manage(stack_paths);
                            // Same `Option<T>`-managed-unconditionally shape as `stack_paths`
                            // above, for the same reason: `Manager::manage` never overwrites,
                            // so every arm must yield a value rather than some arms skipping
                            // the call. `None` on a target with no stack builder, or when the
                            // php-fpm version could not be probed (see `stack::macos_stack`'s
                            // doc comment) — either way a later command that reads this state
                            // sees an honest absence rather than a stale value from a call
                            // that never happened.
                            //
                            // Wrapped in an `RwLock` (unlike `stack_paths` above): Tauri's
                            // managed state cannot be replaced once set, but the installed PHP
                            // runtimes CAN change after launch — the Languages page installs a
                            // version at runtime, and the apply pipeline must see it without a
                            // relaunch. The lock is the seam a later rescan/install writes
                            // through; every reader here just takes the read side.
                            app.manage(std::sync::RwLock::new(stack_runtimes));
                            // Same reasoning as `stack_runtimes` above, for MySQL's own
                            // runtime list (P1 MySQL lifecycle design): `install_mysql`/
                            // `rescan_mysql` write through this after launch, and
                            // `initialize_mysql`/`reset_mysql_root_password`/
                            // `verify_mysql_connection` read it rather than re-probing.
                            app.manage(std::sync::RwLock::new(mysql_runtimes));
                            let mut rx = supervisor.subscribe();
                            let handle = app.handle().clone();
                            tauri::async_runtime::spawn(async move {
                                loop {
                                    match rx.recv().await {
                                        Ok(SupervisorEvent::StateChanged { id, state, detail }) => {
                                            let _ = commands::ServiceStateEvent { id, state, detail }
                                                .emit(&handle);
                                        }
                                        Ok(SupervisorEvent::Log {
                                            id,
                                            ts_ms,
                                            level,
                                            line,
                                        }) => {
                                            let _ = commands::ServiceLogEvent {
                                                id,
                                                ts_ms,
                                                level,
                                                line,
                                            }
                                            .emit(&handle);
                                        }
                                        Ok(SupervisorEvent::Registered { status }) => {
                                            let _ = commands::ServiceRegisteredEvent { status }
                                                .emit(&handle);
                                        }
                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                            continue;
                                        }
                                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                    }
                                }
                            });
                            // Best-effort, like the state.db open above: a
                            // menu-bar tray is a quality-of-life feature, not
                            // a boot-blocking one, so a failure here is
                            // logged and the app continues without it rather
                            // than aborting the whole bootstrap. Gated to
                            // macOS only (P1 tray design, spec D10 — Windows
                            // is out for this slice; see `tray`'s own module
                            // docs for what a Windows-enablement slice still
                            // needs to do). `Arc::clone`, not a move: `Db`
                            // was NOT similarly needed again, but `supervisor`
                            // itself is moved into `app.manage` on the very
                            // next line.
                            #[cfg(target_os = "macos")]
                            if let Err(e) = tray::build(app.handle(), Arc::clone(&supervisor)) {
                                eprintln!(
                                    "openvhost: failed to build the tray icon ({e}); continuing without it"
                                );
                            }

                            // The local control socket the `openvhost` CLI
                            // connects to (P1 CLI design, spec D1). Bound
                            // INSIDE this arm on purpose: the socket must
                            // exist if and only if a supervisor does, so the
                            // degraded-boot arms below — instance lock held
                            // elsewhere, or no resolvable home — deliberately
                            // do NOT bind, and a CLI meeting no socket
                            // correctly reports "the app is not running"
                            // rather than reaching a second, supervisor-less
                            // instance.
                            //
                            // Best-effort, exactly like the state.db open and
                            // the tray above: a control socket is how a
                            // terminal drives this app, not how the app
                            // works. A bind failure (a stale non-socket file
                            // at the path, an over-long OPENVHOST_HOME, a
                            // non-unix target) is logged and the GUI carries
                            // on.
                            //
                            // `bind` deliberately returns a wrapper around a
                            // STD listener: this closure is not running
                            // inside a tokio runtime, and
                            // `tokio::net::UnixListener::bind` panics there.
                            // `serve` — spawned onto tauri's runtime below —
                            // is what converts it. `std::future::pending()`
                            // means "serve for the process lifetime": there
                            // is no orderly-shutdown event, only a quit.
                            //
                            // Which is exactly why the socket's IDENTITY is
                            // managed here, before `serve` consumes the
                            // listener (A1 audit fix). `serve`'s own unlink
                            // sits after a loop this future never lets break,
                            // so it does not run in this app — and a unix
                            // socket is not unlinked when its process exits.
                            // Left behind, the path outlives the app and the
                            // next `openvhost status` gets ECONNREFUSED and
                            // reports "not accepting control connections"
                            // (exit 69) instead of the truthful "not running"
                            // (exit 0). `quit::perform_quit` removes it
                            // through this managed handle, first thing.
                            match openvhost_proc::control::bind(&home) {
                                Ok(listener) => {
                                    app.manage(listener.socket());
                                    let handler: Arc<
                                        dyn openvhost_proc::control::ControlHandler,
                                    > = Arc::new(control::DesktopHandler::new(
                                        app.handle().clone(),
                                        Arc::clone(&supervisor),
                                    ));
                                    tauri::async_runtime::spawn(
                                        openvhost_proc::control::serve(
                                            listener,
                                            handler,
                                            std::future::pending::<()>(),
                                        ),
                                    );
                                }
                                Err(e) => {
                                    eprintln!(
                                        "openvhost: control socket unavailable ({e}); the openvhost CLI cannot reach this instance"
                                    );
                                }
                            }
                            app.manage(supervisor);
                        }
                        Ok(None) => {
                            eprintln!(
                                "openvhost: another instance holds the run lock; not starting the supervisor"
                            );
                        }
                        Err(e) => {
                            eprintln!("openvhost: failed to acquire the run lock: {e}");
                        }
                    }
                }
                Err(e) => {
                    // Fail CLOSED (P0-8 merge-gate fix wave C5): no
                    // cwd-relative "./run" fallback. A relative run dir would
                    // lock/reap against whatever directory the OS happened to
                    // launch us from instead of the real OPENVHOST_HOME —
                    // silently wrong identity for both the single-instance
                    // lock and the orphan registry. Same posture as the
                    // lock-contended arm above: skip the supervisor bootstrap
                    // entirely rather than proceed on a guessed path.
                    eprintln!(
                        "openvhost: cannot resolve OPENVHOST_HOME ({e}); not starting the supervisor"
                    );
                }
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .unwrap_or_else(|e| {
            // Same fatal-error text and exit code the old `.run(ctx)` call
            // used for its `Result` — `Builder::run` is documented as
            // exactly `self.build(context)?.run(|_, _| {})`, so splitting it
            // into `.build(ctx)?.run(closure)` moves this failure from
            // `.run`'s Result to `.build`'s without changing what it means
            // or how it is reported.
            eprintln!("fatal: tauri failed to run: {e}");
            std::process::exit(1);
        });

    // `.build(ctx)?.run(closure)` rather than `.run(ctx)`: this is what
    // exposes `RunEvent`, needed for `Reopen` below (P1 tray design, spec
    // D1) — `Builder::run` only ever calls `App::run(|_, _| {})`, a closure
    // this crate cannot reach into.
    // `_handle`/`_event`, not `handle`/`event`: the whole body below is
    // `#[cfg(target_os = "macos")]`, so on every other target this closure
    // has an empty body and both parameters go unused — the underscore
    // prefix is what keeps that warning-free (it only suppresses the
    // unused-variable lint; both names remain fully usable, and ARE used,
    // in the macOS-only block).
    app.run(|_handle, _event| {
        // `RunEvent` is `#[non_exhaustive]` (verified in the resolved tauri
        // 2.11.5, `app.rs:219`) and this app reacts to `Reopen` only — every
        // other variant, present or future, is intentionally left alone,
        // exactly the posture tauri's own `App::run` doc example takes.
        //
        // `#[cfg]` on this whole `if let` rather than on a `match` arm is
        // deliberate, not cosmetic: `Reopen` itself only exists on macOS
        // (gated the same way upstream), so a `match` with a
        // `#[cfg(target_os = "macos")]` arm plus a wildcard would compile,
        // on every OTHER target, down to a match with a single `_ => {}`
        // arm — exactly the shape clippy's `match_single_binding` flags.
        // This crate cannot verify a non-macOS clippy run from a macOS
        // sandbox, so the body is structured to make that failure mode
        // impossible rather than to hope the lint does not fire.
        #[cfg(target_os = "macos")]
        if let tauri::RunEvent::Reopen { .. } = _event
            && let Some(window) = _handle.get_webview_window("main")
        {
            reopen_window(
                || {
                    let _ = window.show();
                },
                || {
                    let _ = window.unminimize();
                },
                || {
                    let _ = window.set_focus();
                },
            );
        }
    });
}

/// The window actions a Dock click or the tray's "Open OpenVHost" perform on
/// [`tauri::RunEvent::Reopen`] (P1 tray design, spec D1): show, un-minimize,
/// then focus, in that order, so a window that is merely hidden (the common
/// case now that closing hides rather than quits) or one that was minimized
/// before being hidden both end up visible, restored, and frontmost.
///
/// Takes closures rather than a real `WebviewWindow`, mirroring every other
/// lifecycle decision in this app (see `quit::stop_all_with`'s doc comment):
/// `tauri::test`'s mock window dispatcher stubs `is_visible`/`is_focused` to
/// fixed values regardless of what is actually called on it (verified against
/// the resolved tauri 2.11.5), so asserting window STATE after calling the
/// real methods would pass or fail independent of this function. The ORDER —
/// the part a future edit could accidentally shuffle or drop a call from — is
/// what this split makes testable.
///
/// Each call is independent and best-effort: there is no confirmation to
/// answer and nothing to abort if revealing the window does not fully
/// succeed, so (mirroring `perform_quit`'s own straggler handling) a failure
/// in one must not skip the other two.
///
/// `#[cfg(target_os = "macos")]`, matching its only call site above and
/// `quit::app_menu`'s own precedent: `RunEvent::Reopen` exists only on macOS
/// (upstream gates it the same way), so on every other target this function
/// would otherwise be unused — dead code, an error under this workspace's
/// `-D warnings` — outside of a test build's own separately-gated caller.
#[cfg(target_os = "macos")]
fn reopen_window(show: impl FnOnce(), unminimize: impl FnOnce(), focus: impl FnOnce()) {
    show();
    unminimize();
    focus();
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    // Only `reopen_window_shows_then_unminimizes_then_focuses` below needs
    // this, and that test is itself `#[cfg(target_os = "macos")]` — gated
    // identically here so the import is not flagged as unused elsewhere.
    #[cfg(target_os = "macos")]
    use std::sync::Mutex;

    /// Regenerate the committed TS bindings headlessly (no GUI needed):
    /// `cargo test -p openvhost-desktop export_bindings`.
    #[test]
    fn export_bindings() {
        specta_builder()
            .export(
                specta_typescript::Typescript::default(),
                "../src/lib/ipc/bindings.ts",
            )
            .expect("failed to export TS bindings");
    }

    /// VACUITY (neuter-and-watch-it-fail): temporarily reordered
    /// `reopen_window`'s body to `focus(); show(); unminimize();` — this
    /// test failed, recording `["focus", "show", "unminimize"]` against the
    /// asserted `["show", "unminimize", "focus"]`. Restoring the original
    /// order made it pass again.
    #[cfg(target_os = "macos")]
    #[test]
    fn reopen_window_shows_then_unminimizes_then_focuses() {
        let order: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
        reopen_window(
            || order.lock().expect("mutex poisoned").push("show"),
            || order.lock().expect("mutex poisoned").push("unminimize"),
            || order.lock().expect("mutex poisoned").push("focus"),
        );
        assert_eq!(
            order.lock().expect("mutex poisoned").as_slice(),
            ["show", "unminimize", "focus"]
        );
    }
}
