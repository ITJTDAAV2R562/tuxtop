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
    /// What kind of machine this is, from `systemd-detect-virt`: `none` for
    /// bare metal, otherwise the technology — `kvm`, `wsl`, `lxc`,
    /// `microsoft`, `vmware`. Empty when the host could not be asked.
    ///
    /// Every number a guest reports is honest *about the guest*, and that is
    /// exactly why this matters: without it a reader assumes the numbers
    /// describe hardware. owl reports 31 GB because that is what its WSL VM
    /// was given; the machine it runs on has 64.
    #[serde(default)]
    pub virt: String,
    /// `vm`, `container`, or empty. A container shares its host's kernel and
    /// so reports the *host's* uptime and often its core count, while a VM
    /// has its own — a distinction that changes what half these readings mean.
    #[serde(default)]
    pub virt_kind: String,
}

/// How a host's readings should be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Machine {
    /// Real silicon. Core counts are cores, and steal time is always zero.
    Metal,
    /// A virtual machine. Its "cores" are vCPUs carved out of a host that may
    /// be in this fleet too, and steal time becomes meaningful.
    Vm,
    /// A container sharing a kernel with its host.
    Container,
    /// The host could not be asked — an older system without
    /// `systemd-detect-virt`, or one that refused it.
    Unknown,
}

impl HostFacts {
    /// What kind of machine this is.
    ///
    /// `Unknown` rather than assuming metal: claiming a guest is hardware is
    /// the mistake this whole distinction exists to prevent, and an absent
    /// answer is not evidence of bare metal.
    pub fn machine(&self) -> Machine {
        // A value with whitespace is corrupt rather than a technology name -
        // the "none unknown" shape a broken fallback produced. Reading it as
        // anything definite would launder a bug into a claim.
        if self.virt.split_whitespace().count() > 1 {
            return Machine::Unknown;
        }
        match self.virt.as_str() {
            "none" => Machine::Metal,
            "" | "unknown" => Machine::Unknown,
            // systemd calls WSL a container - it has no firmware and no
            // virtual BIOS, so by its definition that is fair. For the
            // question this app asks it is wrong: WSL2 runs its own kernel
            // with its own memory allocation, which is precisely why owl
            // reports 31 GB while the machine it runs on has 64. A container
            // would report its host's. Classified by what its numbers mean,
            // not by how it boots.
            "wsl" => Machine::Vm,
            _ if self.virt_kind == "container" => Machine::Container,
            _ => Machine::Vm,
        }
    }

    /// Whether steal time can ever be non-zero here.
    ///
    /// On bare metal it is structurally zero — there is no hypervisor to take
    /// the time — so showing it beside real numbers implies a measurement
    /// where none exists. On a guest it answers "why is this slow when it
    /// looks idle", which is the question guests actually raise.
    pub fn steal_is_meaningful(&self) -> bool {
        matches!(self.machine(), Machine::Vm | Machine::Unknown)
    }
}

impl HostFacts {
    pub fn is_empty(&self) -> bool {
        self.kernel.is_empty() && self.os.is_empty() && self.cpu_model.is_empty()
    }
}

/// One mounted filesystem worth showing.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct FsEntry {
    /// The device or filesystem `df` named, used to tell one filesystem
    /// mounted twice from two filesystems. Not shown; the mount point is what
    /// a person recognises.
    #[serde(default)]
    pub source: String,
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
        // One filesystem, one entry - even when it is mounted twice. A bind
        // mount lists under both names with identical figures, and on this
        // fleet that is not rare: wader carries /boot and /mnt/hdd_root/boot,
        // owl carries /mnt/c and /usr/lib/wsl/drivers. Listing both would show
        // the same disk as two, which is the same double-count ADR-008 forbids
        // across hosts, one level down.
        //
        // The shortest path wins, because the canonical name is the one the
        // system was built around: /boot over /mnt/hdd_root/boot, /mnt/c over
        // /usr/lib/wsl/drivers.
        let source = f[0].to_string();
        if let Some(seen) = out.iter_mut().find(|e| e.source == source) {
            if mount.len() < seen.mount.len() {
                seen.mount = mount;
            }
            continue;
        }
        out.push(FsEntry {
            source,
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
            "virt" => f.virt = value,
            "virtkind" => f.virt_kind = value,
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

#[cfg(test)]
mod machine_tests {
    use super::*;

    fn facts(virt: &str, kind: &str) -> HostFacts {
        HostFacts {
            virt: virt.into(),
            virt_kind: kind.into(),
            ..Default::default()
        }
    }

    #[test]
    fn bare_metal_is_recognised() {
        // dove, wader and coot all report "none".
        assert_eq!(facts("none", "vm").machine(), Machine::Metal);
    }

    #[test]
    fn a_kvm_guest_and_a_wsl_guest_are_both_virtual_machines() {
        // heron is a Hetzner vServer; owl is WSL2, which is a Hyper-V VM with
        // its own kernel - which is why its 31 GB is the VM's and not N1's 64.
        assert_eq!(facts("kvm", "vm").machine(), Machine::Vm);
        assert_eq!(facts("wsl", "vm").machine(), Machine::Vm);
        assert_eq!(facts("microsoft", "vm").machine(), Machine::Vm);
    }

    #[test]
    fn wsl_is_a_virtual_machine_even_though_systemd_calls_it_a_container() {
        // `systemd-detect-virt --container` succeeds on WSL, so the host
        // itself reports virtkind=container. Trusting that would imply owl
        // shares N1's kernel and memory accounting - the exact confusion this
        // labelling exists to remove.
        assert_eq!(facts("wsl", "container").machine(), Machine::Vm);
        assert!(facts("wsl", "container").steal_is_meaningful());
    }

    #[test]
    fn a_detected_value_is_never_overwritten_by_the_fallback() {
        // systemd-detect-virt exits non-zero when it finds nothing and still
        // prints "none", so a naive `|| echo unknown` produced "none unknown"
        // on every bare-metal host in the fleet.
        assert_eq!(parse_facts("TXI|virt|none\n").machine(), Machine::Metal);
        assert_eq!(
            parse_facts("TXI|virt|none unknown\n").machine(),
            Machine::Unknown,
            "the broken shape must not read as metal either"
        );
    }

    #[test]
    fn a_container_is_told_apart_from_a_virtual_machine() {
        // coot runs LXC containers beside its VMs. A container shares its
        // host's kernel, so half its readings mean something different again.
        assert_eq!(facts("lxc", "container").machine(), Machine::Container);
        assert_eq!(facts("docker", "container").machine(), Machine::Container);
    }

    #[test]
    fn an_unanswered_host_is_not_assumed_to_be_hardware() {
        // Claiming a guest is bare metal is the mistake this distinction
        // exists to prevent, and silence is not evidence of silicon.
        assert_eq!(facts("", "").machine(), Machine::Unknown);
        assert_eq!(facts("unknown", "").machine(), Machine::Unknown);
    }

    #[test]
    fn steal_is_shown_only_where_it_can_be_non_zero() {
        // On bare metal there is no hypervisor to take the time, so a steal
        // figure there implies a measurement that does not exist.
        assert!(!facts("none", "vm").steal_is_meaningful());
        assert!(facts("kvm", "vm").steal_is_meaningful());
        // Unknown errs toward showing it: a hidden real number is worse than
        // a shown zero.
        assert!(facts("", "").steal_is_meaningful());
    }

    #[test]
    fn the_facts_line_is_parsed() {
        let f = parse_facts("TXI|kernel|Linux 6.6\nTXI|virt|kvm\nTXI|virtkind|vm\n");
        assert_eq!(f.virt, "kvm");
        assert_eq!(f.machine(), Machine::Vm);
    }

    #[test]
    fn a_host_from_before_this_existed_still_parses() {
        let f = parse_facts("TXI|kernel|Linux 6.6\nTXI|os|Debian 13\n");
        assert_eq!(f.machine(), Machine::Unknown);
    }
}

#[cfg(test)]
mod bind_mount_tests {
    use super::*;

    #[test]
    fn one_filesystem_mounted_twice_is_one_entry() {
        // wader really does carry /boot and /mnt/hdd_root/boot, the same
        // device under two names with identical figures. Listing both shows
        // one disk as two - and in a per-mount view it also double-counts the
        // fullest thing on the host.
        let txt = "\
TXF|/dev/sda2 1000 600 400 60% /boot
TXF|/dev/sda2 1000 600 400 60% /mnt/hdd_root/boot
TXF|/dev/sda1 2000 400 1600 20% /
";
        let fs = parse_filesystems(txt);
        assert_eq!(fs.len(), 2, "got {fs:?}");
        // The canonical name wins: the system was built around /boot, not
        // around where it happens to be bind-mounted.
        assert!(fs.iter().any(|e| e.mount == "/boot"));
        assert!(!fs.iter().any(|e| e.mount.contains("hdd_root")));
    }

    #[test]
    fn the_shorter_mount_wins_whichever_order_df_lists_them() {
        // df's order is not ours to rely on, so the rule has to hold both ways.
        let a = parse_filesystems(
            "TXF|/dev/x 100 50 50 50% /mnt/wsl/drivers\nTXF|/dev/x 100 50 50 50% /mnt/c\n",
        );
        let b = parse_filesystems(
            "TXF|/dev/x 100 50 50 50% /mnt/c\nTXF|/dev/x 100 50 50 50% /mnt/wsl/drivers\n",
        );
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].mount, "/mnt/c");
        assert_eq!(b[0].mount, "/mnt/c");
    }

    #[test]
    fn two_real_filesystems_are_never_merged() {
        // Identical figures are not identity, and this is not hypothetical.
        // wader carries /boot on /dev/nvme0n1p2 and /mnt/hdd_root/boot on
        // /dev/sda2, both reporting exactly 598112 KB used, because sda is a
        // clone of the nvme. Deduplicating on size-and-usage - the obvious
        // shortcut when the source strings differ - would hide one of two
        // real disks. Only a shared source means one filesystem.
        let fs = parse_filesystems(
            "TXF|/dev/nvme0n1p2 1000 598112 100 60% /boot\n\
             TXF|/dev/sda2 1000 598112 100 60% /mnt/hdd_root/boot\n",
        );
        assert_eq!(fs.len(), 2, "identical figures are not identity");
    }
}
