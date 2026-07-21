// SPDX-License-Identifier: GPL-3.0-or-later
//! OpenServ desktop — Tauri entry point. Command surface arrives in Task 5.

pub fn run() {
    let result = tauri::Builder::default().run(tauri::generate_context!());
    if let Err(e) = result {
        eprintln!("fatal: tauri failed to run: {e}");
        std::process::exit(1);
    }
}
