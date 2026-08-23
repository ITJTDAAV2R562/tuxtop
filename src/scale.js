// Turning a value into a position, a band, or a column count.
//
// The visual half of the metric registry: everything that decides how big,
// how red, or how many. Extracted from app.js to be testable.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxScale = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  /// The three load bands, used identically by tiles, bars and charts.
  ///
  /// Discrete rather than a continuous ramp: a rainbow is more information
  /// than the eye needs here and reads as garish. See ADR-005.
  const band = v => (v >= 90 ? 'crit' : v >= 75 ? 'warn' : 'cool');

  /// The visible window of a log axis: a fixed number of decades below the
  /// fleet peak, rather than everything down to zero.
  ///
  /// Anchoring at 1 byte crushed everything above a megabyte into the top
  /// third - a 600x difference rendered as 69% against 100%. Four decades
  /// gives the range real visual separation, and anything quieter than that
  /// reads as the negligible traffic it is.
  function logWindow(m, peak) {
    const top = Math.max(peak || 0, m.floor || 1);
    return { top, bottom: top / Math.pow(10, m.decades || 4) };
  }

  /// Map a value to 0..1 for the given metric.
  ///
  /// Absolute metrics use their own ceiling and ignore the peak entirely, so
  /// a percentage means the same thing on every host. Rates use the log
  /// window, so one busy host cannot flatten everyone else into slivers.
  function normalise(m, v, peak) {
    if (m.scale === 'absolute') return Math.min(1, (v || 0) / (m.max || 100));
    const { top, bottom } = logWindow(m, peak);
    if (!v || v <= bottom) return 0;
    return Math.min(1, Math.log10(v / bottom) / Math.log10(top / bottom));
  }

  /// The window the history slider maps onto: log-spaced, a minute to a week.
  ///
  /// Continuous rather than preset buttons - the tiers are storage, the
  /// window is a view, and crossing a tier boundary should be invisible.
  const WIN_MIN = 60, WIN_MAX = 604800;
  const sliderToSecs = v =>
    Math.round(WIN_MIN * Math.pow(WIN_MAX / WIN_MIN, v / 1000));

  /// How many core charts or tiles to put in a row.
  ///
  /// "As many as fit" lands on whatever the window happens to allow - 9 on a
  /// typical screen - and 9 divides none of the counts real machines have.
  /// Core counts are 2, 4, 8, 16, 24, 32, so rows of 8 leave a clean
  /// rectangle where rows of 9 leave a ragged tail: 32 cores is 9+9+9+5
  /// against 8+8+8+8.
  ///
  /// Small hosts keep a single row - a 6-core box reads better as one row of
  /// six than as 4+2 - and above that the count snaps down to a multiple of
  /// eight, so a wide window gives 16 per row rather than 19.
  function niceCols(n, maxCols) {
    const fit = Math.max(1, Math.min(n || 1, maxCols));
    if (n <= 8 && n <= maxCols) return n;
    if (fit >= 8) return Math.floor(fit / 8) * 8;
    if (fit >= 4) return 4;
    if (fit >= 2) return 2;
    return 1;
  }

  return { band, logWindow, normalise, sliderToSecs, niceCols, WIN_MIN, WIN_MAX };
});
