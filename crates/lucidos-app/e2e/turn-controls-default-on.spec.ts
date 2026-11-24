import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, ensureOnThreadPane, navigateToApp } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';


const FIRST = 'First I will search the index.';
const LAST = 'Now I will summarize what I found.';

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

function seedThread(title: string): string {
  const threadId = randomUUID();
  const base = Date.now() - 60_000;
  const rows: string[] = [];
  let seq = 0;
  const ev = (type: string, payload: unknown) => {
    const created = new Date(base + seq * 1000).toISOString();
    seq++;
    rows.push(
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ` +
      `('${randomUUID()}', '${type}', '${q(payload)}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`
    );
  };
  ev('MessageReceived', { text: 'Look something up and summarize it.', channel: 'chat' });
  ev('TextStreamed', { text: FIRST });
  ev('ToolCalled', { name: 'web_search', args: { query: 'lucidos' }, description: 'Search the web' });
  ev('ToolResult', { name: 'web_search', result: 'one result' });
  ev('TextStreamed', { text: LAST });
  ev('ResponseGenerated', { text: LAST, model: 'mock', channel: 'chat' });
  const last = new Date(base + seq * 1000).toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${title}', 'chat', '${last}', 1, false, true, 'idle', 'inbox', false, 0)`,
    ...rows,
  ].join(';\n'));
  return threadId;
}

function dropThread(threadId: string): void {
  psql([
    `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
    `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
  ].join(';\n'));
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
}

const control = (page: Page, role: 'toggle-steps' | 'toggle-details') =>
  page.locator(`.response-header .turn-controls [data-role="${role}"]:visible`).first();

test.describe('Turn controls default on', () => {
  test('an untouched transcript shows every prose block and the steps, and each control turns its own off and back on', async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Turn controls default on');
    try {
      await openThread(page, threadId);

      const steps = page.locator('[data-role="inline-step"]:visible');
      const exchange = page.locator('.chat-exchange').first();
      await expect(exchange).toBeVisible({ timeout: 30_000 });

      await expect(steps.first()).toBeVisible({ timeout: 30_000 });
      await expect(exchange).toContainText(FIRST);
      await expect(exchange).toContainText(LAST);
      await expect(control(page, 'toggle-steps')).toHaveAttribute('aria-pressed', 'true');
      await expect(control(page, 'toggle-details')).toHaveAttribute('aria-pressed', 'true');

      await control(page, 'toggle-steps').click();
      await expect(steps).toHaveCount(0);
      await expect(control(page, 'toggle-steps')).toHaveAttribute('aria-pressed', 'false');
      await control(page, 'toggle-steps').click();
      await expect(steps.first()).toBeVisible();
      await expect(control(page, 'toggle-steps')).toHaveAttribute('aria-pressed', 'true');

      await control(page, 'toggle-details').click();
      await expect(exchange).not.toContainText(FIRST);
      await expect(exchange).toContainText(LAST);
      await control(page, 'toggle-details').click();
      await expect(exchange).toContainText(FIRST);
    } finally {
      dropThread(threadId);
    }
  });
});
