// Display formatting. Each name states the invariant, so a failure teaches
// the rule without anyone reading the test body.

const { test } = require('node:test');
const assert = require('node:assert');
const { bps, gb, fmtKb, fmtSpan, humanUptime, shortGpu, rateLabel } = require('../src/format.js');

test('bytes_are_whole_but_larger_units_keep_a_decimal', () => {
  // "1013.0 B/s" is false precision on a counter that moves in packets, while
  // 1.4 MB/s against 1 MB/s is a 40% difference the reader should see.
  assert.strictEqual(bps(1013), '1013 B/s');
  assert.strictEqual(bps(1024), '1.0 KB/s');
  assert.strictEqual(bps(1.5 * 1048576), '1.5 MB/s');
});

test('a_rate_stops_climbing_units_at_gigabytes', () => {
  // Beyond GB/s the unit table runs out; it must saturate, not read undefined.
  assert.ok(bps(5 * 1024 ** 4).endsWith('GB/s'), bps(5 * 1024 ** 4));
});

test('a_zero_or_absent_rate_is_zero_bytes_not_NaN', () => {
  assert.strictEqual(bps(0), '0 B/s');
  assert.strictEqual(bps(undefined), '0 B/s');
  assert.strictEqual(bps(null), '0 B/s');
});

test('memory_switches_unit_exactly_at_the_boundary', () => {
  assert.strictEqual(fmtKb(1023), '1023 KB');
  assert.strictEqual(fmtKb(1024), '1 MB');
  assert.strictEqual(fmtKb(1048575), '1024 MB');
  assert.strictEqual(fmtKb(1048576), '1.0 GB');
});

test('a_span_reads_in_the_unit_that_says_the_most', () => {
  // The thresholds are deliberately not the unit boundaries: 90 seconds reads
  // better as "2 min", and 90 minutes better as "1.5 hr".
  assert.strictEqual(fmtSpan(60), '60 sec');
  assert.strictEqual(fmtSpan(89), '89 sec');
  assert.strictEqual(fmtSpan(90), '2 min');
  assert.strictEqual(fmtSpan(3600), '60 min');
  assert.strictEqual(fmtSpan(5400), '1.5 hr');
  assert.strictEqual(fmtSpan(604800), '7.0 days');
});

test('absent_uptime_is_blank_not_a_host_that_just_booted', () => {
  assert.strictEqual(humanUptime(null), '');
  assert.strictEqual(humanUptime(undefined), '');
  assert.strictEqual(humanUptime(0), '0m');
});

test('uptime_shows_the_two_largest_non_zero_units', () => {
  assert.strictEqual(humanUptime(90), '1m');
  assert.strictEqual(humanUptime(3600 * 5 + 60 * 7), '5h 7m');
  assert.strictEqual(humanUptime(86400 * 9 + 3600 * 22), '9d 22h');
});

test('a_gpu_keeps_the_model_and_drops_the_vendor', () => {
  // Vendor and product line are identical across a fleet; the model is the
  // part that identifies the card.
  assert.strictEqual(shortGpu('NVIDIA GeForce RTX 3080'), 'RTX 3080');
  assert.strictEqual(shortGpu('Tesla T4'), 'T4');
});

test('an_unrecognisable_gpu_name_still_labels_the_chip', () => {
  // An empty chip would read as a rendering fault rather than a dull name.
  assert.strictEqual(shortGpu('NVIDIA'), 'GPU');
  assert.strictEqual(shortGpu(''), 'GPU');
});

test('gigabytes_keep_one_decimal', () => {
  assert.strictEqual(gb(31.04), '31.0');
});

test('a_sample_rate_reads_as_the_rate_it_actually_is', () => {
  // The bug this replaces: the status line said "1 Hz" as a string literal,
  // whatever the interval setting was. A fleet sampled every 30 s sat under a
  // footer claiming 1 Hz - a confident number nobody had measured.
  assert.strictEqual(rateLabel(250), '4 Hz');
  assert.strictEqual(rateLabel(500), '2 Hz');
  assert.strictEqual(rateLabel(1000), '1 s');
  assert.strictEqual(rateLabel(2000), '2 s');
  assert.strictEqual(rateLabel(30000), '30 s');
  assert.strictEqual(rateLabel(60000), '60 s');
});

test('sub_second_reads_in_Hz_and_slower_reads_as_a_duration', () => {
  // Both are offered in the settings dialog in those terms, and "0.033 Hz" is
  // nobody's idea of thirty seconds.
  assert.ok(rateLabel(250).endsWith('Hz'));
  assert.ok(rateLabel(5000).endsWith('s'));
});

test('an_unusable_interval_says_so_rather_than_inventing_a_rate', () => {
  // Guessing here would reintroduce exactly the bug above.
  for (const bad of [0, -1, NaN, Infinity, null, undefined, '1000', {}]) {
    assert.strictEqual(rateLabel(bad), '?', `${String(bad)} must not produce a rate`);
  }
});
