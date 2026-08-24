// Heatmap row reduction. The rule under test is ADR-011: a cell shows the
// worst sample in its bucket, because the view exists to make spikes visible
// and a mean is what hides them.

const test = require('node:test');
const assert = require('node:assert');
const { heatRow, coverage, heatOrder, groupBreaks } = require('../src/heat.js');

const win = { from: 1000, to: 1100 };
const pt = (t, min, mean, max) => ({ t, min, mean, max });

test('a_cell_shows_the_bucket_max_not_its_mean', () => {
  // One 100% sample among ten idle ones, all inside the first cell.
  const pts = [pt(1000, 0, 0, 0), pt(1001, 0, 0, 100), pt(1002, 0, 0, 0)];
  const cells = heatRow(pts, win, 10);
  assert.equal(cells[0].v, 100, 'the spike must survive the reduction');
  // The mean is carried for the hover text, and it is emphatically not 100 --
  // which is exactly why colouring by it would hide the spike.
  assert.ok(cells[0].mean < 1, 'mean of the same bucket is near zero');
});

test('an_empty_bucket_is_a_gap_not_a_zero', () => {
  const cells = heatRow([pt(1000, 0, 5, 5)], win, 10);
  assert.equal(cells[0].v, 5);
  // Nothing arrived for the rest of the window. Zero would draw as "idle",
  // which is a claim we cannot make about a host that said nothing.
  assert.equal(cells[5].v, null);
  assert.equal(cells[5].n, 0);
});

test('points_land_by_timestamp_not_by_index', () => {
  // A host that reported only at the very end of the window. By index these
  // two points would fill the first two cells; by time they belong at the end.
  const cells = heatRow([pt(1090, 0, 0, 40), pt(1099, 0, 0, 60)], win, 10);
  assert.equal(cells[0].v, null);
  assert.equal(cells[9].v, 60, 'both fall in the last tenth of the window');
});

test('the_final_instant_lands_in_the_last_column_not_past_it', () => {
  // t === win.to is the classic off-by-one: floor() alone yields index cols.
  const cells = heatRow([pt(1100, 0, 0, 77)], win, 10);
  assert.equal(cells.length, 10);
  assert.equal(cells[9].v, 77);
});

test('points_outside_the_window_are_dropped', () => {
  const cells = heatRow([pt(500, 0, 0, 99), pt(5000, 0, 0, 99)], win, 10);
  assert.equal(coverage(cells), 0);
});

test('coverage_distinguishes_a_quiet_host_from_a_silent_one', () => {
  const quiet = heatRow(
    Array.from({ length: 10 }, (_, i) => pt(1000 + i * 10, 0, 0, 0)), win, 10);
  const silent = heatRow([], win, 10);
  assert.equal(coverage(quiet), 1, 'reporting zeros is full coverage');
  assert.equal(coverage(silent), 0, 'reporting nothing is none');
});

test('rows_cluster_by_group_with_ungrouped_last', () => {
  const hosts = [
    { name: 'zebra' }, { name: 'dove', group: 'physical' },
    { name: 'alpha' }, { name: 'coot', group: 'physical' },
    { name: 'roller', group: 'VM' },
  ];
  const order = heatOrder(hosts).map(h => h.name);
  // Groups sort case-insensitively, so "physical" precedes "VM" -- alphabetical
  // as a reader means it, not as ASCII means it, where every capital sorts
  // ahead of every lowercase and "VM" would jump the list.
  assert.deepEqual(order, ['dove', 'coot', 'roller', 'zebra', 'alpha']);
});

test('order_within_a_group_is_the_callers_not_ours', () => {
  // The caller has already applied the toolbar's Sort control. Re-sorting by
  // name here would leave a visible control that does nothing -- which is
  // exactly what shipped in 98472b2 and this pins shut.
  const byLoad = [
    { name: 'quiet', group: 'g' },
    { name: 'busiest', group: 'g' },
    { name: 'middling', group: 'g' },
  ];
  assert.deepEqual(heatOrder(byLoad).map(h => h.name),
                   ['quiet', 'busiest', 'middling']);
});

test('group_breaks_mark_every_boundary_and_nothing_else', () => {
  const ordered = heatOrder([
    { name: 'a', group: 'x' }, { name: 'b', group: 'x' },
    { name: 'c', group: 'y' }, { name: 'd' },
  ]);
  assert.deepEqual(groupBreaks(ordered), [2, 3]);
  assert.deepEqual(groupBreaks([]), []);
});

test('the_ramp_uses_the_same_thresholds_as_tiles_and_bars', () => {
  const { ramp } = require('../src/heat.js');
  // Below 75 it is climbing out of the ground colour toward accent...
  assert.deepEqual(ramp(0), { a: '--heat-ground', b: '--accent', k: 0 });
  assert.equal(ramp(0.74).b, '--accent');
  // ...75 is where warn begins, and 90 where crit does. These are band()'s
  // numbers; if band() ever moves, this test says so.
  assert.equal(ramp(0.80).a, '--accent');
  assert.equal(ramp(0.80).b, '--warn');
  assert.equal(ramp(0.95).a, '--warn');
  assert.equal(ramp(0.95).b, '--crit');
  // Exact equality would be wrong to demand: (1 - 0.90) / 0.10 lands on
  // 0.9999999999999998 in binary floating point, and a colour mix cannot
  // tell the difference.
  assert.ok(Math.abs(ramp(1).k - 1) < 1e-9);
});

test('the_ramp_clamps_rather_than_extrapolating', () => {
  const { ramp } = require('../src/heat.js');
  // A log-scaled metric can hand us a value above its window's top.
  assert.equal(ramp(4).b, '--crit');
  assert.ok(ramp(4).k <= 1);
  assert.equal(ramp(-1).k, 0);
});

test('mixHex_interpolates_and_survives_an_unresolved_token', () => {
  const { mixHex } = require('../src/heat.js');
  assert.equal(mixHex('#000000', '#ffffff', 0.5), 'rgb(128,128,128)');
  assert.equal(mixHex('#000', '#fff', 0), 'rgb(0,0,0)');
  // getPropertyValue returns '' for a token that does not exist. Averaging
  // that with black would paint a plausible colour over a real bug.
  assert.equal(mixHex('#ff0000', '', 0.5), '#ff0000');
});
