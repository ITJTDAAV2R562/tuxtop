// Value-to-visual mapping: how big, how red, how many.

const { test } = require('node:test');
const assert = require('node:assert');
const { band, logWindow, normalise, sliderToSecs, niceCols, WIN_MIN, WIN_MAX } =
  require('../src/scale.js');

const PCT = { scale: 'absolute', max: 100 };
const RATE = { scale: 'log', floor: 1e6, decades: 4 };

test('the_load_bands_change_exactly_at_75_and_90', () => {
  // Shared by tiles, bars and charts; an off-by-one here would make the same
  // load read differently in two places on the same screen.
  assert.strictEqual(band(74.9), 'cool');
  assert.strictEqual(band(75), 'warn');
  assert.strictEqual(band(89.9), 'warn');
  assert.strictEqual(band(90), 'crit');
});

test('a_percentage_ignores_the_fleet_peak', () => {
  // Absolute metrics must mean the same thing on every host regardless of
  // what the busiest one is doing, or bars stop being comparable.
  assert.strictEqual(normalise(PCT, 50, 100), 0.5);
  assert.strictEqual(normalise(PCT, 50, 1e9), 0.5);
});

test('a_percentage_over_its_ceiling_clamps_instead_of_overflowing', () => {
  // Load average briefly above 100% must not draw a bar past its track.
  assert.strictEqual(normalise(PCT, 140, 100), 1);
});

test('a_log_axis_spans_a_fixed_number_of_decades_below_the_peak', () => {
  // Anchoring at 1 byte crushed everything above a megabyte into the top
  // third - a 600x difference rendered as 69% against 100%.
  const w = logWindow(RATE, 1e7);
  assert.strictEqual(w.top, 1e7);
  assert.strictEqual(w.bottom, 1e3);
});

test('a_quiet_host_reads_as_empty_rather_than_as_a_sliver', () => {
  assert.strictEqual(normalise(RATE, 0, 1e7), 0);
  assert.strictEqual(normalise(RATE, 500, 1e7), 0, 'below the window floor');
  assert.strictEqual(normalise(RATE, 1e7, 1e7), 1, 'the peak fills the bar');
});

test('a_log_axis_gives_each_decade_the_same_length', () => {
  // The property that makes the axis readable: 10x is always the same
  // distance, wherever on the bar it happens.
  const a = normalise(RATE, 1e4, 1e7);
  const b = normalise(RATE, 1e5, 1e7);
  const c = normalise(RATE, 1e6, 1e7);
  assert.ok(Math.abs((b - a) - (c - b)) < 1e-9, `${a} ${b} ${c}`);
});

test('an_empty_fleet_does_not_collapse_the_log_window', () => {
  // With no traffic the peak is 0; the floor keeps the axis meaningful
  // instead of dividing by zero.
  const w = logWindow(RATE, 0);
  assert.strictEqual(w.top, 1e6);
  assert.ok(w.bottom > 0);
});

test('the_history_slider_spans_exactly_a_minute_to_a_week', () => {
  assert.strictEqual(sliderToSecs(0), WIN_MIN);
  assert.strictEqual(sliderToSecs(0), 60);
  assert.strictEqual(sliderToSecs(1000), WIN_MAX);
  assert.strictEqual(sliderToSecs(1000), 604800);
});

test('the_history_slider_is_log_spaced_not_linear', () => {
  // Halfway must be the geometric middle, or the useful short windows all
  // crowd into the first pixel.
  const mid = sliderToSecs(500);
  assert.ok(mid > 5000 && mid < 7000, `got ${mid}`);
});

test('core_rows_snap_to_eight_rather_than_to_whatever_fits', () => {
  // 9 fits on a typical screen and divides none of the counts real machines
  // have: 32 cores is 9+9+9+5 against 8+8+8+8.
  assert.strictEqual(niceCols(32, 9), 8);
  assert.strictEqual(niceCols(24, 9), 8);
  assert.strictEqual(niceCols(16, 9), 8);
});

test('a_wide_window_steps_up_a_whole_multiple_of_eight', () => {
  assert.strictEqual(niceCols(32, 19), 16);
  assert.strictEqual(niceCols(32, 8), 8);
});

test('a_small_host_keeps_a_single_row', () => {
  // A 6-core box reads better as one row of six than as 4+2.
  assert.strictEqual(niceCols(6, 9), 6);
  assert.strictEqual(niceCols(4, 9), 4);
  assert.strictEqual(niceCols(1, 9), 1);
});

test('a_narrow_window_never_asks_for_more_columns_than_fit', () => {
  // The layout is measured before it is drawn; returning more than fit would
  // overflow the panel rather than wrap.
  for (const n of [1, 2, 4, 6, 8, 12, 16, 24, 32, 64]) {
    for (const max of [1, 2, 3, 5, 8, 9, 20]) {
      const c = niceCols(n, max);
      assert.ok(c >= 1 && c <= max, `niceCols(${n}, ${max}) = ${c}`);
      assert.ok(c <= Math.max(n, 1), `niceCols(${n}, ${max}) = ${c} exceeds the cores`);
    }
  }
});
