// Tuxtop — Windows shell.
//
// Thin by design: everything testable lives in `tuxtop-core`. This file wires
// the sampler to a window and nothing more. If logic accumulates here, it
// belongs in the core crate where it can be tested without a GUI (ADR-006).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod history_store;
mod hosts;
mod supervisor;

use tauri::{AppHandle, Emitter, Manager};
use tuxtop_core::hostlist::{effective_interval, Settings};
use tuxtop_core::HostConfig;

use history_store::{HistoryStore, HistoryUsage};
use supervisor::Supervisor;

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
    let mut all = hosts::load(&app)?;
    tuxtop_core::hostlist::add(&mut all, cfg.clone()).map_err(|e| e.to_string())?;
    hosts::save(&app, &all)?;

    // Start the sampler with the trimmed copy the list actually stored, not
    // the raw dialog input.
    let stored = all.last().cloned().expect("just pushed");
    let settings = hosts::load_settings(&app)?;
    let iv = effective_interval(&stored, &settings);
    sup.start(app.clone(), stored, iv);
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
    tuxtop_core::hostlist::remove(&mut all, &name);
    hosts::save(&app, &all)?;

    sup.stop(&name);
    sup.forget(&name);
    app.state::<HistoryStore>().forget_host(&name);
    let _ = app.emit(supervisor::EVENT_HOSTS, &all);

    Ok(all)
}

/// Persist a new card order after a drag.
///
/// `hosts.toml` is the single source of truth for ordering, so the arrangement
/// survives a restart and stays consistent with what the backend emits.
#[tauri::command]
fn reorder_hosts(
    app: AppHandle,
    names: Vec<String>,
) -> Result<Vec<HostConfig>, String> {
    let mut all = hosts::load(&app)?;
    tuxtop_core::hostlist::reorder(&mut all, &names);
    hosts::save(&app, &all)?;

    let _ = app.emit(supervisor::EVENT_HOSTS, &all);
    Ok(all)
}

/// Current settings.
#[tauri::command]
fn get_settings(app: AppHandle) -> Result<Settings, String> {
    hosts::load_settings(&app)
}

/// Replace settings and restart every host whose effective interval changed.
///
/// Only affected hosts restart. Changing the global interval when most hosts
/// carry an override should not tear down connections that were already
/// sampling at the right rate.
#[tauri::command]
fn set_settings(
    app: AppHandle,
    sup: tauri::State<'_, Supervisor>,
    settings: Settings,
) -> Result<Settings, String> {
    let mut f = hosts::load_file(&app)?;
    let before = f.settings;
    f.settings = Settings {
        interval_secs: settings.interval_secs.clamp(1, 3600),
        history_cap_mb: settings.history_cap_mb.clamp(16, 8192),
    };
    hosts::save_file(&app, &f)?;

    for h in &f.hosts {
        if effective_interval(h, &before) != effective_interval(h, &f.settings) {
            sup.start(app.clone(), h.clone(), effective_interval(h, &f.settings));
        }
    }

    let _ = app.emit(supervisor::EVENT_SETTINGS, &f.settings);
    Ok(f.settings)
}

/// Set or clear one host's interval override, restarting just that host.
#[tauri::command]
fn set_host_interval(
    app: AppHandle,
    sup: tauri::State<'_, Supervisor>,
    name: String,
    interval_secs: Option<u32>,
) -> Result<Vec<HostConfig>, String> {
    let mut f = hosts::load_file(&app)?;

    let Some(h) = f.hosts.iter_mut().find(|h| h.name == name) else {
        return Err(format!("no host named {name}"));
    };
    h.interval_secs = interval_secs.map(|v| v.clamp(1, 3600));
    let updated = h.clone();

    hosts::save_file(&app, &f)?;

    // Restart only the host that changed, at its own effective interval.
    sup.start(app.clone(), updated.clone(), effective_interval(&updated, &f.settings));

    let _ = app.emit(supervisor::EVENT_HOSTS, &f.hosts);
    Ok(f.hosts.clone())
}

/// Measured cost per host, for the settings meter.
#[tauri::command]
fn traffic_stats(sup: tauri::State<'_, Supervisor>) -> Vec<supervisor::HostTraffic> {
    sup.traffic()
}

/// A window of history for one series.
///
/// `from` and `to` are seconds before now, so the frontend can ask for "the
/// last 20 minutes" without needing the two clocks to agree. `max_points`
/// should be about the chart's pixel width: downsampling happens here, where
/// the data is, so the webview only receives what it can draw.
#[tauri::command]
fn query_history(
    store: tauri::State<'_, HistoryStore>,
    host: String,
    metric: String,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> Vec<tuxtop_core::history::Point> {
    let now = history_store::now_secs();
    let from = now.saturating_sub(from_secs_ago);
    let to = now.saturating_sub(to_secs_ago);
    store.query(&host, &metric, from, to, max_points.clamp(1, 4096))
}

/// Several series for one host in a single call.
///
/// A 32-core host needs 32 series to draw its per-core grid; asking one at a
/// time would be 32 round trips per redraw for data that all lives behind the
/// same lock.
#[tauri::command]
fn query_history_many(
    store: tauri::State<'_, HistoryStore>,
    host: String,
    metrics: Vec<String>,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> std::collections::HashMap<String, Vec<tuxtop_core::history::Point>> {
    let now = history_store::now_secs();
    let from = now.saturating_sub(from_secs_ago);
    let to = now.saturating_sub(to_secs_ago);
    let budget = max_points.clamp(1, 4096);

    metrics
        .into_iter()
        .map(|m| {
            let pts = store.query(&host, &m, from, to, budget);
            (m, pts)
        })
        .collect()
}

/// How much the history store is currently holding.
#[tauri::command]
fn history_usage(store: tauri::State<'_, HistoryStore>) -> HistoryUsage {
    store.usage()
}

/// Which hosts currently have a live sampler task.
#[tauri::command]
fn active_hosts(sup: tauri::State<'_, Supervisor>) -> Vec<String> {
    sup.active()
}

fn main() {
    tauri::Builder::default()
        .manage(Supervisor::default())
        .manage(HistoryStore::new())
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            add_host,
            remove_host,
            reorder_hosts,
            get_settings,
            set_settings,
            set_host_interval,
            traffic_stats,
            query_history,
            query_history_many,
            history_usage,
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
            match hosts::load_file(&handle) {
                Ok(f) => {
                    let sup = handle.state::<Supervisor>();
                    for cfg in f.hosts {
                        let iv = effective_interval(&cfg, &f.settings);
                        sup.start(handle.clone(), cfg, iv);
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
