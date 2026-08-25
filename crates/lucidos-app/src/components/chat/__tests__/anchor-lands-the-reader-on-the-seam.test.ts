import { describe, it, expect, beforeEach } from 'vitest';

// Stub HTMLElement before importing the modules that reference it, exactly as
// the sibling scroll suites do.
if (typeof (globalThis as any).HTMLElement === 'undefined') {
  (globalThis as any).HTMLElement = class {};
}
if (typeof globalThis.document !== 'undefined' && !('activeElement' in globalThis.document)) {
  (globalThis.document as any).activeElement = null;
}
if (typeof (globalThis as any).requestAnimationFrame === 'undefined') {
  (globalThis as any).requestAnimationFrame = (cb: any) => { cb(); return 0; };
}
if (typeof (globalThis as any).cancelAnimationFrame === 'undefined') {
  (globalThis as any).cancelAnimationFrame = () => {};
}

import { useMockMO } from './scroll-test-helpers';
import { withScrollAnchor } from '../CreateThreadView';
import { setActiveScrollElement, stopFollowingBottom } from '../scrollState';

/**
 * **Hiding the step log lands the reader on the SEAM the log left behind.**
 *
 * `anchor-holds-the-readers-line.test.ts` covers the reader whose own line
 * survives the reveal. This is the other half, and it is the shape a
 * coding-agent thread is made of: a turn runs dozens of tool calls with nothing
 * said between them, so the reader is parked INSIDE an unbroken run of step
 * rows. Hiding the log takes their line, and every row around them with it.
 *
 * No line is left to hold, so the run's collapse has to be answered rather than
 * dodged. It collapses to one point, the bottom of the last surviving row above
 * them. That point is where they were.
 */

/* Holding a surviving element at its OWN old position is the wrong answer
 * here, whichever element it is. What sits between it and the reader is
 * exactly what the hide removed, and that height is the error. On the turn's
 * response panel it is the whole run above the reader, thousands of pixels on
 * a real turn. That is the reported jump.
 *
 * The model below is DENSE, unlike the sibling suite's sparse offsets. Rows are
 * contiguous, so hiding one closes the space it held, and the expected offset
 * is derived from the layout rather than written by hand.
 */

const CONTAINER_TOP = 120;
const CLIENT_HEIGHT = 600;
const ROW_H = 30;

type Kind = 'prose' | 'step';

/** A transcript of turns, each a contiguous run of fixed-height rows. Hiding
 *  the step log drops every `step` row and closes the space it held. */
function buildTranscript(turnSpecs: Kind[][], scrollTop: number) {
  let stepsHidden = false;
  const rows: { kind: Kind; turn: number }[] = [];
  turnSpecs.forEach((kinds, t) => kinds.forEach(kind => rows.push({ kind, turn: t })));

  const drawn = (i: number) => !(stepsHidden && rows[i].kind === 'step');
  /** Content offset of row `i`, from the rows still drawn above it. */
  const offsetOf = (i: number) => {
    let y = 0;
    for (let k = 0; k < i; k++) if (drawn(k)) y += ROW_H;
    return y;
  };
  const contentHeight = () => rows.reduce((y, _r, i) => y + (drawn(i) ? ROW_H : 0), 0);

  const container: any = {
    _top: scrollTop,
    get scrollTop() { return this._top; },
    // A real container clamps, and the clamp is half of what this suite is
    // about: the hide can leave the wanted offset unreachable.
    set scrollTop(v: number) {
      this._top = Math.max(0, Math.min(v, Math.max(0, contentHeight() - CLIENT_HEIGHT)));
    },
    get scrollHeight() { return contentHeight(); },
    clientHeight: CLIENT_HEIGHT,
    style: { overflow: '' },
    contains: () => false,
    closest: () => null,
    getBoundingClientRect: () => ({
      top: CONTAINER_TOP,
      bottom: CONTAINER_TOP + CLIENT_HEIGHT,
      height: CLIENT_HEIGHT,
      left: 0, right: 400, width: 400,
    }),
  };

  const rectFor = (offset: number, height: number) => {
    const top = CONTAINER_TOP + offset - container.scrollTop;
    return { top, bottom: top + height, height, left: 0, right: 400, width: 400 };
  };

  const rowEls = rows.map((r, i) => ({
    get isConnected() { return drawn(i); },
    matches: (sel: string) => (r.kind === 'step') === sel.includes('inline-step'),
    getBoundingClientRect: () => rectFor(offsetOf(i), drawn(i) ? ROW_H : 0),
  }));

  const turnEls = turnSpecs.map((_kinds, t) => {
    const mine = rows.map((r, i) => (r.turn === t ? i : -1)).filter(i => i >= 0);
    const first = mine[0];
    const span = () => ({
      start: offsetOf(first),
      height: mine.filter(i => drawn(i)).length * ROW_H,
    });
    // The response panel wraps the whole body, so it is the far-above element
    // the correction reaches for once every row candidate has gone.
    const panel = {
      isConnected: true,
      matches: (sel: string) => sel.includes('response-panel'),
      getBoundingClientRect: () => { const s = span(); return rectFor(s.start, s.height); },
    };
    return {
      isConnected: true,
      matches: (_sel: string) => false,
      closest: () => container,
      querySelectorAll: (sel: string) =>
        sel.includes('response-content') ? mine.map(i => rowEls[i]) : [panel],
      getBoundingClientRect: () => { const s = span(); return rectFor(s.start, s.height); },
    };
  });

  container.children = turnEls;
  return {
    container,
    turns: turnEls,
    hideSteps: () => { stepsHidden = true; },
    offsetOf,
    /** Index of the first row of turn `t`. */
    firstRowOf: (t: number) => rows.findIndex(r => r.turn === t),
  };
}

/** Twelve turns, each twenty lines of prose then a hundred tool calls with
 *  nothing said between them. Every turn is far taller than the screen. */
const PROSE_PER_TURN = 20;
const STEPS_PER_TURN = 100;
const TURNS = 12;
const SPEC: Kind[][] = Array.from({ length: TURNS }, () => [
  ...Array<Kind>(PROSE_PER_TURN).fill('prose'),
  ...Array<Kind>(STEPS_PER_TURN).fill('step'),
]);

describe('hiding the step log lands the reader on the seam', () => {
  beforeEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
  });

  it('puts the reader where their run of steps collapsed to', () => {
    const restoreMO = useMockMO();
    const parkedTurn = 5;
    const t = buildTranscript(SPEC, 0);
    // The row the run starts at, and the reader half way down it.
    const runStart = t.firstRowOf(parkedTurn) + PROSE_PER_TURN;
    const runStartOffset = t.offsetOf(runStart);
    t.container.scrollTop = runStartOffset + (STEPS_PER_TURN / 2) * ROW_H;
    // Non-vacuous: the reader really is inside the run.
    expect(t.container.scrollTop).toBeGreaterThan(runStartOffset);

    // The control is pressed on the turn the reader is parked in.
    withScrollAnchor(t.turns[parkedTurn] as any, () => { t.hideSteps(); });

    // Where the run used to start, measured in the transcript the hide left.
    expect(t.container.scrollTop).toBe(t.offsetOf(runStart));
    restoreMO();
  });

  it('holds the reader when their own line is prose the hide keeps', () => {
    const restoreMO = useMockMO();
    const parkedTurn = 5;
    const t = buildTranscript(SPEC, 0);
    // Parked on the tenth prose line of the turn, which survives the hide.
    const proseRow = t.firstRowOf(parkedTurn) + 10;
    t.container.scrollTop = t.offsetOf(proseRow);

    withScrollAnchor(t.turns[parkedTurn] as any, () => { t.hideSteps(); });

    // Their own line stays on the top edge: nothing above it inside their turn
    // was a step.
    expect(t.container.scrollTop).toBe(t.offsetOf(proseRow));
    restoreMO();
  });
});
