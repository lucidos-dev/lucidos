import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, ensureOnThreadPane, waitForVisibleInput, toggleFocusedPin, expectFocusedPinned } from './helpers';

test.describe('Thread pin', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('save a thread from the prompt action button', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('save-test');
    await sendMessage(page, `Say exactly: "saved ${msg}"`);
    await waitForResponse(page);

    await expectFocusedPinned(page, false);
    await toggleFocusedPin(page, 'pinned');
    await expectFocusedPinned(page, true, 5_000);
  });

  test('saved state persists after page reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('save-reload');
    await sendMessage(page, `Say exactly: "persist-save ${msg}"`);
    await waitForResponse(page);

    // The pin shows before its request lands, and a reload aborts a request
    // still in flight. Wait for the engine to take it.
    const saved = page.waitForResponse((r) => r.url().endsWith('/api/v1/threads/save') && r.ok());
    await toggleFocusedPin(page, 'pinned');
    await expectFocusedPinned(page, true, 5_000);
    await saved;

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    await expectFocusedPinned(page, true);
  });

  test('unsave a thread (with confirm)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('unsave-test');
    await sendMessage(page, `Say exactly: "unsave ${msg}"`);
    await waitForResponse(page);

    await toggleFocusedPin(page, 'pinned');
    await expectFocusedPinned(page, true, 5_000);

    await toggleFocusedPin(page, 'unpinned');
    await expect(page.locator('.confirm-dialog')).toBeVisible({ timeout: 5_000 });
    await page.locator('[data-role="confirm-ok"]').click();

    await expectFocusedPinned(page, false, 5_000);
  });
});
