import type { Page } from '@playwright/test';
import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

// An app repaints with the shell, not after the save (the appearance push,
// docs/glossary.md). The shell saves a scale change on a debounce. An app that
// waited for the save, the SSE frame and a re-fetch trailed it by all three.
//
// Every preference save is held open here, so nothing can reach the app over
// the network. It must repaint anyway. Desktop only: the zoom shortcut is the
// keyboard path into a scale change.

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

  /** Open the fixture app in the real shell, with every preference save held
   *  open so no `PreferencesChanged` can follow one. */
  async function openWatchingApp(page: Page) {
    // Seeding `app-window-open` makes `loadApps()` restore the app through the
    // real `AppUiInline`, so the shell pushes to a frame it built itself.
    await page.addInitScript((id) => {
      // An init script runs in EVERY frame, and the app frame's storage throws.
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

  // The app forwards the chord, and the scale panel takes focus from its frame.
  // That window `focus` runs the resume sync. Its preferences refetch must not
  // paint the saved scale back before the debounced save goes out.
  test('a zoom step from inside the app never snaps back', async ({ page }) => {
    const appFrame = await openWatchingApp(page);
    await appFrame.locator('#status').click();
    const shell = page.locator('html');
    const before = await shell.evaluate(scaleOf);

    // Every frame's scale, so a snap back that lasts one frame still shows.
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
    // Past the 500 ms save debounce, the window that refetch lands in.
    await page.waitForTimeout(1_000);
    const seen = await page.evaluate(() => (window as unknown as { scalesSeen: string[] }).scalesSeen);
    expect(seen).toEqual([before, after]);
    expect(await appFrame.locator('html').evaluate(scaleOf)).toBe(after);
  });
});
