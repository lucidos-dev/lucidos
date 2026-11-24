import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy } from './helpers';

test.describe('Composer attach menu (desktop)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('File opens the file chooser and closes the menu', async ({ page }) => {
    await navigateToApp(page);

    await page.locator('.image-attach-anchor button[aria-label="Attach image"]').click();
    const menu = page.locator('.image-attach-menu');
    await expect(menu).toHaveCount(1);

    const [chooser] = await Promise.all([
      page.waitForEvent('filechooser'),
      menu.locator('button', { hasText: 'File' }).click(),
    ]);

    expect(chooser.isMultiple()).toBe(true);
    await expect(menu).toHaveCount(0);
  });
});
