/**
 * Mobile: a focused toggle switch never holds off a pane swipe.
 *
 * A tap on a toggle switch focuses its hidden checkbox. Only a field that
 * opens the keyboard may block a swipe, so a swipe from that state still moves
 * the pane. The spec focuses a real switch on the Appearance page without
 * flipping it, so it changes no setting.
 */
import { test, expect, Page } from './fixtures';
import { assertHealthy, ensureMobileView, navigateToApp, openSettingsView } from './helpers';

/** A left-to-right swipe across the visible content pane, as a touch sequence.
 *  Generic `Event`s carry the touch lists, because WebKit refuses the
 *  Touch and TouchEvent constructors. */
async function swipeRightOnContent(page: Page): Promise<void> {
  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.content-pane-body')).find(
      (e) => e.getBoundingClientRect().width > 0,
    );
    if (!el) throw new Error('no visible content pane');
    const r = el.getBoundingClientRect();
    const y = r.top + r.height / 2;
    const x0 = r.left + r.width * 0.2;
    const mk = (type: string, x: number) => {
      const ev = new Event(type, { bubbles: true, cancelable: true, composed: true });
      const touch = { identifier: 1, target: el, clientX: x, clientY: y, pageX: x, pageY: y };
      const list = type === 'touchend' ? [] : [touch];
      Object.defineProperty(ev, 'touches', { value: list });
      Object.defineProperty(ev, 'targetTouches', { value: list });
      Object.defineProperty(ev, 'changedTouches', { value: [touch] });
      return ev;
    };
    el.dispatchEvent(mk('touchstart', x0));
    for (let dx = 20; dx <= r.width * 0.7; dx += 20) el.dispatchEvent(mk('touchmove', x0 + dx));
    el.dispatchEvent(mk('touchend', x0 + r.width * 0.7));
  });
}

test.describe('Mobile pane swipe with a toggle switch focused', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('swipes from the content pane to the thread pane', async ({ page }) => {
    await navigateToApp(page);
    await openSettingsView(page, 'appearance');
    await ensureMobileView(page, 'content');

    const focused = await page.waitForFunction(() => {
      const input = Array.from(document.querySelectorAll<HTMLInputElement>('.toggle-switch input[type="checkbox"]'))
        .find((e) => e.closest('label')!.getBoundingClientRect().width > 0);
      if (!input) return null;
      input.focus();
      return document.activeElement === input ? input.type : null;
    }, undefined, { timeout: 10_000 });
    expect(await focused.jsonValue()).toBe('checkbox');

    await swipeRightOnContent(page);

    await expect(page.locator('.app-header[data-mobile-view="thread"]').first()).toBeAttached();
  });
});
