import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Answering a question survives a dropped connection.
 *
 * Reported from an iOS PWA: a multi-select answer failed to send twice with
 * "Failed to send answer: unknown error". The third attempt a minute later
 * landed, and each failure emptied the card, so three options were re-picked
 * twice over. That is the half-closed HTTP/2 connection a backgrounded PWA
 * wakes up holding, which WebKit rejects before the request leaves the device.
 * `route.abort('failed')` reproduces the same rejection shape, the transport
 * `TypeError` that `isTransportError` matches.
 *
 * Two halves, one per test: the client retries the POST once, and when that
 * cannot save it either the user keeps their answer.
 *
 * Service workers are blocked, following `sdk-iframe-mount.spec.ts`. A
 * controlled page runs every fetch through the worker's own network session in
 * WebKit. `page.route` cannot see it there, so both tests passed vacuously on
 * `mobile-webkit` with the abort never firing. Blocking costs nothing here:
 * `sw.js` intercepts GETs only and has never handled this POST.
 */

test.use({ serviceWorkers: 'block' });

const ANSWER_ROUTE = '**/answer-question';

/** Seed a CC thread parked on a question, the way the sibling question specs
 *  do. Returns the ids the test drives and cleans up with. */
function seedQuestion(opts: { title: string; question: string; multiSelect: boolean; labels: string[] }) {
  const suffix = randomUUID().slice(0, 8);
  const threadId = randomUUID();
  const toolUseId = `tu-stale-${suffix}`;
  const now = new Date().toISOString();

  const payload = JSON.stringify({
    tool_use_id: toolUseId,
    cc_session_id: 'sess-stale-e2e',
    question: `${opts.question} ${suffix}`,
    options: opts.labels.map((label, i) => ({ id: `opt-${i}`, label: `${label} ${suffix}` })),
    ...(opts.multiSelect ? { multi_select: true } : {}),
  }).replace(/'/g, "''");

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${opts.title} ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-stale-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));

  return { suffix, threadId, toolUseId, rowTitle: `${opts.title} ${suffix}` };
}

function cleanup(threadId: string): void {
  psql([
    `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
    `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
  ].join(';\n'));
}

test.describe('CC AskUserQuestion over a dropped connection', () => {
  test('an answer whose first POST never leaves the device still lands', async ({ page }) => {
    await assertHealthy(page);
    const seed = seedQuestion({
      title: 'CC Stale Answer E2E',
      question: 'Stale connection',
      multiSelect: false,
      labels: ['Retry', 'Other'],
    });

    try {
      let attempts = 0;
      await page.route(ANSWER_ROUTE, async (route) => {
        attempts += 1;
        if (attempts === 1) await route.abort('failed');
        else await route.continue();
      });

      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`.thread-row:has-text("${seed.rowTitle}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const pendingBody = page
        .locator(`.question-body[data-tool-use-id="${seed.toolUseId}"]:visible`)
        .first();
      await expect(pendingBody).toBeVisible({ timeout: 10_000 });

      // ONE tap. The retry is the client's, not the user's.
      await pendingBody.locator('.question-option').nth(0).click();

      await expect.poll(
        () => psql(`SELECT payload->'answer'->>'option_id' FROM events WHERE thread_id = '${seed.threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${seed.toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('opt-0');
      expect(attempts).toBe(2);
      // The retry is silent: the answer landed, so the user is told nothing.
      await expect(page.locator('.toast-error')).toHaveCount(0);
    } finally {
      await page.unroute(ANSWER_ROUTE);
      cleanup(seed.threadId);
    }
  });

  // When the retry cannot save it either, the user keeps their answer. The
  // submit clears the toggles as its send gesture, and the reported failure
  // left the card blank.
  test('a multi-select answer that cannot be sent gives the picks back', async ({ page }) => {
    await assertHealthy(page);
    const seed = seedQuestion({
      title: 'CC Keep Picks E2E',
      question: 'Keep picks',
      multiSelect: true,
      labels: ['One', 'Two', 'Three'],
    });

    try {
      // Both the tap and the client's own retry fail, which is the state the
      // user was actually in.
      await page.route(ANSWER_ROUTE, (route) => route.abort('failed'));

      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`.thread-row:has-text("${seed.rowTitle}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const pendingBody = page
        .locator(`.question-body[data-tool-use-id="${seed.toolUseId}"]:visible`)
        .first();
      await expect(pendingBody).toBeVisible({ timeout: 10_000 });

      await pendingBody.locator('.question-option').nth(0).click();
      await pendingBody.locator('.question-option').nth(2).click();
      await expect(pendingBody.locator('.question-option[aria-pressed="true"]')).toHaveCount(2);

      const submit = page.locator('.prompt-actions-row:visible button[aria-label="Submit answer"]').first();
      await expect(submit).toBeEnabled();
      await submit.click();

      // The card comes back live, still holding both picks, so the retry is one
      // tap rather than a re-pick.
      await expect(pendingBody.locator('.question-option[aria-pressed="true"]'))
        .toHaveCount(2, { timeout: 10_000 });
      await expect(submit).toBeEnabled();

      // ONE message for one failed tap, and it names the cause. The pair the
      // user reported said "Please try again" over "unknown error".
      const errors = page.locator('.toast-error:visible');
      await expect(errors).toHaveCount(1);
      await expect(errors.first()).toContainText('the connection dropped');

      // Nothing reached the engine, so nothing was recorded.
      expect(
        psql(`SELECT COUNT(*) FROM events WHERE thread_id = '${seed.threadId}' AND event_type = 'UserQuestionAnswered'`),
      ).toBe('0');
    } finally {
      await page.unroute(ANSWER_ROUTE);
      cleanup(seed.threadId);
    }
  });
});
