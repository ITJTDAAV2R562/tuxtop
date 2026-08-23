# Spec — ownership: what each process belongs to, and what each unit costs

**Status:** **built** · **Phase:** 10 · Shipped 2026-08-23
process list)

**Goal:** answer "what *is* this?" about a spike, and "what does this service
actually cost?" about a machine — without a services tab, a daemon socket, or
any change to a monitored host.

---

## What this replaces

Phase 10 was originally "systemd services": a table of unit name, load/active/
sub state, and enabled-ness. That was dropped after measuring the fleet.

- **Zero failed units across all five hosts** — 773 units, 162 running, none
  failed. A failed-unit view would render an empty row permanently.
- **It is alerting-shaped.** Its value is *noticing a failure*, but Tuxtop is
  opened when you want to look. A signal that only fires while a window is
  open is the exact thing [Non-goals](../ROADMAP.md#non-goals) rejects, and
  Kuma and Proxmox are already watching unattended.
- **A browsable 137-row unit table is `ssh host systemctl status` with more
  clicks**, and it is a table of strings — there is no spike in it, which is
  what this app is for.

What survived the measurement is not the unit list. It is **ownership**.

---

## The three parts

### A — every process says what it belongs to

`/proc/[pid]/cgroup` is **15 bytes** and names the thing that owns a process:

```
0::/system.slice/manticore.service
0::/system.slice/docker-4f2b….scope
0::/user.slice/user-1000.slice/session-8240.scope
0::/init.scope
```

One read per *ranked* process — the same shape as the command line addition,
about **300 bytes** for the top twenty. It turns `python 39%` into
`python 39% · transcribe-worker.service`, which is the difference between
seeing a spike and understanding it.

It covers containers incidentally and for free: a containerised process
reports `docker-<id>.scope` with no daemon, no socket and no `docker` group
membership. The friendly container *name* would need the socket, which would
need a root-equivalent group change on a monitored host — declined, see
[ADR-004](../DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host)
and [ADR-010](../DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host).
The ID is shown truncated; on a fleet with one container that is most of the
value at none of the cost.

**Parsing rules**
- cgroup **v2** (`0::/path`) is the case that exists on this fleet. Take the
  last path segment.
- cgroup **v1** emits several `N:controller:/path` lines; use the line whose
  path ends in `.service` or `.scope`, and fall back to the `name=systemd`
  controller.
- `init.scope` and `user.slice/.../session-N.scope` are not services. Render
  the session case as the login it is, not as a unit named `session-8240`.
- Unreadable or absent → **empty**, never a guess. A process can exit between
  being ranked and being read, and kernel threads have no cgroup worth naming.

### B — units that keep restarting

```
systemctl show --property=Id --property=NRestarts "*.service"
```

One call, **108 ms**, and only non-zero counts are worth shipping — on dove
that is two units out of 137:

```
transcribe-app.service   restarts=1
indexer-post.service  restarts=1
```

This is the blind spot the failed-unit view could not see. A service that
keeps restarting is **active and not failed**: invisible to `--state=failed`,
invisible to an endpoint check like Kuma, and invisible in the process list
because the PID simply changes.

**`NRestarts` must not be described as "today".** It counts automatic restarts
since the unit was last started explicitly, which may be months ago or five
minutes ago, and it says nothing about *when*. Reporting "3 restarts" beside a
live CPU chart implies a recency the number does not carry.

So Tuxtop records the value **when it first sees it** and shows the delta:

```
transcribe-app.service    1 restart (unchanged while watching)
some-flapper.service      7 restarts  ·  +4 since Tuxtop started
```

The delta is the actionable half — it means *flapping now* — and it is
honestly ours to compute, because the baseline is a thing we observed rather
than a timestamp we inferred.

### C — what a unit actually costs

Reading `cpu.stat`, `memory.current` and `pids.current` for each child of
`/sys/fs/cgroup/system.slice/` gives per-unit resource use with no daemon and
no privileges. Measured on dove: **45 cgroups, 2,549 bytes, 154 ms**.

```
TXG|manticore.service|630641758|483102720|59
```

**This is the part the process list cannot do.** `ProcInfo::rss_kb` in
`procs.rs` carries the standing warning never to sum RSS across processes — shared pages are counted once per process, so a
column of them does not add up to anything real. "How much memory does
manticore use?" is therefore *unanswerable* from a process list, and answered
exactly by `memory.current`. On dove, manticore is 21 processes; as one row it
is 461 MB and 59 tasks.

Two honesty requirements:

- **CPU is cumulative microseconds** and must be a delta of two samples over a
  known interval, like everything else here — never an instantaneous value.
- **`memory.current` includes page cache**, so it reads higher than the sum of
  RSS for the same processes and is not comparable to the process list's
  memory column. The UI must label it as the cgroup charge, not as "memory
  used", or it invites exactly the wrong comparison.

---

## Rendering — no new tab

All three land on the **Processes** view, which already has the host column,
the filter and the sort.

- **A** is a column: `Owner`, beside `Command`. Filterable, so typing a unit
  name narrows to its processes.
- **C** is a **group-by-owner toggle** on the same view: collapsed, one row per
  unit with cgroup CPU and memory; expanded, its processes. The same shape the
  fleet view already uses for host groups.
- **B** rides on the owner row: a restart count where non-zero, warning-toned
  only when the delta since Tuxtop started is non-zero.

A services tab was the thing worth *not* building. This is the same data,
attached to the question people actually ask.

---

## Cadence

Not the 1 Hz stream. Ownership and cgroup sweeps ride the **process channel**
at its existing 5 s cadence, and only while the Processes view is open —
sampling nobody is looking at should cost nothing, which is already how
`set_processes_enabled` works.

Restart counts are slower still: a unit that restarts is news for hours, so
once a minute is generous. Fetch on view open and on a slow timer.

**Total added cost per host per sample:** ~300 bytes of ownership plus ~2.5 KB
of cgroup accounting, against 7.3 KB/s for the metric stream at 1 Hz.

---

## Exit criteria

- [x] `cargo test` passes, including parser tests
- [x] A test parses all four real cgroup line shapes from this fleet, plus a
      cgroup v1 multi-line sample
- [x] A test pins that an unreadable cgroup yields an empty owner, never a
      guessed one
- [x] A test pins that cgroup CPU is a delta and that identical samples report
      zero rather than NaN
- [x] A test pins that the restart delta is measured from first sight, and that
      a unit first seen at 7 restarts reports `+0`
- [x] The Processes view shows an Owner column, filterable
- [x] Group-by-owner shows `manticore.service` as one row with cgroup memory,
      expanding to its 21 processes
- [x] Verified against dove, whose numbers are quoted throughout

---

## Out of scope

- **A Docker socket integration.** One container across five hosts, on the host
  where reading it would require adding the user to the `docker` group — which
  is root-equivalent. Revisit only if containers become how this fleet runs,
  and prefer cgroup files over the socket even then.
- **Starting, stopping or restarting anything** — [ADR-010](../DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host).
- **`user.slice` accounting.** Session scopes are noise on a server; revisit if
  a host turns out to run real work under a user manager.
- **Unit enabled/disabled state and dependency graphs.** That is configuration,
  not behaviour, and it does not change while you watch.

---

## Built

All three parts shipped. Measured on dove: ownership adds ~550 bytes per
sample, the cgroup sweep 2,558 bytes across 45 cgroups, and the restart sweep
runs every twelfth process cycle. `transcribe-worker.service` reads 395 MB
across 43 processes — the figure a process list structurally cannot produce,
since summing RSS double-counts shared pages.
