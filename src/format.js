// Turning numbers into the strings on screen.
//
// Extracted from app.js so it can be tested. Every function here has been
// wrong at least once in a way no test would have caught, because until now
// there were no tests to catch it.
//
// UMD, matching agg.js: index.html loads plain scripts under a CSP nonce and
// the test runner needs `require`. Neither warrants a bundler.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxFormat = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  /// Bytes per second, in the largest unit that keeps the number small.
  ///
  /// Bytes are whole - "1013.0 B/s" is false precision on a counter that
  /// moves in packets - while everything above gets one decimal, because
  /// 1.4 MB/s and 1 MB/s are a 40% difference the reader should see.
  function bps(v) {
    const U = ['B', 'KB', 'MB', 'GB'];
    let n = v || 0, i = 0;
    while (n >= 1024 && i < U.length - 1) { n /= 1024; i++; }
    return `${i ? n.toFixed(1) : Math.round(n)} ${U[i]}/s`;
  }

  /// One decimal, for gigabyte figures that are already in GB.
  const gb = v => Number(v).toFixed(1);

  /// A memory size given in kilobytes, as `/proc` reports it.
  function fmtKb(kb) {
    return kb >= 1048576 ? `${(kb / 1048576).toFixed(1)} GB`
         : kb >= 1024 ? `${(kb / 1024).toFixed(0)} MB`
         : `${kb} KB`;
  }

  /// A time span, in the coarsest unit that still says something.
  ///
  /// The thresholds are deliberately not the unit boundaries: 90 seconds
  /// reads better as "2 min" than "90 sec", and 90 minutes better as "1.5 hr"
  /// than "90 min".
  function fmtSpan(s) {
    if (s < 90) return `${Math.round(s)} sec`;
    if (s < 5400) return `${Math.round(s / 60)} min`;
    if (s < 172800) return `${(s / 3600).toFixed(s < 36000 ? 1 : 0)} hr`;
    return `${(s / 86400).toFixed(1)} days`;
  }

  /// Uptime, at the two largest units that are non-zero.
  ///
  /// Returns empty for an absent value rather than "0m", which would claim
  /// the host just booted.
  function humanUptime(secs) {
    if (secs == null) return '';
    const d = Math.floor(secs / 86400), hh = Math.floor((secs % 86400) / 3600);
    const m = Math.floor((secs % 3600) / 60);
    return d > 0 ? `${d}d ${hh}h` : hh > 0 ? `${hh}h ${m}m` : `${m}m`;
  }

  /// "NVIDIA GeForce RTX 3080" -> "RTX 3080".
  ///
  /// The vendor and product line are the same across a fleet; the model is
  /// the part that identifies the card. Falls back to "GPU" rather than an
  /// empty chip if a driver reports nothing useful.
  function shortGpu(name) {
    // `\\s*` rather than `\\s+`: a driver that reports the vendor and nothing
    // else left "NVIDIA" on the chip, because the prefix only stripped when
    // something followed it. Now it falls through to the label below.
    return String(name)
      .replace(/^NVIDIA\b\s*/i, '')
      .replace(/^GeForce\b\s*/i, '')
      .replace(/^Tesla\b\s*/i, '')
      .trim() || 'GPU';
  }

  /**
   * How often we sample, as the status line says it.
   *
   * This exists because that line used to be the string "1 Hz", hardcoded,
   * while the actual interval was a setting - so a fleet sampled every thirty
   * seconds sat under a footer confidently claiming 1 Hz. A label that states
   * a rate it did not read is the same class of thing as a load figure that
   * was never measured, which is the failure this project is a reaction to.
   *
   * Mirrors `sampler::rate_label` in the backend, which formats the same value
   * for the CLI - keep the two in step.
   *
   * Sub-second rates read as Hz because that is how they are offered and how
   * people say them; a second and slower read as a duration, because "0.033 Hz"
   * is nobody's idea of thirty seconds.
   *
   * @param {number} ms sample interval in milliseconds
   * @returns {string} e.g. "4 Hz", "1 s", "30 s" - or "?" if ms is not a
   *   usable number, since inventing a plausible rate is the bug above.
   */
  function rateLabel(ms) {
    if (typeof ms !== 'number' || !isFinite(ms) || ms <= 0) return '?';
    if (ms < 1000) {
      const hz = 1000 / ms;
      return `${Number.isInteger(hz) ? hz : hz.toFixed(1)} Hz`;
    }
    const s = ms / 1000;
    return `${Number.isInteger(s) ? s : s.toFixed(1)} s`;
  }

  return { bps, gb, fmtKb, fmtSpan, humanUptime, shortGpu, rateLabel };
});
