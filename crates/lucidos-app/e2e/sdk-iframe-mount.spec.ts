import { test, expect } from './fixtures';
import { appPath, createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';


const APP_ID = 'e2e-sdk-mount-test';
let fixture: { cleanup: () => void };

test.use({ serviceWorkers: 'block' });

test.describe('App iframe mount — single fetch on open', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'SDK mount test', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>Mount test</title>
<script src="/api/v1/sdk-prefs.js"></script>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
</head>
<body>
<div id="ready">ready</div>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript((id) => {
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
  }

  test('opening an app via deep-link triggers exactly one sdk-prefs.js fetch from the iframe', async ({ page }) => {
    const fetchesByFrame = new Map<string, number>();
    page.on('request', (req) => {
      if (!req.url().includes('/api/v1/sdk-prefs.js')) return;
      const frameUrl = req.frame().url();
      fetchesByFrame.set(frameUrl, (fetchesByFrame.get(frameUrl) ?? 0) + 1);
    });

    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    await page.waitForTimeout(500);

    const iframeFetches = Array.from(fetchesByFrame.entries()).filter(([url]) => url.includes(appPath(APP_ID)));
    const total = iframeFetches.reduce((acc, [, n]) => acc + n, 0);
    expect(
      total,
      `Expected 1 sdk-prefs.js fetch from the app iframe (mount discipline), got ${total}: ${iframeFetches.map(([u, n]) => `${n}×${u}`).join(', ')}`,
    ).toBe(1);
  });

  test('opening an app via deep-link mounts exactly one app iframe', async ({ page }) => {
    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');
    await expect(
      page.locator('iframe[data-role="app-ui-frame"]:visible'),
    ).toBeVisible({ timeout: 10_000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    await page.waitForTimeout(500);
    const iframeCount = await page.locator('iframe[data-role="app-ui-frame"]').count();
    expect(iframeCount).toBe(1);
  });
});
