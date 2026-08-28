import { test, expect, Page } from './fixtures';
import { randomUUID } from 'crypto';
import { navigateToApp, assertHealthy, disarmFollowSeed } from './helpers';
import { psql } from './db-helpers';

/** A thread must reopen on the turn the reader parked on.
 *
 *  The transcript is WINDOWED and the window's top edge is session state, so a
 *  reload re-seeds it from the newest turns. A pixel offset recorded against a
 *  taller render is then out of reach, and the reader used to land at the top.
 *  A *reading position* names a turn instead (`hooks/useScrollMemory.ts`), and
 *  ThreadView walks the window up to it.
 *
 *  The assertion is the reader's own question: which turn is at the top of the
 *  transcript. Not `scrollTop`, which is exactly the number that stopped
 *  meaning the same thing between the two opens. */

/** Turns in the seeded thread, and steps in each. Enough turns that the reader
 *  can park well above the seed's slice, and enough steps that `STEP_BUDGET`
 *  (160) binds before `INITIAL_WINDOW` (20) does. */
const TURNS = 24;
const STEPS_PER_TURN = 8;

/** Which turn the reader parks on: old enough that the seeded window cannot
 *  hold it, so the walk has real work to do. */
const PARK_ON_TURN = 4;

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

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
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
 *  same thing between the two opens. */
async function restingOn(page: Page): Promise<{ id: string | null; relTop: number }> {
  return page.locator('.thread-content').first().evaluate((el) => {
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
 *  expansion runs off one, so every step here has to be a real move. */
async function nudge(page: Page, by: number): Promise<void> {
  await page.locator('.thread-content').first().evaluate((el, delta) => {
    el.scrollTop = Math.max(0, el.scrollTop + delta);
  }, by);
  await page.waitForTimeout(150);
}

test.describe('A thread reopens on the turn the reader parked on', () => {
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

  test('a reload lands on the same turn, not the top of a re-seeded window', async ({ page }) => {
    const { threadId, messageIds } = seedStepHeavyThread();
    seededThreads.push(threadId);

    await openThread(page, threadId);
    const transcript = page.locator('.thread-content').first();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // The thread opens at the TOP of its seeded slice, so there is nowhere to
    // scroll up from yet. Go to the end first, the way a reader catching up
    // does, and the walk back is then a real one.
    await nudge(page, 100_000);

    // Walk up until the reader is resting in the OLD half of the thread. That
    // is the half a tail-seeded window cannot hold, which is what makes the
    // reload a real test rather than a lucky one.
    await expect.poll(
      async () => {
        await nudge(page, -2400);
        const { id } = await restingOn(page);
        return id ? messageIds.indexOf(id) : TURNS;
      },
      { message: 'scrolling up must reach the older half of the thread', timeout: 60_000 },
    ).toBeLessThan(PARK_ON_TURN + 1);

    // Let the expansion, its anchor correction and the save debounce settle, so
    // what is recorded is what is on screen.
    await page.waitForTimeout(600);
    const parked = await restingOn(page);
    expect(parked.id, 'the reader must be resting on a turn').not.toBeNull();
    // Non-vacuity, both halves. The turn is old enough that a tail-seeded window
    // cannot hold it, so the reload has real work to do. And what was recorded
    // NAMES that turn: a pixel offset here is the bug, whatever the reload then
    // happens to land on.
    expect(messageIds.indexOf(parked.id!)).toBeLessThanOrEqual(PARK_ON_TURN);
    const recorded = await page.evaluate(
      (tid) => localStorage.getItem(`lucidos-scroll-thread-${tid}`), threadId);
    expect(recorded).toBe(`anchor:${parked.relTop}:${parked.id}`);

    await page.reload();
    await expect(transcript.locator('.chat-exchange').first()).toBeVisible();

    // The whole point. A re-seeded window renders only the newest few turns, so
    // the pixel offset the reader left is out of reach of it. The turn is not.
    await expect.poll(
      () => restingOn(page),
      { message: 'the reload must reopen exactly where the reader was', timeout: 60_000 },
    ).toEqual(parked);
  });
});
