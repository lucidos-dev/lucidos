import { randomUUID } from 'crypto';
import { test, expect } from './fixtures';
import { assertHealthy, navigateToApp, ensureOnThreadPane, waitForPaneAtRest, hideDashboardRail, isMobileViewport } from './helpers';
import { psql, cleanupCCThread } from './db-helpers';

for (const status of ['idle', 'running', 'waiting_for_user_answer'] as const) {
  test(`the composer keeps its model picker in place while ${status}`, async ({ page }) => {
    await assertHealthy(page);
    const threadId = randomUUID();
    const now = new Date().toISOString();
    const event = (type: string, payload: unknown) =>
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Model picker regression', 'chat', '${now}', 1, false, true, '${status}', 'inbox', 'active', false, 0, 0, 'none', false)`,
      event('MessageReceived', { text: 'Model picker regression', channel: 'chat' }),
      ...(status === 'waiting_for_user_answer' ? [event('UserQuestionAsked', {
        tool_use_id: 'picker-question', question: 'Which option?',
        options: [{ id: 'a', label: 'Option A' }, { id: 'b', label: 'Option B' }],
      })] : status === 'idle' ? [event('ResponseGenerated', { text: 'Done', images: [] })] : []),
    ].join(';\n'));
    try {
      await page.addInitScript((tid) => localStorage.setItem('lucidos-focused-thread', tid), threadId);
      if (!isMobileViewport(page)) await page.setViewportSize({ width: 2600, height: 1000 });
      await hideDashboardRail(page);
      await navigateToApp(page);
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      const picker = page.locator('.rung-control:visible');
      const trigger = picker.locator('.rung-trigger');
      await expect(picker).toHaveCount(1);
      await expect(picker).not.toHaveAttribute('data-pending');
      // A running turn keeps the list openable and refuses the pick. A pending
      // question card disables the pill until it is answered.
      const answering = status === 'waiting_for_user_answer';
      await expect(trigger).toHaveAttribute('aria-disabled', String(answering));
      await expect(trigger).toHaveAttribute('aria-label', /Model.*\S/);
      const before = await picker.boundingBox();
      if (status === 'running') await expect(trigger).toHaveAttribute('data-tooltip', /turn runs/);
      if (answering) {
        await expect(trigger).toHaveAttribute('data-tooltip', /pending question/);
        await trigger.click({ force: true });
        await expect(page.locator('.control-dropdown:visible')).toHaveCount(0);
      } else {
        await trigger.click();
        await expect(page.locator('.control-dropdown:visible')).toBeVisible();
        await page.keyboard.press('Escape');
      }
      await expect(trigger).toHaveAttribute('aria-expanded', 'false');
      const after = await picker.boundingBox();
      expect(after!.width).toBeCloseTo(before!.width, 1);
      expect(after!.x).toBeCloseTo(before!.x, 1);
      expect(after!.x + after!.width).toBeLessThanOrEqual(page.viewportSize()!.width);
    } finally {
      cleanupCCThread(threadId);
    }
  });
}
