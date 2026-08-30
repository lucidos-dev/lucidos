import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, disableMobileHeaderSticky, disarmFollowSeed, enableMobileHeaderSticky, ensureOnThreadPane, navigateToApp, renderWholeTranscript, waitForScrollSettled } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * **The step-log control stays under the reader's finger.**
 *
 * One press changes the height of EVERY turn, including the ones ABOVE the
 * control. So the correction has to scroll by whatever those gained or lost, or
 * the thing the reader just pressed slides away from them.
 *
 * The cases here are the shapes that were reported, and the sweep in
 * `turn-control-holds-what-you-pressed.spec.ts` is the general one. The last
 * two keep the seeded ARM and press from the end of the thread, which is the
 * park the rule used to exempt.
 *
 * It runs in every project, WebKit included, which implements no scroll
 * anchoring of its own. Seeded through psql: the shape has to be tall turns of
 * alternating prose and steps, each line addressable.
 */

const TURNS = 8;
/** Prose-then-step groups per turn. Enough that one turn is several phone
 *  screens tall, which is what puts its top off the top of the viewport. */
const CHUNKS_PER_TURN = 14;
/** Tool calls per group. One is the interleaved shape, where a surviving line
 *  always sits beside the reader. `STEPS_PER_RUN` is the other shape. */
const STEPS_PER_CHUNK = 1;
/** Tool calls in an unbroken run, for the coding-agent shape: a turn that works
 *  for dozens of calls with nothing said between them. */
const STEPS_PER_RUN = 40;
/** How far the control may move, in CSS px. A correction is written to a whole
 *  pixel, and a sub-pixel row height can round the other way. */
const DRIFT_TOLERANCE_PX = 4;

/** A line the test can find again after the DOM has changed under it. */
const mark = (turn: number, chunk: number) => `MARK-${turn}-${chunk}`;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

/** Tall turns of prose and tool steps. Hiding the log takes a large bite out of
 *  each turn, and showing it puts the same bite back.
 *
 *  `stepsPerChunk` is how many calls follow each line of prose.
 *
 *  `proseLines` keeps the transcript scrollable once the log is hidden. A
 *  thread that collapses shorter than the viewport clamps every offset to zero,
 *  and the clamp would answer for the correction. The short-thread case below
 *  asks for exactly that, through `turns` and `proseLines`.
 *
 *  ZERO drops the chunk's prose row entirely rather than emptying it. A turn is
 *  then its own message, its steps and its answer, which is the smallest shape
 *  that still has something to reveal. The desktop pane is the tightest of the
 *  three, and the row it saves is what fits three turns inside it. */
function seedThread(
  title: string,
  { stepsPerChunk = STEPS_PER_CHUNK, chunks = CHUNKS_PER_TURN, proseLines = 1, turns = TURNS } = {},
): string {
  const threadId = randomUUID();
  const base = Date.now() - 600_000;
  const rows: string[] = [];
  let seq = 0;
  const ev = (type: string, payload: unknown) => {
    const created = new Date(base + seq * 1000).toISOString();
    seq++;
    rows.push(
      `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES ` +
      `('${randomUUID()}', '${type}', '${q(payload)}'::jsonb, '${created}', 'thread', '${threadId}', '${threadId}')`
    );
  };
  for (let t = 0; t < turns; t++) {
    ev('MessageReceived', { text: `Question number ${t}.`, channel: 'chat' });
    for (let c = 0; c < chunks; c++) {
      if (proseLines > 0) {
        const body = 'The quick brown fox jumps over the lazy dog. '.repeat(6 * proseLines);
        ev('TextStreamed', { text: `${mark(t, c)} part ${c}. ${body}` });
      }
      for (let s = 0; s < stepsPerChunk; s++) {
        ev('ToolCalled', {
          name: 'web_search',
          args: { query: `turn ${t} step ${c}.${s}` },
          description: `Search the web for turn ${t} step ${c}.${s}`,
        });
        ev('ToolResult', { name: 'web_search', result: `result ${t}.${c}.${s}` });
      }
    }
    const answer = `Answer to question ${t}.`;
    ev('TextStreamed', { text: answer });
    ev('ResponseGenerated', { text: answer, model: 'mock', channel: 'chat' });
  }
  const last = new Date(base + seq * 1000).toISOString();
  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${title}', 'chat', '${last}', ${turns}, false, true, 'idle', 'inbox', false, 0)`,
    ...rows,
  ].join(';\n'));
  return threadId;
}

function dropThread(threadId: string): void {
  psql([
    `DELETE FROM events WHERE aggregate_id = '${threadId}'`,
    `DELETE FROM thread_summaries WHERE thread_id = '${threadId}'`,
  ].join(';\n'));
}

async function openThread(page: Page, threadId: string, { disarm = true, renderAll = true } = {}): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  // The seed ships ARMED, and a rider is carried back to the live edge by any
  // scroll that is not their own gesture. Parking them anywhere would be undone
  // before the control was ever pressed. Most of this spec is about the reader
  // who is READING, so it starts them disarmed. The two live-edge cases keep the
  // seeded arm, to press from the end of a quiet thread with the ride still on.
  if (disarm) await disarmFollowSeed(page);
  // Hide-on-scroll LIVE rather than inherited: see `disableMobileHeaderSticky`.
  await disableMobileHeaderSticky(page);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-role="inline-step"]:visible').first()).toBeVisible({ timeout: 30_000 });
  // The transcript opens windowed, so the turns this spec parks in and presses
  // on are not all in the DOM yet. Ask for the whole thread the way a reader
  // does. Skipped where the case is about the tail, which is rendered already.
  if (renderAll) await renderWholeTranscript(page);
}

/** Which turn owns the row carrying `text`.
 *
 *  A row rather than the turn's own id, because the cases park deep inside a
 *  tall turn and name the line they park on. */
async function turnCarrying(page: Page, text: string): Promise<string> {
  return await page.evaluate((needle: string) => {
    // THE VISIBLE ONE. Mobile mounts every pane at once, and the compose view
    // reuses the class. So a bare `querySelector` can answer with a box nobody
    // is looking at, whose scroll geometry means nothing. Mirrors
    // `findVisibleThreadContent` in scrollState.ts. Repeated in each `evaluate`
    // below, none of which can share a closure with this one.
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    const row = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *'))
      .find(r => (r.textContent ?? '').includes(needle));
    if (!row) throw new Error(`no row carrying ${needle}`);
    const turn = row.closest('.chat-exchange') as HTMLElement | null;
    const seq = turn?.getAttribute('data-user-seq') ?? '';
    if (!seq) throw new Error(`the row carrying ${needle} is in no turn`);
    return seq;
  }, text);
}

/** The RESPONSE HEADER's own copy of a turn control.
 *
 *  Scoped, because `toggle-collapsed` is rendered TWICE in a turn: the
 *  initiator panel carries its own fold control under the same `data-role`,
 *  for a different fold (`collapsedInitiators`). An unscoped query answers with
 *  the initiator's, being first in document order. Press one and measure the
 *  other and the distance between them reads as drift.
 *
 *  These seeds cannot reach it, since `canCollapseInitiator` drops the control
 *  for a user-message bubble and every seeded turn is one. It is scoped anyway,
 *  because a seed that grows an initiator summary would spring the trap with
 *  nothing in the failure to point at it. */
function controlSelector(seq: string, role: string): string {
  return `.chat-exchange[data-user-seq="${seq}"] .response-header .turn-controls [data-role="${role}"]`;
}

function controlOn(page: Page, seq: string, role = 'toggle-steps') {
  return page.locator(controlSelector(seq, role)).first();
}


/** What `seq`'s turn control and the transcript are doing right now. One round
 *  trip, so a press is compared against one layout. `top` is the control's own
 *  position, which is the whole of what a press must not change. */
interface Geometry {
  top: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
}

async function geometry(page: Page, seq: string, role = 'toggle-steps'): Promise<Geometry> {
  return await page.evaluate(({ sel, r, s }: { sel: string; r: string; s: string }) => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    const btn = el.querySelector<HTMLElement>(sel);
    if (!btn) throw new Error(`no ${r} control on turn ${s}`);
    return {
      top: btn.getBoundingClientRect().top - el.getBoundingClientRect().top,
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
    };
  }, { sel: controlSelector(seq, role), r: role, s: seq });
}

/** Park the transcript so `seq`'s control sits halfway down it, which puts the
 *  reader's own topmost line in the turn ABOVE.
 *
 *  It CONVERGES rather than scrolling once by the measured delta. The chevron
 *  that rendered the whole transcript writes a second `scrollToTop` on the
 *  commit, which undoes a single write made before it. The mobile chrome the
 *  control is measured against also moves with the park's own scroll. */
async function parkControlMidScreen(page: Page, seq: string): Promise<void> {
  for (let round = 0; round < 5; round++) {
    const g = await geometry(page, seq);
    const want = g.clientHeight / 2;
    if (Math.abs(g.top - want) <= 1) return;
    await page.evaluate((d: number) => {
      const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
        .find(c => c.getBoundingClientRect().height > 0)!;
      el.scrollTop += d;
    }, g.top - want);
    await waitForScrollSettled(page);
  }
}

/** Press a turn control on `seq`'s own response header. Returns once the toggle
 *  reports its new state and the position has settled.
 *
 *  The click is DISPATCHED rather than driven through the pointer. Playwright
 *  scrolls a control into view before clicking it, and every case here has
 *  placed the transcript on purpose. */
async function pressControlOn(page: Page, seq: string, role: string, expected: 'true' | 'false'): Promise<void> {
  const toggle = controlOn(page, seq, role);
  await toggle.dispatchEvent('click');
  await expect(toggle).toHaveAttribute('aria-pressed', expected);
  await waitForScrollSettled(page);
}

async function pressStepsOn(page: Page, seq: string, expected: 'true' | 'false'): Promise<void> {
  await pressControlOn(page, seq, 'toggle-steps', expected);
}

/** Take the reader to the END of the thread.
 *
 *  A direct `scrollTop` write, which is not a reader GESTURE, so a standing
 *  follow survives it where one is armed. Two of its three callers press from
 *  the end with the seeded arm intact. The third runs disarmed, which is the
 *  state the clamp round trip was reported from. */
async function parkAtLiveEdge(page: Page): Promise<void> {
  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    el.scrollTop = el.scrollHeight - el.clientHeight;
  });
  await waitForScrollSettled(page);
}

/** Put `role` into the state the measured press moves OUT of, through STORAGE
 *  and a reload rather than through a setup press.
 *
 *  That is the shape reported for these cases: a thread OPENED with the log off
 *  or a turn folded, rather than one the reader toggled a moment ago. The
 *  toggled-a-moment-ago shape is its own case, `pressed a moment ago` below.
 *
 *  Both keys are `store.ts`'s (`STEPS_EXPANDED_KEY`, and the collapsed-exchange
 *  set keyed `threadId:userSeq`). */
async function seedControlState(page: Page, threadId: string, role: string, seq: string): Promise<void> {
  await page.evaluate(({ r, tid, s }: { r: string; tid: string; s: string }) => {
    if (r === 'toggle-steps') localStorage.setItem('lucidos-steps-expanded-v2', 'false');
    else localStorage.setItem('lucidos-collapsed-exchanges', JSON.stringify([`${tid}:${s}`]));
  }, { r: role, tid: threadId, s: seq });
  await page.reload();
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
}

/** Every turn's id, in transcript order. A turn is keyed by its user event's
 *  sequence, not by its position, so they have to be read off the DOM. */
async function turnSeqs(page: Page): Promise<string[]> {
  return await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    const seqs = Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange'))
      .map(t => t.getAttribute('data-user-seq') ?? '')
      .filter(Boolean);
    if (seqs.length === 0) throw new Error('no turn in the transcript');
    return seqs;
  });
}

/** The newest turn's own id. */
async function lastTurnSeq(page: Page): Promise<string> {
  const seqs = await turnSeqs(page);
  return seqs[seqs.length - 1];
}

/** How far the reader is from the END of the thread. It is the premise of the
 *  two live-edge cases below, never their assertion: what a press must hold is
 *  the control, and being on the end is only where the press was made from. */
async function edgeState(page: Page): Promise<{ gap: number }> {
  return await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    return { gap: el.scrollHeight - el.clientHeight - el.scrollTop };
  });
}

test.describe('the step-log control holds what the reader pressed', () => {
  // `openThread` turns the global header pin off. It is global and the e2e
  // database resets only between projects, so put it back. See
  // `disableMobileHeaderSticky`.
  test.afterEach(async ({ page }) => {
    await enableMobileHeaderSticky(page);
  });

  // The gerund is spelled out rather than built from the verb. `hide` plus
  // "ing" is "hideing", and that shipped as a test title.
  for (const { verb, gerund, to } of [
    { verb: 'show', gerund: 'showing', to: 'true' },
    { verb: 'hide', gerund: 'hiding', to: 'false' },
  ] as const) {
    test(`${gerund} the steps holds the control, not the line above it`, async ({ page }) => {
      await assertHealthy(page);
      // THE REPORTED SHAPE. The reader's topmost line is in an EARLIER turn than
      // the one whose control they press, and that earlier turn changes height
      // too. Holding their line therefore leaves the transcript where it is and
      // carries the pressed control away, which is what the old rule did.
      const threadId = seedThread(`Steps control holds the press on ${verb}`);
      try {
        await openThread(page, threadId);
        const seq = await turnCarrying(page, mark(TURNS - 3, 2));
        if (verb === 'show') await pressStepsOn(page, await lastTurnSeq(page), 'false');

        await parkControlMidScreen(page, seq);
        const before = await geometry(page, seq);
        // The park has to have HELD, or the rest measures a control nobody
        // placed. It is the premise of the case, not a detail of the helper.
        expect(
          Math.abs(before.top - before.clientHeight / 2),
          `the park left the control at ${before.top}, not halfway down`,
        ).toBeLessThanOrEqual(4);

        await pressStepsOn(page, seq, to);
        const after = await geometry(page, seq);

        expect(
          Math.abs(after.top - before.top),
          `${gerund} the steps moved the pressed control from ${before.top} to ${after.top}`,
        ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
        // Not vacuous: holding the control REQUIRED a scroll, because the turns
        // above it changed height. A correction that wrote nothing would pass
        // the assertion above only by accident.
        expect(
          Math.abs(after.scrollTop - before.scrollTop),
          'the turns above the control did not change height, so the case proves nothing',
        ).toBeGreaterThan(200);
      } finally {
        dropThread(threadId);
      }
    });
  }

  test('a thread with nothing to scroll still holds the control', async ({ page }) => {
    await assertHealthy(page);
    // The user's own report. The whole transcript fits on screen with the steps
    // hidden, so the reader's topmost line is the FIRST turn's first row and
    // `scrollTop` is 0. Holding that line means holding 0, which left the turn
    // they pressed off the bottom of a now-tall transcript.
    const threadId = seedThread('Steps control on a thread with no scroll', {
      turns: 3,
      chunks: 1,
      proseLines: 0,
      stepsPerChunk: 8,
    });
    try {
      await openThread(page, threadId);
      const seqs = await turnSeqs(page);
      expect(seqs.length, 'the short seed must render all three turns').toBe(3);

      await pressStepsOn(page, seqs[2], 'false');
      const seq = seqs[1];
      const before = await geometry(page, seq);
      expect(
        before.scrollHeight - before.clientHeight,
        `the premise is a transcript with nowhere to scroll: ${JSON.stringify(before)}`,
      ).toBeLessThanOrEqual(10);

      await pressStepsOn(page, seq, 'true');
      const after = await geometry(page, seq);

      expect(
        Math.abs(after.top - before.top),
        `showing the steps moved the pressed control from ${before.top} to ${after.top}`
        + ` (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });

  /** **A reader parked on the end of a quiet thread.**
   *
   *  The one park that used to be exempt: an armed reader on the live edge was
   *  carried to the new edge rather than held. Nothing is arriving on a quiet
   *  thread, so the press wins now (ADR 0064's turn-control block, ADR 0147).
   *
   *  Both cases keep the SEEDED ARM, unlike the rest of this file, and both
   *  press a control the reader can actually reach from the bottom. The turns
   *  are short with their steps hidden, so the last turn's header is on screen
   *  there. The FOLD case is the reported one, and its control is on screen by
   *  construction: a folded turn is a header and a stub. */
  for (const c of [
    { role: 'toggle-steps', name: 'the step log', off: 'false', on: 'true' },
    { role: 'toggle-collapsed', name: 'the fold', off: 'true', on: 'false' },
  ] as const) {
    test(`${c.name} holds its control for a reader on the end of the thread`, async ({ page }) => {
      await assertHealthy(page);
      // Long unbroken runs of steps, so turning the log on grows the last turn
      // by screens. The prose between them is one line, so the same turn is
      // short with the log off and its header sits at the bottom.
      const threadId = seedThread(`Live edge holds ${c.role}`, {
        stepsPerChunk: STEPS_PER_RUN,
        chunks: 2,
        proseLines: 1,
      });
      try {
        await openThread(page, threadId, { disarm: false, renderAll: false });
        const seq = await lastTurnSeq(page);
        await seedControlState(page, threadId, c.role, seq);
        await expect(controlOn(page, seq, c.role)).toHaveAttribute('aria-pressed', c.off);
        await parkAtLiveEdge(page);

        const edgeBefore = await edgeState(page);
        expect(edgeBefore.gap, 'the reader must start on the end of the thread').toBeLessThanOrEqual(2);
        const before = await geometry(page, seq, c.role);
        // A press the reader could not have made proves nothing about a press.
        expect(
          before.top,
          `the control is off screen at ${before.top}, so no finger could reach it`,
        ).toBeGreaterThan(0);
        expect(before.top).toBeLessThan(before.clientHeight);

        await pressControlOn(page, seq, c.role, c.on);
        const after = await geometry(page, seq, c.role);

        expect(
          Math.abs(after.top - before.top),
          `the press moved its own control from ${before.top} to ${after.top}`
          + ` (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`,
        ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
        // Not vacuous: the press has to have grown the thread past its pane, or
        // holding the control costs nothing and the old rule would pass too.
        expect(
          after.scrollHeight - before.scrollHeight,
          'the press grew the transcript by nothing, so the case proves nothing',
        ).toBeGreaterThan(after.clientHeight);
      } finally {
        dropThread(threadId);
      }
    });
  }

  /** **The log was pressed off a moment ago, from the same control.**
   *
   *  Hiding the steps at the end of a thread takes content out from UNDER the
   *  pressed control. No offset holds it there, so the browser clamps and the
   *  control slides down by what the clamp ate. That is geometry.
   *
   *  Showing them again must hold the control where the clamp left it. The
   *  deficit used to be repaid here, which threw the control up the screen and
   *  put the reader on the live edge. That is the reported bug (ADR 0147).
   *
   *  The turns are short with the log ON, so the last turn's header is reachable
   *  from the end of the thread. That is what lets the first press be a real
   *  one, unlike the storage seed the cases above use. */
  test('the step log holds its control when it was pressed a moment ago', async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Live edge clamp round trip', {
      stepsPerChunk: 4,
      chunks: 1,
      proseLines: 0,
      turns: 14,
    });
    try {
      await openThread(page, threadId, { renderAll: false });
      const seq = await lastTurnSeq(page);
      await expect(controlOn(page, seq)).toHaveAttribute('aria-pressed', 'true');
      await parkAtLiveEdge(page);
      expect(
        (await edgeState(page)).gap,
        'the reader must start on the end of the thread',
      ).toBeLessThanOrEqual(2);

      const beforeHide = await geometry(page, seq);
      expect(
        beforeHide.top,
        `the control is off screen at ${beforeHide.top}, so no finger could reach it`,
      ).toBeGreaterThan(0);
      expect(beforeHide.top).toBeLessThan(beforeHide.clientHeight);

      await pressStepsOn(page, seq, 'false');
      const clamped = await geometry(page, seq);
      // Not vacuous: without a clamp here the reverse press has nothing to
      // repay, and the case would pass whatever the correction remembered.
      expect(
        clamped.top - beforeHide.top,
        `the hide did not clamp: the control sat at ${beforeHide.top} and now sits at ${clamped.top}`,
      ).toBeGreaterThan(DRIFT_TOLERANCE_PX);

      await pressStepsOn(page, seq, 'true');
      const after = await geometry(page, seq);

      expect(
        Math.abs(after.top - clamped.top),
        `the press moved its own control from ${clamped.top} to ${after.top}`
        + ` (before ${JSON.stringify(clamped)}, after ${JSON.stringify(after)})`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });
});
