import { test, expect, type Page } from './fixtures';
import { navigateToApp, assertHealthy, openThreadDrawer } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/** Which pane holds DOM focus, named so a failure says where Tab went. */
async function focusedPaneOf(page: Page): Promise<string> {
  return await page.evaluate(() => {
    const a = document.activeElement;
    if (!a) return 'nothing';
    if (a.closest('.thread-drawer')) return 'drawer';
    if (a.closest('.pane-thread')) return 'thread';
    if (a.closest('.pane-content')) return 'content';
    return a.tagName.toLowerCase();
  });
}

// Clicking a drawer row focuses the thread's live question card, so Enter
// answers at once. The per-pane Tab trap follows the focused-pane marker, and
// the click left that marker on the drawer. So Tab from the focused option
// jumped back into the drawer. The thread-entry focus now takes the marker.
//
// Desktop-only: mobile navigates panes and has no Tab trap.
test.describe('Thread entry focus: Tab stays in the thread pane', () => {
  test('Tab from a focused question option stays in the thread pane', async ({ page }) => {
    await assertHealthy(page);

    const suffix = randomUUID().slice(0, 8);
    const threadId = randomUUID();
    const now = new Date().toISOString();
    const payload = JSON.stringify({
      tool_use_id: `tu-tab-${suffix}`,
      cc_session_id: 'sess-e2e',
      question: `Tab question ${suffix}`,
      options: [
        { id: 'opt-0', label: `First ${suffix}` },
        { id: 'opt-1', label: `Second ${suffix}` },
      ],
    }).replace(/'/g, "''");

    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', 'Tab Entry E2E ${suffix}', 'claude_code', '${now}', 1, false, false, 'waiting_for_user_answer', 'inbox', true, 0)`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'MessageReceived', '{"text":"start","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'SessionStarted', '{"session_id":"sess-e2e"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', 'UserQuestionAsked', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
    ].join(';\n'));

    try {
      await navigateToApp(page);
      await openThreadDrawer(page);

      const row = page.locator(`.thread-row:has-text("Tab Entry E2E ${suffix}")`).first();
      await expect(row).toBeVisible({ timeout: 10_000 });
      await row.click();

      const first = page.locator('.pane-thread button', { hasText: `First ${suffix}` }).first();
      await expect(first).toBeFocused({ timeout: 10_000 });

      await page.keyboard.press('Tab');
      expect(await focusedPaneOf(page)).toBe('thread');

      await page.keyboard.press('Shift+Tab');
      expect(await focusedPaneOf(page)).toBe('thread');
    } finally {
      psql([
        `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
        `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
      ].join(';\n'));
    }
  });
});
