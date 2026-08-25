// Sample rate: what is offered, and what is chosen by default.
//
// Sub-second sampling multiplies both the traffic and the work the sampler
// asks of a watched host, so it has to be reachable without being the thing
// that happens to someone who never opened Settings.

const { test, expect } = require('@playwright/test');

async function openSettings(page) {
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();
  await page.click('#settingsBtn');
  await expect(page.locator('#setDlg')).toBeVisible();
}

test('4 Hz and 2 Hz are offered, and one second is still the default', async ({ page }) => {
  await openSettings(page);
  const values = await page.locator('#s-interval option').evaluateAll(
    os => os.map(o => o.value));
  expect(values).toContain('250');
  expect(values).toContain('500');
  // The default is what a fleet runs at unless someone decides otherwise.
  await expect(page.locator('#s-interval')).toHaveValue('1000');
});

test('every host can be set faster than the global rate', async ({ page }) => {
  await openSettings(page);
  // The per-host override is the intended way in: you watch the one box you
  // are investigating at 4 Hz, not all nineteen.
  const row = page.locator('[data-perhost-rows] select[data-host-iv]').first();
  const opts = await row.locator('option').evaluateAll(
    os => os.map(o => ({ v: o.value, t: o.textContent.trim() })));
  expect(opts[0].v).toBe('');
  expect(opts.map(o => o.v)).toContain('250');
  // Labelled as a frequency, because that is how a sub-second rate is chosen.
  expect(opts.find(o => o.v === '250').t).toBe('4 Hz');
  expect(opts.find(o => o.v === '1000').t).toBe('1 s');
});

test('the cost meter reprices when the rate changes', async ({ page }) => {
  await openSettings(page);
  const row = () => page.locator('[data-meter-rows] tr.current td').nth(1).textContent();
  await page.selectOption('#s-interval', '1000');
  const atOneHz = await row();
  await page.selectOption('#s-interval', '250');
  const atFourHz = await row();
  // Four times a second costs four times as much, and the panel has to say so
  // before someone picks it for nineteen hosts.
  expect(atFourHz).not.toBe(atOneHz);
});
