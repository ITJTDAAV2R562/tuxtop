# CLAUDE.md — Tuxtop

Standing rules for any session working in this repo. Read
[docs/DECISIONS.md](docs/DECISIONS.md) before proposing an architectural
change — the reasoning is recorded there with the alternatives that were
rejected and why.

---

## What this is

A Windows-native Task Manager for remote Linux hosts. Tauri 2 + Rust backend,
HTML/CSS frontend. Two data planes: live SSH sampling at 1 Hz, and Beszel's
API for 1-minute history.

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
If a feature seems to need one, it belongs in the slow plane via Beszel, or it
does not ship. [ADR-004](docs/DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host).

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

## Frontend rules

**All colours come from CSS custom properties.** No literal hex, `rgb()` or
`hsl()` in component rules. Hardcoded colours do not survive the theme switch
and typically break in only one direction, which makes them easy to miss.

**Every colour must be defined in all three theme states.** Bare `:root` carries
the complete light palette; `@media (prefers-color-scheme: dark)` guarded as
`:root:not([data-theme="light"])` redefines the tokens; `:root[data-theme="dark"]`
redefines them again. A token defined in only one of the three renders one
theme's text on the other theme's ground. This has already been caught once —
check all three when adding a token.

**Any numeral drawn over a load-coloured surface needs a contrast halo.** The
background colour is unpredictable by definition. Use `--tile-halo`.
[ADR-005](docs/DECISIONS.md#adr-005--load-is-encoded-three-ways-at-once).

**Check both themes before committing a visual change.** A change verified in
dark only is unverified.

---

## Testing

```sh
cargo test        # 33 tests, no GUI toolchain needed, runs anywhere
cargo clippy --all-targets
cargo fmt
```

**Tests are the memory this project does not otherwise have.** Each test name
states the invariant it protects — `iowait_counts_as_idle`,
`partitions_are_not_double_counted`, `identical_samples_report_zero_not_nan`.
Write names that way: a future session should learn the rule from the failure
message alone.

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
