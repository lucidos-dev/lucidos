import { test, expect } from '@playwright/test';
import {
  navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForVisibleInput,
} from './helpers';
import {
  psql, createCCThreadWithChange, cleanupCCThread, cleanupFileFromMain,
} from './db-helpers';
import { randomUUID } from 'crypto';
import type { Page } from '@playwright/test';

/** Click Apply, wait for Done (ChangeApplied keeps section=unread), click Done, poll until dismissed. */
async function applyAndDismiss(page: Page, threadId: string): Promise<void> {
  const applyBtn = page.locator('.thread-action-buttons:visible button.action-btn-confirm:has-text("Apply")').first();
  await expect(applyBtn).toBeVisible({ timeout: 15_000 });
  await applyBtn.click();

  const doneBtn = page.locator('.thread-action-buttons:visible button.action-btn:has-text("Done")').first();
  await expect(doneBtn).toBeVisible({ timeout: 15_000 });
  await doneBtn.click();

  await expect.poll(
    () => psql(`SELECT section FROM thread_summaries WHERE thread_id = '${threadId}'`),
    { intervals: [500], timeout: 10_000 },
  ).toBe('default');
}

/**
 * Browser e2e tests for the CC stuck-in-waiting bug fix.
 *
 * Bug: After applying a CC change, CodingAgentIdled(has_changes=false) from
 * reset_worktree_and_idle set status back to 'waiting', leaving threads
 * permanently stuck in the Review section after engine restart.
 *
 * Fix: CodingAgentIdled now conditionally sets status — 'waiting' only if
 * has_changes=true, 'idle' otherwise. ThreadDismissed clears all CC flags.
 * Startup query resets orphaned waiting threads with no changes.
 */

/** Seed a CC thread stuck in waiting with NO changes (simulates the pre-fix bug). */
function seedStuckThread(suffix: string): { threadId: string } {
  const threadId = randomUUID();
  const now = new Date().toISOString();

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_pinned, has_response, status, section, is_cc, active_children_count, cc_has_changes, cc_requires_restart, cc_is_external_repo) VALUES ('${threadId}', 'E2E Stuck No Changes ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'unread', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"test","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Done.","images":[]}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'CodingAgentIdled', '{"has_changes":false,"is_external_repo":false,"requires_restart":false}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
  ].join(';\n'));

  return { threadId };
}

test.describe('CC stuck-in-waiting regression', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    // Clean up stale waiting CC threads from previous tests to avoid
    // Done banners from old sessions interfering with these tests.
    psql("UPDATE thread_summaries SET status = 'idle', section = 'default' WHERE status = 'waiting' AND is_cc = true");
  });

  test('apply change moves thread from Review to History', async ({ page }) => {
    const suffix = `apply-exits-review-${Date.now()}`;
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Stuck Waiting', suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      // Verify thread is in Review before applying
      await openThreadDrawer(page);
      const reviewSection = page.locator('.list-section-title:has-text("Review"):visible').first();
      await expect(reviewSection).toBeVisible({ timeout: 5_000 });

      await ensureOnThreadPane(page);
      await applyAndDismiss(page, threadId);

      await openThreadDrawer(page);
      const threadInReview = page.locator('.thread-row-review').filter({
        has: page.locator(`[data-thread-nav="${threadId}"]`),
      });
      await expect(threadInReview).toHaveCount(0, { timeout: 5_000 });

      cleanupFileFromMain(file, suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });

  test('Done button dismisses stuck thread without changes', async ({ page }) => {
    const suffix = `done-dismisses-${Date.now()}`;
    const { threadId } = seedStuckThread(suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      const doneBtn = page.locator('.thread-action-buttons:visible button.action-btn:has-text("Done")').first();
      await expect(doneBtn).toBeVisible({ timeout: 15_000 });

      // Should NOT show Apply/Discard (no pending changes)
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Apply")')).toHaveCount(0);
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Discard")')).toHaveCount(0);

      await doneBtn.click();

      // Verify the seeded thread was dismissed — poll DB until status changes.
      // Don't check banner visibility: handleDismissThread may focus
      // another review thread (from a previous test's idle CC session).
      await expect.poll(
        () => psql(`SELECT status FROM thread_summaries WHERE thread_id = '${threadId}'`),
        { intervals: [500], timeout: 10_000 },
      ).toBe('idle');

      await openThreadDrawer(page);
      const threadInReview = page.locator('.thread-row-review').filter({
        has: page.locator(`[data-thread-nav="${threadId}"]`),
      });
      await expect(threadInReview).toHaveCount(0, { timeout: 5_000 });
    } finally {
      cleanupCCThread(threadId);
    }
  });

  test('applied thread stays in History after page reload', async ({ page }) => {
    const suffix = `apply-survives-reload-${Date.now()}`;
    const { threadId, changeId, branch, file } = createCCThreadWithChange('E2E Stuck Waiting', suffix);

    try {
      await page.addInitScript((tid: string) => {
        localStorage.setItem('lucidos-focused-thread', tid);
      }, threadId);
      await navigateToApp(page);

      await applyAndDismiss(page, threadId);

      await page.reload();
      await ensureOnThreadPane(page);
      await waitForVisibleInput(page);

      await openThreadDrawer(page);
      const threadInReview = page.locator('.thread-row-review:visible').filter({
        has: page.locator(`[data-thread-nav="${threadId}"]`),
      });
      await expect(threadInReview).toHaveCount(0, { timeout: 5_000 });

      await ensureOnThreadPane(page);
      await expect(page.locator('.thread-action-buttons:visible')).toHaveCount(0, { timeout: 5_000 });

      const status = psql(`SELECT status FROM thread_summaries WHERE thread_id = '${threadId}'`);
      expect(status).toBe('idle');

      cleanupFileFromMain(file, suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
