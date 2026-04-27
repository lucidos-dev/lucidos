import { test, expect } from '@playwright/test';
import { assertHealthy, navigateToApp, sendMessage, uniqueMessage, waitForResponse } from './helpers';

test.describe('Mobile thread content visibility', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('thread content renders with non-zero height on mobile viewport', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('mobile-content');
    await sendMessage(page, `Say exactly: "hello ${msg}"`);

    const response = await waitForResponse(page);
    const responseText = await response.textContent();
    expect(responseText!.trim().length).toBeGreaterThan(0);

    // .thread-content must have non-zero rendered height on mobile.
    // iOS Safari collapses height: 100% inside flex items — the fix uses
    // position: absolute; inset: 0 instead.
    const contentHeight = await page.evaluate(() => {
      const els = document.querySelectorAll('.thread-content');
      for (const el of els) {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0) return rect.height;
      }
      return 0;
    });
    expect(contentHeight).toBeGreaterThan(100);
  });
});
