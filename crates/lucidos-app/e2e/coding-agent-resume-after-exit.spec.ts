import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, pickComposeDestination, newThread,
  waitForActionPanel, waitForCCToFinish, waitForCCToStart,
  countVisibleResponses, getLatestVisibleResponseText,
} from './helpers';
import { clearAllThreads } from './db-helpers';

test.describe('CC resume after process exit', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('a follow-up on an idle CC thread produces a non-empty response', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg1 = uniqueMessage('cc-resume-exit-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    const firstResponseCount = await countVisibleResponses(page);
    expect(firstResponseCount).toBeGreaterThanOrEqual(1);

    await page.waitForTimeout(2_000);

    const msg2 = uniqueMessage('cc-resume-exit-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    await waitForCCToStart(page, 120_000);

    await waitForCCToFinish(page, 120_000);

    await expect
      .poll(() => countVisibleResponses(page), { timeout: 30_000 })
      .toBeGreaterThanOrEqual(2);

    const found = await page.evaluate((marker) => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, msg2);
    expect(found).toBe(true);
  });

  test('revival preserves conversation content (codeword round-trip)', { tag: '@real-claude-code' }, async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await sendMessage(
      page,
      'Remember the codeword: pineapple. Just acknowledge with one word. Do not create any files.',
    );
    await waitForCCToStart(page, 60_000);
    await waitForCCToFinish(page, 120_000);

    const firstResponseCount = await countVisibleResponses(page);
    expect(firstResponseCount).toBeGreaterThanOrEqual(1);

    await sendFollowUp(
      page,
      'What was the codeword I asked you to remember? Reply with just the word.',
    );

    await waitForCCToStart(page, 120_000);
    await waitForCCToFinish(page, 120_000);

    await expect(async () => {
      const latest = await getLatestVisibleResponseText(page);
      expect(latest.toLowerCase()).toContain('pineapple');
    }).toPass({ timeout: 30_000, intervals: [500, 1000, 2000] });
  });

  test('status label shows Requesting during CC follow-up', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg1 = uniqueMessage('cc-status-label-1');
    await sendMessage(page, `Say exactly: "first ${msg1}" and nothing else. Do not create any files.`);
    await waitForActionPanel(page, 'Archive', 120_000);

    await page.waitForTimeout(2_000);

    const msg2 = uniqueMessage('cc-status-label-2');
    await sendFollowUp(page, `Say exactly: "second ${msg2}" and nothing else. Do not create any files.`);

    const hasStatusLabel = await page.waitForFunction(() => {
      const labels = document.querySelectorAll('.exchange-status-label');
      return Array.from(labels).some(el => {
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return false;
        const text = el.textContent ?? '';
        return text.includes('Requesting') || text.includes('Working');
      });
    }, undefined, { timeout: 30_000 }).then(() => true).catch(() => false);

    expect(hasStatusLabel).toBe(true);

    await waitForCCToFinish(page, 120_000);
  });
});
