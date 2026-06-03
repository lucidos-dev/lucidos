import { test, expect, Page } from '@playwright/test';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';

/** Insert a parent thread carrying a single ChildThreadCompleted exchange.
 *  We seed the typed event directly (rather than spawning a real child) so
 *  the test stays fast + deterministic — the rendering of the card from the
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
        `VALUES ('${parentId}', 'E2E child-completion card', 'chat', '${now}', 2, false, true, 'idle', 'inbox', false, 0, false, false, false)`,
      // Original parent prompt — gives the thread something to render before the card.
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) ` +
        `VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"spawn a sidequest","mode":"human","channel":"chat"}'::jsonb, '${now}', 'thread', '${parentId}', '${parentId}')`,
      // Typed ChildThreadCompleted — the head of the new exchange + source for the card.
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

test.describe('Child-completion card', () => {
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

  test('success: header row shows "Child thread completed:" prefix + linked title + green badge; agent summary lives behind a collapsed disclosure', async ({ page }) => {
    const { parentId, childId } = seed({
      status: 'success',
      title: 'Refactor the foo helper',
      summary: 'Cleaned up the if/else ladder.',
    });
    await openParent(page, parentId);

    // Both desktop SplitLayout and mobile MobileSwipeContainer render the
    // chat tree concurrently; only one is visible at a time.
    const body = page.locator('.child-completion-body:visible').first();
    await expect(body).toBeVisible({ timeout: 10_000 });

    const panel = page.locator('.initiator-panel:visible:has(.child-completion-body)').first();
    await expect(panel.locator('.initiator-summary')).toHaveCount(0);

    const headerRow = body.locator('.child-completion-header-row');
    await expect(headerRow).toContainText('Child thread completed:');
    await expect(headerRow.locator('.child-completion-status-success')).toBeVisible();
    const titleLink = headerRow.locator(`button.accent-link[data-thread-id="${childId}"]`);
    await expect(titleLink).toHaveText('Refactor the foo helper');

    const disclosure = body.locator('.child-completion-disclosure');
    const summaryBlock = disclosure.locator('.child-completion-summary');
    await expect(summaryBlock).toBeHidden();
    await disclosure.locator('summary').click();
    await expect(summaryBlock).toBeVisible();
    await expect(summaryBlock).toContainText('Cleaned up the if/else ladder.');
  });

  test('failure: red badge + "failed" verb in prefix', async ({ page }) => {
    const { parentId } = seed({
      status: 'failure',
      title: 'Failing sidequest',
      summary: 'Tests blew up.',
    });
    await openParent(page, parentId);
    const body = page.locator('.child-completion-body:visible').first();
    await expect(body).toBeVisible({ timeout: 10_000 });
    await expect(body.locator('.child-completion-header-row'))
      .toContainText('Child thread failed:');
    await expect(body.locator('.child-completion-status-failure')).toBeVisible();
    await expect(body.locator('.child-completion-status-success')).toHaveCount(0);
  });

  test('no_changes: gray badge + "completed" verb in prefix', async ({ page }) => {
    const { parentId } = seed({
      status: 'no_changes',
      title: 'Sidequest with nothing to apply',
      summary: 'Nothing to do.',
    });
    await openParent(page, parentId);
    const body = page.locator('.child-completion-body:visible').first();
    await expect(body).toBeVisible({ timeout: 10_000 });
    await expect(body.locator('.child-completion-header-row'))
      .toContainText('Child thread completed:');
    await expect(body.locator('.child-completion-status-no-changes')).toBeVisible();
  });

  test('canceled: yellow badge + "canceled" verb in prefix; empty summary hides the disclosure', async ({ page }) => {
    const { parentId } = seed({
      status: 'canceled',
      title: 'Stopped sidequest',
      summary: '',
    });
    await openParent(page, parentId);
    const body = page.locator('.child-completion-body:visible').first();
    await expect(body).toBeVisible({ timeout: 10_000 });
    await expect(body.locator('.child-completion-header-row'))
      .toContainText('Child thread canceled:');
    await expect(body.locator('.child-completion-status-canceled')).toBeVisible();
    await expect(body.locator('.child-completion-disclosure')).toHaveCount(0);
  });

  test('actor chip does not open a route popover', async ({ page }) => {
    const { parentId } = seed({
      status: 'success',
      title: 'Refactor the foo helper',
      summary: 'done.',
    });
    await openParent(page, parentId);
    const body = page.locator('.child-completion-body:visible').first();
    await expect(body).toBeVisible({ timeout: 10_000 });
    const panel = page.locator('.initiator-panel:visible:has(.child-completion-body)').first();
    const actor = panel.locator('.initiator-actor');
    expect(await actor.evaluate(el => el.tagName)).toBe('SPAN');
    await actor.click({ force: true });
    await expect(page.locator('.message-route-panel:visible')).toHaveCount(0);
  });
});
