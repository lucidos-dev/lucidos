import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, newThread, openThreadDrawer } from './helpers';

test.describe('Overlay dismiss and swallow (synthetic clicks)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  const openRowMenu = async (page: import('@playwright/test').Page) => {
    const opened = await page.evaluate(async () => {
      const visible = (el: Element) => {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      };
      const trigger = [...document.querySelectorAll('.thread-drawer button[aria-label="More thread actions"]')].find(visible);
      if (trigger) { (trigger as HTMLElement).click(); return 'trigger'; }

      const row = [...document.querySelectorAll('.thread-drawer .thread-row')].find(visible);
      if (!row) return 'none';
      const box = row.getBoundingClientRect();
      const at = (type: string) => row.dispatchEvent(new PointerEvent(type, {
        bubbles: true,
        cancelable: true,
        button: 0,
        isPrimary: true,
        clientX: box.left + box.width / 2,
        clientY: box.top + box.height / 2,
      }));
      at('pointerdown');
      await new Promise(resolve => setTimeout(resolve, 600));
      at('pointerup');
      return 'hold';
    });
    expect(opened, 'no drawer row to open a menu from').not.toBe('none');
    await expect(page.locator('.thread-overflow-menu')).toHaveCount(1);
  };

  test('clicking a thread row while a menu is open closes it and does NOT focus the row', async ({ page }) => {
    await navigateToApp(page);

    const msg1 = uniqueMessage('overlay-dismiss-1');
    await sendMessage(page, `say "${msg1}"`);
    await waitForResponse(page);

    await newThread(page);
    const msg2 = uniqueMessage('overlay-dismiss-2');
    await sendMessage(page, `say "${msg2}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    const initiallyFocused = await page.evaluate(() => {
      const focused = document.querySelector('.thread-row-focused');
      return focused?.getAttribute('data-thread-nav') ?? null;
    });
    expect(initiallyFocused).not.toBeNull();

    await openRowMenu(page);

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

    await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);

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

    const clicked = await page.evaluate(() => {
      const buttons = document.querySelectorAll('.thread-drawer button[aria-label="Pin thread"]');
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

    await expect(page.locator('.section-label', { hasText: 'Pinned' }))
      .toHaveCount(0, { timeout: 1_000 });
  });
});
