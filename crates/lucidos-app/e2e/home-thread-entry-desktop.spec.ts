import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer } from './helpers';
import { ensureHomeThread } from './db-helpers';

/** Desktop reaches the home thread from a Home icon in the thread pane's
 *  header, and the drawer draws no row for it (ADR 0362).
 *
 *  Its menu still offers no way to end it: no Archive, no Delete and no Pin. */
test.describe('The home thread from the desktop header', () => {
  test.use({ viewport: { width: 1280, height: 800 } });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // A spec before this one may have truncated the threads, home included.
    ensureHomeThread();
    await navigateToApp(page);
  });

  test('the drawer draws no Home row', async ({ page }) => {
    await openThreadDrawer(page);
    await expect(page.locator('.thread-drawer .thread-row-title', { hasText: /^Home$/ })).toHaveCount(0);
  });

  test('the Home icon opens it, and its menu cannot end it', async ({ page }) => {
    // A narrow thread pane folds the header's actions into ⋯ (see
    // ThreadHeaderActions); both renderings carry the action's class.
    const more = page.locator('.desktop-header .thread-header-more');
    if (await page.locator('.desktop-header .home-thread-btn').count() === 0) await more.click();
    const home = page.locator('.desktop-header .home-thread-btn, .thread-overflow-item.home-thread-btn');
    await expect(home).toHaveCount(1);
    await home.click();

    const title = page.locator('.thread-title-menu:visible');
    await expect(title).toHaveText('Home');

    await title.click();
    const menu = page.locator('.thread-overflow-menu');
    await expect(menu).toBeVisible();
    const items = (await menu.locator('[role="menuitem"]').allTextContents()).map((i) => i.trim());
    expect(items.length).toBeGreaterThan(0);
    for (const forbidden of ['Pin thread', 'Unpin thread', 'Show in Folders']) {
      expect(items).not.toContain(forbidden);
    }
    // Archive and Delete show blocked with the reason, never hidden (ADR 0378).
    const blocked = menu.locator('.thread-overflow-item-blocked[aria-disabled="true"]');
    await expect(blocked).toHaveCount(2);
    await expect(menu.locator('.thread-overflow-note')).toHaveText('The home thread always stays open.');
  });
});
