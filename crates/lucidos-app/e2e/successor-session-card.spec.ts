import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

// A successor session starts in the same thread behind one card, the card any
// fresh session gets. It names where the handoff ran and what the session runs
// on, and folds the opening. The turns before it stay where they were.
test.describe('Successor session card', () => {
  test('draws one card with the opening folded, and keeps the earlier turn', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();
    const opening = `Handoff ${suffix}: phases 0 to 3 are committed. Continue with phase 4.`;
    const row = (type: string, payload: object) =>
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count, coding_agent) VALUES ('${threadId}', 'Successor E2E ${suffix}', 'claude_code', '${now}', 2, false, true, 'idle', 'inbox', true, 0, 'codex')`,
      row('MessageReceived', { text: 'Build the plan', channel: 'claude_code' }),
      row('SessionStarted', { session_id: 's1', channel: 'claude_code', continuity: { kind: 'first' } }),
      row('CodingAgentTextStreamed', { text: `Earlier session reply ${suffix}`, channel: 'claude_code' }),
      row('CodingAgentIdled', { has_changes: false, is_external_repo: false, requires_restart: false }),
      row('MessageReceived', {
        text: opening, channel: 'claude_code', mode: 'system', model: 'gpt-6.1-sol', reasoning_effort: 'medium',
        origin: { kind: 'engine', reason: { kind: 'successor_session' } },
      }),
      row('SessionStarted', {
        session_id: 's2', channel: 'claude_code', coding_agent: 'codex',
        continuity: { kind: 'handoff', checkpoint: 'phase-commit', progress: '4/7', opening: 'narrative', message: opening },
      }),
      row('CodingAgentTextStreamed', { text: `Successor session reply ${suffix}`, channel: 'claude_code' }),
      row('CodingAgentIdled', { has_changes: false, is_external_repo: false, requires_restart: false }),
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      await page.locator(`.thread-row:has-text("Successor E2E ${suffix}")`).first().click();

      const pane = page.locator('.pane-thread');
      await expect(pane.getByText(`Successor session reply ${suffix}`)).toBeVisible({ timeout: 10_000 });
      await expect(pane.getByText(`Earlier session reply ${suffix}`)).toBeVisible();

      const cards = pane.locator('[data-role="handoff-session"]');
      await expect(cards).toHaveCount(1);
      const card = cards.first();
      await expect(card).toContainText('New session');
      await expect(card).toContainText('Handed off');
      await expect(card).toContainText('Carried on from a handoff at phase-commit 4/7.');
      await expect(card).toContainText('Codex');

      // The opening is folded, and nothing else in the thread repeats it.
      await expect(pane.getByText(opening)).toHaveCount(0);
      await card.locator('.event-row-fold-toggle').click();
      await expect(card.getByText(opening)).toBeVisible();
      await expect(pane.getByText(opening)).toHaveCount(1);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
