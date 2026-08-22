# Spec — grouping hosts

**Status:** design, nothing built · **Phase:** 11 · **Depends on:** the metric
registry (landed), the history plane (Phase 8)

**Goal:** a fleet of nineteen reads as five things. Group hosts by role, site
or cluster, and show a group's metrics as one row that expands to its members.

---

## The trap this feature is

Every other feature in Tuxtop shows a number a machine reported. This one shows
a number **Tuxtop computed**, and that is a different risk class entirely.

A group card reading "40% CPU" can mean five boxes evenly at 40%, or one box
pinned at 100% and four idle. Those are opposite situations — one is a healthy
cluster, the other is an outage in progress — and a mean renders them
identically. That is precisely the failure this project was built in response
to: a confident, well-formatted, wrong number.

So the design rule comes before the mechanics:

> **An aggregate must never be able to hide a member.**

Everything below follows from that.

---

## Decision 1 — a group is one optional label per host

```toml
[[hosts]]
name = "dove"
group = "workstations"     # optional; absent means ungrouped
```

Rejected for now:

- **Multiple labels per host.** Composes better, and immediately raises
  questions worth nothing today: does a host in two groups appear on two cards?
  Does the fleet total double-count it? Does killing one group's view stop a
  sampler another group still needs?
- **A tree.** Matches how people describe infrastructure out loud
  ("nuremberg → prod → web"), but a tree needs path syntax, collapse state per
  level, and a rule for what a parent's metric means when children overlap.

A single label is a degenerate case of both, so `hosts.toml` stays
forward-compatible: `group = "x"` can widen to `groups = ["x"]` without
breaking existing files.

**Revisit when** a host genuinely belongs in two groups at once and someone
notices its absence from one of them — not before.

Ungrouped hosts are not put in a synthetic "Other" group. They render exactly
as they do today, beside the group cards. A fleet with no groups configured
must look and behave precisely as it does now.

---

## Decision 2 — percentages aggregate by recombining parts, never by averaging ratios

This is the whole technical content of the phase.

A group's CPU percentage is **not** the mean of its members' percentages. It is
the group's busy core-time divided by the group's total core-time:

```
group cpu%  =  Σ(cpu_i × cores_i) / Σ(cores_i)
```

Consider dove (32 cores, 100%) and heron (4 cores, 0%):

| method | result | what it implies |
|---|---|---|
| mean of ratios | 50% | half the group's compute is busy |
| ratio of sums | **88.9%** | 32 of 36 cores are pinned |

The second is true; the first is a well-formatted lie. The same applies to
memory (`Σused / Σtotal`), GPU memory, and every other ratio. **Mean-of-ratios
is banned in this codebase.** Where a metric is already a ratio, aggregate the
numerator and denominator separately and divide once at the end.

### Per-metric aggregation

Each registry entry gains an `agg` declaration. Adding a metric stays a table
entry, as it is today.

| metric | aggregate | why |
|---|---|---|
| `cores` | concat | the group's cores, one grid. 108 tiles for five hosts |
| `cpu` | ratio of sums, weighted by cores | above |
| `mem` | ratio of sums (bytes) | a 128 GB box and a 4 GB box are not equal votes |
| `swap` | **max** | swap in use anywhere is the signal; averaging it away is the bug |
| `fs` | **max** | one full disk is the problem, not the group's mean fullness |
| `temp` | **max** | averaging temperatures is meaningless. One throttling box must not be cooled by four idle ones |
| `disk`, `net` | **sum** | rates add. Bytes per second across the group is a real quantity |
| `load` | **sum** | runnable processes add. Log scale already carries the magnitude |
| `gpu` | ratio of sums, weighted by GPU count | |
| `gpumem` | ratio of sums (MB) | |

Three different answers — sum, max, ratio-of-sums — and no default. A metric
added without an explicit `agg` must be **excluded from group views**, not
silently averaged. An absent aggregation rule is a missing decision, and the
correct rendering of a missing decision is nothing at all.

---

## Decision 3 — severity is max, magnitude is aggregate

A group card draws its **value** from the aggregate and its **colour band**
from the worst member.

A group whose aggregate CPU is 40% but which contains a host at 97% renders
red, not green. The number answers "how loaded is this group"; the colour
answers "is anything wrong in here". Those are different questions and the
card must not let one mask the other.

This is the existing three-way encoding of [ADR-005](../DECISIONS.md#adr-005--load-is-encoded-three-ways-at-once)
extended one step: fill height from the aggregate, colour band from the
member maximum, cap line at the aggregate.

## Decision 4 — every group shows its spread

The aggregate alone is insufficient by the rule at the top. Each group bar
carries a **member-range whisker**: a light track from the minimum member value
to the maximum, with the aggregate marked on it.

```
  workstations   ├──────●────────────┤   58%     min 12%  ·  max 97%
                 12%    58 (aggregate) 97
```

A tight group and a group tearing itself apart are then distinguishable at a
glance, without expanding it. For `sum` metrics the whisker shows the largest
single contributor instead, which answers the question people actually ask of a
total ("who is most of this?").

---

## Decision 5 — history aggregates on read

Group series are computed from member series at query time, not stored.
Cheaper, and it cannot drift from the members it claims to summarise. It also
means changing a host's group re-labels its history instead of orphaning it.

**How the weight is obtained.** *(Noted during 11c.)* History stores a
percentage, not the numerator and denominator it came from, so a `ratio`
aggregate cannot recombine parts from the past. It weights by the host's
*current* size instead — cores for CPU, gigabytes for memory, taken from the
same `parts` denominator the live view uses. Exact for a physical box, and near
enough for anything whose core count changed inside the seven-day ceiling.

**The gap problem.** Members do not have identical gaps. If a group's CPU is
averaged over whichever members happened to report, a host going silent makes
the group's line move for a reason that has nothing to do with load — a silent
box drops out of the denominator and the group appears to change.

So every aggregated point carries the number of members that contributed, and:

- The chart marks any span where the contributing count is below the group's
  size, the same way a single host's gap is drawn explicitly today.
- The tooltip states it: `58% · 4 of 5 hosts reporting`.

A group summarising four hosts while claiming to summarise five, silently, is
the same bug as a 60-second bucket averaging away a spike.

---

## Rendering

- A group is a **collapsible block**: one row when collapsed, its member cards
  when expanded. Collapse state persists in `localStorage` alongside the other
  view preferences, not in `hosts.toml` — *(revised during 11b)*. It is view
  state of exactly the same kind as the current view, sort order and selected
  metric; putting one such preference in the config file and the rest in
  `localStorage` would split one concept across two stores for no gain.
- Groups are the natural boundary for the fleet view's block packing, which
  should **simplify** the current layout rather than complicate it — the packer
  gains a real grouping key instead of inferring one from core counts.
- **One axis, and the row says what its bar means.** *(Revised during 11b —
  the original rule was wrong.)* This spec first called for separate axes for
  groups and hosts, on the grounds that a group total and a single host are
  not comparable. Building it disproved that: with two axes, dove's 1.1 MB/s
  bar rendered **longer** than the 2.3 MB/s total of the group it belongs to.
  A footnote explaining the discrepancy does not repair a picture that lies.

  A shared axis is in fact the honest one, because a `sum` aggregate is always
  at least as large as its largest member, so lengths stay correctly ordered —
  longer really does mean more bytes. `ratio` and `max` sit on an absolute
  0–100 scale that ignores the peak entirely. The real hazard was never the
  axis but a group being *mistaken* for a host, which row styling and the
  stated composition prevent.

  What the scale note must still do is say what a group's bar means, since it
  differs per metric and cannot be read off the picture: "a group row shows the
  total across its hosts / its highest host / a share weighted by size, banded
  by its worst host."
- A group's header states its composition: `workstations · 3 hosts · 68 cores`.
  A card that will not say what it is made of cannot be checked.

---

## Exit criteria

- [ ] `cargo test` passes, including aggregation tests
- [ ] A test named after the rule — `mean_of_ratios_is_not_the_group_percentage`
      — pins the dove/heron 88.9%-not-50% case
- [ ] A test pins that a group containing a 97% host renders in the critical
      band while its aggregate is 40%
- [ ] A test pins that a metric with no `agg` declaration is absent from group
      views rather than defaulted
- [ ] Assigning `group` to hosts in `hosts.toml` produces collapsible group
      blocks; a file with no groups renders identically to today
- [ ] A group's history chart marks spans where a member was silent, and the
      tooltip states how many contributed
- [ ] Verified against the real five-host fleet with two groups

---

## Out of scope

- **Group-level alerting.** Alerting is a project-wide non-goal.
- **Nested groups**, until a single level is proven insufficient.
- **Automatic grouping** by subnet, hostname pattern or discovered role.
  Guessing a grouping and being wrong is worse than asking for one.
- **Group-level actions** — killing a process across a group, restarting a unit
  on every member. That is a fleet orchestration tool, and it is not this.
