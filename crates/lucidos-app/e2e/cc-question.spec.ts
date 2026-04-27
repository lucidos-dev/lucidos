import { test, expect } from '@playwright/test';
import { navigateToApp, assertHealthy, openThreadDrawer, ensureOnThreadPane } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * Browser e2e for CC AskUserQuestion interactive UI.
 *
 * We inject a synthetic UserQuestionAsked event directly into the DB for a
 * CC thread (mirroring what the engine would do after intercepting CC's
 * AskUserQuestion tool_use). The browser must:
 *   - Render a QuestionCard with the question text + clickable options.
 *   - Persist a UserQuestionAnswered event after the user clicks an option.
 *   - Show the resolved "Answered: <label>" view.
 *
 * Spawning a real CC subprocess that emits AskUserQuestion is out of scope
 * for browser e2e — the parser-level wiring is covered by Rust unit tests.
 */
test.describe('CC AskUserQuestion — interactive answer flow', () => {
  test('clicking an option resolves the question card', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const toolUseId = `tu-e2e-${suffix}`;
    const now = new Date().toISOString();

    // Seed a CC thread + UserQuestionAsked event.
    const msgEventId = randomUUID();
    const sessionStartedId = randomUUID();
    const questionEventId = randomUUID();
    const payload = JSON.stringify({
      tool_use_id: toolUseId,
      cc_session_id: 'sess-e2e',
      question: `Pick option ${suffix}`,
      options: [
        { id: 'opt-0', label: `Yes ${suffix}` },
        { id: 'opt-1', label: `No ${suffix}` },
      ],
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_pinned, has_response, status, section, is_cc, active_children_count) VALUES ('${threadId}', 'CC Question E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'unread', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${msgEventId}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${sessionStartedId}', 'SessionStarted', '{"session_id":"sess-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${questionEventId}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      // Open the seeded thread.
      const row = page.locator(`.thread-row:has-text("CC Question E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();
      await ensureOnThreadPane(page);

      // QuestionCard renders the question + options. Both SplitLayout (desktop)
      // and MobileSwipeContainer (mobile) render simultaneously, so we scope
      // to the visible copy — `.first()` would otherwise pick the hidden one.
      const card = page.locator(`.cc-question-card[data-tool-use-id="${toolUseId}"]:visible`).first();
      await expect(card).toBeVisible({ timeout: 10_000 });
      await expect(card).toContainText(`Pick option ${suffix}`);
      await expect(card).toContainText(`Yes ${suffix}`);
      await expect(card).toContainText(`No ${suffix}`);

      // Click the second option.
      await card.locator('.cc-question-option').nth(1).click();

      // The DB should have a UserQuestionAnswered for this tool_use_id.
      await expect.poll(
        () => psql(`SELECT COUNT(*) FROM events WHERE thread_id = '${threadId}' AND event_type = 'UserQuestionAnswered' AND payload->>'tool_use_id' = '${toolUseId}'`),
        { intervals: [400], timeout: 10_000 },
      ).toBe('1');

      // Card flips to answered state. Same dual-render caveat as above.
      const answered = page.locator('.cc-question-card-answered:visible').first();
      await expect(answered).toBeVisible({ timeout: 10_000 });
      await expect(answered).toContainText(`Answered: No ${suffix}`);
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
