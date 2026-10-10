import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, expectFocusedPinned } from './helpers';

/** The phone's thread title is its own menu button.
 *
 * The title row draws no pin and no ⋯. A tap on the title toggles the thread
 * menu, and a hold opens it. The menu opens under the row, aligned to the
 * title's leading edge, away from the top-right corner.
 *
 * Driven through `page.mouse`, so the gesture goes through real hit-testing and
 * the browser pairs its own click with the lift. Playwright cannot hold a
 * touchscreen tap, and the hold reads no `pointerType`. */
test.describe('Mobile thread title menu', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await navigateToApp(page);
    await sendMessage(page, `say "${uniqueMessage('title-menu')}"`);
    await waitForResponse(page);
  });

  const TITLE = '.mobile-thread-title-row .thread-title-menu:visible';
  const MENU = '.thread-overflow-menu';

  /** The title's centre, checked to be what the browser hit-tests there. */
  const titlePoint = async (page: Page) => {
    const point = await page.evaluate(() => {
      for (const el of document.querySelectorAll('.mobile-thread-title-row .thread-title-menu')) {
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.left < 0 || r.right > window.innerWidth) continue;
        const x = r.left + r.width / 2;
        const y = r.top + r.height / 2;
        const at = document.elementFromPoint(x, y);
        if (at && el.contains(at)) return { x, y };
      }
      return null;
    });
    expect(point, 'the title was not reachable on screen').not.toBeNull();
    return point!;
  };

  test('the row draws no pin and no ⋯', async ({ page }) => {
    await expect(page.locator(TITLE)).toHaveCount(1);
    await expect(page.locator('.mobile-thread-title-row .pin-thread-btn:visible')).toHaveCount(0);
    await expect(page.locator('.mobile-thread-title-row button[aria-label="More thread actions"]:visible')).toHaveCount(0);
  });

  test('a tap opens the menu under the title, aligned left, and a second tap closes it', async ({ page }) => {
    const at = await titlePoint(page);
    await page.mouse.click(at.x, at.y);
    await expect(page.locator(MENU)).toHaveCount(1);

    const boxes = await page.evaluate((menu) => {
      const title = [...document.querySelectorAll('.mobile-thread-title-row .thread-title-menu')]
        .find((el) => el.getBoundingClientRect().width > 0)!.getBoundingClientRect();
      const panel = document.querySelector(menu)!.getBoundingClientRect();
      return { titleLeft: title.left, titleBottom: title.bottom, panelLeft: panel.left, panelTop: panel.top };
    }, MENU);
    expect(Math.abs(boxes.panelLeft - boxes.titleLeft)).toBeLessThanOrEqual(2);
    expect(boxes.panelTop).toBeGreaterThanOrEqual(boxes.titleBottom - 1);

    await page.mouse.click(at.x, at.y);
    await expect(page.locator(MENU)).toHaveCount(0);
  });

  test('a hold opens the menu, and its lift does not close it', async ({ page }) => {
    const at = await titlePoint(page);
    await page.mouse.move(at.x, at.y);
    await page.mouse.down();
    await page.waitForTimeout(700);
    await page.mouse.up();
    await expect(page.locator(MENU)).toHaveCount(1);
  });

  test('a tap outside dismisses the menu', async ({ page }) => {
    const at = await titlePoint(page);
    await page.mouse.click(at.x, at.y);
    await expect(page.locator(MENU)).toHaveCount(1);
    // Below the panel, over the transcript.
    const below = await page.evaluate((menu) => document.querySelector(menu)!.getBoundingClientRect().bottom + 24, MENU);
    await page.mouse.click(page.viewportSize()!.width / 2, below);
    await expect(page.locator(MENU)).toHaveCount(0);
  });

  test('the keyboard reaches Pin thread through the title', async ({ page }) => {
    await page.locator(TITLE).focus();
    await page.keyboard.press('Enter');
    await expect(page.locator(MENU)).toHaveCount(1);
    const pin = page.locator(MENU).getByRole('menuitem', { name: 'Pin thread', exact: true });
    await expect(pin).toBeVisible();
    // A keyboard open focuses the first item. Rove until Pin thread has focus.
    for (let i = 0; i < 4; i++) {
      if (await pin.evaluate((el) => el === document.activeElement)) break;
      await page.keyboard.press('ArrowDown');
    }
    await expect(pin).toBeFocused();
    await page.keyboard.press('Enter');
    await expectFocusedPinned(page, true);
  });
});
