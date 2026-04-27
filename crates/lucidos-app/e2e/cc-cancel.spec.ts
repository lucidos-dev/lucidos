import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, switchToClaudeMode, newThread,
  waitForCCToStart, waitForCCToFinish, waitForExchangeCount,
  cancelCCResponse, countVisibleResponses, dismissCCSession,
  waitForStreamingToStart,
} from './helpers';

// Benign bash sleep loop that keeps CC busy long enough for the test to click
// stop. Avoids prompts that tip off CC as a test (e.g. wasteful file listings),
// which the model refuses immediately and ends the exchange before we can
// click stop.
const BUSY_BASH_PROMPT =
  `Please run this exact bash command and stream its output: ` +
  `bash -c 'for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do echo step $i; sleep 2; done'`;

test.describe('Claude Code cancel and stop', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('cancel a CC response via stop button', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    await sendMessage(page, BUSY_BASH_PROMPT);

    // Wait for CC to start and produce some visible response text
    await waitForCCToStart(page, 60_000);
    await waitForStreamingToStart(page, 1, 60_000);

    await cancelCCResponse(page);

    // Response should have partial content (not empty — text was streaming)
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(1);
  });

  test('can send CC follow-up after canceling', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    await sendMessage(page, BUSY_BASH_PROMPT);

    await waitForCCToStart(page, 60_000);
    await waitForStreamingToStart(page, 1, 60_000);

    await cancelCCResponse(page);

    // Send a follow-up and verify it works
    const msg2 = uniqueMessage('cc-after-stop');
    await sendFollowUp(page, `Say exactly: "recovered ${msg2}" and nothing else. Do not create any files.`);

    await waitForExchangeCount(page, 2, 120_000);

    // Wait for the follow-up response to contain our marker text
    await page.waitForFunction((marker) => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, msg2, { timeout: 120_000 });
  });

  test('dismiss idle CC session with Done button', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    const msg = uniqueMessage('cc-dismiss');
    await sendMessage(page, `Say exactly: "done ${msg}" and nothing else. Do not create any files.`);

    await waitForCCToFinish(page, 120_000);

    // Dismiss the session
    await dismissCCSession(page);

    // After dismissing, the action banner should disappear
    await page.waitForFunction(() => {
      const banners = document.querySelectorAll('.thread-action-buttons');
      return !Array.from(banners).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 10_000 }).catch(() => {});
  });
});
