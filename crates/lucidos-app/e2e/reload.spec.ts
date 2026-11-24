import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, openThreadDrawer, waitForVisibleInput, ensureOnThreadPane, userMessageBody, USER_MSG_SELECTOR, focusedThreadId, clickThreadRow } from './helpers';
import { clearAllThreads } from './db-helpers';

test.describe('Page reload preserves state', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('messages persist after reload and re-selecting thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('reload-state');
    await sendMessage(page, `Say exactly: "reload ${msg}"`);
    await waitForResponse(page);

    const threadId = await focusedThreadId(page);

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    await openThreadDrawer(page);
    await clickThreadRow(page, threadId);
    await ensureOnThreadPane(page);
    await expect(userMessageBody(page)).toBeVisible({ timeout: 10_000 });

    await expect(userMessageBody(page)).toContainText(msg, { timeout: 10_000 });

    const response = page.locator('.response-content:visible').first();
    await expect(response).toBeVisible({ timeout: 10_000 });
    const responseText = await response.textContent();
    expect(responseText!.trim().length).toBeGreaterThan(0);

    await openThreadDrawer(page);
    const focusedRow = page.locator('.thread-row-focused:visible').first();
    await expect(focusedRow).toBeVisible({ timeout: 10_000 });
  });

  test('chat input is usable after reload', async ({ page }) => {
    await navigateToApp(page);

    await page.reload();
    await ensureOnThreadPane(page);
    const input = await waitForVisibleInput(page);

    await expect(input).toBeVisible();
    await input.fill('test message');
    const value = await input.inputValue();
    expect(value).toBe('test message');
  });

  test('prompt input visible in focused thread after reload', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('prompt-reload');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);

    const threadId = await focusedThreadId(page);

    await page.reload();
    await ensureOnThreadPane(page);
    await waitForVisibleInput(page);

    const hasMessages = await page.evaluate((sel) => {
      return Array.from(document.querySelectorAll(sel)).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, USER_MSG_SELECTOR);
    if (!hasMessages) {
      await openThreadDrawer(page);
      await clickThreadRow(page, threadId);
    }

    const input = await waitForVisibleInput(page);
    await input.fill('follow-up after reload');
    const value = await input.inputValue();
    expect(value).toBe('follow-up after reload');

    await expect(userMessageBody(page)).toContainText(msg, { timeout: 15_000 });
  });
});
