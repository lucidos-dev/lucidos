import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, disableMobileDynamicBars, ensureMobileView } from './helpers';

test.describe('Lucidos mark menu (touch)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disableMobileDynamicBars(page);
  });

  test('tapping the mark again closes its menu', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');

    const mark = page.locator('[data-role="brand-menu-toggle"]:visible').first();
    const menu = page.locator('.brand-menu');

    await mark.tap();
    await expect(menu).toBeVisible();

    await mark.tap();
    await expect(menu).toHaveCount(0);
  });

  test('Search everywhere opens from the menu', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');

    const mark = page.locator('[data-role="brand-menu-toggle"]:visible').first();
    const input = page.locator('.search-everywhere-input');

    await mark.tap();
    await page.locator('.brand-menu-item', { hasText: 'Search everywhere' }).tap();

    await expect(input).toBeVisible();
    await expect(page.locator('.brand-menu')).toHaveCount(0);
  });
});
