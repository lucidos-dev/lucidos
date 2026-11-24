import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';


const APP_ID = 'e2e-busy-app';
const BLOCK_MS = 4000;
const TICK_MS = 50;
const WATCH_MS = BLOCK_MS + 1000;

let fixture: { dir: string; cleanup: () => void } | undefined;

test.use({ launchOptions: { args: ['--site-per-process'] } });

test.describe('an app frame cannot starve the shell', () => {
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

  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript(({ id, tick }) => {
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
      (window as unknown as { __hostTicks: number }).__hostTicks = 0;
      setInterval(() => { (window as unknown as { __hostTicks: number }).__hostTicks++; }, tick);
    }, { id: APP_ID, tick: TICK_MS });
  }

  async function ticksDuringBlock(page: import('@playwright/test').Page): Promise<number> {
    const read = () => page.evaluate(
      () => (window as unknown as { __hostTicks: number }).__hostTicks,
    );
    const before = await read();
    await page.waitForTimeout(WATCH_MS);
    return (await read()) - before;
  }

  const TICK_FLOOR = (WATCH_MS / TICK_MS) / 2;

  test('the shell keeps running while an app refuses to yield', async ({ page }) => {
    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 15_000 });

    expect(await ticksDuringBlock(page)).toBeGreaterThan(TICK_FLOOR);
  });

  test('a reload lands back in the app, and the shell still runs', async ({ page }) => {
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
