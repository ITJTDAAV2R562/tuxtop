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

test('a host already in the fleet can be switched to Windows', async ({ page }) => {
  // The per-host table is the *only* path to `os` for a host that already
  // exists - the Add host dialog covers one that does not - and it has never
  // been exercised, because `set_host_os` was missing from the harness stub
  // entirely. So this control has been throwing in the harness for its whole
  // life while working in the app, which is the third time a gap in the stub
  // has presented as an application bug.
  //
  // It matters more than an OS label sounds: a Windows host created as a Linux
  // one runs a POSIX shell command against cmd.exe and fails with "the system
  // cannot find the path specified", an error that explains nothing.
  await openSettings(page);
  const sel = page.locator('[data-perhost-rows] select[data-host-os]').first();
  await expect(sel).toHaveValue('');

  const errors = [];
  page.on('console', m => { if (m.type() === 'error') errors.push(m.text()); });
  await sel.selectOption('windows');

  // It reached the backend and came back, rather than raising the error bar -
  // which is exactly what a missing backend command produced.
  await expect(sel).toHaveValue('windows');
  await expect(page.locator('#errBar')).toHaveCount(0);
  expect(errors, 'the OS change failed in the console').toEqual([]);

  // And it is what the backend now holds, not just what the select shows.
  const stored = await page.evaluate(() => window.__STUB__.hosts()[0].os);
  expect(stored).toBe('windows');
});
