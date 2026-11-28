import { test, expect } from './fixtures';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, pickComposeDestination, newThread,
  waitForCCToStart, waitForCCToFinish, waitForExchangeCount,
  cancelStreamingResponse, countVisibleResponses, dismissCCSession,
  waitForKeptOutputToStart, waitForActionPanel, USER_MSG_SELECTOR,
} from './helpers';
import { psql } from './db-helpers';

const BUSY_BASH_PROMPT =
  `Please run this exact bash command and stream its output: ` +
  `bash -c 'for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15; do echo step $i; sleep 2; done'`;

test.describe('Claude Code cancel and stop', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('cancel a CC response via Cancel button', { tag: '@real-claude-code' }, async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await sendMessage(page, BUSY_BASH_PROMPT);

    await waitForCCToStart(page, 60_000);
    await waitForKeptOutputToStart(page, 1, 60_000);

    await cancelStreamingResponse(page);

    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(1);
  });

  test('can send CC follow-up after canceling', { tag: '@real-claude-code' }, async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await sendMessage(page, BUSY_BASH_PROMPT);

    await waitForCCToStart(page, 60_000);
    await waitForKeptOutputToStart(page, 1, 60_000);

    await cancelStreamingResponse(page);

    const msg2 = uniqueMessage('cc-after-stop');
    await sendFollowUp(page, `Say exactly: "recovered ${msg2}" and nothing else. Do not create any files.`);

    await waitForExchangeCount(page, 2, 120_000);

    await page.waitForFunction((marker) => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, msg2, { timeout: 120_000 });
  });

  test('cancel resumes the same CC session, does not respawn a fresh one', { tag: '@real-claude-code' }, async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const since = psql(`SELECT now()`).trim();

    await sendMessage(page, BUSY_BASH_PROMPT);
    await waitForCCToStart(page, 60_000);
    await waitForKeptOutputToStart(page, 1, 60_000);

    await cancelStreamingResponse(page);

    const msg2 = uniqueMessage('cc-resume-after-cancel');
    await sendFollowUp(page, `Say exactly: "recovered ${msg2}" and nothing else. Do not create any files.`);
    await waitForExchangeCount(page, 2, 120_000);
    await page.waitForFunction((marker) => {
      const els = document.querySelectorAll('.response-content');
      return Array.from(els).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, msg2, { timeout: 120_000 });

    const threadId = psql(
      `SELECT aggregate_id FROM events WHERE event_type='SessionStarted' ` +
      `AND created > '${since}'::timestamptz ORDER BY sequence DESC LIMIT 1`,
    ).trim();
    expect(threadId).toMatch(/^[0-9a-f-]{36}$/);

    const distinctSids = psql(
      `SELECT COUNT(DISTINCT payload->>'cc_session_id') FROM events ` +
      `WHERE aggregate_id='${threadId}' ` +
      `AND event_type IN ('CodingAgentSettingsChanged','CodingAgentIdled') ` +
      `AND COALESCE(payload->>'cc_session_id','') <> ''`,
    ).trim();
    expect(distinctSids).toBe('1');

    const sessionEnded = psql(
      `SELECT COUNT(*) FROM events WHERE aggregate_id='${threadId}' AND event_type='SessionEnded'`,
    ).trim();
    expect(sessionEnded).toBe('0');
  });

  test('dismiss idle CC session with Archive button', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    const msg = uniqueMessage('cc-dismiss');
    await sendMessage(page, `Say exactly: "done ${msg}" and nothing else. Do not create any files.`);

    await waitForActionPanel(page, 'Archive', 120_000);
    await waitForCCToFinish(page, 120_000);

    const threadId = await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'));
    expect(threadId).toMatch(/^[0-9a-f-]{36}$/);

    await dismissCCSession(page);

    await page.waitForFunction(({ marker, sel }) => {
      return !Array.from(document.querySelectorAll(sel)).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(marker);
      });
    }, { marker: msg, sel: USER_MSG_SELECTOR }, { timeout: 10_000 });

    await expect.poll(
      () => psql(
        `SELECT COUNT(*) FROM events WHERE aggregate_id = '${threadId}' ` +
        `AND event_type = 'ThreadArchived'`,
      ).trim(),
      { intervals: [400], timeout: 15_000 },
    ).toBe('1');
  });
});
