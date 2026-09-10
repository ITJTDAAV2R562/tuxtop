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
use tuxtop_core::remote::Capabilities;
use tuxtop_core::service::Service;
use tuxtop_core::supervisor::Supervisor;

mod remote;

/// Tauri commands.
///
/// Every one of these is a one-line delegation to `tuxtop_core::service`,
/// deliberately. The operations used to live here, which meant a headless
/// server would have had to reimplement them and nothing could test them. What
/// stays behind is what is genuinely Tauri's: the window, and turning events
/// into webview topics.
type Svc<'a> = tauri::State<'a, std::sync::Arc<Service>>;

/// **The one place in this process that proxies.**
///
/// Seventeen commands cannot each carry an `if remote` — that is the shape
/// ADR-012 warns about, with five callers of which one forgets. Every *fleet
/// read* goes through here instead: it describes the fleet, and in remote mode
/// the fleet is the server's. Answering one out of the local `hosts.toml`
/// would caption nineteen cards with another machine's configuration.
///
/// Two things know a server exists and they know different verbs (ADR-018
/// decision 4). **Core knows to refuse**: `Service` holds the write refusal
/// and the derived mode, because a refusal in the shell is a refusal the
/// workspace never compiles. **The shell knows to proxy**: this helper owns
/// the socket, and is the only thing that turns a local command into an HTTP
/// request.
///
/// The classes that are *not* here are as deliberate. **History reads** stay
/// local — ADR-017 rule 2 makes history in-memory per instance, so fetching
/// the server's would make that rule meaningless and blend two fleets' `db1`.
/// **Writes** are refused in `Service`, one choke point, and a viewer that
/// reached them through this proxy would be refused by the server anyway.
async fn fleet_read<T, L>(
    svc: &Service,
    command: &str,
    args: serde_json::Value,
    local: L,
) -> Result<T, String>
where
    T: serde::de::DeserializeOwned,
    L: FnOnce() -> Result<T, String>,
{
    match svc.endpoint()? {
        None => local(),
        Some(endpoint) => remote::post(&endpoint, command, args).await,
    }
}

/// No arguments. Half the fleet reads take none, and an empty object is what
/// `api::command` parses most leniently.
fn no_args() -> serde_json::Value {
    serde_json::json!({})
}

#[tauri::command]
async fn list_hosts(svc: Svc<'_>) -> Result<Vec<HostConfig>, String> {
    fleet_read(&svc, "list_hosts", no_args(), || svc.list_hosts()).await
}

/// What this window can do, and whose readings it is showing.
///
/// **`writable` and `endpoint` come from *here*, never from the server's
/// answer.** A writable server does not make this viewer writable: remote
/// writes are ADR-017 rule 4's follow-on phase, and `Service` refuses them
/// meanwhile, so taking the server's `writable` would draw controls whose
/// command is refused one layer down — the exact failure `capabilities`
/// exists to prevent, arriving from the other direction.
///
/// What *is* taken from the server is what describes the machine doing the
/// sampling: `stale_after_ms`, computed from its interval, and the version
/// that produced the events. A mismatch with this build is stated rather than
/// judged.
#[tauri::command]
async fn capabilities(svc: Svc<'_>) -> Result<Capabilities, String> {
    let mut caps = svc.capabilities()?;
    let Some(endpoint) = caps.endpoint.clone() else {
        return Ok(caps);
    };
    match remote::post::<Capabilities>(&endpoint, "capabilities", no_args()).await {
        Ok(server) => {
            caps.stale_after_ms = server.stale_after_ms;
            caps.version_note =
                tuxtop_core::remote::version_mismatch(env!("CARGO_PKG_VERSION"), &server.version);
            caps.version = server.version;
        }
        // Not fatal, and not silent. Losing this answer must not blank the
        // chrome: `endpoint` and `writable` above are ours and still true, so
        // the window keeps saying whose fleet it is showing and the freshness
        // line takes over from here. What is lost is the server's interval, so
        // the threshold falls back to the local one - academic in a state where
        // the link is already down, and stated in the log rather than guessed
        // at in the UI.
        Err(e) => eprintln!("remote: could not read capabilities from {endpoint} — {e}"),
    }
    Ok(caps)
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
async fn get_settings(svc: Svc<'_>) -> Result<Settings, String> {
    fleet_read(&svc, "get_settings", no_args(), || svc.get_settings()).await
}

/// The one command with a genuinely Tauri-shaped side effect: always-on-top is
/// a property of a window, which a headless server does not have.
#[tauri::command]
fn set_settings(app: AppHandle, svc: Svc<'_>, settings: Settings) -> Result<Settings, String> {
    let saved = svc.set_settings(settings)?;
    apply_always_on_top(&app, saved.viewer.always_on_top);
    Ok(saved)
}

#[tauri::command]
fn set_host_interval(
    svc: Svc<'_>,
    name: String,
    interval_ms: Option<u32>,
) -> Result<Vec<HostConfig>, String> {
    svc.set_host_interval(&name, interval_ms)
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
fn set_host_paused(svc: Svc<'_>, name: String, paused: bool) -> Result<Vec<HostConfig>, String> {
    svc.set_host_paused(&name, paused)
}

// These three grew a `Result` with remote mode. On success nothing changes for
// the caller; a proxy failure now rejects the promise instead of being
// impossible, and all three call sites in `app.js` already catch.
#[tauri::command]
async fn traffic_stats(svc: Svc<'_>) -> Result<Vec<tuxtop_core::supervisor::HostTraffic>, String> {
    fleet_read(&svc, "traffic_stats", no_args(), || Ok(svc.traffic_stats())).await
}

#[tauri::command]
async fn process_list(svc: Svc<'_>) -> Result<Vec<tuxtop_core::procs::ProcInfo>, String> {
    fleet_read(&svc, "process_list", no_args(), || Ok(svc.process_list())).await
}

#[tauri::command]
async fn cgroup_list(svc: Svc<'_>) -> Result<Vec<tuxtop_core::supervisor::HostCgroup>, String> {
    fleet_read(&svc, "cgroup_list", no_args(), || Ok(svc.cgroup_list())).await
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
fn query_history_fleet(
    svc: Svc<'_>,
    metric: String,
    from_secs_ago: u64,
    to_secs_ago: u64,
    max_points: usize,
) -> Result<std::collections::HashMap<String, Vec<tuxtop_core::history::Point>>, String> {
    svc.query_history_fleet(&metric, from_secs_ago, to_secs_ago, max_points)
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
        // The updater never checks on its own — `check()` is an explicit call
        // from the frontend, gated behind a setting and a button. Registering
        // the plugin grants the capability; it does not start any polling.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            list_hosts,
            add_host,
            remove_host,
            reorder_hosts,
            get_settings,
            set_settings,
            capabilities,
            set_host_interval,
            set_host_group,
            set_host_os,
            set_host_paused,
            traffic_stats,
            process_list,
            cgroup_list,
            query_history,
            query_history_many,
            query_history_fleet,
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
            // Kept before the service takes them: in remote mode the read loop
            // feeds the same channel and the same store the samplers would.
            let events = tx.clone();
            let store = history.clone();
            let svc = std::sync::Arc::new(Service::new(config, sup, history, tx));
            app.manage(svc.clone());

            // One reader, and a handle that can stop it. Nothing stops it in
            // this phase; it is built this way because retrofitting
            // cancellation onto a running loop is how switching endpoints ends
            // up with two readers feeding one window.
            let reader = std::sync::Arc::new(remote::ReadLoop::default());
            app.manage(reader.clone());

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
                Ok(settings) => {
                    apply_always_on_top(&handle, settings.viewer.always_on_top);
                    // `start_all` started nothing if an endpoint is set - remote
                    // mode replaces the local data plane rather than adding to
                    // it - so this is what produces the events instead. The
                    // window cannot tell the difference, which is the whole
                    // point of swapping at the `Event` seam (ADR-018).
                    if let Some(endpoint) = settings.viewer.server.clone() {
                        eprintln!("watching {endpoint} rather than sampling locally");
                        reader.start(endpoint, events, store);
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
