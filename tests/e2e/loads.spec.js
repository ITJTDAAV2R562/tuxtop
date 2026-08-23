// The page comes up, and stays up.
//
// The bug that motivated this file: `fsPct` was a `const` arrow declared after
// the metric registry that closes over it, so `availableMetrics()` hit it in
// the temporal dead zone during startup and threw at module scope. The whole
// UI came up inert with one console line. Nothing but a browser can catch that.

const { test, expect } = require('@playwright/test');

/** Fail on any console error or uncaught exception, in every test. */
function watchForErrors(page) {
  const errors = [];
  page.on('console', m => {
    if (m.type() !== 'error') return;
    // The harness is served by python's http.server with no favicon.
    if (m.text().includes('favicon')) return;
    errors.push(m.text());
  });
  page.on('pageerror', e => errors.push(String(e)));
  return errors;
}

test('the page loads without a single console error', async ({ page }) => {
  const errors = watchForErrors(page);
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();
  expect(errors).toEqual([]);
});

test('every view renders without error', async ({ page }) => {
  const errors = watchForErrors(page);
  await page.goto('/index.html');

  for (const [id, marker] of [
    ['#viewAll', '.grid.all-mode'],
    // History renders line charts, or per-core small multiples when the
    // subject is a vector metric. Either is a rendered History view.
    ['#viewHist', '.chart, .cores-hist'],
    ['#viewProcs', '.proctable'],
    ['#viewHosts', '.card'],
  ]) {
    await page.click(id);
    await expect(page.locator(marker).first()).toBeVisible({ timeout: 15_000 });
  }
  expect(errors).toEqual([]);
});

test('the fleet is the real one, not a convenient toy', async ({ page }) => {
  // Two bugs reached the user because the harness had five hosts and the
  // fleet has nineteen — a tally 70px narrower, which was the exact margin
  // the toolbar had.
  await page.goto('/index.html');
  await expect(page.locator('.card')).toHaveCount(19);
  await expect(page.locator('.tally')).toContainText('19 hosts');
});
