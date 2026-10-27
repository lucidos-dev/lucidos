import type { Request } from '@playwright/test';
import { test, expect } from './fixtures';
import { gotoWithRetry } from './helpers';
import { SYSTEM_THEME_GRACE_MS } from './systemThemeGrace';

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

async function setHidden(page: import('./fixtures').Page, hidden: boolean): Promise<void> {
  await page.evaluate((h) => {
    Object.defineProperty(document, 'visibilityState', {
      configurable: true,
      get: () => (h ? 'hidden' : 'visible'),
    });
  }, hidden);
}

const PREFERENCE_URL = /\/api\/v1\/preferences(\?|$)/;

function isPreferenceRead(request: Request): boolean {
  return request.method() === 'GET' && PREFERENCE_URL.test(request.url());
}

/** The breadcrumb the shell posts on every repaint (`applyThemeMode`). Seeing
 *  it proves the repaint ran, with no sleep to guess at. */
function isRepaintBreadcrumb(request: Request): boolean {
  if (!request.url().endsWith('/api/v1/internal/client-log')) return false;
  const body = request.postDataJSON() as { category?: string; message?: string } | null;
  return body?.category === 'theme-mode' && body.message === 'applyThemeMode';
}

/** Resolves on the shell's first repaint. Arm it before the page loads. It
 *  never rejects, so a test that does not await it cannot fail on a timeout. */
function firstRepaint(page: import('./fixtures').Page): Promise<void> {
  return new Promise((resolve) => {
    const onRequest = (request: Request) => {
      if (!isRepaintBreadcrumb(request)) return;
      page.off('request', onRequest);
      resolve();
    };
    page.on('request', onRequest);
  });
}

test.describe('system theme survives a late preference load', () => {
  // Startup's first preference load repaints the theme. On a slow runner it
  // lands after the page hid and the OS flipped. Only the follower may move a
  // `system` page, so that repaint has to keep the painted mode.

  // Once `sw.js` controls the page, it sends the preference reads itself, and
  // `page.route` cannot hold them.
  test.use({ serviceWorkers: 'block' });

  test('a preference load landing after the hide and the flip paints nothing', async ({ page }) => {
    const held: Array<() => Promise<void>> = [];
    let sent = 0;
    let releasing = false;
    page.on('request', (request) => {
      if (isPreferenceRead(request)) sent += 1;
    });
    await page.route(PREFERENCE_URL, (route) => {
      if (!isPreferenceRead(route.request()) || releasing) return route.continue();
      held.push(() => route.continue());
    });

    await page.emulateMedia({ colorScheme: 'dark' });
    await gotoWithRetry(page, '/');
    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');
    // The shell reads at startup and again when its event stream opens. A read
    // the hold missed has repainted already, and as the newer load it makes
    // the held one paint nothing.
    await expect.poll(() => sent, 'preference reads sent').toBeGreaterThanOrEqual(2);
    await expect.poll(() => sent - held.length, 'preference reads that escaped the hold').toBe(0);

    await setHidden(page, true);
    await page.emulateMedia({ colorScheme: 'light' });

    const repainted = page.waitForRequest(isRepaintBreadcrumb);
    releasing = true;
    await Promise.all(held.map((release) => release()));
    await repainted;
    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');

    await setHidden(page, false);
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));

    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'light', {
      timeout: SYSTEM_THEME_GRACE_MS,
    });
  });
});

test.describe('system theme follows the OS', () => {
  let startupRepainted: Promise<void>;

  test.beforeEach(async ({ page }) => {
    // No stored theme, so the preference is the `system` default.
    startupRepainted = firstRepaint(page);
    await page.emulateMedia({ colorScheme: 'dark' });
    await gotoWithRetry(page, '/');
    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');
  });

  test('a flip announced while the page is visible is applied', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });

    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'light', {
      timeout: SYSTEM_THEME_GRACE_MS,
    });
  });

  test('a flip announced while hidden waits for the resume, then lands', async ({ page }) => {
    // Startup's first preference load repaints the theme. Hiding before it
    // lands tests that race, which the late-load test above covers on purpose.
    await startupRepainted;
    await setHidden(page, true);
    await page.emulateMedia({ colorScheme: 'light' });

    // The guard: nothing paints while the user is not looking. This is the
    // snapshot-pass flip, and applying it is the light flash.
    await page.waitForTimeout(SYSTEM_THEME_GRACE_MS);
    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');

    // The repair: an iOS PWA is resumed rather than reloaded, so this is the
    // only moment it gets to notice.
    await setHidden(page, false);
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));

    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'light', {
      timeout: SYSTEM_THEME_GRACE_MS,
    });
  });
});
