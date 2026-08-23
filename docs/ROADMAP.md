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

## Phase 5 — The process list — **done, read-only by decision**

The Task Manager half, and the thing nothing off-the-shelf does from Windows.

Full spec: **[specs/process-list.md](specs/process-list.md)**.

- [x] Fleet-wide list: every host, sorted by CPU then memory, host as a column.
- [x] CPU as a percentage of the whole box, stated on screen.
- [x] Remote ranking - 655 bytes measured against a real 479-process host,
      against 85 KB for shipping `/proc/*/stat` raw.
- [x] Own SSH channel at its own cadence, started only while the view is open.
- [x] Kernel threads flagged and hidden behind a toggle.
- [x] **Full command lines**, as an expandable row in the fleet list rather
      than a separate per-host view — that list already carries the host
      column, the filter and the sort, and a second view would duplicate all
      three. Measured on dove: 635 → 2,083 bytes per sample, about 290 B/s per
      host at the 5 s process cadence against 7.3 KB/s for metrics. Truncated
      remotely at 200 characters, because a multi-kilobyte Java command line
      would otherwise dominate a frame. The filter searches the arguments too:
      six processes all called `Runner.Listener` are only tellable apart by
      theirs.
- [x] **Kill and renice — dropped, 2026-08-23.** Tuxtop stays a pure
      observation tool. See
      [ADR-010](DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host).
- Per-process CPU from `/proc/[pid]/stat` `utime + stime` deltas over
  `sysconf(_SC_CLK_TCK)`. **Do not parse `top`** — its output shifts across
  distros, versions and locales, and a decimal comma will silently break it.

**The decision, taken:** this would have been the first feature to *change* a
remote machine rather than read it. It is not being built. The framing was
never privilege — Tuxtop uses the user's own SSH credentials and grants no
capability a terminal does not — it was **aiming**. A fleet view exists so
nineteen hosts look alike at a glance, which is good for seeing and bad for
targeting, and `kill 1` on the card you thought was `owl` is a mistake this UI
would help you make.

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

## Phase 8 — History plane — **done**

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
- [x] **Cap enforcement.** `History::enforce_cap` sheds the finest tier from
      every series until the store fits, applied at startup and whenever the
      setting changes. **Resolution degrades uniformly; coverage never does** —
      every host keeps history and simply gets a coarser one, because a fleet
      where some cards have charts and others do not is the failure that
      disqualified Beszel as the history plane ([ADR-009](DECISIONS.md#adr-009--we-own-history-beszel-is-optional-enrichment)).
      The last tier is never shed. The settings panel reports **measured**
      usage rather than the projection it used to assert as fact, and says so
      when detail has been dropped.
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
- [x] **All temperature sensors**, not only the CPU. Every hwmon reading
      always crossed the wire; only the presentation discarded them. Kept as a
      named list on the `Sample`, with each sensor classified (cpu / drive /
      wireless / board) because **the number is not actionable without its
      subject** — 72 °C is alarming for a CPU and unremarkable for an NVMe.
      Unlabelled sensors are numbered within their driver: dove's board
      exposes four `gigabyte_wmi` inputs, and naming them all alike would show
      one and hide three.

      Three surfaces: the host card's temperature chip keeps showing the CPU
      (the reading the ranking vouches for) and lists every sensor in its
      tooltip; a new **Hottest sensor** fleet metric that always names the
      component; and one history series per sensor, so an NVMe warming up over
      an hour is finally visible. On dove the hottest sensor is an NVMe at
      71.9 °C while the CPU reads 31.6 °C — a 40-degree spike the app could
      not previously show.

**Done.** Verified against dove: Ryzen 5950X, Debian 13, kernel 6.12, 9d 22h
up, `/` at 8.4% against `df`'s 9%, swap 7.2%, and a user/system/iowait/steal
split. Cost measured at **7.3 KB per frame — identical to before the phase**,
because identity is read once and `df` every thirtieth frame.

---

## Phase 10 — Ownership: what a process belongs to, and what a unit costs — **done**

Full spec: **[specs/ownership.md](specs/ownership.md)**.

**Reframed after measuring the fleet.** This was "systemd services": a table of
unit name, state and enabled-ness. Three findings killed that version:

- **Zero failed units across all five hosts** — 773 units, 162 running, none
  failed. A failed-unit view would render an empty row permanently.
- **It is alerting-shaped**, and Tuxtop is opened when you want to look. A
  signal that only fires while a window is open is what [Non-goals](#non-goals)
  rejects; Kuma and Proxmox already watch unattended.
- A browsable 137-row table is `ssh host systemctl status` with more clicks,
  and a table of strings has no spike in it.

What survived is **ownership**, in three parts, all landing on the existing
Processes view rather than a new tab. Built and verified against dove:

- [x] **A — every process says what it belongs to.** `/proc/[pid]/cgroup` is 15
  bytes and names the owner: `manticore.service`, `docker-<id>.scope`, a login
  session. ~300 bytes for the top twenty. Turns `python 39%` into
  `python 39% · transcribe-worker.service`. Covers containers incidentally,
  with no daemon socket and no `docker` group.
- [x] **B — units that keep restarting.** One `systemctl show` call, 108 ms. A
  flapping service is *active and not failed*: invisible to `--state=failed`,
  to an endpoint check, and to the process list, because the PID just changes.
  `NRestarts` carries no recency, so Tuxtop records it at first sight and shows
  the delta — the half that means "flapping now".
- [x] **C — what a unit actually costs.** Per-cgroup `cpu.stat`, `memory.current`,
  `pids.current`: 45 cgroups, 2,549 bytes, 154 ms on dove, no privileges. This
  is the part a process list *cannot* do — summing RSS is banned because shared
  pages are counted once per process, so "how much memory does manticore use?"
  is only answerable from the cgroup. On dove that is 21 processes as one row.

**Docker gets no tab.** One running container across five hosts, on the one
host where reading it would need the user added to the `docker` group — which
is root-equivalent, and so a change to a monitored host that ADR-004 and
ADR-010 both rule out. Container attribution comes free with A regardless.

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
      the per-host table in Settings, the Add host dialog, or by hand in
      `hosts.toml`.
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

- ~~**Windows hosts.**~~ **Built, 2026-08-23** — see
  [specs/windows-hosts.md](specs/windows-hosts.md). N1 runs on the fleet with
  16 cores and its own 63.8 GB, at 997 bytes per frame, over Windows' own
  first-party OpenSSH. The inverse-counter trap, the localised-counter trap
  and the base64 command are all documented there. Processes and services for
  Windows hosts remain a second pass.

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
