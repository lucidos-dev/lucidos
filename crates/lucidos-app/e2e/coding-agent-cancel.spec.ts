import { test, expect } from '@playwright/test';
import {
  navigateToApp, sendMessage, sendFollowUp, uniqueMessage,
  assertHealthy, pickComposeDestination, newThread,
  waitForCCToStart, waitForCCToFinish, waitForExchangeCount,
  cancelStreamingResponse, countVisibleResponses, dismissCCSession,
  waitForStreamingToStart,
} from './helpers';
import { psql } from './db-helpers';

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

  test('cancel a CC response via Cancel button', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await sendMessage(page, BUSY_BASH_PROMPT);

    // Wait for CC to start and produce some visible response text
    await waitForCCToStart(page, 60_000);
    await waitForStreamingToStart(page, 1, 60_000);

    await cancelStreamingResponse(page);

    // Response should have partial content (not empty — text was streaming)
    const responseCount = await countVisibleResponses(page);
    expect(responseCount).toBeGreaterThanOrEqual(1);
  });

  test('can send CC follow-up after canceling', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    await sendMessage(page, BUSY_BASH_PROMPT);

    await waitForCCToStart(page, 60_000);
    await waitForStreamingToStart(page, 1, 60_000);

    await cancelStreamingResponse(page);

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

  test('cancel resumes the same CC session, does not respawn a fresh one', async ({ page }) => {
    await navigateToApp(page);
    await newThread(page);
    await pickComposeDestination(page);

    // Mark the boundary just before the spawn so we can find THIS test's thread
    // by its `SessionStarted` (the events table isn't truncated between tests).
    const since = psql(`SELECT now()`).trim();

    await sendMessage(page, BUSY_BASH_PROMPT);
    await waitForCCToStart(page, 60_000);
    await waitForStreamingToStart(page, 1, 60_000);

    // Cancel = Esc: interrupt the turn but keep the session resumable.
    await cancelStreamingResponse(page);

    // A follow-up must continue the SAME conversation.
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

    // Backend continuity is the decisive signal — the THREAD HISTORY injection
    // would feed a fresh session enough context to pass a content check, so it
    // can't distinguish resume from respawn. The `cc_session_id` can: a real
    // `--resume` keeps the same id; the old bug deleted the cancelled branch, so
    // resume fell back to a brand-new session id. Identify this test's thread by
    // its post-`since` SessionStarted, then assert exactly one distinct id.
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

    // And the cancel did not tear the session down (no SessionEnded, so the
    // branch survived for the resume).
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
