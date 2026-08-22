// Group aggregation — the rules from ADR-008, and nothing else.
//
// This is the only place in Tuxtop that produces a number no machine
// reported. Everything else displays what a host said; a group card displays
// what we computed from several hosts, which is where a monitoring tool gets
// to be confidently wrong on its own account.
//
// The governing rule, from docs/specs/host-groups.md:
//
//     An aggregate must never be able to hide a member.
//
// Kept free of the DOM and of the app's host shape so it can be tested
// directly: `node --test tests/`. The caller maps its hosts through the metric
// registry into the flat `members` shape below.
//
// UMD rather than an ES module: index.html loads plain scripts under a CSP
// nonce, and the test runner needs `require`. Neither warrants a bundler.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxAgg = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  // How a metric combines across the hosts of a group.
  //
  //   'ratio'  recombine parts: sum(numerator) / sum(denominator). The only
  //            correct way to aggregate a percentage - see ADR-008.
  //   'sum'    the values add: rates, load average, counts.
  //   'max'    the worst member wins: temperature, disk fullness, swap.
  //   'concat' vector metrics: every member's tiles, end to end.
  //
  // There is deliberately no default. A metric with no rule is excluded from
  // group views rather than averaged, because an absent rule is a missing
  // decision, and the honest rendering of a missing decision is nothing.
  const KINDS = ['ratio', 'sum', 'max', 'concat'];

  function isNum(v) {
    return typeof v === 'number' && Number.isFinite(v);
  }

  /// Whether a metric can appear in a group view at all.
  function canAggregate(spec) {
    return !!spec && KINDS.indexOf(spec.agg) !== -1;
  }

  /// Combine one metric across a group's members.
  ///
  /// `members` is `[{ host, value, parts, vector }]`, one entry per host in the
  /// group *including hosts that are not reporting* — the count of those is
  /// part of the answer, so they must not be filtered out by the caller.
  ///
  ///   value   the member's scalar reading, or null/undefined if silent
  ///   parts   [numerator, denominator] for 'ratio', pre-scaled so that
  ///           numerator / denominator is exactly the value to display. CPU
  ///           passes [pct * cores, cores]; memory passes [usedGB * 100,
  ///           totalGB]. Carrying the scale in the parts rather than in a flag
  ///           removes a whole class of off-by-100 mistake.
  ///   vector  array of per-core values — required by 'concat' only
  ///
  /// Returns null when the metric declares no aggregation. Otherwise every
  /// field is present, and `value` is null when nothing contributed — never 0,
  /// which would render a dead group as a healthy idle one.
  function aggregateGroup(spec, members) {
    if (!canAggregate(spec)) return null;

    const all = members || [];
    const live = all.filter(m => isNum(m && m.value));
    const values = live.map(m => m.value);

    const out = {
      value: null,
      severity: null,
      min: null,
      max: null,
      contributing: live.length,
      total: all.length,
      top: null,
      vector: null,
      partial: live.length < all.length,
    };

    if (!live.length) return out;

    out.min = Math.min.apply(null, values);
    out.max = Math.max.apply(null, values);
    // Severity is always the worst member, never the aggregate. A group whose
    // mean is calm but which contains a host at 97% must not render calm.
    out.severity = out.max;

    const biggest = live.reduce((a, b) => (b.value > a.value ? b : a));
    out.top = { host: biggest.host, value: biggest.value };

    switch (spec.agg) {
      case 'ratio': {
        // Recombine the parts and divide once at the end. Averaging the
        // members' ratios instead is not an approximation of this number, it
        // is a different quantity that happens to share its units.
        let num = 0, den = 0;
        for (const m of live) {
          const p = m.parts;
          if (!p || !isNum(p[0]) || !isNum(p[1])) continue;
          num += p[0];
          den += p[1];
        }
        // No denominator means the question has no answer for this group -
        // e.g. no member has any swap configured. Zero would be a claim.
        out.value = den > 0 ? num / den : null;
        break;
      }
      case 'sum':
        out.value = values.reduce((a, b) => a + b, 0);
        break;
      case 'max':
        out.value = out.max;
        break;
      case 'concat': {
        const v = [];
        for (const m of live) if (Array.isArray(m.vector)) v.push.apply(v, m.vector);
        out.vector = v;
        out.value = v.length ? Math.max.apply(null, v) : null;
        out.severity = out.value;
        break;
      }
    }

    return out;
  }

  /// Split hosts into groups, preserving the order groups first appear in.
  ///
  /// Ungrouped hosts are not swept into a synthetic "Other": they render as
  /// themselves, exactly as they do with no groups configured at all.
  function groupHosts(hosts) {
    const groups = [];
    const index = new Map();
    const loose = [];

    for (const h of hosts || []) {
      const name = h && typeof h.group === 'string' ? h.group.trim() : '';
      if (!name) {
        loose.push(h);
        continue;
      }
      if (!index.has(name)) {
        index.set(name, { name, hosts: [] });
        groups.push(index.get(name));
      }
      index.get(name).hosts.push(h);
    }

    return { groups, ungrouped: loose };
  }

  return { aggregateGroup, canAggregate, groupHosts, KINDS };
});
