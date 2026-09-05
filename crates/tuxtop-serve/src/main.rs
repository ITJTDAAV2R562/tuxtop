//! Tuxtop as a headless server.
//!
//! The same supervisor, the same history, the same operations — viewed in a
//! browser instead of a window. This exists because the desktop app can only
//! be watched from the desk it runs on, and a fleet is not always looked at
//! from there.
//!
//! ## What it does not do
//!
//! **It grows no authentication, wherever it binds.** `--bind` defaults to
//! `127.0.0.1` and takes any IP you name, but exposure is still somebody
//! else's job: put anything in front that terminates TLS and establishes
//! identity — `ssh -L` needs nothing installed, and nginx, Caddy, a VPN,
//! `tailscale serve` or Cloudflare Access all serve. A monitoring tool
//! inventing its own session handling is a way to acquire a login bug for no
//! benefit.
//!
//! **It is read-only by default.** Viewing is harmless; `add_host` causes an
//! outbound SSH from *this* machine using *its* keys, which is not a capability
//! to hand a browser tab because it happened to reach the port. `--writable`
//! opts in — and is refused outright alongside a wildcard bind.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
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
    tuxtop-serve [--hosts PATH] [--bind ADDR] [--port N] [--web DIR] [--writable]

    --hosts     hosts.toml to read (default ./hosts.toml)
    --bind      IP to listen on (default 127.0.0.1)
    --port      port to listen on (default 8787)
    --web       directory holding index.html (default ./src)
    --writable  allow requests that change configuration

--bind takes an IP, v4 or v6 - a hostname is refused, because resolving one at
bind time is a surprise nobody wants, and there is no shorthand for the
wildcard: if you want 0.0.0.0 you type it. The wildcard together with
--writable is refused outright.

Wherever it binds it authenticates nobody. Put a proxy in front for TLS and
identity - an `ssh -L` tunnel, nginx, Caddy, a VPN, tailscale serve - rather
than expecting this to authenticate anybody.
";

pub struct AppState {
    pub svc: Arc<Service>,
    pub web: PathBuf,
    pub writable: bool,
    /// Every connected browser gets a copy of the event stream.
    pub events: tokio::sync::broadcast::Sender<String>,
}

/// A command line that has been read and checked.
#[derive(Debug, PartialEq, Eq)]
pub struct Args {
    pub hosts: PathBuf,
    pub bind: IpAddr,
    pub port: u16,
    pub web: PathBuf,
    pub writable: bool,
}

impl Args {
    fn addr(&self) -> SocketAddr {
        SocketAddr::new(self.bind, self.port)
    }
}

/// `--help` is neither a run nor an error; the caller prints `USAGE` and exits
/// successfully.
#[derive(Debug, PartialEq, Eq)]
pub enum Parsed {
    Run(Args),
    Help,
}

/// Why a command line was refused.
///
/// A variant per reason rather than a formatted `String`, so a test can assert
/// *which* rule rejected a line. `--bind 0.0.0.0 --writable` returning some
/// error is not evidence the wildcard check exists — a typo in the same line
/// returns one too.
#[derive(Debug, PartialEq, Eq)]
pub enum ArgError {
    NeedsValue(String),
    BadPort(String),
    BadBind(String),
    WritableWildcard(IpAddr),
    Unknown(String),
}

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsValue(flag) => write!(f, "{flag} needs a value"),
            Self::BadPort(v) => write!(f, "bad port: {v}"),
            Self::BadBind(v) => write!(
                f,
                "bad --bind address: {v} - it takes an IP, v4 or v6; \
                 a hostname is not resolved"
            ),
            Self::WritableWildcard(ip) => write!(
                f,
                "--bind {ip} with --writable is refused: anyone who reaches the \
                 port could make this machine open SSH connections using its own \
                 keys. Name the one address to serve on, or drop --writable"
            ),
            Self::Unknown(a) => write!(f, "unknown argument: {a}"),
        }
    }
}

impl ArgError {
    /// Whether the message is better off followed by `USAGE`.
    ///
    /// A mistyped flag wants the list of flags. The wildcard refusal does not:
    /// the line was well-formed and the message already says what to change.
    fn wants_usage(&self) -> bool {
        matches!(self, Self::NeedsValue(_) | Self::Unknown(_))
    }
}

/// Read the command line. Pure: no environment, no filesystem, no binding.
///
/// Extracted from `main` so the two rules below can be asserted at all — the
/// wildcard refusal in particular is one of this crate's two security-shaped
/// invariants, and is a single deleted clause away from inverting.
pub fn parse_args<I: IntoIterator<Item = String>>(args: I) -> Result<Parsed, ArgError> {
    let mut hosts = PathBuf::from("hosts.toml");
    let mut bind = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let mut port: u16 = 8787;
    let mut web = PathBuf::from("src");
    let mut writable = false;

    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Ok(Parsed::Help),
            "--writable" => writable = true,
            "--hosts" | "--bind" | "--port" | "--web" => {
                let Some(v) = it.next() else {
                    return Err(ArgError::NeedsValue(a));
                };
                match a.as_str() {
                    "--hosts" => hosts = v.into(),
                    "--web" => web = v.into(),
                    // An IP and nothing else. A hostname would have to be
                    // resolved at bind time, which can hand you an address you
                    // did not ask for, and can do it differently tomorrow.
                    "--bind" => bind = v.parse().map_err(|_| ArgError::BadBind(v))?,
                    _ => port = v.parse().map_err(|_| ArgError::BadPort(v))?,
                }
            }
            _ => return Err(ArgError::Unknown(a)),
        }
    }

    // ADR-017: the refusal is the wildcard specifically, not non-loopback
    // generally. Binding one tailnet address is a deployment choice, and
    // arguably tighter than loopback behind a proxy that itself listens
    // everywhere. But wildcard plus write means anyone who reaches the port can
    // make this machine open SSH to anywhere with its own keys, and that
    // consequence lands on *other* people's machines - which is where "never
    // fail open on a security path" applies. A named non-loopback address with
    // --writable gets the loud line in `main`, not this.
    if writable && bind.is_unspecified() {
        return Err(ArgError::WritableWildcard(bind));
    }

    Ok(Parsed::Run(Args {
        hosts,
        bind,
        port,
        web,
        writable,
    }))
}

/// Who can reach this address, in the words somebody needs at startup.
///
/// "(read-only)" says what the server refuses. This says who gets to ask, which
/// is the other half and the half that stopped being a constant.
fn reach(ip: IpAddr) -> &'static str {
    if ip.is_loopback() {
        "this machine only"
    } else if ip.is_unspecified() {
        "every interface on this machine"
    } else {
        "anything that can route to this address"
    }
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let args = match parse_args(std::env::args().skip(1)) {
        Ok(Parsed::Run(a)) => a,
        Ok(Parsed::Help) => {
            print!("{USAGE}");
            return std::process::ExitCode::SUCCESS;
        }
        Err(e) => {
            if e.wants_usage() {
                eprintln!("error: {e}\n\n{USAGE}");
            } else {
                eprintln!("error: {e}");
            }
            return std::process::ExitCode::from(2);
        }
    };

    let history = Arc::new(HistoryStore::new());
    let (tx, mut rx) = tokio::sync::mpsc::channel(256);
    let sup = Supervisor::new(
        history.clone(),
        tx.clone(),
        tokio::runtime::Handle::current(),
    );
    let svc = Arc::new(Service::new(Config::new(&args.hosts), sup, history, tx));

    let settings = match svc.start_all() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not load {}: {e}", args.hosts.display());
            return std::process::ExitCode::FAILURE;
        }
    };

    let n = svc.list_hosts().map(|h| h.len()).unwrap_or(0);
    eprintln!(
        "watching {n} hosts from {} at {}",
        args.hosts.display(),
        tuxtop_core::sampler::rate_label(settings.interval_ms),
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

    let addr = args.addr();
    let state = Arc::new(AppState {
        svc,
        web: args.web,
        writable: args.writable,
        events: bcast,
    });

    let listener = match tokio::net::TcpListener::bind(addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("could not bind {addr}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    // What is reachable, not merely where it bound. The wildcard is refused
    // with --writable and allowed without it, so this line is the only place a
    // read-only server open to the world says so.
    eprintln!(
        "listening on http://{addr}/ - {}, {}",
        reach(args.bind),
        if args.writable {
            "writable"
        } else {
            "read-only"
        }
    );
    if args.writable && !args.bind.is_loopback() {
        eprintln!(
            "WARNING: --writable on {}, which is not loopback. Anyone who reaches",
            args.bind
        );
        eprintln!("         this port can make this machine open SSH connections using its");
        eprintln!("         own keys. Establish identity in front of it.");
    }

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

#[cfg(test)]
mod tests {
    //! The command line's two rules, and the one that must not be able to pass
    //! for the wrong reason.
    //!
    //! `parse_args` was lifted out of `main` to make these writable at all.
    //! Neither could be asserted while the parse lived inside an `async fn
    //! main` that binds a socket on its way to the check.

    use super::*;
    use std::net::Ipv6Addr;

    fn parse(args: &[&str]) -> Result<Args, ArgError> {
        match parse_args(args.iter().map(|s| (*s).to_string())) {
            Ok(Parsed::Run(a)) => Ok(a),
            Ok(Parsed::Help) => panic!("no case here passes --help"),
            Err(e) => Err(e),
        }
    }

    #[test]
    fn no_bind_flag_still_binds_loopback() {
        // The default is the whole of the old behaviour. `--bind` shipping with
        // a wildcard default, or with none, would open every install that never
        // asked for one.
        let a = parse(&[]).expect("a bare command line is valid");
        assert_eq!(a.bind, IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(a.addr(), SocketAddr::from(([127, 0, 0, 1], 8787)));

        // Still loopback when other flags are present: a default that survives
        // only an empty command line is not a default.
        let a = parse(&["--port", "9000", "--writable", "--web", "dist"]).unwrap();
        assert!(a.bind.is_loopback(), "{} is not loopback", a.bind);
        assert_eq!(a.addr(), SocketAddr::from(([127, 0, 0, 1], 9000)));
    }

    #[test]
    fn the_wildcard_with_writable_is_refused() {
        for w in ["0.0.0.0", "::"] {
            let ip: IpAddr = w.parse().unwrap();
            assert_eq!(
                parse(&["--bind", w, "--writable"]),
                Err(ArgError::WritableWildcard(ip)),
                "{w} with --writable was not refused by the rule that exists for it"
            );
            // Asserting only `is_err` would stay green with the check deleted,
            // as long as anything else in the line were rejected. So: the exact
            // variant above, and below that neither half alone is refused -
            // which is what makes the combination the only reason it fails.
            assert!(
                parse(&["--bind", w]).is_ok(),
                "{w} alone is allowed; a read-only server may listen anywhere"
            );
            // Order is not the rule either.
            assert_eq!(
                parse(&["--writable", "--bind", w]),
                Err(ArgError::WritableWildcard(ip))
            );
            let msg = parse(&["--bind", w, "--writable"]).unwrap_err().to_string();
            assert!(
                msg.contains("--bind") && msg.contains("--writable"),
                "the refusal must name both flags, or it is a puzzle: {msg}"
            );
        }
        // A named non-loopback address with --writable is a deployment choice,
        // not the refused case. It gets the loud startup line instead.
        assert!(parse(&["--bind", "10.0.0.5", "--writable"]).is_ok());
        assert!(parse(&["--bind", "fd00::1", "--writable"]).is_ok());
        assert!(parse(&["--writable"]).is_ok());
    }

    #[test]
    fn a_hostname_is_never_resolved_at_bind_time() {
        // Resolution can hand you an address you did not ask for, and can hand
        // you a different one tomorrow. An IP is what was typed and what binds.
        for name in ["localhost", "tuxtop.example.com", "0.0.0.0:8787", ""] {
            assert_eq!(
                parse(&["--bind", name]),
                Err(ArgError::BadBind(name.to_string())),
                "{name:?} was accepted as an address"
            );
        }
        assert_eq!(
            parse(&["--bind", "::1"]).unwrap().bind,
            IpAddr::V6(Ipv6Addr::LOCALHOST)
        );
    }
}
