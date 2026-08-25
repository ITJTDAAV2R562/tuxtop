//! HTTP surface: static files, one command endpoint, one event stream.
//!
//! The shape is dictated by the frontend, which talks to its backend through
//! exactly one thing — `invoke(command, args)` — and receives pushed events.
//! So this is `POST /api/:command` and `GET /api/events`, and `app.js` needs no
//! changes at all.

use std::sync::Arc;

use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response, Sse};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};
use tuxtop_core::supervisor::Event;

use crate::AppState;

/// Commands that change configuration.
///
/// Refused unless `--writable`. Adding a host makes this machine open an SSH
/// connection with its own keys, which is not something a browser tab should
/// be able to cause merely by reaching the port. Everything absent from this
/// list only reads.
const MUTATING: &[&str] = &[
    "add_host",
    "remove_host",
    "reorder_hosts",
    "set_settings",
    "set_host_interval",
    "set_host_group",
    "set_host_os",
];

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/events", get(events))
        .route("/api/{command}", post(command))
        .route("/", get(index))
        .route("/{file}", get(asset))
        .with_state(state)
}

/// Turn a supervisor event into the SSE payload the frontend expects.
///
/// The names match the Tauri topics exactly, so the browser shim and the
/// desktop app receive the same events under the same names.
pub fn encode_event(ev: &Event) -> Option<String> {
    let (topic, payload) = match ev {
        Event::Sample(s) => ("tuxtop://sample", serde_json::to_value(s).ok()?),
        Event::Fault { host, fault } => {
            let mut v = serde_json::to_value(fault).ok()?;
            // The frontend routes faults by host; a bare fault cannot be
            // attributed to a card, and attributing one to the wrong card is
            // worse than dropping it.
            if let Some(o) = v.as_object_mut() {
                o.insert("host".into(), json!(host));
            }
            ("tuxtop://fault", v)
        }
        Event::Processes(h) => ("tuxtop://processes", json!(h)),
        Event::HostsChanged(h) => ("tuxtop://hosts-changed", serde_json::to_value(h).ok()?),
        Event::SettingsChanged(s) => ("tuxtop://settings-changed", serde_json::to_value(s).ok()?),
    };
    serde_json::to_string(&json!({ "event": topic, "payload": payload })).ok()
}

async fn events(
    State(st): State<Arc<AppState>>,
) -> Sse<
    impl futures_core::Stream<Item = Result<axum::response::sse::Event, std::convert::Infallible>>,
> {
    let mut rx = st.events.subscribe();
    let stream = async_stream::stream! {
        loop {
            match rx.recv().await {
                Ok(line) => yield Ok(axum::response::sse::Event::default().data(line)),
                // Lagged: this browser could not keep up and missed frames.
                // Continue rather than close - the next sample is a second
                // away and a full redraw follows it.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    };
    Sse::new(stream).keep_alive(axum::response::sse::KeepAlive::default())
}

async fn command(
    State(st): State<Arc<AppState>>,
    Path(cmd): Path<String>,
    // The raw body, parsed leniently rather than through the Json extractor.
    // Half these commands take no arguments, and rejecting them for lacking a
    // content-type header is a failure about protocol trivia rather than about
    // anything the caller got wrong.
    body: String,
) -> Response {
    let args: Value = if body.trim().is_empty() {
        Value::Null
    } else {
        match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(e) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(json!({ "error": format!("body is not JSON: {e}") })),
                )
                    .into_response()
            }
        }
    };
    let arg = |k: &str| args.get(k).cloned().unwrap_or(Value::Null);
    let s = |k: &str| arg(k).as_str().unwrap_or_default().to_string();
    let n = |k: &str| arg(k).as_u64().unwrap_or(0);

    if MUTATING.contains(&cmd.as_str()) && !st.writable {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({
                "error": format!(
                    "{cmd} changes configuration, and this server is read-only. \
                     Adding or editing a host makes it open an SSH connection with \
                     its own keys. Restart with --writable if that is what you want."
                )
            })),
        )
            .into_response();
    }

    let svc = &st.svc;
    let out: Result<Value, String> = match cmd.as_str() {
        // What this server will let the caller do. Asked once at startup so
        // controls that can only fail are never drawn - a button that always
        // returns an error is worse than an absent one, because it looks like
        // a capability.
        "capabilities" => Ok(json!({ "writable": st.writable })),
        "list_hosts" => svc.list_hosts().map(|v| json!(v)),
        "add_host" => serde_json::from_value(arg("cfg"))
            .map_err(|e| e.to_string())
            .and_then(|c| svc.add_host(c))
            .map(|v| json!(v)),
        "remove_host" => svc.remove_host(&s("name")).map(|v| json!(v)),
        "reorder_hosts" => serde_json::from_value::<Vec<String>>(arg("names"))
            .map_err(|e| e.to_string())
            .and_then(|names| svc.reorder_hosts(&names))
            .map(|v| json!(v)),
        "get_settings" => svc.get_settings().map(|v| json!(v)),
        "set_settings" => serde_json::from_value(arg("settings"))
            .map_err(|e| e.to_string())
            .and_then(|st2| svc.set_settings(st2))
            .map(|v| json!(v)),
        "set_host_interval" => svc
            .set_host_interval(&s("name"), arg("intervalMs").as_u64().map(|v| v as u32))
            .map(|v| json!(v)),
        "set_host_group" => svc
            .set_host_group(&s("name"), arg("group").as_str())
            .map(|v| json!(v)),
        "set_host_os" => svc.set_host_os(&s("name"), &s("os")).map(|v| json!(v)),
        "traffic_stats" => Ok(json!(svc.traffic_stats())),
        "set_processes_enabled" => svc
            .set_processes_enabled(arg("enabled").as_bool().unwrap_or(false))
            .map(|_| Value::Null),
        "process_list" => Ok(json!(svc.process_list())),
        "cgroup_list" => Ok(json!(svc.cgroup_list())),
        "history_usage" => Ok(json!(svc.history_usage())),
        "query_history" => Ok(json!(svc.query_history(
            &s("host"),
            &s("metric"),
            n("fromSecsAgo"),
            n("toSecsAgo"),
            n("maxPoints") as usize,
        ))),
        "query_history_fleet" => svc
            .query_history_fleet(
                &s("metric"),
                n("fromSecsAgo"),
                n("toSecsAgo"),
                n("maxPoints") as usize,
            )
            .map(|v| json!(v)),
        "query_history_many" => serde_json::from_value::<Vec<String>>(arg("metrics"))
            .map_err(|e| e.to_string())
            .map(|m| {
                json!(svc.query_history_many(
                    &s("host"),
                    m,
                    n("fromSecsAgo"),
                    n("toSecsAgo"),
                    n("maxPoints") as usize,
                ))
            }),
        // Named rather than a bare 404, because the frontend and the server
        // drifting apart is exactly the bug this would otherwise hide.
        other => Err(format!("unknown command {other}")),
    };

    match out {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

async fn index(State(st): State<Arc<AppState>>) -> Response {
    serve(&st, "index.html").await
}

async fn asset(State(st): State<Arc<AppState>>, Path(file): Path<String>) -> Response {
    serve(&st, &file).await
}

/// Serve one file from the web directory.
///
/// The frontend is flat — every file sits directly in `src/` — so only a bare
/// filename is ever legitimate. Rejecting anything with a separator or a dot
/// segment makes directory traversal impossible by construction rather than by
/// careful normalisation, which is the kind of care that eventually slips.
async fn serve(st: &AppState, file: &str) -> Response {
    if file.is_empty()
        || file.contains('/')
        || file.contains('\\')
        || file.contains("..")
        || file.starts_with('.')
    {
        return (StatusCode::NOT_FOUND, "no").into_response();
    }

    let path = st.web.join(file);
    let Ok(bytes) = tokio::fs::read(&path).await else {
        return (StatusCode::NOT_FOUND, format!("no {file}")).into_response();
    };

    let mime = match path.extension().and_then(|e| e.to_str()) {
        Some("html") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("json") => "application/json",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        _ => "application/octet-stream",
    };
    ([(header::CONTENT_TYPE, mime)], bytes).into_response()
}
