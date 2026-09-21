import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy } from './helpers';
import { psql } from './db-helpers';

/** The transcript render window must leave the reader able to REACH what it
 *  left out. `threadWindow.fillAction` carries why the step budget alone
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

/** How one seeder writes a row: it owns the thread and the timestamp. */
type RowWriter = (type: string, payload: string) => string;

/** A call and its result, the pair every turn in this file is built from.
 *
 *  ONE copy, because the payload shape is the part that goes stale: a field the
 *  fold starts reading is easy to add to three seeders and miss in the
 *  fourth. */
function callPair(row: RowWriter, messageId: string, useId: string, n: number): string[] {
  return [
    row('CodingAgentToolCalled',
      `{"name":"Bash","args":{"command":"echo ${n}"},"description":"Run echo ${n}",` +
      `"channel":"claude_code","tool_use_id":"${useId}","coding_agent":"claude-code",` +
      `"request_event_id":"${messageId}"}`),
    row('CodingAgentToolResult',
      `{"name":"","result":"${n} done","channel":"claude_code","tool_use_id":"${useId}",` +
      `"coding_agent":"claude-code","request_event_id":"${messageId}"}`),
  ];
}

function seedBoundaryBehindHugeTurn(): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const now = new Date().toISOString();

  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const steps: string[] = [];
  for (let i = 0; i < TOOL_CALLS; i++) steps.push(...callPair(row, messageId, `e2e-tool-${i}`, i));

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

/** The text of the message opening every seeded turn, so a test can assert the
 *  reader reached it. */
const PROMPT_TEXT = 'do the work';

/** Pairs in a turn LONGER than one page of history (`THREAD_EVENTS_PAGE_SIZE`,
 *  400). The newest page then lies wholly inside the turn and carries no
 *  boundary, so the fold has nothing to hang the steps on. */
const TOOL_CALLS_PAST_A_PAGE = 260;

function seedTurnLongerThanAPage(): string {
  const threadId = randomUUID();
  const messageId = randomUUID();
  const now = new Date().toISOString();

  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const steps: string[] = [];
  for (let i = 0; i < TOOL_CALLS_PAST_A_PAGE; i++) {
    steps.push(...callPair(row, messageId, `e2e-paged-tool-${i}`, i));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E paged transcript', 'claude_code', '${now}', 1, false, true, 'idle', 'archived', 'active', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + [
      // The ONLY boundary, and it sits more than a page behind the newest event.
      `('${messageId}', 'MessageReceived', '{"text":"${PROMPT_TEXT}","mode":"human","channel":"claude_code"}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`,
      ...steps,
      row('ResponseGenerated', `{"text":"Done.","images":[],"request_event_id":"${messageId}"}`),
    ].join(',\n'),
  ].join(';\n'));

  return threadId;
}

/** Turns in the multi-page thread, and calls in each. Each call is a pair, so
 *  12 turns is 12 * (1 + 200 + 1) = 2,424 events, six pages of history at
 *  `THREAD_EVENTS_PAGE_SIZE`. */
const WALK_TURNS = 12;
const WALK_CALLS_PER_TURN = 100;

function seedThreadSeveralPagesLong(): string {
  const threadId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;

  for (let t = 0; t < WALK_TURNS; t++) {
    const messageId = randomUUID();
    const text = t === 0 ? FIRST_TURN_TEXT : `turn ${t}`;
    rows.push(`('${messageId}', 'MessageReceived', '{"text":"${text}","mode":"human","channel":"claude_code"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
    for (let i = 0; i < WALK_CALLS_PER_TURN; i++) {
      rows.push(...callPair(row, messageId, `walk-${t}-${i}`, i));
    }
    rows.push(row('ResponseGenerated', `{"text":"Done ${t}.","images":[],"request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E multi-page transcript', 'claude_code', '${new Date(base).toISOString()}', 1, false, true, 'idle', 'archived', 'active', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

/** The first turn's own message, pages behind anything a cold open loads. */
const FIRST_TURN_TEXT = 'the very first thing asked';

/** Call pairs in the big turn behind the page boundary, and in the short turn
 *  after it.
 *
 *  Sized off a reported thread of 419 events. Nineteen sit behind the newest
 *  page, and that page carries exactly ONE turn boundary. The fold gives a big
 *  *continuation fragment* under one short turn, and the seed takes the short
 *  turn alone. So the reader opens with nothing above the last message. */
const REPORTED_BIG_TURN_CALLS = 184;
const REPORTED_LAST_TURN_CALLS = 24;

/** The short turn's own message, the only boundary the newest page holds. */
const LAST_TURN_TEXT = 'and then this one';

function seedFragmentUnderOneShortTurn(): string {
  const threadId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row: RowWriter = (type, payload) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;

  const turn = (text: string, calls: number, tag: string, done: boolean) => {
    const messageId = randomUUID();
    rows.push(`('${messageId}', 'MessageReceived', '{"text":"${text}","mode":"human","channel":"claude_code"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
    for (let i = 0; i < calls; i++) rows.push(...callPair(row, messageId, `${tag}-${i}`, i));
    if (done) rows.push(row('ResponseGenerated', `{"text":"Done.","images":[],"request_event_id":"${messageId}"}`));
  };

  turn(FIRST_TURN_TEXT, REPORTED_BIG_TURN_CALLS, 'big', false);
  turn(LAST_TURN_TEXT, REPORTED_LAST_TURN_CALLS, 'last', true);

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E fragment under one short turn', 'claude_code', '${new Date(base).toISOString()}', 2, false, true, 'idle', 'archived', 'active', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  await navigateToApp(page);
}

/** One upward step, as a GESTURE.
 *
 *  Only a gesture retires the standing follow (ADR 0064). And only a gesture
 *  reaches a reader already pinned at the top, who fires no scroll event
 *  however hard they wheel (ADR 0232).
 *
 *  Playwright has no wheel in mobile WebKit and no drag on its touchscreen, so
 *  there a dispatched `WheelEvent` carries the negative delta the handler
 *  reads. It stamps the gesture exactly as a real one does, so the write that
 *  follows is the reader's. */
async function stepUp(page: Page, jump: number, wheelless: boolean): Promise<void> {
  if (!wheelless) {
    await page.mouse.wheel(0, -jump);
    return;
  }
  await page.locator('.thread-content').first().evaluate((el, d) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: -d, bubbles: true }));
    el.scrollTop = Math.max(0, el.scrollTop - d);
  }, jump);
}

/** Walk the transcript up until it holds `text`, and say whether it got there.
 *
 *  Bounded rather than open-ended: a walk that never arrives is the bug, and a
 *  poll would spend the whole test timeout saying so. */
async function walkUpUntilHeld(page: Page, text: string, wheelless: boolean): Promise<boolean> {
  const transcript = page.locator('.thread-content').first();
  const box = (await transcript.boundingBox())!;
  const jump = Math.round(box.height * 0.8);
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  for (let step = 0; step < 200; step++) {
    await stepUp(page, jump, wheelless);
    await page.waitForTimeout(60);
    if (await transcript.evaluate((el, t) => (el.textContent ?? '').includes(t), text)) return true;
  }
  return false;
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

  /** The same promise one layer out, where the transcript holds a PAGE rather
   *  than a thread. A cold open reads the newest 400 events, and this turn is
   *  longer than that, so the page carries no boundary at all. It folded to
   *  nothing, and the transcript called a healthy thread corrupt.
   *
   *  ADR 0230 gives that invariant TWO independent guarantees: the fold keeps
   *  the page's steps in a *continuation fragment*, and the fill fetches the
   *  page behind when the transcript still cannot scroll. This asserts the
   *  invariant, so it holds while either does and fails only if both break.
   *  Which mechanism ran is pinned by the unit tests beside each. */
  test('opens on a page that carries no turn boundary', async ({ page }) => {
    const threadId = seedTurnLongerThanAPage();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // Never the corrupt state: this thread is healthy, just paged.
    await expect(page.locator('.thread-empty-state')).toHaveCount(0);
    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'a page with no boundary must still fill the pane' },
    ).toBeGreaterThan(10);

    // The chevron is the manual way back. `.visible` is the offer itself: the
    // class is what `showUp` toggles, and it was false on the reported thread.
    const chevron = page.locator('.scroll-to-top.visible').first();
    await expect(chevron).toBeVisible();
    await chevron.click();

    // It must REACH the turn's own first message, which lives more than a page
    // behind anything this client opened with. Rendered is not reached:
    // `toBeVisible` is bounding-box based, so it passes the moment the message
    // exists anywhere in a 522-event list, with the reader still at the bottom.
    // The jump is the half that would go untested.
    const prompt = transcript.getByText(PROMPT_TEXT);
    await expect(prompt).toBeVisible({ timeout: 30_000 });
    await expect.poll(
      () => transcript.evaluate(el => el.scrollTop),
      { message: 'the chevron must land the reader at the top', timeout: 30_000 },
    ).toBeLessThan(200);
    const box = await prompt.boundingBox();
    const pane = await transcript.boundingBox();
    expect(box, 'the first message must be on screen').not.toBeNull();
    expect(box!.y).toBeLessThan(pane!.y + pane!.height);
  });

  /** The GESTURE, over a thread six pages long. A reader walking to the top
   *  must never come to rest against history that has not been asked for.
   *
   *  The unit tests pin each decision. Only this one runs the whole loop: the
   *  window grows, a gesture at the top asks for the next page, and the landed
   *  page re-points the edge without moving the reader.
   *
   *  It is also the only check that a reader pinned at the top can ask AGAIN.
   *  A container already there fires no scroll event, so before Phase 5 this
   *  walk froze after one page with five still on the server. */
  test('walks to the top of a thread several pages long', async ({ page, browserName, isMobile }) => {
    test.setTimeout(180_000);
    const threadId = seedThreadSeveralPagesLong();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible({ timeout: 60_000 });

    // The walk must be a GESTURE, which is why this drives input. A bare
    // `scrollTop` write is pinned back to the live edge and never leaves the
    // bottom, and that is the app being right. `stepUp` carries the rest.
    const wheelless = browserName === 'webkit' && isMobile;
    const reached = await walkUpUntilHeld(page, FIRST_TURN_TEXT, wheelless);
    expect(reached, 'walking up must reach the thread\'s first message').toBe(true);
  });

  /** The shape a reader reported: a thread of 419 events whose newest page
   *  carries exactly ONE boundary. The fold gives a big *continuation fragment*
   *  under one short turn, and the seed takes the short turn alone. So the
   *  thread opens with nothing above the last message.
   *
   *  They met it as the wheel doing nothing and the chevron working. Both
   *  halves are asserted. A transcript the reader cannot scroll is the first
   *  one: with nothing above the last turn and no scrollbar there is no gesture
   *  to make, and `fillAction` owes them one. Then the wheel itself, with the
   *  chevron never pressed. */
  test('reaches what is above a page that folds to one short turn', async ({ page, browserName, isMobile }) => {
    test.setTimeout(180_000);
    const threadId = seedFragmentUnderOneShortTurn();
    seededThreads.push(threadId);
    await openThread(page, threadId);

    const transcript = page.locator('.thread-content').first();
    await expect(transcript.getByText(LAST_TURN_TEXT).first()).toBeVisible({ timeout: 60_000 });

    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'the reader must have something to scroll' },
    ).toBeGreaterThan(10);

    const wheelless = browserName === 'webkit' && isMobile;
    const reached = await walkUpUntilHeld(page, FIRST_TURN_TEXT, wheelless);
    expect(reached, 'the wheel alone must reach the thread\'s first message').toBe(true);
  });
});
