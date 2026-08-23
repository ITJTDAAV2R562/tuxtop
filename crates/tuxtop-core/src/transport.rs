//! Getting frames off a remote host.
//!
//! Tuxtop shells out to the system `ssh` binary rather than linking an SSH
//! library. See ADR-007 — briefly: OpenSSH ships on Windows 10+ and every
//! Linux, and delegating to it means `~/.ssh/config`, `ProxyJump`, agent auth,
//! `known_hosts` and hardware keys all work for free and identically to the
//! user's terminal. There is no crypto here for us to get wrong.
//!
//! One long-lived process per host — *not* one per sample. The process runs
//! the loop from [`crate::sampler::sampler_command`] and streams frames until
//! it is dropped.

use std::process::Stdio;
use std::time::Instant;

use tokio::io::{AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;

use std::sync::Arc;

use crate::model::{HostConfig, HostFault, Sample};
use crate::sampler::{self, RateTracker};
use crate::traffic::TrafficCounter;

/// Build the argument list for the `ssh` invocation.
///
/// Split out from spawning so it can be unit-tested without a network.
pub fn ssh_args(host: &HostConfig, remote_cmd: &str) -> Vec<String> {
    let mut args: Vec<String> = [
        // Fail fast instead of hanging on an unreachable host: the UI wants a
        // `HostFault` to display, not an indefinite spinner.
        "-o",
        "ConnectTimeout=8",
        // Keep the pipe alive through NAT idle timeouts, and notice a dead
        // peer rather than waiting forever on a socket nobody will close.
        "-o",
        "ServerAliveInterval=5",
        "-o",
        "ServerAliveCountMax=3",
        // Never block on an interactive prompt; there is no terminal to
        // answer a passphrase or a host-key question.
        "-o",
        "BatchMode=yes",
        // No pty: we want a clean byte stream, not line-discipline echo
        // and \r\n translation corrupting the frames.
        "-T",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();

    if host.port != 22 {
        args.push("-p".into());
        args.push(host.port.to_string());
    }

    // `user@addr` only when a user is configured; a bare alias lets
    // ~/.ssh/config supply the user, which is usually what is wanted.
    if host.user.is_empty() {
        args.push(host.addr.clone());
    } else {
        args.push(format!("{}@{}", host.user, host.addr));
    }

    args.push(remote_cmd.to_string());
    args
}

/// A running sampler process. Dropping this kills the remote loop.
pub struct SshSampler {
    child: Child,
}

impl SshSampler {
    /// Start sampling `host` every `interval_secs`, sending results to `tx`.
    ///
    /// Returns once the process is spawned; sampling continues in a background
    /// task. Errors after startup arrive through `tx` as [`HostFault`] rather
    /// than being returned, because by then there is a card on screen that
    /// needs to say what went wrong.
    pub fn start(
        host: HostConfig,
        interval_secs: u32,
        tx: mpsc::Sender<Result<Sample, HostFault>>,
        traffic: Arc<TrafficCounter>,
    ) -> std::io::Result<Self> {
        // Windows has no /proc. Same transport, same loop, same frame
        // delimiter - only the remote command and its parser differ.
        let windows = host.os.eq_ignore_ascii_case("windows");
        let cmd = if windows {
            crate::windows::win_sampler_command(interval_secs)
        } else {
            sampler::sampler_command(interval_secs)
        };
        let args = ssh_args(&host, &cmd);

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let stderr = child.stderr.take().expect("stderr was piped");

        // ssh writes auth and connection failures to stderr and then exits.
        // Capture it so a fault can name the real cause instead of reporting a
        // bare "process ended".
        let (etx, erx) = tokio::sync::oneshot::channel();
        tokio::spawn(async move {
            let mut buf = String::new();
            let mut r = BufReader::new(stderr);
            let _ = r.read_to_string(&mut buf).await;
            let _ = etx.send(buf);
        });

        if windows {
            tokio::spawn(win_pump(host, stdout, erx, tx, traffic));
        } else {
            tokio::spawn(pump(host, stdout, erx, tx, traffic));
        }

        Ok(Self { child })
    }

    /// Terminate the remote loop.
    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

/// Read stdout, split frames, differentiate, and emit samples.
async fn pump(
    host: HostConfig,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::sync::oneshot::Receiver<String>,
    tx: mpsc::Sender<Result<Sample, HostFault>>,
    traffic: Arc<TrafficCounter>,
) {
    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    let mut chunk = vec![0u8; 16 * 1024];
    let mut tracker = RateTracker::new();
    let mut last = Instant::now();

    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) => break, // process exited
            Ok(n) => n,
            Err(e) => {
                let _ = tx
                    .send(Err(HostFault::SamplerFailed(format!("read failed: {e}"))))
                    .await;
                return;
            }
        };

        // /proc is ASCII. Lossy conversion cannot corrupt a field, and a
        // hard error here would kill an otherwise healthy host over one
        // stray byte.
        traffic.add_bytes(n as u64);
        buf.push_str(&String::from_utf8_lossy(&chunk[..n]));

        let (frames, rest) = sampler::split_frames(&buf);
        let mut emitted = Vec::new();

        for text in frames {
            traffic.add_frame(text.len() as u64);
            let frame = sampler::parse_frame(text);
            let now = Instant::now();
            let elapsed = now.duration_since(last).as_secs_f64();
            last = now;

            let cores_len = frame.stat.cores.len();
            let agg = frame.stat.aggregate;
            let mem = frame.mem;
            let load = frame.load;
            let cpu_temp_c = frame.cpu_temp_c;
            let temps = frame.temps.clone();
            let gpu = frame.gpu.clone();
            let uptime_secs = frame.uptime_secs;
            let facts = frame.facts.clone();
            let filesystems = frame.filesystems.clone();
            let swap_total_kb = frame.mem.swap_total_kb;
            let swap_used_kb = frame
                .mem
                .swap_total_kb
                .saturating_sub(frame.mem.swap_free_kb);
            let prev_agg = tracker.prev_aggregate();
            let (rates, cores) = tracker.push(frame, elapsed);

            // A frame with no CPU rows means the remote cat produced nothing
            // useful — a container with a masked /proc, or a wedged host.
            // Surface it rather than emitting a card full of zeros.
            if cores_len == 0 {
                emitted.push(Err(HostFault::SamplerFailed(
                    "no cpu rows in /proc/stat".into(),
                )));
                continue;
            }

            emitted.push(Ok(Sample {
                host: host.name.clone(),
                cpu: rates.cpu,
                cores,
                mem_used_kb: mem.used_kb(),
                mem_total_kb: mem.total_kb,
                net_rx_bps: rates.net_rx_bps,
                net_tx_bps: rates.net_tx_bps,
                disk_read_bps: rates.disk_read_bps,
                disk_write_bps: rates.disk_write_bps,
                gpu,
                load,
                cpu_temp_c,
                temps,
                swap_used_kb,
                swap_total_kb,
                uptime_secs,
                cpu_breakdown: prev_agg
                    .map(|p| crate::proc::breakdown(&p, &agg))
                    .unwrap_or_default(),
                facts,
                filesystems,
            }));
        }

        let rest = rest.to_string();
        buf = rest;

        for item in emitted {
            if tx.send(item).await.is_err() {
                return; // receiver dropped; nobody is listening
            }
        }
    }

    // Process ended. Whatever ssh said on stderr is the real explanation.
    let msg = stderr.await.unwrap_or_default();
    let trimmed = msg.trim();
    let fault = classify_ssh_error(trimmed);
    let _ = tx.send(Err(fault)).await;
}

/// A running process sampler: its own connection, its own cadence.
///
/// Separate from [`SshSampler`] because the ranking needs two snapshots a
/// second apart, and doing that inside the metric loop would stall 1 Hz
/// sampling for the whole window. Started only while the process view is
/// open, so a view nobody is looking at costs nothing.
pub struct ProcSampler {
    child: Child,
}

impl ProcSampler {
    pub fn start(
        host: HostConfig,
        top_n: usize,
        window_ms: u32,
        interval_secs: u32,
        tx: mpsc::Sender<crate::procs::ProcFrame>,
    ) -> std::io::Result<Self> {
        let cmd = crate::procs::process_loop_command(top_n, window_ms, interval_secs);
        let args = ssh_args(&host, &cmd);

        let mut child = Command::new("ssh")
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped");
        tokio::spawn(proc_pump(host, stdout, tx));

        Ok(Self { child })
    }

    pub async fn stop(mut self) {
        let _ = self.child.kill().await;
    }
}

async fn proc_pump(
    host: HostConfig,
    stdout: tokio::process::ChildStdout,
    tx: mpsc::Sender<crate::procs::ProcFrame>,
) {
    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    let mut chunk = vec![0u8; 16 * 1024];
    // Cgroup CPU is a cumulative counter, so it needs the previous frame and
    // the real time between them. Kept here, per connection, for the same
    // reason `RateTracker` is: the configured interval is what we asked for,
    // not what we got.
    let mut rates = crate::procs::CgroupRates::new();
    let mut restarts = crate::procs::RestartTracker::new();
    let mut last_at: Option<std::time::Instant> = None;

    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        buf.push_str(&String::from_utf8_lossy(&chunk[..n]));

        let (frames, rest) = sampler::split_frames(&buf);
        let texts: Vec<String> = frames.into_iter().map(str::to_string).collect();
        buf = rest.to_string();

        for text in texts {
            let procs = crate::procs::parse_processes(&host.name, &text);
            // An empty frame means the denominator was missing, which is
            // reported as nothing rather than as an idle machine.
            if procs.is_empty() {
                continue;
            }

            let now = std::time::Instant::now();
            let elapsed = last_at.map(|t| now.duration_since(t).as_secs_f64());
            last_at = Some(now);
            let cg = crate::procs::parse_cgroups(&text);
            // The first frame has nothing to differentiate against; its
            // cgroups still carry memory and pid counts, which need no delta.
            let cgroups = rates.update(&cg, elapsed.unwrap_or(0.0));

            // Empty on cycles that did not sweep, which the consumer keeps
            // rather than reading as "nothing has restarted".
            let seen = crate::procs::parse_restarts(&text);
            let frame = crate::procs::ProcFrame {
                host: host.name.clone(),
                procs,
                cgroups,
                restarts: restarts.update(&seen),
            };
            if tx.send(frame).await.is_err() {
                return;
            }
        }
    }
}

/// Turn ssh's stderr into a typed fault.
///
/// Distinguishing auth failure from unreachable is the difference between a
/// thirty-second fix and an hour of guessing, so these are never collapsed
/// into a generic "offline".
pub fn classify_ssh_error(stderr: &str) -> HostFault {
    let lower = stderr.to_ascii_lowercase();

    if lower.contains("permission denied")
        || lower.contains("no matching host key")
        || lower.contains("host key verification failed")
        || lower.contains("too many authentication failures")
    {
        return HostFault::AuthFailed(first_useful_line(stderr));
    }

    if lower.contains("could not resolve")
        || lower.contains("name or service not known")
        || lower.contains("connection refused")
        || lower.contains("connection timed out")
        || lower.contains("no route to host")
        || lower.contains("network is unreachable")
        || lower.contains("operation timed out")
    {
        return HostFault::Unreachable(first_useful_line(stderr));
    }

    if stderr.is_empty() {
        return HostFault::SamplerFailed("ssh exited without a message".into());
    }

    HostFault::SamplerFailed(first_useful_line(stderr))
}

/// ssh often prints banners and warnings before the real error; take the last
/// non-empty line, which is almost always the operative one.
fn first_useful_line(s: &str) -> String {
    s.lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())
        .unwrap_or("unknown error")
        .to_string()
}

/// Read a Windows host's frames and emit samples.
///
/// The Linux pump's shape, with the counters that differ. CPU comes from an
/// inverse idle counter differentiated against its own timestamp; network and
/// disk are cumulative byte counters differentiated against real elapsed
/// time, exactly as `RateTracker` does for `/proc`.
async fn win_pump(
    host: HostConfig,
    stdout: tokio::process::ChildStdout,
    stderr: tokio::sync::oneshot::Receiver<String>,
    tx: mpsc::Sender<Result<Sample, HostFault>>,
    traffic: Arc<TrafficCounter>,
) {
    use crate::windows::{busy_pct, parse_win_frame, WinFrame};

    let mut reader = BufReader::new(stdout);
    let mut buf = String::new();
    let mut chunk = vec![0u8; 16 * 1024];

    let mut prev: Option<WinFrame> = None;
    let mut prev_at: Option<std::time::Instant> = None;
    // Facts arrive once, before the loop, so they are carried forward rather
    // than re-read from every frame.
    let mut facts = crate::facts::HostFacts::default();

    loop {
        let n = match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        traffic.add_bytes(n as u64);
        buf.push_str(&String::from_utf8_lossy(&chunk[..n]));

        let (frames, rest) = sampler::split_frames(&buf);
        let texts: Vec<String> = frames.into_iter().map(str::to_string).collect();
        buf = rest.to_string();

        for text in texts {
            traffic.add_frame(text.len() as u64);
            let f = parse_win_frame(&text);
            if !f.facts.is_empty() {
                facts = f.facts.clone();
            }
            // A frame with no processor rows is not a measurement. PowerShell
            // can emit a warning-only frame if a class query is refused.
            if f.cores.is_empty() {
                continue;
            }

            let now = std::time::Instant::now();
            let elapsed = prev_at.map(|t| now.duration_since(t).as_secs_f64());
            prev_at = Some(now);

            let (cpu, cores) = match &prev {
                // The first frame has nothing to differentiate against. It is
                // skipped rather than reported as zero, the same way the
                // Linux path treats its first /proc/stat read as a baseline.
                None => (0.0, Vec::new()),
                Some(p) => {
                    let mut cs = Vec::with_capacity(f.cores.len());
                    for (name, v, t) in &f.cores {
                        let before = p.cores.iter().find(|c| &c.0 == name);
                        cs.push(
                            before
                                .and_then(|b| busy_pct((b.1, b.2), (*v, *t)))
                                .unwrap_or(0.0),
                        );
                    }
                    // The aggregate is Windows' own `_Total`, not a mean of
                    // the cores: it is computed by the same counters at the
                    // same instant, and averaging ours would drift from it.
                    let agg = match (p.total, f.total) {
                        (Some(a), Some(b)) => busy_pct(a, b).unwrap_or(0.0),
                        _ if !cs.is_empty() => cs.iter().sum::<f32>() / cs.len() as f32,
                        _ => 0.0,
                    };
                    (agg, cs)
                }
            };

            let rate = |a: u64, b: u64| -> f64 {
                match (elapsed, b.checked_sub(a)) {
                    (Some(e), Some(d)) if e > 0.0 => d as f64 / e,
                    _ => 0.0,
                }
            };
            let sum = |v: &[(String, u64, u64)]| -> (u64, u64) {
                v.iter().fold((0, 0), |acc, x| (acc.0 + x.1, acc.1 + x.2))
            };
            let (nr, nt) = sum(&f.nets);
            let (dr, dw) = sum(&f.disks);
            let (pnr, pnt) = prev.as_ref().map(|p| sum(&p.nets)).unwrap_or((0, 0));
            let (pdr, pdw) = prev.as_ref().map(|p| sum(&p.disks)).unwrap_or((0, 0));

            let ready = prev.is_some();
            prev = Some(f.clone());
            if !ready {
                continue;
            }

            let sample = Sample {
                host: host.name.clone(),
                cpu,
                cores,
                mem_used_kb: f.mem_total_kb.saturating_sub(f.mem_free_kb),
                mem_total_kb: f.mem_total_kb,
                net_rx_bps: rate(pnr, nr) as u64,
                net_tx_bps: rate(pnt, nt) as u64,
                disk_read_bps: rate(pdr, dr) as u64,
                disk_write_bps: rate(pdw, dw) as u64,
                gpu: None,
                // Windows has no load average. Reporting zeros would claim a
                // measurement; the UI hides the metric when no host has it.
                load: [0.0; 3],
                cpu_temp_c: None,
                temps: Vec::new(),
                swap_used_kb: 0,
                swap_total_kb: 0,
                uptime_secs: f.uptime_secs,
                cpu_breakdown: Default::default(),
                facts: Some(facts.clone()),
                filesystems: Vec::new(),
            };

            if tx.send(Ok(sample)).await.is_err() {
                return;
            }
        }
    }

    let msg = stderr.await.unwrap_or_default();
    let _ = tx.send(Err(classify_ssh_error(&msg))).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host() -> HostConfig {
        HostConfig {
            name: "dove".into(),
            addr: "dove.example".into(),
            user: "sam".into(),
            port: 22,
            beszel_url: None,
            interval_secs: None,
            os: String::new(),
            group: None,
        }
    }

    #[test]
    fn args_use_batch_mode_and_no_pty() {
        let a = ssh_args(&host(), "true");
        assert!(
            a.contains(&"BatchMode=yes".to_string()),
            "must never prompt"
        );
        assert!(
            a.contains(&"-T".to_string()),
            "no pty: stream must stay clean"
        );
    }

    #[test]
    fn args_omit_port_when_default() {
        let a = ssh_args(&host(), "true");
        assert!(!a.contains(&"-p".to_string()), "22 is implied; -p is noise");
    }

    #[test]
    fn args_include_non_default_port() {
        let mut h = host();
        h.port = 2222;
        let a = ssh_args(&h, "true");
        let i = a.iter().position(|x| x == "-p").expect("-p present");
        assert_eq!(a[i + 1], "2222");
    }

    #[test]
    fn empty_user_defers_to_ssh_config() {
        let mut h = host();
        h.user = String::new();
        let a = ssh_args(&h, "true");
        assert!(a.contains(&"dove.example".to_string()));
        assert!(!a.iter().any(|x| x.contains('@')), "no user@ when unset");
    }

    #[test]
    fn remote_command_is_the_last_argument() {
        let a = ssh_args(&host(), "echo hi");
        assert_eq!(a.last().unwrap(), "echo hi");
    }

    #[test]
    fn auth_failures_are_distinguished_from_unreachable() {
        assert!(matches!(
            classify_ssh_error("sam@dove: Permission denied (publickey)."),
            HostFault::AuthFailed(_)
        ));
        assert!(matches!(
            classify_ssh_error("ssh: connect to host dove port 22: Connection refused"),
            HostFault::Unreachable(_)
        ));
        assert!(matches!(
            classify_ssh_error("ssh: Could not resolve hostname dove: Name or service not known"),
            HostFault::Unreachable(_)
        ));
    }

    #[test]
    fn host_key_problems_count_as_auth_not_unreachable() {
        // We reached the host fine; the trust relationship is the problem.
        assert!(matches!(
            classify_ssh_error("Host key verification failed."),
            HostFault::AuthFailed(_)
        ));
    }

    #[test]
    fn silent_exit_still_produces_a_stated_reason() {
        match classify_ssh_error("") {
            HostFault::SamplerFailed(m) => assert!(!m.is_empty()),
            other => panic!("expected SamplerFailed, got {other:?}"),
        }
    }

    #[test]
    fn banner_noise_does_not_mask_the_real_error() {
        let stderr = "Warning: Permanently added 'dove' to known hosts.\n\
                      sam@dove: Permission denied (publickey).";
        match classify_ssh_error(stderr) {
            HostFault::AuthFailed(m) => assert!(m.contains("Permission denied"), "got {m}"),
            other => panic!("expected AuthFailed, got {other:?}"),
        }
    }
}
