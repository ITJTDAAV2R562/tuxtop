// What to show, and in what order.

const { test } = require('node:test');
const assert = require('node:assert');
const { matchesHost, matchesProcess, sortProcs } = require('../src/filter.js');

test('a_host_filter_matches_its_group_as_well_as_its_name', () => {
  // So typing a group name narrows to it. That composes with grouping rather
  // than duplicating it.
  const h = { name: 'wader', group: 'servers', distro: 'Debian 13' };
  assert.ok(matchesHost(h, 'wader'));
  assert.ok(matchesHost(h, 'servers'));
  assert.ok(matchesHost(h, 'debian'));
  assert.ok(!matchesHost(h, 'workstations'));
});

test('an_empty_host_filter_matches_everything', () => {
  // The no-filter case must not accidentally hide the fleet.
  assert.ok(matchesHost({ name: 'dove' }, ''));
  assert.ok(matchesHost({ name: 'dove' }, undefined));
});

test('a_host_with_no_group_still_matches_by_name', () => {
  // Missing fields must not throw or stringify as "undefined" and match a
  // filter of "undef".
  const h = { name: 'owl' };
  assert.ok(matchesHost(h, 'owl'));
  assert.ok(!matchesHost(h, 'undefined'));
});

test('a_process_filter_reaches_the_command_line', () => {
  // comm is capped at 15 characters, so six processes all called
  // "Runner.Listener" are only tellable apart by their arguments.
  const p = { host: 'dove', user: 'gha', comm: 'Runner.Listener', pid: 2316664,
              cmd: '/home/gha/actions-runner-3/bin/Runner.Listener run' };
  assert.ok(matchesProcess(p, 'pdr-3'));
  assert.ok(matchesProcess(p, 'runner.listener'));
  assert.ok(matchesProcess(p, '2316664'), 'pid is searchable');
  assert.ok(!matchesProcess(p, 'pdr-4'));
});

test('a_unit_name_finds_its_processes_even_when_the_command_differs', () => {
  // manticore.service runs a binary called searchd. Without the owner in the
  // haystack, filtering by the unit you are investigating finds nothing.
  const p = { host: 'dove', user: 'root', comm: 'searchd', pid: 1906570,
              cmd: '/usr/bin/searchd --config /etc/sphinxsearch/sphinx.conf',
              owner: 'manticore.service' };
  assert.ok(matchesProcess(p, 'manticore'));
  assert.ok(matchesProcess(p, 'searchd'), 'the command still matches too');
});

test('a_process_with_no_command_line_is_still_searchable', () => {
  // Kernel threads have none; they must not throw or match "undefined".
  const p = { host: 'dove', user: 'root', comm: 'kworker/3:1', pid: 68 };
  assert.ok(matchesProcess(p, 'kworker'));
  assert.ok(!matchesProcess(p, 'undefined'));
});

const LIST = [
  { host: 'dove',  pid: 1, cpu_pct: 10, rss_kb: 100, comm: 'b' },
  { host: 'aaa',   pid: 2, cpu_pct: 50, rss_kb: 200, comm: 'a' },
  { host: 'wader', pid: 3, cpu_pct: 10, rss_kb: 900, comm: 'c' },
];

test('a_numeric_column_sorts_as_numbers_not_as_text', () => {
  // Sorted as text, 9 comes above 10 and the busiest process is not at the
  // top - which is the one thing this table exists to show.
  const l = [{ cpu_pct: 9, rss_kb: 1 }, { cpu_pct: 10, rss_kb: 1 }];
  assert.deepStrictEqual(
    sortProcs(l, 'cpu_pct', true, true).map(p => p.cpu_pct), [10, 9]);
});

test('equal_values_fall_back_to_cpu_then_memory', () => {
  // Sorting by host must not scramble the ranking within each host - two
  // processes tied on the sort column keep a meaningful order.
  const byHost = sortProcs(LIST, 'host', false, false);
  assert.deepStrictEqual(byHost.map(p => p.host), ['aaa', 'dove', 'wader']);

  const tied = sortProcs(
    [{ host: 'x', cpu_pct: 10, rss_kb: 100 }, { host: 'x', cpu_pct: 10, rss_kb: 900 }],
    'host', false, false);
  assert.deepStrictEqual(tied.map(p => p.rss_kb), [900, 100], 'memory breaks the tie');
});

test('sorting_does_not_mutate_the_list_it_was_given', () => {
  // The caller holds the fleet list; sorting a view of it must not reorder
  // the source and quietly change what the next render iterates.
  const before = LIST.map(p => p.pid);
  sortProcs(LIST, 'cpu_pct', true, true);
  assert.deepStrictEqual(LIST.map(p => p.pid), before);
});

test('direction_reverses_the_column_but_not_the_tiebreak', () => {
  // The tiebreak is always "busiest first"; flipping it too would make an
  // ascending sort put the quietest tied process at the top of its group,
  // which is never what is wanted.
  const asc = sortProcs(LIST, 'cpu_pct', false, true);
  assert.strictEqual(asc[asc.length - 1].cpu_pct, 50);
  assert.strictEqual(asc[0].rss_kb, 900, 'tied at 10%, memory still descends');
});

test('a_paused_host_answers_to_the_word_paused', () => {
  // How you find the two boxes you took down three weeks ago in a fleet of
  // nineteen. Without this the only way back to a paused host is scrolling.
  const paused = { name: 'wader', group: 'physical', paused: true };
  const watched = { name: 'dove', group: 'physical', paused: false };

  assert.ok(matchesHost(paused, 'paused'));
  assert.ok(!matchesHost(watched, 'paused'), 'a watched host must not match');
  // And the word is added, not substituted: the normal filters still work.
  assert.ok(matchesHost(paused, 'wader'));
  assert.ok(matchesHost(paused, 'physical'));
});

test('the_word_paused_is_not_read_out_of_a_hosts_own_name', () => {
  // The flag is appended only when true, so "paused" as a filter means the
  // state and never a host that happens to be called after it.
  assert.ok(!matchesHost({ name: 'dove', group: null }, 'paused'));
  assert.ok(matchesHost({ name: 'paused-rig', group: null }, 'paused'),
    'a literal name still matches on its own text');
});
