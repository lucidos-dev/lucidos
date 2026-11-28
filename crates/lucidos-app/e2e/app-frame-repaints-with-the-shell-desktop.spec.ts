import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';


const APP_ID = 'e2e-appearance-push';

let fixture: { cleanup: () => void } | undefined;

test.describe('an app frame repaints with the shell', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Appearance push', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Appearance push</title>
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
</head>
<body>
<div id="status">init</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `
lucidos.ui.applyPreferences().then(function () {
  lucidos.ui.watchPreferences();
  document.getElementById('status').textContent = 'watching';
});
`,
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  async function openWatchingApp(page: Page) {
    await page.addInitScript((id) => {
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
    await gotoWithRetry(page, '/');

    const frameSelector = `iframe[data-role="app-ui-frame"][src*="${APP_ID}"]`;
    const appFrame = page.frameLocator(frameSelector).first();
    await expect(appFrame.locator('#status')).toHaveText('watching', { timeout: 15_000 });

    await page.route('**/api/v1/preferences**', (route) => {
      if (route.request().method() !== 'PUT') return route.continue();
      return new Promise<void>(() => {});
    });
    return appFrame;
  }

  const scaleOf = (root: Element) => getComputedStyle(root).getPropertyValue('--user-ui-scale').trim();

  test('a zoom step reaches the app while the save is still pending', async ({ page }) => {
    const appFrame = await openWatchingApp(page);
    const shell = page.locator('html');
    const before = await shell.evaluate(scaleOf);

    await page.keyboard.press('ControlOrMeta+Equal');

    await expect.poll(() => shell.evaluate(scaleOf)).not.toBe(before);
    const after = await shell.evaluate(scaleOf);
    await expect.poll(() => appFrame.locator('html').evaluate(scaleOf), { timeout: 2_000 }).toBe(after);
  });

  test('a zoom step from inside the app never snaps back', async ({ page }) => {
    const appFrame = await openWatchingApp(page);
    await appFrame.locator('#status').click();
    const shell = page.locator('html');
    const before = await shell.evaluate(scaleOf);

    await page.evaluate(() => {
      const seen: string[] = [];
      (window as unknown as { scalesSeen: string[] }).scalesSeen = seen;
      const sample = () => {
        const scale = getComputedStyle(document.documentElement).getPropertyValue('--user-ui-scale').trim();
        if (seen[seen.length - 1] !== scale) seen.push(scale);
        requestAnimationFrame(sample);
      };
      sample();
    });

    await page.keyboard.press('ControlOrMeta+Equal');

    await expect.poll(() => shell.evaluate(scaleOf)).not.toBe(before);
    const after = await shell.evaluate(scaleOf);
    await page.waitForTimeout(1_000);
    const seen = await page.evaluate(() => (window as unknown as { scalesSeen: string[] }).scalesSeen);
    expect(seen).toEqual([before, after]);
    expect(await appFrame.locator('html').evaluate(scaleOf)).toBe(after);
  });
});
