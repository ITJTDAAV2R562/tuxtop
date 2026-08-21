# Roadmap

Phases are committable units. Each states what becomes *observably* true when
it lands — not what code exists. Tick the box only when you have seen it work.

Status: **done** · **next** · **planned** · **idea**

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

## Phase 3 — Multi-host — **mostly done, unverified**

Landed with Phase 2 rather than as its own phase:

- [x] `hosts.toml` add/remove, one Tokio task per host
- [x] Reconnect with capped backoff; a good sample resets it
- [x] Faults render as a stated reason on the card
- [x] Hosts that have not reported show as connecting, not up
- [ ] **Not yet verified:** four hosts streaming at once, and killing sshd on
      one leaving the other three untouched. The isolation is written but has
      only been exercised against a single real host.

**Done when:** that last box is ticked against real hosts.

---

## Phase 4 — Beszel history (slow plane) — **planned**

- PocketBase client: auth, read `system_stats`, subscribe over SSE.
- Sparklines extend backwards into history on hosts that have an agent.
- Absent hub is a normal state, never an error.

**Done when:** expanding a card shows the last 24 h behind the live window, and
a host with no agent still works with the live-only view.

---

## Phase 5 — The process list — **planned**

The Task Manager half, and the thing nothing off-the-shelf does from Windows.

- Sortable table: PID, command, CPU%, memory, user.
- Per-process CPU from `/proc/[pid]/stat` `utime + stime` deltas over
  `sysconf(_SC_CLK_TCK)`. **Do not parse `top`** — its output shifts across
  distros, versions and locales, and a decimal comma will silently break it.
- Kill and renice, behind a confirmation, with an explicit sudo story.

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

## Phase 7 — Configurable sample interval — **next**

**Goal:** the interval stops being hardcoded at 1 Hz.

Measured across the live fleet, 19 hosts at 1 Hz is **132 KB/s, 10.8 GB/day**
— see [evidence/sampling-cost.md](evidence/sampling-cost.md). That is not a
reason to sample more slowly by default; 1 Hz is the entire point of the fast
plane. It is a reason for the interval to be a choice.

- A global interval in settings, persisted alongside the host list.
- A per-host override: 1 Hz on the box you are watching, 10 s on the twelve
  you only want to notice going down.
- Changing it must restart that host's sampler without disturbing the others.
- Surface the cost in the UI, so the tool measures itself rather than
  lecturing about measurement.

**Done when:** the interval can be changed per host from the window, survives
a restart, and the fleet's current sampling cost is visible somewhere.

---

## Phase 8 — History plane — **needs a spec first**

**Goal:** metrics over time, not only in the moment. Charts over a window, and
retention that survives a restart.

Deliberately unspecified. It is the largest remaining piece and touches
storage, retention policy, aggregation, and the chart layer at once — the
kind of work that goes wrong when it is started rather than designed. Open
questions before any code:

- **Where does history live?** Beszel already stores 1-minute history and the
  hub is already running (ADR-002). Reading from it is far less work than
  building our own store — but it only covers hosts that run a Beszel agent,
  and only at 1-minute resolution. Our own store would cover every host at
  full resolution and duplicate something that exists.
- **What resolution, for how long?** 1 Hz for an hour is 3,600 points per
  metric per host; across 19 hosts and 8 metrics that is not free. Rolling
  aggregation is the usual answer, and is exactly what Beszel already does.
- **In-memory or on disk?** The app currently keeps a 60-point ring buffer per
  host and nothing survives a restart.
- **What does the UI become?** A time axis is a third dimension on the
  hosts x metrics matrix, and the fleet view has no obvious place for it.

This phase should produce a written spec before an implementation.

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
