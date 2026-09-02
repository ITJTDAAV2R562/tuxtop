# Architecture

```
                        Windows 11 desktop
  ┌──────────────────────────────────────────────────────────┐
  │  Tuxtop (Tauri 2)                                        │
  │  ┌────────────────────────────────────────────────────┐  │
  │  │  WebView2 frontend  —  src/                        │  │
  │  │  host cards · core grid · charts · Mica backdrop   │  │
  │  └──────▲──────────────────────────────▲──────────────┘  │
  │  events │ (JSON `Sample`)     query    │ (window +       │
  │         │                              │  point budget)  │
  │  ┌──────┴──────────────────────────────┴──────────────┐  │
  │  │  Rust backend  —  src-tauri/ + crates/tuxtop-core/ │  │
  │  │  ssh sampler · /proc parsing · rate maths          │  │
  │  │                        │                           │  │
  │  │              Sample ───┴──▶ history cascade        │  │
  │  │                             1Hz/1h · 10s/6h        │  │
  │  │                             60s/24h · 5m/7d        │  │
  │  └──────┬─────────────────────────────────────────────┘  │
  └─────────┼────────────────────────────────────────────────┘
            │ one persistent ssh per host, configurable rate
            │ metrics and processes, two frame kinds on one stream
            ▼
  ┌──────────────────────┐
  │ Linux host           │
  │ sshd → POSIX sh loop │
  │ cat /proc/*          │
  │ (nothing installed)  │
  └──────────────────────┘
```

## One plane, two readings

The central design fact: **a monitoring agent that caches cannot show you a
spike.** Beszel's agent reported 0.14% during 26 seconds of 25% load, then
21.95% for 25 seconds after it stopped — see the
[measurement](evidence/beszel-cadence.md), which is why this project exists.

So the sampling is ours, and everything is read from one stream:

| | |
| --- | --- |
| Transport | SSH, one persistent connection per host |
| Cadence | configurable, 1 Hz default; per-host override |
| Provides | per-core CPU, memory, swap, disk I/O and capacity, network, load, temperature, GPU, uptime, identity, processes |
| Needs on target | nothing — no agent, no root, no open port |
| Live grid | the newest sample |
| History | the same samples, kept in a four-tier in-memory cascade |

Live and historical are not separate sources with different latencies; they are
the same data read at different zoom levels. That is what makes a 100% spike
still readable an hour later instead of averaged into a 60-second bucket.

**A host going quiet must never blank its card.** It keeps its history, marked
stale, and states the reason it stopped — `Unreachable`, `AuthFailed`,
`SamplerFailed` or `Stalled`, never a generic "offline".

This supersedes the original two-plane split, where Beszel owned history
([ADR-002](DECISIONS.md#adr-002--two-data-planes-beszel-for-history-direct-ssh-for-live)
→ [ADR-009](DECISIONS.md#adr-009--we-own-history-beszel-is-optional-enrichment)).
Beszel is now optional enrichment: it covers only hosts running its agent,
and a history view that silently covers part of a fleet is worse than none.

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

## Three views, one matrix

The fleet is a matrix of **hosts x metrics**, and time is its third axis. The
views are the ways to slice it:

- **Host view** - a row: one card per box, every metric for that box.
- **Fleet view** - a column: one metric, every box.
- **Heat view** - that same column, extended over time: one metric, every box,
  the whole retained window. Rows are hosts, columns are time buckets, colour
  is load. Nothing is aggregated across hosts - this is the one view with room
  for all nineteen - and each cell is the **peak** of its bucket rather than
  the mean, because a mean is precisely what hides a spike
  ([ADR-011](DECISIONS.md#adr-011--a-heatmap-cell-shows-the-buckets-max-not-its-mean)).

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
