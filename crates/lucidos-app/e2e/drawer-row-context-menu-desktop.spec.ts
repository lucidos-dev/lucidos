import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer } from './helpers';

/** A right-click on a desktop drawer row opens that row's ⋯ menu (ADR 0285).
 *
 * Driven through `page.mouse`, so the press goes through real hit-testing and
 * the browser dispatches its own `contextmenu`. The menu must be the ⋯ menu
 * itself, open at the pointer, and must never focus the row it came from. */
test.describe('Desktop drawer row context menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  const focusedId = (page: Page) => page.evaluate(() =>
    document.querySelector('.thread-drawer .thread-row-focused')?.getAttribute('data-thread-nav') ?? null);

  /** The first on-screen drawer row that is not the focused one. */
  const otherRow = async (page: Page) => {
    const box = await page.evaluate(() => {
      const focusedNav = document.querySelector('.thread-drawer .thread-row-focused')?.getAttribute('data-thread-nav');
      for (const row of document.querySelectorAll('.thread-drawer .thread-row')) {
        const id = row.getAttribute('data-thread-nav');
        const r = row.getBoundingClientRect();
        if (!id || id === focusedNav || r.width === 0) continue;
        // Left of centre, clear of the chips and the ⋯ on the right.
        const x = r.left + r.width * 0.3;
        const y = r.top + r.height / 2;
        const at = document.elementFromPoint(x, y);
        if (at && row.contains(at) && !at.closest('button')) return { id, x, y };
      }
      return null;
    });
    expect(box, 'no unfocused drawer row was reachable on screen').not.toBeNull();
    return box!;
  };

  const menuItems = (page: Page) =>
    page.locator('.thread-overflow-menu [role="menuitem"]').allTextContents();

  /** Two threads, the drawer open, the second one focused. */
  const twoThreads = async (page: Page) => {
    await navigateToApp(page);
    await sendMessage(page, `say "${uniqueMessage('ctx-menu-1')}"`);
    await waitForResponse(page);
    await newThread(page);
    await sendMessage(page, `say "${uniqueMessage('ctx-menu-2')}"`);
    await waitForResponse(page);
    await openThreadDrawer(page);
  };

  test('a right-click opens the same menu as ⋯, at the pointer, without opening the thread', async ({ page }) => {
    await twoThreads(page);
    const before = await focusedId(page);
    const row = await otherRow(page);

    await page.mouse.click(row.x, row.y, { button: 'right' });
    const menu = page.locator('.thread-overflow-menu');
    await expect(menu).toHaveCount(1);
    await expect(menu).toBeVisible();
    const box = (await menu.boundingBox())!;
    expect(Math.abs(box.x - row.x)).toBeLessThan(2);
    expect(box.y).toBeGreaterThanOrEqual(row.y);
    expect(await focusedId(page)).toBe(before);
    const fromRightClick = await menuItems(page);

    await page.keyboard.press('Escape');
    await expect(menu).toHaveCount(0);

    // The ⋯ is still drawn: the context menu is a shortcut, never the only way.
    const rowEl = page.locator(`.thread-drawer .thread-row[data-thread-nav="${row.id}"]`);
    await rowEl.hover();
    await rowEl.locator('button[aria-label="More thread actions"]').click();
    await expect(menu).toBeVisible();
    expect(await menuItems(page)).toEqual(fromRightClick);
  });

  test('dismissing the menu by clicking another row does not open that row', async ({ page }) => {
    await twoThreads(page);
    const before = await focusedId(page);
    const row = await otherRow(page);

    await page.mouse.click(row.x, row.y, { button: 'right' });
    await expect(page.locator('.thread-overflow-menu')).toBeVisible();

    await page.mouse.click(row.x, row.y);
    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);
    expect(await focusedId(page)).toBe(before);
  });
});
