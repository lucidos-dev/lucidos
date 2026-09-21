import { describe, it, expect, beforeEach, afterEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import {
  foldViewportReading,
  bounceHeight,
  noteViewportResize,
  keyboardCloseState,
  resetKeyboardCloseState,
} from '../keyboardCloseRelayout';

// The recovery for the iOS wedge where the page stops receiving touches once
// the keyboard closes. Both decisions here are pure, so they test with no DOM,
// which is the convention the composer probes already keep.
//
// The episode this answers:
// docs/plans/2026-09-20-the-composer-recovers-when-the-keyboard-closes.md

/** The reported iPhone. The shell is 852 with the keyboard down, and 476 with
 *  it up, while the layout viewport stays at 852 throughout. */
const FULL = 852;
const KEYBOARD_UP = 476;

const up = { height: KEYBOARD_UP, layoutViewport: FULL };
const down = { height: FULL, layoutViewport: FULL };

/** Fold a whole run of readings, and report what the last one settled. */
function run(...samples: { height: number; layoutViewport: number }[]) {
  let watch: number | null = null;
  let closed = false;
  for (const sample of samples) {
    const folded = foldViewportReading(watch, sample);
    watch = folded.watch;
    closed = folded.closed;
  }
  return { watch, closed };
}

describe('foldViewportReading: the edge the wedge starts on', () => {
  it('answers the close, which is the whole trigger', () => {
    expect(run(up, down).closed).toBe(true);
  });

  it('carries the cover across the frames of a slow close', () => {
    // The interactive dismissal. iOS reports the keyboard sliding out as a run
    // of heights, and a keyboard half-way out is neither covered nor restored.
    // Comparing only with the previous reading loses the close outright.
    const midway = [700, 820].map((height) => ({ height, layoutViewport: FULL }));
    expect(run(up, ...midway, down).closed).toBe(true);
  });

  it('says nothing on the keyboard opening', () => {
    // The wedge starts when the keys LEAVE. Relaying out on the way in would
    // cost a layout per focus and answer no report.
    expect(run(down, up).closed).toBe(false);
  });

  it('spends one relayout across the animation, not one per resize', () => {
    // The edge, never the state. Every reading after the first restored one
    // has a restored viewport behind it too.
    expect(run(up, down, down).closed).toBe(false);
    expect(run(up, down, down, down).closed).toBe(false);
  });

  it('stays silent while the keyboard is still up', () => {
    expect(run(up, up).closed).toBe(false);
  });

  it('refuses a partial restore, which is some other chrome leaving', () => {
    // An accessory bar or a toolbar is tens of pixels. The keyboard is 376 of
    // an 852 phone, and only its going is the transition this answers.
    const accessoryBar = { height: FULL - 55, layoutViewport: FULL };
    expect(run(accessoryBar, down).closed).toBe(false);
  });

  it('accepts a restore that lands a hair off, since the height is fractional', () => {
    const nearlyFull = { height: FULL - 1.5, layoutViewport: FULL };
    expect(run(up, nearlyFull).closed).toBe(true);
  });

  it('holds the watch open on a restore still short of the viewport', () => {
    const stillCovered = { height: FULL - 40, layoutViewport: FULL };
    const midway = run(up, stillCovered);
    expect(midway.closed).toBe(false);
    expect(midway.watch).toBe(FULL);
  });

  it('retires the watch on a rotation, a different geometry', () => {
    // Landscape's full height is under portrait's keyboard-up height, so
    // comparing the numbers alone reads a rotation as a close.
    const landscape = { height: 393, layoutViewport: 393 };
    expect(run(up, landscape).closed).toBe(false);
  });

  it('re-arms in the new geometry, so the next close is not lost', () => {
    const landscapeUp = { height: 180, layoutViewport: 393 };
    const landscapeDown = { height: 393, layoutViewport: 393 };
    expect(run(up, landscapeUp, landscapeDown).closed).toBe(true);
  });

  it('keeps the watch through a viewport it cannot measure', () => {
    // A reading it cannot judge must not disarm one it already has.
    expect(run(up, { height: FULL, layoutViewport: 0 }, down).closed).toBe(true);
    expect(run(up, { height: NaN, layoutViewport: FULL }, down).closed).toBe(true);
  });
});

describe('bounceHeight: the relayout that stands in for a keyboard bounce', () => {
  it('travels the keyboard\'s own span', () => {
    // A 476 shell inside an 844 layout viewport is a 368px keyboard.
    expect(bounceHeight(476, 844)).toBe(108);
  });

  // Growing clamps every scroller's offset at that layout, and the restore
  // does not put those offsets back (ADR 0183).
  it('goes down, never up', () => {
    expect(bounceHeight(476, 844)).toBeLessThan(476);
    expect(bounceHeight(844, 844)).toBeLessThan(844);
  });

  it('still moves when no keyboard is up', () => {
    // The state the transition trigger fires in: the keys have already gone, so
    // the span is one pixel. The ledger shows one pixel clearing the wedge.
    expect(bounceHeight(844, 844)).toBe(843);
  });

  it('never asks for a shell of no height', () => {
    expect(bounceHeight(10, 900)).toBe(1);
  });
});

/** Every `--app-height` the shell was written to, in order. The relayout's
 *  whole signature is two writes in one task, so the sequence IS the
 *  assertion. */
let written: string[] = [];
let priorHeight = `${FULL}px`;
let realRoot: unknown;

beforeEach(() => {
  resetKeyboardCloseState();
  written = [];
  priorHeight = `${FULL}px`;
  const doc = globalThis.document as unknown as Record<string, unknown>;
  realRoot = doc.documentElement;
  doc.documentElement = {
    offsetHeight: 0,
    style: {
      getPropertyValue: (name: string) => (name === '--app-height' ? priorHeight : ''),
      setProperty: (name: string, value: string) => {
        if (name !== '--app-height') return;
        written.push(value);
        priorHeight = value;
      },
    },
  };
  const g = globalThis as unknown as Record<string, unknown>;
  g.innerHeight = FULL;
  g.scrollY = 0;
  g.scrollTo = () => {};
});

afterEach(() => {
  const doc = globalThis.document as unknown as Record<string, unknown>;
  doc.documentElement = realRoot;
});

describe('noteViewportResize: the app hands its own reading over', () => {
  it('relayouts on the close, and stamps when it happened', () => {
    noteViewportResize(up, 1000);
    expect(written).toEqual([]);
    noteViewportResize(down, 2000);
    // Away, then straight back. One task, so nothing is painted in between.
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState()).toEqual({ at: 2000, relaidOut: true, closes: 1 });
  });

  it('leaves a healthy page alone on every other resize', () => {
    // The whole cost on a page nobody is stuck on. A resize that opened the
    // keyboard, and a second one at the same height, both write nothing.
    noteViewportResize(down, 1000);
    noteViewportResize(up, 2000);
    noteViewportResize(up, 3000);
    noteViewportResize({ height: KEYBOARD_UP, layoutViewport: FULL }, 4000);
    expect(written).toEqual([]);
    expect(keyboardCloseState().at).toBeNull();
  });

  it('spends one relayout per close, not one per resize in the animation', () => {
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    noteViewportResize(down, 2100);
    noteViewportResize(down, 2200);
    expect(written).toHaveLength(2);
  });

  it('relayouts again on the NEXT close, since a wedge can return', () => {
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    noteViewportResize(up, 3000);
    noteViewportResize(down, 4000);
    expect(written).toHaveLength(4);
    expect(keyboardCloseState().at).toBe(4000);
    // A running total, so a reader counts the closes in any stretch by
    // subtracting the two ends.
    expect(keyboardCloseState().closes).toBe(2);
  });

  it('leaves a unit it does not own alone', () => {
    // Writing `851px` over a `100%` would change the unit for the instant
    // before the restore, on a shell somebody else is sizing.
    priorHeight = '100%';
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    expect(written).toEqual([]);
    expect(keyboardCloseState().relaidOut).toBe(false);
  });

  it('says the relayout could not run on a shell it does not own', () => {
    // `--app-height` unset is a shell this must not start writing. The stamp
    // still lands, so the ledger can rule the recovery out rather than score it.
    priorHeight = '';
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    expect(written).toEqual([]);
    expect(keyboardCloseState()).toEqual({ at: 2000, relaidOut: false, closes: 1 });
  });
});

describe('the shell hands its viewport reading over', () => {
  // The whole fix is one call. Nothing else fails if it goes, and no unit test
  // can see a `useEffect` handler, so the wiring is pinned by reading it.
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../MobileSwipeContainer.tsx'), 'utf-8');

  it('calls the relayout from the handler that owns --app-height', () => {
    expect(source).toContain(
      "noteViewportResize({ height: vv.height, layoutViewport: window.innerHeight })",
    );
  });

  it('calls it AFTER writing the height, so the bounce starts from the new one', () => {
    // Ordering is the invariant. A relayout in front of `setHeight` would
    // bounce off the keyboard-shrunk value and restore that instead.
    const resize = source.slice(source.indexOf('const onResize = () => {'));
    const body = resize.slice(0, resize.indexOf('};'));
    expect(body.indexOf('setHeight(')).toBeGreaterThanOrEqual(0);
    expect(body.indexOf('noteViewportResize(')).toBeGreaterThan(body.indexOf('setHeight('));
  });
});
