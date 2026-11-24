import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { readScrollAnchor, readRowAnchor, rowTargetTop, anchorTargetTop, anchorTurnIsClamped, HEAD_CLAMPED_ATTR, lineBlockOf, readLineAnchor } from '../scrollAnchor';
import { mockTranscript } from './scroll-test-helpers';


const IDS = ['t0', 't1', 't2', 't3', 't4', 't5'];
const TURN = 400;

describe('readScrollAnchor', () => {
  it('names the last turn at or above the viewport top, with its exact offset', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 950 });
    expect(readScrollAnchor(el)).toEqual({ eventId: 't2', relTop: -150 });
  });

  it('answers the turn exactly on the line with a relTop of zero', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 800 });
    expect(readScrollAnchor(el)).toEqual({ eventId: 't2', relTop: 0 });
  });

  it('anchors a reader ABOVE the first turn to that turn, at a positive offset', () => {
    const el = mockTranscript({ ids: IDS, renderFrom: 3, turnHeight: TURN, scrollTop: 0 });
    expect(readScrollAnchor(el)).toEqual({ eventId: 't3', relTop: 0 });
  });

  it('skips a child that carries no id, and one with no box', () => {
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
    const recorded = readScrollAnchor(mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 950 }))!;
    const reopened = mockTranscript({ ids: IDS, renderFrom: 2, turnHeight: TURN, scrollTop: 0 });
    expect(anchorTargetTop(reopened, recorded)).toBe(150);
  });

  it('answers null while the anchored turn is outside the window', () => {
    const recorded = { eventId: 't1', relTop: -150 };
    const reopened = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN });
    expect(anchorTargetTop(reopened, recorded)).toBeNull();
  });

  it('never answers below zero, for an anchor taken over the first turn', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN, scrollTop: 0 });
    expect(anchorTargetTop(el, { eventId: 't0', relTop: 120 })).toBe(0);
  });

  it('answers null for a container that cannot be asked', () => {
    expect(anchorTargetTop({} as HTMLElement, { eventId: 't0', relTop: 0 })).toBeNull();
    expect(readScrollAnchor({} as HTMLElement)).toBeNull();
  });
});

describe('anchorTurnIsClamped', () => {

  it('answers for a turn drawn with its head clamped off, and only that turn', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: TURN });
    el.setHeadClamped('t2', true);
    expect(anchorTurnIsClamped(el, { eventId: 't2', relTop: 0 })).toBe(true);
    expect(anchorTurnIsClamped(el, { eventId: 't3', relTop: 0 })).toBe(false);
  });

  it('is false for a turn that is not rendered at all', () => {
    const el = mockTranscript({ ids: IDS, renderFrom: 4, turnHeight: TURN });
    expect(anchorTurnIsClamped(el, { eventId: 't1', relTop: 0 })).toBe(false);
  });

  it('is the attribute ChatExchange stamps from its rowsHidden', () => {
    const here: string = dirname(fileURLToPath(import.meta.url));
    const source: string = readFileSync(resolve(here, '../ChatExchange.tsx'), 'utf8');
    expect(source).toContain(`${HEAD_CLAMPED_ATTR}={rowsHidden > 0 ? '' : undefined}`);
  });
});

describe('readRowAnchor: a reading position that names a step row', () => {
  const turns = ['t0', 't1', 't2'];
  const make = (scrollTop: number) =>
    mockTranscript({ ids: turns, turnHeight: 1000, rowsPerTurn: 40, scrollTop });

  it('names the row at or above the line, inside the turn at the line', () => {
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
    expect(readRowAnchor(el)).toEqual({ rowEventId: 't1-r0', relTop: 0 });
  });
});

describe('a position saved on a turn drawn with its head clamped', () => {
  const turns = ['t0', 't1'];

  function parkedInClampedTurn() {
    const el = mockTranscript({ ids: turns, turnHeight: 1000, rowsPerTurn: 40, scrollTop: 300 });
    el.setRowsHidden('t0', 20);
    el.scrollTop = 300;
    return el;
  }

  function reopenedWhole(el: ReturnType<typeof parkedInClampedTurn>) {
    el.setRowsHidden('t0', 0);
    el.scrollTop = 0;
  }

  it('the turn anchor lands above the reader', () => {
    const el = parkedInClampedTurn();
    const turnAnchor = readScrollAnchor(el)!;
    reopenedWhole(el);
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

describe('a line anchor resolves only to the kind of element it recorded', () => {
  const IDS = ['t0', 't1', 't2', 't3'];

  it('resolves its path while the turn is drawn as it was', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    el.scrollTop = 1320;
    const anchor = readLineAnchor(el)!;
    expect(anchor.path).toEqual([2]);
    expect(lineBlockOf(el, anchor)).not.toBeNull();
  });

  it('answers null once a row drawn ahead of the block takes its index', () => {
    const el = mockTranscript({ ids: IDS, turnHeight: 400, blocks: { t2: [20, 100, 1000, 200] } });
    el.scrollTop = 1320;
    const anchor = readLineAnchor(el)!;
    el.insertBlock('t2', 2, 40, 'keep-alive-under-card');
    expect(lineBlockOf(el, anchor)).toBeNull();
  });
});
