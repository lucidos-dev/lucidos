import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, openThreadDrawer, assertHealthy } from './helpers';

/** Regression guard for the thread overflow (⋯) menu's horizontal placement.
 *
 *  useAnchoredPosition measures the panel's width via `offsetWidth` while it is
 *  still a plain block child of <body>. Without `width: max-content` that came
 *  back ~viewport-wide, poisoning the arithmetic into a value the clamp
 *  stranded at the left margin. A pure unit test cannot catch this: it depends
 *  on real layout giving the wrong measured width. So this asserts rendered
 *  geometry.
 *
 *  The two layouts open the menu differently, and the assertion follows.
 *  Desktop keeps the drawer row's ⋯ and right-aligns the panel under it. The
 *  mobile row has no ⋯ (`useRowActionsGesture`), so a hold opens the menu and
 *  it left-aligns to the row. The pin under test there is the left edge, plus a
 *  width that is nothing like the viewport. That width is what the original bug
 *  got wrong, and it is measured the same way whichever edge is pinned. */
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

    // Open from the first visible drawer row, recording the edge the panel owes
    // itself to. Scoped to the drawer: on mobile every pane is laid out at
    // once, so a bare query answers with the conversation pane's own ⋯.
    const anchor = await page.evaluate(async () => {
      const visible = (el: Element) => {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      };
      const trigger = [...document.querySelectorAll('.thread-drawer button[aria-label="More thread actions"]')].find(visible);
      if (trigger) {
        const rect = trigger.getBoundingClientRect();
        (trigger as HTMLElement).click();
        return { mode: 'end' as const, edge: rect.right };
      }
      // On screen, not merely laid out. An off-screen pane's row has a negative
      // left, and the position clamp pins the panel to the viewport margin
      // rather than to that row. The comparison below would then fail on
      // geometry that is actually correct.
      const onScreen = (el: Element) => {
        const r = el.getBoundingClientRect();
        return visible(el) && r.left >= 0 && r.right <= window.innerWidth;
      };
      const row = [...document.querySelectorAll('.thread-drawer .thread-row')].find(onScreen);
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

    // Wait for the menu to be both visible AND positioned (it renders
    // `visibility: hidden` for the one measurement frame before `pos` is set).
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

    // The pinned edge sits on its anchor's. Pre-fix the panel was clamped to
    // the ~8px left margin, tens of px adrift; the tolerance absorbs only
    // sub-pixel and offsetWidth-vs-getBoundingClientRect rounding.
    const pinned = anchor!.mode === 'end' ? menu!.right : menu!.left;
    expect(Math.abs(pinned - anchor!.edge)).toBeLessThanOrEqual(5);

    // The measurement itself. A panel measured ~viewport-wide is the fault
    // behind the misplacement. On the left-pinned arm it is the only half the
    // edge check cannot see.
    expect(menu!.width).toBeLessThan(menu!.viewport * 0.9);
  });
});
