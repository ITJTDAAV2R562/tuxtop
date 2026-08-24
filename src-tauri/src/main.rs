// Tuxtop — Windows shell.
//
// Thin by design: everything testable lives in `tuxtop-core`. This file wires
// the sampler to a window and nothing more. If logic accumulates here, it
// belongs in the core crate where it can be tested without a GUI (ADR-006).

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]


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
use tuxtop_core::config::Config;
use tuxtop_core::hostlist::Settings;
use tuxtop_core::HostConfig;

use tuxtop_core::history_store::{HistoryStore, HistoryUsage};
use tuxtop_core::service::Service;
use tuxtop_core::supervisor::Supervisor;

/// Tauri commands.
///
/// Every one of these is a one-line delegation to `tuxtop_core::service`,
/// deliberately. The operations used to live here, which meant a headless
/// server would have had to reimplement them and nothing could test them. What
/// stays behind is what is genuinely Tauri's: the window, and turning events
/// into webview topics.
type Svc<'a> = tauri::State<'a, std::sync::Arc<Service>>;

#[tauri::command]
fn list_hosts(svc: Svc<'_>) -> Result<Vec<HostConfig>, String> {
    svc.list_hosts()
}

#[tauri::command]
fn add_host(svc: Svc<'_>, cfg: HostConfig) -> Result<Vec<HostConfig>, String> {
    svc.add_host(cfg)
}

#[tauri::command]
fn remove_host(svc: Svc<'_>, name: String) -> Result<Vec<HostConfig>, String> {
    svc.remove_host(&name)
}

#[tauri::command]
fn reorder_hosts(svc: Svc<'_>, names: Vec<String>) -> Result<Vec<HostConfig>, String> {
    svc.reorder_hosts(&names)
}

#[tauri::command]
fn get_settings(svc: Svc<'_>) -> Result<Settings, String> {
    svc.get_settings()
}

/// The one command with a genuinely Tauri-shaped side effect: always-on-top is
/// a property of a window, which a headless server does not have.
#[tauri::command]
fn set_settings(app: AppHandle, svc: Svc<'_>, settings: Settings) -> Result<Settings, String> {
    let saved = svc.set_settings(settings)?;
    apply_always_on_top(&app, saved.always_on_top);
    Ok(saved)
}

#[tauri::command]
fn set_host_interval(
    svc: Svc<'_>,
    name: String,
    interval_secs: Option<u32>,
) -> Result<Vec<HostConfig>, String> {
    svc.set_host_interval(&name, interval_secs)
}

#[tauri::command]
fn set_host_group(
    svc: Svc<'_>,
    name: String,
    group: Option<String>,
) -> Result<Vec<HostConfig>, String> {
    svc.set_host_group(&name, group.as_deref())
}

#[tauri::command]
fn set_host_os(svc: Svc<'_>, name: String, os: String) -> Result<Vec<HostConfig>, String> {
    svc.set_host_os(&name, &os)
}

#[tauri::command]
fn traffic_stats(svc: Svc<'_>) -> Vec<tuxtop_core::supervisor::HostTraffic> {
    svc.traffic_stats()
}

#[tauri::command]
fn set_processes_enabled(svc: Svc<'_>, enabled: bool) -> Result<(), String> {
    svc.set_processes_enabled(enabled)
}

#[tauri::command]
fn process_list(svc: Svc<'_>) -> Vec<tuxtop_core::procs::ProcInfo> {
    svc.process_list()
}

#[tauri::command]
fn cgroup_list(svc: Svc<'_>) -> Vec<tuxtop_core::supervisor::HostCgroup> {
    svc.cgroup_list()
}

#[tauri::command]
fn query_history(
    svc: Svc<'_>,
    host: String,
    metric: String,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> Vec<tuxtop_core::history::Point> {
    svc.query_history(&host, &metric, from_secs_ago, to_secs_ago, max_points)
}

#[tauri::command]
fn query_history_many(
    svc: Svc<'_>,
    host: String,
    metrics: Vec<String>,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> std::collections::HashMap<String, Vec<tuxtop_core::history::Point>> {
    svc.query_history_many(&host, metrics, from_secs_ago, to_secs_ago, max_points)
}

#[tauri::command]
fn history_usage(svc: Svc<'_>) -> HistoryUsage {
    svc.history_usage()
}

fn main() {
    tauri::Builder::default()
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

            // Where hosts.toml lives. The OS config directory here; a command
            // line argument in the headless server. Nothing below knows which.
            let dir = app
                .path()
                .app_config_dir()
                .map_err(|e| format!("no config directory available: {e}"))?;
            let config = Config::new(dir.join("hosts.toml"));

            let history = std::sync::Arc::new(HistoryStore::new());
            let (tx, mut rx) = tokio::sync::mpsc::channel(256);
            // Tauri's runtime, not `Handle::current()`: `setup` runs on the
            // main thread outside it, so asking for the current handle here
            // panics — which it did, on launch, after compiling cleanly.
            let rt = tauri::async_runtime::block_on(async {
                tokio::runtime::Handle::current()
            });
            let sup = Supervisor::new(history.clone(), tx.clone(), rt);
            let svc = std::sync::Arc::new(Service::new(config, sup, history, tx));
            app.manage(svc.clone());

            // The only thing in this process that knows the events end up in
            // a webview. A headless server subscribes to the same channel and
            // writes them to an HTTP stream instead.
            let emitter = handle.clone();
            tauri::async_runtime::spawn(async move {
                use tuxtop_core::supervisor::Event;
                while let Some(ev) = rx.recv().await {
                    // Errors are ignored rather than ending the loop: the
                    // webview goes away on every reload, and the samplers and
                    // history must survive that.
                    let _ = match ev {
                        Event::Sample(s) => emitter.emit(EVENT_SAMPLE, &*s),
                        Event::Fault { host, fault } => {
                            emitter.emit(EVENT_FAULT, FaultEvent { host, fault })
                        }
                        Event::Processes(h) => emitter.emit(EVENT_PROCS, &h),
                        Event::HostsChanged(h) => emitter.emit(EVENT_HOSTS, &h),
                        Event::SettingsChanged(st) => emitter.emit(EVENT_SETTINGS, &st),
                    };
                }
            });

            if let Some(window) = app.get_webview_window("main") {
                apply_backdrop(&window);
            }

            // Start everything, and restore the pinned state before the window
            // is shown so it does not visibly jump to the front a moment later.
            //
            // A broken hosts.toml is reported rather than fatal: the window
            // should open and explain itself, not fail to start over a stray
            // comma.
            match svc.start_all() {
                Ok(settings) => apply_always_on_top(&handle, settings.always_on_top),
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
