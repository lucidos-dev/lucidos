import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';

/** Insert a parent thread carrying a single ChildThreadCompleted exchange.
 *  We seed the typed event directly (rather than spawning a real child) so
 *  the test stays fast and deterministic: the rendering of the row from the
 *  typed event is what we want to assert, not the LLM-driven spawn flow. */
function seedParentWithChildCompletion(opts: {
  status: 'success' | 'failure' | 'no_changes' | 'canceled';
  title?: string | null;
  summary?: string;
  pendingChangeIds?: string[];
}): { parentId: string; childId: string; childCompletedEventId: string } {
  const parentId = randomUUID();
  const childId = randomUUID();
  const childCompletedEventId = randomUUID();
  const now = new Date().toISOString();
  const titleField = opts.title === null
    ? ''
    : `,"child_thread_title":"${opts.title ?? 'Refactor the foo helper'}"`;
  const summary = (opts.summary ?? 'Cleaned up the if/else ladder.').replace(/"/g, '\\"');
  const pendingArr = JSON.stringify(opts.pendingChangeIds ?? []);

  psql(
    [
      // Parent thread row.
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
        `VALUES ('${parentId}', 'E2E child-completion row', 'chat', '${now}', 2, false, true, 'idle', 'inbox', false, 0, false, false, false)`,
      // Original parent prompt, giving the thread something to render before the row.
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"spawn a sidequest","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${parentId}', '${parentId}')`,
      // Typed ChildThreadCompleted: the head of the new exchange and the source for the row.
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${childCompletedEventId}', 'ChildThreadCompleted', ` +
        `'{"child_thread_id":"${childId}","status":"${opts.status}","summary":"${summary}","pending_change_ids":${pendingArr}${titleField}}'::jsonb, ` +
        `'${now}', 'thread', '${parentId}', '${parentId}')`,
    ].join(';\n'),
  );

  return { parentId, childId, childCompletedEventId };
}

async function openParent(page: Page, parentId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, parentId);
  await navigateToApp(page);
}

/** The child callback's event row. One of the four kinds that share
 *  `EventRow`, told apart by `data-role` (see `components/chat/EventRow.tsx`).
 *
 *  Both desktop SplitLayout and mobile MobileSwipeContainer render the chat
 *  tree concurrently; only one is visible at a time. */
function childRow(page: Page) {
  return page.locator('.event-row[data-role="child-completion"]:visible').first();
}

test.describe('Child-completion row', () => {
  const seededParents: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededParents.length = 0;
  });

  test.afterEach(() => {
    if (seededParents.length === 0) return;
    const ids = seededParents.map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  function seed(opts: Parameters<typeof seedParentWithChildCompletion>[0]) {
    const seeded = seedParentWithChildCompletion(opts);
    seededParents.push(seeded.parentId);
    return seeded;
  }

  test('success: head reads "Child thread returned:" with a linked title and a good-toned state word; the agent summary lives behind a collapsed fold', async ({ page }) => {
    const { parentId, childId } = seed({
      status: 'success',
      title: 'Refactor the foo helper',
      summary: 'Cleaned up the if/else ladder.',
    });
    await openParent(page, parentId);

    const row = childRow(page);
    await expect(row).toBeVisible({ timeout: 10_000 });
    await expect(row).toHaveAttribute('data-kind', 'child');
    await expect(row).toHaveAttribute('data-state', 'success');

    // The row owns its own prefix, so the surrounding panel adds no summary
    // line that would print the same words twice.
    const panel = page.locator('.initiator-panel:visible:has(.event-row[data-role="child-completion"])').first();
    await expect(panel.locator('.initiator-summary')).toHaveCount(0);

    const head = row.locator('.event-row-head');
    await expect(head).toContainText('Child thread returned:');
    const state = head.locator('.event-row-state');
    await expect(state).toHaveText('success');
    await expect(state).toHaveAttribute('data-tone', 'good');
    const titleLink = head.locator(`button.accent-link[data-thread-id="${childId}"]`);
    await expect(titleLink).toHaveText('Refactor the foo helper');

    const fold = row.locator('details.event-row-fold');
    const foldBody = fold.locator('.event-row-fold-body');
    await expect(foldBody).toBeHidden();
    await fold.locator('summary').click();
    await expect(foldBody).toBeVisible();
    await expect(foldBody).toContainText('Cleaned up the if/else ladder.');
  });

  test('failure: bad-toned state word and a "failed" verb in the prefix', async ({ page }) => {
    const { parentId } = seed({
      status: 'failure',
      title: 'Failing sidequest',
      summary: 'Tests blew up.',
    });
    await openParent(page, parentId);
    const row = childRow(page);
    await expect(row).toBeVisible({ timeout: 10_000 });
    await expect(row).toHaveAttribute('data-state', 'failure');
    await expect(row.locator('.event-row-head')).toContainText('Child thread failed:');
    const state = row.locator('.event-row-state');
    await expect(state).toHaveText('failure');
    await expect(state).toHaveAttribute('data-tone', 'bad');
  });

  test('no_changes: untinted state word and a "returned" verb in the prefix', async ({ page }) => {
    const { parentId } = seed({
      status: 'no_changes',
      title: 'Sidequest with nothing to apply',
      summary: 'Nothing to do.',
    });
    await openParent(page, parentId);
    const row = childRow(page);
    await expect(row).toBeVisible({ timeout: 10_000 });
    await expect(row).toHaveAttribute('data-state', 'no_changes');
    await expect(row.locator('.event-row-head')).toContainText('Child thread returned:');
    const state = row.locator('.event-row-state');
    await expect(state).toHaveText('no changes');
    await expect(state).toHaveAttribute('data-tone', 'none');
  });

  test('canceled: halted-toned state word and a "canceled" verb in the prefix; an empty summary hides the fold', async ({ page }) => {
    const { parentId } = seed({
      status: 'canceled',
      title: 'Stopped sidequest',
      summary: '',
    });
    await openParent(page, parentId);
    const row = childRow(page);
    await expect(row).toBeVisible({ timeout: 10_000 });
    await expect(row).toHaveAttribute('data-state', 'canceled');
    await expect(row.locator('.event-row-head')).toContainText('Child thread canceled:');
    const state = row.locator('.event-row-state');
    await expect(state).toHaveText('canceled');
    await expect(state).toHaveAttribute('data-tone', 'halted');
    await expect(row.locator('details.event-row-fold')).toHaveCount(0);
  });

  test('pending changes the child left are stated on the facts line', async ({ page }) => {
    const { parentId } = seed({
      status: 'success',
      title: 'Sidequest with changes',
      summary: 'Two files touched.',
      pendingChangeIds: [randomUUID(), randomUUID()],
    });
    await openParent(page, parentId);
    const row = childRow(page);
    await expect(row).toBeVisible({ timeout: 10_000 });
    await expect(row.locator('.event-row-meta')).toContainText('2 pending changes');
  });

  test('actor chip does not open a route popover', async ({ page }) => {
    const { parentId } = seed({
      status: 'success',
      title: 'Refactor the foo helper',
      summary: 'done.',
    });
    await openParent(page, parentId);
    const row = childRow(page);
    await expect(row).toBeVisible({ timeout: 10_000 });
    const panel = page.locator('.initiator-panel:visible:has(.event-row[data-role="child-completion"])').first();
    const actor = panel.locator('.initiator-actor');
    expect(await actor.evaluate(el => el.tagName)).toBe('SPAN');
    await actor.click({ force: true });
    await expect(page.locator('.message-route-panel:visible')).toHaveCount(0);
  });
});
