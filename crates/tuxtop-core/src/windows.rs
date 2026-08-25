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
fn win_script(interval_ms: u32) -> String {
    let ms = interval_ms.max(1);
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
           Start-Sleep -Milliseconds {ms}\n\
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
pub fn win_sampler_command(interval_ms: u32) -> String {
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        encode_command(&win_script(interval_ms))
    )
}

/// The process-sampling script, run as its own persistent loop.
///
/// Ranking happens on the far side, as it does for Linux: 409 processes are
/// 16 KB per sample and the top twenty are about 650 bytes. The difference is
/// how the delta is obtained. Linux takes two snapshots inside one iteration;
/// here the loop is persistent, so it keeps the previous snapshot in a
/// variable and differentiates against that — one CIM query per cycle rather
/// than two. Measured on N1: ~574 ms per query against 409 processes, so the
/// saving is real remote CPU on somebody's workstation.
///
/// The consequence, stated because it differs from the Linux path: the window
/// is the sampling interval rather than a fixed second, so a Windows process
/// figure is averaged over five seconds where a Linux one is averaged over
/// one. Both are percentages of the whole box; the Windows one is smoother.
pub fn win_process_command(top_n: usize, interval_ms: u32) -> String {
    let ms = interval_ms.max(1);
    let script = format!(
        "$ErrorActionPreference='SilentlyContinue'\n\
         $ncpu=(Get-CimInstance Win32_ComputerSystem).NumberOfLogicalProcessors\n\
         $prev=@{{}}\n\
         $prevTs=0\n\
         while($true){{\n\
           $q='SELECT IDProcess,Name,PercentProcessorTime,WorkingSetPrivate,Timestamp_Sys100NS FROM Win32_PerfRawData_PerfProc_Process'\n\
           $rows=Get-CimInstance -Query $q | Where-Object {{ $_.Name -ne '_Total' -and $_.Name -ne 'Idle' }}\n\
           $ts=0; if($rows){{ $ts=$rows[0].Timestamp_Sys100NS }}\n\
           $cur=@{{}}\n\
           foreach($r in $rows){{ $cur[[string]$r.IDProcess]=$r.PercentProcessorTime }}\n\
           if($prevTs -gt 0 -and $ts -gt $prevTs){{\n\
             $win=$ts-$prevTs\n\
             $svc=@{{}}\n\
             foreach($s in Get-CimInstance -Query 'SELECT Name,ProcessId FROM Win32_Service WHERE ProcessId <> 0'){{\n\
               $k=[string]$s.ProcessId\n\
               if($svc.ContainsKey($k)){{ $svc[$k]=$svc[$k]+','+$s.Name }} else {{ $svc[$k]=$s.Name }}\n\
             }}\n\
             'TXWPT|'+$ncpu+'|'+$win\n\
             $rows | ForEach-Object {{\n\
               $k=[string]$_.IDProcess\n\
               $d=0; if($prev.ContainsKey($k)){{ $d=$cur[$k]-$prev[$k] }}\n\
               [pscustomobject]@{{ pid=$k; d=$d; ws=$_.WorkingSetPrivate; n=$_.Name }}\n\
             }} | Sort-Object -Property d,ws -Descending | Select-Object -First {top_n} | ForEach-Object {{\n\
               $sv=''; if($svc.ContainsKey($_.pid)){{ $sv=$svc[$_.pid] }}\n\
               'TXWP|'+$_.pid+'|'+$_.d+'|'+$_.ws+'|'+$_.n+'|'+$sv\n\
             }}\n\
             '{delim}'\n\
           }}\n\
           $prev=$cur; $prevTs=$ts\n\
           Start-Sleep -Milliseconds {ms}\n\
         }}\n",
        delim = crate::sampler::FRAME_DELIMITER,
    );
    format!(
        "powershell -NoProfile -NonInteractive -EncodedCommand {}",
        encode_command(&script)
    )
}

/// Parse a Windows process frame into the same `ProcInfo` the Linux path
/// produces, so the fleet list, filter, sort and owner column need no changes.
pub fn parse_win_processes(host: &str, text: &str) -> Vec<crate::procs::ProcInfo> {
    use crate::procs::{OwnerKind, ProcInfo};

    let mut ncpu: u64 = 0;
    let mut window: u64 = 0;
    for line in text.lines() {
        if let Some(rest) = line.trim_end_matches('\r').strip_prefix("TXWPT|") {
            let f: Vec<&str> = rest.split('|').collect();
            if f.len() >= 2 {
                ncpu = f[0].trim().parse().unwrap_or(0);
                window = f[1].trim().parse().unwrap_or(0);
            }
        }
    }
    // Without a window and a core count every percentage would be a guess, so
    // report nothing rather than something plausible — the same rule the
    // Linux parser follows when the jiffy denominator is missing.
    if ncpu == 0 || window == 0 {
        return Vec::new();
    }

    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.trim_end_matches('\r').strip_prefix("TXWP|") else {
            continue;
        };
        let f: Vec<&str> = rest.split('|').collect();
        if f.len() < 5 {
            continue;
        }
        let (Ok(pid), Ok(delta)) = (f[0].trim().parse::<u32>(), f[1].trim().parse::<i64>()) else {
            continue;
        };
        // A process that started during the window has no previous counter and
        // is reported at zero rather than with its whole lifetime's CPU.
        let delta = delta.max(0) as u64;
        let ws: u64 = f[2].trim().parse().unwrap_or(0);
        let service = f[4..].join("|").trim().to_string();

        out.push(ProcInfo {
            host: host.to_string(),
            pid,
            cpu_pct: ((delta as f64 / (window as f64 * ncpu as f64)) * 100.0).clamp(0.0, 100.0)
                as f32,
            rss_kb: ws / 1024,
            // Win32_Process would give the owner, at a query per process.
            // Empty is honest; a guess would not be.
            user: String::new(),
            comm: f[3].trim().to_string(),
            cmd: String::new(),
            owner_kind: if service.is_empty() {
                OwnerKind::None
            } else {
                OwnerKind::Service
            },
            owner: service,
            // Windows has no kernel threads in the Linux sense.
            kernel: false,
        });
    }
    out
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

    /// Real shapes from N1: a service-hosted process, one hosting several
    /// services, and one belonging to none.
    const PROCS: &str = "\
TXWPT|16|10000000
TXWP|2452|8000000|52428800|svchost|Appinfo,AppMgmt
TXWP|4092|1600000|10485760|AdobeARMservice|AdobeARMservice
TXWP|9100|0|4194304|explorer|
";

    #[test]
    fn a_process_percentage_is_of_the_whole_box() {
        // 0.8s of CPU over a 1s window on 16 cores is 5%, matching the Linux
        // convention. Forgetting the core count would report 80%.
        let p = parse_win_processes("n1", PROCS);
        let sv = p.iter().find(|x| x.pid == 2452).unwrap();
        assert!((sv.cpu_pct - 5.0).abs() < 0.01, "got {}", sv.cpu_pct);
    }

    #[test]
    fn a_process_is_attributed_to_its_service() {
        let p = parse_win_processes("n1", PROCS);
        assert_eq!(
            p.iter().find(|x| x.pid == 4092).unwrap().owner,
            "AdobeARMservice"
        );
    }

    #[test]
    fn one_svchost_hosting_several_services_names_them_all() {
        // Naming only the first would attribute a spike to whichever service
        // WMI happened to list first, which is a coin toss dressed as a fact.
        let p = parse_win_processes("n1", PROCS);
        assert_eq!(
            p.iter().find(|x| x.pid == 2452).unwrap().owner,
            "Appinfo,AppMgmt"
        );
    }

    #[test]
    fn a_process_belonging_to_no_service_has_no_owner() {
        let p = parse_win_processes("n1", PROCS);
        let e = p.iter().find(|x| x.pid == 9100).unwrap();
        assert_eq!(e.owner, "");
        assert_eq!(e.owner_kind, crate::procs::OwnerKind::None);
    }

    #[test]
    fn a_frame_with_no_window_reports_nothing_rather_than_zeroes() {
        // Without a window and a core count every percentage is a guess. The
        // Linux parser refuses the same way when its jiffy denominator is
        // missing.
        assert!(parse_win_processes("n1", "TXWP|1|100|4096|x|\n").is_empty());
        assert!(parse_win_processes("n1", "TXWPT|16|0\nTXWP|1|100|4096|x|\n").is_empty());
    }

    #[test]
    fn a_process_that_started_during_the_window_is_not_charged_its_lifetime() {
        // It has no previous counter, so the remote side emits a negative or
        // absent delta. Charging it everything since boot would put every new
        // process at the top of the list.
        let t = "TXWPT|16|10000000\nTXWP|77|-5000000|4096|new|\n";
        assert_eq!(parse_win_processes("n1", t)[0].cpu_pct, 0.0);
    }

    #[test]
    fn the_process_script_keeps_its_own_previous_snapshot() {
        // The point of the design: one CIM query per cycle instead of two,
        // because the loop is persistent and can remember.
        let c = win_process_command(20, 5);
        assert!(c.starts_with("powershell -NoProfile"));
        let script = String::from_utf16(
            &base64_decode(c.rsplit(' ').next().unwrap())
                .chunks(2)
                .map(|p| u16::from_le_bytes([p[0], *p.get(1).unwrap_or(&0)]))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        assert!(script.contains("$prev"), "no previous snapshot kept");
        assert!(script.contains("Sort-Object"), "not ranked on the far side");
        assert!(
            !script.contains("Get-Counter"),
            "counter paths are localised"
        );
    }

    /// Minimal decoder, for the test above only.
    fn base64_decode(s: &str) -> Vec<u8> {
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let idx = |c: u8| T.iter().position(|&t| t == c).unwrap_or(0) as u32;
        let b: Vec<u8> = s.bytes().filter(|&c| c != b'=').collect();
        let mut out = Vec::new();
        for c in b.chunks(4) {
            let mut n = 0u32;
            for (i, &ch) in c.iter().enumerate() {
                n |= idx(ch) << (18 - 6 * i);
            }
            out.push((n >> 16) as u8);
            if c.len() > 2 {
                out.push((n >> 8) as u8);
            }
            if c.len() > 3 {
                out.push(n as u8);
            }
        }
        out
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
