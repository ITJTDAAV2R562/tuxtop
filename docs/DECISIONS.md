# Decisions

Numbered, dated, and written so a future session (human or agent) can tell
what was *decided* from what was merely *assumed*. Each entry states the
alternative that was rejected and why, because the reasons are the part that
rots silently when a decision is recorded without them.

Status values: **accepted**, **superseded by ADR-N**, **revisit when …**.

---

## ADR-001 — Build a client, not another monitoring system

**Date:** 2026-08-16 · **Status:** accepted

### Context

The goal is a Windows-native view of several Linux boxes, in the shape of Task
Manager: live per-core load, memory, disk, network, GPU. The starting reference
was [benapetr/TuxManager], which turned out to be Qt6/C++ reading `/proc` on the
*local* machine — no SSH, no multi-host. Porting it would have meant writing
the remote half from scratch anyway.

Off-the-shelf options were surveyed: Netdata, Beszel, XPipe, MobaXterm, Cockpit,
Zabbix, Prometheus + Grafana. All of them are either web dashboards or
connection managers. None presents a Task-Manager-style live core grid on
Windows.

### Decision

Build a **client**. Storage, alerting, history and multi-host inventory are
solved problems; presentation and live sampling are not. Tuxtop owns the
window and the fast path, and reuses existing infrastructure for everything
slow.

### Consequences

Scope stays small. The project never needs a database, a retention policy, or
an alerting engine — if those are wanted, Beszel already has them and Tuxtop
reads from it (ADR-002).

[benapetr/TuxManager]: https://github.com/benapetr/TuxManager

---

## ADR-002 — Two data planes: Beszel for history, direct SSH for live

**Date:** 2026-08-16 · **Status:** accepted

### Context

Beszel was installed on dove to evaluate it (hub at `:8090`, agent at `:45876`,
served over the tailnet). It is genuinely good: ~23 MB agent, clean multi-host
dashboard, alerts, and a PocketBase backend that exposes both a REST API and
realtime SSE subscriptions.

The obvious move was to reuse it entirely and write only a new presentation
layer — no agent to build, no metrics to collect. **This does not work for the
live view**, and the reason is measurable rather than aesthetic.

### The measurement

The agent was polled directly over SSH while a known 8-core load ran on a
32-core host, with `top` as independent ground truth. Full data in
[`evidence/beszel-cadence.md`](evidence/beszel-cadence.md).

| elapsed | `top` (truth) | agent reported | per-core array |
| ------- | ------------- | -------------- | -------------- |
| 0–26 s  | **~25%**      | `0.14%`        | all zeros |
| 26–38 s | ~25%          | `21.95%`       | `[26,47,26,52,46,48,51,15,…]` |
| 38–51 s | **0.0%** (load ended) | `21.95%` | *byte-identical* |

The agent reported idle for the first 26 seconds of sustained load, then
reported 22% for 25 seconds after the load had stopped. Five consecutive polls
returned a byte-for-byte identical `cpus` array. It serves a cached snapshot
refreshed on its own ~60 s cadence; polling faster returns the same bytes.

Confirmed not tunable: the agent exposes `DISK_USAGE_CACHE`, `SENSORS_TIMEOUT`,
`SMART_INTERVAL` and `DOCKER_TIMEOUT`, but **no sampling-interval setting**. The
60 s cadence is the product's design point — it is what makes the agent cost
23 MB. The hub stores a finest granularity of `1m`, rolled up into
`10m`/`20m`/`120m`/`480m` buckets.

A core grid that lags a minute is not a slow Task Manager. It is a wrong one,
in both directions.

### Decision

Two planes, and the app works with either one absent.

**Slow plane — Beszel, unchanged.** History, trends, alerts, and cross-host
inventory, at 1-minute resolution over the PocketBase REST API and SSE
subscriptions. Zero new code on the Linux side.

**Fast plane — ours.** One persistent SSH connection per host running a shell
loop that cats `/proc` once a second. Per-core CPU, memory, disk I/O, network,
load average. Nothing installed on the target.

### Consequences

- A host with no Beszel agent still gets the full live grid, just no history.
- A host with Beszel but unreachable over SSH still shows history, marked stale.
- We are **not** reimplementing the agent. It does genuinely hard,
  cross-platform work — sensors, SMART, Docker, GPU vendor differences. The
  fast plane is roughly 15 lines of shell plus the parser in
  `crates/tuxtop-core/src/`.
- The 60 s vs 1 Hz difference is visible in the design mockup via a cadence
  toggle, because it is the single fact that justifies this whole architecture.

### Revisit when

Beszel gains a configurable sub-second sampling mode, or a push/WebSocket path
that streams rather than caches. Then the fast plane could be retired.

---

## ADR-003 — Tauri 2 + Rust for the shell

**Date:** 2026-08-16 · **Status:** accepted

### Context

Three candidates: Tauri 2 + Rust, WinUI 3 + C#, Avalonia + C#.

The dev machine has Rust 1.96 and Node 24 installed; **no .NET SDK**. The
development environment is WSL2, while the target is a Windows desktop binary.

### Decision

Tauri 2 with a Rust backend and an HTML/CSS frontend.

### Rationale

- **Real transparency, not a CSS imitation.** The `window-vibrancy` crate gives
  actual Win11 Mica/Acrylic backdrop — the same compositor effect Task Manager
  and Settings use. This was a stated requirement, and it is about four lines
  of Rust.
- **The mockup ports directly.** The design work already exists as HTML/CSS;
  with Tauri it becomes the frontend rather than being re-expressed in XAML.
- **~8 MB binary**, WebView2 already present on Windows 11.
- **No new toolchain.** WinUI 3 and Avalonia would both need a .NET SDK
  installed first, and WinUI 3 cannot be built from WSL at all.

### Consequences

- The Windows binary must be built on the **Windows** side. WSL can build and
  test `tuxtop-core` but never the GUI. Hence ADR-006.
- Frontend is HTML/CSS/JS. It will look native because it is *drawn* to look
  native and sits in a real Mica window — not because it uses native controls.
  Accepted: for a dashboard of custom charts and tiles, almost nothing is a
  stock control anyway.

---

## ADR-004 — Nothing gets installed on the monitored host

**Date:** 2026-08-16 · **Status:** accepted

### Context

The fast plane needs per-second data. Options: ship a small agent binary via
scp on first connect; require `node_exporter`; or run a shell loop over the
existing SSH session.

### Decision

A POSIX `sh` loop over one persistent SSH connection. See
`sampler::sampler_command`.

```sh
while :; do
  cat /proc/stat /proc/meminfo /proc/diskstats /proc/net/dev /proc/loadavg
  echo '--=TUXTOP=--'
  sleep 1
done
```

### Rationale

- Works on any box running sshd, with no root, no install, no open port, and no
  firewall change. Adding a host costs nothing and leaves no trace.
- Uses the SSH auth already in place — agent, `~/.ssh/config`, ProxyJump.
- **One connection, not one per sample.** Spawning `ssh host cmd` each second
  costs a TCP and crypto handshake per reading; that latency dwarfs the
  interval and would make the grid stutter.
- POSIX `sh`, not bash — minimal containers and appliances often have no bash.
  Enforced by a unit test.

### Consequences

- GPU and temperatures need extra commands (`nvidia-smi`, `/sys/class/hwmon`)
  and are handled as optional additions to the loop, absent by default.
- Frames must be delimited. A single read can land mid-`/proc/stat`, and half a
  stat file parses as a *plausible but wrong* snapshot rather than an error —
  which is precisely the failure mode this project exists to avoid. Hence
  `FRAME_DELIMITER` and `split_frames`, which never parse a partial frame.

---

## ADR-005 — Load is encoded three ways at once

**Date:** 2026-08-16 · **Status:** accepted

### Context

A 32-tile core grid is scanned peripherally, not read. The eye should catch a
hot core without parsing digits.

### Decision

Every core tile encodes its load redundantly:

1. **Fill height** — proportional, the primary quantitative channel.
2. **Colour band** — accent below 75%, amber 75–89%, red 90%+. Semantic colour,
   separate from the accent hue.
3. **A crisp cap line** at the fill's leading edge.

The fill itself is an alpha gradient, ~90% opacity at the base fading to ~14%
at the top, so the glass shows through.

### Rationale

The cap line is not decoration. Once the fill fades toward the top, the exact
*level* becomes ambiguous; the cap restores a precise reading while keeping the
translucency. Its opacity scales with load (`calc(var(--l) * 3)`) so it
disappears at idle instead of leaving 32 stray lines along the baseline.

Three discrete bands rather than a continuous rainbow ramp: a rainbow is more
information than the eye needs here and reads as garish in a Fluent window.

### Consequences

Any numeral drawn on top of a load-coloured surface needs a contrast halo,
since the background colour is by definition unpredictable. Dark-theme white
numerals over an amber fill were unreadable at exactly the load level that
matters most. Solved with a `--tile-halo` token. **This applies to any future
surface shaded by load** — notably the planned process list.

---

## ADR-006 — `tuxtop-core` is a separate crate, outside the Tauri workspace

**Date:** 2026-08-16 · **Status:** accepted

### Context

The natural Tauri layout puts everything in `src-tauri/src/`. But Tauri depends
on webkit2gtk when built on Linux, which is not installed on the WSL dev box
and never will be — the Windows binary is built on Windows.

If the parser lived in `src-tauri/`, `cargo test` on the dev machine would try
to build the whole GUI stack and fail. The sampling maths would then have **no
tests runnable where the code is actually written**.

### Decision

`crates/tuxtop-core/` holds all parsing, rate maths and models, with no GUI
dependency. `src-tauri/` is a thin shell that depends on it. The root
`Cargo.toml` workspace deliberately excludes `src-tauri`.

### Consequences

`cargo test` runs anywhere — 33 tests, including a fixture of two real
`/proc/stat` readings from a 32-core host cross-checked against `top`. This
matters more than usual here: the bug that started this project (ADR-002) was a
*plausible wrong number*, not a crash. Only a test that compares against
independent ground truth catches that class of error.

This deviates from the scaffold sketched when the stack was chosen, which put
`proc.rs` under `src-tauri/src/`. The deviation buys a testable core on the
machine where development happens.
