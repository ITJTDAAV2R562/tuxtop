// Deciding whether a release is newer than what is running.
//
// This is here rather than in app.js for the usual reason: it decides a
// *value*, and a value that decides whether to interrupt someone with a banner
// is worth being able to test. The failure that matters is not "we missed an
// update" - it is claiming one exists when it does not, or comparing "0.10.0"
// against "0.9.0" as strings and concluding the fleet is up to date for the
// next ten releases.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxVersion = factory();
}(typeof self !== 'undefined' ? self : this, function () {

  /**
   * Split a version into comparable parts, or null if it is not one.
   *
   * Accepts an optional leading `v`, because a git tag is `v0.5.1` and the
   * number inside the app is `0.5.1`, and every comparison here crosses that
   * boundary at least once.
   *
   * Only the three numeric components and a prerelease tail are read. Build
   * metadata after `+` is explicitly discarded: SemVer says it takes no part
   * in precedence, so `0.6.0+ci.7` and `0.6.0` are the same release and one
   * must not be offered as an update to the other.
   *
   * @param {string} v
   * @returns {{major:number,minor:number,patch:number,pre:string[]}|null}
   */
  function parse(v) {
    if (typeof v !== 'string') return null;
    const m = /^v?(\d+)\.(\d+)\.(\d+)(?:-([0-9A-Za-z.-]+))?(?:\+[0-9A-Za-z.-]+)?$/
      .exec(v.trim());
    if (!m) return null;
    return {
      major: +m[1],
      minor: +m[2],
      patch: +m[3],
      pre: m[4] ? m[4].split('.') : [],
    };
  }

  /**
   * Order two prerelease tails per SemVer §11.
   *
   * The rule that is easy to get wrong: *no* prerelease outranks *any*
   * prerelease, so 0.6.0 is newer than 0.6.0-rc.1. Getting this backwards
   * would offer everyone a downgrade to a release candidate.
   *
   * @param {string[]} a
   * @param {string[]} b
   * @returns {number} -1, 0 or 1
   */
  function cmpPre(a, b) {
    if (!a.length && !b.length) return 0;
    if (!a.length) return 1;
    if (!b.length) return -1;
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      const x = a[i], y = b[i];
      if (x === undefined) return -1;
      if (y === undefined) return 1;
      const nx = /^\d+$/.test(x), ny = /^\d+$/.test(y);
      // Numeric identifiers compare numerically and always rank below
      // alphanumeric ones.
      if (nx && ny) { if (+x !== +y) return +x < +y ? -1 : 1; continue; }
      if (nx !== ny) return nx ? -1 : 1;
      if (x !== y) return x < y ? -1 : 1;
    }
    return 0;
  }

  /**
   * Compare two versions.
   *
   * @param {string} a
   * @param {string} b
   * @returns {number|null} -1 if a < b, 0 if equal, 1 if a > b; null if either
   *   side is unparseable, which the caller must treat as "do not claim
   *   anything" rather than as a comparison result.
   */
  function compare(a, b) {
    const x = parse(a), y = parse(b);
    if (!x || !y) return null;
    for (const k of ['major', 'minor', 'patch']) {
      if (x[k] !== y[k]) return x[k] < y[k] ? -1 : 1;
    }
    return cmpPre(x.pre, y.pre);
  }

  /**
   * Is `candidate` a release worth telling the user about?
   *
   * Deliberately strict: anything it cannot parse is *not* an update. A banner
   * is an interruption, and the cost of staying quiet about a real release is
   * far lower than the cost of announcing one that does not exist - this
   * project's founding bug was a confident wrong number, and "update
   * available" is a claim like any other.
   *
   * @param {string} current version the app is running
   * @param {string} candidate version offered by the endpoint
   * @returns {boolean}
   */
  function isNewer(current, candidate) {
    return compare(current, candidate) === -1;
  }

  /**
   * Should the banner be shown, given what the user has already dismissed?
   *
   * Dismissal is recorded per version, not as a single "don't ask again" flag:
   * dismissing 0.6.0 must not also silence 0.6.1. A stored dismissal for a
   * version that is no longer the latest is simply irrelevant, so nothing has
   * to clean it up.
   *
   * @param {string} current
   * @param {string} candidate
   * @param {string|null} dismissed version the user last dismissed, if any
   * @returns {boolean}
   */
  function shouldNotify(current, candidate, dismissed) {
    if (!isNewer(current, candidate)) return false;
    return compare(dismissed, candidate) !== 0;
  }

  return { parse, compare, isNewer, shouldNotify };
}));
