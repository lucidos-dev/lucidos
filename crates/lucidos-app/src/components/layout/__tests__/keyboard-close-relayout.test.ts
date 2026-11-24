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


const FULL = 852;
const KEYBOARD_UP = 476;

const up = { height: KEYBOARD_UP, layoutViewport: FULL };
const down = { height: FULL, layoutViewport: FULL };

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
    const midway = [700, 820].map((height) => ({ height, layoutViewport: FULL }));
    expect(run(up, ...midway, down).closed).toBe(true);
  });

  it('says nothing on the keyboard opening', () => {
    expect(run(down, up).closed).toBe(false);
  });

  it('spends one relayout across the animation, not one per resize', () => {
    expect(run(up, down, down).closed).toBe(false);
    expect(run(up, down, down, down).closed).toBe(false);
  });

  it('stays silent while the keyboard is still up', () => {
    expect(run(up, up).closed).toBe(false);
  });

  it('refuses a partial restore, which is some other chrome leaving', () => {
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
    const landscape = { height: 393, layoutViewport: 393 };
    expect(run(up, landscape).closed).toBe(false);
  });

  it('re-arms in the new geometry, so the next close is not lost', () => {
    const landscapeUp = { height: 180, layoutViewport: 393 };
    const landscapeDown = { height: 393, layoutViewport: 393 };
    expect(run(up, landscapeUp, landscapeDown).closed).toBe(true);
  });

  it('keeps the watch through a viewport it cannot measure', () => {
    expect(run(up, { height: FULL, layoutViewport: 0 }, down).closed).toBe(true);
    expect(run(up, { height: NaN, layoutViewport: FULL }, down).closed).toBe(true);
  });

  it('reads a zero-height viewport as no reading, never a keyboard', () => {
    const zero = { height: 0, layoutViewport: FULL };
    expect(foldViewportReading(null, zero)).toEqual({ watch: null, closed: false, covered: false });
    expect(run(down, zero, down).closed).toBe(false);
    expect(run(up, zero, down).closed).toBe(true);
  });
});

describe('bounceHeight: the relayout that stands in for a keyboard bounce', () => {
  it('travels the keyboard\'s own span', () => {
    expect(bounceHeight(476, 844)).toBe(108);
  });

  it('goes down, never up', () => {
    expect(bounceHeight(476, 844)).toBeLessThan(476);
    expect(bounceHeight(844, 844)).toBeLessThan(844);
  });

  it('still moves when no keyboard is up', () => {
    expect(bounceHeight(844, 844)).toBe(843);
  });

  it('never asks for a shell of no height', () => {
    expect(bounceHeight(10, 900)).toBe(1);
  });
});

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
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState()).toEqual({ at: 2000, relaidOut: true, closes: 1, path: 'resize' });
  });

  it('leaves a healthy page alone on every other resize', () => {
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
    expect(keyboardCloseState().closes).toBe(2);
  });

  it('leaves a unit it does not own alone', () => {
    priorHeight = '100%';
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    expect(written).toEqual([]);
    expect(keyboardCloseState().relaidOut).toBe(false);
  });

  it('says the relayout could not run on a shell it does not own', () => {
    priorHeight = '';
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    expect(written).toEqual([]);
    expect(keyboardCloseState()).toEqual({ at: 2000, relaidOut: false, closes: 1, path: 'resize' });
  });

  it('retires the stamp when the keys come back', () => {
    noteViewportResize(up, 1000);
    noteViewportResize(down, 2000);
    expect(keyboardCloseState().at).toBe(2000);
    noteViewportResize(up, 3000);
    expect(keyboardCloseState().at).toBeNull();
    expect(keyboardCloseState().path).toBeNull();
    expect(keyboardCloseState().closes).toBe(1);
  });
});

describe('noteKeyboardClosed: the resume that fires no resize', () => {
  it('stamps and relayouts on the caller\'s word', () => {
    noteKeyboardClosed(5000);
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
    expect(keyboardCloseState()).toEqual({ at: 5000, relaidOut: true, closes: 1, path: 'wake' });
  });

  it('ignores the stale shrunk height iOS leaves behind', () => {
    noteKeyboardClosed(5000);
    written = [];
    noteViewportResize(up, 5100);
    noteViewportResize(down, 5200);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });

  it('answers a keyboard the user genuinely reopens after the echo window', () => {
    noteKeyboardClosed(5000);
    noteViewportResize(up, 9000);
    expect(keyboardCloseState().at).toBeNull();
    written = [];
    noteViewportResize(down, 12000);
    expect(keyboardCloseState()).toEqual({ at: 12000, relaidOut: true, closes: 2, path: 'resize' });
    expect(written).toEqual([`${FULL - 1}px`, `${FULL}px`]);
  });

  it('takes one resume once, though it arrives as two events', () => {
    noteKeyboardClosed(5000);
    written = [];
    noteKeyboardClosed(5010);
    expect(written).toEqual([]);
    expect(keyboardCloseState()).toEqual({ at: 5000, relaidOut: true, closes: 1, path: 'wake' });
  });

  it('answers a LATER resume, with no reading in between', () => {
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
    noteKeyboardClosed(5000);
    noteKeyboardClosed(5010);
    written = [];
    noteViewportResize(up, 5100);
    noteViewportResize(down, 5200);
    expect(written).toEqual([]);
    expect(keyboardCloseState().closes).toBe(1);
  });

  it('is not disarmed by a reading the fold refused to judge', () => {
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
  const wakePinned = (t: number) => noteKeyboardClosed(t);
  const wakeCorrected = (t: number) => noteWakeViewport(down, t);

  beforeEach(() => {
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
    wakeCorrected(5000); wakePinned(5010);
    expect(keyboardCloseState().closes).toBe(1);
    expect(written).toHaveLength(2);
  });

  it('lets the pinned half land a close the corrected half could not find', () => {
    resetKeyboardCloseState();
    written = [];
    wakeCorrected(5000);
    expect(keyboardCloseState().at).toBeNull();
    wakePinned(5010);
    expect(keyboardCloseState()).toEqual({ at: 5010, relaidOut: true, closes: 1, path: 'wake' });
    expect(written).toHaveLength(2);
  });

  it('suppresses the echo after a CORRECTED wake too', () => {
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
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../MobileSwipeContainer.tsx'), 'utf-8');

  it('calls the relayout from the handler that owns --app-height', () => {
    expect(source).toContain(
      "noteViewportResize({ height: vv.height, layoutViewport: window.innerHeight })",
    );
  });

  it('calls it AFTER writing the height, so the bounce starts from the new one', () => {
    const resize = source.slice(source.indexOf('const onResize = () => {'));
    const body = resize.slice(0, resize.indexOf('};'));
    expect(body.indexOf('setHeight(')).toBeGreaterThanOrEqual(0);
    expect(body.indexOf('noteViewportResize(')).toBeGreaterThan(body.indexOf('setHeight('));
  });

  it('polls from the handler that owns the height, gated and cleaned up', () => {
    const poll = source.slice(source.indexOf('const pollViewport = () => {'));
    const body = poll.slice(0, poll.indexOf('};'));
    expect(body.indexOf('setHeight(')).toBeGreaterThanOrEqual(0);
    expect(body.indexOf('notePolledViewport(')).toBeGreaterThan(body.indexOf('setHeight('));
    expect(body).toContain('isPerfEnabled()');
    expect(source).toContain('onPerfOptInChange((on) => (on ? startPoll() : stopPoll()))');
    expect(source).toMatch(/stopPoll\(\);\s*unsubscribeOptIn\(\);/);
  });

  it('acts on the change no handler saw, so EVERY handler moves the mark', () => {
    expect(source).toContain('if (vv.height === observedHeight) return;');
    const writes = source.match(/^\s*observedHeight = vv\.height;$/gm) ?? [];
    expect(writes).toHaveLength(4);
    const handlers = [
      'const onResize = () => {',
      'const onWake = () => {',
      'const onLayoutViewportChange = () => {',
    ];
    for (const handler of handlers) {
      const body = source.slice(source.indexOf(handler));
      expect(body.slice(0, body.indexOf('};'))).toContain('observedHeight = vv.height;');
    }
  });

  it('tells the stamp about a wake, which fires no resize of its own', () => {
    const wake = source.slice(source.indexOf('const onWake = () => {'));
    const body = wake.slice(0, wake.indexOf('};'));
    expect(body).toContain('noteKeyboardClosed()');
    expect(body).toContain('if (vvLooksShrunk) noteKeyboardClosed();');
    expect(body).toContain('else noteWakeViewport(');
    expect(body.indexOf('noteKeyboardClosed(')).toBeGreaterThan(body.indexOf('setHeight('));
  });
});

describe('the fix is a leaf', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const source = readFileSync(resolve(here, '../keyboardCloseRelayout.ts'), 'utf-8');

  it('imports nothing at all', () => {
    expect(source).not.toMatch(/^\s*import\s/m);
  });

  it('presses nothing, on any path', () => {
    expect(source).not.toMatch(/\.click\(|dispatchEvent\(|\.focus\(|\.blur\(/);
  });
});
