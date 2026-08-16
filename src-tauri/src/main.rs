// Tuxtop — Windows shell.
//
// Thin by design: everything testable lives in `tuxtop-core`. This file wires
// the sampler to a window and nothing more. If logic accumulates here, it
// belongs in the core crate where it can be tested without a GUI (ADR-006).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hosts;
mod supervisor;

use tauri::{AppHandle, Emitter, Manager};
use tuxtop_core::HostConfig;

use supervisor::Supervisor;

/// Seconds between samples. One second is the Task-Manager feel; see ADR-002.
const INTERVAL_SECS: u32 = 1;

/// The configured hosts, for the frontend to render cards from.
#[tauri::command]
fn list_hosts(app: AppHandle) -> Result<Vec<HostConfig>, String> {
    hosts::load(&app)
}

/// Add a host, persist it, and start watching immediately.
#[tauri::command]
fn add_host(
    app: AppHandle,
    sup: tauri::State<'_, Supervisor>,
    cfg: HostConfig,
) -> Result<Vec<HostConfig>, String> {
    if cfg.name.trim().is_empty() {
        return Err("host needs a name".into());
    }
    if cfg.addr.trim().is_empty() {
        return Err("host needs an address".into());
    }

    let mut all = hosts::load(&app)?;

    if all.iter().any(|h| h.name == cfg.name) {
        return Err(format!("a host named {} already exists", cfg.name));
    }

    all.push(cfg.clone());
    hosts::save(&app, &all)?;

    sup.start(app.clone(), cfg, INTERVAL_SECS);
    let _ = app.emit(supervisor::EVENT_HOSTS, &all);

    Ok(all)
}

/// Stop watching a host and forget it.
#[tauri::command]
fn remove_host(
    app: AppHandle,
    sup: tauri::State<'_, Supervisor>,
    name: String,
) -> Result<Vec<HostConfig>, String> {
    let mut all = hosts::load(&app)?;
    all.retain(|h| h.name != name);
    hosts::save(&app, &all)?;

    sup.stop(&name);
    let _ = app.emit(supervisor::EVENT_HOSTS, &all);

    Ok(all)
}

/// Which hosts currently have a live sampler task.
#[tauri::command]
fn active_hosts(sup: tauri::State<'_, Supervisor>) -> Vec<String> {
    sup.active()
}

fn main() {
    tauri::Builder::default()
        .manage(Supervisor::default())
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            add_host,
            remove_host,
            active_hosts
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            if let Some(window) = app.get_webview_window("main") {
                apply_backdrop(&window);
            }

            // Start a sampler for every configured host.
            //
            // A broken hosts.toml is reported to the frontend rather than
            // panicking: the window should open and explain itself, not fail
            // to start over a stray comma.
            match hosts::load(&handle) {
                Ok(list) => {
                    let sup = handle.state::<Supervisor>();
                    for cfg in list {
                        sup.start(handle.clone(), cfg, INTERVAL_SECS);
                    }
                }
                Err(e) => {
                    eprintln!("could not load hosts: {e}");
                    let _ = handle.emit(supervisor::EVENT_FAULT, e);
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tuxtop");
}

/// Apply the Win11 Mica backdrop.
///
/// Failure is logged, never fatal: Windows 10 has no Mica, and the page paints
/// its own background token, so the app is merely opaque rather than broken.
/// This is the one reason Tauri was chosen over a plain web view (ADR-003).
#[cfg(target_os = "windows")]
fn apply_backdrop(window: &tauri::WebviewWindow) {
    if let Err(e) = window_vibrancy::apply_mica(window, None) {
        eprintln!("mica unavailable, falling back to an opaque window: {e}");
    }
}

#[cfg(not(target_os = "windows"))]
fn apply_backdrop(_window: &tauri::WebviewWindow) {
    // Mica is Windows-only. Other platforms get the painted background.
}
