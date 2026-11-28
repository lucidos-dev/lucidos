import type { Request } from '@playwright/test';
import { test, expect } from './fixtures';
import { gotoWithRetry } from './helpers';
import { SYSTEM_THEME_GRACE_MS } from './systemThemeGrace';


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

function isRepaintBreadcrumb(request: Request): boolean {
  if (!request.url().endsWith('/api/v1/internal/client-log')) return false;
  const body = request.postDataJSON() as { category?: string; message?: string } | null;
  return body?.category === 'theme-mode' && body.message === 'applyThemeMode';
}

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
    startupRepainted = firstRepaint(page);
    await page.emulateMedia({ colorScheme: 'dark' });
    await gotoWithRetry(page, '/');
    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');
    await startupRepainted;
  });

  test('a flip announced while the page is visible is applied', async ({ page }) => {
    await page.emulateMedia({ colorScheme: 'light' });

    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'light', {
      timeout: SYSTEM_THEME_GRACE_MS,
    });
  });

  test('a flip announced while hidden waits for the resume, then lands', async ({ page }) => {
    await setHidden(page, true);
    await page.emulateMedia({ colorScheme: 'light' });

    await page.waitForTimeout(SYSTEM_THEME_GRACE_MS);
    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'dark');

    await setHidden(page, false);
    await page.evaluate(() => document.dispatchEvent(new Event('visibilitychange')));

    await expect(page.locator('html')).toHaveAttribute('data-theme-mode', 'light', {
      timeout: SYSTEM_THEME_GRACE_MS,
    });
  });
});
