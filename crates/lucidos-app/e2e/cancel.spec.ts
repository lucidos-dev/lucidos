import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, sendFollowUp, waitForResponse, uniqueMessage,
  assertHealthy, newThread, cancelStreamingResponse, countVisibleResponses,
  waitForStreamingToStart,
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

    await waitForResponse(page, 90_000);

    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(2);
  });
});
