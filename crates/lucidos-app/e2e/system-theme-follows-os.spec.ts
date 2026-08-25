import { test, expect } from './fixtures';
import { gotoWithRetry } from './helpers';

// A `system` theme preference has to keep following the OS for the whole life
// of the page. The two guards that make that safe are what this covers
// (ADR 0092).
//
// Backgrounding an iOS app makes UIKit flip its trait collection to the
// opposite appearance and back, to render both app-switcher snapshots
// (rdar://7213631). WKWebView passes each flip in as a real media query
// change, so acting on one paints an appearance nobody asked for. Anything
// announced while the document is hidden is therefore dropped, and the page's
// own resume is what repairs it.
//
// The `mobile-webkit` project is the closest automated stand-in for the
// installed PWA, since it runs the same engine under an iPhone user agent.

/** Long enough to cover the settle delay in `preferences.ts` and a repaint. */
const SETTLE_GRACE_MS = 2_000;

async function setHidden(page: import('./fixtures').Page, hidden: boolean): Promise<void> {
  await page.evaluate((h) => {
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => (h ? 'hidden' : 'visible'),
    });
  }, hidden);
}

test.describe('system theme follows the OS', () => {
  test.beforeEach(async ({ page }) => {
    // No stored theme, so the preference is the `system` default.
    await page.emulateMedia({ colorScheme: 'dark' });
    await gotoWithRetry(page, '/');
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');
  });

  test('a flip announced while the page is visible is applied', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });

    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light', {
      timeout: SETTLE_GRACE_MS,
    });
  });

  test('a flip announced while hidden waits for the resume, then lands', async ({ page }) => {
    await setHidden(page, true);
    await page.emulateMedia({ colorScheme: 'light' });

    // The guard: nothing paints while the user is not looking. This is the
    // snapshot-pass flip, and applying it is the light flash.
    await page.waitForTimeout(SETTLE_GRACE_MS);
    await expect(page.locator('html')).toHaveAttribute('data-theme', 'dark');

    // The repair: an iOS PWA is resumed rather than reloaded, so this is the
    // only moment it gets to notice.
    await setHidden(page, false);
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));

    await expect(page.locator('html')).toHaveAttribute('data-theme', 'light', {
      timeout: SETTLE_GRACE_MS,
    });
  });
});
