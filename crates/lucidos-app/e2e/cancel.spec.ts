import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, newThread, cancelStreamingResponse, countVisibleResponses,
  waitForStreamingToStart, waitForVisibleResponseCount,
} from './helpers';

test.describe('Cancel streaming response', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('cancel a streaming response via stop button', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg = uniqueMessage('cancel-stream');
    await sendMessage(page, `Write an extremely long and detailed essay about the entire history of mathematics from ancient times to modern day. Be as verbose as possible. Include: ${msg}`);

    // Wait for streaming to start so there's content before canceling
    await waitForStreamingToStart(page, 5, 60_000);

    await cancelStreamingResponse(page);

    // Response should have partial content (not empty — text was streaming)
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(1);
  });

  test('can send new message after canceling', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);

    const msg1 = uniqueMessage('cancel-then-send');
    await sendMessage(page, `Write a very long essay about space exploration. Be extremely verbose. Include: ${msg1}`);

    // Wait for streaming to start before canceling
    await waitForStreamingToStart(page, 5, 60_000);

    await cancelStreamingResponse(page);

    // Send a new message and verify it works
    const msg2 = uniqueMessage('after-cancel');
    await sendFollowUp(page, `Say exactly: "recovered ${msg2}"`);

    // After a cancel the only status label is the settled "Canceled" one, so a
    // bare waitForResponse() can return before the follow-up turn even starts
    // streaming — then the count below sees just the canceled partial and
    // fails. Wait for the end-state instead: two visible responses with content
    // (the canceled partial + the follow-up's reply).
    await waitForVisibleResponseCount(page, 2);

    // Poll rather than read once: the post-cancel turn's text can still be
    // rendering when waitForResponse returns (its "no Working/Requesting label"
    // check can pass in the brief window before the new turn's status label
    // mounts), so a single read races the second response into the DOM.
    await expect
      .poll(() => countVisibleResponses(page), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(2);
  });
});
