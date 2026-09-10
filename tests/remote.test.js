// The chrome's strings: whose readings these are, how old, and what can be
// saved.
//
// Every one of these decides something a person reads and believes. The
// failures worth naming are not "the label is ugly" - they are a window that
// says `sampling locally` while showing another machine's fleet, and a stale
// grid that says nothing.

const test = require('node:test');
const assert = require('node:assert');
const R = require('../src/remote.js');

test('an_endpoint_is_shown_without_its_scheme_but_with_its_port', () => {
  // The scheme distinguishes nothing - only http is possible, since the viewer
  // refuses https. The port distinguishes two servers on one box, so dropping
  // it would name the wrong machine.
  assert.equal(R.originLabel('http://dove:8787'), 'dove:8787');
  assert.equal(R.originLabel('http://dove:8787/'), 'dove:8787');
  assert.equal(R.originLabel('  http://dove  '), 'dove');
  assert.equal(R.originLabel('dove:9000'), 'dove:9000');
  assert.equal(R.originLabel('http://[::1]:8787'), '[::1]:8787');
});

test('nothing_to_show_is_null_rather_than_an_empty_label', () => {
  for (const v of [null, undefined, '', '   ', 42, {}, 'http://']) {
    assert.equal(R.originLabel(v), null, `${JSON.stringify(v)} produced a label`);
  }
});

test('a_window_says_which_machine_took_its_readings', () => {
  assert.equal(R.identity({ writable: true, endpoint: null }, null), R.LOCAL);
  assert.equal(R.identity({ writable: false, endpoint: 'http://dove:8787' }, null),
    'dove:8787');
});

test('a_browser_tab_is_a_remote_viewer_too', () => {
  // ADR-017 rule 1, noted 2026-09-07: a tab served by tuxtop-serve has been
  // pointed at a server since that shipped, and said nothing about it. The
  // server describes itself as sampling locally and is right to, so the tab
  // supplies the other half.
  const serverCaps = { writable: false, endpoint: null };
  assert.equal(R.identity(serverCaps, 'http://lutik:8787'), 'lutik:8787');
});

test('an_unanswered_capabilities_call_is_not_evidence_of_local_sampling', () => {
  // The claim this whole module exists to prevent. `sampling locally` beside
  // another machine's fleet is the founding bug with a new coat, so with no
  // answer at all the chrome says nothing rather than the wrong thing.
  assert.equal(R.identity(null, null), null);
  assert.equal(R.identity(undefined, undefined), null);
  // But an answer that names an endpoint still counts, however it arrived.
  assert.equal(R.identity(null, 'http://dove:8787'), 'dove:8787');
});

test('freshness_is_judged_against_the_threshold_the_server_sent', () => {
  // The rule itself lives in tuxtop_core::remote::stale_after_ms and travels
  // in capabilities, so this only applies it - a second copy of the rule here
  // is the thing that would drift.
  assert.equal(R.isStale(6000, 3000), true);
  assert.equal(R.isStale(6000, 15000), false, '6 s old against a 5 s sampler');
  assert.equal(R.isStale(3000, 3000), false, 'exactly at the threshold is not past it');
});

test('an_absent_threshold_does_not_become_an_invented_one', () => {
  // The only way it is absent is a server too old to send it, which is a
  // version difference - and capabilities.version_note states that in words.
  // Guessing a number here and judging against it is how a warning nobody
  // believes gets shipped.
  for (const v of [undefined, null, 0, -1, NaN, 'soon']) {
    assert.equal(R.isStale(999999, v), false, `${v} was treated as a threshold`);
  }
  assert.equal(R.isStale(undefined, 3000), false, 'and an unknown age judges nothing');
});

test('the_clock_time_is_local_and_zero_padded', () => {
  // Built from local parts on purpose, so this asserts the same thing in every
  // timezone the app runs in.
  const t = new Date(2026, 8, 10, 9, 3, 7).getTime();
  assert.equal(R.clockTime(t), '09:03:07');
  assert.equal(R.clockTime(new Date(2026, 8, 10, 23, 59, 59).getTime()), '23:59:59');
  assert.equal(R.clockTime(null), null);
  assert.equal(R.clockTime('nope'), null);
});

test('losing_the_server_names_the_endpoint_not_the_fleet', () => {
  // One dead link takes out all nineteen cards at once, which local mode has
  // never done. Nineteen cards captioned "offline" reads as a dead fleet - the
  // generic-offline failure the hard rules forbid - so the endpoint is the
  // subject of the sentence.
  const t = new Date(2026, 8, 10, 14, 3, 11).getTime();
  const note = R.staleNote('dove:8787', t);
  assert.equal(note, 'no readings from dove:8787 since 14:03:11');
  assert.match(note, /dove:8787/, 'the warning must name the link, not a host');
});

test('the_warning_claims_no_more_than_the_viewer_can_observe', () => {
  // The spec asked for "no contact with <endpoint>", and neither viewer can
  // observe contact: an SSE keep-alive is a comment, a comment dispatches no
  // event, and EventSource drops it silently. So a healthy server whose whole
  // fleet is paused sends nothing visible for minutes, and "no contact" would
  // be a confident false statement about a link that is fine - this project's
  // founding hazard in one word.
  const note = R.staleNote('dove:8787', Date.now());
  assert.match(note, /^no readings from /, note);
  assert.ok(!note.includes('contact'), `claims more than it knows: ${note}`);
  assert.ok(!note.includes('offline'), `the banned generic: ${note}`);
});

test('a_connection_that_never_succeeded_invents_no_timestamp', () => {
  // A typo'd endpoint has no last-seen instant, and `since 00:00:00` would be
  // a fabricated fact about a machine nobody ever reached.
  assert.equal(R.staleNote('dove:8787', null), 'no readings from dove:8787 yet');
  assert.equal(R.staleNote('dove:8787', undefined), 'no readings from dove:8787 yet');
  // Sampling locally has no link to lose; per-host faults cover a dead host.
  assert.equal(R.staleNote(null, Date.now()), null);
});

test('an_empty_field_means_the_local_fleet_not_a_server_named_nothing', () => {
  // Clearing the field is how you switch back, so the difference between "" and
  // null is one of this feature's two directions - and `use_endpoint` treats
  // them the same way at the other end, deliberately, because a switch that
  // depended on which of the two arrived would be a switch with a second door.
  assert.equal(R.endpointInput(''), null);
  assert.equal(R.endpointInput('   '), null);
  assert.equal(R.endpointInput(null), null);
  assert.equal(R.endpointInput(undefined), null);

  // What was typed, trimmed, and otherwise untouched: `https://`, a path and a
  // bad port are refused by tuxtop_core::remote::parse_endpoint, which is where
  // that rule lives. A second copy here would be a second thing to drift.
  assert.equal(R.endpointInput('  dove:8787 '), 'dove:8787');
  assert.equal(R.endpointInput('http://dove:8787'), 'http://dove:8787');
  assert.equal(R.endpointInput('https://dove:8787'), 'https://dove:8787',
    'the refusal belongs to the backend, and it names the fix');
});

test('one_server_written_two_ways_is_one_row_not_two', () => {
  // `[settings] server` holds what was typed and a saved entry holds what was
  // typed then, so the marker on the row currently being watched has to see
  // through the scheme. Otherwise saving the endpoint you are on adds a row
  // that looks like somewhere else.
  assert.equal(R.sameEndpoint('dove:8787', 'http://dove:8787'), true);
  assert.equal(R.sameEndpoint('http://DOVE:8787', 'dove:8787'), true);
  // The port is not noise: two servers on one box differ only by it.
  assert.equal(R.sameEndpoint('dove:8787', 'dove:8788'), false);
  assert.equal(R.sameEndpoint('dove:8787', 'coot:8787'), false);
  // Nothing is not a match for nothing - sampling locally is not "the same
  // server" as sampling locally, it is no server at all.
  assert.equal(R.sameEndpoint(null, null), false);
  assert.equal(R.sameEndpoint('', 'dove:8787'), false);
});

test('the_fleet_half_of_settings_needs_a_writable_backend', () => {
  // A read-only server drew a live interval field and a live history limit and
  // returned 403 on Save - two controls that could only fail, before remote
  // mode existed.
  assert.equal(R.editable({ writable: true }, null).fleet, true);
  assert.equal(R.editable({ writable: false }, null).fleet, false);
  assert.equal(R.editable(null, null).fleet, false, 'no answer is not permission');
});

test('the_viewer_half_of_settings_saves_even_in_remote_mode', () => {
  // The exception that makes the Settings split load-bearing: always_on_top is
  // a property of *this* window, and a remote viewer that could not be pinned
  // because pinning is a "setting" would be absurd.
  const remote = { writable: false, endpoint: 'http://dove:8787' };
  assert.deepEqual(R.editable(remote, null), { fleet: false, viewer: true });

  // A tab is the case that cannot: it has no window to keep on top, and no
  // updater whose check could be turned off.
  assert.deepEqual(R.editable(remote, 'http://lutik:8787'),
    { fleet: false, viewer: false });
  assert.deepEqual(R.editable({ writable: true }, 'http://lutik:8787'),
    { fleet: true, viewer: false }, 'a writable server still configures its fleet');
});

test('the_status_line_does_not_claim_an_ssh_this_machine_has_not_opened', () => {
  // In remote mode the ssh belongs to the server. Saying "over ssh" here is a
  // small false claim about which machine is connected to the fleet.
  assert.equal(
    R.modeLine({ version: '0.7.0', rate: '1 s', overridden: 0, label: null }),
    'Tuxtop 0.7.0 · live · 1 s over ssh');
  assert.equal(
    R.modeLine({ version: '0.7.0', rate: '1 s', overridden: 0, label: 'dove:8787' }),
    'Tuxtop 0.7.0 · live · 1 s via dove:8787');
});

test('per_host_overrides_are_still_named_so_the_global_rate_is_not_read_as_the_whole_story', () => {
  assert.match(R.modeLine({ rate: '1 s', overridden: 1 }), /1 host at its own rate$/);
  assert.match(R.modeLine({ rate: '1 s', overridden: 3 }), /3 hosts at its own rate$/);
  assert.equal(R.modeLine({ rate: '4 Hz' }), 'live · 4 Hz over ssh',
    'no version yet is a missing prefix, not the string "undefined"');
  assert.equal(R.modeLine(), 'live · ? over ssh');
});
