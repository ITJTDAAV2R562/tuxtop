//! The remote sampler: the shell loop Tuxtop runs on each host, and the
//! parser for the framed output it produces.
//!
//! Nothing is installed on the target. We open one SSH channel, start a loop
//! that cats the relevant `/proc` files once a second, and parse the frames as
//! they arrive. See `docs/ARCHITECTURE.md` for why this exists alongside
//! Beszel rather than replacing it.

use crate::proc::{self, MemInfo, StatSnapshot};

/// Marks the end of one sample in the remote stream.
///
/// Chosen because it cannot appear in any `/proc` file we read.
pub const FRAME_DELIMITER: &str = "--=TUXTOP=--";

/// The command Tuxtop executes on the remote host.
///
/// Deliberately POSIX `sh`, not bash: some appliances and minimal containers
/// have no bash, and this needs to run everywhere sshd does.
///
/// `interval_ms` bounds how fast the host is polled. One second is the
/// Task-Manager feel; anything slower and the core grid stops being live.
/// Sub-second is available per host, for a box under investigation.
///
/// **Only the `/proc` reads run at that rate.** Everything else in the loop
/// keeps the wall-clock cadence it had at 1 Hz, because the loop divisors are
/// derived from the interval rather than fixed in frames. That matters most
/// for `nvidia-smi`, which is a process spawn costing hundreds of
/// milliseconds: at 4 Hz a frame-counted schedule would run it four times a
/// second on every GPU host, which is real load on a machine we are only
/// supposed to be watching. `df` is the same argument in bytes rather than
/// CPU. The cheap kernel counters are what a spike lives in; the expensive
/// extras change on a scale of seconds and gain nothing from being asked
/// faster.
pub fn sampler_command(interval_ms: u32) -> String {
    let every = |target_ms: u32| (target_ms / interval_ms.max(1)).max(1);
    let df_every = every(DF_EVERY_MS);
    let slow_every = every(SLOW_EVERY_MS);
    let nap = sleep_arg(interval_ms);
    format!(
        "{FACTS_SNIPPET} \
         i=0; \
         while :; do \
           cat /proc/stat /proc/meminfo /proc/diskstats /proc/net/dev /proc/loadavg 2>/dev/null; \
           echo \"TXU|$(cut -d' ' -f1 /proc/uptime 2>/dev/null)\"; \
           if [ $((i % {slow_every})) -eq 0 ]; then \
             {TEMP_SNIPPET} \
             {GPU_SNIPPET} \
           fi; \
           if [ $((i % {df_every})) -eq 0 ]; then \
             df -P -k 2>/dev/null | tail -n +2 | sed 's/^/TXF|/'; \
           fi; \
           i=$((i+1)); \
           echo '{FRAME_DELIMITER}'; \
           sleep {nap}; \
         done"
    )
}

/// Render a millisecond interval as an argument `sleep` will accept.
///
/// Whole seconds stay integers so the common case reads as it always has, and
/// nothing depends on a shell accepting a decimal point it does not need.
/// Fractions are printed with the smallest number of decimals that is exact,
/// because `sleep 0.25` is POSIX-undefined but universally supported: every
/// host in this fleet runs GNU coreutils under dash and takes it, and that was
/// checked before the feature was built rather than after.
pub fn sleep_arg(interval_ms: u32) -> String {
    let ms = interval_ms.max(1);
    if ms.is_multiple_of(1000) {
        return (ms / 1000).to_string();
    }
    let s = format!("{:.3}", ms as f64 / 1000.0);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A sample interval as a person would say it: "4 Hz", "1 s", "30 s".
///
/// Sub-second rates read as a frequency because that is how they are chosen -
/// nobody asks for 250 ms, they ask to watch a box four times a second - and
/// slower ones read as a period, because "0.03 Hz" is nobody's idea of a
/// minute.
pub fn rate_label(interval_ms: u32) -> String {
    let ms = interval_ms.max(1);
    if ms < 1000 {
        let hz = 1000.0 / ms as f64;
        let s = format!("{hz:.1}");
        return format!("{} Hz", s.trim_end_matches('0').trim_end_matches('.'));
    }
    format!("{} s", sleep_arg(ms))
}

/// How often disk capacity is re-read, in milliseconds of wall clock.
pub const DF_EVERY_MS: u32 = 30_000;
/// How often temperatures and GPU stats are re-read, in wall-clock ms.
pub const SLOW_EVERY_MS: u32 = 1_000;

/// Identity, read once before the loop starts.
///
/// None of it changes between frames, so re-reading it 86,400 times a day
/// would be pure waste. It arrives in the first frame and the UI keeps it.
/// `systemd-detect-virt` exits **non-zero when it finds nothing** and still
/// prints "none", so `cmd || echo unknown` appends the fallback to a perfectly
/// good answer - every bare-metal host reported "none unknown". The output is
/// captured and only substituted when it is empty.
const FACTS_SNIPPET: &str = "\
  echo \"TXI|kernel|$(uname -sr 2>/dev/null)\"; \
  echo \"TXI|arch|$(uname -m 2>/dev/null)\"; \
  echo \"TXI|os|$( . /etc/os-release 2>/dev/null; echo \"$PRETTY_NAME\" )\"; \
  echo \"TXI|cpu|$(grep -m1 -E '^(model name|Model)' /proc/cpuinfo 2>/dev/null | cut -d: -f2- | sed 's/^ *//')\"; \
  v=$(systemd-detect-virt 2>/dev/null); echo \"TXI|virt|${v:-unknown}\"; \
  k=$(systemd-detect-virt --container 2>/dev/null); \
  echo \"TXI|virtkind|$([ -n \"$k\" ] && [ \"$k\" != none ] && echo container || echo vm)\";";

/// Emits one `TXT|driver|label|millidegrees` line per hwmon temperature.
///
/// Pipe-delimited because labels contain spaces ("Package id 0", "Sensor 1"),
/// so whitespace splitting would lose the value. Unreadable sensors are
/// skipped rather than failing the frame - a wifi chip that refuses a read
/// must not cost us the CPU temperature.
const TEMP_SNIPPET: &str = "for d in /sys/class/hwmon/hwmon*/; do \
  n=$(cat \"$d/name\" 2>/dev/null); \
  for t in \"$d\"temp*_input; do \
    [ -r \"$t\" ] || continue; \
    l=$(cat \"${t%_input}_label\" 2>/dev/null); \
    echo \"TXT|$n|$l|$(cat \"$t\" 2>/dev/null)\"; \
  done; \
done;";

/// Emits `TXG|index, name, util%, used MiB, total MiB, watts` per GPU.
///
/// Guarded by `command -v`, so a host without the driver contributes nothing
/// and costs no error - the overwhelmingly common case. `nounits` keeps the
/// values bare so parsing does not have to strip suffixes.
const GPU_SNIPPET: &str = "command -v nvidia-smi >/dev/null 2>&1 && \
  nvidia-smi --query-gpu=index,name,utilization.gpu,memory.used,memory.total,power.draw \
  --format=csv,noheader,nounits 2>/dev/null | sed 's/^/TXG|/';";

/// One parsed frame, before deltas are applied.
///
/// Counters here are cumulative-since-boot, exactly as the kernel reports
/// them. Turning them into rates needs two frames — see [`RateTracker`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Frame {
    pub stat: StatSnapshot,
    pub mem: MemInfo,
    pub load: [f32; 3],
    pub net_rx_bytes: u64,
    pub net_tx_bytes: u64,
    pub disk_read_bytes: u64,
    pub disk_write_bytes: u64,
    /// CPU package temperature in degrees C, when a sensor identifies one.
    pub cpu_temp_c: Option<f32>,
    /// Every hwmon reading, not only the CPU's.
    pub temps: Vec<TempSensor>,
    /// First GPU reported by nvidia-smi, if any.
    pub gpu: Option<crate::model::GpuSample>,
    /// Seconds since boot.
    pub uptime_secs: Option<u64>,
    /// Identity, present only in the first frame of a connection.
    pub facts: Option<crate::facts::HostFacts>,
    /// Filesystems, present only in frames where `df` ran.
    pub filesystems: Vec<crate::facts::FsEntry>,
}

/// Split a stream buffer into complete frames, returning the unconsumed tail.
///
/// The caller keeps the tail and prepends it to the next read. A partial frame
/// is never parsed — that is the whole point of the delimiter, since a 1 MB
/// read can land mid-`/proc/stat` and half a stat file parses as a plausible
/// but wrong snapshot.
pub fn split_frames(buf: &str) -> (Vec<&str>, &str) {
    let mut frames = Vec::new();
    let mut rest = buf;

    while let Some(idx) = rest.find(FRAME_DELIMITER) {
        frames.push(&rest[..idx]);
        rest = &rest[idx + FRAME_DELIMITER.len()..];
    }

    (frames, rest)
}

/// Parse one complete frame.
pub fn parse_frame(text: &str) -> Frame {
    let mut frame = Frame {
        stat: proc::parse_stat(text),
        mem: proc::parse_meminfo(text),
        ..Default::default()
    };

    let (rx, tx) = parse_net_dev(text);
    frame.net_rx_bytes = rx;
    frame.net_tx_bytes = tx;

    let (r, w) = parse_diskstats(text);
    frame.disk_read_bytes = r;
    frame.disk_write_bytes = w;

    frame.load = parse_loadavg(text);
    frame.cpu_temp_c = parse_cpu_temp(text);
    frame.temps = parse_temps(text);
    frame.gpu = parse_gpu(text);
    frame.uptime_secs = crate::facts::parse_uptime(text);

    let facts = crate::facts::parse_facts(text);
    frame.facts = (!facts.is_empty()).then_some(facts);
    frame.filesystems = crate::facts::parse_filesystems(text);

    frame
}

/// Parse the first GPU from the `TXG|` lines.
///
/// Only the first is taken for now: the wire type carries one GPU, and a
/// multi-GPU host would need the UI to decide what "the" GPU means before
/// collecting more would be useful.
///
/// A field that will not parse discards the whole reading rather than
/// defaulting to zero - a GPU reported at 0% because its utilisation field was
/// malformed is indistinguishable from an idle one.
pub fn parse_gpu(text: &str) -> Option<crate::model::GpuSample> {
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXG|") else {
            continue;
        };
        let f: Vec<&str> = rest.split(',').map(str::trim).collect();
        // index, name, util, used, total, power
        if f.len() < 6 {
            continue;
        }
        let (Ok(util), Ok(used), Ok(total)) = (
            f[2].parse::<f32>(),
            f[3].parse::<u64>(),
            f[4].parse::<u64>(),
        ) else {
            continue;
        };
        // Power draw is unsupported on some cards and reports [N/A].
        let power = f[5].parse::<f32>().unwrap_or(0.0);

        return Some(crate::model::GpuSample {
            name: f[1].to_string(),
            util_pct: util.clamp(0.0, 100.0),
            mem_used_mb: used,
            mem_total_mb: total,
            power_w: power,
        });
    }
    None
}

/// What a sensor is attached to.
///
/// Kept because naming the component is the whole point. "72C" is alarming
/// for a CPU and unremarkable for an NVMe under load, so a temperature
/// without its subject is a number the reader cannot act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SensorKind {
    Cpu,
    Drive,
    Wireless,
    Board,
    Other,
}

impl SensorKind {
    fn of(driver: &str) -> Self {
        match driver {
            "coretemp" | "k10temp" | "zenpower" | "cpu_thermal" | "soc_thermal" => Self::Cpu,
            "nvme" | "drivetemp" => Self::Drive,
            d if d.starts_with("iwlwifi") || d.starts_with("mt79") => Self::Wireless,
            "acpitz" | "gigabyte_wmi" | "nct6775" | "it87" => Self::Board,
            _ => Self::Other,
        }
    }
}

/// One hwmon reading.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TempSensor {
    /// hwmon driver name, e.g. `nvme`, `k10temp`.
    pub driver: String,
    /// hwmon label, e.g. `Composite`, `Tctl`. Often empty — board sensors
    /// frequently expose several inputs with no label at all.
    pub label: String,
    pub celsius: f32,
    pub kind: SensorKind,
}

impl TempSensor {
    /// A name a person can act on.
    ///
    /// Several sensors routinely share a driver with no label — dove's board
    /// exposes four `gigabyte_wmi` inputs — so unlabelled ones are numbered
    /// within their driver. Without that they collapse into one row and three
    /// readings vanish.
    pub fn name(&self, index_within_driver: usize) -> String {
        if self.label.is_empty() {
            if index_within_driver > 0 {
                format!("{} {}", self.driver, index_within_driver + 1)
            } else {
                self.driver.clone()
            }
        } else {
            format!("{} {}", self.driver, self.label)
        }
    }
}

/// Every plausible hwmon reading, in the order the host reported them.
///
/// The same plausibility floor as `parse_cpu_temp`: a failed sensor commonly
/// reports 0 or something enormous, and publishing that is worse than
/// publishing nothing.
pub fn parse_temps(text: &str) -> Vec<TempSensor> {
    let mut out = Vec::new();

    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXT|") else {
            continue;
        };
        let mut f = rest.split('|');
        let (Some(driver), Some(label), Some(value)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let Ok(milli) = value.trim().parse::<f32>() else {
            continue;
        };
        let celsius = milli / 1000.0;
        if !(5.0..=150.0).contains(&celsius) {
            continue;
        }
        out.push(TempSensor {
            kind: SensorKind::of(driver),
            driver: driver.to_string(),
            label: label.trim().to_string(),
            celsius,
        });
    }
    out
}

/// The hottest sensor, whatever it is attached to.
///
/// Deliberately separate from [`parse_cpu_temp`], which must never return this
/// — an NVMe under load routinely beats the CPU, and on dove it does so by
/// 40 degrees. Reporting that as "CPU temperature" would name the wrong
/// component with total confidence. Reported as *the hottest sensor*, with its
/// name attached, it is exactly the fleet-wide signal worth seeing.
pub fn hottest(temps: &[TempSensor]) -> Option<(String, f32)> {
    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    let mut best: Option<(String, f32)> = None;

    for t in temps {
        let i = seen.entry(t.driver.as_str()).or_insert(0);
        let name = t.name(*i);
        *i += 1;
        if best.as_ref().is_none_or(|(_, c)| t.celsius > *c) {
            best = Some((name, t.celsius));
        }
    }
    best
}

/// Pick the CPU package temperature out of the hwmon lines.
///
/// Ranking matters more than it looks. A box reports temperatures for NVMe
/// drives, chipsets, wifi and the CPU, and an NVMe under load is routinely
/// hotter than the CPU - so "hottest sensor" would report the wrong component
/// with total confidence. Only drivers known to be CPU sensors are considered.
///
/// Within AMD's k10temp, `Tdie` is the real junction temperature and `Tctl` is
/// the control value, which on some parts carries a fixed offset above Tdie.
/// Tdie wins when both are present.
pub fn parse_cpu_temp(text: &str) -> Option<f32> {
    let mut best: Option<(u8, f32)> = None;

    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXT|") else {
            continue;
        };
        let mut f = rest.split('|');
        let (Some(driver), Some(label), Some(value)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let Ok(milli) = value.trim().parse::<f32>() else {
            continue;
        };
        let celsius = milli / 1000.0;

        // Discard implausible readings. A failed or uninitialised sensor
        // commonly reports exactly 0 or a huge value, and a powered CPU below
        // 5C is essentially unheard of outside a lab - so a low floor here
        // costs nothing real and avoids reporting a confidently wrong
        // temperature, which is worse than reporting none.
        if !(5.0..=150.0).contains(&celsius) {
            continue;
        }

        // Lower rank number wins.
        let rank = match (driver, label) {
            ("coretemp", l) if l.starts_with("Package") => 0,
            ("k10temp", "Tdie") | ("zenpower", "Tdie") => 0,
            ("k10temp", "Tctl") | ("zenpower", "Tctl") => 1,
            ("coretemp", l) if l.starts_with("Core") => 2,
            ("k10temp", l) if l.starts_with("Tccd") => 2,
            ("cpu_thermal", _) | ("soc_thermal", _) => 3,
            _ => continue, // nvme, acpitz, wifi, drivetemp: not the CPU
        };

        // At equal rank keep the hottest, which is the meaningful core.
        match best {
            Some((r, t)) if r < rank || (r == rank && t >= celsius) => {}
            _ => best = Some((rank, celsius)),
        }
    }

    best.map(|(_, t)| t)
}

/// Sum receive and transmit bytes across real interfaces in `/proc/net/dev`.
///
/// Loopback is excluded — counting `lo` makes local IPC look like network
/// traffic and can dwarf the real link. Virtual bridges and veth pairs are
/// also skipped, since they double-count container traffic that already
/// crosses a physical NIC.
fn parse_net_dev(text: &str) -> (u64, u64) {
    let (mut rx, mut tx) = (0u64, 0u64);

    for line in text.lines() {
        let Some((iface, rest)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();

        if iface == "lo"
            || iface.starts_with("veth")
            || iface.starts_with("docker")
            || iface.starts_with("br-")
            || iface.starts_with("virbr")
        {
            continue;
        }

        let f: Vec<&str> = rest.split_ascii_whitespace().collect();
        // Columns: rx_bytes ... (8 fields) ... tx_bytes at index 8.
        if f.len() < 9 {
            continue;
        }
        rx = rx.saturating_add(f[0].parse().unwrap_or(0));
        tx = tx.saturating_add(f[8].parse().unwrap_or(0));
    }

    (rx, tx)
}

/// Sum bytes read and written across physical block devices in `/proc/diskstats`.
///
/// Sectors are converted at the kernel's fixed 512 bytes — that constant is
/// part of the diskstats ABI and is *not* the device's physical sector size,
/// so 4Kn drives still report in 512-byte units here.
///
/// Partitions and virtual devices are skipped so their I/O is not counted
/// twice on top of the parent disk.
fn parse_diskstats(text: &str) -> (u64, u64) {
    const SECTOR_BYTES: u64 = 512;
    let (mut read, mut write) = (0u64, 0u64);

    for line in text.lines() {
        let f: Vec<&str> = line.split_ascii_whitespace().collect();
        // major minor name reads merged sectors_read ms writes merged sectors_written ...
        if f.len() < 10 {
            continue;
        }
        let name = f[2];

        if !is_whole_disk(name) {
            continue;
        }

        read = read.saturating_add(f[5].parse::<u64>().unwrap_or(0) * SECTOR_BYTES);
        write = write.saturating_add(f[9].parse::<u64>().unwrap_or(0) * SECTOR_BYTES);
    }

    (read, write)
}

/// Whole physical disks only: `sda`, `nvme0n1`, `vda`, `mmcblk0`.
///
/// Rejects partitions (`sda1`, `nvme0n1p2`) and virtual devices (`loop0`,
/// `dm-0`, `ram0`, `zram0`, `md0`).
fn is_whole_disk(name: &str) -> bool {
    if name.starts_with("loop")
        || name.starts_with("ram")
        || name.starts_with("dm-")
        || name.starts_with("md")
        || name.starts_with("zram")
        || name.starts_with("sr")
    {
        return false;
    }

    if let Some(rest) = name.strip_prefix("nvme") {
        // nvme0n1 is a disk; nvme0n1p1 is a partition.
        return !rest.contains('p');
    }

    if name.starts_with("mmcblk") {
        return !name.contains('p');
    }

    // sda / vda / hda style: a trailing digit means a partition.
    !name.chars().last().is_some_and(|c| c.is_ascii_digit())
}

/// Read the three load averages from a `/proc/loadavg` line.
fn parse_loadavg(text: &str) -> [f32; 3] {
    for line in text.lines() {
        let f: Vec<&str> = line.split_ascii_whitespace().collect();
        // Shape: "0.46 1.10 0.65 1/1234 56789" — the 4th field has a slash,
        // which is what distinguishes it from any other line we cat.
        if f.len() >= 4 && f[3].contains('/') {
            if let (Ok(a), Ok(b), Ok(c)) = (
                f[0].parse::<f32>(),
                f[1].parse::<f32>(),
                f[2].parse::<f32>(),
            ) {
                return [a, b, c];
            }
        }
    }
    [0.0; 3]
}

/// Turns cumulative byte counters into per-second rates.
///
/// Holds the previous frame; the first call after construction yields zero
/// rates because a rate needs two points. That zero is honest, not a
/// placeholder — the UI shows a flat first second and then real numbers.
#[derive(Debug, Default)]
pub struct RateTracker {
    prev: Option<Frame>,
}

/// Per-second rates derived from two consecutive frames.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rates {
    pub cpu: f32,
    pub net_rx_bps: u64,
    pub net_tx_bps: u64,
    pub disk_read_bps: u64,
    pub disk_write_bps: u64,
}

impl RateTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// The previous frame's aggregate CPU row, for computing a breakdown.
    ///
    /// `None` before the second frame, since a breakdown is a delta and needs
    /// two points exactly as a rate does.
    pub fn prev_aggregate(&self) -> Option<crate::proc::CpuTimes> {
        self.prev.as_ref().map(|f| f.stat.aggregate)
    }

    /// Feed the next frame, getting rates and per-core percentages back.
    ///
    /// `elapsed_secs` is how long actually passed, not the requested interval —
    /// a stalled network read makes the two diverge, and dividing by the
    /// requested interval would then overstate the rate.
    pub fn push(&mut self, frame: Frame, elapsed_secs: f64) -> (Rates, Vec<f32>) {
        let Some(prev) = self.prev.replace(frame.clone()) else {
            return (Rates::default(), vec![0.0; frame.stat.cores.len()]);
        };

        let secs = if elapsed_secs > 0.0 {
            elapsed_secs
        } else {
            1.0
        };
        let per_sec =
            |cur: u64, old: u64| -> u64 { ((cur.saturating_sub(old)) as f64 / secs) as u64 };

        let rates = Rates {
            cpu: proc::busy_pct(&prev.stat.aggregate, &frame.stat.aggregate),
            net_rx_bps: per_sec(frame.net_rx_bytes, prev.net_rx_bytes),
            net_tx_bps: per_sec(frame.net_tx_bytes, prev.net_tx_bytes),
            disk_read_bps: per_sec(frame.disk_read_bytes, prev.disk_read_bytes),
            disk_write_bps: per_sec(frame.disk_write_bytes, prev.disk_write_bytes),
        };

        (rates, proc::core_pcts(&prev.stat, &frame.stat))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NET: &str = "\
Inter-|   Receive                    |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets
    lo: 999999 100 0 0 0 0 0 0 999999 100 0 0 0 0 0 0
enp10s0: 1000 10 0 0 0 0 0 0 2000 20 0 0 0 0 0 0
tailscale0: 500 5 0 0 0 0 0 0 700 7 0 0 0 0 0 0
  veth9a: 4444 4 0 0 0 0 0 0 5555 5 0 0 0 0 0 0
";

    const DISKS: &str = "\
   8       0 sda 100 0 200 50 300 0 400 60 0 0 0
   8       1 sda1 90 0 180 40 280 0 380 50 0 0 0
 259       0 nvme0n1 10 0 20 5 30 0 40 6 0 0 0
 259       1 nvme0n1p1 9 0 18 4 28 0 38 5 0 0 0
   7       0 loop0 1 0 2 1 3 0 4 1 0 0 0
";

    #[test]
    fn sampler_command_is_posix_sh() {
        let cmd = sampler_command(1);
        assert!(cmd.contains("while :; do"));
        assert!(cmd.contains(FRAME_DELIMITER));
        // `[[`, arrays and `local` are bashisms that would break on dash/ash.
        assert!(!cmd.contains("[["), "must not require bash");
    }

    #[test]
    fn split_frames_returns_only_complete_frames() {
        let buf = format!("one{FRAME_DELIMITER}two{FRAME_DELIMITER}partial");
        let (frames, rest) = split_frames(&buf);
        assert_eq!(frames, vec!["one", "two"]);
        assert_eq!(rest, "partial", "incomplete frame must stay buffered");
    }

    #[test]
    fn split_frames_handles_no_delimiter_yet() {
        let (frames, rest) = split_frames("half a stat file");
        assert!(frames.is_empty());
        assert_eq!(rest, "half a stat file");
    }

    #[test]
    fn loopback_and_virtual_interfaces_are_excluded() {
        let (rx, tx) = parse_net_dev(NET);
        // enp10s0 + tailscale0 only: lo and veth9a must not contribute.
        assert_eq!(rx, 1000 + 500);
        assert_eq!(tx, 2000 + 700);
    }

    #[test]
    fn partitions_are_not_double_counted() {
        let (read, write) = parse_diskstats(DISKS);
        // sda (200 sectors) + nvme0n1 (20), excluding sda1/nvme0n1p1/loop0.
        assert_eq!(read, (200 + 20) * 512);
        assert_eq!(write, (400 + 40) * 512);
    }

    #[test]
    fn whole_disk_detection() {
        for d in ["sda", "vda", "nvme0n1", "mmcblk0", "hdb"] {
            assert!(is_whole_disk(d), "{d} should count as a disk");
        }
        for p in [
            "sda1",
            "nvme0n1p1",
            "mmcblk0p2",
            "loop0",
            "dm-0",
            "md0",
            "zram0",
            "sr0",
        ] {
            assert!(!is_whole_disk(p), "{p} should be excluded");
        }
    }

    #[test]
    fn loadavg_is_found_among_other_proc_output() {
        let text = format!("cpu 1 2 3 4\n0.46 1.10 0.65 1/1234 56789\n{NET}");
        assert_eq!(parse_loadavg(&text), [0.46, 1.10, 0.65]);
    }

    #[test]
    fn loadavg_absent_yields_zeros_not_garbage() {
        assert_eq!(parse_loadavg("cpu 1 2 3 4\n"), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn first_frame_yields_zero_rates() {
        let mut t = RateTracker::new();
        let frame = parse_frame(&format!("cpu 100 0 50 900\ncpu0 100 0 50 900\n{NET}"));
        let (rates, cores) = t.push(frame, 1.0);
        assert_eq!(rates, Rates::default(), "a rate needs two samples");
        assert_eq!(cores, vec![0.0], "core count still reported on frame one");
    }

    #[test]
    fn rates_divide_by_actual_elapsed_time() {
        let mut t = RateTracker::new();
        let a = parse_frame("cpu 100 0 50 900\ncpu0 100 0 50 900\nenp0: 1000 1 0 0 0 0 0 0 0 0\n");
        let b =
            parse_frame("cpu 200 0 100 1700\ncpu0 200 0 100 1700\nenp0: 3000 1 0 0 0 0 0 0 0 0\n");
        t.push(a, 1.0);
        // 2000 bytes over 2 seconds must read as 1000 B/s, not 2000.
        let (rates, _) = t.push(b, 2.0);
        assert_eq!(rates.net_rx_bps, 1000);
    }

    #[test]
    fn zero_elapsed_does_not_divide_by_zero() {
        let mut t = RateTracker::new();
        let a = parse_frame("cpu 100 0 50 900\nenp0: 1000 1 0 0 0 0 0 0 0 0\n");
        let b = parse_frame("cpu 200 0 100 1700\nenp0: 3000 1 0 0 0 0 0 0 0 0\n");
        t.push(a, 0.0);
        let (rates, _) = t.push(b, 0.0);
        assert!(rates.net_rx_bps < u64::MAX, "must not overflow or panic");
    }

    #[test]
    fn full_frame_round_trip() {
        let text = format!(
            "cpu  1000 20 300 90000 100 5 15 0\ncpu0 1000 20 300 90000 100 5 15 0\n\
             MemTotal: 32791234 kB\nMemAvailable: 25600000 kB\n\
             0.46 1.10 0.65 1/1234 56789\n{NET}{DISKS}"
        );
        let f = parse_frame(&text);
        assert_eq!(f.stat.cores.len(), 1);
        assert_eq!(f.mem.total_kb, 32_791_234);
        assert_eq!(f.load, [0.46, 1.10, 0.65]);
        assert_eq!(f.net_rx_bytes, 1500);
        assert_eq!(f.disk_read_bytes, 220 * 512);
    }
}

#[cfg(test)]
mod temp_tests {
    use super::*;

    /// Real hwmon output from dove (AMD, k10temp) with an NVMe running hotter
    /// than the CPU - the case that makes "hottest sensor wins" wrong.
    const CROW: &str = "\
TXT|acpitz||16800
TXT|acpitz||16800
TXT|nvme|Composite|49850
TXT|nvme|Sensor 1|71850
TXT|nvme|Sensor 2|40850
TXT|k10temp|Tctl|31000
TXT|k10temp|Tccd1|34250
TXT|k10temp|Tccd2|31750
";

    #[test]
    fn a_hot_nvme_is_not_reported_as_the_cpu() {
        // Sensor 1 is 71.85C, far above the CPU. Picking the hottest sensor
        // would confidently report the wrong component.
        let t = parse_cpu_temp(CROW).expect("dove reports a CPU temperature");
        assert!(t < 40.0, "got {t}, which looks like the NVMe");
        assert!((t - 31.0).abs() < 0.01, "expected Tctl 31.0, got {t}");
    }

    #[test]
    fn tdie_is_preferred_over_tctl() {
        // Tctl carries a fixed offset above Tdie on some AMD parts, so Tdie is
        // the real junction temperature.
        let text = "TXT|k10temp|Tctl|59750\nTXT|k10temp|Tdie|49750\n";
        assert!((parse_cpu_temp(text).unwrap() - 49.75).abs() < 0.01);
    }

    #[test]
    fn intel_package_temperature_is_found() {
        let text = "\
TXT|coretemp|Package id 0|54000
TXT|coretemp|Core 0|51000
TXT|coretemp|Core 1|53000
TXT|nvme|Composite|60000
";
        assert!((parse_cpu_temp(text).unwrap() - 54.0).abs() < 0.01);
    }

    #[test]
    fn per_core_is_used_when_no_package_sensor_exists() {
        // Falls back to the hottest core, which is the one that matters.
        let text = "TXT|coretemp|Core 0|44000\nTXT|coretemp|Core 1|61000\n";
        assert!((parse_cpu_temp(text).unwrap() - 61.0).abs() < 0.01);
    }

    #[test]
    fn a_host_with_no_cpu_sensor_reports_none() {
        // A VM typically exposes no CPU sensor at all. None, never a zero that
        // would render as a plausible cold CPU.
        let text = "TXT|nvme|Composite|49850\nTXT|acpitz||16800\n";
        assert_eq!(parse_cpu_temp(text), None);
    }

    #[test]
    fn implausible_readings_are_discarded() {
        // A failed sensor commonly reports exactly 0 or a huge value. Both
        // must yield None rather than a plausible-looking temperature.
        let text = "TXT|k10temp|Tctl|0\nTXT|k10temp|Tccd1|4294967\n";
        assert_eq!(parse_cpu_temp(text), None);
    }

    #[test]
    fn labels_containing_spaces_survive_parsing() {
        // Whitespace splitting would have lost the value of "Package id 0".
        let text = "TXT|coretemp|Package id 0|48000\n";
        assert!((parse_cpu_temp(text).unwrap() - 48.0).abs() < 0.01);
    }

    #[test]
    fn temperature_lines_do_not_disturb_the_rest_of_the_frame() {
        let text = format!("cpu  100 0 50 900\ncpu0 100 0 50 900\n{CROW}");
        let f = parse_frame(&text);
        assert_eq!(f.stat.cores.len(), 1, "temp lines must not parse as cores");
        assert!(f.cpu_temp_c.is_some());
    }

    #[test]
    fn the_sampler_command_collects_temperatures() {
        let cmd = sampler_command(1);
        assert!(cmd.contains("/sys/class/hwmon"));
        assert!(cmd.contains("TXT|"));
        assert!(!cmd.contains("[["), "must stay POSIX sh");
    }
}

#[cfg(test)]
mod gpu_tests {
    use super::*;

    /// Real nvidia-smi output from dove's RTX 3080.
    const CROW_GPU: &str = "TXG|0, NVIDIA GeForce RTX 3080, 0, 1969, 10240, 17.36\n";

    #[test]
    fn parses_a_real_card() {
        let g = parse_gpu(CROW_GPU).expect("dove reports a GPU");
        assert_eq!(g.name, "NVIDIA GeForce RTX 3080");
        assert_eq!(g.util_pct, 0.0);
        assert_eq!(g.mem_used_mb, 1969);
        assert_eq!(g.mem_total_mb, 10240);
        assert!((g.power_w - 17.36).abs() < 0.01);
    }

    #[test]
    fn a_host_without_a_gpu_reports_none() {
        // The overwhelmingly common case: no nvidia-smi, so no TXG lines.
        let text = "cpu  100 0 50 900\nTXT|k10temp|Tctl|31000\n";
        assert_eq!(parse_gpu(text), None);
    }

    #[test]
    fn power_draw_unsupported_does_not_discard_the_reading() {
        // Many cards report [N/A] for power. Utilisation and memory are still
        // worth having, so only power degrades.
        let text = "TXG|0, NVIDIA T400, 12, 300, 2048, [N/A]\n";
        let g = parse_gpu(text).expect("still a valid reading");
        assert_eq!(g.util_pct, 12.0);
        assert_eq!(g.power_w, 0.0);
    }

    #[test]
    fn a_malformed_utilisation_discards_the_reading() {
        // Zero here would be indistinguishable from a genuinely idle GPU.
        let text = "TXG|0, NVIDIA T400, oops, 300, 2048, 15\n";
        assert_eq!(parse_gpu(text), None);
    }

    #[test]
    fn a_truncated_line_is_skipped() {
        assert_eq!(parse_gpu("TXG|0, NVIDIA T400, 12\n"), None);
    }

    #[test]
    fn the_first_gpu_wins_on_a_multi_gpu_host() {
        let text = "TXG|0, NVIDIA A100, 55, 4000, 40960, 210\n\
                    TXG|1, NVIDIA A100, 3, 100, 40960, 60\n";
        let g = parse_gpu(text).unwrap();
        assert_eq!(g.util_pct, 55.0, "index 0 is the one reported");
    }

    #[test]
    fn gpu_lines_do_not_disturb_the_rest_of_the_frame() {
        let text = format!("cpu  100 0 50 900\ncpu0 100 0 50 900\n{CROW_GPU}");
        let f = parse_frame(&text);
        assert_eq!(f.stat.cores.len(), 1, "TXG lines must not parse as cores");
        assert!(f.gpu.is_some());
    }

    #[test]
    fn the_sampler_command_collects_gpu_and_tolerates_absence() {
        let cmd = sampler_command(1);
        assert!(cmd.contains("nvidia-smi"));
        assert!(
            cmd.contains("command -v"),
            "must not error on hosts without it"
        );
        assert!(cmd.contains("TXG|"));
        assert!(!cmd.contains("[["), "must stay POSIX sh");
    }
}

#[cfg(test)]
mod phase9_tests {
    use super::*;

    #[test]
    fn the_command_reads_identity_once_before_the_loop() {
        let cmd = sampler_command(1);
        let facts_at = cmd.find("TXI|kernel").expect("identity is collected");
        let loop_at = cmd.find("while :;").expect("there is a loop");
        assert!(
            facts_at < loop_at,
            "identity must not be re-read every frame"
        );
    }

    #[test]
    fn disk_capacity_is_not_read_every_frame() {
        let cmd = sampler_command(1);
        assert!(cmd.contains("df -P -k"));
        assert!(
            cmd.contains(&format!("% {}", DF_EVERY_MS / 1000)),
            "df should be rate-limited"
        );
    }

    #[test]
    fn the_command_stays_posix_sh() {
        let cmd = sampler_command(1);
        assert!(!cmd.contains("[["), "no bashisms");
        assert!(cmd.contains("$(("), "arithmetic is POSIX $(( ))");
    }

    #[test]
    fn a_frame_carries_facts_uptime_and_filesystems() {
        let text = "\
TXI|kernel|Linux 6.12.101+deb13-amd64
TXI|os|Debian GNU/Linux 13 (trixie)
TXI|cpu|AMD Ryzen 9 5950X 16-Core Processor
cpu  100 0 50 900
cpu0 100 0 50 900
TXU|858066.79
TXF|/dev/nvme0n1p1 1888752112 158276184 1634458828 9% /
";
        let f = parse_frame(text);
        assert_eq!(
            f.stat.cores.len(),
            1,
            "the new lines must not parse as cores"
        );
        assert_eq!(f.uptime_secs, Some(858_066));
        assert_eq!(
            f.facts.as_ref().unwrap().cpu_model,
            "AMD Ryzen 9 5950X 16-Core Processor"
        );
        assert_eq!(f.filesystems.len(), 1);
    }

    #[test]
    fn a_frame_without_them_reports_none_rather_than_empty_strings() {
        // Most frames carry neither: facts come once, df every DF_EVERY.
        let f = parse_frame("cpu 100 0 50 900\ncpu0 100 0 50 900\n");
        assert!(
            f.facts.is_none(),
            "absent facts must be None, not blank strings"
        );
        assert!(f.filesystems.is_empty());
        assert_eq!(f.uptime_secs, None);
    }
}

#[cfg(test)]
mod all_sensor_tests {
    use super::*;

    /// Real hwmon output from dove, where the NVMe runs 40 degrees hotter than
    /// the CPU. This is the case the whole design turns on.
    const CROW: &str = "\
TXT|acpitz||16800
TXT|acpitz||16800
TXT|nvme|Composite|49850
TXT|nvme|Sensor 1|71850
TXT|nvme|Sensor 2|39850
TXT|k10temp|Tctl|31625
TXT|k10temp|Tccd1|33250
TXT|k10temp|Tccd2|34500
TXT|gigabyte_wmi||34000
TXT|gigabyte_wmi||37000
TXT|gigabyte_wmi||31000
TXT|gigabyte_wmi||34000
";

    #[test]
    fn the_hottest_sensor_is_not_the_cpu_temperature() {
        // 71.9C of NVMe beside 31.6C of CPU. Reporting the former as the CPU
        // would name the wrong component with total confidence, which is the
        // failure this whole project was built in response to.
        let temps = parse_temps(CROW);
        let (name, c) = hottest(&temps).unwrap();
        assert_eq!(name, "nvme Sensor 1");
        assert!((c - 71.85).abs() < 0.01);

        let cpu = parse_cpu_temp(CROW).unwrap();
        assert!((cpu - 31.625).abs() < 0.01, "cpu is {cpu}");
        assert!(c > cpu + 30.0, "the two must not be confusable");
    }

    #[test]
    fn unlabelled_sensors_are_numbered_rather_than_collapsed() {
        // dove's board exposes four gigabyte_wmi inputs with no labels. Naming
        // them all "gigabyte_wmi" would silently drop three readings.
        let temps = parse_temps(CROW);
        let wmi: Vec<_> = temps
            .iter()
            .filter(|t| t.driver == "gigabyte_wmi")
            .collect();
        assert_eq!(wmi.len(), 4);

        let names: Vec<String> = wmi.iter().enumerate().map(|(i, t)| t.name(i)).collect();
        assert_eq!(
            names,
            [
                "gigabyte_wmi",
                "gigabyte_wmi 2",
                "gigabyte_wmi 3",
                "gigabyte_wmi 4"
            ]
        );
    }

    #[test]
    fn every_sensor_is_kept_not_only_the_cpus() {
        // The collection always shipped all of these; only the presentation
        // threw them away.
        let temps = parse_temps(CROW);
        assert_eq!(temps.len(), 12);
        assert_eq!(
            temps.iter().filter(|t| t.kind == SensorKind::Drive).count(),
            3
        );
        assert_eq!(
            temps.iter().filter(|t| t.kind == SensorKind::Cpu).count(),
            3
        );
        assert_eq!(
            temps.iter().filter(|t| t.kind == SensorKind::Board).count(),
            6
        );
    }

    #[test]
    fn a_host_with_no_sensors_yields_an_empty_list_not_a_zero() {
        // heron is a VM and reports nothing at all. Zero degrees would render
        // as an implausibly cold machine rather than as an absent sensor.
        assert!(parse_temps("").is_empty());
        assert!(hottest(&[]).is_none());
    }

    #[test]
    fn an_implausible_reading_is_discarded_from_the_list_too() {
        // A failed sensor commonly reports 0 or something enormous. The same
        // floor the CPU ranking uses applies here, or the hottest-sensor bar
        // would be pinned by a broken input on some host forever.
        let t =
            parse_temps("TXT|nvme|Composite|0\nTXT|nvme|Sensor 1|250000\nTXT|k10temp|Tctl|31000\n");
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].driver, "k10temp");
    }

    #[test]
    fn wireless_and_drive_sensors_are_told_apart() {
        let t = parse_temps("TXT|iwlwifi_1||28000\nTXT|drivetemp|Composite|41000\n");
        assert_eq!(t[0].kind, SensorKind::Wireless);
        assert_eq!(t[1].kind, SensorKind::Drive);
    }
}

#[cfg(test)]
mod rate_tests {
    use super::*;

    #[test]
    fn sleep_takes_whole_seconds_as_integers_and_fractions_as_decimals() {
        // `sleep 1` is POSIX; `sleep 0.25` is not, but every host in this
        // fleet runs GNU coreutils and takes it - checked on all eighteen
        // before this shipped. Whole seconds stay integers so the common case
        // never depends on that.
        assert_eq!(sleep_arg(1000), "1");
        assert_eq!(sleep_arg(30_000), "30");
        assert_eq!(sleep_arg(500), "0.5");
        assert_eq!(sleep_arg(250), "0.25");
        assert_eq!(sleep_arg(0), "0.001", "never zero: it would spin the loop");
    }

    #[test]
    fn the_expensive_extras_keep_their_wall_clock_cadence() {
        // nvidia-smi is a process spawn costing hundreds of milliseconds. A
        // frame-counted schedule would run it four times a second at 4 Hz -
        // real load on a machine we are only supposed to be watching.
        let one_hz = sampler_command(1000);
        let four_hz = sampler_command(250);
        assert!(
            one_hz.contains("% 1)"),
            "1 Hz: extras every frame\n{one_hz}"
        );
        assert!(
            four_hz.contains("% 4)"),
            "4 Hz: extras every 4th frame\n{four_hz}"
        );
        // df is the same argument in bytes: 30 s either way.
        assert!(one_hz.contains("% 30)"));
        assert!(four_hz.contains("% 120)"));
    }

    #[test]
    fn a_rate_reads_as_a_frequency_below_a_second_and_a_period_above() {
        assert_eq!(rate_label(250), "4 Hz");
        assert_eq!(rate_label(500), "2 Hz");
        assert_eq!(rate_label(1000), "1 s");
        assert_eq!(rate_label(30_000), "30 s");
    }
}
