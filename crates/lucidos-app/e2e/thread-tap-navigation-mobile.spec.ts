import { test, expect } from './fixtures';
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

    const row = page.locator(`${REAL_THREAD_NAV}:visible`).first();
    await row.tap();

    await expect(page.locator('.app-header').first()).toHaveAttribute('data-mobile-view', 'thread', { timeout: 5_000 });
  });
});
