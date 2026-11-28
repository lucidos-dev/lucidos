import { test, expect, type Page } from './fixtures';
import {
  assertHealthy,
  disableMobileDynamicBars,
  enableMobileDynamicBars,
  ensureMobileView,
  navigateToApp,
} from './helpers';


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
    await enableMobileDynamicBars(page);
  });

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

    await page.waitForTimeout(600);
    expect(await headerState(page)).toEqual(before);

    await page.locator('.search-everywhere-close').tap();
    await expect(input).toBeHidden();
    await page.waitForTimeout(600);
    expect(await headerState(page)).toEqual(before);
  });
});
