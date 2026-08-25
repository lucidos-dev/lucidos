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
 * **The anchor is the reader's own line, not the turn they pressed.**
 *
 * A coding-agent turn runs to several phone screens. A reader reading its tail
 * has its top far above the viewport, so holding the TURN holds a point they
 * cannot see. Each step row revealed between that point and their first visible
 * line pushes them down by its height. The transcript then reads as having
 * scrolled a screen or two back up the thread.
 *
 * So `withScrollAnchor` holds the first row with any part still on screen. It
 * takes the pressed turn only where there is no such row, or where the mutation
 * removed every candidate.
 *
 * The container here carries real children, unlike `mockContainer`, because the
 * candidate scan reads them. Positions are content offsets, and each element's
 * rect follows from its offset and the container's live `scrollTop`, the way
 * `contentOffsetTop` measures.
 */

const CONTAINER_TOP = 120;
const CLIENT_HEIGHT = 500;
const TURN_HEIGHT = 3000;
const ROW_HEIGHT = 20;

interface RowSpec {
  offset: number;
  step?: boolean;
  gone?: boolean;
}

function build(opts: { scrollTop: number; scrollHeight: number; rows: RowSpec[]; pinnedHeight?: number }) {
  const container: any = {
    scrollTop: opts.scrollTop,
    scrollHeight: opts.scrollHeight,
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
  const rectAt = (offset: number, height: number) => {
    const top = CONTAINER_TOP + offset - container.scrollTop;
    return { top, bottom: top + height, height, left: 0, right: 400, width: 400 };
  };
  const rows = opts.rows.map(spec => ({
    spec,
    get isConnected() { return !spec.gone; },
    matches: (sel: string) => (spec.step === true) === sel.includes('inline-step'),
    getBoundingClientRect: () => rectAt(spec.offset, ROW_HEIGHT),
  }));
  // ONE turn, taller than the viewport, starting at the top of the content. Its
  // own top therefore sits above the transcript's top edge, which is the shape
  // the report came from.
  const turnSpec = { offset: 0 };
  const turn: any = {
    isConnected: true,
    closest: () => container,
    querySelectorAll: () => rows,
    getBoundingClientRect: () => rectAt(turnSpec.offset, TURN_HEIGHT),
  };
  // The mobile sticky thread title, drawn over the top of the transcript. It
  // rides the top edge whatever the offset, so its rect is fixed.
  const pinned: any = opts.pinnedHeight
    ? {
        isConnected: true,
        matches: (sel: string) => sel.includes('scroller-pinned'),
        getBoundingClientRect: () => ({
          top: CONTAINER_TOP,
          bottom: CONTAINER_TOP + opts.pinnedHeight!,
          height: opts.pinnedHeight!,
          left: 0, right: 400, width: 400,
        }),
      }
    : null;
  container.children = pinned ? [pinned, turn] : [turn];
  return { container, turn, turnSpec, rows };
}

describe('the turn-control anchor holds the reader, not their turn', () => {
  beforeEach(() => {
    stopFollowingBottom();
    setActiveScrollElement(null);
  });

  it('holds the topmost visible line when the pressed turn is taller than the screen', () => {
    const restoreMO = useMockMO();
    // The reader is 1000px down the turn, so a row reaches the screen only past
    // 980. The one at 970 ends just above it and the one at 1010 starts just
    // below: the second is their topmost visible line.
    const { container, turn, rows } = build({
      scrollTop: 1000,
      scrollHeight: TURN_HEIGHT,
      rows: [{ offset: 0 }, { offset: 900 }, { offset: 970 }, { offset: 1010 }],
    });

    withScrollAnchor(turn, () => {
      // Showing the steps puts 500px of rows between the turn's top and their
      // line. The turn's own top does not move, which is why anchoring it left
      // them adrift by exactly that 500.
      rows[2].spec.offset = 1170;
      rows[3].spec.offset = 1510;
      container.scrollHeight = 3500;
    });

    // Never 1200, which is holding the row that had already scrolled off, and
    // never 1000, which is holding the unmoved turn.
    expect(container.scrollTop).toBe(1500);
    restoreMO();
  });

  it('lands on the seam when the reveal removes the reader’s own line', () => {
    const restoreMO = useMockMO();
    // The reader's topmost line is a STEP row, which hiding the log takes away.
    // Their edge at 1000 falls INSIDE it (990 to 1010), which is what makes the
    // seam theirs to land on. The case below is the one where it does not.
    const { container, turn, rows } = build({
      scrollTop: 1000,
      scrollHeight: TURN_HEIGHT,
      rows: [{ offset: 0 }, { offset: 900 }, { offset: 990, step: true }, { offset: 1050 }],
    });

    withScrollAnchor(turn, () => {
      rows[2].spec.gone = true;
      // 300px of steps went from above them, and their own row closed the 20 of
      // the step that had been sitting on the edge.
      rows[1].spec.offset = 600;
      rows[3].spec.offset = 730;
      container.scrollHeight = 2680;
    });

    // Nothing of theirs is left to hold, so they land where the removed run
    // collapsed to: the bottom of the surviving line above them, 600 + 20. See
    // `anchor-lands-the-reader-on-the-seam.test.ts` for why that is the answer
    // rather than holding the line below at its own old position. Never 1000,
    // which is what holding the unmoved turn would have left.
    expect(container.scrollTop).toBe(620);
    restoreMO();
  });

  it('starts below the sticky title, never on a line hidden behind it', () => {
    const restoreMO = useMockMO();
    // A 40px title over the top of the transcript, so the reader's first
    // READABLE line is the one at 1050. The row at 1010 ends behind the title.
    const { container, turn, rows } = build({
      scrollTop: 1000,
      scrollHeight: TURN_HEIGHT,
      pinnedHeight: 40,
      rows: [{ offset: 0 }, { offset: 900 }, { offset: 1010 }, { offset: 1050 }],
    });

    withScrollAnchor(turn, () => {
      rows[2].spec.offset = 1210;
      rows[3].spec.offset = 1550;
      container.scrollHeight = 3500;
    });

    // Never 1200, which is holding the line the title covers.
    expect(container.scrollTop).toBe(1500);
    restoreMO();
  });

  it('takes no seam when the reader is reading ABOVE the row the reveal removed', () => {
    const restoreMO = useMockMO();
    // Their edge is at 1000 and the turn's FIRST response row starts at 1010,
    // so they are looking at what sits between: their own message, at the top
    // of a turn whose body opens with a step. That message survives the reveal,
    // and it is what stands between them and the row that went.
    const { container, turn, rows } = build({
      scrollTop: 1000,
      scrollHeight: TURN_HEIGHT,
      rows: [{ offset: 1010, step: true }, { offset: 1050 }],
    });

    withScrollAnchor(turn, () => {
      rows[0].spec.gone = true;
      // Only the row BELOW them closes up. Nothing above the edge moves.
      rows[1].spec.offset = 1030;
      container.scrollHeight = 2980;
    });

    // Exactly where they were. A seam here walks DOWN to the first row that
    // lived and puts its top on their edge, dragging them 30px for a reveal
    // that removed nothing they could see.
    expect(container.scrollTop).toBe(1000);
    restoreMO();
  });

  it('takes the seam when the reader sits in the margin inside the removed run', () => {
    const restoreMO = useMockMO();
    // Their edge at 1000 lands in the 8px gap between two rows: the prose row
    // ending at 996 and the step starting at 1004. That gap is the two rows'
    // shared margin. Nothing survives in it, so the reader IS inside the run
    // the reveal is about to take. It is the shape a real transcript makes
    // wherever prose meets a step run (`.response-chunk`'s own margin).
    const { container, turn, rows } = build({
      scrollTop: 1000,
      scrollHeight: TURN_HEIGHT,
      rows: [
        { offset: 900 },
        { offset: 976 },
        { offset: 1004, step: true },
        { offset: 1024, step: true },
        { offset: 1600 },
      ],
    });

    withScrollAnchor(turn, () => {
      rows[2].spec.gone = true;
      rows[3].spec.gone = true;
      // A long run below them goes as well, so the row that survives beneath
      // closes right up under the prose.
      rows[4].spec.offset = 1000;
      container.scrollHeight = 2400;
    });

    // The run collapsed to one point, and they belong on it: the bottom of the
    // surviving prose above, 976 + 20. Refusing the seam here held the response
    // panel instead and left them screens away.
    expect(container.scrollTop).toBe(996);
    restoreMO();
  });

  it('is not decided by the pixel a previous seam left showing', () => {
    const restoreMO = useMockMO();
    // The shape a seam leaves behind. The last press rested the prose row's
    // BOTTOM on the reader's edge. The correction rounds its target to a whole
    // pixel, so that row now measures 1.13px past the edge, and none of it is
    // readable. Taking it as the reader's line holds it, which puts the whole
    // revealed run back under their eye: the 748px WebKit reported.
    const { container, turn, rows } = build({
      scrollTop: 1000,
      scrollHeight: TURN_HEIGHT,
      rows: [{ offset: 900 }, { offset: 981.13 }, { offset: 1006 }],
    });

    withScrollAnchor(turn, () => {
      // Showing the steps puts a 700px run between the two prose rows.
      rows[2].spec.offset = 1706;
      container.scrollHeight = TURN_HEIGHT + 700;
    });

    // Held on the row they can actually READ, 700 below where it was. Holding
    // the hair above them would have left the container at 1000.
    expect(container.scrollTop).toBe(1700);
    restoreMO();
  });

  it('finds the reader’s line in the NEXT turn when this one ends above them', () => {
    const restoreMO = useMockMO();
    // Two turns. The reader rests in the first turn's trailing chrome, below
    // its last row and above the second turn's first one. Reading only the turn
    // their edge is IN answered "no line", and the correction fell back to that
    // turn's own top: a point 1000px above them, so the reveal's growth between
    // the two landed on the reader.
    const first: any = {
      isConnected: true,
      closest: () => container,
      matches: () => false,
      querySelectorAll: () => firstRows,
      getBoundingClientRect: () => rectAt(0, 1040),
    };
    const secondRows: any[] = [];
    const second: any = {
      isConnected: true,
      closest: () => container,
      matches: () => false,
      querySelectorAll: () => secondRows,
      getBoundingClientRect: () => rectAt(1040, 900),
    };
    const container: any = {
      scrollTop: 1000,
      scrollHeight: 3000,
      clientHeight: CLIENT_HEIGHT,
      style: { overflow: '' },
      contains: () => false,
      closest: () => null,
      children: [first, second],
      getBoundingClientRect: () => ({
        top: CONTAINER_TOP,
        bottom: CONTAINER_TOP + CLIENT_HEIGHT,
        height: CLIENT_HEIGHT,
        left: 0, right: 400, width: 400,
      }),
    };
    const rectAt = (offset: number, height: number) => {
      const top = CONTAINER_TOP + offset - container.scrollTop;
      return { top, bottom: top + height, height, left: 0, right: 400, width: 400 };
    };
    const row = (spec: { offset: number }) => ({
      spec,
      isConnected: true,
      matches: () => false,
      getBoundingClientRect: () => rectAt(spec.offset, ROW_HEIGHT),
    });
    // Every one of the first turn's rows is above the reader's edge at 1000.
    const firstRows = [row({ offset: 100 }), row({ offset: 960 })];
    const line = row({ offset: 1100 });
    secondRows.push(line);

    withScrollAnchor(first, () => {
      // Showing the steps grows the first turn by 400, carrying the second
      // turn's rows down with it.
      line.spec.offset = 1500;
      container.scrollHeight = 3400;
    });

    // Held on their own line, 400 below where it was. Holding the first turn
    // would have left them at 1000, a screenful adrift.
    expect(container.scrollTop).toBe(1400);
    restoreMO();
  });

  it('takes the pressed turn when nothing finer is on offer', () => {
    const restoreMO = useMockMO();
    // A turn with no rows and no panels, which is what a bare message is while
    // its response has drawn nothing.
    const { container, turn, turnSpec } = build({
      scrollTop: 0,
      scrollHeight: TURN_HEIGHT,
      rows: [],
    });

    withScrollAnchor(turn, () => { turnSpec.offset = 200; });

    expect(container.scrollTop).toBe(200);
    restoreMO();
  });
});
