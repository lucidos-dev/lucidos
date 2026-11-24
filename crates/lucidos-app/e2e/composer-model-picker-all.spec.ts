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
      await expect(trigger).toHaveAttribute('aria-disabled', 'false');
      await expect(trigger).toHaveAttribute('aria-label', /Model.*\S/);
      const before = await picker.boundingBox();
      await trigger.click();
      await expect(page.locator('.control-dropdown:visible')).toBeVisible();
      await page.keyboard.press('Escape');
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

for (const width of [390, 1440]) {
  test(`a mid-turn rung pick stays pending across reload at ${width}px`, async ({ page, request }) => {
    await assertHealthy(page);
    const threadId = randomUUID();
    const now = new Date().toISOString();
    const event = (type: string, payload: unknown) =>
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${randomUUID()}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;
    psql([
      `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Pending rung regression', 'chat', '${now}', 1, false, true, 'running', 'inbox', 'active', false, 0, 0, 'none', false)`,
      event('MessageReceived', { text: 'Pending rung regression', channel: 'chat', model: 'claude-opus-5-5', reasoning_effort: 'medium' }),
      event('ChatTurnSettingsResolved', { channel: 'chat', model: 'claude-opus-5-5', reasoning_effort: 'medium', request_event_id: randomUUID() }),
    ].join(';\n'));
    try {
      await page.setViewportSize({ width, height: 900 });
      await page.addInitScript(tid => localStorage.setItem('lucidos-focused-thread', tid), threadId);
      await hideDashboardRail(page);
      await navigateToApp(page);
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      const pick = async (effort: string) => {
        const response = await request.post('/api/v1/chat/settings', {
          data: { thread_id: threadId, model: 'claude-opus-5-5', reasoning_effort: effort },
        });
        expect(response.ok(), await response.text()).toBe(true);
      };
      await pick('high');
      const trigger = page.locator('.rung-trigger:visible').first();
      await expect(trigger.locator('.rung-next-turn')).toHaveText('*');
      await expect(trigger).toHaveAttribute('aria-label', /Running:.*medium.*Next turn:.*high/);
      await page.reload();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      await expect(trigger.locator('.rung-next-turn')).toHaveText('*');
      const box = await trigger.boundingBox();
      expect(box!.x).toBeGreaterThanOrEqual(0);
      expect(box!.x + box!.width).toBeLessThanOrEqual(width);
      await trigger.click();
      const summary = page.locator('.rung-turn-summary:visible');
      await expect(summary).toContainText('Running: Opus 5.5 · medium');
      await expect(summary).toContainText('Next turn: Opus 5.5 · high');
      await pick('low');
      await expect(summary).toContainText('Next turn: Opus 5.5 · low');
      await expect(summary).toContainText('Running: Opus 5.5 · medium');
      await page.keyboard.press('Escape');
      psql(`UPDATE thread_summaries SET status = 'idle' WHERE thread_id = '${threadId}'`);
      await page.reload();
      await ensureOnThreadPane(page);
      await waitForPaneAtRest(page);
      await expect(trigger.locator('.rung-next-turn')).toHaveCount(0);
      await expect(trigger).toHaveAttribute('aria-label', /Opus 5.5, low/);
    } finally {
      cleanupCCThread(threadId);
    }
  });
}
