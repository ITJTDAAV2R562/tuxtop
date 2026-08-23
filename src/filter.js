// What to show, and in what order.
//
// The filtering and sorting decisions, separated from the DOM that renders
// them. Both filters here answer "does this row survive", and both have to
// reach the field that actually distinguishes rows - which in each case is
// not the one shown in the main column.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxFilter = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  /// Does this host match the host filter?
  ///
  /// Matches group as well as name, so typing a group name narrows to it.
  /// That composes with grouping rather than duplicating it: groups are the
  /// durable structure, the filter is the ad-hoc question.
  function matchesHost(h, q) {
    if (!q) return true;
    return `${h.name} ${h.group || ''} ${h.distro || ''}`.toLowerCase().includes(q);
  }

  /// Does this process match the process filter?
  ///
  /// Includes the command line, which is where the distinguishing detail
  /// lives: `comm` is capped at 15 characters by the kernel, so six processes
  /// all called "Runner.Listener" are only tellable apart by their arguments.
  function matchesProcess(p, q) {
    if (!q) return true;
    return `${p.host} ${p.user} ${p.comm} ${p.pid} ${p.cmd || ''}`
      .toLowerCase().includes(q);
  }

  /// Sort processes by one column, with a stable meaningful tiebreak.
  ///
  /// Equal values fall back to CPU then memory rather than keeping whatever
  /// order the fleet merge happened to produce, so sorting by host does not
  /// scramble the ranking within each host.
  ///
  /// `numeric` says whether the column compares as a number; a numeric column
  /// sorted as text puts 9 above 10.
  function sortProcs(list, key, desc, numeric) {
    return list.slice().sort((a, b) => {
      let r = numeric
        ? (a[key] - b[key])
        : String(a[key]).localeCompare(String(b[key]));
      if (desc) r = -r;
      return r || (b.cpu_pct - a.cpu_pct) || (b.rss_kb - a.rss_kb);
    });
  }

  return { matchesHost, matchesProcess, sortProcs };
});
