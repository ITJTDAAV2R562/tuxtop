// End-to-end tests against the browser harness.
//
// These cover what the unit tests structurally cannot: load order, layout, and
// whether a control exists at all. Both of this session's worst frontend bugs
// were of that kind — a temporal-dead-zone error at module scope that killed
// the whole page, and a layout measured before the window had settled — and
// neither could have been caught without a real browser.
//
// The harness mirrors the real fleet. See scripts/harness.py.
const { defineConfig, devices } = require('@playwright/test');

const PORT = 8931;

module.exports = defineConfig({
  testDir: './tests/e2e',
  // The app is a dashboard that redraws on a timer; a test that fails once and
  // passes on retry is hiding something. No retries.
  retries: 0,
  fullyParallel: false,
  reporter: [['list']],
  use: {
    baseURL: `http://127.0.0.1:${PORT}`,
    // A fixed size, because layout is one of the things under test.
    viewport: { width: 1470, height: 900 },
    trace: 'off',
  },
  projects: [{ name: 'chromium', use: { ...devices['Desktop Chrome'] } }],
  webServer: {
    command: `python3 scripts/harness.py --serve ${PORT}`,
    url: `http://127.0.0.1:${PORT}/index.html`,
    // Never reuse. A server left running from an earlier run was serving a
    // harness built earlier, so a change to src/ never reached the browser
    // and the suite passed against code that no longer existed. The harness
    // links to src/ now rather than copying it; this makes doubly sure.
    reuseExistingServer: false,
    timeout: 30_000,
  },
});
