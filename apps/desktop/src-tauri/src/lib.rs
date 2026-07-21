// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenVHost desktop — Tauri entry point with typed (tauri-specta) commands.

mod commands;

use tauri_specta::{Builder, collect_commands};

/// Build the specta command collection shared by `run()` (dev-time export)
/// and the `export_bindings` test (committed-bindings regeneration) — kept
/// in one place so the two never drift apart.
fn specta_builder() -> Builder<tauri::Wry> {
    Builder::<tauri::Wry>::new().commands(collect_commands![commands::core_info])
}

pub fn run() {
    let specta_builder = specta_builder();

    // Regenerate the committed TS bindings on every dev run (debug only).
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
