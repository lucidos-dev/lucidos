import { test, expect } from './fixtures';
import {
  navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane, waitForVisibleInput,
} from './helpers';
import {
  psql, createCCThreadWithChange, cleanupCCThread, cleanupFileFromMain,
} from './db-helpers';
import { randomUUID } from 'crypto';
import type { Page } from './fixtures';

/** Click Apply, wait for Archive (ChangeApplied keeps archive_state=inbox), click Archive, poll until archived. */
async function applyAndDismiss(page: Page, threadId: string): Promise<void> {
  const applyBtn = page.locator('.thread-action-buttons:visible button.action-btn-confirm:has-text("Apply")').first();
  await expect(applyBtn).toBeVisible({ timeout: 15_000 });
  await applyBtn.click();

  const archiveBtn = page.locator('.thread-action-buttons:visible button.action-btn:has-text("Archive")').first();
  await expect(archiveBtn).toBeVisible({ timeout: 15_000 });
  await archiveBtn.click();

  await expect.poll(
    () => psql(`SELECT archive_state FROM thread_summaries WHERE thread_id = '${threadId}'`),
    { intervals: [500], timeout: 10_000 },
  ).toBe('archived');
}

/**
 * Browser e2e tests for the CC stuck-in-waiting bug fix.
 *
 * Bug: After applying a CC change, CodingAgentIdled(has_changes=false) from
 * reset_worktree_and_idle set status back to 'waiting', leaving threads
 * permanently stuck in the Review section after engine restart.
 *
 * Fix: CodingAgentIdled now conditionally sets status — 'waiting' only if
 * has_changes=true, 'idle' otherwise. ThreadArchived clears all CC flags.
 * Startup query resets orphaned waiting threads with no changes.
 */

/** Seed a CC thread stuck in waiting with NO changes (simulates the pre-fix bug). */
function seedStuckThread(suffix: string): { threadId: string } {
  const threadId = randomUUID();
  const now = new Date().toISOString();

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) VALUES ('${threadId}', 'E2E Stuck No Changes ${suffix}', 'claude_code', '${now}', 1, false, true, 'waiting', 'inbox', true, 0, false, false, false)`,
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

      // Verify the thread shows the review highlight before applying. The
      // standalone "Review" section was merged into "Current" (commit
      // f584c9ac0); a thread that needs review is now marked by the row-level
      // .thread-row-review class (ThreadDrawer `needsReview`), not a separate
      // section. Mirror the post-apply assertion below.
      // The row carries both the .thread-row-review class and data-thread-nav on
      // the SAME element (ThreadRowContent), so match them with one compound
      // selector — a `has:` descendant filter never matches (an element is not
      // its own descendant).
      await openThreadDrawer(page);
      const threadInReview = page.locator(`.thread-row-review[data-thread-nav="${threadId}"]`);
      await expect(threadInReview).toHaveCount(1, { timeout: 5_000 });

      await ensureOnThreadPane(page);
      await applyAndDismiss(page, threadId);

      // After apply + archive the same row no longer carries the review highlight.
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

      // Should NOT show Apply/Discard (no pending changes)
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Apply")')).toHaveCount(0);
      await expect(page.locator('.thread-action-buttons:visible button:has-text("Discard")')).toHaveCount(0);

      await archiveBtn.click();

      // Verify the seeded thread was dismissed — poll DB until status changes.
      // Don't check banner visibility: handleArchiveThread may focus
      // another review thread (from a previous test's idle CC session).
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

      const status = psql(`SELECT status FROM thread_summaries WHERE thread_id = '${threadId}'`);
      expect(status).toBe('idle');

      cleanupFileFromMain(file, suffix);
    } finally {
      cleanupCCThread(threadId, changeId, branch, file);
    }
  });
});
