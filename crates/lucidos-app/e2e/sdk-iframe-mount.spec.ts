import { test, expect } from '@playwright/test';
import { createIframeAppFixture } from './db-helpers';

// Verifies that opening an app via the deep-link flow mounts exactly one
// iframe and triggers exactly one fetch of /api/v1/sdk-prefs.js *from the
// iframe*. The parent shell uses an inline FOUC IIFE (not the endpoint),
// so the only sdk-prefs.js network request should come from the iframe
// itself.
//
// The mount-discipline bug this test guards against: ContentPane is
// rendered by both SplitLayout (desktop) and MobileSwipeContainer (mobile)
// simultaneously — only one is visible, but both mount AppUiInline → both
// create iframes → both load the app HTML → both fetch sdk-prefs.js. The
// user sees one iframe but the engine sees an extra request, an extra app
// session, double cost on every mount.
//
// Service workers are blocked so page.on('request') counts the actual
// network requests without SW-rebroadcast inflation.

const APP_ID = 'e2e-sdk-mount-test';
let fixture: { cleanup: () => void };

test.use({ serviceWorkers: 'block' });

test.describe('App iframe mount — single fetch on open', () => {
  test.beforeAll(() => {
    // Minimal app that opts into theme integration via the prefs script and
    // signals readiness via a #ready div the test waits on.
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

  test('opening an app via deep-link triggers exactly one sdk-prefs.js fetch from the iframe', async ({ page }) => {
    // Bucket requests by the document that initiated them. The parent's
    // FOUC fetch and the iframe's theme fetch both hit the same URL — only
    // the iframe count is the mount-discipline signal.
    const fetchesByFrame = new Map<string, number>();
    page.on('request', (req) => {
      if (!req.url().includes('/api/v1/sdk-prefs.js')) return;
      const frameUrl = req.frame().url();
      fetchesByFrame.set(frameUrl, (fetchesByFrame.get(frameUrl) ?? 0) + 1);
    });

    // Deep-link flow opens the app via openAppById, which mounts the iframe
    // in both desktop and mobile layouts. The user only sees one — the
    // hidden one is wasted bandwidth + a duplicate device-scoped fetch.
    await page.goto(`/?app=${APP_ID}`);

    const iframeLoc = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(iframeLoc).toBeVisible({ timeout: 10_000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    // Settle window so any late mount-time fetches land before we assert.
    await page.waitForTimeout(500);

    const iframeFetches = Array.from(fetchesByFrame.entries()).filter(([url]) => url.includes(`/api/app/${APP_ID}/`));
    const total = iframeFetches.reduce((acc, [, n]) => acc + n, 0);
    expect(
      total,
      `Expected 1 sdk-prefs.js fetch from the app iframe (mount discipline), got ${total}: ${iframeFetches.map(([u, n]) => `${n}×${u}`).join(', ')}`,
    ).toBe(1);
  });

  test('opening an app via deep-link mounts exactly one app iframe', async ({ page }) => {
    await page.goto(`/?app=${APP_ID}`);
    await expect(
      page.locator('iframe[data-role="app-ui-frame"]:visible'),
    ).toBeVisible({ timeout: 10_000 });
    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 10_000 });

    // After a short settle, count ALL data-role iframes in the DOM (visible
    // and hidden). Both layouts may *render* a wrapper, but only the active
    // layout should mount the iframe element itself.
    await page.waitForTimeout(500);
    const iframeCount = await page.locator('iframe[data-role="app-ui-frame"]').count();
    expect(iframeCount).toBe(1);
  });
});
