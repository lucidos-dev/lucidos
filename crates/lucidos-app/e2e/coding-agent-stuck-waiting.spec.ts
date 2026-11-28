import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForVisibleInput, hideDashboardRail, clickChangeAction, waitForActionPanel,
} from './helpers';
import {
  psql, createCCThreadWithChange, cleanupCCThread, cleanupFileFromMain,
} from './db-helpers';
import { randomUUID } from 'crypto';
import type { Page } from './fixtures';

async function applyAndDismiss(page: Page, threadId: string): Promise<void> {
  await clickChangeAction(page, 'Apply');
  await waitForActionPanel(page, 'Archive', 15_000);
  await clickChangeAction(page, 'Archive');

  await expect.poll(
    () => psql(`SELECT archive_state FROM thread_summaries WHERE thread_id = '${threadId}'`),
    { intervals: [500], timeout: 10_000 },
  ).toBe('archived');
}


function seedStuckThread(suffix: string): { threadId: string } {
  const threadId = randomUUID();
  const now = new Date().toISOString();

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'E2E Stuck No Changes ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, 'none', false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'CodingAgentIdled', '{"has_changes":false,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));

  return { threadId };
}

test.describe('CC stuck-in-waiting regression', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    psql("UPDATE thread_summaries SET status = 'idle', archive_state = 'archived' WHERE status = 'waiting' AND is_coding_agent = true");
  });

  test('apply change moves thread from Review to Archive', async ({ page }) => {
    const suffix = `apply-exits-review-${Date.now()}`;
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Stuck Waiting', suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      await openThreadDrawer(page);
      const threadInReview = page.locator(`.thread-row-review[data-thread-nav="${threadId}"]`);
      await expect(threadInReview).toHaveCount(1, { timeout: 5_000 });

      await ensureOnThreadPane(page);
      await applyAndDismiss(page, threadId);

      await openThreadDrawer(page);
      await expect(threadInReview).toHaveCount(0, { timeout: 5_000 });

      cleanupFileFromMain(file, suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('Archive button dismisses stuck thread without changes', async ({ page }) => {
    const suffix = `archive-dismisses-${Date.now()}`;
    const { threadId } = seedStuckThread(suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const archiveBtn = page.locator('.thread-action-buttons:visible button.action-btn:has-text("Archive")').first();
      await expect(archiveBtn).toBeVisible({ timeout: 15_000 });

      await expect(page.locator('.thread-action-buttons:visible button:has-text("Apply")')).toHaveCount(0);
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Discard")')).toHaveCount(0);

      await archiveBtn.click();

      await expect.poll(
        () => psql(`SELECT status FROM thread_summaries WHERE thread_id = '${threadId}'`),
        { intervals: [500], timeout: 10_000 },
      ).toBe('idle');

      await openThreadDrawer(page);
      const threadInReview = page.locator(`.thread-row-review[data-thread-nav="${threadId}"]`);
      await expect(threadInReview).toHaveCount(0, { timeout: 5_000 });
    } finally {
      cleanupCCThread(threadId);
    }
  });

  test('applied thread stays in Archive after page reload', async ({ page }) => {
    const suffix = `apply-survives-reload-${Date.now()}`;
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Stuck Waiting', suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await hideDashboardRail(page);
      await navigateToApp(page);

      await applyAndDismiss(page, threadId);

      await page.reload();
      await ensureOnThreadPane(page);
      await waitForVisibleInput(page);

      await openThreadDrawer(page);
      const threadInReview = page.locator(`.thread-row-review[data-thread-nav="${threadId}"]:visible`);
      await expect(threadInReview).toHaveCount(0, { timeout: 5_000 });

      await ensureOnThreadPane(page);
      await expect(page.locator('.thread-action-buttons:visible')).toHaveCount(0, { timeout: 5_000 });
      await expect(page.locator('.prompt-actions-row:visible .send-cancel-morph')).toHaveCount(1);

      const status = psql(`SELECT status FROM thread_summaries WHERE thread_id = '${threadId}'`);
      expect(status).toBe('idle');

      cleanupFileFromMain(file, suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
