# Tuxtop

A Windows-native Task Manager for remote Linux boxes.

Live per-core CPU, memory, disk, network and GPU for several hosts at once, in
a real Win11 Mica window — not a browser tab, not a terminal, not a web
dashboard you install as a PWA and pretend is an app.

> **Status: early.** The sampling core is written and tested (33 tests, checked
> against a real 32-core host). The SSH transport and the window are next. See
> [docs/ROADMAP.md](docs/ROADMAP.md).

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

## Building

### The core — anywhere, including WSL

```sh
cargo test
```

33 tests, no GUI toolchain required. This is deliberate: the maths must be
testable on the machine where the code is written, even though that machine
cannot build the Windows binary. See
[ADR-006](docs/DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace).

### The app — on Windows

Requires Rust, Node, and the WebView2 runtime (present by default on Win11).

```powershell
npm install
npm run tauri dev
```

**This cannot be built from WSL.** Tauri needs the Windows toolchain to produce
a Windows binary, and pulls in webkit2gtk if built on Linux.

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
