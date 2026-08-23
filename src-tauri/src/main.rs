// Tuxtop — Windows shell.
//
// Thin by design: everything testable lives in `tuxtop-core`. This file wires
// the sampler to a window and nothing more. If logic accumulates here, it
// belongs in the core crate where it can be tested without a GUI (ADR-006).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod hosts;

/// Event names the webview subscribes to.
///
/// Strings, because that is what Tauri's event system takes. The supervisor
/// emits a typed `Event` and knows nothing about them - which is what lets the
/// same supervisor feed a browser over HTTP later.
const EVENT_SAMPLE: &str = "tuxtop://sample";
const EVENT_FAULT: &str = "tuxtop://fault";
const EVENT_HOSTS: &str = "tuxtop://hosts-changed";
const EVENT_SETTINGS: &str = "tuxtop://settings-changed";
const EVENT_PROCS: &str = "tuxtop://processes";

/// The fault payload the frontend expects: the host name beside the reason.
///
/// A bare fault cannot be attributed to a card, and attributing one to the
/// wrong card is worse than dropping it.
#[derive(Clone, serde::Serialize)]
struct FaultEvent {
    host: String,
    #[serde(flatten)]
    fault: tuxtop_core::HostFault,
}

use tauri::{AppHandle, Emitter, Manager};
use tuxtop_core::hostlist::{effective_interval, Settings};
use tuxtop_core::HostConfig;

use tuxtop_core::history_store::{HistoryStore, HistoryUsage};
use tuxtop_core::supervisor::Supervisor;

/// The configured hosts, for the frontend to render cards from.
#[tauri::command]
fn list_hosts(app: AppHandle) -> Result<Vec<HostConfig>, String> {
    hosts::load(&app)
}

/// Add a host, persist it, and start watching immediately.
#[tauri::command]
fn add_host(
    app: AppHandle,
    sup: tauri::State<'_, std::sync::Arc<Supervisor>>,
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
    sup.start(stored, iv);
    let _ = app.emit(EVENT_HOSTS, &all);

    Ok(all)
}

/// Stop watching a host and forget it.
#[tauri::command]
fn remove_host(
    app: AppHandle,
    sup: tauri::State<'_, std::sync::Arc<Supervisor>>,
    name: String,
) -> Result<Vec<HostConfig>, String> {
    let mut all = hosts::load(&app)?;
    tuxtop_core::hostlist::remove(&mut all, &name);
    hosts::save(&app, &all)?;

    sup.stop(&name);
    sup.forget(&name);
    app.state::<HistoryStore>().forget_host(&name);
    let _ = app.emit(EVENT_HOSTS, &all);

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

    let _ = app.emit(EVENT_HOSTS, &all);
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
    sup: tauri::State<'_, std::sync::Arc<Supervisor>>,
    store: tauri::State<'_, std::sync::Arc<HistoryStore>>,
    settings: Settings,
) -> Result<Settings, String> {
    let mut f = hosts::load_file(&app)?;
    let before = f.settings;
    f.settings = Settings {
        interval_secs: settings.interval_secs.clamp(1, 3600),
        history_cap_mb: settings.history_cap_mb.clamp(16, 8192),
        always_on_top: settings.always_on_top,
    };
    hosts::save_file(&app, &f)?;

    apply_always_on_top(&app, f.settings.always_on_top);
    store.set_cap_mb(f.settings.history_cap_mb);

    for h in &f.hosts {
        if effective_interval(h, &before) != effective_interval(h, &f.settings) {
            sup.start(h.clone(), effective_interval(h, &f.settings));
        }
    }

    let _ = app.emit(EVENT_SETTINGS, &f.settings);
    Ok(f.settings)
}

/// Set or clear one host's interval override, restarting just that host.
#[tauri::command]
fn set_host_interval(
    app: AppHandle,
    sup: tauri::State<'_, std::sync::Arc<Supervisor>>,
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
    sup.start(updated.clone(), effective_interval(&updated, &f.settings));

    let _ = app.emit(EVENT_HOSTS, &f.hosts);
    Ok(f.hosts.clone())
}

/// Set or clear one host's group.
///
/// Unlike the interval, this changes nothing about how the host is sampled, so
/// no sampler is restarted — only the arrangement on screen and the file on
/// disk.
#[tauri::command]
fn set_host_group(
    app: AppHandle,
    name: String,
    group: Option<String>,
) -> Result<Vec<HostConfig>, String> {
    let mut f = hosts::load_file(&app)?;

    if !tuxtop_core::hostlist::set_group(&mut f.hosts, &name, group.as_deref()) {
        return Err(format!("no host named {name}"));
    }

    hosts::save_file(&app, &f)?;
    let _ = app.emit(EVENT_HOSTS, &f.hosts);
    Ok(f.hosts.clone())
}

/// Set a host's operating system, and restart its sampler.
///
/// Unlike the group, this changes the remote command itself — a Linux command
/// against cmd.exe fails with "the system cannot find the path specified",
/// which explains nothing — so the host is restarted rather than left running
/// the wrong sampler until something else happens to restart it.
#[tauri::command]
fn set_host_os(
    app: AppHandle,
    sup: tauri::State<'_, std::sync::Arc<Supervisor>>,
    name: String,
    os: String,
) -> Result<Vec<HostConfig>, String> {
    let mut f = hosts::load_file(&app)?;

    let Some(h) = f.hosts.iter_mut().find(|h| h.name == name) else {
        return Err(format!("no host named {name}"));
    };
    h.os = if os.eq_ignore_ascii_case("windows") {
        "windows".into()
    } else {
        String::new()
    };
    let updated = h.clone();

    hosts::save_file(&app, &f)?;
    sup.start(updated.clone(), effective_interval(&updated, &f.settings));
    let _ = app.emit(EVENT_HOSTS, &f.hosts);
    Ok(f.hosts.clone())
}

/// Measured cost per host, for the settings meter.
#[tauri::command]
fn traffic_stats(sup: tauri::State<'_, std::sync::Arc<Supervisor>>) -> Vec<tuxtop_core::supervisor::HostTraffic> {
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
    store: tauri::State<'_, std::sync::Arc<HistoryStore>>,
    host: String,
    metric: String,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> Vec<tuxtop_core::history::Point> {
    let now = tuxtop_core::history_store::now_secs();
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
    store: tauri::State<'_, std::sync::Arc<HistoryStore>>,
    host: String,
    metrics: Vec<String>,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> std::collections::HashMap<String, Vec<tuxtop_core::history::Point>> {
    let now = tuxtop_core::history_store::now_secs();
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
fn history_usage(store: tauri::State<'_, std::sync::Arc<HistoryStore>>) -> HistoryUsage {
    store.usage()
}

/// Start or stop fleet-wide process sampling.
///
/// Driven by the view being open. Sampling costs a second of remote wall
/// clock per host per cycle, so a process view nobody is looking at should
/// cost nothing at all.
#[tauri::command]
fn set_processes_enabled(
    app: AppHandle,
    sup: tauri::State<'_, std::sync::Arc<Supervisor>>,
    enabled: bool,
) -> Result<(), String> {
    if enabled {
        let hosts = hosts::load(&app)?;
        sup.start_procs(hosts);
    } else {
        sup.stop_procs();
    }
    Ok(())
}

/// Every host's processes, merged and sorted as one fleet list.
#[tauri::command]
fn process_list(sup: tauri::State<'_, std::sync::Arc<Supervisor>>) -> Vec<tuxtop_core::procs::ProcInfo> {
    sup.fleet_procs()
}

/// Per-cgroup CPU, memory and task counts, for the group-by-owner view.
///
/// Sampled on the same channel as the processes, so the two describe the same
/// instant rather than two moments that could disagree.
#[tauri::command]
fn cgroup_list(sup: tauri::State<'_, std::sync::Arc<Supervisor>>) -> Vec<tuxtop_core::supervisor::HostCgroup> {
    sup.fleet_cgroups()
}

fn main() {
    tauri::Builder::default()
        .manage(std::sync::Arc::new(HistoryStore::new()))
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            add_host,
            remove_host,
            reorder_hosts,
            get_settings,
            set_settings,
            set_host_interval,
            set_host_group,
            set_host_os,
            traffic_stats,
            set_processes_enabled,
            process_list,
            cgroup_list,
            query_history,
            query_history_many,
            history_usage
        ])
        .setup(|app| {
            let handle = app.handle().clone();

            // The supervisor is framework-free now: it takes the history store
            // and a channel, and this task is the only thing that knows those
            // events end up in a webview.
            let history = app.state::<std::sync::Arc<HistoryStore>>().inner().clone();
            let (tx, mut rx) = tokio::sync::mpsc::channel(256);
            let sup = Supervisor::new(history, tx);
            app.manage(sup.clone());

            let emitter = handle.clone();
            tauri::async_runtime::spawn(async move {
                use tuxtop_core::supervisor::Event;
                while let Some(ev) = rx.recv().await {
                    let sent = match ev {
                        Event::Sample(s) => emitter.emit(EVENT_SAMPLE, &*s),
                        Event::Fault { host, fault } => emitter.emit(
                            EVENT_FAULT,
                            FaultEvent { host, fault },
                        ),
                        Event::Processes(h) => emitter.emit(EVENT_PROCS, &h),
                    };
                    if sent.is_err() {
                        // The webview is gone; the samplers keep running and
                        // history keeps filling, which is what a reload wants.
                        continue;
                    }
                }
            });

            if let Some(window) = app.get_webview_window("main") {
                apply_backdrop(&window);
            }

            // Restore the pinned state before the window is shown, so it does
            // not visibly jump to the front a moment after appearing.
            if let Ok(s) = hosts::load_settings(&handle) {
                apply_always_on_top(&handle, s.always_on_top);
                // Before any sampling starts, so the store is never briefly
                // uncapped on a fleet large enough to need the limit.
                handle.state::<HistoryStore>().set_cap_mb(s.history_cap_mb);
            }

            // Start a sampler for every configured host.
            //
            // A broken hosts.toml is reported to the frontend rather than
            // panicking: the window should open and explain itself, not fail
            // to start over a stray comma.
            match hosts::load_file(&handle) {
                Ok(f) => {
                    let sup = handle.state::<std::sync::Arc<Supervisor>>();
                    for cfg in f.hosts {
                        let iv = effective_interval(&cfg, &f.settings);
                        sup.start(cfg, iv);
                    }
                }
                Err(e) => {
                    eprintln!("could not load hosts: {e}");
                    let _ = handle.emit(EVENT_FAULT, e);
                }
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tuxtop");
}

/// Pin or unpin the window.
///
/// Failure is logged rather than propagated: a window manager that refuses
/// the request should not fail the settings save, and the stored preference
/// stays truthful about what was asked for.
fn apply_always_on_top(app: &AppHandle, on: bool) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if let Err(e) = window.set_always_on_top(on) {
        eprintln!("could not set always-on-top: {e}");
    }
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
