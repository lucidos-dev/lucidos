import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer } from './helpers';

test.describe('Mobile thread row long press', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  const otherRow = async (page: Page) => {
    const box = await page.evaluate(() => {
      const focused = document.querySelector('.thread-drawer .thread-row-focused');
      const focusedId = focused?.getAttribute('data-thread-nav') ?? null;
      for (const row of document.querySelectorAll('.thread-drawer .thread-row')) {
        const id = row.getAttribute('data-thread-nav');
        const r = row.getBoundingClientRect();
        if (r.width === 0 || r.height === 0 || !id || id === focusedId) continue;
        const x = r.left + r.width / 2;
        const y = r.top + r.height / 2;
        if (x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight) continue;
        const at = document.elementFromPoint(x, y);
        if (at && row.contains(at)) return { id, x, y };
      }
      return null;
    });
    expect(box, 'no unfocused drawer row was reachable on screen').not.toBeNull();
    return box!;
  };

  const focusedId = (page: Page) => page.evaluate(() =>
    document.querySelector('.thread-drawer .thread-row-focused')?.getAttribute('data-thread-nav') ?? null);

  const twoThreads = async (page: Page) => {
    await navigateToApp(page);
    await sendMessage(page, `say "${uniqueMessage('long-press-1')}"`);
    await waitForResponse(page);
    await newThread(page);
    await sendMessage(page, `say "${uniqueMessage('long-press-2')}"`);
    await waitForResponse(page);
    await openThreadDrawer(page);
  };

  test('the row draws no ⋯: the hold is the way in', async ({ page }) => {
    await twoThreads(page);
    await expect(page.locator('.thread-drawer button[aria-label="More thread actions"]')).toHaveCount(0);
    await expect(page.locator('.thread-drawer button[aria-label="Pin thread"]').first()).toBeVisible();
  });

  test('holding a row opens its actions, and does not open the thread', async ({ page }) => {
    await twoThreads(page);
    const before = await focusedId(page);
    const row = await otherRow(page);

    await page.mouse.move(row.x, row.y);
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();

    await expect(page.locator('.thread-overflow-menu')).toHaveCount(1);
    expect(await focusedId(page)).toBe(before);
  });

  test('an ordinary tap still opens the thread', async ({ page }) => {
    await twoThreads(page);
    const row = await otherRow(page);

    await page.mouse.move(row.x, row.y);
    await page.mouse.down();
    await page.mouse.up();

    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);
    await expect.poll(() => focusedId(page)).toBe(row.id);
  });

  test('a drag opens nothing: that is a scroll', async ({ page }) => {
    await twoThreads(page);
    const row = await otherRow(page);

    await page.mouse.move(row.x, row.y);
    await page.mouse.down();
    await page.mouse.move(row.x, row.y - 60, { steps: 6 });
    await page.waitForTimeout(700);
    await page.mouse.up();

    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);
  });
});
