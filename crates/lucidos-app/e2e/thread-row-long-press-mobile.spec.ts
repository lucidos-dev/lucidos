import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer } from './helpers';

/** The mobile drawer row's actions, reached by holding the row.
 *
 * The ⋯ is not rendered here. A 31x27px trigger against the pane's right edge
 * is the hardest place on a phone to hit. So the whole row is the target
 * instead (`useRowActionsGesture`).
 *
 * Driven through `page.mouse` rather than dispatched events, so the gesture
 * goes through real hit-testing and the browser pairs its own click with the
 * lift. Both halves matter. The row has to be reachable where it is drawn. And
 * a fired hold has to swallow that click, or the thread opens too. Playwright
 * cannot hold a touchscreen tap, and `useLongPress` reads no `pointerType`, so
 * a real mouse press exercises the same path. */
test.describe('Mobile thread row long press', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  /** The first ON-SCREEN drawer row that is not the focused one, as a box.
   *
   *  A box with size is not enough here. Mobile lays every pane out at once and
   *  translates the off-screen ones aside, so a row from another pane still
   *  measures. `page.mouse` aims at a point, so the row has to be where the
   *  browser will actually hit-test it. Asking `elementFromPoint` settles both
   *  halves at once. */
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

  /** Two threads, the drawer open, the second one focused. */
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
    // The pin stays, so the row still reads as actionable.
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
    // The hold swallows its own paired click, so the row's tap never runs.
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
    // Past useLongPress's 10px tolerance, then held for longer than the timer
    // would have needed. A cancelled hold must stay cancelled.
    await page.mouse.move(row.x, row.y - 60, { steps: 6 });
    await page.waitForTimeout(700);
    await page.mouse.up();

    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);
  });
});
