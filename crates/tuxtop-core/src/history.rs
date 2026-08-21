//! In-memory history: a tiered cascade of ring buffers.
//!
//! Memory only, by design. A restart starts clean, like Windows Task Manager.
//! History here is low-value data — losing it costs nothing — and that single
//! assumption removes persistence, durability, corruption recovery and
//! migration from the design entirely.
//!
//! See `docs/specs/history-plane.md`.

use std::collections::HashMap;

/// Tier layout: (interval seconds, span seconds).
///
/// Intervals divide evenly (1 -> 10 -> 60 -> 300) so a coarse bucket always
/// covers a whole number of finer ones. Each tier accumulates from the raw
/// samples directly rather than from the tier above, so min and max are true
/// extremes of real readings rather than extremes of already-averaged data.
pub const TIERS: &[(u32, u32)] = &[
    (1, 3_600),     // 1 Hz, 1 hour
    (10, 21_600),   // 10 s, 6 hours
    (60, 86_400),   // 60 s, 24 hours
    (300, 604_800), // 5 min, 7 days
];

/// One point of history. `min` and `max` equal `mean` on the raw tier.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Point {
    /// Unix seconds at the start of the bucket.
    pub t: u64,
    pub min: f32,
    pub mean: f32,
    pub max: f32,
}

/// A fixed-interval ring of buckets.
///
/// Timestamps are not stored per point: the interval is fixed and the ring
/// position implies the time, which is what keeps a raw tier down to 4 bytes
/// per sample rather than 20.
#[derive(Debug)]
struct Tier {
    interval: u32,
    cap: usize,
    /// Mean per bucket, or the raw value on the raw tier.
    mean: Vec<f32>,
    /// Empty on the raw tier, where min and max are the value itself.
    min: Vec<f32>,
    max: Vec<f32>,
    head: usize,
    len: usize,
    /// Bucket time of the newest written entry.
    newest: u64,
    // Accumulator for the bucket currently being filled.
    acc_bucket: u64,
    acc_sum: f64,
    acc_min: f32,
    acc_max: f32,
    acc_n: u32,
}

impl Tier {
    fn new(interval: u32, span: u32, raw: bool) -> Self {
        let cap = (span / interval) as usize;
        Self {
            interval,
            cap,
            mean: Vec::with_capacity(0),
            min: Vec::with_capacity(0),
            max: Vec::with_capacity(0),
            head: 0,
            len: 0,
            newest: 0,
            acc_bucket: 0,
            acc_sum: 0.0,
            acc_min: f32::MAX,
            acc_max: f32::MIN,
            acc_n: 0,
        }
        .with_raw(raw)
    }

    fn with_raw(mut self, raw: bool) -> Self {
        self.mean = vec![0.0; self.cap];
        if !raw {
            self.min = vec![0.0; self.cap];
            self.max = vec![0.0; self.cap];
        }
        self
    }

    fn is_raw(&self) -> bool {
        self.min.is_empty()
    }

    fn bytes(&self) -> usize {
        (self.mean.len() + self.min.len() + self.max.len()) * std::mem::size_of::<f32>()
    }

    fn push(&mut self, t: u64, v: f32) {
        let bucket = t - (t % self.interval as u64);

        if self.acc_n > 0 && bucket != self.acc_bucket {
            self.flush();
        }
        if self.acc_n == 0 {
            // Timestamps are derived from ring position, so every bucket must
            // be written even when nothing arrived for it. Skipping them
            // silently shifts the time of every earlier point - a host that
            // went away for ten minutes would redate its whole history.
            self.fill_gap(bucket);
            self.acc_bucket = bucket;
        }

        self.acc_sum += v as f64;
        self.acc_min = self.acc_min.min(v);
        self.acc_max = self.acc_max.max(v);
        self.acc_n += 1;

        // The raw tier has one sample per bucket, so it can flush immediately
        // and be queryable without waiting for the next sample to close it.
        if self.is_raw() {
            self.flush();
        }
    }

    /// Write empty buckets between the newest entry and `upto`.
    ///
    /// A gap is stored as NaN and skipped on query, so a host that stopped
    /// reporting leaves a hole rather than a straight line implying it was
    /// fine throughout.
    fn fill_gap(&mut self, upto: u64) {
        if self.len == 0 {
            return;
        }
        let step = self.interval as u64;
        let mut next = self.newest + step;
        // A gap longer than the ring means everything held is stale; writing
        // one full ring of holes is enough and bounds the loop.
        let mut budget = self.cap;
        while next < upto && budget > 0 {
            self.mean[self.head] = f32::NAN;
            if !self.is_raw() {
                self.min[self.head] = f32::NAN;
                self.max[self.head] = f32::NAN;
            }
            self.newest = next;
            self.head = (self.head + 1) % self.cap;
            self.len = (self.len + 1).min(self.cap);
            next += step;
            budget -= 1;
        }
        if budget == 0 {
            // The gap exceeded the span; nothing older is meaningful.
            self.newest = upto.saturating_sub(step);
        }
    }

    fn flush(&mut self) {
        if self.acc_n == 0 {
            return;
        }
        let mean = (self.acc_sum / self.acc_n as f64) as f32;

        self.mean[self.head] = mean;
        if !self.is_raw() {
            self.min[self.head] = self.acc_min;
            self.max[self.head] = self.acc_max;
        }
        self.newest = self.acc_bucket;
        self.head = (self.head + 1) % self.cap;
        self.len = (self.len + 1).min(self.cap);

        self.acc_sum = 0.0;
        self.acc_min = f32::MAX;
        self.acc_max = f32::MIN;
        self.acc_n = 0;
    }

    /// Oldest bucket still held.
    fn oldest(&self) -> u64 {
        self.newest
            .saturating_sub((self.len.saturating_sub(1)) as u64 * self.interval as u64)
    }

    /// Points within `[from, to]`, oldest first.
    fn range(&self, from: u64, to: u64) -> Vec<Point> {
        let mut out = Vec::new();
        for i in 0..self.len {
            // Walk backwards from the newest entry.
            let back = self.len - 1 - i;
            let idx = (self.head + self.cap - 1 - back) % self.cap;
            let t = self.newest - back as u64 * self.interval as u64;
            if t < from || t > to {
                continue;
            }
            let mean = self.mean[idx];
            if !mean.is_finite() {
                continue; // a gap: no reading existed for this bucket
            }
            let (min, max) = if self.is_raw() {
                (mean, mean)
            } else {
                (self.min[idx], self.max[idx])
            };
            out.push(Point { t, min, mean, max });
        }
        out
    }
}

/// All tiers for one metric on one host.
#[derive(Debug)]
pub struct Series {
    tiers: Vec<Tier>,
}

impl Default for Series {
    fn default() -> Self {
        Self::new()
    }
}

impl Series {
    pub fn new() -> Self {
        Self {
            tiers: TIERS
                .iter()
                .enumerate()
                .map(|(i, &(iv, span))| Tier::new(iv, span, i == 0))
                .collect(),
        }
    }

    pub fn push(&mut self, t: u64, v: f32) {
        for tier in &mut self.tiers {
            tier.push(t, v);
        }
    }

    pub fn bytes(&self) -> usize {
        self.tiers.iter().map(|t| t.bytes()).sum()
    }

    /// Points covering `[from, to]`, from the finest tier that spans it.
    ///
    /// This is what makes continuous zoom work without preset buttons: the
    /// tiers are storage, the window is a view. Scrubbing across a tier
    /// boundary just yields a slightly coarser band.
    pub fn query(&self, from: u64, to: u64, max_points: usize) -> Vec<Point> {
        let tier = self
            .tiers
            .iter()
            .find(|t| t.len > 0 && t.oldest() <= from)
            .unwrap_or_else(|| self.tiers.last().expect("TIERS is never empty"));

        let pts = tier.range(from, to);
        downsample(pts, max_points)
    }
}

/// Reduce to at most `max_points`, preserving extremes.
///
/// Taking every Nth point would drop spikes — the thing most worth seeing —
/// so each output bucket keeps the true min and max of the points it covers.
pub fn downsample(pts: Vec<Point>, max_points: usize) -> Vec<Point> {
    if max_points == 0 || pts.len() <= max_points {
        return pts;
    }
    let chunk = pts.len().div_ceil(max_points);
    pts.chunks(chunk)
        .map(|c| Point {
            t: c[0].t,
            min: c.iter().fold(f32::MAX, |a, p| a.min(p.min)),
            mean: (c.iter().map(|p| p.mean as f64).sum::<f64>() / c.len() as f64) as f32,
            max: c.iter().fold(f32::MIN, |a, p| a.max(p.max)),
        })
        .collect()
}

/// Every series, keyed by host and metric.
#[derive(Debug, Default)]
pub struct History {
    series: HashMap<(String, String), Series>,
}

impl History {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, host: &str, metric: &str, t: u64, v: f32) {
        // NaN would poison min/max for the life of the bucket.
        if !v.is_finite() {
            return;
        }
        self.series
            .entry((host.to_string(), metric.to_string()))
            .or_default()
            .push(t, v);
    }

    pub fn query(
        &self,
        host: &str,
        metric: &str,
        from: u64,
        to: u64,
        max_points: usize,
    ) -> Vec<Point> {
        self.series
            .get(&(host.to_string(), metric.to_string()))
            .map(|s| s.query(from, to, max_points))
            .unwrap_or_default()
    }

    pub fn bytes(&self) -> usize {
        self.series.values().map(|s| s.bytes()).sum()
    }

    pub fn series_count(&self) -> usize {
        self.series.len()
    }

    /// Drop every series for a host that is no longer watched.
    pub fn forget_host(&mut self, host: &str) {
        self.series.retain(|(h, _), _| h != host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feed `n` seconds of samples starting at t0, one per second.
    fn feed(s: &mut Series, t0: u64, n: u64, f: impl Fn(u64) -> f32) {
        for i in 0..n {
            s.push(t0 + i, f(i));
        }
    }

    #[test]
    fn raw_tier_returns_what_was_put_in() {
        let mut s = Series::new();
        feed(&mut s, 1000, 10, |i| i as f32);
        let pts = s.query(1000, 1009, 100);
        assert_eq!(pts.len(), 10);
        assert_eq!(pts[0].mean, 0.0);
        assert_eq!(pts[9].mean, 9.0);
        assert_eq!(pts[0].t, 1000, "oldest first");
    }

    #[test]
    fn a_spike_survives_into_the_coarse_tiers() {
        // THE test. A 60 s bucket storing only a mean would average a 100%
        // spike down to under 2%, which is a confident, plausible, wrong
        // number - the failure this whole project exists to prevent.
        let mut s = Series::new();
        feed(&mut s, 0, 60, |i| if i == 30 { 100.0 } else { 0.0 });
        // Push past the raw tier's one-hour span so the early window can only
        // be served by an aggregated tier. Without this the query is answered
        // from raw samples and proves nothing about aggregation.
        feed(&mut s, 60, 4_000, |_| 0.0);

        let pts = s.query(0, 60, 10);
        let max = pts.iter().fold(0.0f32, |a, p| a.max(p.max));
        assert_eq!(max, 100.0, "the spike must survive aggregation: {pts:?}");

        // The window is served by the 10 s tier, where one 100% sample among
        // ten is a mean of exactly 10. The mean being an order of magnitude
        // below the max is the whole point: stored alone it would report 10%
        // and the spike would be gone. The band between them is the signal.
        let mean = pts.iter().fold(0.0f32, |a, p| a.max(p.mean));
        assert!(
            mean <= 10.0 && max >= mean * 5.0,
            "mean {mean} should sit far below max {max}"
        );
    }

    #[test]
    fn min_and_max_are_extremes_of_raw_samples() {
        let mut s = Series::new();
        feed(&mut s, 0, 60, |i| i as f32);
        feed(&mut s, 60, 4_000, |_| 0.0); // age the first minute out of raw
        let pts = s.query(0, 59, 1);
        let p = &pts[0];
        assert_eq!(p.min, 0.0, "{pts:?}");
        assert_eq!(p.max, 59.0, "extremes are of raw samples, not of means");
        assert!((p.mean - 29.5).abs() < 0.01, "mean was {}", p.mean);
    }

    #[test]
    fn the_raw_tier_is_bounded_by_its_span() {
        // Two hours of samples must not grow past the one-hour ring.
        let mut s = Series::new();
        feed(&mut s, 0, 7_200, |_| 1.0);
        let bytes = s.bytes();
        feed(&mut s, 7_200, 7_200, |_| 1.0);
        assert_eq!(s.bytes(), bytes, "memory must not grow with time");
    }

    #[test]
    fn a_series_costs_what_the_spec_says() {
        // 3600*4 + (2160 + 1440 + 2016)*12 = 81,792 bytes.
        let s = Series::new();
        assert_eq!(s.bytes(), 3600 * 4 + (2160 + 1440 + 2016) * 12);
        assert!(s.bytes() < 82 * 1024, "~80 KB per series");
    }

    #[test]
    fn old_points_fall_out_of_the_raw_tier() {
        let mut s = Series::new();
        feed(&mut s, 0, 7_200, |i| i as f32);
        // The first hour is gone from the raw tier; asking for it falls
        // through to a coarser tier rather than returning nothing.
        let pts = s.query(0, 100, 50);
        assert!(!pts.is_empty(), "a coarser tier must still cover it");
    }

    #[test]
    fn a_window_the_raw_tier_covers_uses_the_raw_tier() {
        let mut s = Series::new();
        feed(&mut s, 0, 3_000, |i| i as f32);
        // Per-second resolution over the last minute.
        let pts = s.query(2_940, 3_000, 1000);
        assert!(
            pts.len() >= 60,
            "expected per-second detail, got {}",
            pts.len()
        );
    }

    #[test]
    fn downsampling_preserves_extremes() {
        // Taking every Nth point would drop the spike entirely.
        let pts: Vec<Point> = (0..100)
            .map(|i| Point {
                t: i,
                min: 0.0,
                mean: 0.0,
                max: if i == 37 { 99.0 } else { 0.0 },
            })
            .collect();
        let out = downsample(pts, 10);
        assert!(out.len() <= 10);
        assert_eq!(out.iter().fold(0.0f32, |a, p| a.max(p.max)), 99.0);
    }

    #[test]
    fn downsampling_below_the_budget_is_a_no_op() {
        let pts = vec![Point {
            t: 1,
            min: 1.0,
            mean: 1.0,
            max: 1.0,
        }];
        assert_eq!(downsample(pts.clone(), 100), pts);
    }

    #[test]
    fn querying_an_unknown_series_is_empty_not_an_error() {
        let h = History::new();
        assert!(h.query("ghost", "cpu", 0, 100, 10).is_empty());
    }

    #[test]
    fn nan_is_refused_rather_than_poisoning_min_and_max() {
        // One NaN would make min/max NaN for the life of the bucket, and NaN
        // propagates through every later comparison.
        let mut h = History::new();
        h.push("dove", "cpu", 0, 5.0);
        h.push("dove", "cpu", 1, f32::NAN);
        h.push("dove", "cpu", 2, 7.0);
        let pts = h.query("dove", "cpu", 0, 2, 10);
        assert!(pts.iter().all(|p| p.mean.is_finite() && p.max.is_finite()));
        assert_eq!(pts.len(), 2, "the NaN sample is dropped, the others kept");
    }

    #[test]
    fn forgetting_a_host_drops_only_that_host() {
        let mut h = History::new();
        h.push("dove", "cpu", 0, 1.0);
        h.push("heron", "cpu", 0, 1.0);
        h.forget_host("dove");
        assert_eq!(h.series_count(), 1);
        assert!(h.query("dove", "cpu", 0, 10, 10).is_empty());
        assert!(!h.query("heron", "cpu", 0, 10, 10).is_empty());
    }

    #[test]
    fn the_whole_fleet_fits_the_budget() {
        // 19 hosts x 8 scalar metrics, plus 148 per-core series.
        let mut h = History::new();
        for i in 0..19 {
            for m in ["cpu", "mem", "disk", "net", "load", "temp", "gpu", "gpumem"] {
                h.push(&format!("h{i}"), m, 0, 1.0);
            }
        }
        for c in 0..148 {
            h.push("dove", &format!("core.{c}"), 0, 1.0);
        }
        let mb = h.bytes() as f64 / 1024.0 / 1024.0;
        assert!(mb < 30.0, "fleet history should be ~24 MB, got {mb:.1} MB");
    }

    #[test]
    fn a_gap_in_sampling_does_not_invent_points() {
        // A host that went away for ten minutes must leave a hole, not a
        // straight line implying it was fine the whole time.
        let mut s = Series::new();
        feed(&mut s, 0, 60, |_| 5.0);
        feed(&mut s, 660, 60, |_| 5.0);
        let pts = s.query(0, 720, 1000);
        assert!(
            pts.iter().all(|p| p.t <= 60 || p.t >= 660),
            "no points invented across the gap"
        );
    }
}
