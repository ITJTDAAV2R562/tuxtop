// Every filesystem on a host, not only the fullest.
//
// The card shows one number, which is the right single number - one full disk
// is the problem, not the mean fullness of five. But it hides a /boot at 60%
// behind a / at 61%, and on the real fleet coot carries thirteen filesystems.

const { test, expect } = require('@playwright/test');

async function openDiskUsage(page) {
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();
  await page.click('#viewAll');
  await page.selectOption('#metricSel', 'fs');
  await expect(page.locator('.fbar').first()).toBeVisible();
  // Hosts live inside groups, which start collapsed.
  for (const g of await page.locator('.fbar.fgroup .ftoggle').all()) await g.click();
  await expect(page.locator('.fbar[data-name]:not(.part)').first()).toBeVisible();
}

test('a host with one filesystem offers nothing to expand', async ({ page }) => {
  await openDiskUsage(page);
  // A disclosure that opens a list of one is a control that does nothing.
  const single = page.locator('.fbar[data-name="dove"]:not(.part)');
  await expect(single).toBeVisible();
  await expect(single.locator('.ftoggle')).toHaveCount(0);
  await expect(single.locator('.dot')).toHaveCount(1);
});

test('a host with several expands to show every one, fullest first', async ({ page }) => {
  await openDiskUsage(page);
  const coot = page.locator('.fbar[data-name="coot"]:not(.part)');
  await expect(coot.locator('.ftoggle')).toHaveCount(1);
  await coot.locator('.ftoggle').click();

  const parts = page.locator('.fbar.part[data-name="coot"]');
  await expect(parts).toHaveCount(7);

  const pcts = await parts.locator('[data-val]').allTextContents();
  const nums = pcts.map(t => parseFloat(t));
  const sorted = [...nums].sort((a, b) => b - a);
  expect(nums).toEqual(sorted, 'fullest first: the question is what is nearly full');

  // The host's own row keeps showing the fullest, unchanged - everything else
  // in the app reads that scalar.
  expect(parseFloat(await coot.locator('[data-val]').textContent()))
    .toBeCloseTo(nums[0], 0);
});

test('an expanded host still reports its own state, not its mounts', async ({ page }) => {
  await openDiskUsage(page);
  const coot = page.locator('.fbar[data-name="coot"]:not(.part)');
  await coot.locator('.ftoggle').click();
  // No status dot per mount: a filesystem cannot be unreachable on its own,
  // and repeating the host's dot on every row would imply it can.
  await expect(page.locator('.fbar.part[data-name="coot"] .dot')).toHaveCount(0);
  // Swapping the dot for a disclosure must not lose the host's state.
  await expect(coot).not.toHaveClass(/down/);
});

test('collapsing puts the rows away again', async ({ page }) => {
  await openDiskUsage(page);
  const coot = page.locator('.fbar[data-name="coot"]:not(.part)');
  await coot.locator('.ftoggle').click();
  await expect(page.locator('.fbar.part[data-name="coot"]')).toHaveCount(7);
  await page.locator('.fbar[data-name="coot"]:not(.part) .ftoggle').click();
  await expect(page.locator('.fbar.part[data-name="coot"]')).toHaveCount(0);
});
