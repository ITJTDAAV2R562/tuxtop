//! Tuxtop as a headless server.
//!
//! The same supervisor, the same history, the same operations — viewed in a
//! browser instead of a window. This exists because the desktop app can only
//! be watched from the desk it runs on, and a fleet is not always looked at
//! from there.
//!
//! ## What it does not do
//!
//! **It binds to loopback and grows no authentication.** Exposure is somebody
//! else's job — `tailscale serve` in front of it gives TLS and identity, which
//! is how Beszel is already reached on this fleet. A monitoring tool inventing
//! its own session handling is a way to acquire a login bug for no benefit.
//!
//! **It is read-only by default.** Viewing is harmless; `add_host` causes an
//! outbound SSH from *this* machine using *its* keys, which is not a capability
//! to hand a browser tab because it happened to reach the port. `--writable`
//! opts in.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use tuxtop_core::config::Config;
use tuxtop_core::history_store::HistoryStore;
use tuxtop_core::service::Service;
use tuxtop_core::supervisor::Supervisor;

mod api;

const USAGE: &str = "\
tuxtop-serve — watch a fleet from a browser

USAGE:
    tuxtop-serve [--hosts PATH] [--port N] [--web DIR] [--writable]

    --hosts     hosts.toml to read (default ./hosts.toml)
    --port      port to listen on (default 8787)
    --web       directory holding index.html (default ./src)
    --writable  allow requests that change configuration

Binds to 127.0.0.1 only. Put `tailscale serve` in front of it for TLS and
identity rather than expecting this to authenticate anybody.
";

pub struct AppState {
    pub svc: Arc<Service>,
    pub web: PathBuf,
    pub writable: bool,
    /// Every connected browser gets a copy of the event stream.
    pub events: tokio::sync::broadcast::Sender<String>,
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let mut hosts = PathBuf::from("hosts.toml");
    let mut port: u16 = 8787;
    let mut web = PathBuf::from("src");
    let mut writable = false;

    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => {
                print!("{USAGE}");
                return std::process::ExitCode::SUCCESS;
            }
            "--writable" => writable = true,
            "--hosts" | "--port" | "--web" => {
                let Some(v) = it.next() else {
                    eprintln!("error: {a} needs a value\n\n{USAGE}");
                    return std::process::ExitCode::from(2);
                };
                match a.as_str() {
                    "--hosts" => hosts = v.into(),
                    "--web" => web = v.into(),
                    _ => match v.parse() {
                        Ok(p) => port = p,
                        Err(_) => {
                            eprintln!("error: bad port: {v}");
                            return std::process::ExitCode::from(2);
                        }
                    },
                }
            }
            other => {
                eprintln!("error: unknown argument: {other}\n\n{USAGE}");
                return std::process::ExitCode::from(2);
            }
        }
    }

    let history = Arc::new(HistoryStore::new());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let sup = Supervisor::new(
        history.clone(),
        tx.clone(),
        tokio::runtime::Handle::current(),
    );
    let svc = Arc::new(Service::new(Config::new(&hosts), sup, history, tx));

    let settings = match svc.start_all() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not load {}: {e}", hosts.display());
            return std::process::ExitCode::FAILURE;
        }
    };

    let n = svc.list_hosts().map(|h| h.len()).unwrap_or(0);
    eprintln!(
        "watching {n} hosts from {} at {}{}",
        hosts.display(),
        tuxtop_core::sampler::rate_label(settings.interval_ms),
        if writable { "" } else { " (read-only)" }
    );

    // Fan the supervisor's single channel out to however many browsers are
    // connected. A slow or vanished client lags and is dropped by broadcast
    // rather than stalling the samplers behind it.
    let (bcast, _) = tokio::sync::broadcast::channel::<String>(1024);
    let feed = bcast.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            if let Some(line) = api::encode_event(&ev) {
                // Err means nobody is listening, which is the normal state of
                // a server with no browser open. Not a reason to stop.
                let _ = feed.send(line);
            }
        }
    });

    let state = Arc::new(AppState {
        svc,
        web,
        writable,
        events: bcast,
    });

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not bind {addr}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    eprintln!("listening on http://{addr}/");

    if let Err(e) = axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
            eprintln!("\nstopping; ssh sessions close with the process");
        })
        .await
    {
        eprintln!("server error: {e}");
        return std::process::ExitCode::FAILURE;
    }
    std::process::ExitCode::SUCCESS
}
