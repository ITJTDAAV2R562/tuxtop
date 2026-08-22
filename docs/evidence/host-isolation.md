# Host isolation, measured

**Claim under test:** each watched host is independent. Killing one host's
connection must not interrupt any other host's stream.

This was written in Phase 2 and asserted in Phase 3, but never verified — the
isolation code lived in `src-tauri/`, which cannot be built or tested on the
development box at all ([ADR-006](../DECISIONS.md#adr-006--tuxtop-core-is-a-separate-crate-outside-the-tauri-workspace)).
An untested claim about failure behaviour is exactly the kind of confident
assertion this project exists to distrust, so it was moved into `tuxtop-core`
(`fleet::watch_host`) and run for real.

## Method

`tuxtop-fleet dove wader coot owl heron --seconds 75`, at 1 Hz, against five
real hosts totalling 108 cores. The harness runs **the same `fleet::watch_host`
the GUI supervisor runs**, so the result describes the app rather than a
re-implementation.

Three kills, delivered to the *local* `ssh` process:

| t | host | cores |
|---|---|---|
| 20.0 s | owl | 16 |
| 40.1 s | coot | 32 |
| 50.0 s | owl again | 16 |

The victim is matched on the sampler's own frame delimiter (`TUXTOP`), so an
interactive session can never be caught by the pattern.

> **Never test this by stopping `sshd` on the remote host.** That is not a
> test, it is a way to permanently lock yourself out of the machine. The
> failure being simulated is a dead connection, and killing the local client
> reproduces it exactly.

## Result

Every kill was detected, attributed to the right host, and recovered from:

```
  20.0s  !! owl SamplerFailed("ssh exited without a message")
  21.4s  ++ owl recovered
  40.1s  !! coot SamplerFailed("ssh exited without a message")
  43.0s  ++ coot recovered
  50.1s  !! owl SamplerFailed("ssh exited without a message")
  51.4s  ++ owl recovered
```

Recovery took 1.4 s, 2.9 s and 1.3 s — the 1 s backoff plus an ssh handshake.
No fault was ever attributed to a host that had not been killed.

Final counts after 75 s:

```
  coot            70 frames   32 cores  ok
  dove            71 frames   32 cores  ok
  heron           74 frames    4 cores  ok
  owl             73 frames   16 cores  ok
  wader           68 frames   24 cores  ok
```

owl lost two connections and still returned 73 of a possible 75 frames.

## The part that needed checking

Frame counts do not advance perfectly once per second, so a naive read of the
log finds "stalls" in other hosts near each kill and concludes the isolation
leaks. It does not. The stalls are sampling-phase jitter — a host whose remote
loop is slightly slower than, or out of phase with, the status tick — and they
are distributed across the whole run rather than clustered at the kills:

| host | frames / 75 s | zero-ticks | near a kill | elsewhere |
|---|---|---|---|---|
| coot | 70 | 1 | 0 | 1 |
| dove | 71 | 4 | 1 | 3 |
| heron | 74 | 1 | 0 | 1 |
| owl | 73 | 0 | 0 | 0 |
| wader | 68 | 7 | 0 | 7 |

wader's seven zero-ticks are exactly its seven missing frames over 75 s: its
loop runs marginally slow, and that is the whole explanation. The kill windows
cover 32% of the run, so dove's single near-kill tick is what chance predicts
(1.3 expected). **No host stalled because another host died.**

This distinction is the reason the check is written down rather than eyeballed.
"Frame count didn't move for a second, right after a kill" looks like causation
and is not, and a monitoring tool that reasons that way about its own evidence
has no business reporting on anything else.

## What this does not cover

- Killing `sshd` remotely, or a network partition, rather than the local
  client. The fault text would differ (`Unreachable` rather than
  `SamplerFailed`); the isolation path is the same.
- The Tauri supervisor's emit layer, which is a thin adapter over
  `watch_host` — it forwards events and cannot reintroduce coupling between
  hosts, but it has not itself been run under a kill.
