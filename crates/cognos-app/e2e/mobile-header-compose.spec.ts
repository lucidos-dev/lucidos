import { test, expect } from '@playwright/test';
import { assertHealthy, navigateToApp, waitForVisibleInput, blurActiveElement, getHeaderTop } from './helpers';

test.describe('Mobile header in compose view', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('prompt is not auto-focused and header is visible after reload in compose view', async ({ page }) => {
    // Clear focused thread so we land on compose view
    await page.addInitScript(() => {
      localStorage.removeItem('cognos-focused-thread');
    });
    await navigateToApp(page);
    await waitForVisibleInput(page);

    // Give any rAF-based auto-focus time to fire
    await page.waitForTimeout(200);

    // Prompt should NOT be focused (no auto-focus on mobile reload)
    const isFocused = await page.evaluate(() => {
      const els = document.querySelectorAll('[data-role="prompt-input"]');
      return Array.from(els).some(el => document.activeElement === el);
    });
    expect(isFocused).toBe(false);

    // Header should be visible (not hidden by keyboard-open logic)
    expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);
  });

  test('header hides on prompt focus and reappears on blur in compose view', async ({ page }) => {
    // Clear focused thread so we land on compose view
    await page.addInitScript(() => {
      localStorage.removeItem('cognos-focused-thread');
    });
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    // Prompt may auto-focus on compose view load, hiding the header.
    // Blur any focused element so the header reappears before we test.
    await blurActiveElement(page);
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      return header ? header.getBoundingClientRect().top >= 0 : false;
    }, undefined, { timeout: 5_000 });

    // Header should be visible before focus
    expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);

    // Focus the prompt — header should hide
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

    // Blur the prompt — header should reappear
    await input.blur();
    await page.waitForFunction(() => {
      const header = document.querySelector('.app-header');
      if (!header) return false;
      return header.getBoundingClientRect().top >= 0;
    }, undefined, { timeout: 5_000 });

    expect(await getHeaderTop(page)).toBeGreaterThanOrEqual(0);
  });
});
