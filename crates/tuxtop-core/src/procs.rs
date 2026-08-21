//! Fleet-wide process sampling.
//!
//! The ranking happens on the remote host and only the winners cross the
//! wire: a 479-process box is 85 KB of raw `/proc/*/stat` and about 800 bytes
//! once ranked. See `docs/specs/process-list.md`.

use serde::{Deserialize, Serialize};

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
    /// Kernel threads dominate a list sorted by tiny deltas on an idle fleet.
    /// Flagged rather than dropped, so the view can de-emphasise them without
    /// pretending they are absent.
    pub kernel: bool,
}

/// The remote command: snapshot, wait, snapshot, delta, sort, emit the top N.
///
/// Deliberately POSIX `sh`. Nothing is written to the host — both snapshots
/// live in shell variables, so the "nothing but reads" property holds.
pub fn process_command(top_n: usize, window_ms: u32) -> String {
    let secs = (window_ms as f64 / 1000.0).max(0.2);
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
           done"
    )
}

/// Parse one process frame.
///
/// Wire format:
/// ```text
/// TXPT|total_jiffy_delta|window_ms|page_size
/// TXP|pid|jiffy_delta|rss_kb|user|comm
/// ```
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
        // A command may itself contain a pipe, so the tail rejoins.
        let comm = f[4..].join("|").trim().to_string();

        out.push(ProcInfo {
            host: host.to_string(),
            pid,
            cpu_pct: (delta as f32 / total_delta as f32 * 100.0).clamp(0.0, 100.0),
            rss_kb,
            user: f[3].trim().to_string(),
            kernel: rss_kb == 0 && looks_like_kernel_thread(&comm),
            comm,
        });
    }

    out
}

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
