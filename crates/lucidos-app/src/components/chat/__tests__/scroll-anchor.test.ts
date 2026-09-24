import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { readScrollAnchor, readRowAnchor, rowTargetTop, anchorTargetTop, anchorTurnIsClamped, HEAD_CLAMPED_ATTR } from '../scrollAnchor';
import { mockTranscript } from './scroll-test-helpers';

// WHERE THE READER IS, said as content. The pair has to be exact and it has to
// be reversible. Read the anchor from one layout, apply it to another, and the
// same turn lands on the same line. That is the whole promise: the second
// layout is what a reload produces, a re-seeded render window, and the first is
// what the reader left.

const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
const TURN = 400;

describe('readScrollAnchor', () => {
  it('names the last turn at or above the viewport top, with its exact offset', () => {
    // Turn 2 starts 800px into the content; the reader is 950px down. So its top
    // sits 150px ABOVE the viewport top, and turn 3 is below and does not win.
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 950 });
    expect(readScrollAnchor(el)).toEqual({ eventId: 't2', relTop: -150 });
  });

  it('answers the turn exactly on the line with a relTop of zero', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 800 });
    expect(readScrollAnchor(el)).toEqual({ eventId: 't2', relTop: 0 });
  });

  it('anchors a reader ABOVE the first turn to that turn, at a positive offset', () => {
    // Nothing starts at or above the line. Answering null here would record the
    // top of the transcript as "no position", and the top of a re-seeded window
    // is not where they were.
    const el = mockTranscript({ ids: IDS, renderFrom: 3, turnHeight: TURN, scrollTop: 0 });
    expect(readScrollAnchor(el)).toEqual({ eventId: 't3', relTop: 0 });
  });

  it('skips a child that carries no id, and one with no box', () => {
    // The two the transcript really holds besides turns: chrome with nothing
    // behind it, and the mobile title row, which on desktop reports an all-zero
    // rect. Unskipped, the row reads as "on the line" and wins every scan.
    const rect = (top: number, height: number) =>
      ({ top, bottom: top + height, height, left: 0, right: 400, width: 400 });
    const el = {
      getBoundingClientRect: () => rect(0, 800),
      children: [
        { getAttribute: () => null, getBoundingClientRect: () => rect(-900, 400) },
        { getAttribute: () => 'title-row', getBoundingClientRect: () => rect(0, 0) },
        { getAttribute: () => 't2', getBoundingClientRect: () => rect(-150, 400) },
        { getAttribute: () => 't3', getBoundingClientRect: () => rect(250, 400) },
      ],
    } as unknown as HTMLElement;
    expect(readScrollAnchor(el)).toEqual({ eventId: 't2', relTop: -150 });
  });

  it('answers null when nothing on screen can be named', () => {
    const el = mockTranscript({ ids: [], turnHeight: TURN });
    expect(readScrollAnchor(el)).toBeNull();
  });
});

describe('anchorTargetTop', () => {
  it('reproduces the offset the anchor was taken at', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 950 });
    const anchor = readScrollAnchor(el)!;
    el.scrollTop = 0;
    expect(anchorTargetTop(el, anchor)).toBe(950);
  });

  it('lands the same turn on the same line in a SHORTER window', () => {
    // The reported bug, as arithmetic. The reader parked at 950 with the whole
    // thread rendered. A reload re-seeds the window at the newest four turns, so
    // 950 is past the end of it and the old pixel offset means nothing. The
    // anchor still resolves: turn 2 is the window's first, so resting its top
    // 150px above the line is a scrollTop of 150.
    const recorded = readScrollAnchor(mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 950 }))!;
    const reopened = mockTranscript({ ids: IDS, renderFrom: 2, turnHeight: TURN, scrollTop: 0 });
    expect(anchorTargetTop(reopened, recorded)).toBe(150);
  });

  it('answers null while the anchored turn is outside the window', () => {
    // The WAIT signal, not a failure: ThreadView has yet to walk the window up
    // to that turn.
    const recorded = { eventId: 't1', relTop: -150 };
    const reopened = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN });
    expect(anchorTargetTop(reopened, recorded)).toBeNull();
  });

  it('never answers below zero, for an anchor taken over the first turn', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 0 });
    expect(anchorTargetTop(el, { eventId: 't0', relTop: 120 })).toBe(0);
  });

  it('answers null for a container that cannot be asked', () => {
    // The DOM-free unit environment, and the childless fake the content-pane
    // tests drive. Neither may throw.
    expect(anchorTargetTop({} as HTMLElement, { eventId: 't0', relTop: 0 })).toBeNull();
    expect(readScrollAnchor({} as HTMLElement)).toBeNull();
  });
});

describe('anchorTurnIsClamped', () => {
  // The restore waits on a clamped turn: its top is real, but its rows are
  // still arriving. Landing there would settle the restore and stop the walk.

  it('answers for a turn drawn with its head clamped off, and only that turn', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN });
    el.setHeadClamped('t2', true);
    expect(anchorTurnIsClamped(el, { eventId: 't2', relTop: 0 })).toBe(true);
    expect(anchorTurnIsClamped(el, { eventId: 't3', relTop: 0 })).toBe(false);
  });

  it('is false for a turn that is not rendered at all', () => {
    // Absent is the walk's wait, answered by `anchorTargetTop` returning null.
    const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN });
    expect(anchorTurnIsClamped(el, { eventId: 't1', relTop: 0 })).toBe(false);
  });

  it('is the attribute ChatExchange stamps from its rowsHidden', () => {
    // A tripwire: the stamp is a JSX literal, so a rename on one side alone
    // would leave the restore landing on clamped turns again.
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf8');
    expect(source).toContain(`${HEAD_CLAMPED_ATTR}={rowsHidden > 0 ? '' : undefined}`);
  });
});

describe('readRowAnchor: a reading position that names a step row', () => {
  // Three turns of 40 rows, 25px each, so a turn is 1000px tall.
  const turns = ['t0', 't1', 't2'];
  const make = (scrollTop: number) =>
    mockTranscript({ ids: turns, turnHeight: 1000, rowsPerTurn: 40, scrollTop });

  it('names the row at or above the line, inside the turn at the line', () => {
    // 1310px down: turn t1 starts at 1000, so row 12 starts at 1300.
    expect(readRowAnchor(make(1310))).toEqual({ rowEventId: 't1-r12', relTop: -10 });
  });

  it('answers the row exactly on the line with a relTop of zero', () => {
    expect(readRowAnchor(make(1300))).toEqual({ rowEventId: 't1-r12', relTop: 0 });
  });

  it('answers null for a turn with no rows, which keeps the turn anchor', () => {
    expect(readRowAnchor(mockTranscript({ ids: turns, turnHeight: 1000, scrollTop: 1310 }))).toBeNull();
  });

  it('names the first drawn row for a reader exactly on the first drawn turn', () => {
    const el = mockTranscript({ ids: turns, renderFrom: 1, turnHeight: 1000, rowsPerTurn: 40, scrollTop: 0 });
    // The reader sits exactly on t1's first row, which IS at the line.
    expect(readRowAnchor(el)).toEqual({ rowEventId: 't1-r0', relTop: 0 });
  });
});

describe('a position saved on a turn drawn with its head clamped', () => {
  // THE REPORT: "its not remembering the position correctly". The window draws
  // the floor turn's tail. A turn anchor taken there measures from the CLAMPED
  // top. The next open draws the turn whole, and lands the reader above their
  // place by the height of the rows that had been left out.
  const turns = ['t0', 't1'];

  function parkedInClampedTurn() {
    const el = mockTranscript({ ids: turns, turnHeight: 1000, rowsPerTurn: 40, scrollTop: 300 });
    el.setRowsHidden('t0', 20); // t0 shows rows 20..39 only, 500px
    el.scrollTop = 300; // row 32 at the line: 12 rows into what is drawn
    return el;
  }

  /** The next open: the same turn drawn whole, and the reader's row where it
   *  really is, 20 rows further down the content. */
  function reopenedWhole(el: ReturnType<typeof parkedInClampedTurn>) {
    el.setRowsHidden('t0', 0);
    el.scrollTop = 0;
  }

  it('the turn anchor lands above the reader', () => {
    const el = parkedInClampedTurn();
    const turnAnchor = readScrollAnchor(el)!;
    reopenedWhole(el);
    // Row 32 starts 800px into the whole turn; the turn anchor says 300.
    expect(anchorTargetTop(el, turnAnchor)).toBe(300);
    expect(anchorTargetTop(el, turnAnchor)).not.toBe(800);
  });

  it('the row anchor lands the reader on their row', () => {
    const el = parkedInClampedTurn();
    const rowAnchor = readRowAnchor(el)!;
    expect(rowAnchor.rowEventId).toBe('t0-r32');
    reopenedWhole(el);
    expect(rowTargetTop(el, rowAnchor)).toBe(800);
  });

  it('the row anchor waits while its row is still clamped off', () => {
    const el = parkedInClampedTurn();
    el.setRowsHidden('t0', 35);
    expect(rowTargetTop(el, { rowEventId: 't0-r32', relTop: 0 })).toBeNull();
  });
});
