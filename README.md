# Tuxtop

A Windows-native Task Manager for a fleet of remote machines.

Live per-core CPU, memory, disk, network, load, temperature and GPU for every
host at once, in a real Win11 Mica window — not a browser tab, not a terminal,
not a web dashboard you install as a PWA and pretend is an app. **Nothing is
installed on the machines it watches**: SSH and a POSIX shell is the whole
requirement.

**The goal is to see spikes across a fleet, immediately and beautifully.** It
is a *seeing* tool, not a *watching* one: it does not run unattended, remember
anything across restarts, or tell anyone when something breaks. Those are jobs
for Uptime Kuma, Pulse or Proxmox, and they are deliberately out of scope.

> **Status: released.**
> [**v0.2.0**](https://github.com/UZ1sFED3yS/tuxtop/releases/latest) ships a
> Windows installer and a headless Linux server. Twelve phases are done — live
> grid, history, processes, host facts, groups, Windows hosts, the Heat view.
> What is still open is in [docs/ROADMAP.md](docs/ROADMAP.md).

---

## Why this exists

`btop` over SSH works but is a terminal. Netdata, Beszel and Cockpit are web
dashboards. MobaXterm has a small per-session sidebar. XPipe manages
connections. [TuxManager] is Qt6 and local-only — no SSH, no multi-host.

None of them puts a live per-core grid for a whole fleet in a native Windows
window. That is the whole gap.

The nearest thing, Beszel, is genuinely good and cannot go fast: measured, its
agent reported idle through 26 seconds of sustained 25% load, then reported 22%
for 13 seconds after the load stopped, serving byte-identical cached arrays.
The interval is not configurable — it is what makes the agent cost 23 MB.

**A monitoring agent that caches cannot show you a spike**, so Tuxtop samples
its own. That measurement is the reason this exists:
[docs/evidence/beszel-cadence.md](docs/evidence/beszel-cadence.md).

[TuxManager]: https://github.com/benapetr/TuxManager

---

## How it works

One data plane, and it is ours:

- One persistent SSH connection per host, running a POSIX `sh` loop that cats
  `/proc` at the configured interval. Per-core CPU, memory, swap, disk I/O and
  capacity, network, load, temperatures, GPU, uptime and host identity.
  **Nothing is installed on the monitored host** — no agent, no root, no open
  port, no firewall change. Nothing is *changed* there either: every remote
  command is a read ([ADR-010](docs/DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host)).
- The live grid and the history charts are two readings of that same stream.
  History is kept in memory in a four-tier cascade, so a spike is still visible
  at the resolution it happened at.
- The stream is compressed. Consecutive `/proc` frames are nearly identical, so
  `ssh -C` takes this fleet from 7.55 GB/day to 0.73 — measured by ssh itself.

A host that goes silent keeps its history, marked stale, and its card states
why it went quiet rather than showing a generic "offline".

**Windows hosts work too**, over Windows' own OpenSSH Server, with no agent
there either. They are asked for rather than probed — guessing wrong fails in a
way the error cannot explain.

Beszel is supported as optional enrichment for history beyond seven days, on
hosts that happen to run its agent. Nothing requires it
([ADR-009](docs/DECISIONS.md#adr-009--we-own-history-beszel-is-optional-enrichment)).

Full picture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
Reasoning, with rejected alternatives: [docs/DECISIONS.md](docs/DECISIONS.md).

---

## The views

The fleet is a matrix of **hosts × metrics**, and time is its third axis.

| view | slice |
| --- | --- |
| **Hosts** | a row: one card per box, every metric for that box |
| **Fleet** | a column: one metric, every box, right now |
| **Heat** | that column over time: one metric, every box, the whole window |
| **History** | a few subjects in depth, min/mean/max |
| **Processes** | what is actually running, ranked, across the fleet |

Load is encoded three ways at once — fill height, colour band, and a crisp cap
line at the leading edge — so a hot core registers peripherally, before you read
any digits ([ADR-005](docs/DECISIONS.md#adr-005--load-is-encoded-three-ways-at-once)).

**A Heat cell shows the peak of its slice, never the mean.** A host pinned at
100% for twenty seconds inside a two-minute bucket has a mean of 14%, which is
the same arithmetic that made the Beszel agent report 0.14% during real load
([ADR-011](docs/DECISIONS.md#adr-011--a-heatmap-cell-shows-the-buckets-max-not-its-mean)).

Hosts can be grouped, and a group **aggregates without being able to hide a
member**: percentages recombine their parts rather than averaging the ratio, and
severity comes from the worst member
([ADR-008](docs/DECISIONS.md#adr-008--aggregates-must-not-be-able-to-hide-a-member)).

---

## Install

**[Latest release](https://github.com/UZ1sFED3yS/tuxtop/releases/latest)**

### Windows desktop app

Download `tuxtop-<version>-x64-setup.exe` (or the `.msi`) and run it.

The installers are **unsigned**, so SmartScreen shows "Windows protected your
PC" on first run — *More info → Run anyway*. Signing needs a certificate this
project does not have; `SHA256SUMS` is attached instead, since there is no
certificate for anyone to check against.

### Linux headless server

```sh
tar xzf tuxtop-serve-<version>-linux-x86_64.tar.gz
cd tuxtop-serve-<version>-linux-x86_64
./tuxtop-serve --hosts hosts.toml --port 8787
```

Same fleet, same views, in a browser. It binds **loopback only and has no
authentication**, deliberately — put `tailscale serve` or an SSH tunnel in
front of it. It is **read-only unless `--writable`**, because adding a host
makes the serving machine open SSH with its own keys, which is not a capability
a browser tab should have merely by reaching the port.

---

## Configuration

`hosts.toml` — `%APPDATA%\dev.tuxtop.app\hosts.toml` on Windows, or wherever
`--hosts` points:

```toml
[settings]
interval_ms = 1000        # 250 and 500 are offered too; see below
history_cap_mb = 256

[[host]]
name = "dove"
addr = "dove.example.ts.net"
user = "sam"
group = "physical"        # optional; groups aggregate in Fleet and History
beszel_url = "https://dove.example.ts.net"   # optional; history only

[[host]]
name = "heron"
addr = "heron"            # resolved via ~/.ssh/config, ProxyJump included
interval_ms = 250         # per-host, overrides the global rate

[[host]]
name = "n1"
addr = "10.0.0.5"
os = "windows"            # asked for, never probed

[[host]]
name = "wader"
addr = "wader"
paused = true             # planned maintenance; watched again when you resume
```

Everything here is also editable from **Settings** in the window — the per-host
table covers interval, group, OS and pause for hosts that already exist.

**Pausing** takes a host out of the rotation without removing it, for planned
maintenance. No connection is made and no numbers are reported, but the host
keeps its history, group, interval and position — all of which removing it and
adding it back would discard. Its card blanks rather than freezing its last
sample, and the tally counts it apart from the hosts that are up, so pausing a
failing box never makes the fleet look healthier than it is. Pause from the
button on the card, or from the per-host table in Settings; nothing resumes
automatically, since noticing the host was back would mean connecting to it.

**Sub-second sampling** is available at 4 Hz and 2 Hz, default off. It is
usually right on the one host you are investigating rather than the whole
fleet: only the cheap kernel counters run at that rate, while `nvidia-smi`,
`df` and the process ranking keep their own slower cadence, because a
monitoring tool has no business spending a watched machine's CPU.

Authentication is delegated to the Windows OpenSSH agent or Pageant. Tuxtop
never reads a private key, stores a credential, or prompts for a passphrase.

---

## Building from source

### The desktop app

```powershell
cd src-tauri
cargo build --release
.\target\release\tuxtop.exe
```

No `npm` and no `tauri-cli` needed for the binary: the frontend is static files
under `src/`, embedded at compile time. The **installer** additionally needs
`npx tauri build`, which is what CI runs.

It can also be cross-compiled from Linux for the real `x86_64-pc-windows-msvc`
target — `cargo xwin build --target x86_64-pc-windows-msvc`, which needs
`clang-cl` and `llvm-rc`. That is how CI compiles it.

### The headless server

```sh
cargo build --release -p tuxtop-serve
./target/release/tuxtop-serve --hosts hosts.toml --web src
```

`--web` defaults to `./src`; the binary alone is a server that starts and then
404s every page.

### `tuxtop-watch` — the live view in a terminal

The original CLI, still there and still useful over a plain SSH session:

```sh
cargo run --bin tuxtop-watch -- <your-host>
```

```
dove             cpu   25.2%   mem  17.3% (5.4G / 31.3G)   load 0.64 0.31 0.18
                 net rx     3.3 K/s  tx    57.8 K/s   disk r       0 B/s  w   1.2 M/s

   97% █████   94% █████   91% █████   88% ████·   12% █····    3% ·····    1% ·····    0% ·····
```

`<your-host>` is anything `ssh` accepts. Options: `--interval SECS`, `--plain`.
Use **Windows Terminal** rather than the legacy console host, or the block
characters will not render.

### The design mockup

`src/index.html` is self-contained — **double-click it**. With no backend it
falls back to a simulator, which is how the UI was designed before any GUI code
existed.

[Rust]: https://rustup.rs

---

## Repository layout

```
crates/tuxtop-core/   everything: samplers, supervisor, history, config,
                      and the service both shells call. No GUI dependency.
crates/tuxtop-serve/  the headless server. Same service, over HTTP.
src-tauri/            the Windows shell (Tauri 2). Not a workspace member.
src/                  frontend: HTML/CSS/JS, no build step.
tests/                JS unit tests, Playwright suite, the fleet harness.
scripts/              the checkers, the local gate, the harness builder.
docs/                 architecture, decisions, roadmap, CI, evidence.
```

---

## Testing

```sh
cargo test                              # 229 tests, no GUI toolchain needed
node --test 'tests/*.test.js'           # 83 tests: aggregation, scale, heat
npx playwright test                     # 23 tests: layout, load order, controls
bash scripts/verify.sh                  # everything CI runs, locally
```

`cargo test` runs anywhere, including WSL — deliberate, since the machine the
code is written on cannot build the Windows binary
([ADR-006](docs/DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace)).

**Tests are the memory this project does not otherwise have**, so each name
states the invariant it protects: `iowait_counts_as_idle`,
`mean_of_ratios_is_not_the_group_percentage`,
`a_cell_shows_the_bucket_max_not_its_mean`. Three checkers cover what tests
structurally cannot — a theme token missing from one of three states, a metric
with no aggregation rule, a command no frontend code calls.

CI and releases run on self-hosted runners: [docs/CI.md](docs/CI.md).

---

## Non-goals

No database, no alerting engine, no agent to deploy, no credential store, and
nothing that changes a monitored host. Those are solved elsewhere; this is a
client. [ADR-001](docs/DECISIONS.md#adr-001--build-a-client-not-another-monitoring-system).
