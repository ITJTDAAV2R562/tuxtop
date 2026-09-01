// Pausing a host for planned maintenance.
//
// The alternative people reach for is removing the host and adding it back
// afterwards, which throws away its history, its group, its interval override
// and its position in the grid. Pause keeps all of that and stops only the
// sampling - so what these tests actually pin is that the card stops
// *claiming* anything while a machine is deliberately down.
//
// The load-bearing assertion is that the numbers blank rather than freeze. A
// paused card still showing 42% is a confident wrong number about a box that
// is powered off, which is the failure this whole application is a reaction
// to. A test that only checked for a "Paused" caption would pass with the
// stale figures sitting right beside it.

const { test, expect } = require('@playwright/test');

/** The first card, once it has actually reported something. */
async function firstCard(page) {
  await page.goto('/index.html');
  const card = page.locator('.card').first();
  await expect(card).toBeVisible();
  // Wait for a real sample, so "blank" is a change and not the initial state.
  await expect(card.locator('[data-cpu]')).not.toHaveText('—');
  return card;
}

test('a paused card blanks its readings instead of freezing them', async ({ page }) => {
  const card = await firstCard(page);

  await card.locator('[data-pause]').click();

  await expect(card).toHaveClass(/paused/);
  await expect(card.locator('[data-fault-title]')).toHaveText('Paused');
  // Every figure, not just the headline one: a single stale number beside a
  // card that says it is not reporting is the whole bug.
  await expect(card.locator('[data-cpu]')).toHaveText('—');
  await expect(card.locator('[data-ram]')).toHaveText('—');
  await expect(card.locator('[data-net]')).toHaveText('—');
  await expect(card.locator('[data-dio]')).toHaveText('—');
  // Not red. A host somebody took down on purpose is not an incident.
  await expect(card.locator('.dot')).toHaveClass(/paused/);
  await expect(card.locator('.dot')).not.toHaveClass(/warnstate/);
});

test('the readings come back when the host is resumed', async ({ page }) => {
  const card = await firstCard(page);
  await card.locator('[data-pause]').click();
  await expect(card.locator('[data-cpu]')).toHaveText('—');

  // The resume control has to be reachable without a hover: it is the only
  // way out of the state.
  await expect(card.locator('[data-pause]')).toBeVisible();
  await card.locator('[data-pause]').click();

  await expect(card).not.toHaveClass(/paused/);
  await expect(card.locator('[data-cpu]')).not.toHaveText('—');
});

test('a paused host is counted apart from the ones that are up', async ({ page }) => {
  const card = await firstCard(page);
  await expect(page.locator('#npausedWrap')).toBeHidden();

  // Wait for the whole fleet to be reporting, not just the first card. Read
  // too early, "up" is still climbing and the arithmetic below compares
  // against a number that was never the answer.
  await expect.poll(async () => {
    const [up, all] = await Promise.all([
      page.locator('#nup').textContent(),
      page.locator('#nhosts').textContent(),
    ]);
    return Number(all) > 1 && up === all;
  }, { message: 'every host reporting' }).toBe(true);
  const total = Number(await page.locator('#nhosts').textContent());

  await card.locator('[data-pause]').click();

  // Never folded into "up": pausing a dying box must not make the fleet
  // report itself healthier than it was a moment ago.
  await expect(page.locator('#npaused')).toHaveText('1');
  await expect(page.locator('#npausedWrap')).toBeVisible();
  await expect(page.locator('#nup')).toHaveText(String(total - 1));
  // And it is still a host. Pause is not removal.
  await expect(page.locator('#nhosts')).toHaveText(String(total));
});

test('pausing survives being driven from Settings, and the two agree', async ({ page }) => {
  // The second click path, and the one that matters for a fleet already
  // running: the host you want is scrolled off the bottom of the grid.
  const card = await firstCard(page);
  const name = await card.getAttribute('data-name');

  await page.click('#settingsBtn');
  await expect(page.locator('#setDlg')).toBeVisible();
  const box = page.locator(`[data-host-paused="${name}"]`);
  await expect(box).not.toBeChecked();
  await box.check();

  await page.locator('#setDlg button[value="cancel"]').click();
  await expect(page.locator('#setDlg')).toBeHidden();

  // The grid behind the dialog reflects it - one state, not two.
  await expect(page.locator(`.card[data-name="${name}"]`)).toHaveClass(/paused/);

  // And reopening Settings shows the box still ticked, rather than a control
  // that reverted to what it was rendered with.
  await page.click('#settingsBtn');
  await expect(page.locator(`[data-host-paused="${name}"]`)).toBeChecked();
});

test('typing "paused" finds the hosts that are', async ({ page }) => {
  const card = await firstCard(page);
  const name = await card.getAttribute('data-name');
  await card.locator('[data-pause]').click();
  await expect(card).toHaveClass(/paused/);

  await page.fill('#hostFilter', 'paused');
  const shown = page.locator('.card');
  await expect(shown).toHaveCount(1);
  await expect(shown.first()).toHaveAttribute('data-name', name);
});

test('the paused dot is visible in both themes', async ({ page }) => {
  // A colour defined in only some of the three theme states renders one
  // theme's colour on another theme's ground, and fails in one direction
  // only - which is why this asserts both rather than eyeballing dark.
  const card = await firstCard(page);
  await card.locator('[data-pause]').click();
  const dot = card.locator('.dot');

  for (const theme of ['light', 'dark']) {
    await page.evaluate(t => document.documentElement.dataset.theme = t, theme);
    const colour = await dot.evaluate(el => getComputedStyle(el).borderTopColor);
    expect(colour, `${theme}: the paused dot has no border colour`)
      .toMatch(/^rgba?\(/);
    expect(colour, `${theme}: the paused dot is transparent`)
      .not.toMatch(/rgba\(0, 0, 0, 0\)/);
  }
});

test('the traffic meter stops charging for a paused host', async ({ page }) => {
  // The meter answers "what does this fleet cost per day". A paused host is
  // not sampling, so counting it quotes a figure for traffic that is not
  // happening - the miniature version of the wrong number this project is a
  // reaction to. Its byte counters survive the pause on purpose; what must
  // not survive is the projection.
  const card = await firstCard(page);

  await page.click('#settingsBtn');
  await expect(page.locator('#setDlg')).toBeVisible();
  const before = await page.locator('[data-meter-note]').textContent();
  expect(before).not.toContain('paused');
  const cost = await page.locator('[data-meter-rows] tr.current td').nth(1).textContent();

  await page.locator('#setDlg button[value="cancel"]').click();
  await card.locator('[data-pause]').click();
  await page.click('#settingsBtn');

  const after = await page.locator('[data-meter-note]').textContent();
  expect(after, 'the meter must say a host was left out').toContain('paused');
  await expect
    .poll(() => page.locator('[data-meter-rows] tr.current td').nth(1).textContent())
    .not.toBe(cost);
});
