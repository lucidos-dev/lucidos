/**
 * A tap inside the left edge strip opens the thread row beneath it.
 *
 * Reported as a first tap that does nothing, with a second tap that works.
 * `.edge-swipe-left` is a transparent strip over the pane's left 2.5rem. A tap
 * there lands on the strip, and `gateStripTouch` cancels its touchstart to
 * suppress the iOS back swipe. The cancel also drops the click, so the row
 * never hears the tap. `replayEdgeTap` gives the click back.
 *
 * Tapped at a point the strip covers, never at the row's centre: Playwright's
 * `tap()` drives the centre, which sits clear of the strip and passes with or
 * without the fix.
 */
import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
  newThread,
  openThreadDrawer,
  focusedThreadId,
} from './helpers';

const EDGE_TAP_X = 8;

test.describe('Edge tap reaches its row', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a tap at the left edge opens the thread row under the strip', async ({ page }) => {
    await navigateToApp(page);

    await sendMessage(page, `Say exactly: "alpha ${uniqueMessage('edge-A')}"`);
    await waitForResponse(page);
    const first = await focusedThreadId(page);

    await newThread(page);
    await sendMessage(page, `Say exactly: "bravo ${uniqueMessage('edge-B')}"`);
    await waitForResponse(page);
    expect(await focusedThreadId(page), 'the second thread must be the focused one').not.toBe(first);

    await openThreadDrawer(page);
    const row = page.locator(`[data-thread-nav="${first}"]:visible`);
    await expect(row).toBeVisible();
    const box = (await row.boundingBox())!;
    const y = box.y + box.height / 2;

    // The precondition the bug needs: the strip, not the row, is hit here.
    const hit = await page.evaluate(([x, py]) => {
      const el = document.elementFromPoint(x, py);
      return el?.classList.contains('edge-swipe-zone') ? 'strip' : (el?.tagName ?? 'nothing');
    }, [EDGE_TAP_X, y]);
    expect(hit, 'the edge tap must land on the strip').toBe('strip');

    await page.touchscreen.tap(EDGE_TAP_X, y);

    await expect(page.locator('.app-header').first()).toHaveAttribute('data-mobile-view', 'thread', { timeout: 5_000 });
    expect(await focusedThreadId(page)).toBe(first);
  });
});
