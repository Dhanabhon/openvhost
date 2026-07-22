// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenVHost desktop — Tauri entry point with typed (tauri-specta) commands
//! and events. The supervisor lives here as managed state; openvhost-proc
//! stays tauri-free and this crate owns the bridge.

mod commands;

#[cfg(target_os = "macos")]
mod stack;

use std::ffi::OsString;
use std::sync::Arc;

use openvhost_proc::{ServiceSpec, SpawnSpec, Supervisor, SupervisorEvent, default_driver};
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
        ])
        .events(collect_events![
            commands::ServiceStateEvent,
            commands::ServiceLogEvent
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

    let result = tauri::Builder::default()
        .invoke_handler(specta_builder.invoke_handler())
        .setup(move |app| {
            specta_builder.mount_events(app);
            let supervisor = Arc::new(Supervisor::new(default_driver()));
            supervisor.register(demo_ticker_spec());
            #[cfg(target_os = "macos")]
            for spec in stack::macos_stack_specs() {
                supervisor.register(spec);
            }
            let mut rx = supervisor.subscribe();
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match rx.recv().await {
                        Ok(SupervisorEvent::StateChanged { id, state, detail }) => {
                            let _ = commands::ServiceStateEvent { id, state, detail }.emit(&handle);
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
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
            app.manage(supervisor);
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
