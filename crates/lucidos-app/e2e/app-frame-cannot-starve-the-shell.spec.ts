import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

// The reported bug: Chrome showed Page Unresponsive and the dialog named the
// Lucidos tab, not the app. The app iframe carried `allow-same-origin`, so it
// shared the shell's renderer process and its main thread. An app that saturates
// that thread takes the whole workspace with it, and a reload lands straight
// back in the restored app.
//
// The app here blocks its main thread outright, which is the worst an app can
// do. What is asserted is the SHELL: its own timer keeps ticking throughout.
// Before the fix it did not tick at all.

const APP_ID = 'e2e-busy-app';
/** How long the app refuses to yield. Long enough that a shared thread cannot
 *  hide it, short enough to keep the spec quick. */
const BLOCK_MS = 4000;
/** The shell's heartbeat interval, and the window it is measured over. */
const TICK_MS = 50;
const WATCH_MS = BLOCK_MS + 1000;

/** Undefined until `beforeAll` builds it, and on WebKit it never does: every
 *  test in the suite is skipped there, so Playwright runs no `beforeAll` and
 *  still runs the `afterAll` below. */
let fixture: { dir: string; cleanup: () => void } | undefined;

// Playwright's bundled Chromium runs with site isolation OFF, and that is the
// very feature that gives a sandboxed frame its own process. Measured on this
// fixture: real Google Chrome at default flags keeps all 280 host ticks, the
// bundled browser at default flags keeps 41. Without the flag this file would
// assert that the fix does not work, against a browser nobody uses.
test.use({ launchOptions: { args: ['--site-per-process'] } });

test.describe('an app frame cannot starve the shell', () => {
  // The flag above is Chromium's, and Playwright's WebKit models neither it nor
  // Safari's own process assignment: the same app blocks the shell there at
  // 20 ticks of 100. That is the harness, not the product, and asserting a
  // property this browser cannot produce would only pin the harness. The
  // property is measured in Chromium; Safari's is unverified.
  test.skip(({ browserName }) => browserName === 'webkit',
    'Playwright WebKit does not model Safari process assignment for a sandboxed frame');

  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Busy app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>Busy app</title></head>
<body>
<div id="ready">ready</div>
<script>
  // Block after a beat, so the frame is mounted and visible first.
  setTimeout(function () {
    var end = Date.now() + ${BLOCK_MS};
    while (Date.now() < end) { /* refuse to yield */ }
  }, 300);
</script>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  /** Seeding `app-window-open` makes `loadApps()` restore the app through the
   *  real `AppUiInline`. So the iframe carries production's own sandbox rather
   *  than a string copied into this file, and the route is the one the user's
   *  reload took. */
  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript(({ id, tick }) => {
      // An init script runs in EVERY frame, and the app frame's storage throws
      // because it is isolated. Only the top frame is the shell, and only the
      // shell's heartbeat is the measurement.
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
      (window as unknown as { __hostTicks: number }).__hostTicks = 0;
      setInterval(() => { (window as unknown as { __hostTicks: number }).__hostTicks++; }, tick);
    }, { id: APP_ID, tick: TICK_MS });
  }

  /** How many times the shell's own timer fired while the app was blocking. */
  async function ticksDuringBlock(page: import('@playwright/test').Page): Promise<number> {
    const read = () => page.evaluate(
      () => (window as unknown as { __hostTicks: number }).__hostTicks,
    );
    const before = await read();
    await page.waitForTimeout(WATCH_MS);
    return (await read()) - before;
  }

  /** Half the ticks due, a wide floor on purpose. A loaded CI host is late, and
   *  a SHARED thread is not late but absent: the measured before-state was 41
   *  ticks of 280, under 15%. */
  const TICK_FLOOR = (WATCH_MS / TICK_MS) / 2;

  test('the shell keeps running while an app refuses to yield', async ({ page }) => {
    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 15_000 });

    expect(await ticksDuringBlock(page)).toBeGreaterThan(TICK_FLOOR);
  });

  test('a reload lands back in the app, and the shell still runs', async ({ page }) => {
    // "reload got me nothing": the restore is what the user reached for, so it
    // has to keep working rather than be withheld.
    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');
    await expect(page.frameLocator('iframe[data-role="app-ui-frame"]:visible')
      .locator('#ready')).toBeVisible({ timeout: 15_000 });

    await page.reload();

    await expect(page.frameLocator('iframe[data-role="app-ui-frame"]:visible')
      .locator('#ready')).toBeVisible({ timeout: 15_000 });
    expect(await ticksDuringBlock(page)).toBeGreaterThan(TICK_FLOOR);
  });
});
