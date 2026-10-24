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
  notePolledViewport,
  noteWakeViewport,
  noteKeyboardClosed,
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
    expect(keyboardCloseState()).toEqual({ at: 2000, relaidOut: true, closes: 1, path: 'resize' });
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
    expect(keyboardCloseState()).toEqual({ at: 2000, relaidOut: false, closes: 1, path: 'resize' });
  });

  it('retires the stamp when the keys come back', () => {
    // The verdict reading the stamp is named for a keyboard that has GONE. A
    // line citing a close a minute old, with the keys up, says the opposite of
    // the viewport printed beside it.
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    expect(keyboardCloseState().at).toBe(2000);
    noteViewportResize(up, 3000);
    expect(keyboardCloseState().at).toBeNull();
    expect(keyboardCloseState().path).toBeNull();
    // A running total, so the count of closes survives the retirement.
    expect(keyboardCloseState().closes).toBe(1);
  });
});

describe('noteKeyboardClosed: the resume that fires no resize', () => {
  it('stamps and relayouts on the caller\'s word', () => {
    // iOS dismisses the keyboard across a suspend and reports no resize, so the
    // fold can never see this edge. The wake path knows, and says so.
    noteKeyboardClosed(5000);
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState()).toEqual({ at: 5000, relaidOut: true, closes: 1, path: 'wake' });
  });

  it('ignores the stale shrunk height iOS leaves behind', () => {
    // `visualViewport.height` stays pinned at the keyboard-up value after a
    // wake. Folding it would arm a second close on the correction, and spend a
    // relayout on a transition that already had one.
    noteKeyboardClosed(5000);
    written = [];
    noteViewportResize(up, 5100);
    noteViewportResize(down, 5200);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });

  it('answers a keyboard the user genuinely reopens after the echo window', () => {
    // The bound is what makes this possible. An unbounded suppression waits for
    // a reading it accepts. The reopen is then swallowed and the REAL close
    // after it is lost, which is the wedge this module exists to clear.
    noteKeyboardClosed(5000);
    noteViewportResize(up, 9000);
    expect(keyboardCloseState().at).toBeNull();
    written = [];
    noteViewportResize(down, 12000);
    expect(keyboardCloseState()).toEqual({ at: 12000, relaidOut: true, closes: 2, path: 'resize' });
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
  });

  it('takes one resume once, though it arrives as two events', () => {
    // `visibilitychange` AND `pageshow` both fire, milliseconds apart. A second
    // stamp restarts every silence the ledger measures from the close.
    noteKeyboardClosed(5000);
    written = [];
    noteKeyboardClosed(5010);
    expect(written).toEqual([]);
    expect(keyboardCloseState()).toEqual({ at: 5000, relaidOut: true, closes: 1, path: 'wake' });
  });

  it('answers a LATER resume, with no reading in between', () => {
    // The wedge is the state where no reading arrives, so nothing retires the
    // stamp. Guarding on the stamp alone would swallow every resume after the
    // first, and the user picking the phone up again would get no relayout.
    noteKeyboardClosed(5000);
    written = [];
    noteKeyboardClosed(65000);
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState()).toEqual({ at: 65000, relaidOut: true, closes: 2, path: 'wake' });
  });

  it('answers a wake again once a cover has retired the stamp', () => {
    noteKeyboardClosed(5000);
    noteViewportResize(up, 9000);
    noteKeyboardClosed(10000);
    expect(keyboardCloseState()).toEqual({ at: 10000, relaidOut: true, closes: 2, path: 'wake' });
  });

  it('holds the echo window open across the paired event', () => {
    // The second event stamps nothing, but the viewport is no less stale.
    noteKeyboardClosed(5000);
    noteKeyboardClosed(5010);
    written = [];
    noteViewportResize(up, 5100);
    noteViewportResize(down, 5200);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });

  it('is not disarmed by a reading the fold refused to judge', () => {
    // The fold declines a viewport it cannot measure, and keeps its watch. The
    // echo window is time-bound, so such a reading cannot shorten it either.
    noteKeyboardClosed(5000);
    written = [];
    noteViewportResize({ height: KEYBOARD_UP, layoutViewport: 0 }, 5050);
    noteViewportResize(up, 5100);
    noteViewportResize(down, 5200);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });
});

describe('noteWakeViewport: the resume that came back corrected', () => {
  it('folds the restored reading the keys left behind', () => {
    // iOS does not ALWAYS pin the height across a suspend. A corrected one is
    // a real edge to fold. The wake handler is the only observer that will
    // ever offer it, since no resize fired.
    noteViewportResize(up, 1000);
    written = [];
    noteWakeViewport(down, 5000);
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState()).toEqual({ at: 5000, relaidOut: true, closes: 1, path: 'wake' });
  });

  it('says nothing when the keys were never up', () => {
    noteWakeViewport(down, 5000);
    expect(written).toEqual([]);
    expect(keyboardCloseState().at).toBeNull();
  });

  it('cannot double-spend with a resize that did arrive', () => {
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    written = [];
    noteWakeViewport(down, 5000);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });
});

describe('one resume, whichever pair of viewports it arrives with', () => {
  // `visibilitychange` and `pageshow` both fire, milliseconds apart, and iOS
  // can hand them different viewports. All four orderings are one close.
  const wakePinned = (t: number) => noteKeyboardClosed(t);
  const wakeCorrected = (t: number) => noteWakeViewport(down, t);

  beforeEach(() => {
    // The keys were up before the suspend, so a cover is armed.
    noteViewportResize(up, 1000);
    written = [];
  });

  it('pinned, then pinned', () => {
    wakePinned(5000); wakePinned(5010);
    expect(keyboardCloseState().closes).toBe(1);
    expect(written).toHaveLength(2);
  });

  it('pinned, then corrected', () => {
    wakePinned(5000); wakeCorrected(5010);
    expect(keyboardCloseState().closes).toBe(1);
    expect(written).toHaveLength(2);
  });

  it('corrected, then corrected', () => {
    wakeCorrected(5000); wakeCorrected(5010);
    expect(keyboardCloseState().closes).toBe(1);
    expect(written).toHaveLength(2);
  });

  it('corrected, then pinned', () => {
    // The ordering that was unguarded: only the pinned half opened the window,
    // so a corrected first event left the second one to stamp again.
    wakeCorrected(5000); wakePinned(5010);
    expect(keyboardCloseState().closes).toBe(1);
    expect(written).toHaveLength(2);
  });

  it('lets the pinned half land a close the corrected half could not find', () => {
    // The wedge case, and the whole reason the twin test asks for a standing
    // close as well as an open window. No resize ever fired, so no cover is
    // armed, so the corrected half observes no edge and stamps nothing. The
    // pinned half needs its word taken, window or no window.
    resetKeyboardCloseState();
    written = [];
    wakeCorrected(5000);
    expect(keyboardCloseState().at).toBeNull();
    wakePinned(5010);
    expect(keyboardCloseState()).toEqual({ at: 5010, relaidOut: true, closes: 1, path: 'wake' });
    expect(written).toHaveLength(2);
  });

  it('suppresses the echo after a CORRECTED wake too', () => {
    // The window's other job. A covered reading landing just after belongs to
    // the resume, and folding it would arm a close on its own correction.
    wakeCorrected(5000);
    written = [];
    noteViewportResize(up, 5100);
    noteViewportResize(down, 5200);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });
});

describe('notePolledViewport: the reading nobody asked for', () => {
  it('answers a close no event announced', () => {
    notePolledViewport(up, 1000);
    written = [];
    notePolledViewport(down, 4000);
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState().path).toBe('poll');
  });

  it('folds into the SAME watch the resize handler uses', () => {
    // One relayout per close, whichever path saw it first. Two observers of one
    // transition must not spend it twice.
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    written = [];
    notePolledViewport(down, 3000);
    notePolledViewport(down, 6000);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });

  it('costs a desktop nothing, since no reading ever covers', () => {
    for (let t = 1000; t < 40000; t += 3000) notePolledViewport(down, t);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(0);
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

  it('polls from the handler that owns the height, gated and cleaned up', () => {
    // The poll lives HERE rather than in the leaf, because a poll-seen close is
    // by definition one no resize settled the height for. Parked anywhere else
    // it would bounce off the keyboard-shrunk value and restore it.
    const poll = source.slice(source.indexOf('const pollViewport = () => {'));
    const body = poll.slice(0, poll.indexOf('};'));
    expect(body.indexOf('setHeight(')).toBeGreaterThanOrEqual(0);
    expect(body.indexOf('notePolledViewport(')).toBeGreaterThan(body.indexOf('setHeight('));
    // Default off, live, and nothing scheduled while the gate is down.
    expect(body).toContain('perfRecordingOn()');
    expect(source).toContain('onPerfEnabledChange((on) => (on ? startPoll() : stopPoll()))');
    expect(source).toContain('stopPoll();\n      unsubscribeGate();');
  });

  it('acts on the change no handler saw, so EVERY handler moves the mark', () => {
    // The poll stands in for a resize that did not arrive. Mark it in the tick
    // alone, and the first tick after a HANDLED close acts on that close too.
    // It then runs `syncBand` with the keys down inside a live focus window,
    // wiping the band `armBand` reserved at `focusin`. Two earlier fixes exist
    // to protect that reserve.
    expect(source).toContain('if (vv.height === observedHeight) return;');
    // Anchored, so the `let` that declares it is not counted as a write.
    const writes = source.match(/^\s*observedHeight = vv\.height;$/gm) ?? [];
    // One per observer, the tick included. A rotation counts: it moves the
    // viewport, so a tick after one would act on a change already handled.
    expect(writes).toHaveLength(4);
    const handlers = [
      'const onResize = () => {',
      'const onWake = () => {',
      'const onOrientationChange = () => {',
    ];
    for (const handler of handlers) {
      const body = source.slice(source.indexOf(handler));
      expect(body.slice(0, body.indexOf('};'))).toContain('observedHeight = vv.height;');
    }
  });

  it('tells the stamp about a wake, which fires no resize of its own', () => {
    // The hole the nineteenth report found. `onWake` restores --app-height on a
    // resume iOS dismissed the keyboard across, and no reading ever folds.
    const wake = source.slice(source.indexOf('const onWake = () => {'));
    const body = wake.slice(0, wake.indexOf('};'));
    expect(body).toContain('noteKeyboardClosed()');
    // The stamp on the handler's word goes ONLY where the height came back
    // pinned. A corrected one carries a real edge, and handing it to the fold
    // is the only way that close is ever seen: no resize fired for it.
    expect(body).toContain('if (vvLooksShrunk) noteKeyboardClosed();');
    expect(body).toContain('else noteWakeViewport(');
    expect(body.indexOf('noteKeyboardClosed(')).toBeGreaterThan(body.indexOf('setHeight('));
  });
});

describe('the fix is a leaf', () => {
  // The probe imports this module, and the probe is a temporary measure. A
  // dependency taken here reaches the probe, and a cycle through it would
  // reach the store. The poll's gate never enters: it lives with the caller,
  // beside the handler that owns the height.
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../keyboardCloseRelayout.ts'), 'utf-8');

  it('imports nothing at all', () => {
    expect(source).not.toMatch(/^\s*import\s/m);
  });

  it('presses nothing, on any path', () => {
    // ADR 0225. The relayout is the whole recovery, and no path dispatches.
    expect(source).not.toMatch(/\.click\(|dispatchEvent\(|\.focus\(|\.blur\(/);
  });
});
