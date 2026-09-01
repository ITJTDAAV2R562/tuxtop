// Layout, which is the other thing a browser is needed for.
//
// Every assertion here corresponds to a bug that reached the user: the toolbar
// clipping "Add host" off the right edge, the view tabs moving between tabs,
// a button squeezed until its label wrapped, and core charts laid out against
// a width the window had not settled into yet.

const { test, expect } = require('@playwright/test');

const VIEWS = [
  ['Hosts', '#viewHosts'],
  ['Fleet', '#viewAll'],
  ['History', '#viewHist'],
  ['Processes', '#viewProcs'],
];

/// Open the page and wait for the fleet to stop arriving.
///
/// Every test here measures or clicks a toolbar control, and the toolbar is
/// the one thing on the page that is still changing size a second after load:
/// cards are created as hosts report, and the tally beside them counts up from
/// "0 up" to "19 up" while `ncores` fills in. So the buttons move, and
/// Playwright will not click an element that is not *stable* - it waits the
/// full 30s and fails. That is what made the first test in this file the one
/// that failed most, and it failed on the commit before this feature too:
/// alternating A/B runs had baseline failing two of these at load 15 while the
/// current tree passed. Waiting for `nup` to reach `nhosts` waits for the last
/// host, after which nothing in the toolbar moves on its own.
async function load(page) {
  await page.goto('/index.html');
  // The tally, not a card: one of these tests opens straight into History,
  // where there are no cards at all. The toolbar is present in every view and
  // is the thing whose settling actually matters here.
  await expect
    .poll(async () => {
      const [up, all] = await Promise.all([
        page.locator('#nup').textContent(),
        page.locator('#nhosts').textContent(),
      ]);
      return Number(all) > 1 && up === all;
    }, { message: 'every host reporting, so the toolbar stops resizing' })
    .toBe(true);
}

async function toolbar(page) {
  return page.evaluate(() => {
    const tb = document.querySelector('.toolbar');
    const kids = [...tb.children].filter(k => !k.hidden && k.getBoundingClientRect().height > 0);
    const add = document.querySelector('#addBtn').getBoundingClientRect();
    const app = tb.parentElement.getBoundingClientRect();
    return {
      tabsX: Math.round(document.querySelector('.seg[aria-label="View"]').getBoundingClientRect().left),
      addRight: Math.round(add.right),
      addHeight: Math.round(add.height),
      appRight: Math.round(app.right),
      rows: new Set(kids.map(k => {
        const r = k.getBoundingClientRect();
        return Math.round(r.top + r.height / 2);
      })).size,
      pageOverflow: document.documentElement.scrollWidth > window.innerWidth + 1,
    };
  });
}

test('no view clips a control off the edge', async ({ page }) => {
  // "Theme" and "Add host" were cut off entirely on the wider layout. A
  // control you cannot see or click is worse than one on a second row.
  await load(page);
  for (const [name, id] of VIEWS) {
    await page.click(id);
    await page.waitForTimeout(400);
    const t = await toolbar(page);
    expect(t.addRight, `${name}: Add host past the panel edge`).toBeLessThanOrEqual(t.appRight);
    expect(t.pageOverflow, `${name}: page scrolls sideways`).toBe(false);
  }
});

test('the view tabs sit in the same place in every view', async ({ page }) => {
  // They used to move ~400px between views, because a flexible spacer sat
  // before them and the slack changed with which controls a view shows.
  await load(page);
  const seen = [];
  for (const [name, id] of VIEWS) {
    await page.click(id);
    await page.waitForTimeout(400);
    seen.push([name, (await toolbar(page)).tabsX]);
  }
  const first = seen[0][1];
  for (const [name, x] of seen) {
    expect(Math.abs(x - first), `${name} tabs at ${x}, expected ${first}`).toBeLessThanOrEqual(2);
  }
});

test('no toolbar button is squeezed until its label wraps', async ({ page }) => {
  // Stopping the row wrapping let flex shrink the buttons instead, and
  // "Add host" broke onto two lines - the button changing shape between views.
  await load(page);
  const heights = [];
  for (const [, id] of VIEWS) {
    await page.click(id);
    await page.waitForTimeout(400);
    heights.push((await toolbar(page)).addHeight);
  }
  expect(new Set(heights).size, `Add host changed height: ${heights}`).toBe(1);
  expect(heights[0]).toBeLessThan(40);
});

test('core charts lay out against the width they actually get', async ({ page }) => {
  // On first launch History came up four charts to a row and only corrected
  // itself when a tab switch happened to rebuild it. Core counts are 2, 4, 8,
  // 16, 24, 32, so rows snap to a multiple of eight rather than to whatever
  // happens to fit.
  // Wide enough that the two rules disagree.
  //
  // At the default 1470px, eleven charts do not fit but eight do - so "snap
  // to a multiple of eight" and "as many as fit" both answer 8, and the test
  // cannot tell them apart. It passed against a deliberately broken niceCols
  // for exactly that reason. At 1800px about eleven fit, so the correct
  // answer is 8 and the broken one is 11.
  await page.setViewportSize({ width: 1800, height: 900 });

  // Drive History into the fleet-wide per-core view deliberately. Opened on
  // whichever host it remembers, it may show a 4-core box - and four cores is
  // one row under any rule, so the test would pass without exercising
  // anything. The interesting case is a 32-core host.
  await page.addInitScript(() => {
    localStorage.setItem('tuxtop.prefs', JSON.stringify({
      view: 'history', metric: 'cores', slice: { mode: 'metric', metric: 'cores' },
    }));
  });
  await load(page);
  await page.locator('.cores-hist').first().waitFor({ timeout: 15_000 });

  // Both shapes: `.packed` across the fleet, plain in host mode. Both set an
  // explicit column count, because CSS auto-fill picked the same arbitrary 9.
  const cols = await page.evaluate(() =>
    [...document.querySelectorAll('.cores-hist')].map(sec => ({
      cores: sec.querySelectorAll('.core-chart').length,
      cols: +(sec.querySelector('.core-charts').style.gridTemplateColumns.match(/repeat\((\d+)/) || [])[1],
    })));

  expect(cols.length).toBeGreaterThan(0);
  // Without a big host in the set the rule is untested: any count under eight
  // is a single row whatever the rule says.
  const big = cols.filter(c => c.cores >= 16);
  expect(big.length, 'no host with enough cores to exercise the rule').toBeGreaterThan(0);
  // And the width must actually allow more than the answer, or "snap" and
  // "whatever fits" coincide and nothing is being tested.
  expect(Math.max(...big.map(c => c.cols)),
         'viewport too narrow to distinguish the two rules').toBeLessThan(
           await page.evaluate(() => {
             const g = document.querySelector('#grid');
             const avail = Math.max(240, g.clientWidth - 28);
             return Math.max(1, Math.floor((avail - 26 + 5) / (126 + 5)));
           }));
  for (const c of cols) {
    if (c.cores >= 8) {
      expect(c.cols % 8, `${c.cores} cores laid out ${c.cols} to a row`).toBe(0);
    } else {
      expect(c.cols, `${c.cores} cores should be one row`).toBe(c.cores);
    }
  }
});

test('a narrow window wraps rather than clipping', async ({ page }) => {
  // The safe failure. Below the width the controls need, the row must break -
  // never overflow.
  await page.setViewportSize({ width: 1180, height: 900 });
  await load(page);
  for (const [name, id] of VIEWS) {
    await page.click(id);
    await page.waitForTimeout(400);
    const t = await toolbar(page);
    expect(t.addRight, `${name} clipped when narrow`).toBeLessThanOrEqual(t.appRight);
    expect(t.pageOverflow, `${name} scrolls sideways when narrow`).toBe(false);
  }
});

test('leaving a view restores the layout it found', async ({ page }) => {
  // Twice the usual budget, because this test genuinely needs it and the
  // default left none. It drives twelve view switches, each with a 400ms
  // settle, and renders the heat strip - nineteen rows against six hundred
  // columns - four times. Measured alone on an idle box it takes 24s of a 30s
  // limit, so any concurrency at all pushes it over, and it then fails as a
  // 30s timeout that looks exactly like the flake everything else in this file
  // was suffering from. It is not that: it is the one test here that is
  // legitimately slow, and it was the last thing left failing once the real
  // flake - clicking a toolbar that was still resizing - had been fixed.
  //
  // Raising a timeout is the wrong reflex nine times in ten. This is the tenth:
  // the wait was measured rather than guessed, and the work being waited on is
  // the test's own subject, not a slow harness.
  test.setTimeout(60_000);

  // Heat put `heat-mode` on the grid and nothing ever took it off, so
  // `.grid.heat-mode{display:block}` survived into every other view and made
  // each host card claim a full row. Each builder adding its own class and
  // trusting every *other* builder to strip it works right up until a fifth
  // view exists.
  await load(page);
  await expect(page.locator('.card').first()).toBeVisible();

  const gridClass = () => page.locator('#grid').getAttribute('class');
  const others = ['#viewHosts', '#viewAll', '#viewProcs', '#viewHist'];

  const fresh = {};
  for (const id of others) {
    await page.click(id);
    await page.waitForTimeout(400);
    fresh[id] = await gridClass();
  }

  for (const id of others) {
    await page.click('#viewHeat');
    await expect(page.locator('.heatwrap')).toBeVisible();
    await page.click(id);
    await page.waitForTimeout(400);
    expect(await gridClass(), `${id} looks different after visiting Heat`)
      .toBe(fresh[id]);
  }
});
