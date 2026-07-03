import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, openThreadDrawer, assertHealthy } from './helpers';

/** Regression guard for the thread overflow (⋯) menu's horizontal placement.
 *
 *  The menu is portaled to <body> and right-aligned to its ⋯ trigger
 *  (`align: 'end'` in ThreadOverflowMenu → its right edge pins under the
 *  trigger's right edge). useAnchoredPosition measures the panel's width via
 *  `offsetWidth` while it is still a plain block child of <body> (position is
 *  only applied once the offset is computed) — without `width: max-content` that
 *  measurement came back ~viewport-wide, poisoning `rect.right - panelWidth` to a
 *  negative value that the clamp stranded at the left margin. The bug only showed
 *  on mobile, where the full-width drawer row puts the ⋯ at the far right (on
 *  desktop the narrow left drawer pane masked it). A pure unit test can't catch
 *  this — it depends on real layout giving the wrong measured width — so this
 *  asserts the rendered geometry, and runs on the mobile-webkit project. */
test.describe('Thread overflow menu alignment', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test("the overflow menu's right edge pins under its ⋯ trigger", async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('overflow-align');
    await sendMessage(page, `say "${msg}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);

    // Open the menu from the first visible drawer-row ⋯ button, recording that
    // trigger's right edge so we can compare it to the menu once it lands.
    const triggerRight = await page.evaluate(() => {
      const buttons = document.querySelectorAll('button[aria-label="More thread actions"]');
      for (const btn of buttons) {
        const rect = btn.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          (btn as HTMLElement).click();
          return rect.right;
        }
      }
      return null;
    });
    expect(triggerRight).not.toBeNull();

    // Wait for the menu to be both visible AND positioned (it renders
    // `visibility: hidden` for the one measurement frame before `pos` is set).
    await page.waitForFunction(() => {
      const el = document.querySelector('.thread-overflow-menu');
      if (!el) return false;
      const r = el.getBoundingClientRect();
      return r.width > 0 && getComputedStyle(el).visibility === 'visible';
    }, undefined, { timeout: 5_000 });

    const menuRight = await page.evaluate(() => {
      const el = document.querySelector('.thread-overflow-menu');
      return el ? el.getBoundingClientRect().right : null;
    });
    expect(menuRight).not.toBeNull();

    // Right edge of the menu must sit under the right edge of the ⋯ trigger.
    // Pre-fix on mobile the menu was clamped to the ~8px left margin — tens of px
    // adrift from the trigger; the tolerance absorbs only sub-pixel /
    // offsetWidth-vs-getBoundingClientRect rounding.
    expect(Math.abs((menuRight as number) - (triggerRight as number))).toBeLessThanOrEqual(5);
  });
});
