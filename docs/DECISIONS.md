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

**Date:** 2026-08-16 · **Status:** **superseded by [ADR-009](#adr-009--we-own-history-beszel-is-optional-enrichment)**

> Superseded on 2026-08-22. The measurement below stands and is the reason this
> project exists — nothing here is retracted. What changed is the division of
> labour: Phase 8 built our own history store, so Beszel no longer owns the
> slow plane. Read this ADR for *why the fast plane had to be ours*, and
> ADR-009 for who owns history now.

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

---

## ADR-007 — Shell out to the system `ssh`, don't link an SSH library

**Date:** 2026-08-16 · **Status:** accepted · **Supersedes** the `russh`
dependency sketched in ADR-003

### Context

ADR-003 assumed the fast plane would use `russh`, a pure-Rust SSH
implementation. When Phase 1 came to be written, the alternative — spawning the
system `ssh` binary and reading its stdout — turned out to be strictly better
for this use case.

### Decision

`transport.rs` spawns `ssh` via `tokio::process::Command`, one long-lived
process per host, and reads framed `/proc` output from its stdout.

### Rationale

- **Every SSH feature works for free and identically to the user's terminal.**
  `~/.ssh/config` aliases, `ProxyJump`, `Match` blocks, agent forwarding,
  `known_hosts`, hardware keys, FIDO tokens, certificate auth. Reimplementing
  even half of that correctly is a project of its own.
- **The auth story becomes "whatever already works".** The rule of thumb in the
  README — *if `ssh <host>` works in your terminal, this works* — is only true
  because it is literally the same client.
- **OpenSSH ships on Windows 10+**, so there is no extra install on the one
  platform that matters most here.
- **No crypto for us to get wrong**, and no vulnerability surface we are
  responsible for patching.
- **Trivially debuggable.** Add `-v` to the args and you get the exact
  diagnostic output every sysadmin already knows how to read.

### Consequences

- One child process per host. Acceptable: it is one process, not one per
  sample, and it costs a few hundred KB.
- ssh's failure modes arrive as text on stderr, so they must be classified into
  `HostFault` by pattern-matching messages. This is done in
  `classify_ssh_error`, covered by tests, and is the one genuinely fragile part
  — OpenSSH could reword a message. The fallback is `SamplerFailed` carrying
  the raw text, so a reworded message degrades to a less specific but still
  *honest* error, never a wrong one.
- `russh` is dropped from `src-tauri/Cargo.toml`.

### Revisit when

Reviewed 2026-09-02 on the weight argument — one `ssh` process per stream — and
declined. The measurement, taken on a real `-C -T` connection streaming `/proc`
frames, is **~10 MB RSS per client**, and there are two streams per host because
`ProcSampler` opens its own connection and `start_procs` starts one for every
host rather than the focused one. Nineteen hosts: ~190 MB for the grid, ~380 MB
with the process view open. Real, and the cheaper half of it is addressed by
folding the process loop onto the metric connection rather than by changing
clients.

What russh would buy beyond that is ~1 MB per session, no child processes, and
typed errors in place of `classify_ssh_error` pattern-matching English on
stderr. What it would cost is everything under Rationale above becoming ours to
write — `~/.ssh/config`, `ProxyJump`, `known_hosts`, agent auth against
`\\.\pipe\openssh-ssh-agent` and Pageant, FIDO keys, certificates, encrypted
keys — and ~40–60 transitive crates of network-facing crypto against core's four
direct dependencies. That is the same line drawn in the `tuxtop-serve` note: we
hand-rolled base64 and refused to hand-roll an HTTP parser.

Three triggers, any one of which reopens this:

1. An `ssh` binary cannot be assumed — e.g. shipping to a locked-down
   environment without OpenSSH. The original trigger.
2. Client memory is a real complaint *after* the connection fold has landed.
3. `classify_ssh_error` misclassifies a reworded OpenSSH message and costs
   someone real diagnosis time.

Then `russh` returns as a **second** implementation behind the same interface,
feature-flagged, with the system `ssh` still the default — not a replacement.

---

## ADR-008 — Aggregates must not be able to hide a member

**Date:** 2026-08-22 · **Status:** accepted

### Context

Phase 11 groups hosts and shows per-group metrics. Every feature before it
displayed a number some machine reported; this is the first that displays a
number Tuxtop derived, and derivation is where a monitoring tool gets to be
confidently wrong on its own account rather than by repeating someone else.

A group card reading "40% CPU" can mean five hosts evenly at 40%, or one host
pinned at 100% and four idle. A mean renders both identically.

### Decision

Three rules, binding on any aggregate this app ever displays.

**1. Recombine parts; never average ratios.** A group percentage is the sum of
the numerators over the sum of the denominators, computed once at the end:
`Σ(cpu_i × cores_i) / Σ(cores_i)`, `Σused / Σtotal`. Mean-of-ratios is banned.

**2. Severity is max; magnitude is aggregate.** The value shown comes from the
aggregate, the colour band from the worst member. A group containing a critical
host is never drawn calm.

**3. No default aggregation.** Each metric declares `sum`, `max`, or a
ratio-of-sums explicitly. A metric with no declaration is *excluded* from group
views rather than averaged, because an absent rule is a missing decision and
the honest rendering of a missing decision is nothing.

An aggregate additionally carries the spread of its members and the count that
contributed, and states both.

### Rationale

dove at 100% of 32 cores beside heron at 0% of 4 is 88.9% of the group's
compute, not 50%. The mean is not an approximation of the right answer, it is a
different quantity that happens to share its units — the sort of error that
survives review because the output looks reasonable.

Rule 3 exists because rules 1 and 2 will otherwise decay: the next person to
add a metric will get a plausible number from a default they never chose. A
metric silently absent from a group view is a bug someone reports; a metric
silently averaged wrongly is a bug nobody catches.

### Consequences

The metric registry gains an `agg` field. Adding a metric stays a table entry,
but the table entry now has one more mandatory column.

Group history is aggregated on read and records how many members contributed to
each point, so a host going silent cannot move a group's line — the same
requirement as the history plane's explicit gaps, applied one level up.

A group's bar and a host's bar do share one axis, and the scale note states
what the group's bar means for the metric on screen. The first draft of this
ADR forbade a shared axis as a category error; implementing it showed the
opposite — with separate axes a host's bar rendered longer than the total of
the group containing it, which is a lie the axis note cannot repair. A `sum`
aggregate is always at least its largest member, so one axis keeps lengths
correctly ordered, and `ratio`/`max` never consult the peak at all. The hazard
to guard against is a group being mistaken for a host, which is the row
styling's job.

---

## ADR-009 — We own history; Beszel is optional enrichment

**Date:** 2026-08-22 · **Status:** accepted · **Supersedes:** [ADR-002](#adr-002--two-data-planes-beszel-for-history-direct-ssh-for-live)

### Context

ADR-002 split the work in two: Beszel owned history at 1-minute resolution, we
owned the live grid at 1 Hz. That was the right call when the alternative was
building a storage layer we did not need.

Phase 8 changed the facts. The live plane already produces a sample per host
per second; keeping it costs a four-tier cascade of ring buffers and 23.4 MB
for a nineteen-host fleet, bounded by construction at 79.9 KB per series. That
turned out to be far less work than integrating someone else's storage, and it
produces something Beszel structurally cannot: **history at the resolution the
spike actually happened at.**

### Decision

**We own history.** The in-memory cascade in `crates/tuxtop-core/src/history.rs`
is the history plane, for every host, at 1 Hz for the last hour.

**Beszel is optional enrichment, and currently unused.** The only thing it can
still offer is history beyond our seven-day ceiling, for the subset of hosts
that happen to run an agent. Nothing in the app requires it, and nothing
degrades without it.

### Rationale

The asymmetry that decided it: our store covers **every** host, because it is
fed by the same SSH connection that draws the live grid. Beszel covers only
hosts with an agent installed — which, on this fleet, is one of five.

A history plane that silently covers a subset of the fleet is worse than no
history plane. Nineteen cards where four have charts and fifteen do not is not
a monitoring tool, it is a puzzle. And the fix — install the agent everywhere —
is exactly the thing [ADR-004](#adr-004--nothing-gets-installed-on-the-monitored-host)
says we do not do.

Resolution compounds it. A 60-second bucket averaging a 100% spike down to 7%
is the failure this project was built in response to; inheriting it for the
history view would have reintroduced it one plane over.

### Consequences

- **The two-plane framing is retired.** There is one plane — ours — sampled at
  a configurable interval, of which the live grid and history are two readings.
  `docs/ARCHITECTURE.md` describes this shape.
- **History does not survive a restart**, by design, and that is now a
  first-class property rather than a gap Beszel was covering. See
  [specs/history-plane.md](specs/history-plane.md).
- **Container stats and SMART** — the things Beszel collects that we do not —
  would be better collected directly than read second-hand through a hub that
  may not be installed. Neither is currently planned.
- **Nothing to remove.** The Beszel integration was never built, so this ADR
  ratifies an absence rather than deleting code.

### Revisit when

Someone wants history older than seven days, on hosts that already run a Beszel
agent, badly enough to accept that the feature will be missing on every host
that does not.

---

## ADR-010 — Tuxtop only observes; it never changes a monitored host

**Date:** 2026-08-23 · **Status:** accepted

### Context

Phase 5 shipped the process list read-only and deferred kill and renice as
"the decision to take first", on the grounds that it would be the first
feature to *change* a remote machine rather than read it. Phase 10 deferred
systemd start/stop to the same decision rather than answer it twice.

The decision is taken: **no.**

### Decision

Tuxtop is a pure observation tool. It runs no command on a monitored host that
alters its state — no kill, no renice, no `systemctl start|stop|restart`, no
writes of any kind. Every remote command remains a read: `cat`, `awk`,
`getconf`, `df`, `systemctl list-units`, `nvidia-smi` queries.

This sits alongside [ADR-004](#adr-004--nothing-gets-installed-on-the-monitored-host)
as its natural pair. Nothing is installed on the host; nothing is changed on it
either.

### Rationale

The framing was never privilege. Tuxtop connects with the user's own SSH
credentials and grants no capability they do not already have from a terminal
— everything it could offer to do, they can do themselves in less time than
the confirmation dialog would take to read.

What it would add is **the chance to do it to the wrong machine.** The entire
design points that way: a fleet view exists so nineteen hosts look alike at a
glance, cards are packed and reorderable, groups collapse several machines
into one row. Every one of those choices is good for seeing and bad for
aiming. `kill 1` on the card you thought was `owl` is a class of mistake this
UI would actively help you make, and no amount of confirmation copy fixes a
target selected correctly and misidentified.

There is also a plainer reason: a tool that only reads cannot break anything.
"Safe by construction" has held since Phase 0 and is worth more than the
feature.

### Consequences

- **Phase 5 closes complete.** The process list is finished as read-only, and
  the drill-down is the last of it.
- **Phase 10 loses its blocking question.** systemd, if built, is a view of
  unit state — no start, stop or restart. Nothing about it is now deferred.
- **Group-level actions are settled too**, having been listed as out of scope
  in the grouping spec on the assumption the question was still open. It is
  not.
- Anything wanting to *change* a host belongs in a terminal, or in a tool that
  was designed for it. Adding a "safe" exception later reopens all of the
  above, so treat this as load-bearing rather than a default.

### Revisit when

Someone is repeatedly reaching for a terminal *while looking at Tuxtop* to do
the same small thing, and the risk of doing it to the wrong host has been
designed out rather than confirmed away.

---

## ADR-011 — A heatmap cell shows the bucket's max, not its mean

**Date:** 2026-08-24 · **Status:** accepted

### Context

Phase 12 draws the fleet as rows of coloured cells over time. A cell is a time
bucket, and at any window wider than a few minutes a bucket holds many samples:
at 1 Hz over 24 hours, one cell of a 600-column strip covers 144 samples.

Something has to reduce those samples to one colour. `Point` already carries
`min`, `mean` and `max`, so the choice is free either way — which is exactly
why it needs deciding on purpose rather than by whichever field the first
draft reached for.

**Mean is the obvious default and it is wrong here.** A host pinned at 100% for
twenty seconds inside a 144-second bucket has a mean of 14%. That renders as a
pale, unremarkable cell — a confident, well-formatted, wrong impression, and
the same arithmetic that made the Beszel agent report `0.14%` during 25% load.
This project exists because of that number. Reproducing it in our own chart,
in the view whose entire purpose is *seeing spikes*, would be the sharpest
possible own goal.

### Decision

**A heatmap cell is coloured by `max` over its bucket.** The strip answers
"did this host spike in this span", not "what was its average".

Consequences, accepted deliberately:

- **Wider windows look busier, not calmer.** Zooming out from 1 h to 24 h makes
  more cells red, because each cell now covers more chances to spike. This is
  the correct direction: the alternative is a 7-day view that looks idle
  because every spike was averaged into its neighbours.
- **A cell is not comparable to a card's number.** The card shows the latest
  sample; the cell shows the worst in its span. The view says so in words
  rather than leaving it to be inferred, the same way the log-scaled fleet bars
  state their bounds
  ([ADR-005](#adr-005--load-is-encoded-three-ways-at-once) and the log-scale
  note in ARCHITECTURE.md).
- **Hover states the bucket honestly** — `min–max, mean`, and how many samples
  it covers — so the reduction is inspectable rather than a claim.

This is the same rule as
[ADR-008](#adr-008--aggregates-must-not-be-able-to-hide-a-member), one axis
over. ADR-008 says an aggregate across *hosts* must not hide a member; this
says an aggregate across *time* must not hide a moment. Both exist because the
failure mode is a plausible number, not a missing one.

### Rejected

**Mean.** Hides exactly what the view is for. Discussed above.

**Mean, with max on hover.** The colour is the whole interface at a glance;
"available on hover" is not available. A user scanning nineteen rows for
something red never hovers the cell that looks calm.

**A user-facing mean/max toggle.** Two readings of the same chart, one of which
is misleading for the task, and a setting that silently changes what a colour
means between sessions. The strip has one job.

**p95 or similar.** Better than mean and still a reduction that can drop a
one-sample spike — at 1 Hz a 2-second spike inside a 144-sample bucket is
below p95. Also needs the raw samples, which the coarser tiers no longer hold.

---

## ADR-012 — Pause is a third host state, and it lives in `hosts.toml`

**Date:** 2026-09-01 · **Status:** accepted

### Context

A host goes down for planned maintenance. Today the only way to stop Tuxtop
turning its card red is to remove it — which calls `history.forget_host`, and
also discards its group, its interval override and its position in the grid.
Adding it back afterwards rebuilds none of that. So the honest description of
the current workaround is: *delete the record to silence the alarm.*

That leaves three questions that each have a wrong answer worth naming.

### Decision

**1. The flag is `paused: bool` on `HostConfig`, persisted in `hosts.toml`.**

Not runtime state. A maintenance window routinely outlives an app restart, and
a pause that evaporated on relaunch would be useless for the one thing it is
for — you would come back to the wall of red you paused to avoid.

`paused = true` rather than `enabled = false`, chosen for which way the field
fails. An absent bool deserialises as `false`, so a file written by an older
build, a hand-edit that drops the line, or a truncated write all mean
*watching* — the safe answer, and the one a user can see is wrong. `enabled`
would need `default = true`, and every one of those cases would instead
silently stop monitoring the entire fleet. Pinned by
`an_omitted_paused_field_means_watching`.

It is written only when true, because `hosts.toml` is hand-edited and
`paused = false` on nineteen hosts is noise in the file whose readability is
the reason it is TOML.

**2. Enforcement lives in `Supervisor::start`, not in its callers.**

`start` stops any existing task and then refuses to start a paused host. This
is the whole of the mechanism, and it is one place on purpose. Five call sites
restart a host as a side effect of doing something else — `start_all`,
`set_settings`, `set_host_interval`, `set_host_os`, `add_host` — and with the
check in the callers, any one that forgot would silently resume a host
somebody had paused. The sharpest case: `set_settings` restarts every host
whose effective interval changed, so **nudging the global sample rate would
un-pause the whole fleet**. In the supervisor that cannot happen, including
down paths not yet written. `changing_the_global_interval_does_not_resume_a_paused_host`
and `editing_a_paused_host_does_not_resume_it` fail if the check is removed;
both were confirmed by deleting it.

**3. A paused card blanks its readings; it does not freeze them.**

This is the opposite of what a *fault* does, deliberately. A faulted card keeps
its last numbers dimmed, because "it was at 90% when it died" is the most
useful thing on it. Nobody asks that about a machine they powered off
themselves, and the last sample may be days old. Leaving `42%` on the card
would be a confident, well-formatted, wrong number about a box that is not
running — the failure this project was built in response to, arriving through
our own front door. So every figure blanks, the core tiles empty, the
sparkline buffers clear, and no chart is drawn.

For the same reason a paused host is **counted apart from the ones that are
up**, never folded into them: pausing a dying box must not make the fleet
report itself healthier than it was a moment before. It is silent in group
aggregates too, exactly like a faulted host ([ADR-008](#adr-008--aggregates-must-not-be-able-to-hide-a-member)),
rather than contributing a zero that would drag its group's average down and
show a fleet-wide dip that never happened.

The dot gets its own neutral colour rather than reusing the warn colour, which
would claim something is wrong, or the default, which would claim everything
is fine. Three states, not two.

### This is not an exception to ADR-010

[ADR-010](#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host) says
Tuxtop never changes a monitored host. Pause changes *Tuxtop* — it stops us
connecting. Nothing is sent to the machine; strictly less is. The aiming
argument that rules out `kill` does not apply to a control whose entire effect
is that we stop talking to something.

### Rejected

**Runtime-only pause.** Simplest, and wrong for the reason above: maintenance
outlives the session.

**`paused_until = <timestamp>`, with automatic resume.** File-compatible as a
later widening, and still not wanted. Maintenance windows overrun, so a timer
that fires on schedule repaints the wall red at exactly the moment nobody
wants it — and the user then has to work out whether the red is the old
maintenance or a new fault, which is worse than the state they were in.

**Resuming automatically when the host answers again.** Requires probing a
host we have said we are not talking to. Pause means no connection; a
background reachability check is a connection.

**Group-level pause** — "pause the whole `physical` group". The obvious next
ask, and a widening of this same field, but a separate decision: it is exactly
the shape of aiming ADR-010 is careful about, and one click that stops
watching nine machines deserves its own argument.

**Hiding paused hosts from the grid.** Cheaper on space and it makes a paused
host easy to forget. The card stays in place, dimmed, keeping the position
that is muscle memory — and its resume button stays visible without a hover,
since hiding the way out of a state is how a state becomes a trap.

---

## ADR-013 — A Windows remote loop watches its sshd session, not its pipes

**Date:** 2026-09-01 · **Status:** accepted

### Context

Killing the local `ssh` client does not stop the loop on the far side of a
Windows host. On Linux it does — sshd hangs up and the shell dies with SIGHUP,
checked against dove — so this is a Windows-only defect and it went unnoticed
for the same reason: nothing on the Linux path can reproduce it.

The result is one leaked `powershell.exe` per launch, running until the host
reboots. Three had accumulated on n1 and were found only because they were
*also* polling at 5 ms — the unit bug fixed alongside this — and had taken
about three of sixteen cores. At the correct interval an orphan is cheap and
therefore silent, which is worse: it accumulates and nothing points at it.

`verify.sh` counts local `ssh` processes after killing the app and reports
them gone. That is true and irrelevant — the local client always dies. The
remote loop it started is invisible to every gate we have.

### Decision

**The loop finds sshd's per-connection session process at startup and exits
when that process goes.**

sshd forks a session process per connection. When the client dies, that process
exits while the `cmd.exe` and `powershell.exe` beneath it survive — it is the
one link in the chain that reliably breaks. The script walks its own ancestors
once, records the PID, and checks it with `GetProcessById` (a handle open, not
a WMI query) inside the sleep. Measured on n1: reaped 3.4 s after the client
exits, with 45 consecutive samples over 99 s beforehand and no false kill.

The check rides *inside* the interval sleep, in steps of at most a second,
rather than once per cycle. Otherwise a host sampled hourly holds its orphan
for the rest of the hour — sampling slowly should cost the host less, not more.

If no `sshd.exe` ancestor is found the watchdog is disabled rather than firing,
because the dangerous error is killing a live session, not keeping a dead one.
A 30-minute lifetime cap then applies, so *"a remote loop never runs forever"*
holds unconditionally instead of only when the ancestor walk succeeds.

### Rejected

**Noticing a broken stdout.** The obvious approach, and it does not work:
writes keep succeeding after the client is gone. Windows sshd leaves the pipe
intact.

**Waiting for stdin to reach EOF.** Same reason — stdin never closes either.

**A heartbeat written down stdin, with the loop exiting when it stops.** This
was built and it appeared to work, which is why it is worth recording rather
than merely rejecting. `[Console]::OpenStandardInput()` returns a
`System.IO.__ConsoleStream` whose `ReadAsync` only ever completes with bytes
that were *already buffered when it was called*; a read issued against an empty
pipe stays `WaitingForActivation` forever, even after data arrives. A blocking
`Read` on a second runspace does not wake either.

So the mechanism half-worked: with heartbeats faster than the poll there was
usually data already waiting, and a short test passed. At the real 5 s cadence
every Windows host died 30 s in. It was caught by soaking a session for longer
than the lease, and by nothing else — the unit tests asserted on the generated
script text and were all green. A test that only reads what we *meant* to send
cannot see that the far side never receives it.

**Killing stale loops on connect.** Self-healing, and it would clean up the
orphans already out there, but it cannot distinguish an abandoned loop from one
belonging to a second person watching the same fleet. Reaching onto a monitored
host to kill a process is also exactly what ADR-010 forbids.

**A bounded lifetime alone, with the client reconnecting.** Simple and
unconditional, but it trades a permanent orphan for a sampling gap on every
host forever. It survives here only as the fallback for the case where the
watchdog cannot arm, where the alternative is the original bug.

## ADR-014 — One connection per host carries both planes

**Date:** 2026-09-02 · **Status:** accepted · **Supersedes** the separate
`ProcSampler` connection introduced with the process list

### Context

The process view had its own `ssh` per host. It was started fleet-wide the
moment anyone opened the view — `start_procs` iterated every host, not the one
being looked at — so nineteen hosts went from nineteen connections to
thirty-eight. Measured on a real `-C -T` connection streaming `/proc` frames,
each client is **~10 MB RSS**: ~190 MB became ~380 MB, on the Windows box
somebody is using.

The reason recorded in the code was that the ranking needs two snapshots a
second apart, and taking them inside the metric loop would stall 1 Hz sampling
for the whole window. Measured, that reason does not hold. On a 32-core host
with 629 processes the snapshot costs **9.5 ms**; the second is a *window*, not
work — and the metric loop is already sleeping exactly that window, every
iteration.

### Decision

One `ssh` per host. The metric loop opens the ranking window on one iteration
(`i % emit_every == 0`) and closes it on another (`i % emit_every == win_back`),
so the window is the sleep the loop was already doing. `procs::proc_schedule`
derives both numbers from the sample interval.

The two planes are told apart on the wire by **two delimiters**, never by their
line tags.

### Rationale

- **Nineteen fewer processes and ~190 MB of client RSS** with the process view
  open, and nineteen fewer sshd sessions, TCP connections and auth handshakes
  on the fleet.
- **No added remote cost.** It is the same two snapshots per five seconds that
  the second connection was already taking.
- **Pause is enforced in one place again.** `start_procs` was the second way a
  host acquired an ssh connection, and it needed its own `cfg.paused` check —
  precisely the shape [ADR-012](#adr-012--pause-is-a-third-host-state-and-it-lives-in-hoststoml)
  warns about. It is gone.
- **The process list is populated when the view opens**, not five seconds
  later, because it was already arriving.
- On Windows the saving is larger: the second connection was a second
  **PowerShell runtime** resident on the monitored workstation — and, until
  [ADR-013](#adr-013--a-windows-remote-loop-watches-its-sshd-session-not-its-pipes),
  a second orphan per launch. One script means the watchdog has one loop to
  reap rather than two.

### Why two delimiters and not one

Both parsers scan for their own line prefixes and skip everything else, which
makes a single merged frame look free. It is not. `parse_net_dev` splits on the
first `:` and `is_whole_disk` is a denylist, so one process command line —

```text
TXC|1481|/usr/bin/java -Xms512m -Xmx4096m -XX:+UseG1GC -Dserver.port 8080 -Dworkers 16 -jar /opt/app.jar
```

— parses as a disk named `-Xmx4096m` that read 8080 sectors: **4.1 MB of I/O
that never happened**, no error, plausible number. That is the failure this
project was built in response to, so the guard is that the parser never sees
the line. Verified by test rather than argued: the tags genuinely collide too
(`TXG|` is a GPU in a metric frame and a cgroup in a process frame).

### Consequences

- **The process plane is always on.** There is no longer a command to switch it
  off, and `set_processes_enabled` is deleted rather than left as a no-op — a
  control that cannot change anything looks like a capability. The cost paid by
  a host nobody is looking at is ~25 ms of remote CPU per five seconds, about
  0.5% of one core, against 0.016% of a 32-core box.
- Turning it back into something switchable would mean restarting every metric
  connection on a view toggle, which blips every card: a fresh `RateTracker`
  has nothing to differentiate against. The alternative considered was a
  make-before-break swap, and it was judged more complexity than the saving is
  worth.
- Concurrent writers in one remote shell were rejected outright: a metric frame
  is 12,430 bytes, well over `PIPE_BUF`, so two writers splice mid-frame — and
  a backgrounded loop that is not writing to stdout never takes `SIGPIPE`, so
  it outlives the connection on a machine we promised only to observe.

### Revisit when

A host is watched fast enough that 9.5 ms per snapshot stops being noise, or
somebody wants the process plane genuinely off. The schedule already stretches
its cadence at slow intervals; making it *stop* needs the make-before-break
swap above.
