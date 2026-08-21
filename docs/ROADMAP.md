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

**Still open:** reconnect with backoff. A dropped connection currently ends the
stream with a fault rather than retrying.

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

## Phase 6 — GPU and temperatures — **planned**

- `nvidia-smi --query-gpu=utilization.gpu,memory.used,power.draw --format=csv,noheader`
- `/sys/class/hwmon/*/temp*_input`
- Both optional additions to the sampler loop; absence is normal, not an error.

---

## Ideas — not committed

- **Per-core sparklines** instead of single-value tiles at large card sizes.
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
