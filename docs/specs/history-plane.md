# Spec — History plane (Phase 8)

**Status:** draft, for discussion. No code yet.

Metrics over time, not only in the moment. A third view alongside Hosts and
Fleet, with charts over a scrubable window.

---

## Decisions already taken

These came from the design conversation and are not open:

- **Our own store.** Not dependent on Beszel.
- **Memory only.** Nothing is written to disk. A restart starts clean, like
  Windows Task Manager. This is a deliberate simplification, not a limitation
  to be fixed later.
- **History is low-value data.** Losing it costs nothing. That single
  assumption removes persistence, durability, corruption recovery, migration
  and backup from the design in one stroke.
- **Beszel becomes an optional add-on.** It may be installed on all hosts,
  some, or none. Removing it must lose nothing essential. This supersedes its
  role as "the slow plane" in ADR-002.
- **Capped from the start**, with the cap configurable.
- **Resolution:** 1 Hz for the last hour, coarsening with age, out to 24 h.
- **Window selection is continuous**, not a set of preset buttons — the
  interaction model of a stock chart, not of Task Manager.

---

## Storage: a tiered cascade

Three ring buffers per series. When a tier evicts a point, it is folded into
the next tier down.

| tier | interval | span | points | per point | per series |
| --- | --- | --- | --- | --- | --- |
| T0 | 1 Hz | 1 hour | 3,600 | 4 B (raw) | 14.1 KB |
| T1 | 10 s | 6 hours | 2,160 | 12 B (min/mean/max) | 25.3 KB |
| T2 | 60 s | 24 hours | 1,440 | 12 B (min/mean/max) | 16.9 KB |

**Measured against the real fleet** — 19 hosts, 148 cores, 8 scalar metrics:

```
scalar series (19 x 8 = 152)   8.3 MB
per-core          (148)        8.1 MB
TOTAL                         16.5 MB
```

Keeping every 1 Hz sample for 24 h would be **99 MB**. The cascade is 6x
smaller and, more importantly, bounded by construction: a series occupies
56.2 KB whatever happens.

At 16.5 MB for the whole fleet, **memory is not a design constraint here**.
The cap exists as a safety valve for a fleet ten times this size, not as a
routine limit. That is worth saying plainly so the cap is not over-designed.

### Coarse tiers keep min and max, not just the mean

Non-negotiable, and the reason T1 and T2 cost 12 bytes rather than 4.

A 60-second bucket that stores only the mean **averages away a spike**. A core
pegged at 100% for four seconds inside a quiet minute shows up as 7% — a
confident, plausible, wrong number, which is the exact failure this project
was built in response to (see `evidence/beszel-cadence.md`).

Storing min/mean/max costs 3x on the coarse tiers and lets the chart draw a
**band** between min and max with the mean as a line. That is both more honest
and better looking: the band is where the glossy translucent fill goes.

---

## Where the store lives: the Rust backend

Not the frontend. The backend already receives every sample, so it is the
natural owner, and:

- History survives a **frontend** reload; only a process restart clears it,
  which is exactly the stated behaviour.
- The webview does not carry 16 MB of numbers.
- Downsampling happens where the data is, so the frontend receives only what
  it can draw.

## Query API

```
query_history(host, metric, from, to, max_points) -> Vec<Point>
Point { t, min, mean, max }
```

The backend picks the **finest tier that covers the requested window**, then
downsamples to `max_points` — set by the frontend to roughly the chart's pixel
width.

This is what makes continuous zoom work without presets. The tiers are
*storage*; the window is a *view*. Scrubbing from "last 4 minutes" to "last 9
hours" silently crosses from T0 to T1 to T2, and the chart just gets a
slightly coarser band. No buttons, no snapping.

---

## Interaction with Phase 7 (configurable interval)

They are coupled, which is why Phase 7 should not be finalised first.

Tiers are defined by **time span, not sample count**. A host sampled at 10 s
simply puts fewer points into T0 — 360 rather than 3,600 — and its line is
coarser over the last hour. Nothing else changes, and no tier needs
reconfiguring per host.

Phase 7 therefore needs one extra setting beyond the interval itself: the
**history cap** (see open questions). And the interval control should show its
effect on history resolution, since the two are now visibly linked.

---

## Beszel, reframed

ADR-002 called Beszel "the slow plane" and made it responsible for history.
That is no longer true: our own store covers every host at full resolution,
including hosts with no agent.

Beszel becomes **optional enrichment**:

- Absent: everything works, history spans 24 h.
- Present: history can extend *beyond* 24 h from Beszel's own 1-minute
  records, for the hosts that have an agent.

This needs a superseding ADR, since it inverts the original division of
labour.

---

## UI

The existing views stay exactly as they are. History is a third view.

A time axis is a **third dimension** on the hosts x metrics matrix, so the
history view has to pick a slice — the same orthogonal choice as before:

- **One host, many metrics** — CPU, memory, disk, network stacked over time.
  This is the Task Manager shape.
- **One metric, many hosts** — CPU across the fleet over time, lines
  overlaid or small-multiples.

Plus the per-core case from the Task Manager screenshot: **one host, one
vector metric, a small chart per core**.

Chart treatment reuses what already exists: min/max band as a translucent
gradient fill, mean as a crisp line, the same accent tokens, the same
three-band colouring where a metric is a percentage.

---

## Open questions

1. **Per-core history at every tier, or only T0?** Full cascade costs 8.1 MB
   and gives 24 h of per-core detail. T0-only costs 2 MB and gives one hour.
   The Task Manager per-core view only ever shows 60 seconds, which suggests
   an hour is already generous.

2. **Which slice does the history view default to?** One host with many
   metrics is the familiar Task Manager shape; one metric across hosts is the
   more useful thing for a fleet, and matches the Fleet view's logic.

3. **A fourth tier — 5 min x 7 days, +6.9 MB?** Cheap, but "restart clears
   it" means a week of history is unlikely to ever accumulate on a desktop app
   that gets closed.

4. **What is the cap actually expressed in?** A memory budget in MB is honest
   and easy to display; a retention span in hours is easier to reason about.
   Given 16.5 MB for a 19-host fleet, this may be a setting nobody ever
   touches.

5. **Should the window be shared across charts, or per chart?** A shared
   window means scrubbing one chart moves them all, which is right for
   correlating a spike across metrics — and is what the stock-chart model
   implies.
