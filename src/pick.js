// Picking the reading that matters out of the many a host reports.
//
// A recurring problem in this app rather than an incidental one: a box has
// several filesystems and several temperature sensors, and choosing wrongly
// among them produces a confident wrong number - the exact failure the
// project exists in response to. The fullest mount, never the average. The
// CPU's sensor, never merely the hottest.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxPick = factory();
})(typeof self !== 'undefined' ? self : this, function () {
  'use strict';

  /// The fullest filesystem a host reports, or null if it reports none.
  ///
  /// An average across mounts would let a roomy /home hide a full /, which is
  /// the single most common way a Linux box falls over.
  function fullestFs(h) {
    if (!h.fs || !h.fs.length) return null;
    return h.fs.reduce((worst, f) => {
      const pct = f.total_kb ? f.used_kb / f.total_kb * 100 : 0;
      const wp = worst ? (worst.total_kb ? worst.used_kb / worst.total_kb * 100 : 0) : -1;
      return pct > wp ? f : worst;
    }, null);
  }

  /// Percentage of the fullest mount, or null if the host reports none.
  ///
  /// Null rather than zero: a host that has not sent a `df` frame yet has not
  /// told us its disks are empty.
  function fsPct(h) {
    const f = fullestFs(h);
    return f && f.total_kb ? f.used_kb / f.total_kb * 100 : null;
  }

  /// Name one temperature sensor.
  ///
  /// Unlabelled sensors are numbered within their driver: dove's board
  /// exposes four `gigabyte_wmi` inputs, and calling them all "gigabyte_wmi"
  /// would show one reading and silently hide three.
  function sensorName(t, indexWithinDriver) {
    if (!t.label) {
      return indexWithinDriver > 0 ? `${t.driver} ${indexWithinDriver + 1}` : t.driver;
    }
    return `${t.driver} ${t.label}`;
  }

  /// History series key for one sensor.
  ///
  /// Mirrors `sensor_key` in `history_store.rs`; the two must agree or a
  /// chart asks for a series nothing writes and renders "no history yet"
  /// forever. Keyed by driver and label rather than by position, because
  /// plugging in a drive shifts every index after it and a chart would
  /// quietly continue someone else's history under the same name.
  function sensorMetric(t) {
    return t.label
      ? `temp.${t.driver}.${String(t.label).replace(/ /g, '_')}`
      : `temp.${t.driver}.${t.idx || 0}`;
  }

  /// The hottest sensor on a host, whatever it is attached to.
  ///
  /// Deliberately *not* the CPU temperature - an NVMe under load routinely
  /// beats the CPU, and on dove it does so by 40 degrees. Reporting that as
  /// "CPU temperature" would name the wrong component with total confidence.
  /// Always used with its name attached, because 72C is alarming for a CPU
  /// and unremarkable for a drive.
  function hottestSensor(h) {
    if (!h.temps || !h.temps.length) return null;
    return h.temps.reduce((a, b) => (b.celsius > a.celsius ? b : a));
  }

  return { fullestFs, fsPct, sensorName, sensorMetric, hottestSensor };
});
