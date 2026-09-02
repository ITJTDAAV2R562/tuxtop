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
const os = require('os');

const PORT = 8931;

module.exports = defineConfig({
  testDir: './tests/e2e',
  // The app is a dashboard that redraws on a timer; a test that fails once and
  // passes on retry is hiding something. No retries.
  retries: 0,
  // One worker per four cores, not "half the cores".
  //
  // Every page in this suite animates nineteen hosts at 2.5 Hz against canvas,
  // so a worker is a sustained CPU load rather than a mostly-idle browser
  // waiting on a server. Playwright's default of cores/2 is eight on the
  // development box, and at eight the machine cannot keep up: pages stop
  // repainting promptly, toolbar controls never hold still for two consecutive
  // frames, and Playwright - which refuses to click an element that is not
  // stable - waits out the full 30s. It presents as several tests failing at
  // random across specs that have nothing to do with each other, which is
  // exactly what it looks like when someone has broken the app, and it cost
  // most of an afternoon to convince myself it had not.
  //
  // This is not the same fault as the harness-server one in CLAUDE.md, though
  // it wears its face. That one was a single-threaded server and was fixed by
  // making it threaded; this server was measured at eight parallel page loads
  // in under 80ms and is not the bottleneck. Nor is it a timeout to raise -
  // the tests are not slow, the box is saturated. The only honest fix is to
  // stop asking for more concurrency than there is machine.
  //
  // Measured on a 16-core box: eight workers gave one to six failures a run;
  // four gave 34/34 repeatably. That is the ratio below - four cores to a
  // worker - and it has to be a ratio rather than the 4 that used to be
  // hardcoded here, because the number was silently tied to one machine. The
  // first CI run on a GitHub-hosted runner failed fifteen tests with exactly
  // the symptom above: same four workers, a small fraction of the cores.
  workers: Math.max(1, Math.floor(os.cpus().length / 4)),
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
