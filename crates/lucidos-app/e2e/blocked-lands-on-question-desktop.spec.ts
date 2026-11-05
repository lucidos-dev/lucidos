import { test, expect, type Page } from './fixtures';
import { randomUUID } from 'crypto';
import { assertHealthy, navigateToApp, openThreadDrawer } from './helpers';
import { clearAllThreads, psql } from './db-helpers';

/** Enough turns that the transcript scrolls well past one screen. */
const EXCHANGES = 30;

/** A thread parked on a question, its last turn. Returns the question's event
 *  id, which the question card's turn wears as `data-event-id`. */
function seedQuestionThread(threadId: string, title: string, lastAction: string): string {
  const questionId = randomUUID();
  const base = Date.parse(lastAction) - EXCHANGES * 3000;
  const text = 'Seeded text to make the thread tall enough to scroll. '.repeat(10);
  const at = (k: number) => new Date(base + k * 1000).toISOString();
  const row = (id: string, type: string, payload: unknown, created: string) =>
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ('${id}', '${type}', '${JSON.stringify(payload).replace(/'/g, "''")}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`;
  const stmts = [
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, last_user_action, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_change_state, coding_agent_is_external_repo) VALUES ('${threadId}', '${title}', 'chat', '${lastAction}', '${lastAction}', ${EXCHANGES}, false, true, 'waiting_for_user_answer', 'inbox', 'active', false, 0, 0, 'none', false)`,
  ];
  for (let i = 0; i < EXCHANGES; i++) {
    stmts.push(
      row(randomUUID(), 'MessageReceived', { text: `Message ${i + 1}: ${text}`, channel: 'chat' }, at(2 * i)),
      row(randomUUID(), 'ResponseGenerated', { text: 'Response.', images: [] }, at(2 * i + 1)),
    );
  }
  stmts.push(row(questionId, 'UserQuestionAsked', {
    tool_use_id: `tu-${questionId.slice(0, 8)}`,
    question: `${title} question?`,
    options: [{ id: 'a', label: 'Yes' }, { id: 'b', label: 'No' }],
  }, at(2 * EXCHANGES)));
  psql(stmts.join(';\n'));
  return questionId;
}

function visibleScroller(page: Page) {
  return page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find((c) => c.getBoundingClientRect().height > 0);
    return el ? { top: el.scrollTop, max: el.scrollHeight - el.clientHeight } : null;
  });
}

/** Is the question card's turn top inside the transcript's viewport? */
function questionInView(page: Page, eventId: string) {
  return page.evaluate((eid) => {
    const scroller = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find((el) => el.getBoundingClientRect().height > 0);
    const card = Array.from(document.querySelectorAll<HTMLElement>(`[data-event-id="${eid}"]`))
      .find((el) => el.getBoundingClientRect().height > 0);
    if (!scroller || !card) return false;
    const s = scroller.getBoundingClientRect();
    const q = card.getBoundingClientRect();
    return q.top >= s.top - 1 && q.top < s.bottom;
  }, eventId);
}

async function scrollTranscriptToTop(page: Page) {
  const box = await page.locator('.thread-content:visible').first().boundingBox();
  if (!box) throw new Error('no visible transcript');
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.wheel(0, -100_000);
  await expect.poll(async () => (await visibleScroller(page))?.top).toBe(0);
  // Past the scroll memory's save debounce.
  await page.waitForTimeout(800);
}

async function expectLandsOnQuestion(page: Page, eventId: string) {
  await expect.poll(() => questionInView(page, eventId), { timeout: 6_000 }).toBe(true);
  await page.waitForTimeout(1500);
  expect(await questionInView(page, eventId)).toBe(true);
}

test.describe('Opening a thread from Blocked lands on its question', () => {
  const ids = [randomUUID(), randomUUID(), randomUUID()];
  const questions: string[] = [];
  const row = (page: Page, id: string) => page.locator(`.thread-row[data-thread-nav="${id}"]:visible`);

  test.beforeEach(async ({ page, context }) => {
    await assertHealthy(page);
    clearAllThreads();
    await context.addInitScript(() => {
      localStorage.setItem('lucidos-drawer-grouping', 'ongoing');
      localStorage.setItem('lucidos-drawer-selected-ongoing-group', 'blocked');
    });
    const now = Date.now();
    questions.length = 0;
    ids.forEach((id, i) => {
      questions.push(seedQuestionThread(id, `blocked-${i}`, new Date(now - i * 60_000).toISOString()));
    });
    await navigateToApp(page);
    await openThreadDrawer(page);
    await expect(row(page, ids[2])).toBeVisible();
  });

  test.afterEach(() => {
    psql([
      `DELETE FROM events WHERE aggregate_id IN ('${ids.join("','")}')`,
      `DELETE FROM thread_summaries WHERE thread_id IN ('${ids.join("','")}')`,
    ].join(';\n'));
  });

  test('a first open', async ({ page }) => {
    await row(page, ids[0]).click();
    await expectLandsOnQuestion(page, questions[0]);
  });

  test('a revisit after the reader scrolled up', async ({ page }) => {
    await row(page, ids[0]).click();
    await expectLandsOnQuestion(page, questions[0]);
    await scrollTranscriptToTop(page);
    await row(page, ids[1]).click();
    await expectLandsOnQuestion(page, questions[1]);
    await row(page, ids[0]).click();
    await expectLandsOnQuestion(page, questions[0]);
  });

  test('a tap on the open row after the reader scrolled up', async ({ page }) => {
    await row(page, ids[0]).click();
    await expectLandsOnQuestion(page, questions[0]);
    await scrollTranscriptToTop(page);
    await row(page, ids[0]).click();
    await expectLandsOnQuestion(page, questions[0]);
  });

  test('the next row after an archive', async ({ page }) => {
    await row(page, ids[2]).click();
    await expectLandsOnQuestion(page, questions[2]);
    await scrollTranscriptToTop(page);
    // A thread waiting on a question offers no Archive, so the middle row is a
    // failed thread instead.
    psql([
      `DELETE FROM events WHERE aggregate_id = '${ids[1]}'`,
      `UPDATE thread_summaries SET status = 'failed' WHERE thread_id = '${ids[1]}'`,
    ].join(';\n'));
    await page.reload();
    await openThreadDrawer(page);
    await row(page, ids[1]).click();
    await expect(row(page, ids[1])).toHaveClass(/thread-row-focused/);
    await page.locator('button[aria-label="Archive thread"]:visible').first().click();
    await expect(row(page, ids[2])).toHaveClass(/thread-row-focused/);
    await expectLandsOnQuestion(page, questions[2]);
  });
});
