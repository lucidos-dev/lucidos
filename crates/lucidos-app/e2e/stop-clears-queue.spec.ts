import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import {
  assertHealthy, ensureOnThreadPane, navigateToApp, openThreadDrawer,
  REAL_THREAD_ROW, USER_MSG_SELECTOR, waitForVisibleInput,
} from './helpers';
import { psql } from './db-helpers';

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
    const queuedOneId = randomUUID();
    const queuedTwoId = randomUUID();
    const accepted = (messageId: string, text: string, created: string) =>
      `INSERT INTO events (id, aggregate, aggregate_id, event_type, payload, created) ` +
        `VALUES ('${randomUUID()}', 'system', 'system', 'OrdinarySubmissionAccepted', ` +
        `'${JSON.stringify({ type: 'OrdinarySubmissionAccepted', data: {
          thread_id: threadId, message_event_id: messageId, entry_id: randomUUID(),
          request: {
            type: 'agent-chat', thread_id: threadId, event_id: messageId,
            mode: 'human', message: text, image_hashes: [],
          },
        } })}'::jsonb, '${created}')`;
    const at = (secondsAgo: number) => new Date(Date.now() - secondsAgo * 1000).toISOString();
    const [t0, t1, t2, t3] = [at(4), at(3), at(2), at(1)];

    psql([
      `INSERT INTO thread_summaries (` +
        `thread_id, title, source, last_activity, message_count, is_saved, has_response, status, ` +
        `archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_change_state, coding_agent_is_external_repo` +
      `) VALUES (` +
        `'${threadId}', '${title}', 'chat', '${t3}', 3, false, false, 'running', ` +
        `'inbox', 'active', false, 0, 0, 'none', false` +
      `)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${activeMessageId}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: activeMarker, channel: 'chat' })}'::jsonb, '${t0}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'TextStreamed', ` +
        `'${JSON.stringify({ text: 'Still working...', request_event_id: activeMessageId })}'::jsonb, '${t1}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${queuedOneId}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: queuedOne, channel: 'chat' })}'::jsonb, '${t2}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${queuedTwoId}', 'MessageReceived', ` +
        `'${JSON.stringify({ text: queuedTwo, channel: 'chat' })}'::jsonb, '${t3}', 'thread', '${threadId}', '${threadId}')`,
      accepted(queuedOneId, queuedOne, t2),
      accepted(queuedTwoId, queuedTwo, t3),
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`${REAL_THREAD_ROW}:visible`, { hasText: title }).first();
      await expect(row).toBeVisible();
      await row.click();
      await ensureOnThreadPane(page);

      const group = page.locator('.queued-message-group:visible').first();
      await expect(group.locator('.queued-message-group-summary')).toContainText('Queued (2)');

      await page.locator('button.send-cancel-morph[aria-label="Cancel"]:not(:disabled)').first().click();

      await expect(page.locator('.queued-message-group:visible')).toHaveCount(0);
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedOne })).toHaveCount(0);
      await expect(page.locator(`${USER_MSG_SELECTOR}:visible`).filter({ hasText: queuedTwo })).toHaveCount(0);

      const input = await waitForVisibleInput(page);
      await expect(input).toHaveValue(new RegExp(`${queuedOne}[\\s\\S]*${queuedTwo}`));
      await expect(input).not.toHaveValue(new RegExp(activeMarker));
    } finally {
      psql([
        `DELETE FROM events WHERE thread_id = '${threadId}'`,
        `DELETE FROM events WHERE event_type = 'OrdinarySubmissionAccepted' ` +
          `AND payload->'data'->>'thread_id' = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
