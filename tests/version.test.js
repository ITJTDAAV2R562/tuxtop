const test = require('node:test');
const assert = require('node:assert');
const { parse, compare, isNewer, shouldNotify } = require('../src/version.js');

test('a_newer_patch_is_an_update', () => {
  assert.ok(isNewer('0.5.1', '0.5.2'));
  assert.ok(isNewer('0.5.1', '0.6.0'));
  assert.ok(isNewer('0.5.1', '1.0.0'));
});

test('the_same_version_is_not_an_update', () => {
  assert.ok(!isNewer('0.5.1', '0.5.1'));
});

test('an_older_release_is_never_offered_as_an_update', () => {
  // A rolled-back `latest` must not walk everyone backwards.
  assert.ok(!isNewer('0.6.0', '0.5.9'));
  assert.ok(!isNewer('1.0.0', '0.9.9'));
});

test('versions_compare_numerically_not_as_strings', () => {
  // The bug this exists to prevent: "0.10.0" < "0.9.0" as strings, which would
  // report the fleet up to date for ten releases running.
  assert.ok(isNewer('0.9.0', '0.10.0'));
  assert.ok(isNewer('1.9.0', '1.10.0'));
  assert.ok(!isNewer('0.10.0', '0.9.0'));
});

test('a_leading_v_is_accepted_because_tags_carry_one', () => {
  // The tag is v0.5.1 and the app knows itself as 0.5.1; every comparison
  // crosses that boundary.
  assert.ok(isNewer('0.5.1', 'v0.5.2'));
  assert.ok(isNewer('v0.5.1', '0.5.2'));
  assert.strictEqual(compare('v0.5.1', '0.5.1'), 0);
});

test('an_unparseable_version_is_not_an_update', () => {
  // Strict on purpose: a banner is an interruption, and announcing an update
  // that does not exist is the confident-wrong-answer failure in miniature.
  for (const bad of ['', 'latest', 'nightly', '0.5', '0.5.1.2', null, undefined, 7, {}]) {
    assert.ok(!isNewer('0.5.1', bad), `${String(bad)} must not read as an update`);
    assert.ok(!isNewer(bad, '0.5.1'), `${String(bad)} must not be a comparable current`);
  }
  assert.strictEqual(compare('0.5.1', 'garbage'), null);
});

test('a_prerelease_is_older_than_the_release_it_precedes', () => {
  // SemVer 11: no prerelease outranks any prerelease. Backwards here would
  // offer everyone a downgrade to a release candidate.
  assert.ok(isNewer('0.6.0-rc.1', '0.6.0'));
  assert.ok(!isNewer('0.6.0', '0.6.0-rc.1'));
  assert.strictEqual(compare('0.6.0-rc.1', '0.6.0'), -1);
});

test('prerelease_identifiers_order_by_semver_rules', () => {
  assert.strictEqual(compare('0.6.0-rc.1', '0.6.0-rc.2'), -1);
  assert.strictEqual(compare('0.6.0-rc.2', '0.6.0-rc.10'), -1, 'numeric, not lexical');
  assert.strictEqual(compare('0.6.0-alpha', '0.6.0-beta'), -1);
  // A numeric identifier always ranks below an alphanumeric one.
  assert.strictEqual(compare('0.6.0-1', '0.6.0-alpha'), -1);
  // A longer tail wins when every shared identifier is equal.
  assert.strictEqual(compare('0.6.0-rc', '0.6.0-rc.1'), -1);
});

test('build_metadata_does_not_make_a_release_newer', () => {
  // SemVer says build metadata takes no part in precedence. Treating it as a
  // difference would offer 0.6.0+ci.7 as an update to an identical 0.6.0.
  assert.strictEqual(compare('0.6.0', '0.6.0+ci.7'), 0);
  assert.ok(!isNewer('0.6.0', '0.6.0+ci.7'));
});

test('parse_reads_the_parts_and_rejects_the_rest', () => {
  assert.deepStrictEqual(parse('v1.2.3'), { major: 1, minor: 2, patch: 3, pre: [] });
  assert.deepStrictEqual(parse('1.2.3-rc.4'),
    { major: 1, minor: 2, patch: 3, pre: ['rc', '4'] });
  assert.strictEqual(parse('1.2'), null);
  assert.strictEqual(parse('not a version'), null);
});

test('dismissing_one_version_does_not_silence_the_next', () => {
  // The whole point of recording the dismissed version rather than a boolean.
  assert.ok(!shouldNotify('0.5.1', '0.6.0', '0.6.0'), 'dismissed one stays quiet');
  assert.ok(shouldNotify('0.5.1', '0.6.1', '0.6.0'), 'the next one speaks up');
});

test('nothing_is_notified_when_there_is_no_newer_release', () => {
  assert.ok(!shouldNotify('0.5.1', '0.5.1', null));
  assert.ok(!shouldNotify('0.5.1', '0.4.0', null));
  assert.ok(!shouldNotify('0.5.1', 'garbage', null));
});

test('an_undismissed_update_notifies', () => {
  assert.ok(shouldNotify('0.5.1', '0.6.0', null));
  assert.ok(shouldNotify('0.5.1', '0.6.0', undefined));
  // A dismissal recorded for a version that is no longer offered is simply
  // irrelevant, so nothing has to prune it.
  assert.ok(shouldNotify('0.5.1', '0.6.0', '0.5.9'));
});
