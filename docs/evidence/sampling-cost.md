# Evidence: what 1 Hz sampling actually costs

Measured 2026-08-21 against the live fleet, because `--interval` is hardcoded
at 1 Hz and nothing had ever checked the bill. A monitoring tool that has not
measured itself is in a poor position to lecture anyone.

## Frame size

One sample frame is `/proc/stat`, `/proc/meminfo`, `/proc/diskstats`,
`/proc/net/dev`, `/proc/loadavg`, the hwmon temperature lines and the
nvidia-smi line. Measured over ssh:

| host | cores | bytes / frame |
| --- | --- | --- |
| dove | 32 | 7,415 |
| wader | 24 | 9,521 |
| heron | 4 | 4,332 |

Frame size tracks disk and interface count more than core count: `wader` has
fewer cores than `dove` but more block devices. Average ~7.1 KB.

## Fleet cost at 1 Hz

19 hosts, ~148 cores:

| interval | throughput | per day |
| --- | --- | --- |
| **1 s** | **132 KB/s** | **10.8 GB** |
| 2 s | 66 KB/s | 5.4 GB |
| 5 s | 26 KB/s | 2.2 GB |
| 10 s | 13 KB/s | 1.1 GB |
| 30 s | 4.4 KB/s | 0.36 GB |
| 60 s | 2.2 KB/s | 0.18 GB |

Before compression — ssh compression is off by default, and `/proc` text
compresses extremely well, so the wire figure is likely far lower. The
*decompressed* volume still has to be parsed 19 times a second.

## Reading

10.8 GB/day is not free. It crosses a tailnet, and at least one host is a
rented VPS. Against that, 1 Hz is the entire point of the fast plane — the
Beszel measurement that started this project showed what a 60 s cadence looks
like, and it is not a Task Manager.

The conclusion is not "sample more slowly". It is that the interval has to be
a **setting**, so it can differ per situation: 1 Hz on the box you are
actively watching, 10 s on the twelve you merely want to notice going down.
A per-host override is the natural shape, since the host list already exists
and already persists.

## Reproducing

```sh
ssh <host> "cat /proc/stat /proc/meminfo /proc/diskstats /proc/net/dev /proc/loadavg" | wc -c
```

Add the hwmon and nvidia-smi snippets from `sampler.rs` for the full frame.
