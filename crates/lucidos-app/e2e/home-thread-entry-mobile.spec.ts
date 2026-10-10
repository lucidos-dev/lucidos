import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, ensureMobileView } from './helpers';
import { ensureHomeThread } from './db-helpers';

/** A phone reaches the home thread from a Home icon paired with the mark in the
 *  thread pane's header. The thread drawer's header has no Home icon, so the
 *  menu its mark opens keeps a Home row; the thread pane's menu drops it. */
test.describe('The home thread from the phone header', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A spec before this one may have truncated the threads, home included.
    ensureHomeThread();
    await navigateToApp(page);
    await ensureMobileView(page, 'thread');
  });

  test('Home and the mark centre together, and Home opens it', async ({ page }) => {
    const home = page.locator('.mobile-thread-header .header-mark-pair .home-thread-btn');
    await expect(home).toBeVisible();

    const offset = await page.evaluate(() => {
      const row = document.querySelector('.mobile-thread-header .mobile-header-row')!.getBoundingClientRect();
      const pair = document.querySelector('.mobile-thread-header .header-mark-pair')!.getBoundingClientRect();
      return (pair.left + pair.right) / 2 - (row.left + row.right) / 2;
    });
    expect(Math.abs(offset), `the pair sits ${offset}px off the row middle`).toBeLessThan(2.5);

    await home.click();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });

  test('only the thread drawer\'s menu carries a Home row', async ({ page }) => {
    const homeRow = page.locator('.brand-menu .home-thread-btn');

    await page.locator('.mobile-thread-header [data-role="brand-menu-toggle"]').click();
    await expect(page.locator('.brand-menu')).toBeVisible();
    await expect(homeRow).toHaveCount(0);
    await page.keyboard.press('Escape');
    await expect(page.locator('.brand-menu')).toBeHidden();

    await ensureMobileView(page, 'threads');
    await page.locator('.mobile-threads-header [data-role="brand-menu-toggle"]').click();
    const first = page.locator('.brand-menu [role="menuitem"]').first();
    await expect(first).toHaveText('Home');
    await first.click();
    await expect(page.locator('.brand-menu')).toBeHidden();
    await expect(page.locator('.thread-title-menu:visible')).toHaveText('Home');
  });
});
