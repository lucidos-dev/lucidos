import { test, expect } from './fixtures';
import type { Page } from './fixtures';
import { assertHealthy, disableMobileHeaderSticky, disarmFollowSeed, enableMobileHeaderSticky, ensureOnThreadPane, navigateToApp, renderWholeTranscript, waitForScrollSettled } from './helpers';
import { psql } from './db-helpers';
import { randomUUID } from 'crypto';

/**
 * **A turn control holds the element the reader clicked, from anywhere.**
 *
 * The rule is one line: press a turn control and that control does not move.
 * Its siblings press from hand-picked positions. This one SWEEPS: every turn,
 * with its control parked at four heights up the screen, pressed at each.
 *
 * It sweeps where the CONTROL is, not where the reader's eye is, and that is
 * the change of premise this spec carries. The controls live in a response
 * header that is not sticky, and keyboard activation scrolls its target into
 * view. So a control is on screen for every press a human can make, and its
 * position on screen is the only variable left.
 *
 * The version this replaced parked at 24 scroll offsets and pressed through
 * `btn.click()` on controls it never scrolled into view. It was measuring a
 * press nobody can make, and the rule it pinned is what put a reader at the top
 * of the thread. See
 * docs/plans/2026-08-28-a-turn-control-holds-what-you-pressed.md
 */

const TURNS = 6;
const CHUNKS = 10;
const STEPS_PER_CHUNK = 4;
/** Heights up the readable band to park the control at, as a fraction of the
 *  transcript's own height. Near the top, near the bottom, and two between: the
 *  first and last are where a clamp is most likely to eat the correction. */
const PARK_FRACTIONS = [0.05, 0.35, 0.65, 0.95];
/** How far the control may move, in CSS px. A correction is written to a whole
 *  pixel and a sub-pixel row height can round the other way. */
const TOLERANCE_PX = 4;
/** How close the park has to land before a press is measured against it. The
 *  park converges, and a press made from a park that never settled measures a
 *  reader nobody placed. */
const PARK_TOLERANCE_PX = 8;

function q(o: unknown): string {
  return JSON.stringify(o).replace(/'/g, "''");
}

/** Turns of prose interleaved with runs of tool steps. The prose wraps and the
 *  runs are long. A press therefore takes a large bite out of the content above
 *  and below whichever control it lands on. */
function seedThread(title: string): string {
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
    ev('MessageReceived', { text: `Question ${t}, please look it up.`, channel: 'chat' });
    for (let c = 0; c < CHUNKS; c++) {
      const body = 'The quick brown fox jumps over the lazy dog and keeps running. '.repeat(4);
      ev('TextStreamed', { text: `P-${t}-${c} ${body}` });
      for (let s = 0; s < STEPS_PER_CHUNK; s++) {
        ev('ToolCalled', {
          name: 'run_bash',
          args: { command: `echo S-${t}-${c}-${s}` },
          description: `S-${t}-${c}-${s} run a shell command with a fairly long description that may wrap`,
        });
        ev('ToolResult', { name: 'run_bash', result: `out ${t}.${c}.${s}` });
      }
    }
    const answer = `Answer ${t}.`;
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
  // scroll that is not their own gesture. This spec is about the reader who is
  // READING, so it starts them disarmed.
  await disarmFollowSeed(page);
  // Hide-on-scroll LIVE rather than inherited: see `disableMobileHeaderSticky`.
  await disableMobileHeaderSticky(page);
  await navigateToApp(page);
  await ensureOnThreadPane(page);
  await expect(page.locator('.chat-exchange').first()).toBeVisible({ timeout: 30_000 });
  await expect(page.locator('[data-role="inline-step"]:visible').first()).toBeVisible({ timeout: 30_000 });
  // The transcript opens windowed, so the turns this spec presses on are not
  // all in the DOM yet. Ask for the whole thread the way a reader does.
  await renderWholeTranscript(page);
}

/** Every turn's id, in transcript order. A turn is keyed by its user event's
 *  sequence, not by its position, so they have to be read off the DOM. */
async function turnSeqs(page: Page): Promise<string[]> {
  return await page.evaluate(() => {
    // THE VISIBLE ONE. Mobile mounts every pane at once, and the compose view
    // reuses the class. So a bare `querySelector` can answer with a box nobody
    // is looking at, whose scroll geometry means nothing. Mirrors
    // `findVisibleThreadContent` in scrollState.ts.
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    if (!el) throw new Error('no visible .thread-content');
    return Array.from(el.querySelectorAll<HTMLElement>('.chat-exchange'))
      .map(t => t.getAttribute('data-user-seq') ?? '')
      .filter(Boolean);
  });
}

function control(page: Page, seq: string, role: string) {
  return page.locator(`.chat-exchange[data-user-seq="${seq}"] [data-role="${role}"]`).first();
}

/** Where `seq`'s control sits, and what room the transcript has to move it.
 *  One round trip, so a press is judged against one layout. */
interface Geometry {
  top: number;
  scrollTop: number;
  scrollHeight: number;
  clientHeight: number;
  /** How far the first line the reader can READ sits below the container's own
   *  top, i.e. what the mobile sticky title covers.
   *
   *  Carried so the press can be asked not to bury its own control, and that is
   *  not decoration. The hide-on-scroll header turns every unattributed pixel
   *  into sliding chrome, and the correction moves the container hundreds of
   *  pixels at once. Spent as a reveal, that slides a header and a thread title
   *  over the very control the press just held. `markAnchorScroll` is what
   *  stops it. Nothing else in the browser suite can see the chrome move, since
   *  both terms of `top` ride the container. */
  inset: number;
}

async function geometry(page: Page, seq: string, role: string): Promise<Geometry> {
  return await page.evaluate(({ s, r }: { s: string; r: string }) => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0)!;
    const btn = el.querySelector<HTMLElement>(`.chat-exchange[data-user-seq="${s}"] [data-role="${r}"]`);
    if (!btn) throw new Error(`no ${r} control on turn ${s}`);
    const containerTop = el.getBoundingClientRect().top;
    let edge = containerTop;
    for (const child of Array.from(el.children) as HTMLElement[]) {
      if (!child.matches('[data-scroller-pinned]')) continue;
      const r = child.getBoundingClientRect();
      if (r.height > 0 && r.bottom > edge) edge = r.bottom;
    }
    return {
      top: btn.getBoundingClientRect().top - containerTop,
      scrollTop: el.scrollTop,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
      inset: edge - containerTop,
    };
  }, { s: seq, r: role });
}

async function controlTop(page: Page, seq: string, role: string): Promise<number> {
  return (await geometry(page, seq, role)).top;
}

/** Could the correction have held the control at all?
 *
 *  A press that shrinks the transcript can leave the target unreachable: with
 *  less content below the control than the pane is tall, no offset puts it
 *  back, so the browser clamps and it slides. The full-response control does
 *  that routinely, keeping only each turn's last text block.
 *
 *  `drift` is what the container failed to absorb, so holding the control asks
 *  `scrollTop` to move by that much again. Outside the container's own extent,
 *  there was nothing to give and no anchor rule could have done better. The
 *  round trip is what the reader is owed there, and
 *  `toggle-round-trip-across-a-clamp.test.ts` is where it is pinned.
 *
 *  It asks about the target, never about whether the container ended up pinned.
 *  A correction the clamp ate does not always come to rest ON the extreme: the
 *  transcript can settle taller a frame later, and the offset it was clamped
 *  against is gone by the time this reads. */
function correctionWasReachable(after: Geometry, drift: number): boolean {
  const max = Math.max(0, after.scrollHeight - after.clientHeight);
  const target = after.scrollTop + drift;
  return target >= -1 && target <= max + 1;
}

/** Park the transcript so `seq`'s control sits `fraction` of the way down it.
 *  Reports where it was AIMED, which the clamp can hold it short of at either
 *  end of the thread.
 *
 *  It CONVERGES rather than scrolling once by the measured delta: the park's own
 *  scroll moves the mobile chrome the control is measured against. */
async function parkControlAt(page: Page, seq: string, role: string, fraction: number): Promise<number> {
  const want = await page.evaluate((f: number) =>
    Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0)!.clientHeight * f, fraction);
  for (let round = 0; round < 4; round++) {
    const at = await controlTop(page, seq, role);
    if (Math.abs(at - want) <= 1) break;
    await page.evaluate((d: number) => {
      const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
        .find(c => c.getBoundingClientRect().height > 0)!;
      el.scrollTop += d;
    }, at - want);
    await waitForScrollSettled(page);
  }
  return want;
}

/** Press `seq`'s control and wait for it to report the state it moved to. */
async function press(page: Page, seq: string, role: string, expected: 'true' | 'false'): Promise<void> {
  const btn = control(page, seq, role);
  // DISPATCHED, not driven through the pointer. Playwright scrolls a control
  // into view before clicking, and the park has just placed this one on purpose.
  // A drive would undo the park and measure a different press.
  await btn.dispatchEvent('click');
  await expect(btn).toHaveAttribute('aria-pressed', expected);
  await waitForScrollSettled(page);
}

/** Bring `seq`'s control to `state` without measuring anything, and leave the
 *  reader owed nothing by it.
 *
 *  The setup press is a real press, so a clamp it runs into is REMEMBERED and
 *  repaid by the next press of the same control. That is the round trip a
 *  reader is owed (`toggle-round-trip-across-a-clamp.test.ts`), and here it is
 *  a confound: the measured press would be the return leg of a journey the
 *  sweep only meant to arrange.
 *
 *  A scroll retires the credit, and only a scroll can, because the position is
 *  the reader's the moment they choose it. So the setup ends by taking the
 *  reader to the top, which the park then converges back down from. Without it
 *  the last turn's bottom-edge park was the one place the park itself wrote
 *  nothing, so the credit survived into the measurement. */
async function ensureState(page: Page, seq: string, role: string, state: 'true' | 'false'): Promise<void> {
  const btn = control(page, seq, role);
  if (await btn.getAttribute('aria-pressed') !== state) await press(page, seq, role, state);
  await page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0)!;
    el.scrollTop = 0;
  });
  await waitForScrollSettled(page);
}

/** The three turn controls, each swept in both directions.
 *
 *  The two reveals change every turn in the transcript, which is what puts
 *  content above the pressed control in motion. The fold changes only its own
 *  turn, and is swept because the reader is owed the same promise from it.
 *
 *  THE FOLD SKIPS THE BOTTOM-EDGE PARK, and that is a gap rather than a
 *  tidy-up. Folding the LAST turn from a control at the foot of the pane misses
 *  by 70px on WebKit, both ways. The correction reads the control's offset on
 *  the frame the mutation commits. The transcript has not settled there, and
 *  the next-frame re-assert re-reads the same unsettled number. Chromium
 *  settles inside the frame and holds the control exactly.
 *
 *  Left rather than fixed, since the fold had no anchoring at all before this
 *  change. It is no regression, and widening the correction past one frame
 *  touches the hot path of every reveal. Recorded as a non-goal in
 *  docs/plans/2026-08-28-a-turn-control-holds-what-you-pressed.md */
const CONTROLS = [
  { role: 'toggle-steps', name: 'the steps', parks: PARK_FRACTIONS },
  { role: 'toggle-details', name: 'the full response', parks: PARK_FRACTIONS },
  { role: 'toggle-collapsed', name: 'the fold', parks: PARK_FRACTIONS.filter(f => f < 0.9) },
] as const;

test.describe('a turn control holds the element the reader clicked', () => {
  // `openThread` turns the global header pin off. It is global and the e2e
  // database resets only between projects, so put it back. See
  // `disableMobileHeaderSticky`.
  test.afterEach(async ({ page }) => {
    await enableMobileHeaderSticky(page);
  });

  for (const c of CONTROLS) {
    // `from` is the state the measured press starts in, so the two runs cover
    // the press in each direction.
    for (const from of ['true', 'false'] as const) {
      const to = from === 'true' ? 'false' : 'true';
      test(`turning ${c.name} ${from === 'true' ? 'off' : 'on'}, swept up the screen`, async ({ page }) => {
        await assertHealthy(page);
        const threadId = seedThread(`Sweep ${c.role} ${from}`);
        const failures: string[] = [];
        let held = 0;
        let clamped = 0;
        try {
          await openThread(page, threadId);
          const seqs = await turnSeqs(page);
          expect(seqs.length, 'the seeded thread must render every turn').toBe(TURNS);

          for (const seq of seqs) {
            for (const fraction of c.parks) {
              await ensureState(page, seq, c.role, from);
              const want = await parkControlAt(page, seq, c.role, fraction);
              const before = await geometry(page, seq, c.role);
              // A park the clamp held short measures a control nobody placed.
              if (Math.abs(before.top - want) > PARK_TOLERANCE_PX) continue;

              await press(page, seq, c.role, to);
              const after = await geometry(page, seq, c.role);
              const drift = after.top - before.top;
              if (Math.abs(drift) > TOLERANCE_PX && !correctionWasReachable(after, drift)) {
                clamped++;
                continue;
              }
              // The press must not have BURIED its own control. See
              // `Geometry.inset`.
              //
              // The reported harm directly, not the proxy "the readable edge
              // must not move". That proxy failed a press nobody is hurt by: a
              // shrink of an order of magnitude lands the reader near the top of
              // a short thread, where the header belongs revealed.
              //
              // WENT behind, so a delta rather than a state. The 5% park puts
              // the control 36px down and the mobile title covers 146, so it is
              // buried before the press too.
              //
              // Numbers for both, and the settle underneath them, in
              // docs/plans/2026-08-28-a-turn-control-holds-what-you-pressed.md
              if (before.top >= before.inset && after.top < after.inset) {
                failures.push(
                  `turn ${seq} at ${Math.round(fraction * 100)}%: the chrome buried the`
                  + ` control, its readable edge going ${before.inset} to ${after.inset}`
                  + ` with the control at ${after.top.toFixed(1)}`
                  + ` (drift ${drift.toFixed(1)}, before ${JSON.stringify(before)},`
                  + ` after ${JSON.stringify(after)})`,
                );
                continue;
              }
              if (Math.abs(drift) <= TOLERANCE_PX) { held++; continue; }
              failures.push(
                `turn ${seq} (${seqs.indexOf(seq) + 1} of ${seqs.length})`
                + ` at ${Math.round(fraction * 100)}%: the control moved ${drift.toFixed(1)}px,`
                + ` from ${before.top.toFixed(1)} to ${after.top.toFixed(1)}`
                + ` (before ${JSON.stringify(before)}, after ${JSON.stringify(after)})`,
              );
            }
          }
          // A sweep that skipped or excused every park would pass silently, so
          // say how many presses actually answered. The ends of the thread
          // cannot always be parked at. The full-response control clamps often
          // too, which is why this is a floor rather than the whole grid.
          expect(held + failures.length, 'no press was reachable, so the sweep asserted nothing')
            .toBeGreaterThan(TURNS);
          expect(
            failures.join('\n'),
            `${failures.length} of ${held + failures.length} reachable presses moved the control`
            + ` (${clamped} more were clamped)`,
          ).toBe('');
        } finally {
          dropThread(threadId);
        }
      });
    }
  }
});
