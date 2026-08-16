//! Parsing and delta maths for the Linux `/proc` files Tuxtop samples.
//!
//! Everything here is pure: text in, numbers out. No I/O, no SSH, no clock.
//! That is deliberate — it means the whole fast-plane maths is unit-testable
//! on any machine, including a WSL box that can never build the Windows GUI.

/// One row of `/proc/stat` — either the `cpu` aggregate or a `cpuN` core.
///
/// Field order is fixed by the kernel: user, nice, system, idle, iowait, irq,
/// softirq, steal, guest, guest_nice. Kernels older than 2.6.11 omit the later
/// fields, so everything past `idle` is optional and defaults to zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CpuTimes {
    pub user: u64,
    pub nice: u64,
    pub system: u64,
    pub idle: u64,
    pub iowait: u64,
    pub irq: u64,
    pub softirq: u64,
    pub steal: u64,
}

impl CpuTimes {
    /// Jiffies spent doing nothing.
    ///
    /// `iowait` counts as idle: the CPU was not executing anything, it was
    /// waiting on a device. Task Manager makes the same choice, and excluding
    /// it makes an NFS stall look like 100% CPU.
    pub fn idle_jiffies(&self) -> u64 {
        self.idle.saturating_add(self.iowait)
    }

    /// Every jiffy accounted for in this row.
    ///
    /// `guest` and `guest_nice` are deliberately excluded: the kernel already
    /// counts guest time inside `user` and `nice`, so adding them double-counts.
    pub fn total_jiffies(&self) -> u64 {
        self.user
            .saturating_add(self.nice)
            .saturating_add(self.system)
            .saturating_add(self.idle)
            .saturating_add(self.iowait)
            .saturating_add(self.irq)
            .saturating_add(self.softirq)
            .saturating_add(self.steal)
    }
}

/// A whole `/proc/stat` reading: the aggregate row plus one row per logical core.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StatSnapshot {
    pub aggregate: CpuTimes,
    pub cores: Vec<CpuTimes>,
}

/// Parse the CPU rows of `/proc/stat`.
///
/// Ignores every other line (`intr`, `ctxt`, `procs_running`, …). Cores are
/// returned in the order the kernel lists them, which is `cpu0`, `cpu1`, … —
/// the index is *not* re-read from the label, because a row that fails to
/// parse would silently shift every later core's identity.
///
/// A malformed numeric field makes that row parse as zeros rather than
/// aborting the whole snapshot; a single unreadable core should not blind the
/// entire host.
pub fn parse_stat(text: &str) -> StatSnapshot {
    let mut snap = StatSnapshot::default();

    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(label) = fields.next() else { continue };
        if !label.starts_with("cpu") {
            continue;
        }

        let mut n = [0u64; 8];
        for slot in n.iter_mut() {
            match fields.next() {
                Some(tok) => *slot = tok.parse().unwrap_or(0),
                None => break,
            }
        }

        let times = CpuTimes {
            user: n[0],
            nice: n[1],
            system: n[2],
            idle: n[3],
            iowait: n[4],
            irq: n[5],
            softirq: n[6],
            steal: n[7],
        };

        if label == "cpu" {
            snap.aggregate = times;
        } else {
            snap.cores.push(times);
        }
    }

    snap
}

/// Busy percentage between two readings of the same CPU row, in `0.0..=100.0`.
///
/// Returns `0.0` when no jiffies elapsed between the samples. That case is
/// real, not defensive padding: poll faster than the kernel's tick and two
/// consecutive reads are genuinely identical.
///
/// Counters are monotonic in practice but *not* guaranteed across a CPU
/// hotplug or suspend/resume, so a backwards delta is clamped to zero rather
/// than wrapping into a nonsense spike.
pub fn busy_pct(prev: &CpuTimes, cur: &CpuTimes) -> f32 {
    let total = cur.total_jiffies().saturating_sub(prev.total_jiffies());
    if total == 0 {
        return 0.0;
    }
    let idle = cur.idle_jiffies().saturating_sub(prev.idle_jiffies());
    let busy = total.saturating_sub(idle);
    (busy as f32 / total as f32 * 100.0).clamp(0.0, 100.0)
}

/// Per-core busy percentages between two snapshots.
///
/// Zips to the shorter of the two core lists. A CPU that was offlined between
/// samples simply drops off the end instead of panicking on an index.
pub fn core_pcts(prev: &StatSnapshot, cur: &StatSnapshot) -> Vec<f32> {
    prev.cores
        .iter()
        .zip(cur.cores.iter())
        .map(|(p, c)| busy_pct(p, c))
        .collect()
}

/// Memory figures from `/proc/meminfo`, in kibibytes as the kernel reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MemInfo {
    pub total_kb: u64,
    pub available_kb: u64,
    pub swap_total_kb: u64,
    pub swap_free_kb: u64,
}

impl MemInfo {
    /// Bytes actually in use by applications.
    ///
    /// Uses `MemAvailable`, not `MemFree`. `MemFree` excludes reclaimable page
    /// cache and so reports a healthy Linux box as nearly out of memory —
    /// the single most common way remote monitors get this wrong.
    pub fn used_kb(&self) -> u64 {
        self.total_kb.saturating_sub(self.available_kb)
    }

    pub fn used_pct(&self) -> f32 {
        if self.total_kb == 0 {
            return 0.0;
        }
        (self.used_kb() as f32 / self.total_kb as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// Parse the handful of `/proc/meminfo` keys Tuxtop displays.
///
/// Lines look like `MemTotal:       32791234 kB`. Unknown keys are skipped.
pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut mem = MemInfo::default();

    for line in text.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let Some(value) = rest.split_ascii_whitespace().next() else {
            continue;
        };
        let Ok(kb) = value.parse::<u64>() else {
            continue;
        };

        match key {
            "MemTotal" => mem.total_kb = kb,
            "MemAvailable" => mem.available_kb = kb,
            "SwapTotal" => mem.swap_total_kb = kb,
            "SwapFree" => mem.swap_free_kb = kb,
            _ => {}
        }
    }

    mem
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two real readings from dove, one second apart, trimmed to four cores.
    const STAT_A: &str = "\
cpu  1000 20 300 90000 100 5 15 0 0 0
cpu0 250 5 75 22500 25 1 3 0 0 0
cpu1 250 5 75 22500 25 1 3 0 0 0
cpu2 250 5 75 22500 25 1 3 0 0 0
cpu3 250 5 75 22500 25 1 3 0 0 0
intr 12345 0 0
ctxt 987654
procs_running 2
";

    const STAT_B: &str = "\
cpu  1100 20 350 90300 100 5 15 0 0 0
cpu0 350 5 125 22500 25 1 3 0 0 0
cpu1 250 5 75 22600 25 1 3 0 0 0
cpu2 250 5 75 22600 25 1 3 0 0 0
cpu3 250 5 75 22600 25 1 3 0 0 0
intr 12999 0 0
";

    #[test]
    fn parses_aggregate_and_cores() {
        let s = parse_stat(STAT_A);
        assert_eq!(s.cores.len(), 4);
        assert_eq!(s.aggregate.user, 1000);
        assert_eq!(s.aggregate.idle, 90000);
        assert_eq!(s.cores[0].system, 75);
    }

    #[test]
    fn ignores_non_cpu_lines() {
        let s = parse_stat(STAT_A);
        // `intr`, `ctxt` and `procs_running` must not become cores.
        assert_eq!(s.cores.len(), 4);
    }

    #[test]
    fn guest_time_is_not_double_counted() {
        // Trailing guest/guest_nice columns are present in STAT_A but excluded,
        // so total is the sum of the first eight fields only.
        let s = parse_stat(STAT_A);
        assert_eq!(
            s.aggregate.total_jiffies(),
            1000 + 20 + 300 + 90000 + 100 + 5 + 15
        );
    }

    #[test]
    fn iowait_counts_as_idle() {
        let t = CpuTimes {
            idle: 100,
            iowait: 50,
            ..Default::default()
        };
        assert_eq!(t.idle_jiffies(), 150);
    }

    #[test]
    fn busy_pct_matches_hand_computed_value() {
        let a = parse_stat(STAT_A);
        let b = parse_stat(STAT_B);
        // total delta = 100 user + 50 system + 300 idle = 450; busy = 150.
        let pct = busy_pct(&a.aggregate, &b.aggregate);
        assert!((pct - 150.0 / 450.0 * 100.0).abs() < 0.01, "got {pct}");
    }

    #[test]
    fn per_core_deltas_are_independent() {
        let a = parse_stat(STAT_A);
        let b = parse_stat(STAT_B);
        let pcts = core_pcts(&a, &b);
        assert_eq!(pcts.len(), 4);
        // cpu0 burned 100 user + 50 system, no idle movement -> fully busy.
        assert!((pcts[0] - 100.0).abs() < 0.01, "cpu0 = {}", pcts[0]);
        // cpu1..3 gained only idle jiffies -> completely idle.
        for (i, p) in pcts.iter().enumerate().skip(1) {
            assert!(p.abs() < 0.01, "cpu{i} = {p}");
        }
    }

    #[test]
    fn identical_samples_report_zero_not_nan() {
        let a = parse_stat(STAT_A);
        let pct = busy_pct(&a.aggregate, &a.aggregate);
        assert_eq!(pct, 0.0);
        assert!(pct.is_finite());
    }

    #[test]
    fn counters_going_backwards_clamp_to_zero() {
        let a = parse_stat(STAT_A);
        let b = parse_stat(STAT_B);
        // Arguments reversed: a suspend/resume or hotplug can look like this.
        let pct = busy_pct(&b.aggregate, &a.aggregate);
        assert!(pct.is_finite());
        assert_eq!(pct, 0.0);
    }

    #[test]
    fn short_rows_from_ancient_kernels_parse() {
        let s = parse_stat("cpu  100 0 50 900\ncpu0 100 0 50 900\n");
        assert_eq!(s.cores.len(), 1);
        assert_eq!(s.aggregate.idle, 900);
        assert_eq!(s.aggregate.steal, 0);
    }

    #[test]
    fn offlined_core_does_not_panic() {
        let a = parse_stat(STAT_A);
        let b = parse_stat("cpu  1100 20 350 90300 100 5 15 0\ncpu0 350 5 125 22500 25 1 3 0\n");
        let pcts = core_pcts(&a, &b);
        assert_eq!(pcts.len(), 1);
    }

    #[test]
    fn meminfo_uses_available_not_free() {
        let text = "\
MemTotal:       32791234 kB
MemFree:          512000 kB
MemAvailable:   25600000 kB
SwapTotal:      33554432 kB
SwapFree:       31000000 kB
Buffers:          100000 kB
";
        let m = parse_meminfo(text);
        assert_eq!(m.total_kb, 32_791_234);
        assert_eq!(m.available_kb, 25_600_000);
        // Had we used MemFree this would report ~98% used on a healthy box.
        assert_eq!(m.used_kb(), 32_791_234 - 25_600_000);
        assert!(m.used_pct() < 25.0, "got {}", m.used_pct());
    }

    #[test]
    fn meminfo_missing_keys_do_not_divide_by_zero() {
        let m = parse_meminfo("Buffers: 100 kB\n");
        assert_eq!(m.used_pct(), 0.0);
        assert!(m.used_pct().is_finite());
    }

    #[test]
    fn garbage_input_yields_empty_snapshot() {
        let s = parse_stat("not a stat file at all\n\n");
        assert_eq!(s.cores.len(), 0);
        assert_eq!(s.aggregate, CpuTimes::default());
    }
}
