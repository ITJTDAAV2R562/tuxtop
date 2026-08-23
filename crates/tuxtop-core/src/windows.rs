//! Sampling a Windows host.
//!
//! The same shape as the Linux sampler and for the same reasons — one
//! persistent SSH connection running a loop, raw counters differentiated
//! here rather than trusted from the far side — but Windows has no `/proc`,
//! so the readings come from CIM performance classes instead.
//!
//! Nothing is installed. Windows ships OpenSSH Server as a first-party
//! optional feature and PowerShell in the box; both are already present on
//! the host this was written against.
//!
//! ## The trap this module exists to get right
//!
//! `Win32_PerfRawData_PerfOS_Processor.PercentProcessorTime` is an **inverse**
//! counter: it accumulates *idle* 100-nanosecond ticks, not busy ones. Read
//! the obvious way it reported **79%** on a machine sitting at about 11.
//! Busy is `100 × (1 − Δcounter / Δtimestamp)`.
//!
//! Three tempting alternatives are all worse:
//!
//! - `Win32_PerfFormattedData_*` computes the delta itself, over WMI's own
//!   refresh window. That is the cached-value problem this whole project was
//!   built in response to, in a different costume.
//! - `Get-Counter` paths are **localised**. `"\Processor(_Total)\% Processor
//!   Time"` does not exist on a German or Russian Windows, and fails at
//!   runtime on exactly the machines nobody tests on.
//! - `Win32_Processor.LoadPercentage` is coarse and cached.

use serde::{Deserialize, Serialize};

use crate::facts::HostFacts;

/// The PowerShell script the remote host runs, as plain text.
///
/// Kept separate from the command that carries it so it can be read, and so
/// the encoding below is the only thing between here and the wire.
fn win_script(interval_secs: u32) -> String {
    let secs = interval_secs.max(1);
    // Facts are read once, outside the loop: none of them change between
    // frames, and Win32_Processor is among the slower classes to query.
    format!(
        "$ErrorActionPreference='SilentlyContinue'\n\
         $os=Get-CimInstance Win32_OperatingSystem\n\
         $cpu=Get-CimInstance Win32_Processor | Select-Object -First 1\n\
         'TXWI|os|'+$os.Caption\n\
         'TXWI|kernel|'+$os.Version\n\
         'TXWI|cpu|'+$cpu.Name\n\
         'TXWI|arch|'+$os.OSArchitecture\n\
         while($true){{\n\
           $os=Get-CimInstance Win32_OperatingSystem\n\
           'TXWM|'+$os.TotalVisibleMemorySize+'|'+$os.FreePhysicalMemory\n\
           'TXWU|'+[int]((Get-Date)-$os.LastBootUpTime).TotalSeconds\n\
           foreach($c in Get-CimInstance Win32_PerfRawData_PerfOS_Processor){{\n\
             'TXWC|'+$c.Name+'|'+$c.PercentProcessorTime+'|'+$c.Timestamp_Sys100NS }}\n\
           foreach($n in Get-CimInstance Win32_PerfRawData_Tcpip_NetworkInterface){{\n\
             'TXWN|'+$n.Name+'|'+$n.BytesReceivedPersec+'|'+$n.BytesSentPersec }}\n\
           foreach($d in Get-CimInstance Win32_PerfRawData_PerfDisk_PhysicalDisk){{\n\
             'TXWD|'+$d.Name+'|'+$d.DiskReadBytesPersec+'|'+$d.DiskWriteBytesPersec }}\n\
           '{delim}'\n\
           Start-Sleep -Seconds {secs}\n\
         }}\n",
        delim = crate::sampler::FRAME_DELIMITER,
    )
}

/// UTF-16LE, base64 — the encoding `powershell -EncodedCommand` expects.
///
/// Hand-rolled rather than pulling a dependency in for forty lines, and
/// tested against a known vector.
fn encode_command(script: &str) -> String {
    const TBL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut bytes = Vec::with_capacity(script.len() * 2);
    for u in script.encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }

    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let b = [c[0], *c.get(1).unwrap_or(&0), *c.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TBL[(n >> 18 & 63) as usize] as char);
        out.push(TBL[(n >> 12 & 63) as usize] as char);
        out.push(if c.len() > 1 {
            TBL[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if c.len() > 2 {
            TBL[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// The command to run on the remote host.
///
/// The script is **base64-encoded** rather than quoted inline, because it has
/// to survive whatever shell is between ssh and PowerShell. Inline, `$os`
/// was eaten by a POSIX shell on the way through, and cmd.exe - the default
/// shell for Windows OpenSSH - mangles nested double quotes in its own way.
/// `-EncodedCommand` has no metacharacters at all: the payload is `[A-Za-z0-9+/=]`,
/// which every shell in the chain leaves alone.
pub fn win_sampler_command(interval_secs: u32) -> String {
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        encode_command(&win_script(interval_secs))
    )
}

/// One host's raw Windows counters, before differentiation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WinFrame {
    pub facts: HostFacts,
    /// Kilobytes, as `Win32_OperatingSystem` reports them.
    pub mem_total_kb: u64,
    pub mem_free_kb: u64,
    pub uptime_secs: Option<u64>,
    /// `(name, idle_counter, timestamp)` — one per logical processor, plus
    /// `_Total`, which is kept separate rather than summed.
    pub cores: Vec<(String, u64, u64)>,
    pub total: Option<(u64, u64)>,
    /// `(name, rx_bytes, tx_bytes)` cumulative.
    pub nets: Vec<(String, u64, u64)>,
    /// `(name, read_bytes, write_bytes)` cumulative.
    pub disks: Vec<(String, u64, u64)>,
}

fn num(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok()
}

/// Parse one Windows frame.
///
/// Unparseable lines are skipped rather than failing the frame: PowerShell
/// writes warnings to the same stream, and one noisy line must not cost the
/// whole sample.
pub fn parse_win_frame(text: &str) -> WinFrame {
    let mut f = WinFrame::default();

    for line in text.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(rest) = line.strip_prefix("TXWI|") {
            if let Some((k, v)) = rest.split_once('|') {
                let v = v.trim().to_string();
                match k {
                    "os" => f.facts.os = v,
                    // Windows has no kernel version in the Linux sense; the OS
                    // build number is the closest true equivalent and is
                    // labelled as such rather than invented.
                    "kernel" => f.facts.kernel = format!("Windows build {v}"),
                    "cpu" => f.facts.cpu_model = v,
                    "arch" => f.facts.arch = v,
                    _ => {}
                }
            }
        } else if let Some(rest) = line.strip_prefix("TXWM|") {
            if let Some((t, fr)) = rest.split_once('|') {
                if let (Some(t), Some(fr)) = (num(t), num(fr)) {
                    f.mem_total_kb = t;
                    f.mem_free_kb = fr;
                }
            }
        } else if let Some(rest) = line.strip_prefix("TXWU|") {
            f.uptime_secs = num(rest);
        } else if let Some(rest) = line.strip_prefix("TXWC|") {
            let p: Vec<&str> = rest.split('|').collect();
            if p.len() >= 3 {
                if let (Some(v), Some(t)) = (num(p[1]), num(p[2])) {
                    if p[0].trim() == "_Total" {
                        f.total = Some((v, t));
                    } else {
                        f.cores.push((p[0].trim().to_string(), v, t));
                    }
                }
            }
        } else if let Some(rest) = line.strip_prefix("TXWN|") {
            let p: Vec<&str> = rest.split('|').collect();
            if p.len() >= 3 {
                if let (Some(a), Some(b)) = (num(p[1]), num(p[2])) {
                    f.nets.push((p[0].trim().to_string(), a, b));
                }
            }
        } else if let Some(rest) = line.strip_prefix("TXWD|") {
            let p: Vec<&str> = rest.split('|').collect();
            // `_Total` is a sum the host already computed; keeping it beside
            // the per-disk rows would double every byte.
            if p.len() >= 3 && p[0].trim() != "_Total" {
                if let (Some(a), Some(b)) = (num(p[1]), num(p[2])) {
                    f.disks.push((p[0].trim().to_string(), a, b));
                }
            }
        }
    }
    f
}

/// Busy percentage from two readings of an inverse idle counter.
///
/// `100 × (1 − Δidle / Δtime)`. Taking the ratio directly instead reports an
/// idle machine as nearly pinned — measured at 79% on a host at about 11.
///
/// Returns `None` when the counters did not advance, rather than zero: a
/// frame that covers no time is not a measurement of an idle machine.
pub fn busy_pct(prev: (u64, u64), now: (u64, u64)) -> Option<f32> {
    let d_idle = now.0.checked_sub(prev.0)?;
    let d_time = now.1.checked_sub(prev.1)?;
    if d_time == 0 {
        return None;
    }
    let busy = 1.0 - (d_idle as f64 / d_time as f64);
    Some((busy * 100.0).clamp(0.0, 100.0) as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output from N1, an i9-9900 running Windows 11 Pro.
    const N1: &str = "\
TXWI|os|Microsoft Windows 11 Pro
TXWI|kernel|10.0.26200
TXWI|cpu|Intel(R) Core(TM) i9-9900 CPU @ 3.10GHz
TXWI|arch|64-bit
TXWM|66890464|30209384
TXWU|913073
TXWC|0|7488757968750|134319522527774461
TXWC|1|7992388437500|134319522527774461
TXWC|_Total|7818168193359|134319522527774461
TXWN|Realtek PCIe GbE Family Controller|28128128791|13080916921
TXWN|Realtek PCIe GbE Family Controller _3|0|0
TXWD|0 E:|883794432|3926739968
TXWD|1 C:|225587885568|493772840448
TXWD|_Total|226471680000|497699580416
";

    #[test]
    fn a_real_windows_frame_parses() {
        let f = parse_win_frame(N1);
        assert_eq!(f.facts.cpu_model, "Intel(R) Core(TM) i9-9900 CPU @ 3.10GHz");
        assert_eq!(f.facts.os, "Microsoft Windows 11 Pro");
        assert_eq!(
            f.mem_total_kb, 66_890_464,
            "63.8 GB, the machine's real memory"
        );
        assert_eq!(f.uptime_secs, Some(913_073));
        assert_eq!(f.cores.len(), 2);
        assert!(f.total.is_some());
    }

    #[test]
    fn the_build_number_is_labelled_rather_than_passed_off_as_a_kernel() {
        // Windows has no kernel version in the Linux sense. "10.0.26200" in a
        // field beside "Linux 6.12" invites a comparison that means nothing.
        assert_eq!(parse_win_frame(N1).facts.kernel, "Windows build 10.0.26200");
    }

    #[test]
    fn the_total_pseudo_core_is_kept_apart_from_the_real_ones() {
        // `_Total` is an average the host already computed. Left in the list
        // it would appear as a seventeenth core on a sixteen-core box.
        let f = parse_win_frame(N1);
        assert!(f.cores.iter().all(|c| c.0 != "_Total"));
        assert_eq!(
            f.cores.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["0", "1"]
        );
    }

    #[test]
    fn the_disk_total_row_is_dropped_so_bytes_are_not_counted_twice() {
        let f = parse_win_frame(N1);
        assert_eq!(f.disks.len(), 2, "_Total must not join the per-disk rows");
        assert!(f.disks.iter().all(|d| d.0 != "_Total"));
    }

    #[test]
    fn the_idle_counter_is_inverse_and_reading_it_directly_is_wrong() {
        // Measured on N1: taking the ratio directly reported 79% on a machine
        // sitting at about 11. This is the whole reason the module exists.
        //
        // 0.79 of the window spent idle => 21% busy.
        let prev = (0u64, 0u64);
        let now = (7_900_000u64, 10_000_000u64);
        let busy = busy_pct(prev, now).unwrap();
        assert!((busy - 21.0).abs() < 0.01, "got {busy}");
        assert!(busy < 50.0, "an idle machine must not read as busy");
    }

    #[test]
    fn a_fully_idle_core_reads_zero_and_a_pinned_one_reads_a_hundred() {
        assert_eq!(busy_pct((0, 0), (1_000, 1_000)).unwrap(), 0.0);
        assert_eq!(busy_pct((0, 0), (0, 1_000)).unwrap(), 100.0);
    }

    #[test]
    fn a_frame_covering_no_time_is_not_a_measurement_of_idleness() {
        // Two reads inside the same counter tick. Reporting 0% would claim
        // the machine was idle for an interval that did not happen.
        assert_eq!(busy_pct((5, 100), (5, 100)), None);
    }

    #[test]
    fn a_counter_that_went_backwards_yields_nothing_rather_than_a_wrapped_value() {
        // A reboot or a counter reset. Unsigned subtraction would produce
        // something astronomical and clamp to a confident 100%.
        assert_eq!(busy_pct((10, 100), (5, 200)), None);
        assert_eq!(busy_pct((10, 200), (20, 100)), None);
    }

    #[test]
    fn powershell_noise_does_not_cost_the_whole_frame() {
        // PowerShell writes warnings to the same stream. One noisy line must
        // not discard a sample that is otherwise sound.
        let noisy = format!("WARNING: something\n{N1}Get-CimInstance : blah\n");
        let f = parse_win_frame(&noisy);
        assert_eq!(f.mem_total_kb, 66_890_464);
        assert_eq!(f.cores.len(), 2);
    }

    #[test]
    fn the_script_avoids_the_localised_and_cached_apis() {
        let c = win_script(1);
        assert!(c.contains("Win32_PerfRawData_PerfOS_Processor"));
        assert!(!c.contains("PerfFormattedData"), "WMI's own cached delta");
        assert!(!c.contains("Get-Counter"), "counter paths are localised");
        assert!(!c.contains("LoadPercentage"), "coarse and cached");
    }

    #[test]
    fn the_encoding_matches_powershell_s_expectation() {
        // UTF-16LE then base64. Known vector: "hi" is h\0i\0 -> aABpAA==
        assert_eq!(encode_command("hi"), "aABpAA==");
        assert_eq!(encode_command("a"), "YQA=");
        assert_eq!(encode_command(""), "");
    }

    #[test]
    fn the_command_carries_no_character_a_shell_could_touch() {
        // The reason for encoding at all: inline, a POSIX shell ate `$os` on
        // the way through, and cmd.exe - the default shell for Windows
        // OpenSSH - mangles nested double quotes in its own way.
        let c = win_sampler_command(2);
        let payload = c.rsplit(' ').next().unwrap();
        assert!(!payload.is_empty());
        assert!(
            payload
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '+' || ch == '/' || ch == '='),
            "payload must be base64 only, got {payload}"
        );
        assert!(
            !c.contains('$'),
            "no shell variable can survive to be expanded"
        );
        assert!(
            !c.contains('\''),
            "no quoting for a shell to disagree about"
        );
    }
}
