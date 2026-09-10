// Saved servers: several fleets - or several customers - as one selection.
//
// The reason this file exists rather than three more tests in remote.spec.js is
// the rule it is built around: a new field needs a control on **both** paths,
// and the second one is the one that gets forgotten. Host `os` shipped with a
// backend, a hosts.toml entry and a documented example, reachable only from the
// Add host dialog - so it worked for a host that did not exist yet and for no
// other. A test that adds an entity and then edits it passes against exactly
// that bug, because the Add path is the one that works.
//
// So every edit here is made to an endpoint that was already in the harness's
// list when the page loaded.

const { test, expect } = require('@playwright/test');

/// The page, once the whole fleet has stopped arriving.
///
/// The helper `layout.spec`, `pause.spec` and `remote.spec` all use, for the
/// reason written up in the first of them: cards are created as hosts report,
/// so for the first second the grid is being torn down and rebuilt several
/// times a second and a click lands on a detached node.
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

/// Settings, with the saved-server list disclosed.
///
/// No `evaluate` to check whether the `<details>` is open: it is `open` in the
/// markup, like the per-host table beside it. The conditional version timed out
/// under the parallel suite while the locator had plainly resolved — the shape
/// CLAUDE.md says to simplify rather than investigate, and here the simpler
/// test and the more discoverable control are the same change.
async function openSaved(page) {
  await page.locator('#settingsBtn').click();
  await expect(page.locator('#setDlg')).toBeVisible();
  await expect(page.locator('[data-endpoint-rows] tr').first()).toBeVisible();
}

const row = (page, name) =>
  page.locator(`[data-endpoint-rows] tr:has([data-ep-name="${name}"])`);

test('an endpoint that already existed can be renamed and repointed', async ({ page }) => {
  // `lab` comes from the harness's starting list, not from this test. That is
  // the whole assertion: the entity that was already there when the page
  // loaded is the one with no control in every version of this bug.
  await load(page);
  await openSaved(page);
  await expect(page.locator('[data-ep-url="lab"]')).toHaveValue('coot:9000');

  // One field at a time, each awaited: every commit redraws the table from the
  // backend's answer, so a second fill issued before that lands on a detached
  // input. The backend takes both fields together - `update_endpoint` is one
  // operation so a server that moves and is renamed never passes through a
  // state on disk that is neither - and this is the browser's half of it.
  //
  // **Tab, on the input that was just filled.** `fill` fires `input` and not
  // `change` - measured, by removing this and watching the test fail every
  // time - so something has to move focus, and that blur is the commit.
  //
  // Two tidier-looking versions of that hung under the parallel suite, each on
  // a *resolved* locator: a `page.evaluate` reading the `<details>` state, and
  // a `focus()` on the add-row box. Both are the shape CLAUDE.md says to
  // simplify rather than investigate. A keystroke needs no second element -
  // the locator re-resolves at the moment of the press, and until something
  // commits, `[data-ep-name="lab"]` is still exactly what it was.
  await page.locator('[data-ep-name="lab"]').fill('lab fleet');
  await page.locator('[data-ep-name="lab"]').press('Tab');
  await expect(page.locator('[data-ep-name="lab fleet"]')).toHaveValue('lab fleet');
  await expect(page.locator('[data-ep-name="lab"]')).toHaveCount(0);

  await page.locator('[data-ep-url="lab fleet"]').fill('coot:9100');
  await page.locator('[data-ep-url="lab fleet"]').press('Tab');
  await expect(page.locator('[data-ep-url="lab fleet"]')).toHaveValue('coot:9100');

  // The half that matters: close the dialog and ask again. A test that edits
  // the DOM and reads the DOM back passes against a backend that did nothing.
  await page.keyboard.press('Escape');
  await expect(page.locator('#setDlg')).toBeHidden();
  await openSaved(page);
  await expect(row(page, 'lab fleet')).toBeVisible();
  await expect(page.locator('[data-ep-url="lab fleet"]')).toHaveValue('coot:9100');
  await expect(row(page, 'a customer'), 'the edit reached the wrong row')
    .toBeVisible();
});

test('a saved server switches the fleet the same way typing one does', async ({ page }) => {
  // Selecting goes through the same `use_endpoint` as typing: a second path
  // would be a second teardown to forget, which is ADR-012's lesson wearing
  // different clothes. What that buys is asserted here as sameness - the
  // observable result is the one `typing a server address switches the fleet
  // without a restart` already pins for the typed path.
  await load(page);
  await expect(page.locator('.card[data-name="coot"]')).toBeVisible();
  await openSaved(page);

  await row(page, 'a customer').locator('[data-ep-use]').click();

  // The row for the server now on screen is marked, which is the one thing the
  // list can say that the address box cannot.
  await expect(row(page, 'a customer')).toHaveClass(/current/);
  await expect(row(page, 'lab'), 'every row claims to be the one being watched')
    .not.toHaveClass(/current/);

  // The word belongs to this table and to no other. It was a `::after` on
  // `.meter-table` first - a class three tables wear - and it labelled the
  // sample-interval meter "2 s - watching", a confident sentence about the
  // wrong thing, which is this project's founding failure in a stylesheet.
  //
  // The first version of *this* assertion passed against that bug, because
  // generated content is not in `textContent` and there was nothing to read.
  // It is real text in the row now, which is what makes the next two lines
  // mean anything.
  await expect(row(page, 'a customer')).toContainText('watching');
  await expect(page.locator('[data-meter-rows] tr.current'),
    'the sample-interval meter says which server is being watched')
    .not.toContainText('watching');

  await page.keyboard.press('Escape');
  await expect(page.locator('#remotebar')).toBeVisible();
  await expect(page.locator('#remotebar [data-chrome-who]')).toHaveText('dove:8787');
  await expect(page.locator('.card[data-name="c2-coot"]')).toBeVisible();
  await expect(page.locator('.card[data-name="coot"]'),
    'the fleet we left is still on screen').toHaveCount(0);
});

test('forgetting a saved server does not leave the fleet it names', async ({ page }) => {
  // Tidying a list is not a request to switch. A window that went back to its
  // own fleet because somebody deleted the note it was reading would be doing
  // something nobody asked for, and it would read as a crash.
  //
  // Started from an endpoint that was already set when the page loaded, so the
  // state under test is not one this test assembled.
  await page.addInitScript(() => { window.__stubEndpoint = 'http://dove:8787'; });
  await load(page);
  await expect(page.locator('#remotebar [data-chrome-who]')).toHaveText('dove:8787');
  await openSaved(page);
  await expect(row(page, 'a customer')).toHaveClass(/current/);

  await row(page, 'a customer').locator('[data-ep-drop]').click();
  await expect(row(page, 'a customer')).toHaveCount(0);
  await expect(row(page, 'lab'), 'forgetting one forgot the others').toBeVisible();

  await page.keyboard.press('Escape');
  await expect(page.locator('#remotebar')).toBeVisible();
  await expect(page.locator('#remotebar [data-chrome-who]')).toHaveText('dove:8787');
  await expect(page.locator('.card[data-name="c2-coot"]')).toBeVisible();
});

test('a browser tab is offered no saved server it could not point anywhere', async ({ page }) => {
  // The tab has no local hosts.toml, so what it would be editing is the
  // server's own list - that machine's notes about where *it* can point,
  // reaching nothing this tab can select. Disabled rather than hidden, for the
  // reason the interval field is: the values are worth reading.
  await page.addInitScript(() => { window.__TUXTOP_ENDPOINT__ = 'http://lutik:8787'; });
  await load(page);
  await openSaved(page);

  for (const sel of ['#ep-name', '#ep-url', '#ep-add', '[data-ep-name="lab"]',
                     '[data-ep-url="lab"]', '[data-ep-use="lab"]',
                     '[data-ep-drop="lab"]']) {
    await expect(page.locator(sel), `${sel} can only fail in a tab`).toBeDisabled();
  }
  // And the address box above it, which is the same half of the settings.
  await expect(page.locator('#s-server')).toBeDisabled();
});
