// Turning a host's history into a row of heatmap cells.
//
// The reduction from many samples to one cell is `max`, never `mean` - see
// ADR-011. A mean would render a host that pinned 100% for twenty seconds
// inside a two-minute bucket as a pale 14% cell, which is the arithmetic this
// whole project exists in response to.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxHeat = factory();
}(typeof self !== 'undefined' ? self : this, function () {

  /**
   * Lay `points` onto a fixed grid of `cols` cells spanning `win`.
   *
   * Points are placed by timestamp, never by index: a host that went quiet has
   * fewer points, and indexing would slide its whole history sideways so that
   * an outage on one host silently shifted every reading on it. This is the
   * same rule Phase 11c settled for group history.
   *
   * @param {{t:number,min:number,mean:number,max:number}[]} points
   * @param {{from:number,to:number}} win window in unix seconds
   * @param {number} cols how many cells the row is drawn with
   * @returns {{v:number|null,min:number,mean:number,n:number}[]} one entry per
   *   column; `v` is null where no sample landed, which is drawn as a gap
   *   rather than as zero.
   */
  function heatRow(points, win, cols) {
    const n = Math.max(1, cols | 0);
    const span = Math.max(1, win.to - win.from);
    const cells = new Array(n);
    for (let i = 0; i < n; i++) cells[i] = { v: null, min: 0, mean: 0, n: 0 };

    for (const p of points || []) {
      if (p == null || p.t < win.from || p.t > win.to) continue;
      // The final instant belongs to the last column, not to one past the end:
      // floor() alone puts t === win.to at index n.
      const i = Math.min(n - 1, Math.floor((p.t - win.from) / span * n));
      const c = cells[i];
      // ADR-011: the cell takes the worst sample in its bucket.
      c.v = c.v === null ? p.max : Math.max(c.v, p.max);
      c.min = c.n === 0 ? p.min : Math.min(c.min, p.min);
      c.mean += p.mean;
      c.n += 1;
    }
    for (const c of cells) if (c.n) c.mean /= c.n;
    return cells;
  }

  /**
   * How much of a row actually has data, as a fraction of its columns.
   *
   * Reported in words next to the strip. A row that is 40% gaps looks much
   * like a quiet one at a glance, and the difference between "idle" and "not
   * reporting" is the whole point of keeping faults distinguishable.
   */
  function coverage(cells) {
    if (!cells.length) return 0;
    return cells.filter(c => c.v !== null).length / cells.length;
  }

  /**
   * Order hosts for display: grouped hosts first, by group name, then the
   * ungrouped. Every host keeps its own row - the heatmap is the one view with
   * room for all nineteen, so nothing is aggregated and ADR-008 has nothing to
   * hide behind.
   */
  function heatOrder(hosts) {
    const key = h => (h.group ? '0' + h.group : '1') + ' ' + h.name;
    return [...hosts].sort((a, b) => key(a).localeCompare(key(b)));
  }

  /** Where the group label changes, so the view can rule a line between them. */
  function groupBreaks(ordered) {
    const out = [];
    for (let i = 1; i < ordered.length; i++) {
      if ((ordered[i].group || '') !== (ordered[i - 1].group || '')) out.push(i);
    }
    return out;
  }

  /**
   * Where a normalised load sits on the shared three-band ramp.
   *
   * The same thresholds the core tiles and fleet bars use (75 and 90), so a
   * red cell and a red tile mean the same thing - ADR-005 encodes load three
   * ways at once, and a fourth encoding that disagreed would undo that.
   * Between the stops it interpolates, because a strip of only three colours
   * cannot tell 5% from 70% and most of a fleet's life is spent down there.
   *
   * @param {number} t normalised 0..1
   * @returns {{a:string,b:string,k:number}} two CSS custom properties to mix,
   *   and how far between them (0 = all `a`).
   */
  function ramp(t) {
    const v = Math.max(0, Math.min(1, t || 0));
    if (v <= 0.75) return { a: '--heat-ground', b: '--accent', k: v / 0.75 };
    if (v <= 0.90) return { a: '--accent', b: '--warn', k: (v - 0.75) / 0.15 };
    return { a: '--warn', b: '--crit', k: (v - 0.90) / 0.10 };
  }

  /** Mix two `#rgb`/`#rrggbb` colours. `k` of 0 returns `a`. */
  function mixHex(a, b, k) {
    const parse = h => {
      const s = String(h).trim().replace('#', '');
      const f = s.length === 3 ? s.split('').map(c => c + c).join('') : s;
      const n = parseInt(f, 16);
      return Number.isNaN(n) ? null : [(n >> 16) & 255, (n >> 8) & 255, n & 255];
    };
    const x = parse(a), y = parse(b);
    // A token that did not resolve is a bug to see, not one to average with
    // black: fall back to whichever side parsed.
    if (!x || !y) return x ? a : b;
    const f = Math.max(0, Math.min(1, k || 0));
    const c = i => Math.round(x[i] + (y[i] - x[i]) * f);
    return `rgb(${c(0)},${c(1)},${c(2)})`;
  }

  return { heatRow, coverage, heatOrder, groupBreaks, ramp, mixHex };
}));
