//! Fleet-wide process sampling.
//!
//! The ranking happens on the remote host and only the winners cross the
//! wire: a 479-process box is 85 KB of raw `/proc/*/stat` and about 800 bytes
//! once ranked. See `docs/specs/process-list.md`.

use std::collections::HashMap;

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
    /// Full command line, truncated remotely. Empty when the process exited
    /// between ranking and reading it, or for a kernel thread, which has none.
    pub cmd: String,
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
            cmd: cmds.remove(&pid).unwrap_or_default(),
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
    format!(
        "while :; do {}; echo '{}'; sleep {}; done",
        process_command(top_n, window_ms),
        crate::sampler::FRAME_DELIMITER,
        interval_secs.max(1),
    )
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
