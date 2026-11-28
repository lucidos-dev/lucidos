import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, openThreadDrawer, assertHealthy } from './helpers';

test.describe('Thread overflow menu alignment', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('the overflow menu pins to the control that opened it', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('overflow-align');
    await sendMessage(page, `say "${msg}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    const anchor = await page.evaluate(async () => {
      const visible = (el: Element) => {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      };
      const onScreen = (el: Element) => {
        const r = el.getBoundingClientRect();
        return visible(el) && r.left >= 0 && r.right <= window.innerWidth;
      };
      const findTrigger = () =>
        [...document.querySelectorAll('.thread-drawer button[aria-label="More thread actions"]')].find(visible);
      const findRow = () => [...document.querySelectorAll('.thread-drawer .thread-row')].find(onScreen);
      for (let i = 0; i < 100 && !findTrigger() && !findRow(); i++) {
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      const trigger = findTrigger();
      if (trigger) {
        const rect = trigger.getBoundingClientRect();
        (trigger as HTMLElement).click();
        return { mode: 'end' as const, edge: rect.right };
      }
      const row = findRow();
      if (!row) return null;
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
      return { mode: 'start' as const, edge: box.left };
    });
    expect(anchor, 'no drawer row to open a menu from').not.toBeNull();

    await page.waitForFunction(() => {
      const el = document.querySelector('.thread-overflow-menu');
      if (!el) return false;
      const r = el.getBoundingClientRect();
      return r.width > 0 && getComputedStyle(el).visibility === 'visible';
    }, undefined, { timeout: 5_000 });

    const menu = await page.evaluate(() => {
      const el = document.querySelector('.thread-overflow-menu');
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, width: r.width, viewport: window.innerWidth };
    });
    expect(menu).not.toBeNull();

    const CLAMP_MARGIN = 8;
    const pinned = anchor!.mode === 'end' ? menu!.right : menu!.left;
    const owed = anchor!.mode === 'end'
      ? Math.min(anchor!.edge, menu!.viewport - CLAMP_MARGIN)
      : Math.max(anchor!.edge, CLAMP_MARGIN);
    expect(Math.abs(pinned - owed)).toBeLessThanOrEqual(5);

    expect(menu!.width).toBeLessThan(menu!.viewport * 0.9);
  });
});
