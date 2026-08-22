// Tests for group aggregation.
//
// Names state the invariant, so a future session learns the rule from the
// failure message alone — the same convention the Rust tests follow.
//
//     node --test tests/
//
// Uses node's built-in runner. No npm, no bundler, no dependency.

const { test } = require('node:test');
const assert = require('node:assert');
const { aggregateGroup, canAggregate, groupHosts } = require('../src/agg.js');

// The fleet this project is actually developed against, which is also the
// clearest illustration of why mean-of-ratios is wrong: dove has eight times
// heron's cores, so the two are nothing like equal votes.
const CPU = { agg: 'ratio' };
const cpuMember = (host, pct, cores) => ({
  host,
  value: pct,
  parts: [pct * cores, cores],
});

test('mean_of_ratios_is_not_the_group_percentage', () => {
  // dove: 32 cores pinned. heron: 4 cores idle. 32 of 36 cores are busy.
  const g = aggregateGroup(CPU, [cpuMember('dove', 100, 32), cpuMember('heron', 0, 4)]);

  assert.ok(Math.abs(g.value - 88.89) < 0.01, `expected 88.9%, got ${g.value}`);
  // The number the naive implementation produces. Stated explicitly because
  // it is plausible, well-formatted, and wrong - the exact failure mode this
  // project exists in response to.
  assert.notStrictEqual(Math.round(g.value), 50);
});

test('equal_sized_hosts_still_average_normally', () => {
  // The weighting must not distort the case where it should not apply.
  const g = aggregateGroup(CPU, [cpuMember('a', 20, 8), cpuMember('b', 40, 8)]);
  assert.ok(Math.abs(g.value - 30) < 0.001);
});

test('severity_comes_from_the_worst_member_not_the_aggregate', () => {
  // One host on fire among four idle ones. The group must not render calm.
  const g = aggregateGroup(CPU, [
    cpuMember('a', 97, 4),
    cpuMember('b', 2, 4),
    cpuMember('c', 1, 4),
    cpuMember('d', 3, 4),
  ]);
  assert.ok(g.value < 30, `aggregate should be calm, got ${g.value}`);
  assert.strictEqual(g.severity, 97);
});

test('a_metric_with_no_agg_rule_is_excluded_not_defaulted', () => {
  // The rule that keeps the other rules from decaying: the next person to add
  // a metric must not get a plausible number from a default they never chose.
  assert.strictEqual(aggregateGroup({ label: 'Whatever' }, [{ host: 'a', value: 5 }]), null);
  assert.strictEqual(canAggregate({ agg: 'mean' }), false);
  assert.strictEqual(canAggregate({ agg: 'sum' }), true);
});

test('a_group_with_nothing_reporting_is_null_not_zero', () => {
  // Zero is a claim about the fleet. "Nothing answered" is not.
  const g = aggregateGroup(CPU, [
    { host: 'a', value: null },
    { host: 'b', value: undefined },
  ]);
  assert.strictEqual(g.value, null);
  assert.strictEqual(g.severity, null);
  assert.strictEqual(g.contributing, 0);
  assert.strictEqual(g.total, 2);
});

test('a_silent_member_is_counted_not_dropped', () => {
  // The count is part of the answer. A group summarising four hosts while
  // claiming to summarise five, silently, is the history plane's averaged-away
  // spike one level up.
  const g = aggregateGroup(CPU, [
    cpuMember('a', 50, 8),
    cpuMember('b', 50, 8),
    { host: 'c', value: null },
  ]);
  assert.strictEqual(g.contributing, 2);
  assert.strictEqual(g.total, 3);
  assert.strictEqual(g.partial, true);
});

test('a_ratio_with_no_denominator_is_null_not_zero', () => {
  // No member has swap configured. "0% swap used" would be a statement about
  // swap that does not exist.
  const SWAP = { agg: 'ratio' };
  const g = aggregateGroup(SWAP, [
    { host: 'a', value: 0, parts: [0, 0] },
    { host: 'b', value: 0, parts: [0, 0] },
  ]);
  assert.strictEqual(g.value, null);
});

test('memory_aggregates_by_bytes_not_by_host', () => {
  // A 128 GB box at 50% and a 4 GB box at 100% is 51.5% of the group's memory,
  // not 75%. Big hosts dominate because they are big.
  const MEM = { agg: 'ratio' };
  const g = aggregateGroup(MEM, [
    { host: 'big', value: 50, parts: [64 * 100, 128] },
    { host: 'small', value: 100, parts: [4 * 100, 4] },
  ]);
  assert.ok(Math.abs(g.value - 51.515) < 0.01, `got ${g.value}`);
  assert.strictEqual(g.severity, 100);
});

test('temperature_takes_the_max_because_a_mean_cools_a_throttling_host', () => {
  const TEMP = { agg: 'max' };
  const g = aggregateGroup(TEMP, [
    { host: 'a', value: 89 },
    { host: 'b', value: 31 },
    { host: 'c', value: 33 },
  ]);
  assert.strictEqual(g.value, 89);
  assert.strictEqual(g.min, 31);
});

test('rates_sum_and_name_their_largest_contributor', () => {
  // "Who is most of this?" is the question people actually ask of a total.
  const NET = { agg: 'sum' };
  const g = aggregateGroup(NET, [
    { host: 'dove', value: 9e6 },
    { host: 'heron', value: 1e6 },
    { host: 'wader', value: 2e6 },
  ]);
  assert.strictEqual(g.value, 12e6);
  assert.strictEqual(g.top.host, 'dove');
});

test('spread_distinguishes_a_calm_group_from_a_lopsided_one', () => {
  // Both average 50%. Only one of them is fine.
  const even = aggregateGroup(CPU, [cpuMember('a', 50, 8), cpuMember('b', 50, 8)]);
  const split = aggregateGroup(CPU, [cpuMember('a', 100, 8), cpuMember('b', 0, 8)]);
  assert.strictEqual(even.value, split.value);
  assert.strictEqual(even.max - even.min, 0);
  assert.strictEqual(split.max - split.min, 100);
});

test('cores_concatenate_into_one_grid', () => {
  const CORES = { agg: 'concat' };
  const g = aggregateGroup(CORES, [
    { host: 'a', value: 40, vector: [10, 40] },
    { host: 'b', value: 90, vector: [90, 5, 5] },
  ]);
  assert.deepStrictEqual(g.vector, [10, 40, 90, 5, 5]);
  assert.strictEqual(g.severity, 90);
});

test('ungrouped_hosts_are_not_swept_into_a_synthetic_group', () => {
  // A fleet with no groups configured must render exactly as it does today.
  const { groups, ungrouped } = groupHosts([
    { name: 'dove', group: 'workstations' },
    { name: 'heron' },
    { name: 'coot', group: 'workstations' },
    { name: 'wader', group: '  ' },
  ]);
  assert.strictEqual(groups.length, 1);
  assert.deepStrictEqual(groups[0].hosts.map(h => h.name), ['dove', 'coot']);
  assert.deepStrictEqual(ungrouped.map(h => h.name), ['heron', 'wader']);
});

test('groups_keep_the_order_they_first_appear_in', () => {
  // Host order is drag-to-reorder state the user set deliberately; groups must
  // inherit it rather than sorting themselves alphabetically behind their back.
  const { groups } = groupHosts([
    { name: 'a', group: 'zulu' },
    { name: 'b', group: 'alpha' },
    { name: 'c', group: 'zulu' },
  ]);
  assert.deepStrictEqual(groups.map(g => g.name), ['zulu', 'alpha']);
});
