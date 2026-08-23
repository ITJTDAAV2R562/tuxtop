# Spec — Fleet-wide process list (Phase 5)

**Status:** settled. Ready to implement.

*"What is the busiest process anywhere in my fleet right now?"* — a question
neither Task Manager nor Beszel can answer, and the one that follows directly
from seeing a spike.

---

## Decisions taken

- **Fleet-wide first**, with the command line as an expandable row rather
  than a separate per-host view *(revised when built: the fleet list already
  has the host column, filter and sort, so a second view would have
  duplicated them for no gain)*.
- **CPU as a percentage of the whole box**, not of one core.
- **Read-only, permanently.** No kill, no renice — ADR-010.

---

## Collection: rank remotely, ship the winners

Measured on dove, which runs 479 processes:

| approach | bytes per sample |
| --- | --- |
| ship raw `/proc/*/stat` | **85 KB** — 12x the entire metric frame |
| ship full `ps` output | 21 KB |
| **rank remotely, ship top 20** | **~800 bytes** |

Shipping raw is not an option at fleet scale, so the delta and the sort happen
on the far side and only the winners cross the wire. Still shell, still no
agent, still nothing written to the host.

### Why not `ps`

`ps` reports `%CPU` as an **average over the process's entire lifetime**, not
current usage. A process that burned a core for an hour and then went idle
still reads high. For a task manager that is not an approximation, it is the
wrong number — so CPU comes from a delta of `utime + stime` in
`/proc/[pid]/stat` across a known interval.

### Why a separate channel

Process sampling needs two snapshots separated by a real interval. Doing that
inside the metric loop would stall the 1 Hz sampling by however long the
process window takes.

So processes run on their **own SSH channel, on a slower cadence, started only
while the view is open**. A view nobody is looking at costs nothing — which is
also what makes fleet-wide affordable.

---

## CPU as a percentage of the box

```
cpu% = pid_jiffy_delta / total_jiffy_delta * 100
```

where `total_jiffy_delta` is the change in the aggregate `/proc/stat` `cpu`
row over the same window — which already covers every core.

`top` uses the other convention and will happily report 3200% on a 32-core
host. Percent-of-box was chosen because a fleet view is a **comparison**: a
process at 50% means the same thing on a 4-core VM and a 32-core workstation
only under this convention. The other reading is available by multiplying by
core count, and the UI states which it is using.

---

## Traps that produce confident wrong numbers

The reason this project exists, applied here:

- **PID reuse.** A short-lived process can be replaced between snapshots and
  the delta would then be nonsense. Guarded by comparing process start time
  (`/proc/[pid]/stat` field 22), which is unique per PID incarnation.
- **Summing RSS is not memory used.** Shared pages are counted once per
  process, so a column of RSS values does not add up to the system total and
  must never be presented as if it does.
- **`comm` is truncated to 15 characters.** Enough to identify, not enough to
  distinguish two JVMs. The full command line is fetched only for the drill-
  down, where one host is in view and the bytes are affordable.
- **Kernel threads** (`migration/8`, `kworker/...`) will dominate a list
  sorted by tiny deltas on an idle fleet. They are collected but flagged, so
  the view can de-emphasise them without pretending they are absent.

---

## Wire format

One line per process, from the remote ranking:

```
TXP|pid|starttime|jiffy_delta|rss_kb|uid|comm
TXPT|total_jiffy_delta|interval_ms      -- the denominator
TXPU|uid|username                       -- resolved once per distinct uid
```

---

## UI

A third view alongside Hosts and Fleet, sharing their conventions:

- One list, every host, sorted by CPU descending — the fleet's busiest work.
- Host name as a column, since "where" is half the answer.
- Clicking a row drills into that host: its processes only, full command
  lines.
- Same load bands as everywhere else, so a hot process reads hot.

## Deliberately never — kill and renice

*(Settled 2026-08-23. This section previously read "deliberately later".)*

Not being built. Tuxtop stays a pure observation tool; see
[ADR-010](../DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host).

The reasoning that made it look merely deferred is the reasoning that closes
it. The framing was never privilege — Tuxtop connects with the user's own SSH
credentials and grants no capability they lack. It was **mistakes**: killing
PID 1 on the wrong host because two cards look alike. And that is not a
confirmation-dialog problem, because the target was selected correctly and
misidentified. This UI is built to make nineteen machines look alike at a
glance, which is exactly what makes it a poor place to aim from.

---

## Why the command line rides on its own line

The wire gained `TXC|pid|command line` beside `TXP`, rather than another field
on `TXP`.

Both `comm` and the command line may contain a pipe, and only one field can be
the one that rejoins the tail. Keying by pid also means an absent command line
— a kernel thread has none, and a process can exit between being ranked and
being read — simply leaves the field empty instead of shifting every field
after it.

`comm` is capped at 15 characters by the kernel, so the two are genuinely
different information rather than the same string abbreviated. On dove that is
the difference between six rows reading `Runner.Listener` and six distinct
GitHub Actions runners.

**Cost, measured on dove (474 processes):** 635 → 2,083 bytes per sample.
Truncated to `CMD_MAX_CHARS` (200) on the far side, so the bytes are never
spent — Java and Chrome routinely produce multi-kilobyte command lines, and at
twenty processes those alone would dwarf the rest of the frame.
