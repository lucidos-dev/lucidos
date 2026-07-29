import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import {
  assertHealthy, ensureOnThreadPane, navigateToApp, openThreadDrawer,
  REAL_THREAD_ROW, USER_MSG_SELECTOR, waitForVisibleInput,
} from './helpers';
import { psql } from './db-helpers';

/**
 * Stop on a chat thread with queued follow-ups returns the queued messages to
 * the compose box (retracting them) instead of leaving them to re-run as a new
 * response above "Response canceled". Seeded via psql (no live LLM): a running
 * thread with an active streaming turn + two persisted queued MessageReceived.
 * See docs/plans/2026-07-19-stop-clears-queued-messages.md.
 */
test.describe('Stop clears queued messages to compose', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('retracts queued follow-ups and returns their text to compose', async ({ page }) => {
    const threadId = randomUUID();
    const activeMessageId = randomUUID();
    const title = `Stop-clears-queue e2e ${randomUUID().slice(0, 8)}`;
    const activeMarker = `active-${randomUUID().slice(0, 8)}`;
    const queuedOne = `queued-one-${randomUUID().slice(0, 8)}`;
    const queuedTwo = `queued-two-${randomUUID().slice(0, 8)}`;
    const t0 = new Date().toISOString();
    const t1 = new Date(Date.now() + 1000).toISOString();
    const t2 = new Date(Date.now() + 2000).toISOString();
    const t3 = new Date(Date.now() + 3000).toISOString();

    psql([
      `INSERT INTO thread_summaries (` +
        `thread_id, title, source, last_activity, message_count, is_saved, has_response, status, ` +
        `archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, coding_agent_has_diff` +
      `) VALUES (` +
        `'${threadId}', '${title}', 'chat', '${t3}', 3, false, false, 'running', ` +
        `'inbox', 'active', false, 0, 0, false, false, false, false` +
      `)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${activeMessageId}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: activeMarker, channel: 'chat' })}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'TextStreamed', ` +
        `'${JSON.stringify({ text: 'Still working...', request_event_id: activeMessageId })}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: queuedOne, channel: 'chat' })}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: queuedTwo, channel: 'chat' })}'::jsonb, '${t3}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`${REAL_THREAD_ROW}:visible`, { hasText: title }).first();
      await expect(row).toBeVisible();
      await row.click();
      await ensureOnThreadPane(page);

      // The two follow-ups are queued behind the active streaming turn.
      const group = page.locator('.queued-message-group:visible').first();
      await expect(group.locator('.queued-message-group-summary')).toContainText('Queued (2)');

      // Press Stop (the Send→Cancel morph). Don't use cancelStreamingResponse():
      // this DB-seeded thread has no live loop, so cancel settles it via
      // ResponseAborted, not "Canceled" — we assert the queue→compose behavior.
      await page.locator('button.send-cancel-morph[aria-label="Cancel"]:not(:disabled)').first().click();

      // The queued group is retracted (QueuedMessageRemoved) → the bubbles vanish.
      await expect(page.locator('.queued-message-group:visible')).toHaveCount(0);
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedOne })).toHaveCount(0);
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedTwo })).toHaveCount(0);

      // Their text is returned to compose, FIFO — the active turn's text is not.
      const input = await waitForVisibleInput(page);
      await expect(input).toHaveValue(new RegExp(`${queuedOne}[\\s\\S]*${queuedTwo}`));
      await expect(input).not.toHaveValue(new RegExp(activeMarker));
    } finally {
      psql([
        `DELETE FROM events WHERE thread_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
