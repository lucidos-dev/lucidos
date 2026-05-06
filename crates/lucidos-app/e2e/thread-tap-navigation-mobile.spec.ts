/**
 * Mobile thread row tap navigates to the thread pane.
 *
 * Regression: tapping a thread row in the mobile threads pane was focusing the
 * thread but leaving the user on the threads list — they had to manually swipe
 * to see the thread they just selected. Root cause: a cursor=-1, stack=[] edge
 * case in pushThreadEntry threw a TypeError that aborted focusThread before
 * its navigateToPane('thread') call.
 *
 * Setup: a single-message scenario doesn't reliably reproduce — the tap
 * doesn't always invoke focusThread in that state. Two messages leave
 * focusedThreadId on B and a different unfocused row to tap, matching what
 * real users hit when revisiting prior threads.
 */
import { test, expect } from '@playwright/test';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  uniqueMessage,
  waitForResponse,
  newThread,
  openThreadDrawer,
  REAL_THREAD_NAV,
} from './helpers';

test.describe('Mobile thread tap navigation', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('tapping a thread row navigates to the thread pane', async ({ page }) => {
    await navigateToApp(page);

    const msgA = uniqueMessage('tap-A');
    await sendMessage(page, `Say exactly: "alpha ${msgA}"`);
    await waitForResponse(page);

    await newThread(page);
    const msgB = uniqueMessage('tap-B');
    await sendMessage(page, `Say exactly: "bravo ${msgB}"`);
    await waitForResponse(page);

    await openThreadDrawer(page);
    await expect(page.locator('.app-header').first()).toHaveAttribute('data-mobile-view', 'threads');

    // Tap (not click) the row — emulate a real touch event so the swipe
    // container's touchstart/touchend handlers fire alongside the row's
    // onClick, just like on a real device.
    const row = page.locator(`${REAL_THREAD_NAV}:visible`).first();
    await row.tap();

    // Bug: focus changed but mobileView stayed on 'threads' because focusThread
    // threw inside pushThreadNavState before reaching navigateToPane('thread').
    await expect(page.locator('.app-header').first()).toHaveAttribute('data-mobile-view', 'thread', { timeout: 5_000 });
  });
});
