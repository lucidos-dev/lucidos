import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, waitForVisibleInput } from './helpers';
import { psql } from './db-helpers';

/**
 * Browser e2e for the cascading-archive UI gate.
 *
 * The descendant-blocking rule is computed in two layers:
 * 1. Projection (`event_bus_projection.rs`) maintains
 *    `thread_summaries.blocking_descendant_count` on every CC/Chat lifecycle
 *    event. Rust integration tests cover the projection layer.
 * 2. The frontend renders `Archive` only when `resolveActions` admits it —
 *    `meta.blockingDescendantCount > 0` flips the resolver to return `[]`,
 *    so the button stops rendering.
 *
 * This spec exercises layer 2 directly: we seed the projection value the
 * frontend reads, navigate to the parent, and assert the Archive button is
 * absent. We then flip the count to zero (mirroring what the projection
 * would do after the child idles) and assert the button reappears. Spawning
 * a real CC subprocess would take minutes per scenario; the projection layer
 * is already covered by Rust unit tests.
 */
function seedParentWithRunningCcChild(): { parentId: string; childId: string } {
  const parentId = randomUUID();
  const childId = randomUUID();
  const now = new Date().toISOString();

  psql(
    [
      // Parent chat thread — idle, inbox, has_response so it's archive-eligible
      // by every gate EXCEPT the descendants check we're testing. The
      // projection has already counted the running CC sub-thread:
      // blocking_descendant_count = 1.
      `INSERT INTO thread_summaries (` +
        `thread_id, title, source, last_activity, message_count, is_saved, has_response, ` +
        `status, archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, ` +
        `blocking_descendant_count` +
        `) VALUES (` +
        `'${parentId}', 'cascade archive parent', 'chat', '${now}', 2, false, true, ` +
        `'idle', 'inbox', 'active', false, 1, 1, ` +
        `false, false, false, ` +
        `1` +
        `)`,
      // Give the parent SOMETHING to render — at minimum a user message and a
      // response so it doesn't render the empty-compose layout.
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"do something with a sub-thread","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${parentId}', '${parentId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'ResponseGenerated', '{"text":"Working on it.","images":[]}'::jsonb, '${now}', 'thread', '${parentId}', '${parentId}')`,
      // Running CC sub-thread. status='running' + archive_state='inbox' +
      // is_coding_agent=true is the canonical "blocking" combo per `is_blocking`.
      `INSERT INTO thread_summaries (` +
        `thread_id, parent_thread_id, title, source, last_activity, message_count, is_saved, has_response, ` +
        `status, archive_state, state, is_coding_agent, active_children_count, total_children_count, ` +
        `coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo, ` +
        `blocking_descendant_count` +
        `) VALUES (` +
        `'${childId}', '${parentId}', 'cascade archive child', 'claude_code', '${now}', 1, false, false, ` +
        `'running', 'inbox', 'active', true, 0, 0, ` +
        `false, false, false, ` +
        `0` +
        `)`,
    ].join(';\n'),
  );

  return { parentId, childId };
}

async function openParent(page: Page, parentId: string): Promise<void> {
  // Land on the parent thread on first paint — avoids a drawer click race.
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
    // Wait for the visible prompt input — dual-layout means both desktop and
    // mobile copies render, but only one is laid out. Asserting on `:visible`
    // scopes to the active one.
    await waitForVisibleInput(page);
    await expect(
      page
        .locator(`[data-role="prompt-input"][data-thread-id="${parentId}"]:visible`)
        .first(),
    ).toBeVisible({ timeout: 15_000 });

    // Archive must NOT render while the CC sub-thread is running. Pin still
    // renders (it's independent of resolveActions and reads is_saved only).
    // The Archive button has no aria-label — match its `.action-btn` class +
    // exact "Archive" text. `:text-is` enforces an exact match so the "Archive..."
    // spinner state (Archive in progress) doesn't accidentally satisfy the
    // "not present" assertion.
    const archiveBtn = page.locator('button.action-btn:text-is("Archive"):visible');
    await expect(archiveBtn).toHaveCount(0, { timeout: 5_000 });
    await expect(page.locator('button[aria-label="Pin thread"]:visible').first())
      .toBeVisible();

    // Flip the child to idle and zero the parent's blocking count — what the
    // projection would do on the child's `CodingAgentIdled`. The frontend
    // refetches threads on focus and on the SSE notify; we cause a re-render
    // by reloading. (A more elegant push-based assertion would require the
    // projection itself to fire, but we're testing the rendering rule here.)
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

    // Now Archive must render — every gate is clear.
    await expect(
      page.locator('button.action-btn:text-is("Archive"):visible').first(),
    ).toBeVisible({ timeout: 10_000 });
  });
});
