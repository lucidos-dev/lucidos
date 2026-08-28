import { describe, it, expect } from 'vitest';
import { readScrollAnchor, anchorTargetTop } from '../scrollAnchor';
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
