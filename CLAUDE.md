# CLAUDE.md — Tuxtop

Standing rules for any session working in this repo. Read
[docs/DECISIONS.md](docs/DECISIONS.md) before proposing an architectural
change — the reasoning is recorded there with the alternatives that were
rejected and why.

---

## What this is

A Windows-native Task Manager for remote Linux hosts. Tauri 2 + Rust backend,
HTML/CSS frontend. One data plane: SSH sampling at a configurable interval
(1 Hz default), feeding both the live grid and an in-memory history cascade.

---

## The central hazard: plausible wrong numbers

This project exists because a monitoring agent reported `0.14%` while a host
was at 25% load, and `21.95%` for 13 seconds after the load had stopped. No
error, no crash, no warning — a confident, well-formatted, wrong number.

That is the failure mode to design against. It shapes several rules below.

**A number that looks reasonable is not evidence that it is right.** When you
touch sampling maths, check it against independent ground truth (`top`,
`htop`, `/proc` read by hand) — not against whether the output looks plausible.

---

## Hard rules

**Never let the parser see a partial frame.**
A truncated `/proc/stat` parses successfully into a wrong snapshot. `split_frames`
returns only complete frames and buffers the tail. Do not "simplify" this by
parsing whatever has arrived.

**CPU percentage is always a delta of two samples.**
Never report an instantaneous value. Specifically:
- `iowait` counts as **idle** — otherwise an NFS stall reads as 100% CPU.
- `guest`/`guest_nice` are **excluded** from the total — the kernel already
  counts them inside `user`/`nice`, so adding them double-counts.
- Memory uses **`MemAvailable`**, never `MemFree` — `MemFree` reports a healthy
  Linux box as nearly out of memory.

Each of these is pinned by a named test. If you change one, the test name tells
you what invariant you are breaking.

**One SSH connection per host, held open.**
Never spawn a process or open a connection per sample. A handshake per second
costs more than the interval it is measuring.

**Nothing is installed on the monitored host.**
No agent, no binary copied over, no package, no open port, no firewall change.
If a feature seems to need one, it is collected over the existing SSH stream or
it does not ship. [ADR-004](docs/DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host).

**Nothing is *changed* on the monitored host either — Tuxtop only observes.**
No kill, no renice, no `systemctl start|stop|restart`, no writes. Every remote
command is a read. This was decided deliberately, not left undone:
[ADR-010](docs/DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host).

The argument is not privilege — Tuxtop uses the user's own SSH credentials and
grants no capability a terminal does not. It is **aiming**. This UI exists to
make nineteen machines look alike at a glance; that is good for seeing and bad
for targeting, and `kill 1` on the card you thought was another host is a
mistake it would help you make. A confirmation dialog does not help when the
target was selected correctly and misidentified.

If a future phase seems to want a "safe" exception, it reopens Phase 5's
process actions, Phase 10's start/stop and group-level actions all at once.
Treat it as load-bearing.

**Never let one data plane's absence blank a card.**
No Beszel agent means no history — the live grid still works. SSH down means no
live data — history still renders, marked stale. Always state which part is
missing; never show a generic "offline".

**Faults keep their reason.**
`HostFault` distinguishes `Unreachable`, `AuthFailed`, `SamplerFailed` and
`Stalled`. Do not collapse them. Telling `AuthFailed` from `Unreachable` is the
difference between a thirty-second fix and an hour of guessing.

**No silently-swallowed errors.**
Every error path either logs with context, propagates, or renders as a
`HostFault` the user can see. A `unwrap_or_default()` that hides a parse failure
reproduces exactly the bug this project was built in response to. The one
sanctioned exception is per-field parsing inside `/proc` parsers, where a
malformed field degrades that field to zero rather than discarding the whole
snapshot — that is deliberate and documented at each site.

**`src-tauri` is not a workspace member — keep it that way.**
Tauri pulls webkit2gtk on Linux, which is absent on the WSL dev box. Adding it
to the workspace breaks `cargo test` on the machine where development happens.
[ADR-006](docs/DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace).

**No blocking I/O in async tasks.**
One Tokio task per host. A host that hangs must degrade only its own card.

---

## A feature is not done until you can name the click path

Say, in one sentence, what a user clicks to reach the thing you built. If you
cannot, it is not finished — however green the tests are.

```sh
python3 scripts/check-commands-reachable.py
```

Three commands had shipped with no caller before anyone noticed:

- `set_host_group` — host grouping was reachable only from the Add host
  dialog, so it worked for a host that did not exist yet and for no other.
  Every host already in the fleet had no path to the feature at all.
- `history_usage` — the settings panel multiplied a hardcoded 79.9 KB by a
  series count and reported the product as fact, while the measured figure sat
  in the backend unused.
- `active_hosts` — dead outright.

Each was individually invisible: the Rust compiled and was tested, the JS ran,
and neither side can see that the other never calls it. This is the failure
mode of building a backend and a frontend in the same sitting, and it is worth
a check rather than care.

**A new field on `HostConfig` needs a control, not just a parser.** The
reachability check above covers commands and cannot cover fields: `HostFacts`
also has an `os`, so nothing automated can tell `cfg.os` from `facts.os`.
Host `os` shipped with a backend, a `hosts.toml` entry and a documented
example — and no way to set it, so a Windows host added through the UI was
created as a Linux one and failed with "the system cannot find the path
specified", an error explaining nothing. When adding a field, wire **both**
paths: the Add host dialog for new hosts, and the per-host table in Settings
for the ones that already exist. The second is the one that gets forgotten,
and it is the one that matters for a fleet already running.

Two related habits:

- **Prefer a measurement to an estimate whenever one exists.** This project was
  built in response to a confident wrong number; computing a plausible figure
  in the frontend while the real one is a command away is the same sin in
  miniature.
- **When a stub command is missing, fix the stub.** Twice now a gap in
  `tests/harness/stub.js` has presented as an application bug. The stub is test
  infrastructure — an error there costs debugging time and teaches nothing.

---

## Frontend rules

**All colours come from CSS custom properties.** No literal hex, `rgb()` or
`hsl()` in component rules. Hardcoded colours do not survive the theme switch
and typically break in only one direction, which makes them easy to miss.

**Every colour must be defined in all three theme states — and there is a checker.**

```sh
python3 scripts/check-theme-tokens.py
```

Run it after touching tokens. This bug has landed twice (`--viz-*`, then the
metal/reveal set): tokens go into the `@media` block and miss
`:root[data-theme="dark"]`, so an explicit dark toggle silently falls back to
light colours. It fails in one direction only, which is why eyeballing misses it.

The three states: bare `:root` carries the complete light palette;
`@media (prefers-color-scheme: dark)` guarded as `:root:not([data-theme="light"])`
redefines the tokens; `:root[data-theme="dark"]` redefines them again. A token
defined in only some of them renders one theme's colour on another theme's
ground.

**Any numeral drawn over a load-coloured surface needs a contrast halo.** The
background colour is unpredictable by definition. Use `--tile-halo`.
[ADR-005](docs/DECISIONS.md#adr-005--load-is-encoded-three-ways-at-once).

**Check both themes before committing a visual change.** A change verified in
dark only is unverified.

---

## Testing

```sh
cargo test        # 136 tests, no GUI toolchain needed, runs anywhere
cargo clippy --all-targets
cargo fmt
node --test 'tests/*.test.js'           # group aggregation rules (ADR-008)
python3 scripts/check-theme-tokens.py   # CSS tokens in all three theme states
python3 scripts/check-agg-declared.py   # every metric declares how it aggregates
python3 scripts/check-commands-reachable.py   # no command shipped unreachable
```

**Tests are the memory this project does not otherwise have.** Each test name
states the invariant it protects — `iowait_counts_as_idle`,
`partitions_are_not_double_counted`, `identical_samples_report_zero_not_nan`,
`mean_of_ratios_is_not_the_group_percentage`. Write names that way: a future
session should learn the rule from the failure message alone.

**JS tests use node's built-in runner** (`node --test`), which needs no npm,
no package.json and no bundler — matching how the frontend is already built.

**Pure logic goes in a module beside `app.js`, not inside it.** `app.js` keeps
the DOM; everything that decides a *value* lives in a UMD module and is tested:

| module | decides |
| --- | --- |
| `src/format.js` | numbers → the strings on screen |
| `src/scale.js` | value → position, band, column count |
| `src/pick.js` | which of a host's many readings matters (fullest mount, hottest sensor) |
| `src/filter.js` | what to show and in what order |
| `src/agg.js` | how a group combines (ADR-008) |

Load order in `index.html` matters: the modules come first and `app.js` binds
them at the top of its IIFE. Adding logic to `app.js` that could be in a module
means adding logic nothing can test — the frontend went 2,792 lines with zero
coverage that way, and shipped two bugs in one session because of it.

**When a rule matters, check that breaking it fails the test.** A test written
against already-correct code can pass for the wrong reason. Both aggregation
rules in ADR-008 were verified by mutation: turning the ratio into a mean, and
taking severity from the aggregate, each failed exactly the tests named after
those rules.

`crates/tuxtop-core/tests/real_host.rs` checks parsing against captured
`/proc/stat` from a real 32-core host and cross-checks the computed percentage
against `top` from the same second. When you change the maths, this is the test
that catches a plausible-but-wrong result.

---

## Commit style

- Subject ≤70 chars, imperative, no trailing period.
- Body explains *why*, wrapped at 72.
- Prefixes: `feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `chore:`.
- No `Co-Authored-By:` trailer.

---

## Keeping docs true

A decision that changes behaviour, an interface, or a settled approach must
update the affected doc **in the same commit**:

- [docs/DECISIONS.md](docs/DECISIONS.md) — supersede the old ADR, don't delete
  it. The rejected path is the valuable part.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) — if data flow or layout moved.
- [docs/ROADMAP.md](docs/ROADMAP.md) — tick phases only when the observable
  outcome is verified, not when the code exists.

Two sources disagreeing is worse than one source missing.
