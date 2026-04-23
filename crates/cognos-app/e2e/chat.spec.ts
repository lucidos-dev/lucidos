import { test, expect } from '@playwright/test';
import { navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy, openThreadDrawer, userMessageBody } from './helpers';

test.describe('Chat - send and receive messages', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send a message and see a response', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('chat-basic');
    await sendMessage(page, `Say exactly: "hello ${msg}"`);

    // User message should appear in the thread (use first visible one)
    await expect(userMessageBody(page)).toContainText(msg, { timeout: 10_000 });

    // Wait for the LLM response to finish
    const response = await waitForResponse(page);
    const responseText = await response.textContent();
    expect(responseText!.trim().length).toBeGreaterThan(0);
  });

  test('thread appears in the sidebar after sending a message', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('sidebar-thread');
    await sendMessage(page, `Say exactly: "acknowledged ${msg}"`);
    await waitForResponse(page);

    // Open the thread drawer
    await openThreadDrawer(page);

    // A thread row should appear in the sidebar
    const threadRows = page.locator('.thread-row:visible');
    await expect(threadRows.first()).toBeVisible({ timeout: 15_000 });

    // The focused thread should be highlighted
    const focusedRow = page.locator('.thread-row-focused:visible').first();
    await expect(focusedRow).toBeVisible();
  });

  test('response has non-empty content', async ({ page }) => {
    await navigateToApp(page);

    await sendMessage(page, `What is 2 + 2? Reply with just the number.`);
    const response = await waitForResponse(page);

    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);
  });
});
