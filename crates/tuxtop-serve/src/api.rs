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
    "set_host_paused",
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
        "set_host_paused" => svc
            .set_host_paused(&s("name"), arg("paused").as_bool().unwrap_or(false))
            .map(|v| json!(v)),
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

#[cfg(test)]
mod tests {
    //! The two invariants this crate exists to hold, and had never tested.
    //!
    //! Both were argued for carefully in prose — the module docs above and in
    //! `main.rs` — and neither was ever an assertion. A comment does not fail.
    //! `cargo-mutants` found both at once: it deleted the `!` from the
    //! read-only check, inverting it, and turned four of the five clauses of
    //! the traversal guard into `&&`, and the suite stayed green because this
    //! crate had no tests in it at all.
    //!
    //! **The traversal tests need real files behind the guard.** The first
    //! version of them asserted `NOT_FOUND` for `../secret.toml` and friends
    //! and killed nothing, because a path the guard *lets through* also
    //! answers `NOT_FOUND` when it names a file that does not exist. The two
    //! outcomes were indistinguishable, so the assertion held with the guard
    //! removed. Every rejected name below therefore points at a file that
    //! genuinely exists: the only reason each one 404s is the guard, and
    //! disabling any single clause turns its case into a 200.
    //!
    //! The handlers are called directly rather than over a socket. `serve` and
    //! `command` are where the rules live; going through a client would test
    //! axum's routing on the way to testing them.

    use super::*;
    use tuxtop_core::config::Config;
    use tuxtop_core::history_store::HistoryStore;
    use tuxtop_core::service::Service;
    use tuxtop_core::supervisor::Supervisor;

    const SECRET: &str = "password = \"hunter2\"";

    /// A web root, plus the files each guard clause is the only thing hiding.
    ///
    /// Returns `(web_root, base)`. `base` is one directory up — where a
    /// successful traversal would land.
    fn web_root(name: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        let mut base = std::env::temp_dir();
        base.push(format!("tuxtop-web-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let dir = base.join("src");
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        std::fs::write(base.join("secret.toml"), SECRET).unwrap();
        std::fs::write(dir.join("index.html"), "<h1>fleet</h1>").unwrap();
        std::fs::write(dir.join("app.js"), "// app").unwrap();
        std::fs::write(dir.join("styles.css"), "body{}").unwrap();
        std::fs::write(dir.join("data.json"), "{}").unwrap();
        std::fs::write(dir.join("icon.svg"), "<svg/>").unwrap();
        std::fs::write(dir.join("shot.png"), [0x89, b'P', b'N', b'G']).unwrap();
        // Each of these exists *only* so that the clause rejecting it is the
        // sole reason its request fails.
        std::fs::write(dir.join("sub/nested.txt"), "reachable via a separator").unwrap();
        std::fs::write(dir.join("x..txt"), "reachable via a dot segment").unwrap();
        std::fs::write(dir.join(".env"), "reachable via a leading dot").unwrap();
        (dir, base)
    }

    fn state(dir: &std::path::Path, writable: bool) -> Arc<AppState> {
        let (tx, _rx) = tokio::sync::mpsc::channel(16);
        let history = Arc::new(HistoryStore::new());
        let sup = Supervisor::new(
            history.clone(),
            tx.clone(),
            tokio::runtime::Handle::current(),
        );
        let mut hosts = std::env::temp_dir();
        hosts.push(format!("tuxtop-serve-test-{}.toml", std::process::id()));
        let svc = Arc::new(Service::new(Config::new(&hosts), sup, history, tx));
        let (events, _) = tokio::sync::broadcast::channel(16);
        Arc::new(AppState {
            svc,
            web: dir.to_path_buf(),
            writable,
            events,
        })
    }

    async fn body_of(r: Response) -> String {
        let b = axum::body::to_bytes(r.into_body(), 1 << 20).await.unwrap();
        String::from_utf8_lossy(&b).into_owned()
    }

    #[tokio::test]
    async fn a_bare_filename_in_the_web_root_is_served() {
        // The other half of the guard. Rejecting everything would pass every
        // traversal test below and be a server that serves nothing.
        let (dir, base) = web_root("ok");
        let st = state(&dir, false);
        let r = serve(&st, "index.html").await;
        assert_eq!(r.status(), StatusCode::OK);
        assert!(body_of(r).await.contains("fleet"));
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn no_path_escapes_the_web_root() {
        // One case per clause, each naming a file that really is there, so
        // "rejected" and "not found" cannot be confused. Percent-encodings are
        // absent deliberately: axum decodes the path before `serve` sees it,
        // and a separator is a separator once decoded.
        let (dir, base) = web_root("traverse");
        let st = state(&dir, false);

        for (attempt, why) in [
            ("../secret.toml", "the traversal itself"),
            (
                "..\\secret.toml",
                "the Windows separator - this app ships a Windows build",
            ),
            ("sub/nested.txt", "a separator with no dot segment at all"),
            (
                "x..txt",
                "a dot segment with no separator, harmless and still refused",
            ),
            (
                ".env",
                "a dotfile: not a traversal, equally not ours to hand out",
            ),
            ("..", "the dot segment alone"),
            (
                "",
                "the empty name, which would join to the directory itself",
            ),
        ] {
            let r = serve(&st, attempt).await;
            assert_eq!(
                r.status(),
                StatusCode::NOT_FOUND,
                "{attempt:?} was served - {why}"
            );
            assert!(
                !body_of(r).await.contains("hunter2"),
                "{attempt:?} leaked a file above the web root"
            );
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn the_route_handler_enforces_the_guard_too() {
        // `asset` is what the router actually calls. A test only of `serve`
        // would still pass if the handler stopped delegating to it.
        let (dir, base) = web_root("handler");
        let st = state(&dir, false);
        let r = asset(State(st.clone()), Path("../secret.toml".into())).await;
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert!(!body_of(r).await.contains("hunter2"));

        let home = index(State(st)).await;
        assert_eq!(home.status(), StatusCode::OK);
        assert!(
            body_of(home).await.contains("fleet"),
            "index served nothing"
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn scripts_and_styles_keep_a_content_type_a_browser_will_run() {
        // A browser refuses to execute `application/octet-stream`, so losing a
        // match arm here is a blank page rather than a visible error.
        let (dir, base) = web_root("mime");
        let st = state(&dir, false);
        for (file, want) in [
            ("index.html", "text/html"),
            ("app.js", "text/javascript"),
            ("styles.css", "text/css"),
            // Not only the three the page cannot run without. An SVG served as
            // octet-stream will not render inline, and a JSON body a browser
            // refuses to parse is a fetch that fails for no visible reason.
            ("data.json", "application/json"),
            ("icon.svg", "image/svg+xml"),
            ("shot.png", "image/png"),
        ] {
            let r = serve(&st, file).await;
            let got = r.headers()[header::CONTENT_TYPE].to_str().unwrap();
            assert!(got.starts_with(want), "{file} served as {got}");
        }
        let _ = std::fs::remove_dir_all(base);
    }

    async fn post(st: &Arc<AppState>, cmd: &str, body: &str) -> StatusCode {
        command(State(st.clone()), Path(cmd.to_string()), body.to_string())
            .await
            .status()
    }

    #[tokio::test]
    async fn a_read_only_server_refuses_every_mutating_command() {
        // `add_host` makes *this* machine open an outbound SSH connection with
        // *its own* keys - not a capability a browser tab acquires by reaching
        // the port. The whole list is walked rather than one example, so a
        // command added to MUTATING later is covered by a test that already
        // exists.
        let (dir, base) = web_root("ro");
        let st = state(&dir, false);
        for cmd in MUTATING {
            assert_eq!(
                post(&st, cmd, "{}").await,
                StatusCode::FORBIDDEN,
                "{cmd} was allowed on a read-only server"
            );
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn a_read_only_server_still_answers_a_read() {
        // The direction the missing `!` breaks. Without this, a mutant that
        // swaps which side is refused passes the test above untouched: every
        // mutating command is still forbidden, and nothing notices that every
        // *reading* command became forbidden too.
        let (dir, base) = web_root("ro-read");
        let st = state(&dir, false);
        assert_eq!(post(&st, "capabilities", "").await, StatusCode::OK);
        assert_eq!(post(&st, "list_hosts", "").await, StatusCode::OK);
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn writable_is_what_lifts_the_refusal_and_nothing_else() {
        // The same command, the same body, the same server - one flag apart.
        // Anything else that made the refusal go away would be a second way in.
        let (dir, base) = web_root("rw");
        let ro = state(&dir, false);
        let rw = state(&dir, true);
        let body = r#"{"names":[]}"#;
        assert_eq!(
            post(&ro, "reorder_hosts", body).await,
            StatusCode::FORBIDDEN
        );
        assert_ne!(
            post(&rw, "reorder_hosts", body).await,
            StatusCode::FORBIDDEN,
            "--writable did not lift the refusal"
        );
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn capabilities_tells_the_truth_about_which_server_this_is() {
        // The frontend hides controls on this answer. A server claiming to be
        // writable when it is not would draw buttons that can only error,
        // which is the failure `capabilities` exists to prevent.
        let (dir, base) = web_root("caps");
        for writable in [false, true] {
            let st = state(&dir, writable);
            let r = command(State(st), Path("capabilities".to_string()), String::new()).await;
            let v: Value = serde_json::from_str(&body_of(r).await).unwrap();
            assert_eq!(v["writable"], json!(writable));
        }
        let _ = std::fs::remove_dir_all(base);
    }

    #[tokio::test]
    async fn events_reach_the_browser_under_the_names_the_desktop_app_uses() {
        // The browser shim and the Tauri app subscribe to the same topics. An
        // encoder that dropped an event, or renamed one, would leave a fleet
        // that connects and then never updates - and `encode_event` returning
        // None for everything was a surviving mutant.
        let sample = Event::Sample(Box::new(tuxtop_core::Sample {
            host: "dove".into(),
            cpu: 42.0,
            ..Default::default()
        }));
        let out = encode_event(&sample).expect("a sample must encode");
        assert!(out.contains("tuxtop://sample"), "got {out}");
        assert!(out.contains("dove"), "got {out}");

        let fault = Event::Fault {
            host: "wader".into(),
            fault: tuxtop_core::HostFault::AuthFailed("no key".into()),
        };
        let out = encode_event(&fault).expect("a fault must encode");
        assert!(out.contains("tuxtop://fault"), "got {out}");
        // Attributing a fault to the wrong card is worse than dropping it, so
        // the host has to travel with it.
        assert!(out.contains("wader"), "got {out}");
    }
}
