import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, ensureOnThreadPane, waitForVisibleInput } from './helpers';

// Scope to the focused thread's header actions. The drawer rows now carry their
// own pin button (extraClass="thread-row-action"), and on mobile the drawer pane
// is always mounted (off-screen but CSS-:visible), so an unscoped selector's
// .first()/count() would resolve to an off-screen drawer-row pin instead of the
// focused thread's. `.thread-view-header-actions` is the focused thread's header
// (ThreadView on desktop, MobileThreadTitleBar on mobile — the inactive variant
// is display:none), never a drawer row.
const SAVE_BTN = '.thread-view-header-actions button[aria-label="Pin thread"]:visible';
const UNSAVE_BTN = '.thread-view-header-actions button[aria-label="Remove thread from Pinned section"]:visible';

test.describe('Thread pin', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('save a thread from the prompt action button', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('save-test');
    await sendMessage(page, `Say exactly: "saved ${msg}"`);
    await waitForResponse(page);

    const saveBtn = page.locator(SAVE_BTN).first();
    await expect(saveBtn).toBeVisible({ timeout: 10_000 });
    await saveBtn.click();

    const savedBtn = page.locator(UNSAVE_BTN).first();
    await expect(savedBtn).toBeVisible({ timeout: 5_000 });
  });

  test('saved state persists after page reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('save-reload');
    await sendMessage(page, `Say exactly: "persist-save ${msg}"`);
    await waitForResponse(page);

    await page.locator(SAVE_BTN).first().click();
    await expect(page.locator(UNSAVE_BTN).first()).toBeVisible({ timeout: 5_000 });

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    await expect(page.locator(UNSAVE_BTN).first()).toBeVisible({ timeout: 10_000 });
  });

  test('unsave a thread (with confirm)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('unsave-test');
    await sendMessage(page, `Say exactly: "unsave ${msg}"`);
    await waitForResponse(page);

    await page.locator(SAVE_BTN).first().click();
    const unsaveBtn = page.locator(UNSAVE_BTN).first();
    await expect(unsaveBtn).toBeVisible({ timeout: 5_000 });

    await unsaveBtn.click();
    await expect(page.locator('.confirm-dialog')).toBeVisible({ timeout: 5_000 });
    await page.locator('.confirm-btn-ok').click();

    await expect(page.locator(SAVE_BTN).first()).toBeVisible({ timeout: 5_000 });
  });
});
