// The Add host dialog's defaults and keyboard behaviour.
//
// The dialog began life as a design mockup and shipped its mockup values -
// "macaw" in Hostname and "macaw.example.ts.net" in Address - so adding a
// real host meant deleting someone else's example first.
//
// Selecting a field's contents when you tab into it is the browser's own
// behaviour, not ours: there is deliberately no handler for it in app.js, and
// a mutation test confirmed one would be dead code. These tests pin the
// behaviour anyway, because it is what makes the remaining default harmless -
// a focus handler added later for some other reason could silently collapse
// the selection to a caret, and typing would start appending to "macaw".

const { test, expect } = require('@playwright/test');

async function openDialog(page) {
  await page.goto('/index.html');
  await expect(page.locator('.card').first()).toBeVisible();
  await page.click('#addBtn');
  await expect(page.locator('#addDlg')).toBeVisible();
}

/** What the browser has selected inside `sel`, as a string. */
function selectionIn(page, sel) {
  return page.$eval(sel, el => el.value.slice(el.selectionStart, el.selectionEnd));
}

test('the address field ships no default to delete', async ({ page }) => {
  await openDialog(page);
  await expect(page.locator('#f-addr')).toHaveValue('');
  // The hostname keeps its default: one example name reads as a hint, and it
  // is selected on open, so it costs no keystrokes to replace.
  await expect(page.locator('#f-name')).toHaveValue('macaw');
});

test('opening the dialog selects the hostname, so typing replaces it', async ({ page }) => {
  await openDialog(page);
  expect(await selectionIn(page, '#f-name')).toBe('macaw');
  await page.keyboard.type('coot');
  await expect(page.locator('#f-name')).toHaveValue('coot');
});

test('tabbing into a filled field selects it, so typing replaces rather than appends', async ({ page }) => {
  await openDialog(page);

  // Give Address something to tab back into - the realistic case is returning
  // to a field to fix a typo, not the pristine dialog.
  await page.keyboard.press('Tab');
  await expect(page.locator('#f-addr')).toBeFocused();
  await page.keyboard.type('192.0.2.13');

  // Backwards into a field holding a default...
  await page.keyboard.press('Shift+Tab');
  expect(await selectionIn(page, '#f-name')).toBe('macaw');

  // ...and forwards into a field holding what we just typed. Both directions,
  // because they are separate paths through the browser's focus handling.
  await page.keyboard.press('Tab');
  expect(await selectionIn(page, '#f-addr')).toBe('192.0.2.13');

  // The assertion that distinguishes selected from merely focused: with a
  // caret instead of a selection this yields "192.0.2.13dove".
  await page.keyboard.type('dove');
  await expect(page.locator('#f-addr')).toHaveValue('dove');
});
