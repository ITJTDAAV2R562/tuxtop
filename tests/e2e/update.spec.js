// The update notice.
//
// The rule this pins is that nothing installs itself. The check may raise a
// banner and may do nothing else: no download begins, and no version changes,
// until somebody presses the button. A regression here would not look like a
// bug - it would look like the app quietly updating, which is exactly the
// behaviour that was ruled out when this was designed.
//
// The other load-bearing assertion is that a *failed* check is silent in the
// UI. An isolated fleet has no route to GitHub, and interrupting someone on
// every launch to say so would train them to ignore the banner that matters.

const { test, expect } = require('@playwright/test');

/** Load the page with a stubbed answer from `plugin:updater|check`.
 *
 * The stub is installed before any script runs, because app.js fires the check
 * during startup - setting it afterwards would race the thing under test.
 *
 * Deliberately does *not* wait for the fleet to finish arriving. The banner is
 * a statement about the application, not about any host, and it lives outside
 * the grid - so nothing here is rebuilt by a host report and no click can land
 * on a detached node. Waiting anyway is what the first version of this file
 * did, and it cost real time: nine tests each holding a full nineteen-host
 * page while four workers ran, which pushed the whole suite from 2.2m to 4.8m
 * and started timing out the 5s settle predicate in layout.spec. The tests
 * were not slow; the box was saturated, by this file.
 *
 * `loadSettled` is for the one test that touches the toolbar, where the
 * fleet-still-arriving hazard is real.
 */
async function load(page, update) {
  await page.addInitScript(u => { window.__stubUpdate = u; }, update);
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();
}

/** As `load`, but waits until every host has reported.
 *
 * The toolbar resizes as the tally counts up, and Playwright refuses to click
 * an element that is not stable - so a click on #settingsBtn placed too early
 * waits out the full timeout. Same helper the pause and layout specs use, and
 * for the same reason.
 */
async function loadSettled(page, update) {
  await load(page, update);
  await expect
    .poll(async () => {
      const [up, all] = await Promise.all([
        page.locator('#nup').textContent(),
        page.locator('#nhosts').textContent(),
      ]);
      return Number(all) > 1 && up === all;
    }, { timeout: 15_000 })
    .toBe(true);
}

const AVAILABLE = {
  rid: 1, currentVersion: '0.5.1', version: '0.6.0',
  date: null, body: 'Notes', rawJson: {},
};

test('a newer release raises a notice naming both versions', async ({ page }) => {
  await load(page, AVAILABLE);

  const note = page.locator('#updNote');
  await expect(note).toBeVisible();
  // Both numbers, because "an update is available" without saying from what
  // is not enough to decide whether you care.
  await expect(page.locator('#updVersion')).toHaveText('0.6.0');
  await expect(page.locator('#updCurrent')).toHaveText('0.5.1');
});

test('no notice appears when the app is already current', async ({ page }) => {
  // The plugin answers null when there is nothing newer.
  await load(page, null);
  await expect(page.locator('#updNote')).toBeHidden();
});

test('nothing is downloaded until the button is pressed', async ({ page }) => {
  // The whole point of the feature. If this fails, the app is auto-updating.
  await load(page, AVAILABLE);
  await expect(page.locator('#updNote')).toBeVisible();
  expect(await page.evaluate(() => window.__stubInstalled)).toBeUndefined();

  await page.click('#updInstall');
  await expect
    .poll(() => page.evaluate(() => window.__stubInstalled))
    .toBe(true);
});

test('the notice warns that installing closes the app', async ({ page }) => {
  // The app exits partway through a Windows install. Unannounced that reads
  // as a crash, and every ssh session goes with it.
  await load(page, AVAILABLE);
  await page.click('#updInstall');
  await expect(page.locator('#updProgress')).toContainText(/close and reopen/i);
});

test('the release notes button opens the releases page, not the webview', async ({ page }) => {
  // A plain <a href> would navigate the webview away from the app, which is
  // not recoverable without a restart.
  await load(page, AVAILABLE);
  await page.click('#updPage');
  await expect
    .poll(() => page.evaluate(() => window.__stubOpened))
    .toContain('/releases');
  // And the app is still the app.
  await expect(page.locator('#grid')).toBeVisible();
});

test('dismissing hides the notice and it stays hidden on reload', async ({ page }) => {
  await load(page, AVAILABLE);
  await expect(page.locator('#updNote')).toBeVisible();

  await page.click('#updDismiss');
  await expect(page.locator('#updNote')).toBeHidden();

  // Dismissal is remembered per version, so the same release stays quiet.
  await load(page, AVAILABLE);
  await expect(page.locator('#updNote')).toBeHidden();
});

test('dismissing one version does not silence the next', async ({ page }) => {
  // The reason dismissal records a version rather than a boolean. Getting this
  // wrong means one dismissal silences every future release, and the feature
  // stops working for good without ever failing.
  await load(page, AVAILABLE);
  await page.click('#updDismiss');
  await expect(page.locator('#updNote')).toBeHidden();

  await load(page, { ...AVAILABLE, version: '0.6.1' });
  await expect(page.locator('#updNote')).toBeVisible();
  await expect(page.locator('#updVersion')).toHaveText('0.6.1');
});

test('a failed check says nothing in the UI', async ({ page }) => {
  // An isolated fleet cannot reach GitHub, and that is the normal state, not
  // an event. Nagging about it on every launch teaches people to dismiss the
  // banner without reading it.
  await page.addInitScript(() => { window.__stubUpdateThrows = true; });
  await page.addInitScript(() => {
    // Fail the check the way an offline machine does.
    const install = setInterval(() => {
      if (!window.__TAURI__) return;
      clearInterval(install);
      const real = window.__TAURI__.core.invoke;
      window.__TAURI__.core.invoke = (cmd, args) => cmd === 'plugin:updater|check'
        ? Promise.reject(new Error('network unreachable'))
        : real(cmd, args);
    }, 0);
  });
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();

  await expect(page.locator('#updNote')).toBeHidden();
  // And it did not take the page down with it.
  await expect(page.locator('#grid')).toBeVisible();
});

test('the setting reports what the last check found', async ({ page }) => {
  // The check is deliberately quiet, so Settings is the only place a
  // permanently broken one is distinguishable from a fleet that is up to date.
  await loadSettled(page, AVAILABLE);
  await page.click('#settingsBtn');
  await expect(page.locator('#s-update')).toBeChecked();
  await expect(page.locator('#updStatus')).toContainText('0.6.0');
});

test('the release notes button asks for the version being offered', async ({ page }) => {
  // Two bugs in one line, both found on the first real click rather than here:
  // the button opened the releases *index*, and the installer's allowlist was
  // scoped to `releases/*`, which does not match `releases`. So it failed with
  // "Not allowed to open url". Asking for the specific tag is both the more
  // useful page and the one the scope was actually written for.
  await load(page, AVAILABLE);
  await page.click('#updPage');
  await expect
    .poll(() => page.evaluate(() => window.__stubOpened))
    .toBe('https://github.com/ITJTDAAV2R562/tuxtop/releases/tag/v0.6.0');
});

test('the status line names the running version and the real sample rate', async ({ page }) => {
  // It used to read "live · 1 Hz over ssh" as a string literal, whatever the
  // interval was, and never said which build you were running. A footer
  // asserting a rate nobody read is this project's founding bug in miniature.
  await load(page, null);
  const note = page.locator('[data-mode-note]');
  await expect(note).toContainText('0.0.0-test');
  await expect(note).toContainText('1 s over ssh');
});

test('changing the sample rate updates the status line', async ({ page }) => {
  // The half that makes it a reading rather than a claim: it has to stop being
  // true and get fixed, not be written once at startup.
  await loadSettled(page, null);
  await page.click('#settingsBtn');
  await page.selectOption('#s-interval', '250');
  await page.click('#setForm button[value="save"]');
  await expect(page.locator('[data-mode-note]')).toContainText('4 Hz over ssh');
});

test('release notes are reachable without an update being available', async ({ page }) => {
  // The banner's button was the only route to the notes, so it could only be
  // pressed on a build that was already out of date. That is how a wrong URL
  // scope survived a release: on the newest version there was no button to
  // press, so neither the user nor the tests could reach it.
  await loadSettled(page, null);            // nothing newer - no banner at all
  await expect(page.locator('#updNote')).toBeHidden();

  await page.click('#settingsBtn');
  await expect(page.locator('#updVersionLine')).toContainText('0.0.0-test');
  await page.click('#updNotesNow');
  await expect
    .poll(() => page.evaluate(() => window.__stubOpened))
    .toBe('https://github.com/ITJTDAAV2R562/tuxtop/releases/tag/v0.0.0-test');
});
