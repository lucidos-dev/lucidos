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

/** Only the user names the home thread (ADR 0362). Its Rename dialog offers
 *  no Suggest link, and a turn on it leaves its title alone. No suffix, so
 *  desktop Chromium, phone Chromium and iPhone WebKit all run it. */
test.describe('Naming the home thread', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await gotoWithRetry(page, `/#thread=${ensureHomeThread()}`);
    await ensureOnThreadPane(page);
    await waitForThreadTitle(page);
  });

  test('a turn keeps the title, and Rename offers no Suggest link', async ({ page }) => {
    await sendMessage(page, `Say exactly: "${uniqueMessage('home-naming')}"`);
    await waitForResponse(page);

    expect(await getVisibleTitleText(page)).toBe('Home');

    expect(await clickVisibleElement(page, '.thread-title-menu')).toBe(true);
    const menu = page.locator('.thread-overflow-menu');
    await menu.getByRole('menuitem', { name: 'Rename…', exact: true }).click();
    const dialog = page.locator('.confirm-dialog');
    await expect(dialog.locator('.prompt-input')).toHaveValue('Home');
    await expect(dialog.locator('[data-role="prompt-suggest"]')).toHaveCount(0);
    await dialog.locator('[data-role="prompt-cancel"]').click();
  });
});
