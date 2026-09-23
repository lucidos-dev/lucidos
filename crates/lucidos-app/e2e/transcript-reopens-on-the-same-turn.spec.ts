import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, disarmFollowSeed } from './helpers';
import { psql } from './db-helpers';

/** WHERE a thread reopens, over one seeded thread longer than a page.
 *
 *  The transcript is WINDOWED and the window's top edge is session state, so a
 *  reload re-seeds it from the newest turns. A pixel offset recorded against a
 *  taller render is then out of reach, and the reader used to land at the top.
 *  A *reading position* names a turn instead (`hooks/useScrollMemory.ts`), and
 *  ThreadView walks the window up to it.
 *
 *  The transcript is also PAGED (ADR 0230), and that bounds the promise. A turn
 *  inside the loaded pages is restored exactly. A turn BEHIND them is not
 *  chased: the thread opens at the top of the newest page and fetches nothing
 *  (ADR 0234). One test each, and the second is the guard against the chase
 *  being added later. A third covers a turn the thread SHRANK under since the
 *  save, which lands on the nearest reachable offset at once.
 *
 *  The assertion is the reader's own question: which turn is at the top of the
 *  transcript. Not `scrollTop`, which is exactly the number that stopped
 *  meaning the same thing between the two opens. */

/** Turns in the seeded thread, and steps in each. Enough turns that the reader
 *  can park well above the seed's slice, and enough steps that `STEP_BUDGET`
 *  (160) binds before `INITIAL_WINDOW` (20) does. */
const TURNS = 24;
const STEPS_PER_TURN = 8;

/** Events one seeded turn carries: its message, a call and a result for each
 *  step, and the response. */
const EVENTS_PER_TURN = 2 + STEPS_PER_TURN * 2;

/** One page of history: `THREAD_EVENTS_PAGE_SIZE` in
 *  `store/actions/thread-loading.ts`. Restated rather than imported, that
 *  module pulling in the whole store. The `beforeEach` below checks the seed
 *  against it, so a fixture edit cannot silently make either test vacuous. */
const PAGE_SIZE = 400;

/** Oldest turns a cold open does not load, whole or in part. The seed is 432
 *  events against a page of 400, so 32 stay on the server: turn 0 entire, and
 *  all but the tail of turn 1. */
const TURNS_BEHIND_THE_PAGE =
  Math.ceil(Math.max(0, TURNS * EVENTS_PER_TURN - PAGE_SIZE) / EVENTS_PER_TURN);

/** Which turn the reader parks on for the in-page test. Two constraints, and it
 *  has to satisfy both. Old enough that a re-seeded window cannot hold it, so
 *  the walk has real work. Inside the newest page, so the position is one the
 *  app still honours. */
const PARK_ON_TURN = 4;

/** How long to watch before concluding nothing will fetch older history.
 *
 *  A restore that cannot resolve its turn gives up after `RESTORE_DEADLINE_MS`
 *  (3s in `hooks/useScrollMemory.ts`), re-arming only while the transcript is
 *  still growing under it. A chase would start inside that window. The margin
 *  on top covers a slow cold open. */
const NO_CHASE_WINDOW_MS = 5_000;

function seedStepHeavyThread(): { threadId: string; messageIds: string[] } {
  const threadId = randomUUID();
  const now = new Date().toISOString();
  const messageIds: string[] = [];

  const row = (type: string, payload: string) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${now}', 'thread', '${threadId}', '${threadId}')`;

  const rows: string[] = [];
  for (let t = 0; t < TURNS; t++) {
    const messageId = randomUUID();
    messageIds.push(messageId);
    rows.push(`('${messageId}', 'MessageReceived', ` +
      `'{"text":"turn ${t}","mode":"human","channel":"claude_code"}'::jsonb, ` +
      `'${now}', 'thread', '${threadId}', '${threadId}')`);
    for (let s = 0; s < STEPS_PER_TURN; s++) {
      const useId = `e2e-${t}-${s}`;
      rows.push(row('CodingAgentToolCalled',
        `{"name":"Bash","args":{"command":"echo ${t}.${s}"},"description":"Run echo ${t}.${s}",` +
        `"channel":"claude_code","tool_use_id":"${useId}","coding_agent":"claude-code",` +
        `"request_event_id":"${messageId}"}`));
      rows.push(row('CodingAgentToolResult',
        `{"name":"","result":"${t}.${s} done","channel":"claude_code","tool_use_id":"${useId}",` +
        `"coding_agent":"claude-code","request_event_id":"${messageId}"}`));
    }
    rows.push(row('ResponseGenerated',
      `{"text":"Finished turn ${t}.","images":[],"request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E reopen on the same turn', 'claude_code', '${now}', ${TURNS}, false, true, 'idle', 'archived', 'active', true, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));

  return { threadId, messageIds };
}

/** The key a thread's *reading position* is recorded under
 *  (`threadScrollKey`). */
function scrollKey(threadId: string): string {
  return `lucidos-scroll-thread-${threadId}`;
}

/** Open the thread, optionally with a reading position already recorded.
 *
 *  `reading` is the stored form the app itself writes, which the first test
 *  asserts verbatim. So seeding one here is the same value under the same key,
 *  rather than a shape this file invented. */
async function openThread(page: Page, threadId: string, reading?: string): Promise<void> {
  // The key is computed HERE and handed over. The init script runs in the
  // browser, where `scrollKey` does not exist, and a second literal is how the
  // two spellings drift.
  await page.addInitScript(([tid, key, rec]: [string, string, string | null]) => {
    localStorage.setItem('lucidos-focused-thread', tid);
    if (rec !== null) localStorage.setItem(key, rec);
  }, [threadId, scrollKey(threadId), reading ?? null] as [string, string, string | null]);
  // The *follow seed* ships armed, and it speaks for a thread with no reading
  // position. That is the FIRST open here, which must start somewhere the
  // reader can then scroll up from rather than riding the live edge.
  await disarmFollowSeed(page);
  await navigateToApp(page);
}

/** Where the reader is resting, as the app itself would record it: the last turn
 *  whose top is at or above the container's, and that turn's exact offset. The
 *  same rule as `readScrollAnchor`, restated here so the spec asserts the
 *  reader's own question rather than trusting the code under test.
 *
 *  Deliberately NOT `scrollTop`, which is the number that stops meaning the
 *  same thing between the two opens.
 *
 *  Read in a frame with no WebKit repaint nudge (`utils/webkitRepaint.ts`). A
 *  nudge moves the content a pixel and translates the container back over it,
 *  so the reader sees nothing move. An offset measured from the container's
 *  box in that frame is a pixel off all the same. */
async function restingOn(page: Page): Promise<{ id: string | null; relTop: number }> {
  return page.locator('.thread-content').first().evaluate(async (el) => {
    for (let frame = 0; frame < 60 && el.style.transform.includes('translateZ'); frame++) {
      await new Promise(requestAnimationFrame);
    }
    const top = el.getBoundingClientRect().top;
    const turns = Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange'));
    let earliest: { id: string | null; relTop: number } | null = null;
    // Backwards, like the rule it mirrors. A reader sitting ABOVE the first turn
    // still rests on that turn, at a POSITIVE offset: the transcript's own top
    // padding puts one there at `scrollTop` zero, which is an ordinary place to
    // be and not "nowhere".
    for (let i = turns.length - 1; i >= 0; i--) {
      const rect = turns[i].getBoundingClientRect();
      if (rect.height <= 0) continue;
      // `+ 0` normalizes the negative zero `Math.round` answers for a turn a
      // fraction of a pixel above the line, which a deep equality tells apart.
      const relTop = Math.round(rect.top - top) + 0;
      const id = turns[i].getAttribute('data-event-id');
      if (relTop <= 0) return { id, relTop };
      earliest = { id, relTop };
    }
    return earliest ?? { id: null, relTop: 0 };
  });
}

/** Scroll `by` pixels and let the window expansion, its anchor correction and
 *  the save debounce settle.
 *
 *  A write that does not MOVE the container fires no scroll event. The window
 *  expansion runs off one, so every step here has to be a real move. The wheel
 *  event beside it covers the one step that cannot be. A reader already pinned
 *  at the top asks for the page behind by GESTURE (ADR 0232). A walk that only
 *  wrote `scrollTop` would wedge there, with history still on the server. */
async function nudge(page: Page, by: number): Promise<void> {
  await page.locator('.thread-content').first().evaluate((el, delta) => {
    el.dispatchEvent(new WheelEvent('wheel', { deltaY: delta, bubbles: true }));
    el.scrollTop = Math.max(0, el.scrollTop + delta);
  }, by);
  await page.waitForTimeout(150);
}

/** How far ABOVE the container's top to put the parked turn's own top.
 *
 *  Above rather than exactly on, and that is the whole point of the offset.
 *  This file and `readScrollAnchor` both name the last turn at or above the
 *  line, and both round to whole pixels. A device pixel is a third of a CSS one
 *  at 3x, so a turn resting exactly on the line rounds either way. The answer
 *  then becomes the turn before it, which is what mobile-webkit did. */
const PARK_ABOVE_THE_LINE_PX = 8;

/** Park the reader on one turn, at a known offset.
 *
 *  A walk that stops at "some turn old enough" cannot say WHICH. A 2400px step
 *  lands wherever it lands, and on this thread that can be past the page a
 *  reload loads. */
async function parkOn(page: Page, eventId: string): Promise<void> {
  await page.locator('.thread-content').first().evaluate((el, [id, above]: [string, number]) => {
    const turn = el.querySelector<HTMLElement>(`[data-event-id="${id}"]`);
    if (!turn) throw new Error(`the turn to park on is not rendered: ${id}`);
    el.scrollTop += turn.getBoundingClientRect().top - el.getBoundingClientRect().top + above;
  }, [eventId, PARK_ABOVE_THE_LINE_PX] as [string, number]);
  await page.waitForTimeout(150);
}

/** Which seeded turn the reader is resting on, or -1 for none of them.
 *
 *  An INDEX rather than an id, so a failure names the turn instead of a uuid
 *  nobody can place. */
async function restingTurn(page: Page, messageIds: string[]): Promise<number> {
  const at = await restingOn(page);
  return messageIds.indexOf(at.id ?? '');
}

/** Is this request a read of history OLDER than the page a cold open gets?
 *
 *  Two shapes, and a chase would use either. A backfill pages backwards with
 *  `before_created` (`api/threads.ts`). The whole-thread read asks for no
 *  `limit` at all. A catch-up read carries `after` and is neither. */
function isOlderHistoryRead(url: string, threadId: string): boolean {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return false;
  }
  if (!parsed.pathname.endsWith(`/threads/${threadId}/events`)) return false;
  if (parsed.searchParams.has('before_created')) return true;
  return !parsed.searchParams.has('limit') && !parsed.searchParams.has('after');
}

test.describe('Where a thread reopens', () => {
  const seededThreads: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    seededThreads.length = 0;
    // The fixture is load-bearing in both directions, so it is CHECKED rather
    // than described. A seed inside one page would make the second test
    // vacuous. A parked turn among the oldest ones would make the first test
    // assert what the app no longer does.
    expect(TURNS * EVENTS_PER_TURN, 'the seed must be longer than one page')
      .toBeGreaterThan(PAGE_SIZE);
    expect(PARK_ON_TURN, 'the parked turn must sit inside the newest page')
      .toBeGreaterThanOrEqual(TURNS_BEHIND_THE_PAGE);
  });

  test.afterEach(() => {
    if (seededThreads.length === 0) return;
    const ids = seededThreads.map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('a reload lands on the same turn, not the top of a re-seeded window', async ({ page }) => {
    test.setTimeout(180_000);
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    await openThread(page, threadId);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // Non-vacuity, asserted where it is checkable. The seed always takes the
    // NEWEST turn, so its arrival says the window is laid down. Only then is an
    // absent turn evidence rather than a race. The turn this test parks on must
    // be outside that window, or the reload has nothing to walk.
    await expect(transcript.locator(`[data-event-id="${messageIds[TURNS - 1]}"]`)).toHaveCount(1);
    await expect(transcript.locator(`[data-event-id="${messageIds[PARK_ON_TURN]}"]`)).toHaveCount(0);

    // The thread opens at the TOP of its seeded slice, so there is nowhere to
    // scroll up from yet. Go to the end first, the way a reader catching up
    // does, and the walk back is then a real one.
    await nudge(page, 100_000);

    // Walk all the way to the thread's FIRST turn. The page boundary lies two
    // turns in front of it. So this pulls the rest of the history in and grows
    // the window over every turn. Nothing can then arrive to move the reader
    // while the position below is being recorded.
    await expect.poll(
      async () => {
        await nudge(page, -2400);
        return transcript.locator(`[data-event-id="${messageIds[0]}"]`).count();
      },
      { message: 'scrolling up must reach the first turn', timeout: 90_000 },
    ).toBe(1);

    // Park EXACTLY, on a turn the newest page holds. Under test is a restore
    // landing on the same turn at the same offset. A poll that stops wherever a
    // 2400px step left the reader cannot assert that.
    //
    // Re-parked until it HOLDS. A grow armed by the last step of the walk
    // corrects the scroll a beat later, and that correction would otherwise
    // throw the park away.
    await expect.poll(
      async () => {
        await parkOn(page, messageIds[PARK_ON_TURN]);
        return restingTurn(page, messageIds);
      },
      { message: 'the reader must come to rest on the turn they are parked on', timeout: 30_000 },
    ).toBe(PARK_ON_TURN);

    // Let the expansion, its anchor correction and the save debounce settle, so
    // what is recorded is what is on screen.
    await page.waitForTimeout(600);
    const parked = await restingOn(page);
    expect(messageIds.indexOf(parked.id ?? ''), 'the park must hold while the save settles')
      .toBe(PARK_ON_TURN);
    // And what was recorded NAMES that turn: a pixel offset here is the bug,
    // whatever the reload then happens to land on.
    const recorded = await page.evaluate(
      (key) => localStorage.getItem(key), scrollKey(threadId));
    expect(recorded).toBe(`anchor:${parked.relTop}:${parked.id}`);

    await page.reload();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // The whole point. A re-seeded window renders only the newest few turns, so
    // the pixel offset the reader left is out of reach of it. The turn is not:
    // it sits inside the page this open loaded, and the walk reaches it.
    await expect.poll(
      () => restingTurn(page, messageIds),
      { message: 'the reload must reopen on the turn the reader parked on', timeout: 60_000 },
    ).toBe(PARK_ON_TURN);

    // And at the same offset. A pixel of slack, not more: both measurements
    // round, and a device pixel is a third of a CSS one at 3x.
    const restored = await restingOn(page);
    expect(Math.abs(restored.relTop - parked.relTop), 'and at the same offset')
      .toBeLessThanOrEqual(1);
  });

  /** The other side of the promise, and the decision behind it (ADR 0234).
   *
   *  A reader who walks far enough back leaves a position naming a turn the
   *  next open does not load. The app opens at the top of the newest page and
   *  fetches nothing to chase it. A chase spends exactly what paging bought,
   *  and this test is what stops one coming back.
   *
   *  The position is seeded rather than walked to. It is the same key and the
   *  same stored form the test above reads back off the app's own write. */
  test('a position behind the loaded page opens at the top of the newest page', async ({ page }) => {
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    let olderReads = 0;
    // `page.on('request')` rather than `page.route()`: the service worker
    // re-issues GET /api/v1/* itself, and route handlers do not see those.
    page.on('request', req => {
      if (isOlderHistoryRead(req.url(), threadId)) olderReads += 1;
    });

    // Turn 0 is wholly behind the page a cold open loads.
    const behindThePage = `anchor:0:${messageIds[0]}`;
    await openThread(page, threadId, behindThePage);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // The count below means "nothing chased" only if nothing else had a reason
    // to fetch. ADR 0230's escalation asks for a page when the transcript
    // cannot scroll, so rule that out by measurement rather than by argument.
    await expect.poll(
      () => transcript.evaluate(el => el.scrollHeight - el.clientHeight),
      { message: 'the newest page must fill the pane on its own' },
    ).toBeGreaterThan(10);

    // The restore waits before it gives up, so a chase would start inside that
    // window. Watch it out before concluding there is none.
    await page.waitForTimeout(NO_CHASE_WINDOW_MS);

    expect(olderReads, 'the open must not fetch history to chase the position').toBe(0);
    await expect(transcript.locator(`[data-event-id="${messageIds[0]}"]`)).toHaveCount(0);

    // The reader is somewhere inside the newest page, never behind it. WHERE
    // inside is the render window's business and is tested beside the window:
    // mobile-webkit takes a fill round chromium does not, which moves the top
    // of the drawn slice by several turns. What this test owns is the
    // give-up, and a chase would put the reader on turn 0.
    expect(await restingTurn(page, messageIds),
      'the open must rest on a turn the newest page holds')
      .toBeGreaterThanOrEqual(TURNS_BEHIND_THE_PAGE);

    // The record is KEPT, not retired. A client that does hold the history, or
    // a later open after a backfill, still lands the reader on their turn.
    const recorded = await page.evaluate(
      (key) => localStorage.getItem(key), scrollKey(threadId));
    expect(recorded, 'giving up must not destroy the reading position').toBe(behindThePage);
  });

  /** A position the thread SHRANK under lands at once, then holds still.
   *
   *  The reader parked at the bottom, and the content below their turn has got
   *  shorter since: a Thinking row folded, or the keyboard was open. The exact
   *  offset is out of reach, and no wait can bring it back. So the reader lands
   *  on the nearest reachable offset at once, and nothing moves them after. */
  test('a position the thread shrank under lands at once and holds still', async ({ page }) => {
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    // The newest turn, measured when far more content sat below its top.
    await openThread(page, threadId, `anchor:-4000:${messageIds[TURNS - 1]}`);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // Well inside `RESTORE_DEADLINE_MS` (3s), so only an immediate landing passes.
    await page.waitForTimeout(500);
    const landed = await transcript.evaluate(el => ({
      top: el.scrollTop,
      max: el.scrollHeight - el.clientHeight,
    }));
    expect(landed.max, 'the transcript must scroll, or landing proves nothing').toBeGreaterThan(10);
    expect(landed.top, 'the open must land on the reachable end at once')
      .toBeGreaterThanOrEqual(landed.max - 1);
    // The named turn is on screen. Not the turn at the top line: this one is
    // shorter than the pane, so the turn above it reaches the line.
    const onScreen = await transcript.evaluate((el, id) => {
      const turn = el.querySelector(`[data-event-id="${id}"]`)!.getBoundingClientRect();
      const view = el.getBoundingClientRect();
      return turn.top < view.bottom && turn.bottom > view.top;
    }, messageIds[TURNS - 1]);
    expect(onScreen, 'the turn the reader parked on must be on screen').toBe(true);

    // Past every restore deadline, the reader has not moved a pixel.
    const drift = await transcript.evaluate(async (el) => {
      const start = el.scrollTop;
      let worst = 0;
      const until = performance.now() + 4_000;
      while (performance.now() < until) {
        await new Promise(requestAnimationFrame);
        worst = Math.max(worst, Math.abs(el.scrollTop - start));
      }
      return worst;
    });
    expect(drift, 'nothing may move the transcript after it opens').toBeLessThanOrEqual(1);
  });
});
