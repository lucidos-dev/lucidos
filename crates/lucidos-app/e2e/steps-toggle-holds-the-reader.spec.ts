import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, disableMobileHeaderSticky, disarmFollowSeed, enableMobileHeaderSticky, ensureOnThreadPane, navigateToApp } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * **The step-log toggle keeps the reader's topmost line at the top.**
 *
 * The control is transcript-wide, so one press changes the height of EVERY
 * turn, the part of the reader's own turn above them included.
 * `withScrollAnchor` corrects for whatever changed above its anchor, so what
 * the anchor IS decides who is held.
 *
 * A turn is often taller than the phone viewport. Anchoring the whole turn
 * holds a point far above the screen. Each step row revealed between that
 * point and the reader's first visible line pushes them down. That is the
 * report this spec came from.
 *
 * So the reader is parked with a KNOWN line at the top of the transcript, deep
 * inside a tall turn. That line has to still be there afterwards.
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
 *  for dozens of calls with nothing said between them. Tall enough that a
 *  reader parked inside one run cannot see either end of it. */
const STEPS_PER_RUN = 40;
/** How far the anchored line may move, in CSS px. A correction is written to a
 *  whole pixel, and a sub-pixel row height can round the other way. */
const DRIFT_TOLERANCE_PX = 4;

/** A line the test can find again after the DOM has changed under it. */
const mark = (turn: number, chunk: number) => `MARK-${turn}-${chunk}`;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

/** Tall turns of prose and tool steps. Hiding the log takes a large bite out of
 *  each turn, and showing it puts the same bite back.
 *
 *  `stepsPerChunk` is how many calls follow each line of prose. It decides
 *  whether the reader can be parked with no surviving line near them.
 *
 *  `proseLines` keeps the transcript scrollable once the log is hidden. A
 *  thread that collapses shorter than the viewport clamps every offset to zero,
 *  and the clamp would answer for the correction. */
function seedThread(
  title: string,
  { stepsPerChunk = STEPS_PER_CHUNK, chunks = CHUNKS_PER_TURN, proseLines = 1 } = {},
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
  for (let t = 0; t < TURNS; t++) {
    ev('MessageReceived', { text: `Question number ${t}, please look it up.`, channel: 'chat' });
    for (let c = 0; c < chunks; c++) {
      const body = 'The quick brown fox jumps over the lazy dog. '.repeat(6 * proseLines);
      ev('TextStreamed', { text: `${mark(t, c)} working through part ${c}. ${body}` });
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
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, is_coding_agent, active_children_count) VALUES ('${threadId}', '${title}', 'chat', '${last}', ${TURNS}, false, true, 'idle', 'inbox', false, 0)`,
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

async function openThread(page: Page, threadId: string): Promise<void> {
  await page.addInitScript((tid: string) => {
    localStorage.setItem('lucidos-focused-thread', tid);
  }, threadId);
  // The seed ships ARMED, and a rider is carried back to the live edge by any
  // scroll that is not their own gesture. Parking them anywhere would be undone
  // before the control was ever pressed. This spec is about the reader who is
  // READING, so it starts them disarmed.
  await disarmFollowSeed(page);
  // Hide-on-scroll LIVE rather than inherited: see `disableMobileHeaderSticky`.
  await disableMobileHeaderSticky(page);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-role="inline-step"]:visible').first()).toBeVisible({ timeout: 30_000 });
}

/** The row carrying `text`: which turn it belongs to, and how far its top sits
 *  from the first line of the transcript the reader can READ.
 *
 *  That edge is the container's own top, or the bottom of the sticky thread
 *  title drawn over it on mobile. The rule mirrors `readerTopEdge`. A line
 *  hidden behind the title is not the one the correction holds, so measuring
 *  against the container edge measured the wrong reader. */
async function measureLine(page: Page, text: string): Promise<{ seq: string; offset: number }> {
  return await page.evaluate((needle: string) => {
    const el = document.querySelector('.thread-content') as HTMLElement | null;
    if (!el) throw new Error('no .thread-content');
    let edge = el.getBoundingClientRect().top;
    for (const child of Array.from(el.children) as HTMLElement[]) {
      if (!child.matches('[data-scroller-pinned]')) continue;
      const r = child.getBoundingClientRect();
      if (r.height > 0 && r.bottom > edge) edge = r.bottom;
    }
    const row = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *'))
      .find(r => (r.textContent ?? '').includes(needle));
    if (!row) throw new Error(`no row carrying ${needle}`);
    const turn = row.closest('.chat-exchange') as HTMLElement | null;
    return {
      seq: turn?.getAttribute('data-user-seq') ?? '',
      offset: row.getBoundingClientRect().top - edge,
    };
  }, text);
}

/** Park the transcript with `text`'s line on that edge, and report which turn it
 *  belongs to. The line is deep inside its turn, so the turn's own top ends up
 *  well above the screen.
 *
 *  It CONVERGES rather than scrolling once by the measured offset. The edge is
 *  the bottom of the mobile sticky title, and the park's own scroll is what
 *  hides or reveals the header that title rides. One write therefore lands the
 *  line a header's height off the edge it was measured against, whenever the
 *  scroll moves the chrome. Re-measuring closes that, and the loop settles on
 *  the second round with the chrome at rest. */
async function parkLineAtTop(page: Page, text: string): Promise<string> {
  const { seq } = await measureLine(page, text);
  for (let round = 0; round < 4; round++) {
    const { offset } = await measureLine(page, text);
    if (Math.abs(offset) <= 1) break;
    await page.evaluate((d: number) => {
      const el = document.querySelector('.thread-content') as HTMLElement;
      el.scrollTop += d;
    }, offset);
    await page.waitForTimeout(400);
  }
  return seq;
}

/** Where `text`'s line sits now. An unchanged reading means the reader has not
 *  moved. */
async function lineOffsetFromTop(page: Page, text: string): Promise<number> {
  return (await measureLine(page, text)).offset;
}

function stepsToggle(page: Page, seq: string) {
  return page
    .locator(`.chat-exchange[data-user-seq="${seq}"] .response-header .turn-controls [data-role="toggle-steps"]`)
    .first();
}

/** Press the step-log control on `seq`'s own response header. Returns once the
 *  toggle reports its new state and the position has settled. */
async function pressStepsOn(page: Page, seq: string, expected: 'true' | 'false'): Promise<void> {
  const toggle = stepsToggle(page, seq);
  await toggle.dispatchEvent('click');
  await expect(toggle).toHaveAttribute('aria-pressed', expected);
  await page.waitForTimeout(400);
}

/** The newest turn's own id. A turn is keyed by its user event's sequence, not
 *  by its position, so the last one has to be read off the DOM. */
async function lastTurnSeq(page: Page): Promise<string> {
  return await page.evaluate(() => {
    const turns = document.querySelectorAll<HTMLElement>('.thread-content .chat-exchange');
    const last = turns[turns.length - 1];
    if (!last) throw new Error('no turn in the transcript');
    return last.getAttribute('data-user-seq') ?? '';
  });
}


/** Park the reader, press the control on the turn they are reading, and report
 *  where their top line was and where it ended up.
 *
 *  The click is DISPATCHED rather than driven through the pointer. Playwright
 *  scrolls a control into view before clicking it. The header of a tall turn is
 *  off the top of the screen, so the drive itself would move the reader. */
async function parkPressAndMeasure(
  page: Page,
  text: string,
  expected: 'true' | 'false',
): Promise<{ before: number; after: number }> {
  const seq = await parkLineAtTop(page, text);
  const toggle = stepsToggle(page, seq);
  const before = await lineOffsetFromTop(page, text);
  // The park has to have HELD, or the rest measures a reader nobody placed.
  expect(Math.abs(before), `the park left the line at ${before}, not at the top`).toBeLessThanOrEqual(2);
  await toggle.dispatchEvent('click');
  await expect(toggle).toHaveAttribute('aria-pressed', expected);
  await page.waitForTimeout(400);
  return { before, after: await lineOffsetFromTop(page, text) };
}

/** Park the reader deep INSIDE the run of steps after `text`'s line. Reports
 *  how far below the reader's edge that line's bottom then sits.
 *
 *  A negative reading is the line being off the top of the screen, which is the
 *  whole point: the reader can see neither end of the run they are in. */
async function parkInsideTheRunAfter(page: Page, text: string, into: number): Promise<number> {
  return await page.evaluate(({ needle, depth }: { needle: string; depth: number }) => {
    const el = document.querySelector('.thread-content') as HTMLElement;
    const rows = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *'));
    const at = rows.findIndex(r => (r.textContent ?? '').includes(needle));
    if (at < 0) throw new Error(`no row carrying ${needle}`);
    const steps = rows.slice(at + 1).filter(r => r.matches('[data-role="inline-step"]'));
    const target = steps[Math.min(depth, steps.length - 1)];
    if (!target) throw new Error(`no step run after ${needle}`);
    el.scrollTop += target.getBoundingClientRect().top - el.getBoundingClientRect().top;
    return rows[at].getBoundingClientRect().bottom - el.getBoundingClientRect().top;
  }, { needle: text, depth: into });
}

/** How far `text`'s line's BOTTOM sits from the reader's first readable line.
 *  Zero means the reader is resting on the seam a removed run left behind. */
async function seamOffset(page: Page, text: string): Promise<number> {
  return await page.evaluate((needle: string) => {
    const el = document.querySelector('.thread-content') as HTMLElement;
    let edge = el.getBoundingClientRect().top;
    for (const child of Array.from(el.children) as HTMLElement[]) {
      if (!child.matches('[data-scroller-pinned]')) continue;
      const r = child.getBoundingClientRect();
      if (r.height > 0 && r.bottom > edge) edge = r.bottom;
    }
    const row = Array.from(el.querySelectorAll<HTMLElement>('.response-content > *'))
      .find(r => (r.textContent ?? '').includes(needle));
    if (!row) throw new Error(`no row carrying ${needle}`);
    return row.getBoundingClientRect().bottom - edge;
  }, text);
}

test.describe('the step-log toggle holds the reader still', () => {
  // `openThread` turns the global header pin off. It is global and the e2e
  // database resets only between projects, so put it back. See
  // `disableMobileHeaderSticky`.
  test.afterEach(async ({ page }) => {
    await enableMobileHeaderSticky(page);
  });

  test('showing the steps keeps the reader on their own line, not on their turn', async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Steps toggle holds the reader');
    try {
      await openThread(page, threadId);
      // Start from steps OFF, so the press under test is the one that SHOWS
      // them. Any turn's control does it, the setting being transcript-wide.
      await pressStepsOn(page, await lastTurnSeq(page), 'false');

      const line = mark(TURNS - 3, CHUNKS_PER_TURN - 2);
      const { before, after } = await parkPressAndMeasure(page, line, 'true');
      expect(
        Math.abs(after - before),
        `showing the steps moved the reader's top line from ${before} to ${after}`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });

  test('hiding the steps keeps the reader on their own line too', async ({ page }) => {
    await assertHealthy(page);
    const threadId = seedThread('Steps toggle holds the reader on hide');
    try {
      await openThread(page, threadId);

      const line = mark(TURNS - 3, CHUNKS_PER_TURN - 2);
      const { before, after } = await parkPressAndMeasure(page, line, 'false');
      expect(
        Math.abs(after - before),
        `hiding the steps moved the reader's top line from ${before} to ${after}`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });

  test('a reader inside a long run of steps lands on the seam it left', async ({ page }) => {
    await assertHealthy(page);
    // The coding-agent shape: dozens of calls in a row with nothing said
    // between them. The reader's own line is a step, and so is every row for
    // screens in either direction, so the hide takes all of it.
    const chunks = 6;
    const threadId = seedThread('Steps toggle inside a run', {
      stepsPerChunk: STEPS_PER_RUN,
      chunks,
      proseLines: 3,
    });
    try {
      await openThread(page, threadId);

      const line = mark(TURNS - 4, chunks - 3);
      const seq = (await measureLine(page, line)).seq;
      const wasAbove = await parkInsideTheRunAfter(page, line, Math.floor(STEPS_PER_RUN / 2));
      // Not vacuous: the line the reader will land under has to be off the top
      // of the screen when they press, or holding it would be free.
      expect(
        wasAbove,
        `the park left the preceding line ${wasAbove}px below the edge, not above it`,
      ).toBeLessThan(-100);

      await pressStepsOn(page, seq, 'false');

      const rest = await seamOffset(page, line);
      expect(
        Math.abs(rest),
        `the run collapsed but left the reader ${rest}px off its seam`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);

      // Pressing it again puts the run back ABOVE them, and the line they are
      // now reading must not move for it. That is the "on and off" half: the
      // surviving prose holds still across both presses.
      const below = mark(TURNS - 4, chunks - 2);
      const beforeShow = (await measureLine(page, below)).offset;
      await pressStepsOn(page, seq, 'true');
      const afterShow = (await measureLine(page, below)).offset;
      expect(
        Math.abs(afterShow - beforeShow),
        `showing the steps again moved the reader's line from ${beforeShow} to ${afterShow}`,
      ).toBeLessThanOrEqual(DRIFT_TOLERANCE_PX);
    } finally {
      dropThread(threadId);
    }
  });
});
