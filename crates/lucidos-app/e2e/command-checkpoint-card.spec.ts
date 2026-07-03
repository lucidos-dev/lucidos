import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';

/** CheckpointCard render + Undo (ADR 0002, Phase 4).
 *
 *  A real checkpoint needs the command guard to fire on a reversible bash
 *  command during a live LLM turn — not deterministically e2e-able. So we seed
 *  the typed events directly: a `MessageReceived` to anchor the exchange plus a
 *  `CommandCheckpointed` step routed into it via `request_event_id`. Loading the
 *  thread must render the CheckpointCard with its one-click Undo.
 *
 *  Clicking Undo POSTs `/api/v1/command-checkpoint/undo`. The seeded checkpoint
 *  has no git safety ref behind it (we only wrote the event), so the restore
 *  fails and the endpoint returns 400 — we assert the UI surfaces that failure
 *  as a toast rather than silently swallowing it. */

const SUMMARY = 'Delete the stale artifacts directory';
const COMMAND = 'rm -rf data/artifacts/stale';

/** Seed a chat thread carrying one MessageReceived + a CommandCheckpointed step
 *  (routed into the message's exchange) + a terminal ResponseGenerated so the
 *  exchange renders as a complete, idle turn. Returns the ids for cleanup. */
function seedThreadWithCheckpoint(): { threadId: string; checkpointId: string } {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const checkpointId = randomUUID();
  const now = new Date().toISOString();

  psql(
    [
      // Chat thread row — idle, in the inbox, has a response.
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
        `VALUES ('${threadId}', 'E2E command checkpoint', 'chat', '${now}', 1, false, true, 'idle', 'inbox', false, 0, false, false, false)`,
      // The user prompt — anchors the exchange the card renders inside.
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${messageId}', 'MessageReceived', '{"text":"clean up the stale artifacts","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      // The checkpoint snapshot taken before the reversible command. `request_event_id`
      // groups it into the MessageReceived exchange (the real engine stamps the same).
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'CommandCheckpointed', ` +
        `'{"checkpoint_id":"${checkpointId}","command":"${COMMAND}","summary":"${SUMMARY}","request_event_id":"${messageId}"}'::jsonb, ` +
        `'${now}', 'thread', '${threadId}', '${threadId}')`,
      // Terminal — marks the exchange complete so it renders as a finished idle turn.
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Done.","images":[],"request_event_id":"${messageId}"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'),
  );

  return { threadId, checkpointId };
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
}

test.describe('Command checkpoint card', () => {
  const seededThreads: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededThreads.length = 0;
  });

  test.afterEach(() => {
    if (seededThreads.length === 0) return;
    const ids = seededThreads.map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  function seed() {
    const seeded = seedThreadWithCheckpoint();
    seededThreads.push(seeded.threadId);
    return seeded;
  }

  test('renders the card with summary + command + a one-click Undo', async ({ page }) => {
    const { threadId } = seed();
    await openThread(page, threadId);

    // Dual-layout: desktop SplitLayout and mobile MobileSwipeContainer both
    // render the chat tree; only one is visible.
    const card = page.locator('[data-role="checkpoint-card"]:visible').first();
    await expect(card).toBeVisible({ timeout: 10_000 });
    await expect(card).toContainText(SUMMARY);
    await expect(card.locator('.checkpoint-command')).toHaveText(COMMAND);
    await expect(card.locator('[data-role="checkpoint-undo"]')).toBeVisible();
  });

  test('Undo on a ref-less checkpoint surfaces the failure as a toast', async ({ page }) => {
    const { threadId } = seed();
    await openThread(page, threadId);

    const card = page.locator('[data-role="checkpoint-card"]:visible').first();
    await expect(card).toBeVisible({ timeout: 10_000 });

    // The seeded checkpoint has no git safety ref, so the undo restore fails
    // (400). The card must surface that — not swallow it into a silent no-op.
    await card.locator('[data-role="checkpoint-undo"]').click();
    const toast = page.locator('.toast-error:visible').first();
    await expect(toast).toBeVisible({ timeout: 10_000 });
    await expect(toast).toContainText('Undo failed');

    // The card stays un-reverted (no "Reverted ✓"); the Undo affordance remains.
    await expect(card.locator('.checkpoint-reverted')).toHaveCount(0);
    await expect(card.locator('[data-role="checkpoint-undo"]')).toBeVisible();
  });
});
