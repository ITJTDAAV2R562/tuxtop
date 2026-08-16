# Tuxtop

A Windows-native Task Manager for remote Linux boxes.

Live per-core CPU, memory, disk, network and GPU for several hosts at once, in
a real Win11 Mica window — not a browser tab, not a terminal, not a web
dashboard you install as a PWA and pretend is an app.

> **Status: early — there is a working CLI, but no GUI yet.**
> `tuxtop-watch` streams live per-core CPU from a remote host over SSH and runs
> today on Windows, Linux and macOS. The Tauri window is Phase 2 and does not
> build yet. See [docs/ROADMAP.md](docs/ROADMAP.md).

---

## Why this exists

`btop` over SSH works but is a terminal. Netdata, Beszel and Cockpit are web
dashboards. MobaXterm has a small per-session sidebar. XPipe manages
connections. [TuxManager] is Qt6 and local-only — no SSH, no multi-host.

None of them puts a live per-core grid for several Linux hosts in a native
Windows window. That is the whole gap.

The nearest thing, Beszel, is genuinely good and **is still used here** for
history and alerts. It just cannot go fast: measured, its agent reported idle
through 26 seconds of sustained 25% load, then reported 22% for 13 seconds
after the load stopped, serving byte-identical cached arrays. The interval is
not configurable — it is what makes the agent cost 23 MB.

So Tuxtop reads history from Beszel and samples the live view itself.
Measurement: [docs/evidence/beszel-cadence.md](docs/evidence/beszel-cadence.md).

[TuxManager]: https://github.com/benapetr/TuxManager

---

## How it works

Two data planes, either of which can be absent:

- **Fast plane** — one persistent SSH connection per host running a POSIX `sh`
  loop that cats `/proc` once a second. Per-core CPU, memory, disk I/O,
  network, load. **Nothing is installed on the monitored host**, no root, no
  open port, no firewall change.
- **Slow plane** — the Beszel hub's PocketBase API for 1-minute history,
  trends and alerts.

A host with only SSH gets the live grid without history. A host reachable only
through Beszel shows history marked stale. Neither case blanks the card.

Full picture: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
Reasoning, with rejected alternatives: [docs/DECISIONS.md](docs/DECISIONS.md).

---

## Design

An interactive mockup of the intended UI — live core grid, expandable host
cards, the glass treatment — was built before any GUI code:

**https://claude.ai/code/artifact/b1cd78c2-097e-44a9-8ba4-9f7f0c463bf1**

It runs on simulated data and includes a cadence toggle that switches between
1 Hz and Beszel's 60 s, which is the clearest way to see why the fast plane
exists.

Load is encoded three ways at once — fill height, colour band, and a crisp cap
line at the leading edge — so a hot core registers peripherally, before you
read any digits.

---

## Repository layout

```
crates/tuxtop-core/   parsing, rate maths, wire types. No GUI dependency.
src-tauri/            the Windows shell (Tauri 2). Not a workspace member.
src/                  frontend: HTML/CSS/JS, no build step.
docs/                 architecture, decisions, roadmap, evidence.
```

---

## Running it today

### `tuxtop-watch` — the live view, in a terminal

Works on **Windows**, Linux and macOS. Needs only [Rust] and an `ssh` client
(Windows 10+ and Windows 11 ship OpenSSH; check with `ssh -V`).

```powershell
git clone https://github.com/UZ1sFED3yS/tuxtop
cd tuxtop
cargo run --bin tuxtop-watch -- <your-host>
```

**No Rust on Windows?** Cross-compile the `.exe` from WSL or any Linux box
instead — the CLI has no GUI dependencies, so this works cleanly and saves a
rustup + MSVC Build Tools install:

```sh
sudo apt install mingw-w64            # one time
rustup target add x86_64-pc-windows-gnu   # one time

./scripts/build-windows.sh /mnt/c/Users/you/tuxtop/
```

Then in PowerShell: `.\tuxtop-watch.exe <your-host>`. Use **Windows Terminal**
rather than the legacy console host, or the colours and block characters will
not render.

`<your-host>` is anything `ssh` accepts — an alias from your `~/.ssh/config`,
a hostname, or `user@host`. If `ssh <your-host>` works in your terminal, this
works.

```
dove             cpu   25.2%   mem  17.3% (5.4G / 31.3G)   load 0.64 0.31 0.18
                 net rx     3.3 K/s  tx    57.8 K/s   disk r       0 B/s  w   1.2 M/s

   97% █████   94% █████   91% █████   88% ████·   12% █····    3% ·····    1% ·····    0% ·····
    0% ·····    2% ·····    0% ·····    1% ·····    0% ·····    0% ·····    1% ·····    0% ·····
```

Options: `--interval SECS` (default 1), `--plain` (no colour or cursor
movement, one line per sample — good for piping to a file).

Nothing is installed on the target. No root, no open port, no agent.

[Rust]: https://rustup.rs

### The design mockup

`src/index.html` is self-contained — **double-click it**, or open it in any
browser. It runs on simulated data and is the same page as the hosted
[mockup](#design).

### `cargo test`

38 tests, no GUI toolchain required, runs anywhere including WSL. Deliberate:
the maths must be testable on the machine where the code is written, even
though that machine cannot build the Windows binary.
[ADR-006](docs/DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace).

### The GUI — not yet

`src-tauri/` is scaffolding that has never been compiled. There is no
`package.json` and no icon set, so **`npm run tauri dev` will not work**.
Phase 2 of the [roadmap](docs/ROADMAP.md) wires it up, on Windows, where it
has to be built — Tauri pulls in webkit2gtk if built on Linux.

---

## Configuration

`hosts.toml`:

```toml
[[host]]
name = "dove"
addr = "dove.example.ts.net"
user = "sam"
beszel_url = "https://dove.example.ts.net"   # optional; history only

[[host]]
name = "heron"
addr = "heron"          # resolved via ~/.ssh/config, ProxyJump included
```

Authentication is delegated to the Windows OpenSSH agent or Pageant. Tuxtop
never reads a private key, stores a credential, or prompts for a passphrase.

---

## Non-goals

No database, no alerting engine, no agent to deploy, no credential store.
Those are solved; this is a client. [ADR-001](docs/DECISIONS.md#adr-001--build-a-client-not-another-monitoring-system).
