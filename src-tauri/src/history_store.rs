//! The app's history store: one `History` behind a lock, fed by the sampler.
//!
//! Lives here rather than in the frontend so it survives a webview reload and
//! the webview does not carry tens of MB of numbers. Only a process restart
//! clears it, which is the intended behaviour.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use tuxtop_core::history::{History, Point};
use tuxtop_core::Sample;

/// How many recorded samples pass between cap checks.
///
/// `bytes()` walks every series, so doing it on every frame would be a
/// measurable cost to stay under a limit that moves slowly — a fleet gains
/// memory a few kilobytes at a time. Roughly once a minute at 1 Hz with a
/// handful of hosts.
const CAP_CHECK_EVERY: u64 = 64;

#[derive(Default)]
pub struct HistoryStore {
    inner: Mutex<History>,
    /// Memory ceiling in bytes. Zero means unset, which is treated as
    /// unlimited rather than as "shed everything" — a cap that has not been
    /// read from disk yet must not throw history away.
    cap_bytes: AtomicU64,
    /// Finest interval still held, in seconds, for the settings panel.
    finest_secs: AtomicU32,
    writes: AtomicU64,
}

impl HistoryStore {
    pub fn new() -> Self {
        Self {
            finest_secs: AtomicU32::new(1),
            ..Default::default()
        }
    }

    /// Set the ceiling, and apply it immediately.
    ///
    /// Applied at once rather than at the next check so that lowering the cap
    /// in Settings has a visible effect while the panel is still open —
    /// a limit that appears to do nothing is indistinguishable from a broken
    /// one.
    pub fn set_cap_mb(&self, mb: u32) {
        let bytes = mb as u64 * 1024 * 1024;
        self.cap_bytes.store(bytes, Ordering::Relaxed);
        if bytes > 0 {
            let finest = self.inner.lock().unwrap().enforce_cap(bytes as usize);
            self.finest_secs.store(finest, Ordering::Relaxed);
        }
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

        // Enforce on a subsample. Held under the same lock as the writes it
        // is bounding, so the check can never see a partly-written frame.
        let cap = self.cap_bytes.load(Ordering::Relaxed);
        if cap > 0 && self.writes.fetch_add(1, Ordering::Relaxed) % CAP_CHECK_EVERY == 0 {
            let finest = h.enforce_cap(cap as usize);
            self.finest_secs.store(finest, Ordering::Relaxed);
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
            full_series_bytes: tuxtop_core::history::full_series_bytes() as u64,
            finest_secs: self.finest_secs.load(Ordering::Relaxed),
        }
    }

    pub fn forget_host(&self, host: &str) {
        self.inner.lock().unwrap().forget_host(host);
    }
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct HistoryUsage {
    /// Measured, not projected: what the store is holding right now.
    pub bytes: u64,
    pub series: u64,
    /// What one series costs once every tier has filled, derived from the
    /// tier cascade. Multiplying gives the steady state a fleet grows into.
    pub full_series_bytes: u64,
    /// Finest interval still held anywhere, in seconds. Above 1 means the cap
    /// has shed resolution, and the panel must say so.
    pub finest_secs: u32,
}

/// Wall-clock seconds. History is anchored to real time rather than uptime so
/// a chart axis can show when something happened, not merely how long ago.
pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
