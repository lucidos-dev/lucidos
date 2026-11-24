import { test, expect, type Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, openThreadDrawer, showOngoingGroup, openSettingsView, waitForEventStream } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

async function focusPlace(page: Page): Promise<string> {
  return await page.evaluate(() => {
    const a = document.activeElement as HTMLElement | null;
    if (!a || a === document.body) return 'body';
    if (a.matches('.thread-drawer-tree')) return 'drawer';
    if (a.closest('.scale-modal')) return 'scale-modal';
    if (a.closest('.thread-drawer')) return `drawer:${a.textContent?.trim()}`;
    if (a.closest('.pane-content')) return 'content';
    if (a.closest('.pane-thread')) return 'thread';
    return a.tagName.toLowerCase();
  });
}

test.describe('Keyboard Tab', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the thread list is the drawer pane\'s one tab stop, and Tab keeps it there', async ({ page }) => {
    clearAllThreads();
    psql(seedThreadRow({
      id: randomUUID(), title: `attention-${Date.now()}`, now: new Date().toISOString(),
      status: 'failed', archiveState: 'inbox',
    }));
    await navigateToApp(page);
    await waitForEventStream(page);
    await openThreadDrawer(page);
    await showOngoingGroup(page, 'Blocked');

    await page.locator('.thread-drawer:not(.thread-drawer-collapsed) .thread-drawer-list > .thread-drawer-tree').focus();
    await expect.poll(() => focusPlace(page)).toBe('drawer');
    await page.keyboard.press('Tab');
    expect(await focusPlace(page)).toBe('drawer');
    await page.keyboard.press('Shift+Tab');
    expect(await focusPlace(page)).toBe('drawer');
  });

  test('a dialog keeps Tab inside it and hands focus back on close', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    await openSettingsView(page, 'appearance');
    const opener = page.locator('[data-search-anchor="appearance:ui-scale"] button.settings-option');
    await opener.click();
    await expect(page.locator('.scale-modal')).toBeVisible();

    await expect.poll(() => focusPlace(page)).toBe('scale-modal');
    for (let i = 0; i < 3; i++) {
      await page.keyboard.press('Tab');
      expect(await focusPlace(page), `after Tab ${i + 1}`).toBe('scale-modal');
    }
    await page.keyboard.press('Shift+Tab');
    expect(await focusPlace(page)).toBe('scale-modal');

    await page.keyboard.press('Escape');
    await expect(page.locator('.scale-modal')).toBeHidden();
    await expect(opener).toBeFocused();
  });

  test('Tab away from a free-text dropdown closes its menu', async ({ page }) => {
    await navigateToApp(page);
    await waitForEventStream(page);
    await openSettingsView(page, 'models');
    const input = page.locator('.pane-content .dropdown-input[placeholder="e.g. 500"]');
    const menus = page.locator('.dropdown-menu:visible');
    await expect(input).toBeVisible();
    await expect(page.locator('.pane-content .dropdown-skeleton')).toHaveCount(0);
    await input.click();
    await expect(menus).toHaveCount(1);

    await page.keyboard.press('Shift+Tab');
    await expect(menus).toHaveCount(0);
    expect(await focusPlace(page)).toBe('content');
  });
});
