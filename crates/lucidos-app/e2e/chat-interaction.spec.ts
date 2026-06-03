import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse,
  uniqueMessage, assertHealthy, countExchanges, waitForExchangeCount, newThread,
  assertUserMessagesVisible, userMessageBody, waitForVisibleResponseCount,
} from './helpers';

test.describe('Chat interaction - multi-turn conversation', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send initial message and receive response', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('chat-init');
    await sendMessage(page, `Say exactly: "hello ${msg}"`);

    // User message should appear
    await expect(userMessageBody(page)).toContainText(msg, { timeout: 10_000 });

    // Response should complete
    const response = await waitForResponse(page);
    const text = await response.textContent();
    expect(text!.trim().length).toBeGreaterThan(0);

    // Should have exactly one visible exchange
    const exchanges = await countExchanges(page);
    expect(exchanges).toBe(1);
  });

  test('send follow-up while response is still working', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('followup-working');
    await sendMessage(page, `Write a detailed paragraph about the history of computing. Be very thorough and verbose. Include the marker: ${msg1}`);

    // Wait for the response to start streaming (working state)
    await page.waitForFunction(() => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').length > 0;
      });
    }, undefined, { timeout: 30_000 });

    // Send follow-up while first response is still working
    const msg2 = uniqueMessage('followup-interrupt');
    await sendFollowUp(page, `Say exactly: "interrupted ${msg2}"`);

    // Wait for responses to finish
    await waitForResponse(page, 120_000);

    // Should have at least two visible exchanges (original + follow-up)
    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(2);

    // The follow-up user message should be visible somewhere
    await assertUserMessagesVisible(page, [msg2]);
  });

  test('send follow-up after response is finished', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    // Send first message and wait for completion
    const msg1 = uniqueMessage('followup-done-1');
    await sendMessage(page, `Say exactly: "first ${msg1}"`);
    await waitForResponse(page);

    // Verify first response completed
    const firstResponse = page.locator('.response-content:visible').first();
    await expect(firstResponse).toBeVisible();

    // Send follow-up
    const msg2 = uniqueMessage('followup-done-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}"`);

    // Wait for second response
    await waitForResponse(page);

    // Should have at least two visible exchanges
    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(2);

    // Both user messages should be visible (helper handles dual-layout safety)
    await assertUserMessagesVisible(page, [msg1, msg2]);

    // At least two visible responses should have content. Wait for the
    // end-state — after the first turn settled, waitForResponse() above can
    // resolve before the follow-up turn starts streaming.
    await waitForVisibleResponseCount(page, 2);
    const visibleResponseCount = await page.evaluate(() => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
      }).length;
    });
    expect(visibleResponseCount).toBeGreaterThanOrEqual(2);
  });

  test('three sequential messages all appear in same thread', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('seq-1');
    const msg2 = uniqueMessage('seq-2');
    const msg3 = uniqueMessage('seq-3');

    // Send three messages sequentially, waiting for each exchange to appear
    // before checking its response (waitForResponse sees stale responses otherwise).
    await sendMessage(page, `Say exactly: "one ${msg1}"`);
    await waitForExchangeCount(page, 1);
    await waitForResponse(page);

    await sendFollowUp(page, `Say exactly: "two ${msg2}"`);
    await waitForExchangeCount(page, 2);
    await waitForResponse(page);

    await sendFollowUp(page, `Say exactly: "three ${msg3}"`);
    await waitForExchangeCount(page, 3);
    await waitForResponse(page);

    // All three user messages should be visible in the thread
    await assertUserMessagesVisible(page, [msg1, msg2, msg3]);

    // Should have at least 3 visible exchanges
    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(3);

    // All visible responses should have content. Wait for the end-state: the
    // last waitForResponse() can resolve before the third turn streams (the
    // prior turn's label is already settled).
    await waitForVisibleResponseCount(page, 3);
    const visibleResponseCount = await page.evaluate(() => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).filter(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
      }).length;
    });
    expect(visibleResponseCount).toBeGreaterThanOrEqual(3);
  });
});
