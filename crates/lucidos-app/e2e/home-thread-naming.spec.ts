import { test, expect } from './fixtures';
import {
  assertHealthy,
  clickVisibleElement,
  ensureOnThreadPane,
  getVisibleTitleText,
  gotoWithRetry,
  sendMessage,
  uniqueMessage,
  waitForResponse,
  waitForThreadTitle,
} from './helpers';
import { ensureHomeThread } from './db-helpers';

/** Only the user names the home thread (ADR 0362). Its menu offers Rename…
 *  but not Suggest name, and a turn on it raises no suggested name. No suffix,
 *  so desktop Chromium, phone Chromium and iPhone WebKit all run it. */
test.describe('Naming the home thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await gotoWithRetry(page, `/#thread=${ensureHomeThread()}`);
    await ensureOnThreadPane(page);
    await waitForThreadTitle(page);
  });

  test('a turn raises no suggested name, and the menu offers Rename… alone', async ({ page }) => {
    await sendMessage(page, `Say exactly: "${uniqueMessage('home-naming')}"`);
    await waitForResponse(page);

    await expect(page.locator('.toast-container').getByText('Suggested name')).toHaveCount(0);
    expect(await getVisibleTitleText(page)).toBe('Home');

    expect(await clickVisibleElement(page, '.thread-title-menu')).toBe(true);
    const menu = page.locator('.thread-overflow-menu');
    await expect(menu.getByRole('menuitem', { name: 'Rename…', exact: true })).toBeVisible();
    await expect(menu.getByRole('menuitem', { name: 'Suggest name', exact: true })).toHaveCount(0);
  });
});
