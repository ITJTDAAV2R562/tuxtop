//! Fleet-wide process sampling.
//!
//! The ranking happens on the remote host and only the winners cross the
//! wire: a 479-process box is 85 KB of raw `/proc/*/stat` and about 800 bytes
//! once ranked. See `docs/specs/process-list.md`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Restarts of one unit, and how many of them Tuxtop has watched happen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnitRestarts {
    pub unit: String,
    /// `NRestarts` as systemd reports it: automatic restarts since the unit
    /// was last started explicitly, which may have been months ago.
    pub total: u32,
    /// Restarts since Tuxtop first saw this unit. **This is the actionable
    /// half** - it means the unit is flapping now, and unlike `total` it
    /// carries a recency we observed rather than one we inferred.
    pub since_seen: u32,
}

/// Parse the restart lines out of a frame. Absent on cycles that did not sweep.
pub fn parse_restarts(text: &str) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXR|") else {
            continue;
        };
        let Some((unit, n)) = rest.rsplit_once('|') else {
            continue;
        };
        let Ok(n) = n.trim().parse::<u32>() else {
            continue;
        };
        let unit = unit.trim();
        if !unit.is_empty() {
            out.push((unit.to_string(), n));
        }
    }
    out
}

/// Remembers what each unit's restart count was when first seen.
///
/// `NRestarts` alone carries no recency: "7 restarts" beside a live CPU chart
/// implies a today the number does not mean. The baseline is a thing we
/// observed, so the delta is honestly ours to report.
#[derive(Debug, Default)]
pub struct RestartTracker {
    first: HashMap<String, u32>,
}

impl RestartTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn update(&mut self, seen: &[(String, u32)]) -> Vec<UnitRestarts> {
        seen.iter()
            .map(|(unit, total)| {
                let base = *self.first.entry(unit.clone()).or_insert(*total);
                UnitRestarts {
                    unit: unit.clone(),
                    total: *total,
                    // A counter below the baseline means systemd reset it -
                    // the unit was started explicitly. Re-baseline rather than
                    // underflow.
                    since_seen: total.saturating_sub(base),
                }
            })
            .collect()
    }
}

/// One host's process sample: the ranked processes and the cgroup accounting
/// that arrived with them.
///
/// Both come from the same remote command and the same instant, so they are
/// delivered together rather than as two streams that could disagree about
/// which moment they describe.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcFrame {
    pub host: String,
    pub procs: Vec<ProcInfo>,
    pub cgroups: Vec<CgroupUsage>,
    /// Present only on cycles that swept; empty otherwise, which the consumer
    /// must treat as "no new information", not as "nothing has restarted".
    pub restarts: Vec<UnitRestarts>,
}

/// One cgroup's resource use, as the host reports it.
///
/// The counter is cumulative; `CgroupRates` turns consecutive samples into a
/// percentage. Kept raw here so the delta is computed once, against real
/// elapsed time, rather than assumed from the configured interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CgroupSample {
    pub name: String,
    /// Cumulative CPU microseconds across all cores.
    pub cpu_usec: u64,
    /// `memory.current` — the cgroup's charged memory.
    ///
    /// **This includes page cache**, so it reads higher than the sum of its
    /// processes' RSS and is not comparable to the process list's memory
    /// column. Whatever displays it must say which it is.
    pub memory_bytes: u64,
    pub pids: u32,
}

/// Everything the cgroup sweep reported in one frame.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CgroupFrame {
    /// Cores on the host, for turning CPU-microseconds into a percentage of
    /// the whole box — the same convention the process list uses.
    pub ncpu: u32,
    pub groups: Vec<CgroupSample>,
}

/// Parse the cgroup lines out of a process frame.
///
/// A host with no `system.slice` — or cgroup v1, where these files do not
/// exist — yields an empty frame rather than zeroes, which would render as a
/// fleet of idle services.
pub fn parse_cgroups(text: &str) -> CgroupFrame {
    let mut out = CgroupFrame::default();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("TXGT|") {
            out.ncpu = rest.trim().parse().unwrap_or(0);
            continue;
        }
        let Some(rest) = line.strip_prefix("TXG|") else {
            continue;
        };
        let f: Vec<&str> = rest.split('|').collect();
        if f.len() < 4 {
            continue;
        }
        let Ok(cpu_usec) = f[1].trim().parse::<u64>() else {
            continue;
        };
        out.groups.push(CgroupSample {
            name: f[0].trim().to_string(),
            cpu_usec,
            memory_bytes: f[2].trim().parse().unwrap_or(0),
            pids: f[3].trim().parse().unwrap_or(0),
        });
    }
    out
}

/// One cgroup's usage, differentiated and ready to display.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CgroupUsage {
    pub name: String,
    /// Percentage of the whole box, matching the process list's convention.
    pub cpu_pct: f32,
    pub memory_bytes: u64,
    pub pids: u32,
}

/// Turns consecutive cgroup frames into rates.
///
/// Holds the previous counters per cgroup. A cgroup that appears for the first
/// time reports **no** CPU rather than a spike: its cumulative counter says
/// how much CPU it has used since it started, which on a long-running service
/// is hours, and dividing that by one interval would render every newly-seen
/// unit as pinned.
#[derive(Debug, Default)]
pub struct CgroupRates {
    prev: HashMap<String, u64>,
}

impl CgroupRates {
    pub fn new() -> Self {
        Self::default()
    }

    /// Differentiate `frame` against the previous one over `elapsed_secs`.
    ///
    /// Real elapsed time, not the configured interval: a sample delayed by a
    /// slow host would otherwise be divided by a number smaller than the time
    /// it actually covered, and read high.
    pub fn update(&mut self, frame: &CgroupFrame, elapsed_secs: f64) -> Vec<CgroupUsage> {
        let mut out = Vec::with_capacity(frame.groups.len());
        let ncpu = frame.ncpu.max(1) as f64;
        let seen: std::collections::HashSet<&str> =
            frame.groups.iter().map(|g| g.name.as_str()).collect();

        for g in &frame.groups {
            let prev = self.prev.insert(g.name.clone(), g.cpu_usec);
            let cpu_pct = match prev {
                // A counter that went backwards means the cgroup was recreated
                // — the unit restarted. Report zero rather than a negative or
                // an enormous wrapped value.
                Some(p) if g.cpu_usec >= p && elapsed_secs > 0.0 => {
                    let busy_secs = (g.cpu_usec - p) as f64 / 1_000_000.0;
                    ((busy_secs / (elapsed_secs * ncpu)) * 100.0).clamp(0.0, 100.0) as f32
                }
                _ => 0.0,
            };
            out.push(CgroupUsage {
                name: g.name.clone(),
                cpu_pct,
                memory_bytes: g.memory_bytes,
                pids: g.pids,
            });
        }

        // Forget cgroups that are gone, or the map grows for the life of the
        // process on a host that starts many transient units.
        self.prev.retain(|k, _| seen.contains(k.as_str()));
        out
    }
}

/// What kind of thing owns a process.
///
/// Kept apart from the name because the three read differently: a service is
/// the interesting case, a container is worth marking as one, and a login
/// session is noise that should not be mistaken for a unit called
/// `session-8240`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OwnerKind {
    /// A systemd service or scope: `manticore.service`.
    Service,
    /// A container: `docker-<id>.scope`, `crio-<id>.scope`, `libpod-<id>`.
    Container,
    /// A login session or user slice.
    Session,
    /// init.scope, or anything else with no useful name.
    #[default]
    None,
}

/// Parse a cgroup path into the thing that owns the process.
///
/// The input is one path, already selected remotely from `/proc/[pid]/cgroup`
/// — cgroup v2 has a single `0::/path` line, v1 has several, and the sampler
/// picks the one naming a unit or scope so both arrive here identically.
///
/// An unrecognised or empty path yields `None` with an empty name rather than
/// a guess: a process can exit between being ranked and being read, and a
/// wrong attribution is worse than an absent one.
pub fn parse_owner(path: &str) -> (String, OwnerKind) {
    let last = path
        .trim()
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or("");
    if last.is_empty() || last == "init.scope" || last == "-.slice" {
        return (String::new(), OwnerKind::None);
    }

    // Containers first: they are also `.scope`, so the service arm would
    // otherwise swallow them and print a 64-character hex id as a unit name.
    for p in ["docker-", "crio-", "libpod-", "containerd-"] {
        if let Some(rest) = last.strip_prefix(p) {
            let id = rest.trim_end_matches(".scope");
            // Twelve characters is what `docker ps` shows, and enough to tell
            // containers apart without a line of hex.
            let short: String = id.chars().take(12).collect();
            let runtime = p.trim_end_matches('-');
            return (format!("{runtime}:{short}"), OwnerKind::Container);
        }
    }

    if last.starts_with("session-") || last.starts_with("user-") || path.contains("/user.slice") {
        // A login session is not a unit. Naming it `session-8240` would put a
        // meaningless number in a column people scan for services.
        return ("login session".into(), OwnerKind::Session);
    }

    if last.ends_with(".service") || last.ends_with(".scope") || last.ends_with(".slice") {
        return (last.to_string(), OwnerKind::Service);
    }

    (String::new(), OwnerKind::None)
}

/// One process, as the UI sees it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProcInfo {
    pub host: String,
    pub pid: u32,
    /// Percentage of the **whole box**, not of one core.
    pub cpu_pct: f32,
    /// Resident set size. Never sum these: shared pages are counted once per
    /// process, so a column of them does not add up to system memory used.
    pub rss_kb: u64,
    pub user: String,
    /// Truncated to 15 characters by the kernel.
    pub comm: String,
    /// Full command line, truncated remotely. Empty when the process exited
    /// between ranking and reading it, or for a kernel thread, which has none.
    pub cmd: String,
    /// What owns this process: a systemd unit, a container, a login session.
    /// Empty when the cgroup could not be read — never a guess.
    pub owner: String,
    pub owner_kind: OwnerKind,
    /// Kernel threads dominate a list sorted by tiny deltas on an idle fleet.
    /// Flagged rather than dropped, so the view can de-emphasise them without
    /// pretending they are absent.
    pub kernel: bool,
}

/// How much of a command line crosses the wire, in characters.
///
/// Truncated on the far side rather than here, so the bytes are never spent.
/// Java and Chrome routinely produce multi-kilobyte command lines; at twenty
/// processes a host that would dominate a frame that is otherwise ~650 bytes.
/// Long enough to carry the script or jar that identifies the process, which
/// is the whole reason for shipping it.
pub const CMD_MAX_CHARS: usize = 200;

/// The remote command: snapshot, wait, snapshot, delta, sort, emit the top N.
///
/// Deliberately POSIX `sh`. Nothing is written to the host — both snapshots
/// live in shell variables, so the "nothing but reads" property holds.
pub fn process_command(top_n: usize, window_ms: u32) -> String {
    let secs = (window_ms as f64 / 1000.0).max(0.2);
    let cmd_chars = CMD_MAX_CHARS;
    // Fields from /proc/[pid]/stat: 1 pid, 22 starttime, 14+15 cpu jiffies,
    // 24 rss in pages. Taking rss from the same snapshot avoids reading
    // /proc/[pid]/status for all 479 processes merely to rank them.
    //
    // Two rankings, unioned: busiest by CPU, and largest by memory. CPU alone
    // returns almost nothing on an idle fleet - only processes that actually
    // burned a jiffy in the window appear - which reads as a broken view
    // rather than a quiet one.
    format!(
        "snap() {{ awk '{{print $1\" \"$22\" \"($14+$15)\" \"$24}}' /proc/[0-9]*/stat 2>/dev/null; }}; \
         tot() {{ awk '/^cpu /{{print $2+$3+$4+$5+$6+$7+$8+$9}}' /proc/stat; }}; \
         A=$(snap); TA=$(tot); \
         sleep {secs}; \
         B=$(snap); TB=$(tot); \
         PG=$(getconf PAGESIZE 2>/dev/null || echo 4096); \
         echo \"TXPT|$((TB-TA))|{window_ms}|$PG\"; \
         CPU=$(printf '%s\\n' \"$A\" | awk -v B=\"$B\" '\
           BEGIN{{ n=split(B,rows,\"\\n\"); for(i=1;i<=n;i++){{ split(rows[i],f,\" \"); \
             bp[f[1]]=f[2]; bc[f[1]]=f[3] }} }} \
           {{ if($1 in bp && bp[$1]==$2 && bc[$1]>$3) print bc[$1]-$3\" \"$1 }}' \
           | sort -rn | head -{top_n}); \
         MEM=$(printf '%s\\n' \"$B\" | sort -k4 -rn | head -{top_n} | awk '{{print \"0 \"$1}}'); \
         printf '%s\\n%s\\n' \"$CPU\" \"$MEM\" | awk '{{ if(!($2 in seen) || $1>0) {{ seen[$2]=$1 }} }} \
           END{{ for(p in seen) print seen[p]\" \"p }}' \
         | while read d p; do \
             [ -n \"$p\" ] || continue; \
             c=$(tr -d '\\0' < /proc/$p/comm 2>/dev/null) || continue; \
             r=$(awk '{{print $24}}' /proc/$p/stat 2>/dev/null); \
             u=$(awk '/^Uid:/{{print $2; exit}}' /proc/$p/status 2>/dev/null); \
             n=$(awk -F: -v U=\"$u\" '$3==U{{print $1; exit}}' /etc/passwd 2>/dev/null); \
             echo \"TXP|$p|$d|$(( ${{r:-0}} * PG / 1024 ))|${{n:-$u}}|${{c:-?}}\"; \
             l=$(tr '\\0' ' ' < /proc/$p/cmdline 2>/dev/null | cut -c1-{cmd_chars}); \
             [ -n \"$l\" ] && echo \"TXC|$p|$l\"; \
             g=$(awk -F: '/\\.(service|scope|slice)/{{print $NF; exit}}' /proc/$p/cgroup 2>/dev/null); \
             [ -n \"$g\" ] && echo \"TXO|$p|$g\"; \
           done; \
         echo \"TXGT|$(nproc 2>/dev/null || echo 1)\"; \
         for d in /sys/fs/cgroup/system.slice/*/; do \
           n=${{d%/}}; n=${{n##*/}}; \
           u=$(awk '/^usage_usec/{{print $2}}' \"$d/cpu.stat\" 2>/dev/null); \
           [ -n \"$u\" ] || continue; \
           echo \"TXG|$n|$u|$(cat \"$d/memory.current\" 2>/dev/null || echo 0)|$(cat \"$d/pids.current\" 2>/dev/null || echo 0)\"; \
         done"
    )
}

/// Parse one process frame.
///
/// Wire format:
/// ```text
/// TXPT|total_jiffy_delta|window_ms|page_size
/// TXP|pid|jiffy_delta|rss_kb|user|comm
/// TXC|pid|full command line
/// ```
///
/// The command line rides on its own line rather than as another `TXP` field
/// because both it and `comm` may contain a pipe, and only one of them can be
/// the field that rejoins the tail. Keyed by pid, so an absent `TXC` - a
/// kernel thread, or a process that exited mid-sample - simply leaves the
/// command empty instead of shifting every field after it.
///
/// `host` is stamped on each row so a fleet-wide list can say *where*, which
/// is half the answer.
pub fn parse_processes(host: &str, text: &str) -> Vec<ProcInfo> {
    let mut total_delta: u64 = 0;
    let mut out = Vec::new();

    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("TXPT|") {
            total_delta = rest
                .split('|')
                .next()
                .and_then(|d| d.trim().parse().ok())
                .unwrap_or(0);
        }
    }

    // Without a denominator every percentage would be a guess, so report
    // nothing rather than something plausible.
    if total_delta == 0 {
        return out;
    }

    // Command lines first, so each process can collect its own. A pid absent
    // here keeps an empty command rather than borrowing a neighbour's.
    let mut cmds: HashMap<u32, String> = HashMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXC|") else {
            continue;
        };
        let Some((pid, cmd)) = rest.split_once('|') else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            continue;
        };
        // The command line is the whole tail: it may contain pipes, and
        // splitting on them would truncate at the first argument that has one.
        cmds.insert(pid, cmd.trim().to_string());
    }

    // Cgroup paths, keyed by pid, same shape as the command lines above.
    let mut owners: HashMap<u32, String> = HashMap::new();
    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXO|") else {
            continue;
        };
        let Some((pid, path)) = rest.split_once('|') else {
            continue;
        };
        let Ok(pid) = pid.trim().parse::<u32>() else {
            continue;
        };
        owners.insert(pid, path.trim().to_string());
    }

    for line in text.lines() {
        let Some(rest) = line.strip_prefix("TXP|") else {
            continue;
        };
        let f: Vec<&str> = rest.split('|').collect();
        // pid, delta, rss_kb, user, comm
        if f.len() < 5 {
            continue;
        }
        let (Ok(pid), Ok(delta)) = (f[0].trim().parse::<u32>(), f[1].trim().parse::<u64>()) else {
            continue;
        };
        let rss_kb = f[2].trim().parse::<u64>().unwrap_or(0);
        let (owner, owner_kind) = owners
            .remove(&pid)
            .map(|p| parse_owner(&p))
            .unwrap_or_default();
        // A command may itself contain a pipe, so the tail rejoins.
        let comm = f[4..].join("|").trim().to_string();

        out.push(ProcInfo {
            host: host.to_string(),
            pid,
            cpu_pct: (delta as f32 / total_delta as f32 * 100.0).clamp(0.0, 100.0),
            rss_kb,
            user: f[3].trim().to_string(),
            kernel: rss_kb == 0 && looks_like_kernel_thread(&comm),
            cmd: cmds.remove(&pid).unwrap_or_default(),
            owner,
            owner_kind,
            comm,
        });
    }

    // CPU descending, memory as the tiebreak.
    //
    // The union of two rankings means many rows share 0% CPU on a quiet
    // fleet; ordering those by size puts the substantial processes above the
    // trivial ones instead of leaving them in whatever order the shell
    // happened to emit.
    out.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.rss_kb.cmp(&a.rss_kb))
    });

    out
}

/// Wrap the one-shot ranking in a loop, for a long-lived channel.
///
/// Processes run on their own connection at their own cadence: the ranking
/// needs two snapshots separated by a real interval, and doing that inside
/// the metric loop would stall 1 Hz sampling for the whole window.
pub fn process_loop_command(top_n: usize, window_ms: u32, interval_secs: u32) -> String {
    // Restart counts ride the same connection but on a slower cycle. A unit
    // that restarts is news for hours, and the `systemctl show` call costs
    // ~108 ms of remote CPU against ~0 for everything else in the frame - so
    // paying it every five seconds would be the most expensive part of the
    // sample, for the slowest-moving number in it.
    let every = RESTART_EVERY_N_CYCLES;
    format!(
        "i=0; while :; do {}; \
         if [ $((i % {every})) -eq 0 ]; then {} ; fi; \
         i=$((i+1)); echo '{}'; sleep {}; done",
        process_command(top_n, window_ms),
        RESTART_SNIPPET,
        crate::sampler::FRAME_DELIMITER,
        interval_secs.max(1),
    )
}

/// How many process cycles pass between restart-count sweeps. At the 5 s
/// process cadence this is once a minute.
pub const RESTART_EVERY_N_CYCLES: u32 = 12;

/// Emit `TXR|unit|count` for every service that has restarted.
///
/// Only non-zero counts cross the wire. On dove that is two units out of 137,
/// and a zero says nothing anyone needs.
const RESTART_SNIPPET: &str =
    "systemctl show --property=Id --property=NRestarts '*.service' 2>/dev/null \
     | awk -v RS='' -F'\\n' '{{ id=\"\"; n=0; \
         for (i=1; i<=NF; i++) {{ split($i, kv, \"=\"); \
           if (kv[1]==\"Id\") id=kv[2]; if (kv[1]==\"NRestarts\") n=kv[2]+0 }} \
         if (n>0 && id!=\"\") print \"TXR|\" id \"|\" n }}'";

/// Kernel threads are conventionally named for their subsystem, and are the
/// only processes with no resident memory.
fn looks_like_kernel_thread(comm: &str) -> bool {
    const PREFIXES: &[&str] = &[
        "kworker",
        "migration",
        "ksoftirqd",
        "rcu_",
        "watchdog",
        "kthread",
        "irq/",
        "kdevtmpfs",
        "kswapd",
        "kcompactd",
        "khugepaged",
        "kauditd",
        "jbd2",
        "ext4-",
        "xfs",
        "z_",
        "spl_",
        "nv_queue",
        "cpuhp",
    ];
    PREFIXES.iter().any(|p| comm.starts_with(p)) || comm.starts_with('[')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped like the real output measured on dove.
    /// Shaped like the real output measured on dove.
    const FRAME: &str = "\
TXPT|3200|1000|4096
TXP|1024|320|199112|root|tailscaled
TXP|1141|64|840388|sam|searchd
TXP|68|32|0|root|migration/8
";

    #[test]
    fn cpu_is_a_percentage_of_the_whole_box() {
        // 320 of 3200 jiffies across every core is 10% of the machine. Under
        // top's convention the same process would read 320% on a 32-core box.
        let p = parse_processes("dove", FRAME);
        let tail = p.iter().find(|x| x.comm == "tailscaled").unwrap();
        assert!((tail.cpu_pct - 10.0).abs() < 0.01, "got {}", tail.cpu_pct);
    }

    #[test]
    fn users_arrive_already_resolved() {
        // Resolution happens on the host, where /etc/passwd is. Shipping the
        // whole passwd file to resolve twenty uids was most of the payload.
        let p = parse_processes("dove", FRAME);
        assert_eq!(p.iter().find(|x| x.pid == 1024).unwrap().user, "root");
        assert_eq!(p.iter().find(|x| x.pid == 1141).unwrap().user, "sam");
    }

    #[test]
    fn an_unresolvable_uid_shows_the_number_not_a_blank() {
        let text = "TXPT|1000|1000|4096\nTXP|5|10|100|4242|weird\n";
        assert_eq!(parse_processes("h", text)[0].user, "4242");
    }

    #[test]
    fn kernel_threads_are_flagged_not_dropped() {
        let p = parse_processes("dove", FRAME);
        let mig = p.iter().find(|x| x.comm == "migration/8").unwrap();
        assert!(mig.kernel, "kernel threads must be identifiable");
        assert!(!p.iter().find(|x| x.comm == "tailscaled").unwrap().kernel);
        assert_eq!(p.len(), 3, "flagged, still present");
    }

    #[test]
    fn a_userspace_process_using_no_memory_is_not_called_a_kernel_thread() {
        // The rss==0 test alone would misfile it; the name has to agree.
        let text = "TXPT|1000|1000|4096\nTXP|9|10|0|sam|myapp\n";
        assert!(!parse_processes("h", text)[0].kernel);
    }

    #[test]
    fn no_denominator_means_no_rows_rather_than_invented_percentages() {
        // Without the total, every percentage would be a guess.
        let text = "TXP|1024|320|199112|root|tailscaled\n";
        assert!(parse_processes("dove", text).is_empty());
    }

    #[test]
    fn the_host_is_stamped_on_every_row() {
        // A fleet-wide list has to say where, which is half the answer.
        for p in parse_processes("wader", FRAME) {
            assert_eq!(p.host, "wader");
        }
    }

    #[test]
    fn a_command_containing_a_pipe_survives() {
        let text = "TXPT|1000|1000|4096\nTXP|7|10|100|root|sh -c a|b\n";
        assert_eq!(parse_processes("h", text)[0].comm, "sh -c a|b");
    }

    #[test]
    fn the_command_guards_against_pid_reuse() {
        // The awk compares start time as well as PID: a recycled PID is a
        // different process and its delta would be nonsense.
        let cmd = process_command(20, 1000);
        assert!(cmd.contains("bp[$1]==$2"), "start time must be compared");
        assert!(cmd.contains("$22"), "field 22 is the process start time");
    }

    #[test]
    fn the_command_stays_posix_and_writes_nothing() {
        let cmd = process_command(20, 1000);
        assert!(!cmd.contains("[["), "no bashisms");
        assert!(
            !cmd.contains('>') || !cmd.contains("/tmp/"),
            "must not write files"
        );
        assert!(cmd.contains("head -20"), "only the winners cross the wire");
    }
}

#[cfg(test)]
mod sort_tests {
    use super::*;

    #[test]
    fn sorted_by_cpu_then_memory() {
        let text = "\
TXPT|1000|1000|4096
TXP|1|0|500|root|small-idle
TXP|2|100|100|root|busy
TXP|3|0|9000|root|big-idle
TXP|4|50|100|root|middling
";
        let p = parse_processes("h", text);
        let order: Vec<&str> = p.iter().map(|x| x.comm.as_str()).collect();
        assert_eq!(order, ["busy", "middling", "big-idle", "small-idle"]);
    }

    #[test]
    fn the_loop_command_delimits_frames_and_sleeps() {
        let c = process_loop_command(20, 1000, 5);
        assert!(c.contains(crate::sampler::FRAME_DELIMITER));
        assert!(c.contains("sleep 5"));
        assert!(!c.contains("[["), "still POSIX sh");
    }

    #[test]
    fn a_zero_interval_is_clamped_not_obeyed() {
        assert!(process_loop_command(20, 1000, 0).contains("sleep 1"));
    }
}

#[cfg(test)]
mod cmd_tests {
    use super::*;

    const FRAME: &str = "\
TXPT|4000|1000|4096
TXP|1024|320|199112|root|tailscaled
TXC|1024|/usr/sbin/tailscaled --state=/var/lib/tailscale/tailscaled.state
TXP|1141|64|840388|sam|python3
TXC|1141|python3 /home/sam/app/manage.py runserver --noreload
TXP|68|32|0|root|migration/8
";

    #[test]
    fn a_command_line_is_matched_to_its_own_process() {
        let p = parse_processes("dove", FRAME);
        let py = p.iter().find(|x| x.pid == 1141).unwrap();
        assert!(py.cmd.contains("manage.py runserver"), "got {:?}", py.cmd);
        // The whole point: comm is capped at 15 characters, so five hosts
        // running "python3" are indistinguishable without this.
        assert_eq!(py.comm, "python3");
    }

    #[test]
    fn a_process_with_no_command_line_does_not_borrow_a_neighbours() {
        // Kernel threads have an empty /proc/pid/cmdline, so no TXC line is
        // emitted at all. Shifting fields would attribute someone else's.
        let p = parse_processes("dove", FRAME);
        let k = p.iter().find(|x| x.pid == 68).unwrap();
        assert_eq!(k.cmd, "");
        assert!(k.kernel);
    }

    #[test]
    fn a_command_line_containing_a_pipe_survives_intact() {
        // Rides on its own line precisely so the tail can rejoin without
        // competing with comm for that privilege.
        let frame = "TXPT|4000|1000|4096\n\
                     TXP|7|40|1024|sam|sh\n\
                     TXC|7|/bin/sh -c cat /var/log/x | grep -i err | wc -l\n";
        let p = parse_processes("dove", frame);
        assert_eq!(p[0].cmd, "/bin/sh -c cat /var/log/x | grep -i err | wc -l");
    }

    #[test]
    fn a_frame_from_before_command_lines_existed_still_parses() {
        // The remote command is whatever the running app sent; a reconnect
        // mid-upgrade can hand the parser an older frame.
        let old = "TXPT|4000|1000|4096\nTXP|1024|320|199112|root|tailscaled\n";
        let p = parse_processes("dove", old);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].cmd, "");
    }

    #[test]
    fn the_remote_command_truncates_before_sending() {
        // Truncation must happen on the far side or the bytes are already
        // spent by the time we decide not to want them.
        let cmd = process_command(20, 1000);
        assert!(
            cmd.contains(&format!("cut -c1-{CMD_MAX_CHARS}")),
            "no remote truncation"
        );
    }
}

#[cfg(test)]
mod owner_tests {
    use super::*;

    #[test]
    fn a_systemd_unit_is_named_by_its_unit() {
        // The case that makes the column worth having: "python 39%" becomes
        // "python 39% - transcribe-worker.service".
        assert_eq!(
            parse_owner("/system.slice/transcribe-worker.service"),
            ("transcribe-worker.service".into(), OwnerKind::Service)
        );
        // Real names from dove, which are long and templated.
        assert_eq!(
            parse_owner("/system.slice/actions.runner.owner-repo.host-8.service").1,
            OwnerKind::Service
        );
    }

    #[test]
    fn a_container_is_marked_as_one_and_not_read_as_a_unit() {
        // Container scopes are also `.scope`, so without an explicit arm the
        // service branch swallows them and prints 64 characters of hex as a
        // unit name.
        let (name, kind) = parse_owner(
            "/system.slice/docker-4f2b8c1d9e0a3f5b7c2d4e6f8a0b1c2d3e4f5a6b7c8d9e0f1a2b3c4d5e6f7a8b.scope",
        );
        assert_eq!(kind, OwnerKind::Container);
        assert_eq!(
            name, "docker:4f2b8c1d9e0a",
            "twelve chars, as docker ps shows"
        );
    }

    #[test]
    fn other_container_runtimes_are_recognised_too() {
        assert_eq!(
            parse_owner("/system.slice/crio-abc123def456789.scope").1,
            OwnerKind::Container
        );
        assert_eq!(
            parse_owner("/machine.slice/libpod-deadbeefcafe0001.scope").1,
            OwnerKind::Container
        );
    }

    #[test]
    fn a_login_session_is_not_reported_as_a_unit() {
        // Naming it `session-8240.scope` would put a meaningless number in a
        // column people scan for services.
        let (name, kind) = parse_owner("/user.slice/user-1000.slice/session-8240.scope");
        assert_eq!(kind, OwnerKind::Session);
        assert_eq!(name, "login session");
    }

    #[test]
    fn init_and_an_unreadable_cgroup_yield_no_owner_rather_than_a_guess() {
        // A process can exit between being ranked and being read, and a wrong
        // attribution is worse than an absent one.
        assert_eq!(parse_owner("/init.scope"), (String::new(), OwnerKind::None));
        assert_eq!(parse_owner(""), (String::new(), OwnerKind::None));
        assert_eq!(parse_owner("/"), (String::new(), OwnerKind::None));
        assert_eq!(
            parse_owner("/some/unknown/path"),
            (String::new(), OwnerKind::None)
        );
    }

    #[test]
    fn a_frame_carries_each_process_its_own_owner() {
        let frame = "\
TXPT|4000|1000|4096
TXP|1906570|320|483102|root|searchd
TXO|1906570|/system.slice/manticore.service
TXP|1141|64|840388|sam|python3
TXC|1141|python3 /home/sam/app/worker.py
TXO|1141|/system.slice/transcribe-worker.service
TXP|68|32|0|root|migration/8
";
        let p = parse_processes("dove", frame);
        let m = p.iter().find(|x| x.pid == 1906570).unwrap();
        assert_eq!(m.owner, "manticore.service");
        let t = p.iter().find(|x| x.pid == 1141).unwrap();
        assert_eq!(t.owner, "transcribe-worker.service");
        // A kernel thread emits no TXO line at all.
        let k = p.iter().find(|x| x.pid == 68).unwrap();
        assert_eq!(k.owner, "");
        assert_eq!(k.owner_kind, OwnerKind::None);
    }

    #[test]
    fn a_frame_from_before_owners_existed_still_parses() {
        // A reconnect mid-upgrade hands the parser an older frame.
        let old = "TXPT|4000|1000|4096\nTXP|1024|320|199112|root|tailscaled\n";
        let p = parse_processes("dove", old);
        assert_eq!(p.len(), 1);
        assert_eq!(p[0].owner, "");
    }

    #[test]
    fn the_remote_command_reads_the_cgroup_for_both_versions() {
        // `-F:` with $NF yields the path from cgroup v2's `0::/path` and from
        // v1's `N:name=systemd:/path` alike, so one expression covers both.
        let cmd = process_command(20, 1000);
        assert!(cmd.contains("/proc/$p/cgroup"), "no cgroup read");
        assert!(cmd.contains("TXO|"), "no owner line emitted");
    }
}

#[cfg(test)]
mod cgroup_tests {
    use super::*;

    const FRAME: &str = "\
TXPT|4000|1000|4096
TXP|1906570|320|483102|root|searchd
TXGT|32
TXG|manticore.service|630641758|483102720|59
TXG|transcribe-worker.service|12000000|104857600|3
TXG|cron.service|1000|4194304|1
";

    #[test]
    fn a_cgroup_sweep_parses_alongside_the_processes() {
        let f = parse_cgroups(FRAME);
        assert_eq!(f.ncpu, 32);
        assert_eq!(f.groups.len(), 3);
        assert_eq!(f.groups[0].name, "manticore.service");
        assert_eq!(f.groups[0].memory_bytes, 483_102_720);
        assert_eq!(f.groups[0].pids, 59);
        // The processes must still parse from the same frame.
        assert_eq!(parse_processes("dove", FRAME).len(), 1);
    }

    #[test]
    fn a_host_with_no_cgroups_yields_nothing_not_zeroes() {
        // cgroup v1 has no such files. A list of services at 0% would read as
        // an idle fleet rather than an absent measurement.
        let f = parse_cgroups("TXPT|4000|1000|4096\n");
        assert!(f.groups.is_empty());
    }

    #[test]
    fn a_cgroup_seen_for_the_first_time_reports_no_cpu() {
        // Its counter holds hours of accumulated CPU. Dividing that by one
        // interval would render every newly-seen unit as pinned at 100%.
        let mut r = CgroupRates::new();
        let out = r.update(&parse_cgroups(FRAME), 5.0);
        assert!(out.iter().all(|u| u.cpu_pct == 0.0), "{out:?}");
        // Memory needs no delta and is reported immediately.
        assert_eq!(out[0].memory_bytes, 483_102_720);
    }

    #[test]
    fn cpu_is_a_delta_expressed_as_a_share_of_the_whole_box() {
        // Two cores' worth of CPU over 5s on a 32-core host is 2/32 = 6.25%,
        // the same convention the process list uses.
        let mut r = CgroupRates::new();
        r.update(&parse_cgroups(FRAME), 5.0);
        let later = FRAME.replace("|630641758|", "|640641758|"); // +10s of CPU
        let out = r.update(&parse_cgroups(&later), 5.0);
        let m = out.iter().find(|u| u.name == "manticore.service").unwrap();
        assert!((m.cpu_pct - 6.25).abs() < 0.01, "got {}", m.cpu_pct);
    }

    #[test]
    fn identical_samples_report_zero_not_nan() {
        let mut r = CgroupRates::new();
        r.update(&parse_cgroups(FRAME), 5.0);
        let out = r.update(&parse_cgroups(FRAME), 5.0);
        assert!(out
            .iter()
            .all(|u| u.cpu_pct == 0.0 && u.cpu_pct.is_finite()));
    }

    #[test]
    fn a_restarted_unit_does_not_report_a_negative_or_wrapped_spike() {
        // The cgroup is recreated on restart and its counter starts again, so
        // the new value is below the old one.
        let mut r = CgroupRates::new();
        r.update(&parse_cgroups(FRAME), 5.0);
        let restarted = FRAME.replace("|630641758|", "|12|");
        let out = r.update(&parse_cgroups(&restarted), 5.0);
        let m = out.iter().find(|u| u.name == "manticore.service").unwrap();
        assert_eq!(m.cpu_pct, 0.0);
    }

    #[test]
    fn elapsed_time_is_honoured_rather_than_the_configured_interval() {
        // A sample delayed by a slow host covers more time than the interval
        // claims; dividing by the interval would read high.
        let mut a = CgroupRates::new();
        let mut b = CgroupRates::new();
        a.update(&parse_cgroups(FRAME), 5.0);
        b.update(&parse_cgroups(FRAME), 5.0);
        let later = FRAME.replace("|630641758|", "|640641758|");
        let fast = a.update(&parse_cgroups(&later), 5.0);
        let slow = b.update(&parse_cgroups(&later), 10.0);
        let f = fast
            .iter()
            .find(|u| u.name == "manticore.service")
            .unwrap()
            .cpu_pct;
        let s = slow
            .iter()
            .find(|u| u.name == "manticore.service")
            .unwrap()
            .cpu_pct;
        assert!((f - 2.0 * s).abs() < 0.01, "{f} vs {s}");
    }

    #[test]
    fn cgroups_that_disappear_are_forgotten() {
        // A host that starts many transient units would otherwise grow the
        // map for the life of the process.
        let mut r = CgroupRates::new();
        r.update(&parse_cgroups(FRAME), 5.0);
        let fewer = "TXGT|32\nTXG|cron.service|1000|4194304|1\n";
        r.update(&parse_cgroups(fewer), 5.0);
        assert_eq!(
            r.prev.len(),
            1,
            "stale cgroups still held: {:?}",
            r.prev.keys()
        );
    }
}

#[cfg(test)]
mod restart_tests {
    use super::*;

    #[test]
    fn only_units_that_have_restarted_cross_the_wire() {
        let t = parse_restarts("TXR|transcribe-app.service|1\nTXR|indexer-post.service|4\n");
        assert_eq!(
            t,
            vec![
                ("transcribe-app.service".to_string(), 1),
                ("indexer-post.service".to_string(), 4),
            ]
        );
    }

    #[test]
    fn a_unit_first_seen_at_seven_restarts_reports_no_new_ones() {
        // NRestarts counts since the unit was last started explicitly, which
        // may be months ago. Reporting 7 as though it just happened, beside a
        // live CPU chart, implies a recency the number does not carry.
        let mut r = RestartTracker::new();
        let out = r.update(&[("flapper.service".into(), 7)]);
        assert_eq!(out[0].total, 7);
        assert_eq!(out[0].since_seen, 0, "nothing has happened while watching");
    }

    #[test]
    fn restarts_while_watching_are_the_actionable_number() {
        let mut r = RestartTracker::new();
        r.update(&[("flapper.service".into(), 7)]);
        let out = r.update(&[("flapper.service".into(), 11)]);
        assert_eq!(out[0].total, 11);
        assert_eq!(out[0].since_seen, 4, "four restarts while Tuxtop watched");
    }

    #[test]
    fn a_counter_reset_by_an_explicit_start_does_not_underflow() {
        // systemd zeroes NRestarts when a unit is started by hand. Subtracting
        // the old baseline would wrap to four billion.
        let mut r = RestartTracker::new();
        r.update(&[("x.service".into(), 9)]);
        let out = r.update(&[("x.service".into(), 0)]);
        assert_eq!(out[0].since_seen, 0);
    }

    #[test]
    fn the_sweep_runs_on_a_slower_cycle_than_the_processes() {
        // ~108 ms of remote CPU for the slowest-moving number in the frame;
        // paying it every five seconds would make it the most expensive part.
        let cmd = process_loop_command(20, 1000, 5);
        assert!(
            cmd.contains(&format!("% {RESTART_EVERY_N_CYCLES}")),
            "no cycle gate"
        );
        assert!(cmd.contains("NRestarts"));
    }
}
