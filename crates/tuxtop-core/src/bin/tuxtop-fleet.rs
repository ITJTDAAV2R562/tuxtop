//! `tuxtop-fleet` — many hosts at once, in a terminal.
//!
//! Exists to verify the one claim in this project that no test can make: that
//! hosts are genuinely isolated from each other. Killing one host's ssh must
//! leave every other host streaming without a stutter.
//!
//! It runs the *same* `fleet::watch_host` the GUI supervisor runs, so what it
//! proves is true of the app rather than of a re-implementation.
//!
//! ```text
//! cargo run --bin tuxtop-fleet -- dove wader coot owl heron
//! ```
//!
//! Frame counts are the evidence. A host whose count keeps advancing after
//! another host's connection is killed was not affected by it.

use std::collections::BTreeMap;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;
use tuxtop_core::fleet::{watch_host, HostEvent};
use tuxtop_core::model::HostConfig;
use tuxtop_core::TrafficCounter;

const USAGE: &str = "\
tuxtop-fleet — watch several hosts at once and prove they are independent

USAGE:
    tuxtop-fleet <host>... [--interval SECS] [--seconds N]

    <host>       ssh targets: aliases from ~/.ssh/config, or user@host
    --interval   seconds between samples (default 1)
    --seconds    stop after N seconds (default: run until interrupted)

Prints a status line every second with each host's cumulative frame count,
and an event line whenever a host faults or recovers.

To verify isolation, kill one host's ssh process from another terminal and
watch the other hosts' counts keep advancing:

    pkill -f 'ssh.*<hostname>'          # Linux
    taskkill /PID <pid> /F              # Windows, from Task Manager

Never stop sshd on the remote host to test this. That is not a test, it is
a way to lock yourself out of the machine.
";

struct Args {
    hosts: Vec<String>,
    interval: u32,
    seconds: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut hosts = Vec::new();
    let mut interval = 1u32;
    let mut seconds = None;
    let mut it = std::env::args().skip(1);

    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--interval" => {
                let v = it.next().ok_or("--interval needs a value")?;
                interval = v.parse().map_err(|_| format!("bad interval: {v}"))?;
                if interval == 0 {
                    return Err("interval must be at least 1 second".into());
                }
            }
            "--seconds" => {
                let v = it.next().ok_or("--seconds needs a value")?;
                seconds = Some(v.parse().map_err(|_| format!("bad duration: {v}"))?);
            }
            other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
            other => hosts.push(other.to_string()),
        }
    }

    if hosts.is_empty() {
        return Err("no hosts given".into());
    }
    Ok(Args {
        hosts,
        interval,
        seconds,
    })
}

/// What we know about one host, for the status line.
#[derive(Default)]
struct HostState {
    frames: u64,
    cores: usize,
    cpu: f32,
    fault: Option<String>,
    last: Option<Instant>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if msg.is_empty() {
                print!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            eprintln!("error: {msg}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };

    let (tx, mut rx) = mpsc::channel(256);
    let mut state: BTreeMap<String, HostState> = BTreeMap::new();

    for spec in &args.hosts {
        let (user, addr) = match spec.split_once('@') {
            Some((u, a)) => (u.to_string(), a.to_string()),
            None => (String::new(), spec.clone()),
        };
        let cfg = HostConfig {
            name: addr.clone(),
            addr,
            user,
            port: 22,
            beszel_url: None,
            interval_secs: None,
            os: String::new(),
            group: None,
        };
        state.insert(cfg.name.clone(), HostState::default());
        tokio::spawn(watch_host(
            cfg,
            args.interval,
            Arc::new(TrafficCounter::new()),
            tx.clone(),
        ));
    }
    // Our own handle would otherwise keep the channel open forever.
    drop(tx);

    let started = Instant::now();
    println!(
        "watching {} hosts at {}s — kill one host's ssh and watch the others' \
         frame counts keep advancing\n",
        state.len(),
        args.interval
    );

    let mut ticker = tokio::time::interval(Duration::from_secs(1));
    ticker.tick().await; // the first tick is immediate

    loop {
        tokio::select! {
            item = rx.recv() => {
                let Some((host, event)) = item else { break };
                let Some(s) = state.get_mut(&host) else { continue };
                match event {
                    HostEvent::Sample(sample) => {
                        if s.fault.is_some() {
                            println!("{:>6.1}s  ++ {host} recovered", started.elapsed().as_secs_f32());
                            s.fault = None;
                        }
                        s.frames += 1;
                        s.cores = sample.cores.len();
                        s.cpu = sample.cpu;
                        s.last = Some(Instant::now());
                    }
                    HostEvent::Fault(f) => {
                        let text = format!("{f:?}");
                        println!("{:>6.1}s  !! {host} {text}", started.elapsed().as_secs_f32());
                        s.fault = Some(text);
                    }
                }
            }
            _ = ticker.tick() => {
                let elapsed = started.elapsed();
                println!("{:>6.1}s  {}", elapsed.as_secs_f32(), status(&state));
                if args.seconds.is_some_and(|n| elapsed.as_secs() >= n) {
                    break;
                }
            }
        }
    }

    println!("\nfinal frame counts:");
    for (name, s) in &state {
        println!(
            "  {name:<12} {:>5} frames  {:>3} cores  {}",
            s.frames,
            s.cores,
            match &s.fault {
                Some(f) => format!("FAULTED: {f}"),
                None => "ok".to_string(),
            }
        );
    }

    // A host that produced nothing at all is a failed run, and should not exit
    // zero — otherwise a scripted check would call a total outage a success.
    if state.values().any(|s| s.frames == 0) {
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// One compact cell per host: name, frame count, current CPU.
fn status(state: &BTreeMap<String, HostState>) -> String {
    state
        .iter()
        .map(|(name, s)| match &s.fault {
            Some(_) => format!("{name} DOWN"),
            None => format!("{name} {}f {:.0}%", s.frames, s.cpu),
        })
        .collect::<Vec<_>>()
        .join("  ")
}
