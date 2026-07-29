import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse,
  uniqueMessage, assertHealthy, countExchanges, newThread,
  waitForStreamingToStart, assertUserMessagesVisible, countVisibleResponses,
  waitForVisibleResponseCount,
} from './helpers';

test.describe('Follow-ups during and after streaming', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send follow-up while response is still streaming', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('stream-followup-1');
    await sendMessage(page, `Write a very long and detailed essay about the number ${msg1.slice(-6)}. Be extremely verbose, use many paragraphs.`);

    await waitForStreamingToStart(page, 10);

    const msg2 = uniqueMessage('stream-followup-2');
    await sendFollowUp(page, `Say exactly: "interrupted ${msg2}"`);

    await waitForResponse(page, 120_000);

    await assertUserMessagesVisible(page, [msg1.slice(-6), msg2]);

    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(2);
  });

  test('send multiple follow-ups in rapid succession while streaming', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('rapid-1');
    await sendMessage(page, `Write a very long essay about rivers. Be extremely verbose. Include: ${msg1}`);

    await waitForStreamingToStart(page);

    // Rapid-fire two more messages without waiting
    const msg2 = uniqueMessage('rapid-2');
    const msg3 = uniqueMessage('rapid-3');
    await sendFollowUp(page, `Say exactly: "second ${msg2}"`);
    await sendFollowUp(page, `Say exactly: "third ${msg3}"`);

    await waitForResponse(page, 120_000);

    await assertUserMessagesVisible(page, [msg1, msg2, msg3]);

    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(3);
  });

  test('send follow-up after response is fully generated', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('after-done-1');
    await sendMessage(page, `Say exactly: "done ${msg1}"`);
    await waitForResponse(page);

    const msg2 = uniqueMessage('after-done-2');
    await sendFollowUp(page, `Say exactly: "followup ${msg2}"`);
    await waitForResponse(page);

    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(2);

    // The waitForResponse() above can resolve before the follow-up turn streams
    // (turn 1's label is already settled), so wait for the end-state first.
    await waitForVisibleResponseCount(page, 2);
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(2);
  });

  test('double follow-up near response completion shows no error toast', async ({ page }) => {
    // Regression: sending follow-ups right as a response finishes caused a 409
    // "Thread is not active" error toast because the inject API races with the
    // thread leaving active_threads. The fix falls back to sendMessage on 409.
    await navigateToApp(page);
    await newThread(page);

    // Short prompt → response finishes fast, maximising the race window
    const msg1 = uniqueMessage('race-1');
    await sendMessage(page, `Say exactly: "done ${msg1}"`);

    // Don't wait for streaming — fire follow-ups immediately so at least one
    // lands right as (or just after) the response completes.
    const msg2 = uniqueMessage('race-2');
    const msg3 = uniqueMessage('race-3');
    await sendFollowUp(page, `Say exactly: "second ${msg2}"`);
    await sendFollowUp(page, `Say exactly: "third ${msg3}"`);

    // Wait for all exchanges to complete
    await waitForResponse(page, 120_000);

    // All three user messages must be visible
    await assertUserMessagesVisible(page, [msg1, msg2, msg3]);

    // No error toast should have appeared (the old bug showed
    // "Failed to inject prompt: 409 Thread is not active")
    const errorToasts = page.locator('.toast-error');
    await expect(errorToasts).toHaveCount(0);

    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(3);
  });

  test('multiple follow-ups after each response completes', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const markers: string[] = [];
    for (let i = 1; i <= 3; i++) {
      const msg = uniqueMessage(`multi-done-${i}`);
      markers.push(msg);

      if (i === 1) {
        await sendMessage(page, `Say exactly: "reply-${i} ${msg}"`);
      } else {
        await sendFollowUp(page, `Say exactly: "reply-${i} ${msg}"`);
      }
      await waitForResponse(page);
    }

    await assertUserMessagesVisible(page, markers);

    // The final waitForResponse() in the loop can resolve before the third turn
    // streams (the prior turn's label is already settled), so wait for the
    // end-state — three responses with content — before counting.
    await waitForVisibleResponseCount(page, 3);
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(3);
  });
});
