# Architecture

```
                        Windows 11 desktop
  ┌──────────────────────────────────────────────────────────┐
  │  Tuxtop (Tauri 2)                                        │
  │  ┌────────────────────────────────────────────────────┐  │
  │  │  WebView2 frontend  —  src/                        │  │
  │  │  host cards · core grid · charts · Mica backdrop   │  │
  │  └───────────────▲────────────────────────────────────┘  │
  │                  │ Tauri events (JSON `Sample`, 1 Hz)    │
  │  ┌───────────────┴────────────────────────────────────┐  │
  │  │  Rust backend  —  src-tauri/ + crates/tuxtop-core/ │  │
  │  │  ssh sampler · /proc parsing · rate maths          │  │
  │  └──────┬──────────────────────────────┬──────────────┘  │
  └─────────┼──────────────────────────────┼─────────────────┘
            │ FAST PLANE                   │ SLOW PLANE
            │ ssh, 1 Hz                    │ https, on demand
            ▼                              ▼
  ┌──────────────────┐            ┌──────────────────────┐
  │ Linux host       │            │ Beszel hub           │
  │ sshd → sh loop   │            │ PocketBase REST/SSE  │
  │ cat /proc/*      │            │ 1-minute history     │
  │ (nothing         │            │ alerts, inventory    │
  │  installed)      │            └──────────────────────┘
  └──────────────────┘
```

## The two planes

The central design fact: **the two data sources have different jobs because
they have different latencies.** See
[ADR-002](DECISIONS.md#adr-002--two-data-planes-beszel-for-history-direct-ssh-for-live)
and the [measurement](evidence/beszel-cadence.md).

| | Fast plane | Slow plane |
| --- | --- | --- |
| Transport | SSH, one persistent connection per host | HTTPS to the Beszel hub |
| Cadence | 1 Hz | 60 s (fixed, not tunable) |
| Provides | per-core CPU, memory, disk I/O, network, load | history, trends, alerts, inventory |
| Needs on target | nothing | a Beszel agent |
| If unavailable | no live grid; history still renders | no history; live grid still renders |

Neither plane is required. A host with only SSH gets a live grid with no past;
a host whose SSH is down but which reports to Beszel shows history marked
stale. **Never let one plane's absence blank the card** — say which part is
missing.

## Crate layout

```
crates/tuxtop-core/     no GUI dependency; builds and tests anywhere
  src/proc.rs           /proc/stat + /proc/meminfo parsing, delta maths
  src/sampler.rs        the remote shell loop, frame splitting, rate tracking
  src/model.rs          wire types shared with the frontend
  tests/real_host.rs    fixtures from a real 32-core host, checked vs `top`

src-tauri/              thin Windows shell; NOT a workspace member
  src/main.rs           Tauri setup, Mica backdrop, event pump
  tauri.conf.json

src/                    frontend: HTML/CSS/JS, no build step
```

`src-tauri` is excluded from the workspace on purpose — see
[ADR-006](DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace).
`cargo test` at the repo root builds only `tuxtop-core` and passes on Linux,
WSL, macOS or Windows.

## Fast-plane data flow

1. **Connect.** One SSH session per host, authenticated through the Windows
   OpenSSH agent or Pageant. Host list comes from `hosts.toml`, with
   `~/.ssh/config` resolving aliases, jump hosts and keys.
2. **Start the loop.** One channel runs `sampler::sampler_command(1)` — a POSIX
   `sh` loop catting `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`,
   `/proc/net/dev` and `/proc/loadavg`, then echoing a delimiter and sleeping.
3. **Frame.** Reads are appended to a buffer; `split_frames` returns only
   *complete* frames and keeps the tail. A partial `/proc/stat` parses as a
   plausible-but-wrong snapshot, so it must never reach the parser.
4. **Parse.** `parse_frame` turns text into a `Frame` of cumulative counters.
5. **Differentiate.** `RateTracker::push` holds the previous frame and produces
   per-second rates plus per-core percentages. The first frame yields zeros —
   a rate genuinely needs two points.
6. **Emit.** A `Sample` is serialised to the frontend as a Tauri event.

### Why the maths lives in its own module

CPU percentage is a delta of cumulative jiffies, not an instantaneous value:

```
busy% = 1 - Δ(idle + iowait) / Δ(total)
```

Three things are easy to get wrong and all produce *plausible* numbers rather
than errors:

- **`iowait` counts as idle.** Excluding it makes an NFS stall look like 100%
  CPU.
- **`guest`/`guest_nice` must not be added to the total.** The kernel already
  counts guest time inside `user` and `nice`; adding them double-counts.
- **`MemAvailable`, not `MemFree`.** `MemFree` excludes reclaimable page cache
  and reports a healthy Linux box as nearly out of memory.

Each is pinned by a named test. Wrong-but-plausible is this project's central
hazard — it is exactly how the Beszel agent misled us for 26 seconds.

## Slow-plane notes

The hub is PocketBase, so it offers a REST API and realtime SSE subscriptions.
Auth is email/password producing a JWT.

Stats live in `system_stats`, keyed by `type` (`1m`, `10m`, `20m`, `120m`,
`480m`). Field names are terse: `cpu`, `m`/`mu` (memory total/used), `dp` (disk
percent), `ni` (network per interface), `cpus` (per-core array), `t`
(temperatures), `g` (GPU).

## Threading

Tauri's async runtime is Tokio. One task per host owns that host's SSH
connection and never blocks the others; a host that hangs degrades only its own
card. **No blocking I/O on the UI thread** — all SSH work happens in tasks and
reaches the frontend through events.

## What is deliberately absent

- **No database.** History belongs to Beszel. Tuxtop keeps only a short
  in-memory ring buffer per host for the sparklines.
- **No alerting.** Beszel has it.
- **No agent to install.** [ADR-004](DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host).
- **No credential storage.** SSH auth is delegated to the OS agent. Tuxtop
  never reads a private key or prompts for a passphrase.

## Tauri pitfalls that fail silently

Three bugs during Phase 2 presented **identically**: the window opens, renders
correctly, and every control is inert. No crash, no console output anywhere you
would naturally look, nothing in the Rust logs. Recognising the shape is worth
more than the individual fixes.

**1. CSP nonce defeats `'unsafe-inline'`.**
Tauri injects a nonce into the page's `script-src`. Per the CSP spec, once a
nonce is present the browser *ignores* `'unsafe-inline'` — so an inline
`<script>` is blocked outright. The same applies to `style-src` and inline
`style=""` attributes. Fix: keep CSS and JS in external files under
`script-src 'self'`. There is now no inline script or style in `src/`, and the
CSP declares no `'unsafe-inline'` at all.

**2. The ACL denies everything without a capability file.**
Tauri 2 grants nothing by default. With no `src-tauri/capabilities/*.json`,
`gen/schemas/capabilities.json` is literally `{}` and `core:event:listen` is
denied — so the first `await listen(...)` throws and every line after it in
that async function never runs. Fix: `capabilities/default.json` granting
`core:default` to the `main` window, whose label is pinned in `tauri.conf.json`
because both the capability scope and the Mica lookup reference it by name.

**3. An unhandled rejection in startup is invisible.**
Both bugs above surfaced as a rejected promise inside `startLive()`, which
browsers swallow into the console. `src/app.js` now installs
`unhandledrejection` and `error` handlers that render the failure into the grid
with a pointer to devtools.

The through-line matches this project's founding bug: **a silent dead UI is the
same class of failure as a silent wrong number.** Neither announces itself, and
both are indistinguishable from working software until you check.

### Testing the live path without a GUI

`tests/harness/` stubs `window.__TAURI__` so the real `src/app.js` runs in an
ordinary browser against a fake backend. Use it before hunting a UI bug by
reading code — the "adding a second host removes the first" bug survived
several rounds of inspection and was found in one pass there.

## Two views, one matrix

The fleet is a matrix of **hosts x metrics**. The two views are the two ways to
slice it:

- **Host view** - a row: one card per box, every metric for that box.
- **Fleet view** - a column: one metric, every box.

`src/app.js` holds a `METRICS` registry so this is a table rather than a pile
of renderers. Each entry declares two things.

**Shape** - how the metric is drawn.

| shape | meaning | rendering |
| --- | --- | --- |
| `vector` | one value per core / disk / NIC | tile grid per host |
| `scalar` | one value per host | one comparable bar per host |

**Scale** - how values are made comparable *between hosts*.

| scale | used for | why |
| --- | --- | --- |
| `absolute` | CPU, memory, GPU | already a percentage; 0-100 is shared and needs no transform |
| `log` | disk I/O, network, load | a rate, spanning orders of magnitude across a fleet |

### Why log for rates

A linear axis lets one busy host flatten everyone else into invisible slivers.
Normalising each host against its own peak fixes visibility but destroys the
comparison - it hides that one box moves a thousand times the traffic of
another. Log keeps both.

The scale spans a **window of decades below the fleet peak**, not from zero.
The first attempt anchored at 1 byte and crushed everything above a megabyte
into the top third: a 600x spread rendered as 69% against 100%. Four decades
below the peak spreads the same range across 22% to 100%.

A log bar looks exactly like a linear one, so the view states its real bounds
in words and draws decade gridlines on the track. Without that, every
comparison on screen is quietly misread.

### Adding a metric

Add an entry to `METRICS` with `label`, `shape`, `scale`, an accessor
(`scalar` and/or `vector`), and `fmt`. The picker is built from the registry,
so no markup changes. Everything Beszel already collects - temperatures,
per-filesystem usage, container stats - fits this shape.
