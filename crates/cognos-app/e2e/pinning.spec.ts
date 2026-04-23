import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForVisibleInput } from './helpers';

test.describe('Thread pinning', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('pin a thread and verify pin indicator', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('pin-test');
    await sendMessage(page, `Say exactly: "pinned ${msg}"`);
    await waitForResponse(page);

    // Open drawer and find the thread
    await openThreadDrawer(page);
    const threadNav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadNav).toBeVisible({ timeout: 15_000 });

    // Click the pin button
    const pinBtn = threadNav.locator('button[aria-label="Pin thread"]');
    await pinBtn.click();

    // After pinning, the button label should change to "Unpin thread"
    const unpinBtn = threadNav.locator('button[aria-label="Unpin thread"]');
    await expect(unpinBtn).toBeVisible({ timeout: 5_000 });
  });

  test('pinned thread persists after page reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('pin-reload');
    await sendMessage(page, `Say exactly: "persist-pin ${msg}"`);
    await waitForResponse(page);

    // Open drawer, find thread, pin it
    await openThreadDrawer(page);
    const threadNav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadNav).toBeVisible({ timeout: 15_000 });
    const threadId = await threadNav.getAttribute('data-thread-nav');

    await threadNav.locator('button[aria-label="Pin thread"]').click();
    await expect(threadNav.locator('button[aria-label="Unpin thread"]')).toBeVisible({ timeout: 5_000 });

    // Reload the page
    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    // Open drawer and verify it's still pinned
    await openThreadDrawer(page);

    // Wait for thread rows to load
    await expect(page.locator('[data-thread-nav]:visible').first()).toBeVisible({ timeout: 15_000 });

    const reloadedThread = page.locator(`[data-thread-nav="${threadId}"]:visible`).first();
    await expect(reloadedThread).toBeVisible({ timeout: 10_000 });
    await expect(reloadedThread.locator('button[aria-label="Unpin thread"]')).toBeVisible({ timeout: 5_000 });
  });

  test('unpin a thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('unpin-test');
    await sendMessage(page, `Say exactly: "unpin ${msg}"`);
    await waitForResponse(page);

    // Open drawer, pin it, then unpin it
    await openThreadDrawer(page);
    const threadNav = page.locator('[data-thread-nav]:visible').first();
    await expect(threadNav).toBeVisible({ timeout: 15_000 });

    await threadNav.locator('button[aria-label="Pin thread"]').click();
    await expect(threadNav.locator('button[aria-label="Unpin thread"]')).toBeVisible({ timeout: 5_000 });

    await threadNav.locator('button[aria-label="Unpin thread"]').click();
    await expect(threadNav.locator('button[aria-label="Pin thread"]')).toBeVisible({ timeout: 5_000 });
  });
});
