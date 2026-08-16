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
- [x] 33 tests pass, including real 32-core fixtures cross-checked against `top`

---

## Phase 1 — SSH transport — **next**

One persistent connection per host, streaming frames.

- Implement `ssh.rs` with `russh`: connect, auth via the OS SSH agent, open one
  channel, run `sampler_command`, feed bytes to `split_frames`.
- Resolve hosts through `~/.ssh/config` (aliases, `ProxyJump`, `IdentityFile`).
- Reconnect with backoff; surface the real reason as a `HostFault` rather than
  a generic "offline".

**Done when:** a CLI example prints live per-core percentages for dove once a
second, and those numbers track `htop` on the same box in real time.

**Watch for:** never spawn a process per sample. One connection, one channel,
long-lived. A handshake per second would dominate the interval.

---

## Phase 2 — Tauri shell with a real Mica backdrop — **planned**

- Tauri 2 project wired to `tuxtop-core`.
- `window-vibrancy` for genuine Win11 Mica/Acrylic.
- Port the mockup's HTML/CSS as the frontend, replacing its simulator with
  Tauri events.

**Done when:** the window opens on Windows with a real Mica backdrop and the
core grid animates from live dove data.

**Note:** must be built on the Windows side. WSL cannot compile the GUI.

---

## Phase 3 — Multi-host — **planned**

- `hosts.toml` for add/remove; one Tokio task per host.
- A hung host degrades only its own card.
- Faults render as a stated reason on the card.

**Done when:** four hosts stream at once, and killing sshd on one leaves the
other three unaffected while that card explains what happened.

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
