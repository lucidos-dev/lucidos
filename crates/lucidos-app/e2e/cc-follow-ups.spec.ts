import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, switchToClaudeMode, newThread, countExchanges,
  waitForActionPanel, waitForCCToFinish, waitForExchangeCount,
  waitForCCToStart, assertUserMessagesVisible,
} from './helpers';

test.describe('Claude Code follow-ups during and after working', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('send follow-up while CC is still working', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    const msg1 = uniqueMessage('cc-working-1');
    await sendMessage(page, `Say exactly: "response ${msg1}" and nothing else. Do not create any files.`);

    // Wait for CC to start responding (not just optimistic exchange render)
    await waitForCCToStart(page, 60_000);

    // Send follow-up (CC may be working or just finished — both are valid)
    const msg2 = uniqueMessage('cc-working-2');
    await sendFollowUp(page, `Say exactly: "follow-up ${msg2}" and nothing else. Do not create any files.`);

    // Wait for both exchanges and CC to finish
    await waitForExchangeCount(page, 2, 120_000);
    await waitForCCToFinish(page, 120_000);

    await assertUserMessagesVisible(page, [msg1, msg2]);
  });

  test('send multiple follow-ups while CC is working', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    const msg1 = uniqueMessage('cc-multi-1');
    await sendMessage(page, `Say exactly: "response ${msg1}" and nothing else. Do not create any files.`);

    // Wait for CC to start responding (not just optimistic exchange render)
    await waitForCCToStart(page, 60_000);

    // Send two follow-ups (wait between them to avoid debounce)
    const msg2 = uniqueMessage('cc-multi-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}". Do not create any files.`);

    await waitForExchangeCount(page, 2, 120_000);

    const msg3 = uniqueMessage('cc-multi-3');
    await sendFollowUp(page, `Say exactly: "third ${msg3}". Do not create any files.`);

    await waitForExchangeCount(page, 3, 180_000);
    await waitForCCToFinish(page, 180_000);

    await assertUserMessagesVisible(page, [msg1, msg2, msg3]);
  });

  test('send follow-up after CC finishes and goes idle', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    const msg1 = uniqueMessage('cc-after-idle-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Done', 120_000);

    // CC is now idle — send follow-up
    const msg2 = uniqueMessage('cc-after-idle-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    await waitForCCToFinish(page, 120_000);

    await assertUserMessagesVisible(page, [msg1, msg2]);

    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(2);
  });

  test('multiple sequential follow-ups after CC idle', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await switchToClaudeMode(page);

    const msg1 = uniqueMessage('cc-seq-1');
    await sendMessage(page, `Say exactly: "one ${msg1}". Do not create any files.`);
    await waitForActionPanel(page, 'Done', 120_000);

    const msg2 = uniqueMessage('cc-seq-2');
    await sendFollowUp(page, `Say exactly: "two ${msg2}". Do not create any files.`);
    // Wait for 2nd exchange to confirm msg2 was processed
    await waitForExchangeCount(page, 2, 120_000);
    await waitForCCToFinish(page, 120_000);

    const msg3 = uniqueMessage('cc-seq-3');
    await sendFollowUp(page, `Say exactly: "three ${msg3}". Do not create any files.`);
    await waitForExchangeCount(page, 3, 120_000);
    await waitForCCToFinish(page, 120_000);

    await assertUserMessagesVisible(page, [msg1, msg2, msg3]);

    const exchanges = await countExchanges(page);
    expect(exchanges).toBeGreaterThanOrEqual(3);
  });
});
