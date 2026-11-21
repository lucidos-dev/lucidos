import { test, expect } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * A follow-up typed under an open question card, read once the reader pressed
 * Cancel, shows the agent's Thinking row while the agent works on it.
 *
 * Reported: the stopped turn's withheld proposal and idle landed under the
 * follow-up, so it read as finished. The transcript showed nothing below the
 * message, and only the title spinner said the agent was working. The rows are
 * seeded, as the engine writes them, since the fake Claude Code cannot ask.
 */
test.describe('coding-agent follow-up after a canceled question', () => {
  test('shows a Thinking row under the follow-up while the agent works', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const title = `CC Follow-up Thinking E2E ${suffix}`;
    const followUp = `relay or pair ${suffix}?`;
    const start = Date.now() - 60_000;
    const firstId = randomUUID();
    const followUpId = randomUUID();
    const cc = { channel: 'claude_code' };
    const rows: [string, Record<string, unknown>, string?][] = [
      ['MessageReceived', { ...cc, text: 'start' }, firstId],
      ['CodingAgentInputRead', { ...cc, input_event_id: firstId, started_turn: true }],
      ['CodingAgentTextStreamed', { ...cc, text: 'working on it' }],
      ['MessageReceived', { ...cc, text: followUp }, followUpId],
      ['CodingAgentPromptSent', { ...cc, text: followUp, input_event_id: followUpId }],
      ['UserQuestionAsked', { ...cc, tool_use_id: `q-${suffix}#q0`, cc_session_id: 'sess-e2e', question: 'Which origin?', options: [{ id: 'opt-0', label: 'Document the trust' }] }],
      ['UserQuestionAnswered', { ...cc, tool_use_id: `q-${suffix}#q0`, answer: { kind: 'Canceled' } }],
      ['CodingAgentToolResult', { ...cc, name: 'AskUserQuestion', result: 'rejected', tool_use_id: `q-${suffix}` }],
      ['ResponseCanceled', { ...cc, cause: 'user_stop' }],
      ['ProposalWithheld', { ...cc, reason: 'turn_incomplete', files: ['src/a.rs'], branch_name: `e2e-${suffix}` }],
      ['CodingAgentIdled', { ...cc, has_changes: true }],
      ['CodingAgentInputRead', { ...cc, input_event_id: followUpId, started_turn: true }],
    ];
    const at = (i: number) => new Date(start + i * 1000).toISOString();
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${title}', 'claude_code', '${at(rows.length)}', 2, false, true, 'running', 'inbox', true, 0)`,
      ...rows.map(([type, payload, id], i) => {
        const json = JSON.stringify(payload).replace(/'/g, "''");
        return `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${id ?? randomUUID()}', '${type}', '${json}'::jsonb, '${at(i)}', 'thread', '${threadId}', '${threadId}')`;
      }),
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);
      const row = page.locator(`.thread-row:has-text("${title}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      const turn = page.locator(`.chat-exchange:visible:has-text("${followUp}")`).last();
      await expect(turn).toBeVisible({ timeout: 10_000 });
      await expect(turn.locator('[data-thinking-row]')).toBeVisible({ timeout: 10_000 });
    } finally {
      psql([
        `DELETE FROM events WHERE thread_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
