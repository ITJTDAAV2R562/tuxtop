# Roadmap

Phases are committable units. Each states what becomes *observably* true when
it lands — not what code exists. Tick the box only when you have seen it work.

Status: **done** · **next** · **planned** · **idea**

---

## What this is for

**To see spikes across a fleet, immediately and beautifully.** That is the
whole goal. Every feature is judged against it.

It is a *seeing* tool, not a *watching* tool. It does not need to run
unattended, remember anything across restarts, or tell anyone when something
breaks — see [Non-goals](#non-goals).

---

## Phase 0 — Core sampling maths — **done**

The `/proc` parsing and rate maths, with no GUI dependency.

- [x] `/proc/stat` parsing, aggregate + per-core
- [x] Delta maths: `iowait` as idle, no guest double-count, backwards-counter clamp
- [x] `/proc/meminfo` via `MemAvailable`
- [x] `/proc/net/dev` excluding loopback and virtual interfaces
- [x] `/proc/diskstats` whole disks only, no partition double-count
- [x] Frame delimiting so a partial `/proc/stat` never parses
- [x] `RateTracker` dividing by real elapsed time
- [x] 38 tests pass, including real 32-core fixtures cross-checked against `top`

---

## Phase 1 — SSH transport — **done**

One persistent connection per host, streaming frames.

- [x] `transport.rs` spawns the system `ssh` (ADR-007, superseding `russh`),
      one long-lived process per host, streaming framed `/proc` output
- [x] `~/.ssh/config` aliases, `ProxyJump` and agent auth all work, because it
      is the same client the user's terminal uses
- [x] ssh's stderr classified into typed `HostFault`s — auth vs unreachable vs
      sampler failure — never a generic "offline"
- [x] `tuxtop-watch` CLI renders a live core grid in the terminal
- [x] **Verified against dove:** 8 busy cores of 32 read as 25.0–25.2% against
      a true 25.0%, detected in one sample and recovered in one sample. The
      same load took the Beszel agent 26 s to notice and 13 s to forget.
      See [evidence](evidence/beszel-cadence.md#follow-up-the-same-test-against-tuxtops-own-sampler).

Reconnect with backoff landed with the supervisor in Phase 2.

---

## Phase 2 — Tauri shell with a real Mica backdrop — **done**

- [x] Tauri 2 wired to `tuxtop-core`; `src-tauri` is its own workspace root
- [x] `window-vibrancy` Mica, applied with failure logged and non-fatal
- [x] Frontend runs the mockup's HTML/CSS on real events, with the simulator
      kept as a fallback so the page still opens standalone as a browser mockup
- [x] `hosts.toml` in the OS config dir; add and remove from the UI
- [x] Faults render on the card with the reason and a suggested fix
- [x] Verified on Windows: window opens with Mica, core grid animates from
      live dove data

**Three silent-failure bugs cost most of this phase.** All presented the same
way — the window renders and nothing responds. Recorded in
[ARCHITECTURE.md](ARCHITECTURE.md#tauri-pitfalls-that-fail-silently) so the
next session recognises the shape rather than re-deriving it.

---

## Phase 3 — Multi-host — **done**

Landed with Phase 2 rather than as its own phase:

- [x] `hosts.toml` add/remove, one Tokio task per host
- [x] Reconnect with capped backoff; a good sample resets it
- [x] Faults render as a stated reason on the card
- [x] Hosts that have not reported show as connecting, not up
- [x] **Verified against five real hosts** — 108 cores, 1 Hz, three connections
      killed mid-flight. Each was detected, attributed to the right host, and
      recovered in 1.3–2.9 s; no other host dropped a frame. The apparent
      stalls in the log are sampling-phase jitter, and the check that
      distinguishes them from causation is part of the record.
      See [evidence](evidence/host-isolation.md).

The isolation loop moved from `src-tauri/supervisor.rs` into
`tuxtop-core::fleet` to make this possible. It had been untestable by
construction: the crate it lived in cannot be built on the development box, so
the most consequential control flow in the app was the only part with no tests.

## Phase 4 — Beszel as optional enrichment — **closed, nothing to build**

Written when Beszel was "the slow plane" and owned history. Phase 8 changed
that: our own store covers **every** host at full resolution, including the
ones with no agent, so Beszel is not load-bearing and never was integrated.

[ADR-009](DECISIONS.md#adr-009--we-own-history-beszel-is-optional-enrichment)
supersedes ADR-002 and records the reasoning. The deciding asymmetry: our store
covers every host because the live grid already feeds it, while Beszel covers
only hosts running its agent — one of five on this fleet. A history view that
silently covers part of a fleet is worse than none.

What would still be worth having, if anyone ever wants it:

- History beyond our seven-day ceiling, from Beszel's own records, for hosts
  that happen to run an agent.
- Nothing else. Container stats and SMART would be better collected directly
  than read second-hand through a hub that may not be installed.

**Closed** rather than deferred: there is no work here until someone wants
history older than a week badly enough to accept it being missing on most
hosts.

---

## Phase 5 — The process list — **built, read-only**

The Task Manager half, and the thing nothing off-the-shelf does from Windows.

Full spec: **[specs/process-list.md](specs/process-list.md)**.

- [x] Fleet-wide list: every host, sorted by CPU then memory, host as a column.
- [x] CPU as a percentage of the whole box, stated on screen.
- [x] Remote ranking - 655 bytes measured against a real 479-process host,
      against 85 KB for shipping `/proc/*/stat` raw.
- [x] Own SSH channel at its own cadence, started only while the view is open.
- [x] Kernel threads flagged and hidden behind a toggle.
- [ ] **Per-host drill-down** with full command lines.
- [ ] **Kill and renice** - see the spec on why the framing is mistakes, not
      privilege.
- Per-process CPU from `/proc/[pid]/stat` `utime + stime` deltas over
  `sysconf(_SC_CLK_TCK)`. **Do not parse `top`** — its output shifts across
  distros, versions and locales, and a decimal comma will silently break it.
- Kill and renice, behind a confirmation.

**The decision to take first:** this is the first feature that *changes* a
remote machine rather than reading it. Everything so far has been `cat`
against `/proc` — safe by construction, and the reason "nothing is installed
on the host" has held. Killing a process is not that, and needs an explicit
answer on privilege: own processes only, sudo when available, or a per-host
setting. Take it once here; Phase 10's start/stop is the same question.

**Open design question:** rows shaded by load need a contrast halo on their
numerals — see [ADR-005](DECISIONS.md#adr-005--load-is-encoded-three-ways-at-once).

---

## Phase 6 — GPU and temperatures — **done**

- [x] **Temperatures.** `/sys/class/hwmon` read in the sampler loop, emitted as
      pipe-delimited `TXT|driver|label|millidegrees` lines. Only known CPU
      drivers are considered, ranked — an NVMe under load is routinely hotter
      than the CPU, so "hottest sensor wins" names the wrong component with
      total confidence. Verified against a real host: reports 31C where `Tctl`
      reads 31C. A host with no sensor yields `None`, never a zero.
- [x] **GPU.** `nvidia-smi` appended to the same loop, guarded by `command -v`
      so a host without the driver contributes nothing and costs no error.
      Verified against an RTX 3080: reports 0%, 1969 / 10240 MiB, 18W matching
      nvidia-smi exactly. A malformed utilisation field discards the reading
      rather than defaulting to zero, which would be indistinguishable from an
      idle card.

Absence is normal for both, not an error.

---

## Phase 7 — Configurable sample interval, with a live traffic meter — **done**

**Goal:** the interval stops being hardcoded at 1 Hz, and the app shows what
its own sampling costs.

- [x] A global interval, persisted in `hosts.toml` beside the host list.
- [x] A per-host override — 1 Hz on the box being watched, 10 s on the twelve
      that only need to be noticed going down. `interval_secs: Option<u32>` on
      the host, falling back to the global.
- [x] Changing it restarts that host's sampler and leaves the others streaming.
- [x] The history cap setting, which Phase 8 depends on.
- [x] **Measured, not estimated.** `SshSampler` already reads every byte off
      the pipe, so `TrafficCounter` counts them: bytes per host and last frame
      size. The settings panel shows current throughput for the hosts actually
      configured, plus projections at other intervals.

Frame size is effectively constant for a given host — it tracks disk and
interface count, not load — so at interval *I* the rate is exactly
`frame_bytes / I`, and the projection is arithmetic rather than guesswork.

**Design note.** A monitoring tool that has never measured itself is in a poor
position to lecture anyone. This is the first number the app reports about its
own behaviour rather than someone else's, and it is held to the same standard
as the rest: measured, attributed per host, never rounded into a reassuring
shape.

Superseded [evidence/sampling-cost.md](evidence/sampling-cost.md), which was
extrapolated by hand from three hosts.

---

## Phase 8 — History plane — **built; cap enforcement outstanding**

**Goal:** metrics over time, not only in the moment. Charts over a window, and
retention that survives a restart.

Full spec: **[specs/history-plane.md](specs/history-plane.md)**.

- [x] Four-tier cascade in `crates/tuxtop-core/src/history.rs`, memory only.
- [x] Gaps written explicitly, so a silent host leaves a hole rather than a
      straight line implying it was fine. **Only half true until 11c**: the
      store recorded the gap, but the query dropped it and the chart joined
      the two ends — drawing exactly the straight line the storage comment
      promised it prevented. `drawHistory` now splits the series into runs on
      an outsized time delta and shades the hole.
- [x] Stored in the Rust backend, queried with a window and a point budget.
- [x] History view with min/max bands, a shared window, and a slider spanning
      a minute to a week continuously.
- [x] Contextual entry: from a host card or a fleet block, that host and its
      metrics; from the Fleet view, that metric across every host.
- [x] **Per-core charts** — the Task Manager small-multiples shape, one chart
      per core at a fixed size, fetched for the whole host in a single call.
- [x] **Subject picker** — change host or metric without leaving History.
- [ ] **Cap enforcement.** The cap is configurable and displayed but nothing
      evicts on it yet. At ~24 MB for a 19-host fleet nothing has come close;
      it matters around 100 hosts.
- [x] **Superseding ADR for Beszel.** [ADR-009](DECISIONS.md#adr-009--we-own-history-beszel-is-optional-enrichment)
      supersedes ADR-002; Phase 4 is closed.

Settled:

- **Our own store, memory only.** A restart starts clean, like Task Manager.
  History is low-value data; losing it costs nothing, which removes
  persistence, durability and migration from the design entirely.
- **Four-tier cascade** — 1 Hz/1 h, 10 s/6 h, 60 s/24 h, 5 min/7 days. 23.4 MB
  for the whole fleet, bounded by construction at 79.9 KB per series.
- **Coarse tiers keep min/mean/max**, never just the mean. A 60 s bucket
  averaging a 100% spike down to 7% is the exact failure this project exists
  to prevent — and the min/max band is where the translucent fill goes.
- **Stored in Rust**, queried with a window and a point budget, so continuous
  zoom crosses tiers invisibly and needs no preset buttons.
- **History inherits its slice** from wherever it was entered: from a host,
  one host and many metrics; from the fleet, one metric and many hosts.
- **Beszel drops to optional enrichment** beyond our seven-day ceiling,
  superseding its role as the slow plane in ADR-002. See ADR-009.



---

## Phase 9 — Host facts and the data already on the floor — **done**

Cheap wins, several of which are already parsed and thrown away. Grounded in
what Beszel actually stores, checked against its schema.

- [x] **Filesystem usage.** The largest real gap. Beszel stores disk total,
      used and percent; we collect disk *I/O* and no capacity at all — so the
      single most common way a Linux box falls over is invisible here. From
      `/proc/mounts` plus `statvfs`, per mount, excluding pseudo-filesystems.
- [x] **Host identity** — CPU model, distro, kernel. Task Manager names the
      processor at the top of its CPU pane, and a fleet view of 19 boxes badly
      wants to know which are which. One `uname -srm`, `/etc/os-release` and
      `/proc/cpuinfo` read, cached per connection rather than per sample —
      none of it changes between frames.
- [x] **Uptime.** From `/proc/uptime`. Beszel stores it; we do not.
- [x] **Swap.** `MemInfo` already parses `SwapTotal` and `SwapFree`; `Sample`
      simply never carried them.
- [x] **CPU breakdown.** We compute busy% from user/system/iowait and then
      discard the split. Showing iowait separately is genuinely diagnostic:
      "the CPU is not busy, it is waiting on disk" is a different problem with
      a different fix.
- [ ] **All temperature sensors**, not only the CPU. We rank sensors and keep
      one; dove also reports NVMe, chipset and GPU. Left undone: the ranking
      exists and works, and a second sensor list is presentation rather than
      collection.

**Done.** Verified against dove: Ryzen 5950X, Debian 13, kernel 6.12, 9d 22h
up, `/` at 8.4% against `df`'s 9%, swap 7.2%, and a user/system/iowait/steal
split. Cost measured at **7.3 KB per frame — identical to before the phase**,
because identity is read once and `df` every thirtieth frame.

---

## Phase 10 — systemd services — **planned**

Both reference apps have this: Task Manager has a Services tab, and Beszel
tracks 70 units on dove. Pairs naturally with the process list — a failed unit
and a runaway process are the same question asked twice.

- Unit name, load/active/sub state, and whether it is enabled.
- Failed units surfaced without being hunted for.
- Read-only first. Start and stop are the same trust decision as kill, and
  should be taken once, in Phase 5, rather than twice.

---

## Phase 11 — Grouping hosts into clusters — **done**

Group hosts by role, site or cluster and aggregate per group, so a fleet of
nineteen reads as five things.

Full spec: **[specs/host-groups.md](specs/host-groups.md)**. The open questions
listed here previously are answered there; the short version:

- **A group is one optional label per host.** Not multi-label, not a tree —
  both are widenings a single label stays compatible with, and neither earns
  its complexity yet.
- **Percentages aggregate by recombining their parts, never by averaging the
  ratio.** dove at 100% of 32 cores and heron at 0% of 4 is 88.9%, not 50%.
  See [ADR-008](DECISIONS.md#adr-008--aggregates-must-not-be-able-to-hide-a-member).
- **Severity is max, magnitude is aggregate.** A group averaging 40% that
  contains a host at 97% renders red.
- **Every group shows its spread**, so a tight group and one tearing itself
  apart are distinguishable without expanding it.
- **History aggregates on read**, and marks any span where a member was silent
  rather than quietly summarising fewer hosts than it claims.

This is the first feature that shows a number *Tuxtop computed* rather than one
a machine reported, which is a different risk class from everything built so
far — hence a spec before code, as Phase 8 had.

- [x] **11a — aggregation core.** `src/agg.js`, 14 tests under `node --test`,
      every metric in the registry declaring its rule, and
      `scripts/check-agg-declared.py` failing the commit if one does not.
      Both ADR-008 rules verified by mutation, not just by passing.
- [x] **11b — group blocks in the UI.** Collapsible in both fleet shapes:
      scalar metrics get a group row with a member-range whisker, vector
      metrics get one block holding every member's cores with each tile
      attributed to its host. Severity from the worst member, composition and
      partial reporting stated. `group` is a field on `HostConfig`, set from
      the Add host dialog or by hand in `hosts.toml`.
      Testing revised two spec decisions — see the notes marked *revised
      during 11b* in the spec, and ADR-008's consequences.
- [x] **11c — group history**, aggregated on read from the members' own
      series, so a group cannot drift from what it summarises and re-labelling
      a host re-labels its past. Series are aligned by timestamp, never by
      index: a gap is skipped rather than returned, so a host that went quiet
      simply has fewer points. Each aggregated point carries how many members
      contributed; incomplete spans are shaded and the header states what
      fraction of the window was short.

      Found and fixed a pre-existing bug on the way: charts drew straight
      lines across outages. See the Phase 8 note above.

It lives in JS rather than Rust because the frontend must also run standalone
as a browser mockup with no Tauri backend, so the rules would otherwise need
two implementations — and two implementations of ADR-008 is one more than can
be kept honest.

---

## Non-goals

**Alerting.** Deliberately out of scope, not merely unbuilt.

Tuxtop is a desktop app you close. An alerting system that only fires while a
window is open is worse than none, because you would come to rely on it. That
is a job for something that runs unattended — Uptime Kuma, Pulse, Proxmox — and
those already exist here. Beszel keeps its alerts for the same reason.

**Persistence.** History is memory only and clears on restart, by design. See
[specs/history-plane.md](specs/history-plane.md).

**Multi-user, auth, tokens.** This is a single-user desktop application.

---

## Landed outside the phase list

Work that arrived from design conversation rather than the plan:

- **Metric registry** — host view and fleet view as the two slices of a
  hosts x metrics matrix. Adding a metric is a table entry, not a renderer.
  See [ARCHITECTURE.md](ARCHITECTURE.md#two-views-one-matrix).
- **Fleet view** — one metric across every host, with log scaling over a
  decade window for rates and absolute for percentages.
- **Drag to reorder**, persisted in `hosts.toml`; sorting by name or by the
  metric on screen.
- **Block packing** — blocks sized to core count and packed, so a fleet of 19
  fits one screen instead of scrolling.
- **Metal surfaces and Fluent reveal highlight.**
- **Theme-token checker** (`scripts/check-theme-tokens.py`), after the same
  missing-token bug landed twice.

---

## Ideas — not committed

- **Windows hosts.** SSH to Windows works and OpenSSH ships with it, but there
  is no `/proc`, so it needs a second sampler shape — PowerShell performance
  counters or CIM — behind the same `Sample`. Lean version: detect the OS once
  at connect (host facts already do this) and pick a command set. Until then
  a Windows box can be watched through its WSL instance, which is how `owl` is
  reachable today, and which reports the Windows kernel's view of CPU and
  memory anyway.
- **Browser access, not only the desktop app.** The frontend is already static
  HTML/CSS/JS against a small command surface, and the backend is already Rust
  holding all the state. Serving the same commands over HTTP instead of Tauri
  IPC would make the whole UI reachable from a browser with no second
  implementation. The work is an HTTP layer beside `invoke`, not a rewrite -
  but it turns a single-user desktop app into something listening on a port,
  which is a different security posture and should be a deliberate decision.

- **Per-core sparklines** instead of single-value tiles at large card sizes.
- **More metrics** now that the registry exists: temperatures, per-filesystem
  usage, per-NIC and per-disk vectors. Each is a table entry, not a renderer.
- **Card size scaled to core count** — a 32-core box and a 4-core box currently
  get identical footprints. Open question from the mockup review.
- **Detail as a separate window** rather than inline accordion, so two hosts
  can be compared side by side.
- **Sub-second sampling** for a "performance" mode. `/proc/stat` at 250 ms is
  cheap; the limit is SSH round-trip, not the kernel.
- **systemd unit view** — reuse the `systemd_services` shape Beszel already
  collects.
- **Linux and macOS builds.** Tauri is cross-platform; only `window-vibrancy`
  is Windows-specific. Not a goal, but nothing blocks it.
