// The chrome that says whose readings these are, and how old.
//
// ADR-017 rule 1: the mode is visible on screen at all times, with freshness,
// in the chrome rather than in Settings. The failure being designed against is
// not a crash - it is a window that looks identical in both modes while showing
// another machine's readings from four minutes ago, which is this project's
// founding bug arriving over a new transport.
//
// Everything here runs against the harness's nineteen hosts, because the layout
// half is only interesting at that size: the toolbar was within ~70px of
// clipping "Add host" at nineteen, which is why the strip is its own row rather
// than a corner of it.

const { test, expect } = require('@playwright/test');

/// The page, once the whole fleet has stopped arriving.
///
/// Cards are created as hosts report and every arrival rebuilds the grid, so
/// for the first second the toolbar is resizing and elements under the pointer
/// are being replaced several times a second. Waiting for `nup` to reach
/// `nhosts` waits for the last host - the helper `layout.spec` and `pause.spec`
/// both use, for the reason written up there.
async function load(page) {
  await page.goto('/index.html');
  await expect
    .poll(async () => {
      const [up, all] = await Promise.all([
        page.locator('#nup').textContent(),
        page.locator('#nhosts').textContent(),
      ]);
      return Number(all) > 1 && up === all;
    }, { message: 'every host reporting, so the grid stops rebuilding' })
    .toBe(true);
  return page;
}

/// Make the harness answer as a viewer pointed at a server.
///
/// Set before load rather than evaluated into a running page: the stub reads
/// these at startup, and a `page.evaluate` right after `goto` lands in the
/// window where a navigation destroys the execution context.
async function asRemoteViewer(page, opts = {}) {
  await page.addInitScript(o => {
    window.__stubEndpoint = 'http://dove:8787';
    if (o.staleAfterMs !== undefined) window.__stubStaleAfterMs = o.staleAfterMs;
    if (o.quietAfterMs !== undefined) window.__stubQuietAfterMs = o.quietAfterMs;
    if (o.versionNote !== undefined) window.__stubVersionNote = o.versionNote;
  }, opts);
  return load(page);
}

test('sampling locally draws no strip at all', async ({ page }) => {
  // The control, and the reason the strip is worth having: local mode is
  // unchanged, so the strip *appearing* is itself the first signal that the
  // numbers came from somewhere else. A strip that read "sampling locally" on
  // every desktop launch would be a line people stop seeing by the second day.
  await load(page);
  await expect(page.locator('#remotebar')).toBeHidden();
  // And nothing claims a stale link when there is no link.
  expect(await page.evaluate(() => document.body.dataset.stale)).toBeUndefined();
  // The status line still says what it always said, to the character.
  await expect(page.locator('[data-mode-note]')).toContainText('over ssh');
});

test('a remote viewer says which machine took its readings', async ({ page }) => {
  await asRemoteViewer(page);
  const bar = page.locator('#remotebar');
  await expect(bar).toBeVisible();
  // The scheme is dropped - only http is possible - and the port is kept,
  // because two servers on one box differ only by it.
  await expect(bar.locator('[data-chrome-who]')).toHaveText('dove:8787');
  // `over ssh` is a claim about *this* machine's connections, and in remote
  // mode this machine has none.
  await expect(page.locator('[data-mode-note]')).toContainText('via dove:8787');
  await expect(page.locator('[data-mode-note]')).not.toContainText('over ssh');
});

test('losing the server does not blank the grid', async ({ page }) => {
  // The one failure local mode has never had: a single dead link takes out all
  // nineteen cards at once. Nineteen cards each saying "offline" reads as a
  // dead fleet rather than a dead link, so the grid keeps its last readings
  // and the strip says it is the link.
  await asRemoteViewer(page, { staleAfterMs: 1200, quietAfterMs: 1500 });

  const cards = page.locator('.card');
  const before = await cards.count();
  expect(before).toBeGreaterThan(1);
  // Real numbers on screen now, so "still there" below is a claim about
  // something rather than about an empty grid.
  await expect(cards.first().locator('[data-cpu]')).not.toHaveText('—');

  const bar = page.locator('#remotebar');
  await expect(bar.locator('[data-chrome-age]'))
    .toContainText('no readings from dove:8787 since', { timeout: 15_000 });
  // The time is a wall clock, which is what someone can compare against when
  // they last looked at the machine.
  await expect(bar.locator('[data-chrome-age]')).toContainText(/\d\d:\d\d:\d\d/);

  // The grid is intact, with its readings, and no card claims a fault.
  expect(await cards.count()).toBe(before);
  // Still a number, not the em dash a blanked card shows.
  await expect(cards.first().locator('[data-cpu]')).not.toHaveText('—');
  // And no card claims a fault of its own: the link died, not the host.
  await expect(page.locator('.card [data-fault]:not([hidden])')).toHaveCount(0);
  // And the plain label is not shown beside the warning: the warning already
  // names the endpoint, and saying it twice costs the one line that matters.
  await expect(bar.locator('[data-chrome-who]')).toBeHidden();
});

test('the stale strip is legible in both themes', async ({ page }) => {
  // A colour defined in only some of the three theme states renders one
  // theme's colour on another theme's ground, and fails in one direction only.
  // This has landed twice in this repo, which is why it is asserted rather
  // than eyeballed - and why the *fresh* state is checked too: a warn ground
  // that were identical to the calm one would pass a test that only looked at
  // one of them.
  await asRemoteViewer(page, { staleAfterMs: 1200, quietAfterMs: 1500 });
  const bar = page.locator('#remotebar');
  const mark = bar.locator('.rb-mark');

  const ground = () => bar.evaluate(el => getComputedStyle(el).backgroundColor);
  const dot = () => mark.evaluate(el => getComputedStyle(el).backgroundColor);

  /// A colour that will actually paint something.
  ///
  /// Not a regex on `rgb(`: `color-mix()` computes to `color(srgb 0.7 0.4 0.05
  /// / 0.14)` in Chromium, so a test matching only the rgb spelling passes
  /// against a transparent `color()` and fails against a perfectly good one.
  /// What matters is that it is a colour and its alpha is not zero.
  const paints = (c, why) => {
    expect(c, `${why}: no colour at all`).toMatch(/^(rgba?|color)\(/);
    expect(c, `${why}: fully transparent`).not.toMatch(/[/,]\s*0\s*\)$/);
  };

  const calm = {};
  for (const theme of ['light', 'dark']) {
    await page.evaluate(t => document.documentElement.dataset.theme = t, theme);
    calm[theme] = { ground: await ground(), dot: await dot() };
    paints(calm[theme].dot, `${theme}: the fresh mark`);
  }

  await expect(bar.locator('[data-chrome-age]'))
    .toBeVisible({ timeout: 15_000 });

  const alarmed = {};
  for (const theme of ['light', 'dark']) {
    await page.evaluate(t => document.documentElement.dataset.theme = t, theme);
    const g = await ground();
    const d = await dot();
    alarmed[theme] = { ground: g, dot: d };
    paints(g, `${theme}: the stale strip's ground`);
    paints(d, `${theme}: the stale mark`);
    // The whole point of the state: it has to look different from calm, in
    // this theme, not merely be defined somewhere.
    expect(g, `${theme}: stale looks exactly like fresh`).not.toBe(calm[theme].ground);
    expect(d, `${theme}: the stale mark looks exactly like the fresh one`)
      .not.toBe(calm[theme].dot);
  }

  // And the two themes are not the same colour, which is what a hardcoded
  // value produces - or a token defined only under `prefers-color-scheme` and
  // not `[data-theme]`.
  //
  // **Both states, not just the calm one.** Checking only `calm` left this
  // green against a stale ground written as `rgba(180,105,14,.14)`, because
  // the calm ground beside it was still a token and still flipped. The state
  // that a hardcoded colour is most tempting in is the alarming one.
  expect(calm.light.ground, 'the fresh strip renders one ground in both themes')
    .not.toBe(calm.dark.ground);
  expect(alarmed.light.ground, 'the stale strip renders one ground in both themes')
    .not.toBe(alarmed.dark.ground);
  expect(alarmed.light.dot, 'the stale mark renders one colour in both themes')
    .not.toBe(alarmed.dark.dot);
});

test('a build difference is stated rather than guessed', async ({ page }) => {
  // A viewer one release ahead of its server reads a renamed field as absent
  // and draws a plausible wrong number. The honest response is to say the two
  // disagree - not to work out from a version string whether it matters.
  await asRemoteViewer(page, {
    versionNote: 'this window is 0.8.0; its readings come from 0.7.0',
  });
  const note = page.locator('#remotebar [data-chrome-note]');
  await expect(note).toBeVisible();
  await expect(note).toContainText('0.8.0');
  await expect(note).toContainText('0.7.0');
  // Stated, not judged: nothing here decides which build is right.
  await expect(note).not.toContainText('newer');
  await expect(note).not.toContainText('older');
});

/// The toolbar's shape: how tall it is, and whether anything is pushed out.
const toolbar = page => page.evaluate(() => {
  const bar = document.querySelector('.toolbar');
  const box = bar.getBoundingClientRect();
  const pad = parseFloat(getComputedStyle(bar).paddingRight);
  // Only what is actually drawn: a read-only backend hides "Add host", and a
  // hidden element's zero-width rect is inside every box, which would make the
  // check below quietly vacuous.
  const drawn = [...bar.children].filter(el => el.getBoundingClientRect().width > 0);
  const last = drawn[drawn.length - 1].getBoundingClientRect();
  return {
    height: Math.round(box.height),
    // The signal `nowrap` produces at this width and `wrap` does not.
    overflows: bar.scrollWidth > bar.clientWidth + 1,
    // The last control in the row is the one that goes first.
    lastInside: last.right <= box.right - pad + 1,
    drawn: drawn.length,
  };
});

/// A width where wrapping and clipping give *different* answers.
///
/// Measured twice, because the first number was wrong in a way that reads as
/// fine. At Playwright's default 1280 the toolbar fits on one row whether it
/// wraps or not, so an assertion made there passes against
/// `flex-wrap:nowrap` and tests nothing - the same shape as the core-column
/// test that once ran at a width where "snap to eight" and "as many as fit"
/// both answered 8. 1100 fixed that for the *writable* toolbar and was still
/// vacuous here, because the baseline below is read-only and so has no "Add
/// host" taking up ~100px.
///
/// At 1000, with nineteen hosts and a read-only backend: wrapping gives two
/// rows, 97px, nothing outside the box and all seven controls drawn.
/// `nowrap` gives one row, 56px, `scrollWidth` past `clientWidth`, the last
/// control outside the right edge and only six of the seven drawn at all.
const NARROW = { width: 1000, height: 720 };

test('the strip costs the toolbar nothing at nineteen hosts', async ({ page }) => {
  // The layout half of the decision: the endpoint gets a row of its own rather
  // than a corner of a toolbar that was already within ~70px of clipping "Add
  // host" at nineteen hosts. So what is asserted is that the row below did not
  // move, at a width where it would have.
  await page.setViewportSize(NARROW);

  // The baseline is a **read-only local** server, not a writable one: remote
  // mode also hides "Add host", so comparing against the writable local
  // toolbar measures that instead and reports 56px against 97px for reasons
  // that have nothing to do with the strip. Read-only local and remote draw
  // the same controls, and differ only by the strip.
  await page.addInitScript(() => { window.__stubReadonly = true; });
  await load(page);
  const n = Number(await page.locator('#nhosts').textContent());
  expect(n, 'the harness must mirror the real fleet, not five convenient hosts')
    .toBeGreaterThanOrEqual(19);
  await expect(page.locator('#remotebar')).toBeHidden();
  const without = await toolbar(page);
  expect(without.overflows, 'the toolbar overflows before the strip exists')
    .toBe(false);
  expect(without.lastInside, 'a control is already outside the toolbar').toBe(true);

  await asRemoteViewer(page);
  expect(Number(await page.locator('#nhosts').textContent())).toBe(n);
  await expect(page.locator('#remotebar')).toBeVisible();
  const withStrip = await toolbar(page);
  expect(withStrip.drawn, 'the two toolbars are not comparable')
    .toBe(without.drawn);
  expect(withStrip.overflows, 'the strip pushed a toolbar control out of the row')
    .toBe(false);
  expect(withStrip.lastInside, 'a toolbar control left the row').toBe(true);
  expect(withStrip.height, 'the strip cost the toolbar a row')
    .toBe(without.height);

  // And it is above the toolbar rather than inside it, which is the half of
  // the decision the heights alone cannot distinguish.
  const above = await page.evaluate(() => {
    const bar = document.querySelector('#remotebar').getBoundingClientRect();
    const tb = document.querySelector('.toolbar').getBoundingClientRect();
    return bar.height > 0 && bar.bottom <= tb.top + 1;
  });
  expect(above, 'the strip is not on a row of its own above the toolbar').toBe(true);
});

test('a read-only server offers no setting it would refuse', async ({ page }) => {
  // True before remote mode existed, and never tested: `data-readonly` hid Add
  // host, Remove, Pause and the drag grip, and left the interval, the history
  // limit and the update check live - three controls whose Save returned 403.
  await page.addInitScript(() => { window.__stubReadonly = true; });
  await load(page);
  await page.locator('#settingsBtn').click();
  await expect(page.locator('#setDlg')).toBeVisible();

  for (const id of ['#s-interval', '#s-cap']) {
    await expect(page.locator(id), `${id} can only fail on a read-only server`)
      .toBeDisabled();
  }
  // Disabled with a reason: a greyed-out control and no explanation is its own
  // small puzzle, and the value is worth reading on a fleet you cannot change.
  const why = page.locator('#setWhy');
  await expect(why).toBeVisible();
  await expect(why).toContainText('read-only');
});

test('a remote viewer can still be pinned', async ({ page }) => {
  // The exception that makes the Settings split load-bearing: always_on_top is
  // a property of *this* window, so a viewer that could not be pinned because
  // pinning is a "setting" and settings belong to the server would be absurd.
  await asRemoteViewer(page);
  await page.locator('#settingsBtn').click();
  await expect(page.locator('#setDlg')).toBeVisible();

  await expect(page.locator('#s-interval'), 'the interval is the server\'s')
    .toBeDisabled();
  await expect(page.locator('#s-cap')).toBeDisabled();
  await expect(page.locator('#s-ontop'), 'pinning is this window\'s own')
    .toBeEnabled();
  await expect(page.locator('#s-update')).toBeEnabled();
  await expect(page.locator('#setWhy')).toContainText('dove:8787');
});
