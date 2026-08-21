//! Host facts and filesystem usage.
//!
//! Two kinds of data that do not belong in the per-second frame: identity,
//! which never changes, and disk capacity, which changes slowly. Both are
//! emitted on their own cadence rather than 86,400 times a day.

/// What a machine is. Read once per connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostFacts {
    /// e.g. "Linux 6.12.101+deb13-amd64"
    pub kernel: String,
    /// e.g. "Debian GNU/Linux 13 (trixie)"
    pub os: String,
    /// e.g. "AMD Ryzen 9 5950X 16-Core Processor"
    pub cpu_model: String,
    /// e.g. "x86_64"
    pub arch: String,
}

impl HostFacts {
    pub fn is_empty(&self) -> bool {
        self.kernel.is_empty() && self.os.is_empty() && self.cpu_model.is_empty()
    }
}

/// One mounted filesystem worth showing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FsEntry {
    pub mount: String,
    pub total_kb: u64,
    pub used_kb: u64,
}

impl FsEntry {
    pub fn used_pct(&self) -> f32 {
        if self.total_kb == 0 {
            return 0.0;
        }
        (self.used_kb as f32 / self.total_kb as f32 * 100.0).clamp(0.0, 100.0)
    }
}

/// Filesystem sources that are not disks.
///
/// A real `df` is mostly noise: on a plain Debian box, seven of eight lines
/// are tmpfs, udev, efivarfs or per-service credential mounts. Reporting
/// `/tmp` on tmpfs as "12% disk used" would be reporting RAM as disk.
///
/// Excluding by source rather than requiring a `/dev/` prefix keeps ZFS pools
/// and network mounts, whose sources are a pool name or `host:/export`.
const PSEUDO: &[&str] = &[
    "tmpfs",
    "devtmpfs",
    "udev",
    "efivarfs",
    "overlay",
    "squashfs",
    "proc",
    "sysfs",
    "cgroup",
    "cgroup2",
    "none",
    "ramfs",
    "debugfs",
    "tracefs",
    "securityfs",
    "pstore",
    "bpf",
    "configfs",
    "fusectl",
    "mqueue",
    "hugetlbfs",
    "binfmt_misc",
    "systemd-1",
    "devpts",
    "autofs",
    "nsfs",
];

fn is_pseudo(source: &str) -> bool {
    if PSEUDO.contains(&source) {
        return true;
    }
    // snap mounts and fuse helpers appear under varied names.
    source.starts_with("/dev/loop") || source.starts_with("gvfsd") || source.starts_with("portal")
}

/// Parse the `TXF|` lines emitted from `df -P -k`.
///
/// Duplicate mount points (bind mounts list twice) keep the first entry, and
/// zero-sized filesystems are dropped: a 0 KB mount is a placeholder, and
/// dividing by it would be a percentage of nothing.
pub fn parse_filesystems(text: &str) -> Vec<FsEntry> {
    let mut out: Vec<FsEntry> = Vec::new();

    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXF|") else {
            continue;
        };
        let f: Vec<&str> = rest.split_ascii_whitespace().collect();
        // source, 1k-blocks, used, available, capacity, mount
        if f.len() < 6 {
            continue;
        }
        if is_pseudo(f[0]) {
            continue;
        }
        let (Ok(total), Ok(used)) = (f[1].parse::<u64>(), f[2].parse::<u64>()) else {
            continue;
        };
        if total == 0 {
            continue;
        }
        // A mount point can contain spaces; df puts it last, so rejoin.
        let mount = f[5..].join(" ");
        if out.iter().any(|e| e.mount == mount) {
            continue;
        }
        out.push(FsEntry {
            mount,
            total_kb: total,
            used_kb: used,
        });
    }

    out
}

/// The fullest filesystem, which is the one that matters.
///
/// A host with a roomy `/home` and a full `/` is in trouble, and an average
/// across mounts would hide it.
pub fn fullest(all: &[FsEntry]) -> Option<&FsEntry> {
    all.iter().max_by(|a, b| {
        a.used_pct()
            .partial_cmp(&b.used_pct())
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

/// Parse the `TXI|` identity lines.
pub fn parse_facts(text: &str) -> HostFacts {
    let mut f = HostFacts::default();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXI|") else {
            continue;
        };
        let Some((key, value)) = rest.split_once('|') else {
            continue;
        };
        let value = value.trim().to_string();
        if value.is_empty() {
            continue;
        }
        match key {
            "kernel" => f.kernel = value,
            "os" => f.os = value,
            "cpu" => f.cpu_model = value,
            "arch" => f.arch = value,
            _ => {}
        }
    }
    f
}

/// Seconds since boot, from the `TXU|` line.
pub fn parse_uptime(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.strip_prefix("TXU|"))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v >= 0.0)
        .map(|v| v as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `df -P -k` from dove, prefixed as the sampler emits it.
    const CROW_DF: &str = "\
TXF|udev              16383292         0   16383292       0% /dev
TXF|tmpfs              3280928      1184    3279744       1% /run
TXF|/dev/nvme0n1p1  1888752112 158276184 1634458828       9% /
TXF|tmpfs             16404628         8   16404620       1% /dev/shm
TXF|tmpfs                 5120         0       5120       0% /run/lock
TXF|tmpfs                 1024         0       1024       0% /run/credentials/systemd-journald.service
TXF|tmpfs             16404632   1843144   14561488      12% /tmp
";

    #[test]
    fn only_real_filesystems_survive() {
        // Seven of eight lines are noise. /tmp on tmpfs is 12% used but it is
        // RAM, and reporting it as disk would be reporting the wrong thing.
        let fs = parse_filesystems(CROW_DF);
        assert_eq!(fs.len(), 1, "got {fs:?}");
        assert_eq!(fs[0].mount, "/");
        assert_eq!(fs[0].total_kb, 1_888_752_112);
    }

    #[test]
    fn usage_matches_what_df_reported() {
        let fs = parse_filesystems(CROW_DF);
        // df said 9%.
        assert!(
            (fs[0].used_pct() - 8.38).abs() < 0.1,
            "got {}",
            fs[0].used_pct()
        );
    }

    #[test]
    fn zfs_and_network_mounts_are_kept() {
        // Excluding by source rather than requiring /dev/ is what keeps these.
        let text = "\
TXF|tank/data 1000000 400000 600000 40% /tank
TXF|nas:/export 2000000 1000000 1000000 50% /mnt/nas
TXF|//fileserver/share 500000 250000 250000 50% /mnt/smb
";
        let fs = parse_filesystems(text);
        assert_eq!(fs.len(), 3, "got {fs:?}");
    }

    #[test]
    fn the_fullest_mount_is_found_not_the_average() {
        // A roomy /home must not hide a full /.
        let text = "\
TXF|/dev/sda1 100000 95000 5000 95% /
TXF|/dev/sdb1 1000000 100000 900000 10% /home
";
        let fs = parse_filesystems(text);
        let worst = fullest(&fs).unwrap();
        assert_eq!(worst.mount, "/");
        assert!(worst.used_pct() > 94.0);
    }

    #[test]
    fn a_zero_sized_mount_is_dropped_not_divided_by() {
        let fs = parse_filesystems("TXF|/dev/sr0 0 0 0 - /media/cdrom\n");
        assert!(fs.is_empty());
    }

    #[test]
    fn a_mount_point_with_spaces_survives() {
        let fs = parse_filesystems("TXF|/dev/sdc1 1000 500 500 50% /mnt/my backup\n");
        assert_eq!(fs[0].mount, "/mnt/my backup");
    }

    #[test]
    fn bind_mounts_are_not_counted_twice() {
        let text = "\
TXF|/dev/sda1 100000 50000 50000 50% /
TXF|/dev/sda1 100000 50000 50000 50% /
";
        assert_eq!(parse_filesystems(text).len(), 1);
    }

    #[test]
    fn snap_loop_mounts_are_excluded() {
        let text = "TXF|/dev/loop3 130000 130000 0 100% /snap/core/1234\n";
        assert!(
            parse_filesystems(text).is_empty(),
            "a full snap is not a full disk"
        );
    }

    #[test]
    fn facts_parse_from_real_output() {
        let text = "\
TXI|kernel|Linux 6.12.101+deb13-amd64
TXI|arch|x86_64
TXI|os|Debian GNU/Linux 13 (trixie)
TXI|cpu|AMD Ryzen 9 5950X 16-Core Processor
";
        let f = parse_facts(text);
        assert_eq!(f.cpu_model, "AMD Ryzen 9 5950X 16-Core Processor");
        assert_eq!(f.os, "Debian GNU/Linux 13 (trixie)");
        assert!(!f.is_empty());
    }

    #[test]
    fn missing_facts_are_empty_not_invented() {
        // A container may have no /etc/os-release and no model name.
        let f = parse_facts("TXI|kernel|Linux 6.1\nTXI|os|\n");
        assert_eq!(f.kernel, "Linux 6.1");
        assert!(f.os.is_empty());
        assert!(f.cpu_model.is_empty());
    }

    #[test]
    fn uptime_parses_and_rejects_nonsense() {
        assert_eq!(parse_uptime("TXU|858066.79\n"), Some(858_066));
        assert_eq!(parse_uptime("TXU|not-a-number\n"), None);
        assert_eq!(parse_uptime("cpu 1 2 3 4\n"), None);
    }
}
