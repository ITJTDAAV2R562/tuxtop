// Tuxtop — Windows shell.
//
// SCAFFOLD. This file has never been compiled: the GUI cannot be built from
// the WSL dev box (see ADR-006), so Phase 2 of docs/ROADMAP.md is where this
// first runs and gets corrected. Treat versions and API shapes here as
// intent, not as verified fact.
//
// Everything that can be tested without a window lives in `tuxtop-core` and
// is covered there. This file should stay thin — if logic is accumulating
// here, it belongs in the core crate where it can be tested.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::Manager;

#[cfg(target_os = "windows")]
use window_vibrancy::apply_mica;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let window = app
                .get_webview_window("main")
                .expect("main window is declared in tauri.conf.json");

            // Real Win11 Mica, not a CSS approximation. This is the whole
            // reason Tauri was chosen over a plain web view (ADR-003).
            //
            // Failure is non-fatal and deliberately logged rather than
            // swallowed: on Windows 10 there is no Mica, and the app should
            // still open with its own painted background.
            #[cfg(target_os = "windows")]
            if let Err(e) = apply_mica(&window, None) {
                eprintln!("mica backdrop unavailable, falling back to opaque: {e}");
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tuxtop");
}
