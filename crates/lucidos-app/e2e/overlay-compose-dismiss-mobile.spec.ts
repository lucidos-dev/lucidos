import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, disableMobileDynamicBars, ensureMobileView } from './helpers';

test.describe('Overlay swallows a touch-driven sibling action', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await disableMobileDynamicBars(page);
  });

  test('tapping Search threads while the Lucidos menu is open dismisses it without opening search', async ({ page }) => {
    await navigateToApp(page);
    await ensureMobileView(page, 'threads');

    const mark = page.locator('.mobile-threads-header [data-role="brand-menu-toggle"]').first();
    const menu = page.locator('.brand-menu');
    const searchBtn = page.locator('.mobile-threads-header button[aria-label="Search threads"]').first();

    await mark.tap();
    await expect(menu).toBeVisible();

    await searchBtn.tap({ force: true });

    await expect(menu).toHaveCount(0);

    const searchFocused = await page.evaluate(() => {
      const active = document.activeElement as HTMLElement | null;
      return !!active?.closest('.mobile-thread-search-bar');
    });
    expect(searchFocused).toBe(false);
  });
});
