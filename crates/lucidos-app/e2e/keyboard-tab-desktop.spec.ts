import { test, expect, type Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, openThreadDrawer, openDrawerView, openSettingsView, waitForEventStream } from './helpers';
import { psql, clearAllThreads, seedThreadRow } from './db-helpers';

/** Where DOM focus sits, named so a failure says where Tab went. */
async function focusPlace(page: Page): Promise<string> {
  return await page.evaluate(() => {
    const a = document.activeElement as HTMLElement | null;
    if (!a || a === document.body) return 'body';
    if (a.matches('.thread-drawer')) return 'drawer';
    if (a.closest('.scale-modal')) return 'scale-modal';
    if (a.closest('.thread-drawer')) return `drawer:${a.textContent?.trim()}`;
    if (a.closest('.pane-content')) return 'content';
    if (a.closest('.pane-thread')) return 'thread';
    return a.tagName.toLowerCase();
  });
}

// Keyboard Tab across the shell. Desktop-only: mobile navigates panes and has
// no Tab trap (the mobile projects skip `-desktop.spec.ts`). The audit behind
// these is docs/plans/2026-09-30-keyboard-tab-audit-fixes.md.
test.describe('Keyboard Tab', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('Tab reaches the thread list from "See all statuses" in a status view', async ({ page }) => {
    // The drawer is its own single tab stop. The trap never counted it, so in
    // a status view Tab bounced on the link and the list was unreachable.
    clearAllThreads();
    psql(seedThreadRow({
      id: randomUUID(), title: `attention-${Date.now()}`, now: new Date().toISOString(),
      status: 'failed', archiveState: 'inbox',
    }));
    await navigateToApp(page);
    await waitForEventStream(page);
    await openThreadDrawer(page);
    await openDrawerView(page, 'Needs attention');

    // Picking the view focused the drawer pane, so ⌘⇧1 would close it now.
    await page.locator('.thread-drawer:not(.thread-drawer-collapsed)').focus();
    await expect.poll(() => focusPlace(page)).toBe('drawer');
    await page.keyboard.press('Tab');
    expect(await focusPlace(page)).toBe('drawer:See all statuses');
    await page.keyboard.press('Tab');
    expect(await focusPlace(page)).toBe('drawer');
  });

  test('a dialog keeps Tab inside it and hands focus back on close', async ({ page }) => {
    // The UI scale dialog had no trap of its own. Focus stayed on the button
    // that opened it, and Tab walked the Settings page underneath.
    await navigateToApp(page);
    await waitForEventStream(page); // the navigate arrives over the stream
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
    // A free-text dropdown opens its menu on focus. Tab left it open, and a
    // Settings page tabbed through collected a stack of floating menus.
    await navigateToApp(page);
    await waitForEventStream(page);
    await openSettingsView(page, 'models');
    const input = page.locator('.pane-content .dropdown-input[placeholder="e.g. 500"]');
    const menus = page.locator('.dropdown-menu:visible');
    // The Models page settles its controls as their reads land. A control
    // swapped in mid-test would reopen its menu on its own, so start settled.
    await expect(input).toBeVisible();
    await expect(page.locator('.pane-content .dropdown-skeleton')).toHaveCount(0);
    await input.click();
    await expect(menus).toHaveCount(1);

    // Backward, onto the row's explainer icon: forward could land on the next
    // free-text field, which opens a menu of its own on focus.
    await page.keyboard.press('Shift+Tab');
    await expect(menus).toHaveCount(0);
    expect(await focusPlace(page)).toBe('content');
  });
});
