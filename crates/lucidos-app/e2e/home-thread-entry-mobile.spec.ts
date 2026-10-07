import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';
import { ensureHomeThread } from './db-helpers';

/** A phone reaches the home thread from the first row of the Lucidos menu
 *  (ADR 0362). No thread list draws it. */
test.describe('The home thread from the mobile Lucidos menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A spec before this one may have truncated the threads, home included.
    ensureHomeThread();
    await navigateToApp(page);
  });

  test('Home is the first row, and it opens the home thread', async ({ page }) => {
    await page.locator('[data-role="brand-menu-toggle"]:visible').first().click();
    const menu = page.locator('.brand-menu');
    await expect(menu).toBeVisible();

    const first = menu.locator('[role="menuitem"]').first();
    await expect(first).toHaveText('Home');
    await first.click();

    await expect(menu).toHaveCount(0);
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });
});
