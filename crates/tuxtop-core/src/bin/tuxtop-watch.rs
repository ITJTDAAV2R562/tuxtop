//! `tuxtop-watch` — the fast plane, in a terminal.
//!
//! Proves the sampler end to end before any GUI exists, and stays useful
//! afterwards as the way to check whether a problem is in the data or in the
//! window.
//!
//! ```text
//! cargo run --bin tuxtop-watch -- dove
//! cargo run --bin tuxtop-watch -- sam@dove.example --interval 1
//! ```

use std::env;
use std::process::ExitCode;

use tokio::sync::mpsc;
use tuxtop_core::model::{HostConfig, HostFault};
use tuxtop_core::transport::SshSampler;

const USAGE: &str = "\
tuxtop-watch — live per-core CPU from a remote Linux host over SSH

USAGE:
    tuxtop-watch <host> [--interval SECS] [--plain]

    <host>        ssh target: an alias from ~/.ssh/config, hostname, or user@host
    --interval    seconds between samples (default 1)
    --plain       no ANSI colour or cursor movement; one line per sample

Nothing is installed on the target. Auth uses your existing ssh agent
and ~/.ssh/config, exactly as `ssh <host>` would.
";

struct Args {
    host: String,
    interval: u32,
    plain: bool,
    /// Exit after this many samples. Makes the run scriptable, and makes the
    /// cost report at exit reachable - under a signal it never prints.
    frames: Option<u64>,
}

fn parse_args() -> Result<Args, String> {
    let mut host = None;
    let mut interval = 1u32;
    let mut plain = false;
    let mut frames = None;
    let mut it = env::args().skip(1);

    while let Some(a) = it.next() {
        match a.as_str() {
            "-h" | "--help" => return Err(String::new()),
            "--plain" => plain = true,
            "--frames" => {
                let v = it.next().ok_or("--frames needs a value")?;
                frames = Some(v.parse().map_err(|_| format!("bad frame count: {v}"))?);
            }
            "--interval" => {
                let v = it.next().ok_or("--interval needs a value")?;
                interval = v.parse().map_err(|_| format!("bad interval: {v}"))?;
                if interval == 0 {
                    return Err("interval must be at least 1 second".into());
                }
            }
            other if other.starts_with('-') => return Err(format!("unknown flag: {other}")),
            other => host = Some(other.to_string()),
        }
    }

    Ok(Args {
        host: host.ok_or("no host given")?,
        interval,
        plain,
        frames,
    })
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

    // Split user@host so ~/.ssh/config still resolves a bare alias.
    let (user, addr) = match args.host.split_once('@') {
        Some((u, a)) => (u.to_string(), a.to_string()),
        None => (String::new(), args.host.clone()),
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

    let (tx, mut rx) = mpsc::channel(16);
    let traffic = std::sync::Arc::new(tuxtop_core::TrafficCounter::new());
    let sampler = match SshSampler::start(cfg.clone(), args.interval, tx, traffic.clone()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("could not launch ssh: {e}");
            eprintln!("is the OpenSSH client installed and on PATH?");
            return ExitCode::FAILURE;
        }
    };

    eprintln!("connecting to {} ...", cfg.addr);

    let mut first = true;
    let mut rendered_lines = 0usize;
    let mut seen = 0u64;

    while let Some(item) = rx.recv().await {
        match item {
            Ok(s) => {
                if first {
                    eprintln!(
                        "connected — {} cores. first sample is a baseline; \
                         rates start on the second.",
                        s.cores.len()
                    );
                    if let Some(f) = &s.facts {
                        eprintln!("  {}  |  {}  |  {}", f.cpu_model, f.os, f.kernel);
                    }
                    if let Some(u) = s.uptime_secs {
                        eprintln!("  up {}", human_uptime(u));
                    }
                    eprintln!();
                    first = false;
                }
                if !s.filesystems.is_empty() {
                    for fs in &s.filesystems {
                        eprintln!(
                            "  disk {:<20} {:>5.1}%  ({:.1} / {:.1} GB)",
                            fs.mount,
                            fs.used_pct(),
                            fs.used_kb as f64 / 1048576.0,
                            fs.total_kb as f64 / 1048576.0
                        );
                    }
                }
                if args.plain {
                    println!(
                        "cpu {:5.1}%  mem {:5.1}%  net rx {:>9} tx {:>9}  load {:.2}  temp {}",
                        s.cpu,
                        pct(s.mem_used_kb, s.mem_total_kb),
                        bytes(s.net_rx_bps),
                        bytes(s.net_tx_bps),
                        s.load[0],
                        temp(s.cpu_temp_c),
                    );
                    let b = s.cpu_breakdown;
                    println!(
                        "  usr {:4.1}%  sys {:4.1}%  io {:4.1}%  steal {:4.1}%   swap {:5.1}%",
                        b.user,
                        b.system,
                        b.iowait,
                        b.steal,
                        pct(s.swap_used_kb, s.swap_total_kb),
                    );
                    if let Some(g) = &s.gpu {
                        println!(
                            "  gpu {} {:.0}%  {} / {} MiB  {:.0}W",
                            g.name, g.util_pct, g.mem_used_mb, g.mem_total_mb, g.power_w
                        );
                    }
                } else {
                    if rendered_lines > 0 {
                        // Redraw in place.
                        print!("\x1b[{rendered_lines}A");
                    }
                    rendered_lines = render(&s);
                }

                seen += 1;
                if args.frames.is_some_and(|n| seen >= n) {
                    break;
                }
            }
            Err(f) => {
                if !args.plain && rendered_lines > 0 {
                    println!();
                }
                eprintln!("\n{}", describe(&f));
                sampler.stop().await;
                return ExitCode::FAILURE;
            }
        }
    }

    sampler.stop().await;
    report_cost(&traffic, args.interval);
    ExitCode::SUCCESS
}

/// What this session actually cost, printed on exit.
fn report_cost(t: &tuxtop_core::TrafficCounter, interval: u32) {
    let s = t.snapshot();
    if s.frames_total == 0 {
        return;
    }
    let per_sec = s.bytes_per_sec_at(interval);
    eprintln!(
        "\n{} frames, {} mean/frame, {}/s at {}s -- {:.2} GB/day for this one host",
        s.frames_total,
        bytes(s.mean_frame_bytes() as u64),
        bytes(per_sec as u64),
        interval,
        per_sec * 86400.0 / 1024.0 / 1024.0 / 1024.0,
    );
}

/// Draw the core grid. Returns how many lines were printed, so the next
/// frame can move the cursor back up over exactly this much.
fn render(s: &tuxtop_core::model::Sample) -> usize {
    let mut lines = 0;

    println!(
        "\x1b[1m{:<16}\x1b[0m cpu \x1b[1m{:5.1}%\x1b[0m   mem {:5.1}% ({} / {})   \
         load {:.2} {:.2} {:.2}\x1b[K",
        s.host,
        s.cpu,
        pct(s.mem_used_kb, s.mem_total_kb),
        gib(s.mem_used_kb),
        gib(s.mem_total_kb),
        s.load[0],
        s.load[1],
        s.load[2],
    );
    lines += 1;

    println!(
        "{:<16} net rx {:>9}/s  tx {:>9}/s   disk r {:>9}/s  w {:>9}/s   cpu {}\x1b[K",
        "",
        bytes(s.net_rx_bps),
        bytes(s.net_tx_bps),
        bytes(s.disk_read_bps),
        bytes(s.disk_write_bps),
        temp(s.cpu_temp_c),
    );
    lines += 1;

    if let Some(g) = &s.gpu {
        println!(
            "{:<16} gpu \x1b[1m{:.0}%\x1b[0m  {} / {} MiB  {:.0}W  ({})\x1b[K",
            "", g.util_pct, g.mem_used_mb, g.mem_total_mb, g.power_w, g.name
        );
        lines += 1;
    }

    println!("\x1b[K");
    lines += 1;

    // Eight cores per row keeps it readable on a 32-core box.
    for chunk in s.cores.chunks(8) {
        let mut row = String::from("  ");
        for (i, v) in chunk.iter().enumerate() {
            row.push_str(&format!("{}{:3.0}% {} ", colour(*v), v, bar(*v)));
            if i < chunk.len() - 1 {
                row.push_str("\x1b[0m ");
            }
        }
        row.push_str("\x1b[0m\x1b[K");
        println!("{row}");
        lines += 1;
    }

    lines
}

/// Same three bands the GUI uses: accent below 75, amber to 89, red above.
fn colour(v: f32) -> &'static str {
    if v >= 90.0 {
        "\x1b[31m"
    } else if v >= 75.0 {
        "\x1b[33m"
    } else if v >= 1.0 {
        "\x1b[36m"
    } else {
        "\x1b[90m"
    }
}

/// A five-cell bar using block elements, so load reads without parsing digits.
fn bar(v: f32) -> String {
    const CELLS: usize = 5;
    let filled = ((v / 100.0) * CELLS as f32).round() as usize;
    let mut s = String::new();
    for i in 0..CELLS {
        s.push(if i < filled { '█' } else { '·' });
    }
    s
}

/// A host with no CPU sensor - normal on a VM - shows a dash, never a zero
/// that would read as an implausibly cold CPU.
fn temp(c: Option<f32>) -> String {
    match c {
        Some(v) => format!("{v:.0}C"),
        None => "  -".into(),
    }
}

fn human_uptime(secs: u64) -> String {
    let d = secs / 86400;
    let h = (secs % 86400) / 3600;
    let m = (secs % 3600) / 60;
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else {
        format!("{m}m")
    }
}

fn pct(used: u64, total: u64) -> f32 {
    if total == 0 {
        return 0.0;
    }
    used as f32 / total as f32 * 100.0
}

fn gib(kb: u64) -> String {
    format!("{:.1}G", kb as f64 / 1024.0 / 1024.0)
}

fn bytes(b: u64) -> String {
    const U: [&str; 4] = ["B", "K", "M", "G"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", v as u64, U[i])
    } else {
        format!("{v:.1} {}", U[i])
    }
}

/// Say what went wrong and what to do about it.
fn describe(f: &HostFault) -> String {
    match f {
        HostFault::AuthFailed(m) => format!(
            "authentication failed: {m}\n\
             check that your key is loaded: `ssh-add -l`, and that `ssh <host>` works."
        ),
        HostFault::Unreachable(m) => format!(
            "host unreachable: {m}\n\
             check the address and that you are on the right network or VPN."
        ),
        HostFault::SamplerFailed(m) => format!(
            "sampler failed: {m}\n\
             the host answered but /proc could not be read as expected."
        ),
        HostFault::Stalled { since_secs } => {
            format!("no data for {since_secs}s — the connection stalled.")
        }
    }
}
