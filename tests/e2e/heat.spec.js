// The Heat view: the fleet as rows, time as columns.
//
// The rule these guard is ADR-011 - a cell shows its bucket's peak, never its
// mean - and the thing that makes the view worth having: every host gets a
// row, so nothing is aggregated away.

const { test, expect } = require('@playwright/test');

async function openHeat(page) {
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();
  await page.click('#viewHeat');
  await expect(page.locator('.heatwrap')).toBeVisible();
  // Rows draw from history, which the stub answers asynchronously.
  await expect(page.locator('.heatrow').first()).toBeVisible();
}

test('every host gets its own row, none aggregated away', async ({ page }) => {
  const errors = [];
  page.on('pageerror', e => errors.push(String(e)));
  await openHeat(page);

  const configured = await page.evaluate(async () =>
    (await window.__TAURI__.core.invoke('list_hosts')).length);
  // The harness mirrors the real fleet - nineteen hosts, grouped - precisely
  // so a view that quietly summarises some of them fails here.
  expect(configured).toBeGreaterThan(10);
  await expect(page.locator('.heatrow')).toHaveCount(configured);
  expect(errors).toEqual([]);
});

test('rows are grouped, and every group is labelled', async ({ page }) => {
  await openHeat(page);
  const groups = await page.locator('.heatgroup').allTextContents();
  expect(groups.length).toBeGreaterThan(1);
  // A heading per group, and no group heading repeated - the ordering has to
  // actually cluster, not just print a label above each row.
  expect(new Set(groups).size).toBe(groups.length);
});

test('a cell is painted from history, not left blank', async ({ page }) => {
  await openHeat(page);
  // Sample the middle of the first row's canvas and assert something was
  // drawn. A view that renders its scaffold but never paints looks identical
  // to a quiet fleet in a screenshot.
  const painted = await page.evaluate(() => {
    const cv = document.querySelector('.heatrow canvas');
    const ctx = cv.getContext('2d');
    const d = ctx.getImageData(0, 0, cv.width, cv.height).data;
    let opaque = 0;
    for (let i = 3; i < d.length; i += 4) if (d[i] > 0) opaque++;
    return opaque;
  });
  expect(painted).toBeGreaterThan(0);
});

test('the window slider redraws the strip', async ({ page }) => {
  await openHeat(page);
  await expect(page.locator('#histbar')).toBeVisible();
  const before = await page.locator('[data-hist-span]').textContent();
  // Drag the window wide open. The span label is the view stating its own
  // bounds, which is the same contract the log-scaled fleet bars have.
  await page.locator('#histWindow').fill('900');
  await expect(page.locator('[data-hist-span]')).not.toHaveText(before);
});

test('the view says a cell is a peak, not an average', async ({ page }) => {
  await openHeat(page);
  // ADR-011 accepts that a cell is not comparable to a card's number, on the
  // condition that the view says so rather than leaving it to be inferred.
  await expect(page.locator('[data-heat-note]')).toBeVisible();
  await expect(page.locator('[data-heat-note]')).toContainText('peak');
});

test('clicking a row opens that host in History', async ({ page }) => {
  await openHeat(page);
  const host = await page.locator('.heatrow').first().getAttribute('data-host');
  await page.locator('.heatrow').first().locator('canvas').click();
  await expect(page.locator('#viewHist')).toHaveAttribute('aria-pressed', 'true');
  // Landing in History on some other host would be worse than not linking.
  await expect(page.locator('#histSubject')).toHaveValue(host);
});

test('Heat keeps its own metric, and does not offer vector ones', async ({ page }) => {
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();

  // Fleet defaults to a vector metric (CPU cores). Heat has one row per host,
  // so it cannot draw that - and must not silently redraw Fleet's choice.
  await page.click('#viewAll');
  const fleetMetric = await page.locator('#metricSel').inputValue();

  await page.click('#viewHeat');
  const heatOptions = await page.locator('#metricSel option').allTextContents();
  expect(heatOptions).not.toContain('CPU cores');

  await page.click('#viewAll');
  await expect(page.locator('#metricSel')).toHaveValue(fleetMetric);
});
