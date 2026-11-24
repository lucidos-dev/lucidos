import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, waitForVisibleInput, blurActiveElement, getHeaderTop, enableMobileDynamicBars, disableMobileDynamicBars } from './helpers';

test.describe('Mobile header in compose view', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await enableMobileDynamicBars(page);
  });

  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('prompt is not auto-focused and header is visible after reload in compose view', async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.removeItem('lucidos-focused-thread');
    });
    await navigateToApp(page);
    await waitForVisibleInput(page);

    await page.waitForTimeout(200);

    const isFocused = await page.evaluate(() => {
      const els = document.querySelectorAll('[data-role="prompt-input"]');
      return Array.from(els).some(el => document.activeElement === el);
    });
    expect(isFocused).toBe(false);

    expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);
  });

  test('header hides on prompt focus and reappears on blur in compose view', async ({ page }) => {
    await page.addInitScript(() => {
      localStorage.removeItem('lucidos-focused-thread');
    });
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await blurActiveElement(page);
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      return header ? header.getBoundingClientRect().top >= 0 : false;
    }, undefined, { timeout: 5_000 });

    expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);

    await input.focus();
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      if (!header) return false;
      return header.getBoundingClientRect().bottom <= 0;
    }, undefined, { timeout: 5_000 });

    const headerBottom = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      return header ? header.getBoundingClientRect().bottom : 999;
    });
    expect(headerBottom).toBeLessThanOrEqual(0);

    await input.blur();
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      if (!header) return false;
      return header.getBoundingClientRect().top >= 0;
    }, undefined, { timeout: 5_000 });

    expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);
  });
});
