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
