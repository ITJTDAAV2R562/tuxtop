# Evidence: Beszel agent sampling cadence

Raw data behind [ADR-002](../DECISIONS.md#adr-002--two-data-planes-beszel-for-history-direct-ssh-for-live).
Captured 2026-08-16 against **dove** — Debian 13, 32 logical cores, 31 GiB RAM,
RTX 3080 — running Beszel hub and agent **v0.18.7**.

Recorded because the conclusion it supports ("do not build the live view on the
Beszel agent") is the kind of thing a future session would otherwise re-derive
from scratch, or worse, quietly reverse.

---

## Setup

- Hub: `/opt/beszel/beszel`, systemd `beszel-hub`, `0.0.0.0:8090`
- Agent: `/opt/beszel-agent/beszel-agent`, systemd `beszel-agent`, `:45876`
- The agent runs an SSH server; the hub connects to it and reads a JSON
  snapshot. The hub's private key is at `/opt/beszel/beszel_data/id_ed25519`,
  so the same snapshot can be requested by hand.

Requesting a snapshot directly:

```sh
ssh -i /opt/beszel/beszel_data/id_ed25519 -p 45876 u@localhost
```

The agent writes one JSON object on session open, then idles — it is
poll-on-connect, not a push stream. Relevant fields:

```json
{"cpu":0.11,"cpub":[0.04,0.07,0,0,99.89],
 "cpus":[1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0,1,0,0,0,0,0,0,0,0,0,0,0,0,0,0,0],
 "m":31.29,"mu":5.38,"la":[0.46,1.1,0.65], ...}
```

`cpus` is the per-core array — the data we wanted. **Collection was never the
problem; freshness was.**

---

## Test 1 — storage granularity

```sql
select type, count(*), min(created), max(created) from system_stats group by type;
```

```
10m|2|2026-08-16 19:40:00.003Z|2026-08-16 19:50:00.003Z
1m |38|2026-08-16 19:22:08.424Z|2026-08-16 19:59:08.437Z
20m|1|2026-08-16 19:50:00.003Z|2026-08-16 19:50:00.003Z
```

Consecutive `1m` records sit exactly 60 s apart. Finest stored resolution is
one minute, rolled up into coarser buckets over time.

---

## Test 2 — does polling faster help?

Eight sustained busy loops on 32 cores (≈25% aggregate) for 45 s, with the
agent polled every ~4 s and `top` sampled at the same moment.

| t (s) | `top` idle → busy | agent `cpu` | agent `cpus[0:8]` |
| ----- | ----------------- | ----------- | ----------------- |
| 17 | 24.6 → **~25%** | `0.14` | all zeros |
| 21 | 25.1 → ~25% | `0.14` | all zeros |
| 26 | 25.1 → ~25% | `0.14` | all zeros |
| 30 | 25.3 → ~25% | `0.14` | all zeros |
| 34 | 25.6 → ~25% | `0.14` | all zeros |
| 38 | 24.8 → ~25% | `0.14` | all zeros |
| 43 | 24.8 → ~25% | **`21.95`** | `[26,47,26,52,46,48,51,15]` |
| 47 | 25.4 → ~25% | `21.95` | `[26,47,26,52,46,48,51,15]` |
| 51 | 25.1 → ~25% | `21.95` | `[26,47,26,52,46,48,51,15]` |
| 55 | 24.9 → ~25% | `21.95` | `[26,47,26,52,46,48,51,15]` |
| 00 | **0.3 → ~0%** (load ended) | `21.95` | `[26,47,26,52,46,48,51,15]` |
| 04 | 0.0 → 0% | `21.95` | `[26,47,26,52,46,48,51,15]` |
| 08 | 0.0 → 0% | `21.95` | `[26,47,26,52,46,48,51,15]` |
| 12 | 0.0 → 0% | `14.94` | `[6,26,29,21,36,31,37,47]` |

### Reading

- **26 s of sustained 25% load reported as `0.14%`** — a false negative long
  enough to miss an entire short job.
- **`21.95%` reported for 13 s after the load stopped** — a false positive,
  which is worse: it points at a problem that no longer exists.
- The `cpus` array was **byte-identical across five consecutive polls**. Not
  slow-moving; literally the same cached bytes.

Polling faster than the agent's internal refresh returns the same snapshot
again. The cadence is a property of the agent, not of how often you ask.

---

## Test 3 — is the interval configurable?

Documented agent environment variables include `ALL_PROXY`, `AMD_SYSFS`,
`DATA_DIR`, `DISABLE_SSH`, `DISK_USAGE_CACHE`, `DOCKER_HOST`, `DOCKER_TIMEOUT`,
`EXCLUDE_CONTAINERS`, `SENSORS_TIMEOUT`, `SMART_INTERVAL`, and others.

There is **no sampling-interval or cache-time variable**. The nearest are
`DISK_USAGE_CACHE` (extra disks only), `SENSORS_TIMEOUT` (2 s), `SMART_INTERVAL`
(1 h) and `DOCKER_TIMEOUT` (2100 ms) — none affects CPU collection.

This is a design point, not an oversight. The 60 s cadence is what lets the
agent idle at ~23 MB RSS.

---

## Conclusion

Beszel is correct for what it is: a lightweight historical monitor. It is
structurally unable to drive a Task-Manager-feel core grid, and no
configuration change alters that.

Hence the two-plane architecture. Beszel keeps history, trends and alerts;
Tuxtop's own SSH sampler provides the live view.

**Do not "optimise" the fast plane away by pointing it at the Beszel API.** It
will look like it works — the numbers are plausible and the API is pleasant —
and it will be wrong by up to a minute in both directions.

---

## Reproducing

The hub and agent remain installed on dove. To repeat:

```sh
sudo cp /opt/beszel/beszel_data/id_ed25519 /tmp/hubkey
sudo chown "$USER" /tmp/hubkey && chmod 600 /tmp/hubkey

for i in $(seq 8); do timeout 45 sh -c 'while :; do :; done' & done
for i in $(seq 14); do
  timeout 2 ssh -i /tmp/hubkey -p 45876 -o StrictHostKeyChecking=no \
    -o UserKnownHostsFile=/dev/null u@localhost 2>/dev/null | head -1 \
    | grep -oE '"cpu":[0-9.]+'
  top -bn1 | grep '^%Cpu'
  sleep 2
done

rm -f /tmp/hubkey   # do not leave the hub's private key lying in /tmp
```
