import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, waitForVisibleInput, expectFocusedPinned } from './helpers';
import { psql } from './db-helpers';

function seedParentWithRunningCcChild(): { parentId: string; childId: string } {
  const parentId = randomUUID();
  const childId = randomUUID();
  const now = new Date().toISOString();

  psql(
    [
      `INSERT INTO thread_summaries (` +
        `thread_id, title, source, last_activity, message_count, is_saved, has_response, ` +
        `status, archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_change_state, coding_agent_is_external_repo, ` +
        `blocking_descendant_count` +
        `) VALUES (` +
        `'${parentId}', 'cascade archive parent', 'chat', '${now}', 2, false, true, ` +
        `'idle', 'inbox', 'active', false, 1, 1, ` +
        `'none', false, ` +
        `1` +
        `)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"do something with a sub-thread","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${parentId}', '${parentId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Working on it.","images":[]}'::jsonb, '${now}', 'thread', '${parentId}', '${parentId}')`,
      `INSERT INTO thread_summaries (` +
        `thread_id, parent_thread_id, title, source, last_activity, message_count, is_saved, has_response, ` +
        `status, archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_change_state, coding_agent_is_external_repo, ` +
        `blocking_descendant_count` +
        `) VALUES (` +
        `'${childId}', '${parentId}', 'cascade archive child', 'claude_code', '${now}', 1, false, false, ` +
        `'running', 'inbox', 'active', true, 0, 0, ` +
        `'none', false, ` +
        `0` +
        `)`,
    ].join(';\n'),
  );

  return { parentId, childId };
}

async function openParent(page: Page, parentId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, parentId);
  await navigateToApp(page);
}

test.describe('Cascading archive — Archive button gating', () => {
  const seededThreads: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededThreads.length = 0;
  });

  test.afterEach(() => {
    if (seededThreads.length === 0) return;
    const ids = seededThreads.map(id => `'${id}'`).join(',');
    psql(
      `DELETE FROM events WHERE thread_id IN (${ids}); ` +
      `DELETE FROM thread_summaries WHERE thread_id IN (${ids})`,
    );
  });

  test('archive button hidden on parent while CC sub-thread is running; reappears once it idles', async ({ page }) => {
    const { parentId, childId } = seedParentWithRunningCcChild();
    seededThreads.push(parentId, childId);

    await openParent(page, parentId);
    await waitForVisibleInput(page);
    await expect(
      page
        .locator(`[data-role="prompt-input"][data-thread-id="${parentId}"]:visible`)
        .first(),
    ).toBeVisible({ timeout: 15_000 });

    const archiveBtn = page.locator(
      'button.action-btn:not([aria-disabled="true"]):not(:disabled):has(.action-slot-label:text-is("Archive")):visible',
    );
    await expect(archiveBtn).toHaveCount(0, { timeout: 5_000 });
    await expect(page.locator('button.action-btn-blocked[aria-disabled="true"]:visible')).toHaveCount(1);
    await expectFocusedPinned(page, false);

    psql(
      [
        `UPDATE thread_summaries SET status='idle' WHERE thread_id='${childId}'`,
        `UPDATE thread_summaries SET blocking_descendant_count=0 WHERE thread_id='${parentId}'`,
      ].join(';\n'),
    );

    await page.reload();
    await waitForVisibleInput(page);
    await expect(
      page
        .locator(`[data-role="prompt-input"][data-thread-id="${parentId}"]:visible`)
        .first(),
    ).toBeVisible({ timeout: 15_000 });

    await expect(
      page.locator('button.action-btn:has(.action-slot-label:text-is("Archive")):visible').first(),
    ).toBeVisible({ timeout: 10_000 });
  });
});
