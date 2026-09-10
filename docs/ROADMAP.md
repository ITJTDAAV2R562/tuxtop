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
- [x] Its own cadence on the host's **one** SSH connection — it had a second
      per host, started fleet-wide on view open, at ~10 MB of client RSS each
      ([ADR-014](DECISIONS.md#adr-014--one-connection-per-host-carries-both-planes)).
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

## Phase 12 - Heat: the fleet over time - **done**

One row per host, time across, colour by load. Neither existing view can show
this: the live grid is every host at one instant, History is a window but only
a few subjects. Here the whole window and the whole fleet are on screen at
once, which is what turns "coot spiked twenty minutes ago" from something you
go looking for into something you notice.

- **A cell is the peak of its bucket, never the mean.**
  [ADR-011](DECISIONS.md#adr-011--a-heatmap-cell-shows-the-buckets-max-not-its-mean).
  At 1 Hz over a day one cell covers 144 samples; a host pinned at 100% for
  twenty seconds inside one has a mean of 14%, which is the same arithmetic
  that made the Beszel agent report 0.14% during real load. Verified by
  mutation: colouring by mean fails `a_cell_shows_the_bucket_max_not_its_mean`.
- **Every host keeps its own row.** This is the one view with room for all
  nineteen, so nothing is aggregated and ADR-008 has nothing to hide behind -
  groups are headings, not summaries.
- **Columns are bounded by the sample rate, not by pixels.** Drawing a 60 s
  window as 1200 pixel-wide cells invents 1140 empty ones and then reports
  them as "95% gap" - a missing-data warning manufactured entirely by the
  chart's own resolution. Wide windows fall back to three pixels per cell.
- **Only a real deficit is called a gap.** A column is one second and samples
  arrive about once a second, so ordinary jitter empties 2-10% of columns on a
  healthy host. Flagging that made every row shout. Below 90% is a host
  genuinely delivering less than asked: against the real fleet, towhee at 31
  samples of 60 stood out while its neighbours at 58-60 stayed quiet.
- **One query for the fleet.** `query_history_fleet` is the mirror of
  `query_history_many` and exists for the same reason - the slider redraws on
  every drag, and nineteen round trips per redraw is the cost it avoids. Hosts
  with no history come back as empty vectors rather than missing keys, so
  "not reporting" stays distinguishable from "not configured": N1 renders as
  **no data**, which is how its WSL-vs-Windows misconfiguration is visible at
  a glance.
- **Clicking a row opens that host in History**, because a cell that catches
  the eye is a question about one host.

Verified against the real nineteen-host fleet through `tuxtop-serve`, in both
themes.

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
- **Pause a host** for planned maintenance, 2026-09-01. Stops sampling without
  removing the host, keeping the history, group, interval and grid position
  that removal discards. Enforced in one place — `Supervisor::start` refuses a
  paused host, so no other code path can resume one as a side effect of an
  unrelated edit. The card blanks its readings rather than freezing them, and
  the tally counts paused apart from up. See
  [ADR-012](DECISIONS.md#adr-012--pause-is-a-third-host-state-and-it-lives-in-hoststoml).

---

## Ideas — not committed

- ~~**Windows hosts.**~~ **Built, 2026-08-23** — see
  [specs/windows-hosts.md](specs/windows-hosts.md). N1 runs on the fleet with
  16 cores and its own 63.8 GB, at 997 bytes per frame, over Windows' own
  first-party OpenSSH. The inverse-counter trap, the localised-counter trap
  and the base64 command are all documented there. Processes landed too, with
  service ownership in the same column Linux uses for systemd units - and
  no services view, for the same reason there is no systemd one.

- ~~**Browser access, not only the desktop app.**~~ **Built, 2026-08-24** -
  `tuxtop-serve`. It was an HTTP layer beside `invoke` rather than a rewrite,
  as predicted: `src/http.js` installs a `__TAURI__` implementation backed by
  fetch and EventSource, and only when nothing else has, so the frontend needed
  no changes at all. The security posture was taken as the deliberate decision
  it deserved - no authentication of its own, fronted by a proxy that does TLS
  and identity (`tailscale serve` here, but an `ssh -L` tunnel, nginx, Caddy or a
  VPN equally); read-only unless `--writable`, because `add_host` makes the
  serving machine open SSH with its own keys. It bound loopback only until
  Phase 14 added `--bind`, which changes where it listens and nothing about what
  it authenticates. See CLAUDE.md, "Two shells,
  one service".

- ~~**Distribution.**~~ **Shipped, 2026-08-25** - `v0.2.0`: a Windows `.msi`
  and NSIS installer, a Linux `tuxtop-serve` tarball, and `SHA256SUMS`, built
  from the tagged commit after re-running the full CI gate on it. Verified the
  way a stranger would: downloaded the published assets, checked all three
  sums, extracted the tarball into an empty directory and served from it.

  GitHub Actions was unavailable on this account for billing, which is why it
  sat written-but-unrun for a day. **Self-hosted runners are not billed**, so
  moving CI and the release onto our own hardware removed the constraint, and
  32 cores beat a hosted runner's 2.

  **Superseded when the repo was made public.** That arrangement was safe under
  one condition written down at the time — a private repo with no forks — and
  publishing fired it: a self-hosted runner executes whatever a pull request
  contains. Everything is back on GitHub-hosted runners, which a public repo
  gets at no charge, so the billing constraint that caused all of this no
  longer applies either. See [CI.md](CI.md).

  Nothing is code-signed, so SmartScreen warns on first run and the notes say
  so. `v0.1.0` remains the old `tuxtop-watch` CLI release; the number was not
  reused.

- **Per-core sparklines** instead of single-value tiles at large card sizes.
- **More metrics.** ~~Per-filesystem usage~~ **built, 2026-08-25**: the Fleet
  view expands a host into one row per filesystem, fullest first, behind the
  same disclosure a group uses. The card's own reading is unchanged - the
  fullest mount is the right single number, and everything else in the app
  reads that scalar - so this is a `rows` accessor on the existing `fs` metric
  rather than a second metric.

  The roadmap used to claim each of these was "a table entry, not a renderer".
  That is false for anything not core-shaped: the vector path is written
  around cores throughout (`|| h.cores`, `"${name} core ${i}"`, "N cores"
  headers), so a labelled per-host list needed its own render path.

  Still open, and both need backend work rather than a registry entry: **per-NIC
  and per-disk vectors**. `Sample` carries only `net_rx_bps` and
  `disk_read_bps` aggregates, so per-device rates mean parsing per device and
  holding per-device state in `RateTracker`.
- **Card size scaled to core count** — a 32-core box and a 4-core box currently
  get identical footprints. Open question from the mockup review.
- **Detail as a separate window** rather than inline accordion, so two hosts
  can be compared side by side.
- ~~**Sub-second sampling.**~~ **Built, 2026-08-25** - 4 Hz and 2 Hz, default
  off, per host or globally. What unblocked it was compression: 4 Hz across
  the whole fleet now costs less than half what 1 Hz cost before `ssh -C`.
  Two things had to change beyond the number. The expensive extras keep their
  wall-clock cadence rather than a frame count, because `nvidia-smi` is a
  process spawn and running it four times a second is real load on a machine
  we only watch; and the process ranking has a 1 Hz floor for the same reason.
  The interval moved from seconds to milliseconds throughout, with a migration
  so an existing `interval_secs` is not silently reset.
- **systemd unit view** — reuse the `systemd_services` shape Beszel already
  collects.
- **Linux and macOS builds.** Tauri is cross-platform; only `window-vibrancy`
  is Windows-specific. Not a goal, but nothing blocks it.

---

## Phase 13 — Update notices - **done**

**Goal:** the app tells you a newer release exists, and installs nothing until
you say so.

Twelve phases shipped with no way to learn about a new build. The installers
are unsigned and downloaded by hand, so no OS mechanism was ever going to
mention one — in practice a machine ran whatever was installed on it, for as
long as nobody thought to look.

The shape is settled in
[ADR-015](DECISIONS.md#adr-015--the-app-asks-github-about-updates-and-installs-nothing-on-its-own):
one check per launch, a dismissable notice, and a download that starts on a
button press and not before. Dismissal is per version, so it goes quiet about
the release you dismissed and speaks up about the next one.

What this cost, and what was learned:

- **It is the first outbound connection the app makes on its own.** Everything
  else talks to a host named in `hosts.toml`. That is why it is a setting -
  `update_check`, default on, false for an isolated fleet - rather than
  unconditional behaviour.
- **A failed check is silent in the UI.** Settings reports the outcome instead.
  Without somewhere to read it, a permanently broken check looks exactly like a
  fleet that is always current.
- **`set_settings` rebuilds `Settings` field by field so it can clamp**, and
  the frontend's save handler rebuilt it too. Both would have dropped the new
  field and had serde hand back its default - turning the check *back on* for
  anyone who turned it off. Fixed in both places, and
  `turning_the_update_check_off_survives_a_save` fails if either regresses.
- **The signing key is minisign, not Authenticode.** Different mechanism, same
  word; code signing stays declined. Losing the minisign key is unrecoverable.

### Not done, deliberately

`tuxtop-serve` gets no updater. It is a tarball on a Linux box.

---

## Phase 14 — Remote mode: one sampler, many viewers — **steps 1–3 of 4 done**

Decided in
[ADR-017](DECISIONS.md#adr-017--one-sampler-many-viewers-the-endpoint-is-the-mode),
which holds the reasoning, the four rules the implementation must not break,
and the three non-goals. This is the sequence.

Running Tuxtop on several boxes today duplicates the config, the keys, and —
the part that matters — the sampling: each instance is another nineteen sshd
sessions and nineteen shell loops on machines we promised only to observe. The
fix is to fan in rather than out.

1. ~~**`--bind ADDR` on `tuxtop-serve`**, default `127.0.0.1`.~~ **Done,
   2026-09-05.** IP only — a hostname is refused rather than resolved at bind
   time — no shorthand for the wildcard, and `0.0.0.0` or `::` together with
   `--writable` is refused by `parse_args`, which is the only place the rule
   lives. A *named* non-loopback address with `--writable` gets a loud startup
   line instead, per ADR-017; the startup line now states who can reach the
   server rather than only what it refuses.

   The extraction came first, as planned: parsing was inline in `async fn
   main()` and `main.rs` held zero tests, so neither rule could be asserted at
   all. `parse_args` is pure and returns `Parsed`/`ArgError`.
   `the_wildcard_with_writable_is_refused` asserts the error **variant**, not
   merely `is_err` — "returns some error" is also what a typo elsewhere in the
   line does, which is the same way the first traversal test passed with the
   guard removed. It further asserts that neither `--bind 0.0.0.0` alone nor
   `--writable` alone is refused, so the combination is demonstrably the only
   reason. Verified by hand-mutation: deleting the guard, widening it to all
   non-loopback, and defaulting `bind` to the wildcard each fail the test named
   for the rule they break.

   The doc sweep landed in the same commit: "binds to 127.0.0.1 only" was
   asserted in `README.md`, `SECURITY.md`, `CLAUDE.md`, this file and
   `main.rs` twice, and all six became false the same day.

2. **Remote mode in the desktop app, read-only.** `server` in `[settings]`;
   absent means sample locally. Native window, no local sampling, no local
   keys. The mode and the age of the data are visible in the chrome, not in
   Settings — a window that looks identical in both modes while showing stale
   remote readings is this project's founding bug with a new coat.

   The shape is settled in
   [ADR-018](DECISIONS.md#adr-018--the-desktop-viewer-speaks-plain-http-at-the-event-seam):
   the swap happens at the `supervisor::Event` seam, the client is hand-rolled
   plain HTTP that refuses `https://`, and it splits — parser in core where it
   is tested, socket in `src-tauri` where nothing ever compiles it here.

   **Re-specced 2026-09-08, before a line of it was written.** The first
   version listed the files and named eight tests and had no answer for the
   *command* plane at all. It moved the event stream and left `list_hosts`,
   `get_settings`, `process_list`, `cgroup_list` and `traffic_stats` answering
   out of the local service — which in remote mode is the fleet you switched
   away from, or nothing. That is not a missing feature. It is nineteen cards
   captioned with a different machine's configuration, a Processes view that
   is empty rather than absent, and a settings dialog quoting an interval
   nothing is sampling at. Worse, `set_host_paused` is drawn beside those
   cards, appears to succeed, and edits a `dove` that is not the `dove` on
   screen — ADR-010's aiming argument, arriving through a door nobody had
   opened yet. What follows replaces the file list.

   ### The three command classes

   Every command falls in exactly one. Which one is a decision rather than an
   implementation detail, so it is recorded as ADR-018 decision 4.

   | class | commands | in remote mode |
   | --- | --- | --- |
   | events | the SSE stream | from the server — this is the read loop |
   | fleet reads | `list_hosts`, `get_settings`, `capabilities`, `process_list`, `cgroup_list`, `traffic_stats` | **proxied** |
   | history reads | `query_history`, `query_history_many`, `query_history_fleet`, `history_usage` | answered **locally**, deliberately |
   | writes | `add_host`, `remove_host`, `reorder_hosts`, `set_host_*`, the fleet half of `set_settings` | **refused**, with a reason |

   **History is not proxied, and that is a decision rather than a shortcut.**
   ADR-017 rule 2 says history is in-memory per instance and discarded on a
   switch. Pulling the server's history would make that rule meaningless and
   would blend two fleets' `db1` the moment step 3 lands. So the read loop
   records every arriving `Sample` into the local `HistoryStore`, exactly as
   `Supervisor` does when sampling, and a remote viewer's charts honestly mean
   *what this window has seen since it connected*.
   `remote_samples_are_recorded_in_the_local_history_store`.

   **Writes are refused in the service, not merely hidden in the frontend.**
   `capabilities.writable` is false in remote mode so the controls are not
   drawn — but a hidden control is a fact about a stylesheet, and the command
   behind it stays reachable. The refusal lives in `Service`, one choke point,
   for the reason ADR-012 gives about five callers of which one forgets.
   `a_remote_viewer_refuses_to_write_to_its_local_hosts_toml`.

   ### Why `Settings` splits, beyond tidiness

   `always_on_top` is a property of **this window**. A remote viewer that
   could not be pinned, because pinning is a "setting" and settings belong to
   the server, is absurd — and it is exactly what one undivided `Settings`
   struct forces. The split is therefore load-bearing rather than cosmetic: in
   remote mode the **fleet** half (`interval_ms`, `history_cap_mb`) is the
   server's and is refused, while the **viewer** half (`server`,
   `always_on_top`, `update_check`) is this machine's and still saves.
   `pinning_the_window_still_works_when_the_fleet_is_someone_elses`.

   **One `[settings]` table on disk, via `#[serde(flatten)]`,** so an existing
   `hosts.toml` loads unchanged. Measured rather than assumed (2026-09-08):
   flatten round-trips through `toml` 0.8 in both directions, and a per-field
   `#[serde(default = "…")]` still applies to a key missing from a table that
   is present. A table absent *entirely* falls to `HostsFile`'s own
   `#[serde(default)]` instead, so `Settings` keeps its hand-written
   `impl Default` — a derived one would read every pre-settings file as
   `interval_ms = 0`.

   **The save hazard is confirmed, not hypothetical.** `app.js:3032` builds
   the `set_settings` payload from four named fields and does not carry
   `server`; the `#s-ontop` handler at `app.js:3010` spreads the existing
   object and would. Two save paths, one of which drops the endpoint — the
   Phase 13 shape, already present. So `set_settings` takes `server` from disk
   and never from the request: switching endpoints is step 3's `use_endpoint`
   and has no second door.
   `viewer_settings_survive_a_fleet_settings_save`.

   **The mode is derived from whether `server` is set, never stored** — a
   stored `mode` can contradict the URL beside it.
   `remote_mode_is_derived_not_stored`.

   **Assert the state after the call, not before it.** `start_all` must start
   nothing in remote mode, and `Service::start_all` was once replaceable with
   `Ok(Default::default())` while a test named for launch still passed,
   because `add_host` had already started the hosts and the test asserted
   something true before the call. Use a fresh supervisor.
   `start_all_starts_nothing_when_an_endpoint_is_set`.

   ### The wire, captured off the socket

   Taken from a running `tuxtop-serve` on 2026-09-08 rather than reasoned
   about, because every assumption below was wrong in at least one way:

   ```text
   HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n
   cache-control: no-cache\r\ntransfer-encoding: chunked\r\n\r\n
   97\r\ndata: {"event":"tuxtop://fault","payload":{…}}\n\n\r\n
   3\r\n:\n\n\r\n
   ```

   Three things follow.

   **It is chunked.** The desktop client cannot read `data:` lines off a TCP
   stream; it has to de-chunk first, and a chunk boundary lands mid-frame by
   construction. A de-chunker is a parser, so it lives in core beside
   `split_sse_frames` — ADR-018 decision 3's reason exactly: a parser
   `src-tauri` owns is a parser no test here ever runs.

   **A quiet fleet sends `:\n\n`.** That is axum's keep-alive: a complete SSE
   frame carrying a comment and no `data:` line, on every idle connection, as
   the *normal* case rather than an edge one. Decoding it as a truncated event
   is the founding bug wearing a new transport.
   `a_keepalive_comment_is_not_an_event`.

   **One event happened to be one chunk here, and nothing guarantees it.**
   `split_sse_frames` inherits the rule its name points at — mirror
   `split_frames`, which is in `sampler.rs` and re-exported from `lib.rs`, not
   in `transport.rs` where this line said to look until 2026-09-09: complete
   frames out, tail buffered, never a partial frame handed to the parser.
   `split_sse_frames_returns_only_complete_frames`.

   The bytes above become a fixture, the way `real_host.rs` holds a captured
   `/proc/stat`: the parser is tested against a response a server actually
   sent, fed one byte at a time, not against one we wrote to match the parser.

   ### Freshness

   **Measured at arrival and judged against the server's own interval.**
   `Sample` carries no timestamp (`model.rs`) and this step adds none —
   arrival is honest here because the stream never replays. But the threshold
   cannot be a constant: a server sampling at 5 s reads as permanently stale
   against a 1 Hz expectation. The **rule** lives in core, and the number it
   yields travels in `capabilities` as `stale_after_ms`, so the browser — which
   has no core — carries no second copy of it to drift.
   `freshness_is_measured_against_the_servers_interval_not_a_constant`.

   **Losing the server must not blank the grid.** This is the one failure
   local mode has never had: a single dead link takes out all nineteen cards
   at once. Keep the last grid, mark it stale, and name the **endpoint** as the
   subject rather than the hosts. Nineteen cards each saying "offline" reads as
   a dead fleet rather than a dead link, which is the generic-offline failure
   the hard rules already forbid.
   `losing_the_server_does_not_blank_the_grid`.

   ~~and say *no contact with `<endpoint>` since HH:MM:SS*~~ — **corrected
   2026-09-10 to *no readings from `<endpoint>` since HH:MM:SS*.** "No contact"
   is a claim about the link, and **neither viewer can observe the link.** A
   server's SSE keep-alive is a comment; a comment dispatches no event, so the
   desktop read loop decodes it as `Keepalive` and `EventSource` in a browser
   drops it silently. A healthy server whose whole fleet is paused therefore
   sends nothing a viewer can see for as long as the pause lasts, and "no
   contact with dove:8787" would be a confident false statement about a link
   that is fine — the founding hazard in three words. "No readings" is true
   whether the fleet is quiet, the server is gone, or the network is, and it
   still names the endpoint, which was the point of the sentence.
   `the_warning_claims_no_more_than_the_viewer_can_observe`.

   Found by asking why a `cargo-mutants`-style deletion of the server's
   `.keep_alive(...)` survived: the answer is that nothing downstream can see a
   keep-alive, which is also the answer to what the wording could honestly
   claim.

   ### Reconnecting

   **Specced 2026-09-09, because it was the one thing here nobody had
   decided.** The step said what a lost server looks like on screen and never
   said what the loop does about it, which leaves the implementer inventing a
   policy in their first ten minutes — the situation ADR-018 exists to
   prevent. Four rules:

   **It retries forever, and it does not back off.** A viewer left open
   overnight against a server that restarts must come back on its own; a
   window that has to be relaunched to reconnect is a window nobody trusts to
   be showing the present. Exponential backoff exists to protect a shared
   service from many clients, and this is one client against one machine the
   user named — what backoff buys instead is a delay grown to minutes, so the
   server returns and the grid does not, which reads as the app being broken
   at exactly the moment it was fixed.

   **The delay is a pure function in core, not a constant in the loop.** The
   read loop lives in `src-tauri`, which nothing here compiles (ADR-018
   decision 3), so a number written inline there is a number no test ever
   sees. `remote::reconnect_delay(attempt) -> Duration` is pure, lives beside
   the parser, and the socket loop only calls it.
   `reconnect_delay_is_bounded_so_a_returning_server_is_noticed_promptly`
   asserts the ceiling, which is the property that fails if somebody later
   "improves" it into a backoff.

   **The first connect after a switch reports its error; later drops do
   not.** They are different events and collapsing them costs a real thing: a
   typo'd endpoint that silently retries forever looks exactly like a server
   that is down, and the user has no way to tell that nothing was ever
   reachable. So the connect `use_endpoint` triggers surfaces its failure —
   step 3's path, but step 2 builds the loop that tells them apart — while a
   drop from an established connection goes quietly to the stale chrome above.
   `a_typo_in_an_endpoint_is_reported_rather_than_retried_in_silence`.

   **A refused endpoint is never retried at all.** `https://` and an
   unparseable URL are refused at parse time (ADR-018 decision 2), which is a
   verdict rather than a failure — retrying it every few seconds would be a
   loop that cannot ever succeed.

   ### `capabilities`

   **It becomes a Tauri command.** `app.js` currently catches its absence and
   concludes *"the desktop app, which can do it all"* — false the moment that
   app points at a read-only server, and the result is buttons that can only
   return an error, which is the exact thing the command exists to prevent. It
   reports *effective* capability, and the catch block dies with it. It also
   carries the endpoint, `stale_after_ms`, and the version of whatever
   produced the events, because a viewer one release ahead reads a renamed
   field as absent and draws a plausible wrong number; a mismatch is stated,
   not guessed. `a_version_mismatch_is_stated_not_guessed`.

   **It has to be re-read, not read once.** Step 3 switches endpoints at
   runtime, at which point every field of it changes. The frontend re-invokes
   `capabilities` on `tuxtop://settings-changed` — one line here, and step 3
   then needs no frontend change at all.

   **A read-only server already draws three controls that can only fail.**
   Noticed while specifying this, and true today, before remote mode exists:
   `data-readonly` hides Add host, Remove, Pause and the drag grip and
   disables the per-host table, but the Settings dialog's own interval,
   history-limit and update-check fields stay live and Save returns 403. Fixed
   here rather than filed, because this step is what first points the desktop
   app at that path.

   ### The chrome

   **The browser is already a remote viewer, and has been since
   `tuxtop-serve` shipped.** A tab served by a server is pointed at a server by
   definition: its numbers were taken by a machine it is not on, at an interval
   it did not choose, and it says nothing about either. `app.js` takes
   `globalThis.__TAURI__` and never asks which implementation it got, so the
   distinction does not currently exist in the frontend at all. ADR-017 rule 1
   binds that tab exactly as it binds the desktop window, so the chrome is
   **shared frontend work driven by data both backends supply** — endpoint
   identity, age of the last event, the server's interval — and not a
   desktop-only path.

   That is also what makes it *testable*. `src-tauri` is outside the workspace
   and is never compiled here, so a chrome built only for the desktop window
   could be verified only by building on Windows. Built for both, the whole of
   it is reachable from the Playwright harness.

   **It needs a layout decision, not a spare corner.** The tally was ~70px from
   overflowing the toolbar at nineteen hosts. The endpoint and its freshness
   get their own element, with a Playwright test at the harness's nineteen
   hosts, in both themes.

   **`#tbsub` is a mockup string that shipped.** The titlebar subtitle reads
   `— dove.example.ts.net` in `index.html`, and no code has ever written to
   it: a hardcoded hostname belonging to nobody's fleet, for fourteen phases.
   ~~It is also precisely the element this step needs.~~

   **Wrong, and corrected 2026-09-10 while implementing commit 4.** It is not
   in the chrome of any released build either: `startLive()` does
   `document.querySelector('.titlebar')?.remove()` unconditionally, because
   Windows draws the real titlebar (`decorations: true`). `startLive` runs
   whenever `__TAURI__` is present — the desktop app, a browser tab through the
   `http.js` shim, **and** the Playwright harness through the stub — so
   `#tbsub` exists only in the simulator, `index.html` opened with no backend
   at all. That is also why nothing has ever written to it. A chrome built
   there would be invisible in exactly the three modes that need it.

   So the other sentence in this section is the operative one, and the layout
   decision was taken rather than inherited: **the endpoint and its freshness
   are their own row above the toolbar** (`#remotebar`), hidden entirely when
   this window does its own sampling. Rejected: a corner of the toolbar, which
   is what "not a spare corner" was already warning against — though the
   ~70px figure is out of date, since `.toolbar` has had `flex-wrap:wrap`
   since the History select first clipped "Add host", so an extra element
   wraps rather than clips.

   `#tbsub` stays a mockup string in the simulator. Removing it is a Phase 2
   line item ("drop this in favour of the real OS titlebar") and not this
   step's to spend.

   Its strings are pure and go in `src/remote.js` with the other modules, not
   into `app.js`; the frontend went 2,792 lines with zero coverage that way.

   ### Files

   **New**
   - `crates/tuxtop-core/src/remote.rs` — endpoint parsing, request building,
     response-head parsing, de-chunking, `split_sse_frames`, `decode_event`,
     freshness, `Capabilities`.
   - `crates/tuxtop-core/tests/sse_capture.rs` + the captured response above.
   - `src-tauri/src/remote.rs` — connect, read loop, emit, record, and the
     blocking `POST` the proxy uses.
   - `src/remote.js` and `tests/remote.test.js` — the chrome's strings.
   - `tests/e2e/remote.spec.js`.

   **Changed**
   - `hostlist.rs` — the settings split.
   - `service.rs` — `start_all`, `capabilities`, the write refusal.
   - `lib.rs` — the new module.
   - `src-tauri/src/main.rs` — the `capabilities` command, the read loop in
     `setup`, and one dispatch point every command goes through.
   - `crates/tuxtop-serve/src/api.rs` — `capabilities` grows fields; the
     server answers the same shape the desktop does. Its own test,
     `capabilities_tells_the_truth_about_which_server_this_is`, asserts
     today's `{writable}` and changes with it — extend that test rather than
     adding a second one alongside it.
   - `src/app.js`, `src/index.html`, `src/styles.css`.
   - `tests/harness/stub.js` — the stub needs every new command, and a gap
     there has twice presented as an application bug.

   ### The dispatch point

   Seventeen commands cannot each carry an `if remote` — that is the shape
   ADR-012 warns about, with five callers of which one forgets. One helper in
   `main.rs` takes the command name, the arguments and a closure producing the
   local answer, and is the only place that *proxies*.
   `check-commands-reachable.py` goes on counting them.

   **Two things know a server exists, and they know different verbs.** An
   earlier draft of this line called the helper "the only place in the process
   that knows a server exists", which cannot be true beside a write refusal
   that lives in `Service` — core has to know the mode in order to refuse.
   Read literally it argues for hoisting the refusal out of core, which is the
   wrong direction: a refusal in the shell is a refusal the workspace never
   compiles and no test here ever runs. So, precisely:

   - **core knows to *refuse*** — `Service` holds the write refusal and the
     derived mode, and is the choke point ADR-012 asks for.
   - **the shell knows to *proxy*** — the `main.rs` helper owns the socket,
     and is the only place turning a local command into an HTTP request.

   A viewer that reached the writes through the proxy rather than the local
   service would still be refused, by the server, for the same reason —
   `tuxtop-serve` is read-only unless `--writable`.

   The proxy's I/O blocks, so it does not run on the async runtime: ADR-018's
   revisit note already says a blocking client belongs on its own thread, and
   that applies to the request path before it applies to `ureq`.

   ### Commits

   1. ~~core: the wire — `remote.rs` and the captured-response fixture.~~ Done.
   2. ~~core: the settings split, `start_all`, `capabilities`, the write
      refusal.~~ Done.
   3. ~~`src-tauri`: the read loop, the dispatch point, the `capabilities`
      command.~~ Done, third of the four to land.
   4. ~~frontend: the chrome, the stub, the Playwright spec, and the settings
      fields a read-only server should never have offered.~~ Done, before 3.

   ### Verified against a real server, 2026-09-10

   A green build is not a launch, and a launch in local mode is not remote
   mode. `verify.sh` closed the Windows build and the smoke test — 16 ssh
   sessions opened, gone within 0 s — but the live `hosts.toml` names no
   server, so the smoke test exercised the path this step does not change.

   So: a `tuxtop-serve` on the WSL box, `--bind 0.0.0.0`, read-only, watching
   two hosts of its own (`coot` reachable, `wader` at `127.0.0.1:9` to produce
   a real fault); the desktop app's `[settings] server` pointed at
   `http://localhost:8788`; the config backed up first and restored afterwards,
   sha256 confirmed identical. Windows reaches a WSL listener on `localhost`.

   What it showed, in order:

   - **`ssh.exe: 0`.** Remote mode started no samplers of its own, which is
     `start_all_starts_nothing_when_an_endpoint_is_set` on the real thing.
   - **One** established connection to 8788, not two.
   - The grid was the **server's** fleet — `coot` and `wader`, names that exist
     in no local config — with `wader`'s "Host unreachable: ssh: connect to
     host 127.0.0.1 port 9: Connection refused" decoded off the wire, so a
     fault reached the right card.
   - The status line read `Tuxtop 0.7.0 · live · 1 s via localhost:8788`. **The
     "1 s" is the proof**: the local file says `interval_ms = 2000` and the
     server samples at 1000, so the interval on screen came through the
     fleet-read proxy rather than off this machine's disk.
   - Killing the server left the app up, logged `remote: localhost:8788 closed
     the stream`, and said nothing further — `Next::Quiet`. The grid kept every
     reading it had, and the strip turned amber: **`no readings from
     localhost:8788 since 12:53:43`**.
   - Restarting the server got the fleet back with no relaunch and no further
     log line, on a new source port. It retries forever and does not back off.
   - Restored: sha256 of `hosts.toml` identical to the backup, no `server` key,
     no `ssh.exe` left behind, app and server both stopped.

   Not covered, and worth knowing: nothing exercised a **write** refusal
   through the UI, or a `version_note` (both builds were 0.7.0).

   Commit 3 is the one nothing here compiles. Build it through
   `scripts/verify.sh`, which drives the Windows toolchain at `/mnt/c` — and
   remember a green build is not a launch. Two startup panics have shipped
   past one; the smoke test is what catches a `setup` that panics.

   **Commits 1 and 2 landed 2026-09-09.** Three things a later session needs
   that the spec above does not say:

   - **`serde_json` is now one of core's dependencies**, 25 crates to 29.
     Reading back a payload our own server serialised needs a JSON
     deserialiser, and hand-rolling one is not the base64 argument. The
     ADR-018 consequence line claiming core gains no dependency is corrected
     in place rather than amended away.
   - **`effective_interval_ms` takes `&FleetSettings`**, not `&Settings` — the
     signature follows the split, since it only ever read `interval_ms`.
   - **`set_settings` compares the fleet half *after* clamping** and refuses
     only if it actually changed. A viewer saving its own half sends the fleet
     half back untouched (that is what `{...s, always_on_top}` does in
     `app.js`), and refusing it would be a window that cannot be pinned — the
     thing the split exists to prevent.

   **Commit 4 landed 2026-09-10**, out of order: commit 3 is `src-tauri` and
   needs the Windows toolchain and a launch, while the chrome is specced as
   shared frontend work driven by data both backends already supply, so all of
   it is reachable from the Playwright harness on the dev box. Between the two
   commits the desktop window reads *sampling locally* unconditionally, which
   is true during that window rather than a wrong number.

   Four things found by building it that the spec did not say:

   - **`refreshCapabilities` has to run before `refreshModeNote`.** The status
     line quotes the machine that is sampling, and painting it first left
     `over ssh` on a remote viewer for the life of the session, because nothing
     re-ran it. Caught by the E2E test, not by reading — which is the argument
     for having built the chrome for both backends rather than only the
     desktop.
   - **`set_host_os` was missing from `tests/harness/stub.js` entirely**, so
     the per-host OS dropdown has thrown in the harness since it shipped while
     working in the app. That is the *only* path to `os` for a host that
     already exists, and the third time a gap in the stub has presented as an
     application bug. Added, with
     `a_host_already_in_the_fleet_can_be_switched_to_windows` in
     `interval.spec.js` — which fails when the stub command is removed again.
   - **Two E2E assertions were vacuous at Playwright's default width.** The
     toolbar fits one row at 1280 whether it wraps or not, so a clipping
     assertion made there passes against `flex-wrap:nowrap`. It runs at 1000
     against a read-only backend now, where wrapping gives 97px with all seven
     controls drawn and `nowrap` gives 56px with one pushed out of the row
     entirely. The first fix, 1100, was still vacuous — the read-only baseline
     has no "Add host" taking up ~100px.
   - **A both-themes test can check the wrong state.** Asserting that the
     *calm* ground differs between light and dark stayed green against a stale
     ground written as `rgba(180,105,14,.14)`, because the calm ground beside
     it was still a token and still flipped. The alarming state is the one a
     hardcoded colour is most tempting in, and it is now asserted too.

3. ~~**Switching endpoints without a restart.**~~ **Done, 2026-09-10.** Needs `Supervisor::stop_all`,
   which does not exist yet; the teardown belongs there rather than in the
   caller that switches. History is discarded across a switch, never appended.

   **`HistoryStore` has no `clear` either** — only `forget_host(name)` — and
   discarding history needs one. Two fleets each with a host called `db1` would
   otherwise blend charts, and one customer's spike on another's graph looks
   entirely fine.

   **One switch method: `Service::use_endpoint`.** Not a check in each caller.
   Five callers already restart hosts as a side effect of something else
   (`start_all`, `set_settings`, `set_host_interval`, `set_host_os`,
   `add_host`), and the pause rule survives only because it lives in
   `Supervisor::start` and nowhere else — ADR-012. Switching back to local
   restarts the fleet, which makes it the sixth member of that family and the
   one most likely to quietly resume a machine somebody took down.
   `switching_back_to_local_does_not_resume_a_paused_host`.

   `stop_all` is unconditional: stopping an already-stopped host is a no-op,
   and pause is enforced on the way back, not on the way out.

   **`use_endpoint` cannot own the whole switch, and the seam has to be named
   here rather than discovered.** The socket lives in `src-tauri` (ADR-018
   decision 3), so core cannot reach the read loop. Split it: `use_endpoint`
   stops the samplers, clears the history, persists the endpoint and announces
   `SettingsChanged`; the shell owns an abortable read loop and restarts it on
   that announcement. So the loop must be **cancellable from the start** — a
   `JoinHandle` the shell keeps, not a `spawn` and forget. Step 2 builds it
   that way even though step 2 never cancels it, because retrofitting
   cancellation onto a running loop is how a switch leaves two loops feeding
   one window.
   `switching_endpoints_leaves_exactly_one_reader`.

   **`switching_endpoints_leaves_exactly_one_reader` has no home in this tree,
   and that is a correction to the line above rather than a test that was
   skipped (2026-09-10).** `ReadLoop` is in `src-tauri`, which is outside the
   workspace (ADR-006): `cargo test` never compiles it and CI's `core` job
   never sees it. A `#[cfg(test)]` there would compile under `cargo xwin` and
   never execute — a test that cannot fail, which is the same trade ADR-018
   decision 3 refuses for a parser. So the invariant is held **structurally**
   and measured **on the real thing**: `start` aborts whatever was running
   before it installs a replacement, `stop` is the local-mode half that `start`
   cannot cover, and the count of established connections to the endpoint after
   a switch is what says whether that worked. That measurement is in the
   verification below, beside step 2's "one established connection, not two".

   ~~Nothing in the frontend changes: step 2 already re-reads `capabilities` on
   `tuxtop://settings-changed`.~~

   **Half true, and corrected 2026-09-10 before anyone starts.** The *chrome*
   needs no change — that much holds, and it is why step 2 spent a line on
   re-reading `capabilities`. But the endpoint has to be **typeable
   somewhere**, and as written this step ships a switch nobody can reach:
   ADR-017 part 4 says switching is a supported act "from Settings or the
   command line", and neither exists after step 2.

   It will not even build. `scripts/check-commands-reachable.py` parses
   `generate_handler![…]` and requires every command to be invoked from
   `src/app.js`; `use_endpoint` with no caller fails that gate in `verify.sh`
   and in CI's `core` job. So this is a stop, not a nicety — and it is the
   third instance of the shape CLAUDE.md now has a rule for: a backend, a
   config key and a documented example, with no control.

   ### What step 3 owes the frontend

   - **A field in Settings, beside "Always on top" and the update check.**
     `server` is a *viewer* setting (ADR-018 decision 4), so it sits with the
     other two and is gated by `TuxRemote.editable(...).viewer` — which means
     it stays editable in remote mode, since that is how you switch away or
     back to local.
   - **It must call `use_endpoint`, not `set_settings`.** `set_settings` takes
     `server` from disk and never from the request, deliberately: `app.js` has
     two save paths and one drops the field. That refusal is what makes
     `use_endpoint` the only door, so a Settings form that posted `server`
     through `set_settings` would silently do nothing.
   - **Clearing the field switches back to local**, which is the sixth member
     of the family `switching_back_to_local_does_not_resume_a_paused_host`
     guards.
   - **The first connect's failure has to reach the window.** The read loop
     logs a `Next::Report` failure to stderr, and a release build has no
     stderr (`windows_subsystem = "windows"`), so the log line exists for a
     debug build and the smoke test. `use_endpoint` is the path that can
     return the error synchronously to the caller, and it must — otherwise a
     typo'd endpoint is indistinguishable from a server that is down, which is
     the distinction `next_after_failure` was built to preserve.
   - **`ReadLoop::stop`'s `#[allow(dead_code)]` comes off here.** It was
     shipped unused on purpose; this is its caller.

   Two E2E tests, named for the invariants rather than the feature:
   `typing a server address switches the fleet without a restart`, and — for
   the path the new CLAUDE.md rule says gets forgotten —
   `clearing the server address returns to the local fleet`, which must start
   from an endpoint that was already set when the page loaded rather than one
   the test typed itself.

   ### What building it found

   - **The grid had to be re-seeded, and nothing announced that it should be.**
     `use_endpoint` emits `SettingsChanged` and not `HostsChanged` — correctly,
     because in remote mode the host list is the server's and core has never
     seen it — so the nineteen cards of the fleet you just left stay on screen
     until something asks `list_hosts` again. The launch path already did that
     inline in `startLive`; it is now `seedFleet(fresh)`, shared, and the switch
     passes `fresh` so every card is dropped first. A host of the same name on
     the new fleet would otherwise inherit the old one's readings and sparkline
     — ADR-017 rule 2's frontend half, where the backend has already discarded
     the history behind it. `lastEventAt` is reset with them: it belongs to the
     connection, and carrying it over reports a new endpoint as current before
     anything has arrived from it.

     The stub deliberately does **not** emit `hosts-changed` either, for the
     reason a stub gap has three times presented as an application bug: one that
     pushed the list would leave the re-seed untested while looking green.

   - **The switch is confirmed by asking, not by the call returning.** A
     *refused* endpoint (`https://`) changes nothing at all, while a first
     connect that failed has switched the window anyway — and both reach the
     frontend as a rejected promise. So the handler re-reads `capabilities` and
     compares the endpoint before and after; only a change re-seeds. Inferring
     it from the rejection would tear down a perfectly good fleet on a typo the
     backend had already refused.

   - **The field's value comes from `capabilities`, never from
     `get_settings`.** That call is a *fleet read* and is proxied in remote
     mode, so it answers with the server's settings — in which no server is
     named, because a server samples locally and is right to say so. Filling
     the field from it shows an empty box on a window that is plainly watching
     somebody. `the field cannot be cleared if it never showed what it holds`
     is the assertion that catches it.

   - **`use_endpoint` saves through `save_file`, not `save_fleet`.**
     `refuse_if_remote` would refuse the one write that has to keep working:
     switching *away* from a server. It is a viewer setting — this machine's,
     not the fleet's (ADR-018 decision 4) — so it was never the write the
     refusal is for, but the two are one line apart and the wrong one compiles.

   - **The endpoint is refused before anything is torn down.** `https://` is a
     verdict rather than a failure, and a switch that stopped the fleet on its
     way to refusing would leave the window watching nothing — with the
     samplers it had just killed not coming back until somebody noticed.
     `a_refused_endpoint_leaves_the_window_where_it_was`.

   ### Verified against a real server, 2026-09-10

   The E2E suite drives the stub, and the stub is the one thing that cannot
   show this: it has no samplers to stop and no socket to hold open, which is
   most of what a switch *is*. So: a `tuxtop-serve --bind 0.0.0.0 --port 8788`
   on the WSL box, read-only, watching two hosts of its own — one reachable,
   one at `127.0.0.1:9` for a real fault — against the desktop app on Windows
   watching its own nineteen. The live `hosts.toml` was backed up first and its
   sha256 confirmed identical afterwards.

   Measured, in order:

   - **Local before the switch: 16 `ssh.exe`, 0 connections to 8788.** 19 hosts,
     17 up, 2 paused, status line `Tuxtop 0.7.0 · live · 2 s over ssh`.
   - **After typing `localhost:8788` into Settings → `ssh.exe` **16 → 0**.**
     `Supervisor::stop_all` on the real thing: switching to a server tore down
     every local sampler rather than leaving nineteen sshd sessions on machines
     we promised only to observe.
   - **Exactly one established connection to 8788, and this is where
     `switching_endpoints_leaves_exactly_one_reader` is actually checked** —
     the assertion that has no home in the workspace, taken here as a
     measurement instead.
   - The grid became the **server's** fleet — `pipit` and `snipe`, names in no
     local config — with `snipe` carrying "Host unreachable: ssh: connect to
     host 127.0.0.1 port 9: Connection refused". The strip read
     `● localhost:8788`, "Add host" was gone, and the charts started from
     nothing rather than continuing the local fleet's.
   - **The status line read `live · 1 s via localhost:8788` while the local
     file said `interval_ms = 2000`** — the same proof step 2 used, and it
     still holds after a *runtime* switch rather than a launch.
   - Settings in remote mode: the interval showed **1 second** and the history
     limit **64 MB** — the server's, not this machine's 2 s and 256 MB — both
     disabled, with *"The sample interval and history limit belong to
     localhost:8788, which is doing the sampling."* Server, Always on top and
     the update check stayed enabled, and the per-host table listed the
     server's two hosts.
   - **Clearing the field: `ssh.exe` 0 → 17, connections to 8788 → 0.** The
     fleet came back, the reader stopped (`ReadLoop::stop`, the half that
     shipped unused), and the `server` key left `[settings]`.
   - **17, not 19.** The two paused hosts stayed paused through both switches —
     `switching_back_to_local_does_not_resume_a_paused_host` on the real fleet,
     which is the assertion that matters most here because the switch back
     restarts *everything*.
   - Restored: sha256 identical to the backup, no `server` key, no `ssh.exe`
     left behind, app and server both stopped.

   **Noticed and left, with the reason.** During the switch the settings
   meter can render one mixed frame — the new fleet's traffic rows beside the
   old fleet's paused count ("1 reporting host, with 2 paused" when the new
   fleet has none) — because the meter's own 2 s timer can start before the
   switch and finish after it. It corrects itself on the next tick, which was
   confirmed rather than assumed. Fixing it properly means a generation counter
   on an async render, which is more machinery than a ≤2 s transient in an open
   dialog is worth; recorded here so the next session knows it was seen and
   priced rather than missed.

   **Driving the app from WSL cost more than the feature did**, and the lesson
   is in the `verifying-remote-mode-from-wsl` memory: a cursor teleported with
   `SetCursorPos` and clicked *focuses* the element under it and dispatches no
   click, because Chromium takes its hit-target from mouse movement — so a nudge
   and a settle are needed before the button events. Two clicks were lost to
   that before it was diagnosed, and one of them landed somewhere unintended and
   resumed a paused host, which the restore undid. Screenshot immediately before
   every click: the toolbar scrolls with the page, so a coordinate read off an
   older capture is a coordinate for a different page.

4. **Saved endpoints**, so several fleets — or several customers — are one
   selection rather than one edit.

   **They live in the local `hosts.toml`**, which stays local in remote mode
   along with the local host list — that list is precisely what you switch back
   *to*. `[[endpoints]]` with a name and a URL, round-tripped by `Config` the
   way `HostsFile` already is.

   **TOML field order is load-bearing for the third time.** Plain tables must
   precede arrays-of-tables, so `HostsFile` reads `[settings]`, then
   `[[endpoints]]`, then `[[host]]` — and a struct that declares them in any
   other order serialises fine and fails to parse.
   `settings_are_written_before_the_host_array` already asserts half of this;
   extend it rather than adding a second test that checks the same rule from a
   different angle.

   **Selecting a saved endpoint goes through the same `use_endpoint` as typing
   one.** A second path is a second teardown to forget, which is the ADR-012
   lesson wearing different clothes.
   `a_saved_endpoint_switches_through_the_same_path_as_a_typed_one`.

   **A new field needs a control on both paths.** Adding an endpoint and
   editing one that already exists are two places, and the second is the one
   that gets forgotten — host `os` shipped with a backend, a `hosts.toml` entry
   and a documented example, reachable only from the Add host dialog and so
   only for a host that did not exist yet.
   `check-commands-reachable.py` covers commands and cannot cover fields.

   **So the forgotten path gets the named test** (added 2026-09-10; this
   section previously named a test for the switch and none for the edit, which
   left the one thing it warns about unguarded):
   `an endpoint that already existed can be renamed and repointed`, in
   `tests/e2e/endpoints.spec.js`.

   **It must edit an endpoint it did not create, and that is the whole
   assertion.** A test that adds one and then edits it would have passed
   against the host-`os` bug too: the Add dialog worked, and the hole was
   specifically the entity that was already there when the page loaded. So the
   endpoint comes from the `hosts.toml` the harness starts with — the same
   shape as `a host already in the fleet can be switched to Windows` in
   `interval.spec.js`, which was written when `set_host_os` turned out to be
   missing from the stub entirely, and which fails if that stub command is
   removed again.

   Two things follow for the stub: `tests/harness/stub.js` needs the endpoint
   commands *and* a starting `[[endpoints]]` list, since a harness with none
   makes the test above unwritable rather than merely weak.

~~Worth doing on the way through: the `broadcast` buffer is 16 and that is
tight once several clients are normal.~~ **Checked 2026-09-07: there is nothing
to do.** The buffer is `1024` and has been since the crate was written
(`main.rs`, `4ab9b65`) — ~54 s of events at nineteen hosts and 1 Hz, not 0.8 s.
The `16` is `api.rs:315`, inside `mod tests`: a fixture, added later by the test
commit `2019bb2`, and read here as though it were the server. Left as a
correction rather than deleted, because the next reader will otherwise
rediscover the `16`, and the only edit it invites is to a test helper.


---

## Phase 15 — Nothing we start outlives us — **done, verified 2026-09-08**

**Goal:** however Tuxtop dies — cleanly, crashed, or `taskkill /F` — nothing it
started is still running on a monitored host a minute later.

This is filed as a broken promise rather than a resource leak, which is what
earns it a slot ahead of feature work.
[ADR-004](DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host)
and
[ADR-010](DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host)
say the monitored host receives nothing and is changed in no way, every command
a read. A PowerShell sampler loop polling WMI every two seconds, outliving the
application that started it by five days, is something we left running on a
machine we promised only to observe.

**The evidence, and the line between what was measured and what was
inferred.** Measured: fifteen orphaned `ssh.exe` clients on one Windows host in
the fleet, all with dead parents, all carrying our own `--=TUXTOP=--` delimiter,
with creation times spanning about three days. The host sat at 55–62% idle;
killing the orphans took it to 5–7%, `WmiPrvSE` falling from 355% of a core to
3.6%, and `sshd` back to the listener alone. Roughly five of sixteen cores,
invisible, for five days.

**Inferred, not measured: that it is one orphan per Tuxtop instance that died.**
It is the natural reading of the timestamps and it was never confirmed — the
parent processes were long gone by the time anyone looked, so nothing
established that each dead parent was Tuxtop rather than something else
spawning through the same path. Recorded as inference because the count is the
kind of plausible, well-shaped number this project exists to distrust, and
because of what it would otherwise do to the implementer: **a repro that
produces a different number of orphans is not a failure to reproduce.** The
mechanism below is what to verify against, not the count.

**Two mechanisms, and neither is the one ADR-013 reasoned about.**

- `transport.rs` sets `.kill_on_drop(true)`, which fires on drop and only on
  drop. `taskkill /F`, a crash, a dev-loop rebuild or the OS killing the app
  runs no destructor, so the child is orphaned, still connected, still
  answering keepalives. CLAUDE.md already concedes the premise in the
  smoke-test note — *"`taskkill /F` leaves no chance to run a destructor"* — and
  nobody followed it to the far side.
- `windows.rs` puts the lifetime cap in the `elseif` arm of
  `if($wdi -ne 0){…}`, so it applies **only when the ancestor walk failed**.
  Walk succeeds and client abandoned: the per-connection `sshd` is still alive
  because the client is, `GetProcessById($wdi)` keeps succeeding, and nothing
  bounds the loop. See the dated correction in
  [ADR-013](DECISIONS.md#adr-013--a-windows-remote-loop-watches-its-sshd-session-not-its-pipes).

### Why this goes before Phase 14 step 2

Not merely "fix bugs first". Step 2's fourth commit is verified by building on
Windows, launching, killing and repeating — which **is** the repro, so doing it
first manufactures orphans at exactly the rate you iterate. Worse, those
orphans burn cores on the hosts step 2 exists to display: debugging a remote
viewer's numbers against a host whose load is your own leak is this project's
founding trap with extra steps, and the leak is invisible from the app.

### The fix

**Primary — a Windows Job Object**, created at startup with
`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`, each `ssh` child assigned to it right
after spawn in `transport.rs`'s `ssh_command`, which already reaches for
`creation_flags` and is the natural seat for the `#[cfg(windows)]` block. When
the last handle closes — including under `TerminateProcess` and on a crash —
the kernel kills everything in the job. It is the only mechanism that survives
a hard parent death. Keep the handle non-inheritable so children do not hold
the job open. `CREATE_SUSPENDED` + assign + `ResumeThread` closes the
spawn/assign race; for `ssh` that window is negligible and the simple form is
enough. `win32job` wraps the calls if hand-rolling the winapi is not wanted.

**Secondary — make the cap unconditional**, so the sentence in ADR-013 becomes
true. Rename it: `UNWATCHED_MAX_MS` is wrong the moment it also bounds a
watched loop.

**Keep the cap at 30 minutes; do not lengthen it.** A multi-hour cap for the
watched path is tempting and is a cap nobody will ever soak, so it ships
unverified — and unverified is the entire failure mode here. Thirty minutes is
soak-testable in one sitting, and the cost of it firing on a healthy connection
is one reconnect, which the existing comment already accepts as the trade.

**Optional — reap on startup.** Sweep for `ssh.exe` whose command line carries
the sampler marker and whose parent is gone. It helps only machines that
already have orphans, which is every machine anyone has killed this on.

### The trap, restated because it has already caught this exact code once

**Unit tests cannot check any of it.** They assert on the script text we
generate, and the whole failure mode is the far side behaving differently from
what the text implies — a heartbeat design passed every unit test and would
have dropped every Windows host in the fleet 30 s in. A test named for the cap
already exists and asserts the string is present; it passed throughout.

So: **verify against a real Windows host, and soak for longer than any timeout
in the mechanism.** Confirm a live session survives well past the cap *first*,
then that the remote dies after the client is killed. Checking only the second
half is how the original broken design got as far as it did.

### Verified on a Windows host, 2026-09-08

A **live pre-fix orphan was still running** when this was checked, which made
the verification far better than it would otherwise have been: it is a control
that could not be faked. `ssh.exe` with a dead parent, the whole chain intact —
`ssh → sshd → sshd → cmd → powershell` — created 21:05:31 the previous evening
by one of our own smoke-test runs, still alive 19.3 hours later, its sampler
loop having burned 2,817 s of CPU: **4.1% of a core, continuously, on a machine
nothing was monitoring**. The fix was committed at 21:36 and the binary built
at 21:46, so the orphan predates both by half an hour.

**The mechanism was checked directly rather than by its outcome**, which
matters, because outcome alone proves very little here: across roughly a
hundred pre-fix sessions since that host last booted, exactly *one* orphan
exists. At that rate a clean run of sixteen would happen about a third of the
time with no fix at all. So `IsProcessInJob` was asked instead:

```
tuxtop.exe        -> not-in-job    (correct: it creates the job, it does not join it)
ssh (pre-fix)     -> not-in-job    (the 9/7 orphan, sitting there as the control)
ssh x16 (fixed)   -> IN-JOB        (every child of the fixed build)
```

- **Hard kill.** `taskkill /F`, no destructor: `ssh` 17 → 1 and `sshd` 5 → 3
  within 8 s, the remote sampler loop gone with them. The only survivors were
  the pre-fix orphan and its loop.
- **Soak, 30 min 2 s.** The loop started 16:48:43 and was still alive at
  17:18:45 across thirty consecutive one-minute samples — no false kill. The
  cap then fired and a replacement loop appeared at 17:18:48, **five seconds
  later**, with the app up and `ssh=16` at every sample including the
  transition. So the recycle is not visible as an outage.
- **No fault card on the recycle**, by code: `closing_fault(got_data, reported)`
  returns `None` when data arrived, and `backoff_secs(0)` is 1 s.

**What is still not known**, and is worth writing down rather than losing in a
green result: *why* the incidental pipe-close teardown let that one session
through on 9/7. The mechanism has never been named. The job object closes the
hole regardless of the answer, since the kernel does the killing however the
process died — that is the argument for it — but "closes it regardless" is
reasoning, and the one observed failure remains unexplained.

### Landed

- The lifetime cap is unconditional and renamed `REMOTE_LOOP_MAX_MS`. The new
  test `the_lifetime_cap_bounds_a_watched_loop_too` asserts the cap is *not*
  nested under the session watch, and was checked by reverting the hoist: it
  fails against the old arrangement. The older test beside it, which asserts
  only that the cap is present, passes either way — which is exactly how the
  bug survived.
- `transport::win_job` puts every `ssh` child into a job object with
  `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. `windows-sys` was already in core's
  Windows tree via tokio, so this cost **no new crates**: 24 before, 24 after.

### Exit criteria — all met

- [x] `bash scripts/verify.sh` green, including the Windows build and smoke
      test. Note that a green smoke test is *not* evidence the job object
      works — it cannot distinguish the new mechanism from the incidental
      pipe-close one, which is why `IsProcessInJob` was asked directly.
- [x] On a real Windows host: `taskkill /F` the app, and no `ssh.exe` orphan
      survives — and no `cmd.exe`/`powershell.exe` sampler loop survives on the
      monitored host either. The second half is the one that matters and the
      one every existing gate misses.
- [x] A session soaked past 30 minutes with no false kill — 30 min 2 s, then a
      replacement five seconds after the cap.
- [x] The count of `sshd.exe` on the monitored host returns to its listener.
