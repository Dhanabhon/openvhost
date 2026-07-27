// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenVHost desktop — Tauri entry point with typed (tauri-specta) commands
//! and events. The supervisor lives here as managed state; openvhost-proc
//! stays tauri-free and this crate owns the bridge.

mod commands;
mod quit;

// Ungated: `stack::StackPaths` is a portable type named by `commands.rs` on
// every target. Only the macOS stack BUILDER inside is `#[cfg]`-gated.
mod stack;

use std::ffi::OsString;
use std::sync::Arc;

use openvhost_proc::{
    FileRegistry, InstanceLock, ServiceSpec, SpawnSpec, Supervisor, SupervisorEvent,
    default_driver, default_reaper,
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
            commands::plan_site_apply,
            commands::apply_sites,
        ])
        .events(collect_events![
            commands::ServiceStateEvent,
            commands::ServiceLogEvent,
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

    let result = builder
        .on_menu_event(|app, event| {
            if event.id() == quit::QUIT_MENU_ITEM_ID {
                // Ask, don't quit — unless the UI has never acked (see
                // `quit::UiReady`), in which case no dialog can appear and
                // asking would make the app unquittable from its own menu.
                if !quit::request_quit(app) {
                    let handle = app.clone();
                    tauri::async_runtime::spawn(async move {
                        if let Err(e) = quit::perform_quit(&handle).await {
                            eprintln!("openvhost: quit failed: {e}");
                        }
                    });
                }
            }
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                // Prevent only if the UI has acked that it is listening (see
                // `quit::UiReady`). Before that, closing behaves exactly as it
                // did before this feature rather than wedging on a dialog that
                // will never render.
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

            // A2: serializes `apply_sites` end to end (plan -> commit -> validate
            // -> restart) so two overlapping Apply calls cannot interleave their
            // commit/rollback or their stop/start of the same services. Managed
            // unconditionally and up front, same reasoning as `UiReady` above —
            // `apply_sites` also requires `Db`/`Arc<Supervisor>`/`Option<StackPaths>`
            // to be managed before it is reachable at all, so this is never
            // observed absent by a caller that could actually invoke the command.
            app.manage(commands::ApplyLock::default());

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
                            supervisor.register(demo_ticker_spec());
                            #[cfg(target_os = "macos")]
                            let (stack_paths, stack_runtimes) = {
                                let stack = stack::macos_stack();
                                for spec in stack.specs {
                                    supervisor.register(spec);
                                }
                                (stack.paths, stack.runtimes)
                            };
                            // No stack builder for this target yet, so `None` is the
                            // NORMAL state here — the home resolved fine, there is
                            // simply nothing to point the Web Server page at. See
                            // `commands::stack_paths` for the message that renders.
                            #[cfg(not(target_os = "macos"))]
                            let (stack_paths, stack_runtimes): (
                                Option<stack::StackPaths>,
                                Option<openvhost_core::InstalledRuntimes>,
                            ) = (None, None);
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
                                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                            continue;
                                        }
                                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                    }
                                }
                            });
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
        .run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("fatal: tauri failed to run: {e}");
        std::process::exit(1);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

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
}
