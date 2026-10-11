import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import {
  apiRequest, assertHealthy, clickVisibleElement, ensureMobileView,
  navigateToApp, waitForExchangeCount, waitForVisibleElement,
} from './helpers';
import { clearNotifications, psql } from './db-helpers';

/** More exchanges than one page of events holds (`THREAD_EVENTS_PAGE_SIZE` is
 *  400 events, two per exchange here). So the thread opens on its newest page,
 *  and the deep link's render-all fetches the rest behind it. */
const EXCHANGES = 260;

/** A long thread waiting on a question, which is its last turn. Returns the
 *  thread and the `UserQuestionAsked` a question notification points at. */
function seedLongQuestionThread(): { threadId: string; questionId: string } {
  const threadId = randomUUID();
  const questionId = randomUUID();
  const base = Date.now() - EXCHANGES * 3000;
  const text = 'Seeded text to make the thread tall enough to scroll. '.repeat(10);
  const at = (k: number) => new Date(base + k * 1000).toISOString();
  const row = (id: string, type: string, payload: unknown, created: string) =>
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${id}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`;
  const stmts: string[] = [];
  for (let i = 0; i < EXCHANGES; i++) {
    stmts.push(
      row(randomUUID(), 'MessageReceived', { text: `Message ${i + 1}: ${text}`, channel: 'chat' }, at(2 * i)),
      row(randomUUID(), 'ResponseGenerated', { text: 'Response.', images: [] }, at(2 * i + 1)),
    );
  }
  const asked = at(2 * EXCHANGES);
  stmts.push(row(questionId, 'UserQuestionAsked', {
    tool_use_id: `tu-${questionId.slice(0, 8)}`,
    question: 'Long thread question?',
    options: [{ id: 'a', label: 'Yes' }, { id: 'b', label: 'No' }],
  }, asked));
  stmts.unshift(
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Long question thread', 'chat', '${asked}', ${EXCHANGES}, false, true, 'waiting_for_user_answer', 'inbox', 'active', false, 0, 0, 'none', false)`,
  );
  psql(stmts.join(';\n'));
  return { threadId, questionId };
}

test.describe('A question notification into a long, unloaded thread', () => {
  let threadId: string | null = null;

  test.afterEach(() => {
    if (!threadId) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
    ].join(';\n'));
    threadId = null;
  });

  test('lands on the question and stays there when the older history folds in', async ({ page }) => {
    // The link lands and the ride glides to the question at the live edge. The
    // older history then folds in above, and its hold corrects the offset
    // mid-glide. The glide must carry on to the question. Stopped short, it
    // leaves the reader tens of thousands of pixels up in old turns.
    await assertHealthy(page);
    clearNotifications();
    const seeded = seedLongQuestionThread();
    threadId = seeded.threadId;

    await navigateToApp(page);
    const res = await apiRequest(page).post('/api/v1/notifications', {
      headers: { 'content-type': 'application/json' },
      data: { title: 'Long thread question', message: 'tap to answer', thread_id: seeded.threadId, event_id: seeded.questionId },
    });
    expect(res.ok(), `POST /api/v1/notifications -> ${res.status()}`).toBeTruthy();

    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item', 'Long thread question');

    // The whole history has folded in: every turn plus the question.
    await ensureMobileView(page, 'thread');
    await waitForExchangeCount(page, EXCHANGES + 1, 20_000);

    // Where the question sits inside the transcript's viewport, once every
    // glide has had time to end. Polled, so it passes as soon as it settles
    // there, and a reader stranded above it fails.
    const questionInView = () => page.evaluate((eid) => {
      const scroller = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
        .find((el) => el.getBoundingClientRect().height > 0);
      const question = Array.from(document.querySelectorAll<HTMLElement>(`[data-event-id="${eid}"]`))
        .find((el) => el.getBoundingClientRect().height > 0);
      if (!scroller || !question) return false;
      const s = scroller.getBoundingClientRect();
      const q = question.getBoundingClientRect();
      return q.top >= s.top - 1 && q.top < s.bottom;
    }, seeded.questionId);
    await expect.poll(questionInView, { timeout: 5_000 }).toBe(true);
    // And it stays: nothing later carries the reader off it.
    await page.waitForTimeout(1500);
    expect(await questionInView()).toBe(true);
  });
});
