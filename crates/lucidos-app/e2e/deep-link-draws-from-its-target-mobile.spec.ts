import { test, expect } from './fixtures';
import { randomUUID } from 'crypto';
import {
  apiRequest, assertHealthy, clickVisibleElement, countExchanges, ensureMobileView,
  navigateToApp, waitForVisibleElement,
} from './helpers';
import { clearNotifications, psql } from './db-helpers';

/** Three pages of events (`THREAD_EVENTS_PAGE_SIZE` is 400, two per exchange
 *  here), so the thread opens on its newest page. */
const EXCHANGES = 600;

/** The linked turn sits on the page behind the newest one, half way up. */
const TARGET = 300;

function seedLongThread(): { threadId: string; targetId: string } {
  const threadId = randomUUID();
  let targetId = '';
  const base = Date.now() - EXCHANGES * 3000;
  const text = 'Seeded text to make the thread tall enough to scroll. '.repeat(3);
  const at = (k: number) => new Date(base + k * 1000).toISOString();
  const row = (id: string, type: string, payload: unknown, created: string) =>
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${id}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`;
  const stmts: string[] = [];
  for (let i = 0; i < EXCHANGES; i++) {
    const messageId = randomUUID();
    if (i === TARGET) targetId = messageId;
    stmts.push(
      row(messageId, 'MessageReceived', { text: `Message ${i + 1}: ${text}`, channel: 'chat' }, at(2 * i)),
      row(randomUUID(), 'ResponseGenerated', { text: 'Response.', images: [] }, at(2 * i + 1)),
    );
  }
  const last = at(2 * EXCHANGES);
  stmts.unshift(
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', 'Long linked thread', 'chat', '${last}', ${EXCHANGES}, false, true, 'completed', 'inbox', 'active', false, 0, 0, 'none', false)`,
  );
  psql(stmts.join(';\n'));
  return { threadId, targetId };
}

test.describe('A deep link into an old turn of a long thread', () => {
  let threadId: string | null = null;

  test.afterEach(() => {
    if (!threadId) return;
    psql([
      `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
      `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
    ].join(';\n'));
    threadId = null;
  });

  test('draws from the target turn, never the whole thread, and stays on it', async ({ page }) => {
    // The link claims the target's turn (ADR 0631). Turns older than it stay
    // undrawn until the reader scrolls up. A render-all drew all 600, which
    // is what got the iOS page killed.
    await assertHealthy(page);
    clearNotifications();
    const seeded = seedLongThread();
    threadId = seeded.threadId;

    await navigateToApp(page);
    const res = await apiRequest(page).post('/api/v1/notifications', {
      headers: { 'content-type': 'application/json' },
      data: { title: 'Old turn', message: 'tap to read', thread_id: seeded.threadId, event_id: seeded.targetId },
    });
    expect(res.ok(), `POST /api/v1/notifications -> ${res.status()}`).toBeTruthy();

    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.notifications-bell');
    await waitForVisibleElement(page, '.notification-item', 10_000);
    await clickVisibleElement(page, '.notification-item', 'Old turn');

    await ensureMobileView(page, 'thread');
    const target = `[data-event-id="${seeded.targetId}"]`;
    await waitForVisibleElement(page, target, 20_000);

    const state = () => page.evaluate((selector) => {
      const visible = (el: Element) => el.getBoundingClientRect().height > 0;
      const scroller = Array.from(document.querySelectorAll('.thread-content')).find(visible);
      const turn = Array.from(document.querySelectorAll(selector)).find(visible);
      const firstDrawn = Array.from(document.querySelectorAll('.chat-exchange')).find(visible);
      const s = scroller?.getBoundingClientRect();
      const t = turn?.getBoundingClientRect();
      return {
        inView: !!s && !!t && t.bottom > s.top && t.top < s.bottom,
        firstDrawnText: firstDrawn?.textContent?.slice(0, 200) ?? null,
      };
    }, target);

    let last: unknown = null;
    const inView = async () => {
      const now = await state();
      last = now;
      return now.inView;
    };
    await expect.poll(inView, { timeout: 5_000 }).toBe(true).catch((error: Error) => {
      throw new Error(`target not in view, last state: ${JSON.stringify(last)}\n\n${error.message}`);
    });

    // The claim releases about a second after landing. The window must keep
    // its floor at the target, and nothing may carry the reader off it.
    await page.waitForTimeout(3000);
    expect(await inView(), `target left view, state: ${JSON.stringify(last)}`).toBe(true);
    const drawn = await countExchanges(page);
    expect(drawn, 'the link drew turns older than its target').toBeLessThanOrEqual(EXCHANGES - TARGET);
    expect((await state()).firstDrawnText, 'the first drawn turn is not the target').toContain(`Message ${TARGET + 1}:`);
  });
});
