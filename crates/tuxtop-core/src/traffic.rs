//! Counting what the sampler actually costs.
//!
//! A monitoring tool that has never measured itself is in a poor position to
//! lecture anyone. These are the first numbers the app reports about its own
//! behaviour rather than someone else's, so they are measured, attributed per
//! host, and never rounded into a reassuring shape.

use std::sync::atomic::{AtomicU64, Ordering};

/// Per-host byte counters, written by the sampler's read loop.
///
/// Relaxed ordering throughout: these are statistics, not synchronisation.
/// A count that is momentarily one frame stale costs nothing, and paying for
/// stronger ordering on every socket read would be absurd.
#[derive(Debug, Default)]
pub struct TrafficCounter {
    bytes_total: AtomicU64,
    frames_total: AtomicU64,
    last_frame_bytes: AtomicU64,
}

impl TrafficCounter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record bytes lifted off the socket.
    pub fn add_bytes(&self, n: u64) {
        self.bytes_total.fetch_add(n, Ordering::Relaxed);
    }

    /// Record one complete parsed frame.
    pub fn add_frame(&self, bytes: u64) {
        self.frames_total.fetch_add(1, Ordering::Relaxed);
        self.last_frame_bytes.store(bytes, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TrafficStats {
        TrafficStats {
            bytes_total: self.bytes_total.load(Ordering::Relaxed),
            frames_total: self.frames_total.load(Ordering::Relaxed),
            last_frame_bytes: self.last_frame_bytes.load(Ordering::Relaxed),
        }
    }
}

/// What one host has cost so far.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrafficStats {
    pub bytes_total: u64,
    pub frames_total: u64,
    /// Size of the most recent complete frame.
    pub last_frame_bytes: u64,
}

impl TrafficStats {
    /// Mean bytes per frame over the session.
    ///
    /// Preferred over the last frame alone for projections: frame size drifts
    /// slightly as interfaces and mounts come and go, and one unlucky frame
    /// should not set the estimate for the whole fleet.
    pub fn mean_frame_bytes(&self) -> f64 {
        if self.frames_total == 0 {
            return 0.0;
        }
        self.bytes_total as f64 / self.frames_total as f64
    }

    /// Bytes per second this host would cost at `interval_secs`.
    ///
    /// Exact rather than estimated: frame size tracks disk and interface
    /// count, not load, so it is effectively constant for a given host and
    /// the rate really is size divided by interval.
    pub fn bytes_per_sec_at(&self, interval_secs: u32) -> f64 {
        if interval_secs == 0 {
            return 0.0;
        }
        self.mean_frame_bytes() / interval_secs as f64
    }
}

/// Sum per-host stats into a fleet total.
pub fn fleet_total(all: &[TrafficStats]) -> TrafficStats {
    TrafficStats {
        bytes_total: all.iter().map(|s| s.bytes_total).sum(),
        frames_total: all.iter().map(|s| s.frames_total).sum(),
        last_frame_bytes: all.iter().map(|s| s.last_frame_bytes).sum(),
    }
}

/// Bytes per second the whole fleet would cost at `interval_secs`.
pub fn fleet_bytes_per_sec_at(all: &[TrafficStats], interval_secs: u32) -> f64 {
    all.iter().map(|s| s.bytes_per_sec_at(interval_secs)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stats(bytes: u64, frames: u64) -> TrafficStats {
        TrafficStats {
            bytes_total: bytes,
            frames_total: frames,
            last_frame_bytes: bytes.checked_div(frames).unwrap_or(0),
        }
    }

    #[test]
    fn counts_accumulate() {
        let c = TrafficCounter::new();
        c.add_bytes(1000);
        c.add_frame(1000);
        c.add_bytes(1200);
        c.add_frame(1200);

        let s = c.snapshot();
        assert_eq!(s.bytes_total, 2200);
        assert_eq!(s.frames_total, 2);
        assert_eq!(s.last_frame_bytes, 1200);
        assert!((s.mean_frame_bytes() - 1100.0).abs() < 0.01);
    }

    #[test]
    fn a_host_that_has_not_reported_yields_zero_not_nan() {
        // Dividing by zero frames must not produce NaN, which would render as
        // "NaN KB/s" in the settings panel.
        let s = TrafficStats::default();
        assert_eq!(s.mean_frame_bytes(), 0.0);
        assert!(s.mean_frame_bytes().is_finite());
        assert_eq!(s.bytes_per_sec_at(1), 0.0);
    }

    #[test]
    fn a_zero_interval_does_not_divide_by_zero() {
        let s = stats(7000, 1);
        assert_eq!(s.bytes_per_sec_at(0), 0.0);
        assert!(s.bytes_per_sec_at(0).is_finite());
    }

    #[test]
    fn projection_scales_inversely_with_interval() {
        // 7 KB frames at 1 Hz is 7 KB/s; at 10 s it is a tenth of that.
        let s = stats(7000, 1);
        assert!((s.bytes_per_sec_at(1) - 7000.0).abs() < 0.01);
        assert!((s.bytes_per_sec_at(10) - 700.0).abs() < 0.01);
        assert!((s.bytes_per_sec_at(60) - 116.67).abs() < 0.01);
    }

    #[test]
    fn mean_frame_size_is_used_not_the_last_one() {
        // One unlucky frame - a mount appearing mid-session - must not set the
        // estimate for the whole fleet.
        let c = TrafficCounter::new();
        c.add_bytes(1000);
        c.add_frame(1000);
        c.add_bytes(9000); // an outlier
        c.add_frame(9000);
        let s = c.snapshot();
        assert_eq!(s.last_frame_bytes, 9000);
        assert!(
            (s.mean_frame_bytes() - 5000.0).abs() < 0.01,
            "mean, not last"
        );
    }

    #[test]
    fn fleet_projection_matches_the_measured_fleet() {
        // The three hosts actually measured: 7415, 9521 and 4332 bytes/frame.
        let fleet = [stats(7415, 1), stats(9521, 1), stats(4332, 1)];
        let at_1 = fleet_bytes_per_sec_at(&fleet, 1);
        assert!((at_1 - 21268.0).abs() < 1.0, "got {at_1}");
        // Ten times the interval, a tenth of the traffic.
        assert!((fleet_bytes_per_sec_at(&fleet, 10) - at_1 / 10.0).abs() < 0.01);
    }

    #[test]
    fn an_empty_fleet_costs_nothing() {
        assert_eq!(fleet_bytes_per_sec_at(&[], 1), 0.0);
        assert_eq!(fleet_total(&[]), TrafficStats::default());
    }
}
