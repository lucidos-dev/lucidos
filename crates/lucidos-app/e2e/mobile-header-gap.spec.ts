/**
 * Regression test: no gap between the fixed mobile header and the
 * sticky thread title bar / content below.
 *
 * Root cause: useHideOnScroll sets --mobile-header-height from JS.
 * offsetHeight truncates to integer, leaving a fractional-pixel gap.
 * getBoundingClientRect().height gives subpixel accuracy, closing it.
 *
 * This has regressed multiple times — see commits 7eb8c4b4, 12f56528,
 * fe4ab2f7, 9d50d02a, 5b6d9e5f, fe082117.
 */
import { test, expect } from '@playwright/test';
import { assertHealthy, navigateToApp, sendMessage, uniqueMessage, waitForResponse, ensureOnThreadPane } from './helpers';

test.describe('Mobile header gap regression', () => {
  test.use({ viewport: { width: 375, height: 812 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('--mobile-header-height matches actual header height (subpixel)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('header-gap');
    await sendMessage(page, `Say exactly: "gap test ${msg}"`);
    await waitForResponse(page);
    await ensureOnThreadPane(page);

    // Wait for useHideOnScroll to measure and set the CSS variable
    await page.waitForFunction(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim() !== ''
    , { timeout: 5_000 });

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      if (!header) return { error: 'no header' };
      const actualHeight = header.getBoundingClientRect().height;
      const cssVar = getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim();
      if (!cssVar) return { error: 'no CSS var' };
      const remSize = parseFloat(getComputedStyle(document.documentElement).fontSize) || 16;
      const cssVarPx = parseFloat(cssVar) * remSize;
      return { actualHeight, cssVarPx, diff: Math.abs(actualHeight - cssVarPx) };
    });

    expect(result).not.toHaveProperty('error');
    const { actualHeight, cssVarPx, diff } = result as { actualHeight: number; cssVarPx: number; diff: number };

    // The CSS variable must match the actual header height within 0.5px.
    // If this fails, someone likely reverted getBoundingClientRect back to offsetHeight.
    expect(diff).toBeLessThan(0.5);
    expect(actualHeight).toBeGreaterThan(0);
    expect(cssVarPx).toBeGreaterThan(0);
  });

  test('thread title bar sits flush against header bottom when sticky', async ({ page }) => {
    await navigateToApp(page);

    // Send a long message to ensure scrollable content
    const msg = uniqueMessage('flush-check');
    await sendMessage(page, `Repeat the word "test" 50 times, each on its own line. Start with: ${msg}`);
    await waitForResponse(page);
    await ensureOnThreadPane(page);

    // Wait for CSS variable to be set before scrolling
    await page.waitForFunction(() =>
      getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim() !== ''
    , { timeout: 5_000 });

    // Scroll down to make the title bar sticky
    await page.evaluate(() => {
      const tc = document.querySelector('.mobile-swipe-pane .thread-content.visible');
      if (tc) tc.scrollTop = 200;
    });

    // Wait for scroll to settle and rAF-debounced measurement to fire
    await page.waitForTimeout(100);

    const result = await page.evaluate(() => {
      const header = document.querySelector('.app-header');
      const titleRow = document.querySelector('.mobile-swipe-pane .mobile-thread-title-row');
      if (!header || !titleRow) return { error: 'missing elements' };
      const headerBottom = header.getBoundingClientRect().bottom;
      const titleTop = titleRow.getBoundingClientRect().top;
      return { gap: titleTop - headerBottom };
    });

    expect(result).not.toHaveProperty('error');
    const { gap } = result as { gap: number };

    // Gap must be less than 1px — any more means content peeks through.
    // Negative values (overlap) are fine and expected from subpixel rounding.
    expect(gap).toBeLessThan(1);
  });
});
