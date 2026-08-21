//! The app's history store: one `History` behind a lock, fed by the sampler.
//!
//! Lives here rather than in the frontend so it survives a webview reload and
//! the webview does not carry tens of MB of numbers. Only a process restart
//! clears it, which is the intended behaviour.

use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tuxtop_core::history::{History, Point};
use tuxtop_core::Sample;

#[derive(Default)]
pub struct HistoryStore {
    inner: Mutex<History>,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one sample against the wall clock.
    ///
    /// Stores `cpu`, `mem`, `disk`, `net`, `load`, `temp`, `swap`, `fs`,
    /// `gpu`, `gpumem` and `core.N` — the same keys the frontend metric
    /// registry uses, so a chart asks for exactly what it already displays.
    pub fn record(&self, s: &Sample) {
        let t = now_secs();
        let mut h = self.inner.lock().unwrap();

        h.push(&s.host, "cpu", t, s.cpu);
        if s.mem_total_kb > 0 {
            h.push(&s.host, "mem", t, s.mem_used_kb as f32 / s.mem_total_kb as f32 * 100.0);
        }
        h.push(&s.host, "disk", t, (s.disk_read_bps + s.disk_write_bps) as f32);
        h.push(&s.host, "net", t, (s.net_rx_bps + s.net_tx_bps) as f32);
        h.push(&s.host, "load", t, s.load[0]);

        if s.swap_total_kb > 0 {
            h.push(&s.host, "swap", t,
                s.swap_used_kb as f32 / s.swap_total_kb as f32 * 100.0);
        }

        // The fullest mount, never an average: a roomy /home must not hide a
        // full /. Absent on most frames, since df runs on a slow cadence.
        if let Some(f) = tuxtop_core::facts::fullest(&s.filesystems) {
            h.push(&s.host, "fs", t, f.used_pct());
        }

        // Absent sensors record nothing rather than zero. A zero would be
        // indistinguishable from a genuinely cold CPU or an idle card.
        if let Some(c) = s.cpu_temp_c {
            h.push(&s.host, "temp", t, c);
        }
        if let Some(g) = &s.gpu {
            h.push(&s.host, "gpu", t, g.util_pct);
            if g.mem_total_mb > 0 {
                h.push(&s.host, "gpumem", t,
                    g.mem_used_mb as f32 / g.mem_total_mb as f32 * 100.0);
            }
        }

        for (i, v) in s.cores.iter().enumerate() {
            h.push(&s.host, &format!("core.{i}"), t, *v);
        }
    }

    pub fn query(
        &self,
        host: &str,
        metric: &str,
        from: u64,
        to: u64,
        max_points: usize,
    ) -> Vec<Point> {
        self.inner.lock().unwrap().query(host, metric, from, to, max_points)
    }

    pub fn usage(&self) -> HistoryUsage {
        let h = self.inner.lock().unwrap();
        HistoryUsage {
            bytes: h.bytes() as u64,
            series: h.series_count() as u64,
        }
    }

    pub fn forget_host(&self, host: &str) {
        self.inner.lock().unwrap().forget_host(host);
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct HistoryUsage {
    pub bytes: u64,
    pub series: u64,
}

/// Wall-clock seconds. History is anchored to real time rather than uptime so
/// a chart axis can show when something happened, not merely how long ago.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
