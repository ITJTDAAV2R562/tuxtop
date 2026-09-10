// The chrome that says whose readings these are, and how old.
//
// ADR-017 rule 1: the mode is visible on screen at all times, with freshness.
// That is the founding hazard in new clothes - in remote mode the numbers were
// taken by a machine you are not on, at an interval it chose, possibly a while
// ago, and a window that looks identical in both modes while showing stale
// remote data *is* the confident wrong number.
//
// **The rule binds the browser too.** A tab served by `tuxtop-serve` is pointed
// at a server by definition: its numbers were taken by a machine it is not on,
// at an interval it did not choose, and it has said nothing about either since
// that shipped. So everything here is driven by data both backends supply -
// endpoint identity, age of the last event, the server's interval - rather than
// by which shell is running it.
//
// Here rather than in app.js for the usual reason: these decide *values*, and
// the frontend went 2,792 lines with zero coverage by putting values in app.js.
(function (root, factory) {
  if (typeof module === 'object' && module.exports) module.exports = factory();
  else root.TuxRemote = factory();
}(typeof self !== 'undefined' ? self : this, function () {

  /** What the titlebar says when this process is doing the sampling. */
  const LOCAL = 'sampling locally';

  /**
   * An endpoint as it should be read on screen: `dove:8787`.
   *
   * The scheme is dropped because only one is possible - the viewer refuses
   * `https://` (ADR-018 decision 2) - so it is four characters that
   * distinguish nothing. The **port is kept**: two servers on one box differ
   * only by it, and a chrome that hid that would name the wrong machine.
   *
   * @param {unknown} endpoint
   * @returns {string|null} null when there is nothing to show
   */
  function originLabel(endpoint) {
    if (typeof endpoint !== 'string') return null;
    const t = endpoint.trim().replace(/^https?:\/\//i, '').replace(/\/+$/, '');
    return t || null;
  }

  /**
   * Whose readings these are.
   *
   * `servedFrom` is the tab's own origin, which only the browser shim knows -
   * a server describes itself as sampling locally, and it is right to, so the
   * tab in front of it has to supply the other half.
   *
   * Returns **null** when there is no answer at all rather than falling back to
   * `LOCAL`: a failed `capabilities` call is not evidence that this window is
   * sampling its own fleet, and saying so would be the one claim this whole
   * module exists to prevent.
   *
   * @param {{endpoint?:string|null}|null|undefined} caps
   * @param {string|null|undefined} servedFrom
   * @returns {string|null}
   */
  function identity(caps, servedFrom) {
    const label = originLabel((caps && caps.endpoint) || servedFrom);
    if (label) return label;
    return caps ? LOCAL : null;
  }

  /**
   * Has the stream gone quiet for longer than the server's own interval allows?
   *
   * `staleAfterMs` comes from `capabilities`, computed by
   * `tuxtop_core::remote::stale_after_ms`, so the rule lives in one place and
   * this carries no second copy of it to drift from.
   *
   * An absent or nonsensical threshold answers **false**, and that is
   * deliberate rather than optimistic: the only way it can be absent is a
   * server too old to send it, which is a version difference, and
   * `capabilities.version_note` states that in words instead of leaving this
   * function to invent a threshold and judge against it.
   *
   * @param {number} ageMs
   * @param {number} staleAfterMs
   * @returns {boolean}
   */
  function isStale(ageMs, staleAfterMs) {
    if (!Number.isFinite(ageMs) || !Number.isFinite(staleAfterMs)) return false;
    if (staleAfterMs <= 0) return false;
    return ageMs > staleAfterMs;
  }

  /**
   * Local wall-clock `HH:MM:SS` for an epoch-milliseconds instant.
   *
   * A clock time rather than "12 s ago", because the number people need is the
   * one they can compare against when they last looked at the machine.
   *
   * @param {unknown} ms
   * @returns {string|null}
   */
  function clockTime(ms) {
    if (!Number.isFinite(ms)) return null;
    const d = new Date(ms);
    const p = n => String(n).padStart(2, '0');
    return `${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  }

  /**
   * What to say when the server has gone quiet.
   *
   * **It names the endpoint, not the hosts.** Losing one server takes out all
   * nineteen cards at once, which is a failure local mode has never had.
   * Nineteen cards each captioned "offline" reads as a dead fleet rather than a
   * dead link - the generic-offline failure the hard rules already forbid - so
   * the grid keeps its last readings, marked stale, and the link says it is the
   * link.
   *
   * With no `lastSeen` at all the time is left off rather than invented: that
   * is a connection that never succeeded, and "since 00:00:00" would be a
   * fabricated fact about a machine nobody reached.
   *
   * @param {string|null} label from `originLabel`
   * @param {number|null|undefined} lastSeenMs
   * @returns {string|null} null when there is nothing to warn about
   */
  function staleNote(label, lastSeenMs) {
    if (!label) return null;
    const t = clockTime(lastSeenMs);
    return t ? `no contact with ${label} since ${t}` : `no contact with ${label}`;
  }

  /**
   * Which halves of the Settings dialog can actually be saved.
   *
   * Two questions, not one, because `Settings` is two halves that answer to
   * different machines (ADR-018 decision 4):
   *
   * - **fleet** (`interval_ms`, `history_cap_mb`) describes the machine doing
   *   the sampling. A read-only server refuses it, and so does a viewer in
   *   remote mode, where the fleet on screen is not the one this file
   *   configures.
   * - **viewer** (`always_on_top`, `update_check`) is a property of *this
   *   window*, and saves in remote mode - a viewer that could not be pinned
   *   because pinning is a "setting" would be absurd. What it needs is a local
   *   service to save it, which a browser tab does not have: a tab has no
   *   window to keep on top, and the update check belongs to an app, not to a
   *   `tuxtop-serve` that ships no updater.
   *
   * Both were live on a read-only server before this, and Save returned 403 -
   * three controls that could only fail, which is exactly what `capabilities`
   * exists to prevent.
   *
   * @param {{writable?:boolean}|null|undefined} caps
   * @param {string|null|undefined} servedFrom the tab's own origin, if a tab
   * @returns {{fleet:boolean, viewer:boolean}}
   */
  function editable(caps, servedFrom) {
    return {
      fleet: !!(caps && caps.writable),
      viewer: !servedFrom,
    };
  }

  /**
   * The status line under the grid.
   *
   * `over ssh` is a claim about *this* machine's connections, and in remote
   * mode this machine has none - the ssh belongs to the server. So the line
   * names the server instead. Local mode is unchanged to the character, which
   * is what keeps `update.spec.js` honest about the interval it quotes.
   *
   * @param {{version?:string|null, rate?:string, overridden?:number,
   *          label?:string|null}} o
   * @returns {string}
   */
  function modeLine(o) {
    const { version, rate, overridden, label } = o || {};
    const via = label ? `via ${label}` : 'over ssh';
    const n = Number(overridden) || 0;
    return (version ? `Tuxtop ${version} · ` : '')
      + `live · ${rate || '?'} ${via}`
      + (n ? ` · ${n} host${n === 1 ? '' : 's'} at its own rate` : '');
  }

  return { LOCAL, originLabel, identity, isStale, clockTime, staleNote, editable, modeLine };
}));
