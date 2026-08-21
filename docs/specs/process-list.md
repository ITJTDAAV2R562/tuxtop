# Spec — Fleet-wide process list (Phase 5)

**Status:** settled. Ready to implement.

*"What is the busiest process anywhere in my fleet right now?"* — a question
neither Task Manager nor Beszel can answer, and the one that follows directly
from seeing a spike.

---

## Decisions taken

- **Fleet-wide first**, per-host as the drill-down.
- **CPU as a percentage of the whole box**, not of one core.
- **Read-only first.** No kill, no renice in this phase.

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

## Deliberately later

Kill and renice. Read-only ships sooner and keeps the "nothing but `cat`"
property, which has held since Phase 0, a while longer. When it does land, the
framing is not privilege — Tuxtop connects with the user's own SSH credentials
and grants no capability they lack — but **mistakes**: killing PID 1 on the
wrong host because two cards look alike. The work is confirmation and context,
not a permission model.
