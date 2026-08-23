# Spec — History plane (Phase 8)

**Status:** **built** · **Phase:** 8 · Shipped 2026-08-22

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
  role as "the slow plane" in ADR-002 (see ADR-009).
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
| T3 | 5 min | 7 days | 2,016 | 12 B (min/mean/max) | 23.6 KB |

**Measured against the real fleet** — 19 hosts, 148 cores, 8 scalar metrics,
with per-core carried through every tier:

```
scalar series (19 x 8 = 152)  11.9 MB
per-core          (148)       11.5 MB
TOTAL                         23.4 MB
```

Keeping every 1 Hz sample for 24 h would be **99 MB**, for a quarter of the
span. A series occupies **79.9 KB, bounded by construction**, whatever
happens.

At 23.4 MB the whole fleet costs less than a browser tab, so **memory is not a
design constraint at this scale**. It becomes one around 100 hosts (~125 MB),
which is what the cap is for. Worth stating plainly so the cap does not get
over-designed for a limit nobody here will reach.

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
including hosts with no agent. Ratified in
[ADR-009](../DECISIONS.md#adr-009--we-own-history-beszel-is-optional-enrichment).

Beszel becomes **optional enrichment**:

- Absent: everything works, history spans 24 h.
- Present: history can extend *beyond* 24 h from Beszel's own 1-minute
  records, for the hosts that have an agent.

This needs a superseding ADR, since it inverts the original division of
labour.

---

## UI

The existing views stay exactly as they are. History is a third view — but it
does **not** have a default slice, because it inherits one.

### History inherits its slice from wherever you entered

A time axis is a third dimension on the hosts x metrics matrix, so the history
view has to pick a slice. Rather than making that a control the user sets, it
comes from context:

| entered from | history shows |
| --- | --- |
| **Hosts** view, clicking a host | that host, many metrics — the Task Manager shape |
| **Fleet** view, with a metric selected | that metric, every host |
| **Fleet** view, clicking one host's block | that host, many metrics |
| the History tab directly | whatever slice was last shown |

The user is already looking at a host or a metric when they ask for history,
so the slice they want is the one already on screen. Making them re-pick it
would be asking a question the app can already answer.

The slice remains **switchable once inside** — inheriting a default is not the
same as being locked to it — but it should rarely need touching.

### Window

**Shared across every chart on screen.** Scrubbing one moves them all, which
is the entire point when correlating a spike: seeing that the CPU jump and the
disk jump are the same second is the question being asked. A per-chart window
would make that comparison manual and error-prone.

### Chart treatment

Reuses what exists: min/max band as a translucent gradient fill, mean as a
crisp line over it, the same accent tokens, and the same three-band colouring
where a metric is a percentage.

The per-core case gets the Task Manager shape — one small chart per core, in
the same packed-block layout the fleet view already uses, so a 32-core host
and a 2-core host stay comparable.

---

## Open questions

1. **Cap units.** A memory budget in MB is honest and directly displayable;
   a retention span in hours is easier to reason about. Leaning MB, shown
   beside live usage, since the tiers already express the span.

2. **Eviction order when the cap is hit.** Dropping the coarsest tier first
   loses the most span for the least memory; dropping per-core first keeps
   whole-host history intact. Probably per-core T3 first, then T3, then T2.
