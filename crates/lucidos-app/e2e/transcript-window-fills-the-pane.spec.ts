import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';

/** The transcript render window must leave the reader able to REACH what it
 *  left out. `threadWindow.windowNeedsFill` carries why the step budget alone
 *  cannot promise that; the arithmetic is unit-tested beside it.
 *
 *  This seeds the exact shape both reported threads had: a huge working turn,
 *  then a small `ChangeApplied` boundary as the newest exchange. The assertion
 *  is what the reader can DO, not what the window counted. The transcript
 *  scrolls, and more than the boundary is on screen. */

/** Tool-call pairs in the seeded working turn. Each pair is two steps, so this
 *  clears `STEP_BUDGET` (160) several times over and the seed can only take the
 *  boundary after it. */
const TOOL_CALLS = 120;

function seedBoundaryBehindHugeTurn(): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const now = new Date().toISOString();

  const row = (type: string, payload: string) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const steps: string[] = [];
  for (let i = 0; i < TOOL_CALLS; i++) {
    const useId = `e2e-tool-${i}`;
    steps.push(row('CodingAgentToolCalled',
      `{"name":"Bash","args":{"command":"echo step ${i}"},"description":"Run echo step ${i}",` +
      `"channel":"claude_code","tool_use_id":"${useId}","coding_agent":"claude-code",` +
      `"request_event_id":"${messageId}"}`));
    steps.push(row('CodingAgentToolResult',
      `{"name":"","result":"step ${i} done","channel":"claude_code","tool_use_id":"${useId}",` +
      `"coding_agent":"claude-code","request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E windowed transcript', 'claude_code', '${now}', 1, false, true, 'idle', 'archived', 'active', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + [
      // The working turn: one prompt carrying every tool call as a step.
      `('${messageId}', 'MessageReceived', '{"text":"do the work","mode":"human","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      ...steps,
      row('ResponseGenerated', `{"text":"Done.","images":[],"request_event_id":"${messageId}"}`),
      // The boundary. Its own exchange, and a small one: this is all the seed
      // takes, and on its own it cannot fill a pane.
      row('ChangeApplied', `{"change_id":"${randomUUID()}","commits":["fix: e2e"],"client_update":false}`),
    ].join(',\n'),
  ].join(';\n'));

  return threadId;
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
}

test.describe('Windowed transcript', () => {
  const seededThreads: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededThreads.length = 0;
  });

  test.afterEach(() => {
    if (seededThreads.length === 0) return;
    const ids = seededThreads.map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('fills the pane when the seeded slice cannot', async ({ page }) => {
    const threadId = seedBoundaryBehindHugeTurn();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // Both halves of the invariant, and the first is the reported symptom.
    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'the transcript must be scrollable while turns sit above the window' },
    ).toBeGreaterThan(10);
    await expect(transcript.locator('.chat-exchange')).toHaveCount(2);
  });
});
