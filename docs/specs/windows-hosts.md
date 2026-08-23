# Spec — Windows hosts

**Status:** **built** · Shipped 2026-08-23

**Goal:** a Windows machine appears on the fleet view beside the Linux ones,
with per-core CPU, memory and uptime that mean what they say.

---

## Nothing is installed, and nothing is enabled

Windows ships **OpenSSH Server** as a first-party optional feature and
PowerShell in the box. On N1, the machine this was written against, `sshd` was
already Running with Automatic startup before any of this began — so the
[ADR-004](../DECISIONS.md#adr-004--nothing-gets-installed-on-the-monitored-host)
position holds unchanged: no agent, no package, no port opened for us.

The transport is the same one every Linux host uses — one persistent SSH
connection running a loop — so the supervisor, history, fleet view, groups and
charts need no changes at all. Only the remote command and its parser differ.

## The trap this exists to get right

`Win32_PerfRawData_PerfOS_Processor.PercentProcessorTime` is an **inverse
counter**: it accumulates *idle* 100-nanosecond ticks. Busy is
`100 × (1 − Δcounter / Δtimestamp)`.

Measured on N1, taking the ratio directly reported **79%** on a machine sitting
at about 11. Verified the right way against ground truth: over two real frames
the per-core figures ranged 17–66%, and `_Total` — which Windows computes
independently — read 35.75% against a mean of 37.32% across the individual
cores.

Three tempting alternatives are all worse, and are asserted against in tests:

| API | why not |
| --- | --- |
| `Win32_PerfFormattedData_*` | computes the delta itself over WMI's own refresh window — the cached-value problem this project was built in response to |
| `Get-Counter` | counter paths are **localised**; `"\Processor(_Total)\% Processor Time"` does not exist on a German or Russian Windows |
| `Win32_Processor.LoadPercentage` | coarse and cached |

## The command is base64, not quoted

The script is sent with `powershell -EncodedCommand`, UTF-16LE then base64.

Inline quoting does not survive the journey. A POSIX shell in the path ate
`$os` before PowerShell ever saw it, and cmd.exe — the **default shell for
Windows OpenSSH** — mangles nested double quotes in its own way. The encoded
payload is `[A-Za-z0-9+/=]` with no metacharacters at all, so every shell
between ssh and PowerShell leaves it alone.

The encoder is forty lines rather than a dependency, and is tested against
known vectors.

## Wire format

```
TXWI|key|value                  facts, emitted once before the loop
TXWM|total_kb|free_kb
TXWU|uptime_secs
TXWC|core|idle_counter|timestamp
TXWN|iface|rx_bytes|tx_bytes
TXWD|disk|read_bytes|write_bytes
```

`TXW` rather than the Linux prefixes so a Windows frame is distinguishable at
a glance, and so a stray line of PowerShell error text cannot be read as data.
Unparseable lines are skipped: PowerShell writes warnings and a `#< CLIXML`
header to the same stream, and one noisy line must not cost a sound sample.

**Measured on N1:** 997 bytes per frame, 16 cores, against ~7.3 KB/s for a
Linux host at 1 Hz.

## Two rows that must not be mistaken for data

- **`_Total`** appears in the processor class as a seventeenth entry on a
  sixteen-core box. It is an average Windows already computed, kept apart from
  the real cores rather than listed among them.
- **`_Total`** appears again in the disk class. Left beside the per-disk rows
  it would double every byte, so it is dropped.

## Which host is Windows

An explicit `os = "windows"` on the host in `hosts.toml`, defaulting to Linux.

Detection was considered and rejected for now: probing costs a round trip on
every connect, and guessing wrong produces a host that fails in a way the
fault text cannot explain. Adding a Windows host is a deliberate act; saying
so is one word.

## Exit criteria

- [x] `cargo test` passes, including the inverse-counter and encoding tests
- [x] Real N1 output parses, with `_Total` excluded from both cores and disks
- [x] The generated command runs on N1 and yields frames
- [x] A Windows host added to `hosts.toml` shows a card with per-core CPU
- [x] Its memory reads the machine's, not a guest's

## Processes, and why there is no services view

**Processes** rank on the far side, as they do for Linux — 409 processes are
16 KB per sample and the top twenty are about 1 KB. The delta is obtained
differently: Linux takes two snapshots inside one iteration, while here the
loop is persistent, so it keeps the previous snapshot in a variable and
differentiates against that. One CIM query per cycle instead of two, which on
a measured ~574 ms query is real CPU on somebody's workstation.

The consequence is worth stating because it differs: a Windows process figure
is averaged over the sampling interval where a Linux one is averaged over a
fixed second. Both are percentages of the whole box; the Windows one is
smoother.

Measured on N1: **1,061 bytes per frame**, top twenty of 409.

**Ownership comes from `Win32_Service`**, joined on process id — the Windows
equivalent of the cgroup unit, and it lands in the same Owner column. One
`svchost` frequently hosts several services, so all of them are named:
attributing a spike to whichever service WMI happened to list first is a coin
toss dressed as a fact. The query is cheap when filtered
(`WHERE ProcessId <> 0` is 97 ms against 783 ms unfiltered).

**There is no services view**, for the same reason Phase 10 has no systemd
one. A browsable table of 333 service states is `Get-Service` with more
clicks, and surfacing "auto-start but not running" is alerting-shaped — its
value is noticing a failure, and Tuxtop is opened when you want to look. See
[ADR-010](../DECISIONS.md#adr-010--tuxtop-only-observes-it-never-changes-a-monitored-host)
and the ownership spec.

## Out of scope for now
- **Temperature and GPU.** Windows exposes neither through a stable
  first-party API worth trusting.
- **Auto-detection of the OS.** See above.

---

## Built

N1 runs on the fleet as a Windows host: `os = "windows"` in `hosts.toml`,
16 cores, 63.8 GB — the machine's own memory, not the 31 GB its WSL guest
reports. 997 bytes per frame.

One thing this spec did not anticipate: the `os` field shipped with no way to
set it. The backend had it and `hosts.example.toml` documented it, but the
Add host dialog never sent it, so a Windows host created through the UI was
sampled as a Linux one and failed with "the system cannot find the path
specified". A field needs a control in **both** places — the Add dialog and
the per-host table in Settings — and the second is the one that gets
forgotten. Recorded in CLAUDE.md.
