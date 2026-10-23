import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  disableMobileDynamicBars,
  enableMobileDynamicBars,
  ensureMobileView,
  navigateToApp,
} from './helpers';

/** Opening Search everywhere on a phone leaves the screen behind it still.
 *
 *  The palette focuses its own field. A field inside an overlay moves nothing
 *  behind it: the header, the spacer and the transcript all hold still, with
 *  dynamic bars on as well (`hooks/useHideOnScroll.ts`). */

async function headerState(page: Page): Promise<{ top: number; spacer: string }> {
  return page.evaluate(() => {
    const header = document.querySelector('.app-header');
    return {
      top: header ? header.getBoundingClientRect().top : Number.NaN,
      spacer: getComputedStyle(document.documentElement).getPropertyValue('--mobile-header-height').trim(),
    };
  });
}

test.describe('Search everywhere (phone)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // The header only slides with dynamic bars on. Must precede navigate.
    await enableMobileDynamicBars(page);
  });

  // The pref is global and the e2e database resets only between projects.
  test.afterEach(async ({ page }) => {
    await disableMobileDynamicBars(page);
  });

  test('opening it keeps the header and the spacer where they were', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');
    const before = await headerState(page);
    expect(before.top).toBe(0);

    await page.locator('[data-role="brand-menu-toggle"]:visible').first().tap();
    await page.locator('.brand-menu-item', { hasText: 'Search everywhere' }).tap();
    const input = page.locator('.search-everywhere-input');
    await expect(input).toBeFocused();

    // Longer than the header's glide, so a slide that started has landed.
    await page.waitForTimeout(600);
    expect(await headerState(page)).toEqual(before);

    // Closing brings nothing back either, since nothing left.
    await page.locator('.search-everywhere-close').tap();
    await expect(input).toBeHidden();
    await page.waitForTimeout(600);
    expect(await headerState(page)).toEqual(before);
  });
});
