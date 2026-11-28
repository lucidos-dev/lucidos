import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';


const SUMMARY = 'Delete the stale artifacts directory';
const COMMAND = 'rm -rf data/artifacts/stale';

function seedThreadWithCheckpoint(counts: { restores: number; removes: number } | null = { restores: 2, removes: 1 }): { threadId: string; checkpointId: string } {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const checkpointId = randomUUID();
  const now = new Date().toISOString();

  psql(
    [
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) ` +
        `VALUES ('${threadId}', 'E2E command checkpoint', 'chat', '${now}', 1, false, true, 'idle', 'inbox', false, 0, 'none', false)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${messageId}', 'MessageReceived', '{"text":"clean up the stale artifacts","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'CommandCheckpointed', ` +
        `'{"checkpoint_id":"${checkpointId}","command":"${COMMAND}","summary":"${SUMMARY}","request_event_id":"${messageId}"` +
        (counts ? `,"restores":${counts.restores},"removes":${counts.removes}` : '') +
        `}'::jsonb, ` +
        `'${now}', 'thread', '${threadId}', '${threadId}')`,
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

  function seed(counts?: { restores: number; removes: number } | null) {
    const seeded = seedThreadWithCheckpoint(counts === undefined ? { restores: 2, removes: 1 } : counts);
    seededThreads.push(seeded.threadId);
    return seeded;
  }

  test('renders the card with summary + command + a one-click Undo', async ({ page }) => {
    const { threadId } = seed();
    await openThread(page, threadId);

    const card = page.locator('[data-role="checkpoint-card"]:visible').first();
    await expect(card).toBeVisible({ timeout: 10_000 });
    await expect(card).toContainText(SUMMARY);
    await expect(card.locator('.step-note-detail')).toHaveText(COMMAND);
    await expect(card.locator('[data-role="checkpoint-undo"]')).toBeVisible();
    await expect(card.locator('.checkpoint-scope')).toHaveText(
      'Undo will restore 2 files and remove 1 file this step created.',
    );
  });

  test('a checkpoint with no recorded counts still renders, without a scope line', async ({ page }) => {
    const { threadId } = seed(null);
    await openThread(page, threadId);

    const card = page.locator('[data-role="checkpoint-card"]:visible').first();
    await expect(card).toBeVisible({ timeout: 10_000 });
    await expect(card.locator('[data-role="checkpoint-undo"]')).toBeVisible();
    await expect(card.locator('.checkpoint-scope')).toHaveCount(0);
  });

  test('Diff opens the change, and explains a reclaimed snapshot pair', async ({ page }) => {
    const { threadId } = seed();
    await openThread(page, threadId);

    const card = page.locator('[data-role="checkpoint-card"]:visible').first();
    await expect(card).toBeVisible({ timeout: 10_000 });
    await card.locator('[data-role="checkpoint-diff"]').click();

    const modal = page.locator('[data-role="checkpoint-diff-modal"]:visible').first();
    await expect(modal).toBeVisible({ timeout: 10_000 });
    await expect(modal).toContainText(SUMMARY);
    await expect(modal).toContainText('reclaimed');
    await expect(modal.locator('.folder-tree')).toHaveCount(0);
  });

  test('Undo on a ref-less checkpoint surfaces the failure as a toast', async ({ page }) => {
    const { threadId } = seed();
    await openThread(page, threadId);

    const card = page.locator('[data-role="checkpoint-card"]:visible').first();
    await expect(card).toBeVisible({ timeout: 10_000 });

    await card.locator('[data-role="checkpoint-undo"]').click();
    const toast = page.locator('.toast-error:visible').first();
    await expect(toast).toBeVisible({ timeout: 10_000 });
    await expect(toast).toContainText('Undo failed');

    await expect(card.locator('.checkpoint-reverted')).toHaveCount(0);
    await expect(card.locator('[data-role="checkpoint-undo"]')).toBeVisible();
  });
});
