import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, ensureOnThreadPane, waitForVisibleInput } from './helpers';

test.describe('Thread save (formerly pin)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('save a thread from the prompt action button', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('save-test');
    await sendMessage(page, `Say exactly: "saved ${msg}"`);
    await waitForResponse(page);

    const saveBtn = page.locator('button[aria-label="Save thread"]:visible').first();
    await expect(saveBtn).toBeVisible({ timeout: 10_000 });
    await saveBtn.click();

    const savedBtn = page.locator('button[aria-label="Unsave thread"]:visible').first();
    await expect(savedBtn).toBeVisible({ timeout: 5_000 });
  });

  test('saved state persists after page reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('save-reload');
    await sendMessage(page, `Say exactly: "persist-save ${msg}"`);
    await waitForResponse(page);

    await page.locator('button[aria-label="Save thread"]:visible').first().click();
    await expect(page.locator('button[aria-label="Unsave thread"]:visible').first()).toBeVisible({ timeout: 5_000 });

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    await expect(page.locator('button[aria-label="Unsave thread"]:visible').first()).toBeVisible({ timeout: 10_000 });
  });

  test('unsave a thread (with confirm)', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('unsave-test');
    await sendMessage(page, `Say exactly: "unsave ${msg}"`);
    await waitForResponse(page);

    await page.locator('button[aria-label="Save thread"]:visible').first().click();
    const unsaveBtn = page.locator('button[aria-label="Unsave thread"]:visible').first();
    await expect(unsaveBtn).toBeVisible({ timeout: 5_000 });

    await unsaveBtn.click();
    await expect(page.locator('.confirm-dialog')).toBeVisible({ timeout: 5_000 });
    await page.locator('.confirm-btn-ok').click();

    await expect(page.locator('button[aria-label="Save thread"]:visible').first()).toBeVisible({ timeout: 5_000 });
  });
});
