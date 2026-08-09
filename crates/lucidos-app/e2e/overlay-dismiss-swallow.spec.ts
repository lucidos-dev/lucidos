import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer } from './helpers';

/** The `<Overlay>` dismiss-and-swallow contract, driven through SYNTHETIC clicks
 *  (`element.click()`), which is the branch that has no preceding outside
 *  pointerdown and therefore falls to the click-capture fallback in
 *  `useDismissOnOutside` (see `.claude/rules/frontend.md` § Modals & Popovers,
 *  point 4). Every real overlay depends on that fallback, but nothing else
 *  exercises it, so this file is its canary.
 *
 *  The drawer row's overflow (⋯) menu is the overlay under test. It used to be
 *  the thread filter, until the filter became a panel INSIDE the thread drawer
 *  pane (`ThreadFilterPanel`) and stopped being an overlay at all. */
test.describe('Overlay dismiss and swallow (synthetic clicks)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  const openRowMenu = async (page: import('@playwright/test').Page) => {
    const opened = await page.evaluate(() => {
      const buttons = document.querySelectorAll('button[aria-label="More thread actions"]');
      for (const btn of buttons) {
        const rect = btn.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          (btn as HTMLElement).click();
          return true;
        }
      }
      return false;
    });
    expect(opened, 'a drawer row overflow button was visible').toBe(true);
    await expect(page.locator('.thread-overflow-menu')).toHaveCount(1);
  };

  test('clicking a thread row while a menu is open closes it and does NOT focus the row', async ({ page }) => {
    await navigateToApp(page);

    // Need at least two threads so we can click one that isn't currently focused.
    const msg1 = uniqueMessage('overlay-dismiss-1');
    await sendMessage(page, `say "${msg1}"`);
    await waitForResponse(page);

    await newThread(page);
    const msg2 = uniqueMessage('overlay-dismiss-2');
    await sendMessage(page, `say "${msg2}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    // After sending the second message we expect the second thread to be focused.
    const initiallyFocused = await page.evaluate(() => {
      const focused = document.querySelector('.thread-row-focused');
      return focused?.getAttribute('data-thread-nav') ?? null;
    });
    expect(initiallyFocused).not.toBeNull();

    await openRowMenu(page);

    // Click a thread row that is NOT the currently focused one.
    const clickedId = await page.evaluate((focusedId) => {
      const rows = document.querySelectorAll('.thread-row');
      for (const row of rows) {
        const id = row.getAttribute('data-thread-nav');
        const rect = row.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0 && id && id !== focusedId) {
          (row as HTMLElement).click();
          return id;
        }
      }
      return null;
    }, initiallyFocused);
    expect(clickedId).not.toBeNull();
    expect(clickedId).not.toBe(initiallyFocused);

    // The menu must close.
    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);

    // The focused thread must NOT have changed: the click was consumed by the
    // dismiss.
    const stillFocused = await page.evaluate(() => {
      const focused = document.querySelector('.thread-row-focused');
      return focused?.getAttribute('data-thread-nav') ?? null;
    });
    expect(stillFocused).toBe(initiallyFocused);
  });

  test('clicking a row action button while a menu is open closes it and does NOT activate it', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('overlay-dismiss-pin');
    await sendMessage(page, `say "${msg}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);
    await openRowMenu(page);

    // Click the row's Pin button behind the open menu. Per the dismiss-and-swallow
    // contract this must dismiss the menu and NOT pin the thread.
    const clicked = await page.evaluate(() => {
      const buttons = document.querySelectorAll('button[aria-label="Pin thread"]');
      for (const btn of buttons) {
        const rect = btn.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          (btn as HTMLElement).click();
          return true;
        }
      }
      return false;
    });
    expect(clicked).toBe(true);

    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);

    // The thread must NOT have been pinned: no Pinned section appears. Polled,
    // so a row that lands there late still fails the assertion.
    await expect(page.locator('.drawer-section-label', { hasText: 'Pinned' }))
      .toHaveCount(0, { timeout: 1_000 });
  });
});
