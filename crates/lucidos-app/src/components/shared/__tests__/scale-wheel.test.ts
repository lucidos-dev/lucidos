import { describe, it, expect, beforeEach, afterEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
import {
  WHEEL_GESTURE_GAP_MS,
  createWheelZoom,
  emptyWheelBank,
  foldWheelEvent,
  takeWheelStep,
  wheelZoomDistance,
  type WheelBank,
} from '../scaleWheel';

/** How a gesture reached the page: a wheel turned with Cmd or Ctrl held, or a
 *  trackpad pinch, which Chrome flags as ctrl-wheel with no key down. */
const KEYED_WHEEL = true;
const PINCH = false;

/** Feed a stream of events and count the steps they buy, draining greedily.
 *  The per-frame cap lives in the component; here every banked step is taken,
 *  which is what says how many the gesture ASKED for. */
function stepsFor(
  deltas: number[], { deltaMode = 0, gapMs = 8, keyHeld = KEYED_WHEEL } = {},
): number {
  let bank: WheelBank = emptyWheelBank();
  let at = 1000;
  let steps = 0;
  for (const d of deltas) {
    at += gapMs;
    bank = foldWheelEvent(bank, wheelZoomDistance(d, deltaMode), at, keyHeld);
    for (;;) {
      const taken = takeWheelStep(bank);
      bank = taken.bank;
      if (taken.step === 0) break;
      steps += taken.step;
    }
  }
  return steps;
}

describe('wheelZoomDistance', () => {
  it('flips the sign, so scrolling up zooms in', () => {
    expect(wheelZoomDistance(-100, 0)).toBe(100);
    expect(wheelZoomDistance(100, 0)).toBe(-100);
  });

  it('converts a line-mode notch to the same distance as a pixel-mode one', () => {
    // Firefox reports three lines for one detent. It has to buy one step, the
    // same as Chrome's 100 pixels.
    expect(wheelZoomDistance(-3, 1)).toBe(wheelZoomDistance(-100, 0));
  });

  it('answers zero for a delta that is not a number', () => {
    expect(wheelZoomDistance(NaN, 0)).toBe(0);
  });
});

describe('a mouse notch is still exactly one step', () => {
  it('in pixel mode', () => {
    expect(stepsFor([-100])).toBe(1);
    expect(stepsFor([100])).toBe(-1);
  });

  it('in line mode', () => {
    expect(stepsFor([-3], { deltaMode: 1 })).toBe(1);
  });

  it('three notches are three steps', () => {
    expect(stepsFor([-100, -100, -100])).toBe(3);
  });

  it('a slow notch smaller than 100px still steps, every time', () => {
    // macOS Chrome reports a slow mouse tick as a few pixels. Each tick lands
    // after the gesture gap, so nothing can bank across them.
    const slow = { gapMs: WHEEL_GESTURE_GAP_MS + 50 };
    expect(stepsFor([-4, -4, -4], slow)).toBe(3);
    expect(stepsFor([40, 40], slow)).toBe(-2);
  });
});

describe('a pinch spends distance, not events', () => {
  it('twenty small-delta events are no step, not twenty', () => {
    // A macOS trackpad pinch, roughly. Twenty events carrying 4px each are 80px
    // of travel, short of one notch.
    expect(stepsFor(Array(20).fill(-4), { keyHeld: PINCH })).toBe(0);
  });

  it('the steps track the distance travelled', () => {
    // 50 events of 8px is 400px, which is four notches.
    expect(stepsFor(Array(50).fill(-8), { keyHeld: PINCH })).toBe(4);
    // 40 events of 8px is 320px: three notches, and the remainder buys nothing.
    expect(stepsFor(Array(40).fill(-8), { keyHeld: PINCH })).toBe(3);
  });

  it('its first event is not floored to a notch', () => {
    // The floor belongs to a slow mouse tick. A pinch's first event is one
    // sliver of the gesture, and flooring it bought a step nobody asked for.
    const bank = foldWheelEvent(emptyWheelBank(), 4, 1000, PINCH);
    expect(bank.distance).toBe(4);
    expect(takeWheelStep(bank).step).toBe(0);
  });
});

describe('the bank belongs to one gesture', () => {
  it('an idle gap drops what the previous gesture banked', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 90, 1000, KEYED_WHEEL);
    bank = takeWheelStep(bank).bank;
    bank = foldWheelEvent(bank, 10, 1008, KEYED_WHEEL);
    bank = foldWheelEvent(bank, 90, 1008 + WHEEL_GESTURE_GAP_MS + 1, KEYED_WHEEL);
    // The new gesture opens with exactly one notch, not one plus the 10 left over.
    expect(bank.distance).toBe(100);
  });

  it('a zero-distance event leaves the gesture unopened', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 0, 1000, KEYED_WHEEL);
    bank = foldWheelEvent(bank, 4, 1008, KEYED_WHEEL);
    expect(takeWheelStep(bank).step).toBe(1);
  });

  it('a continuous gesture keeps banking', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 90, 1000, KEYED_WHEEL);
    bank = foldWheelEvent(bank, 90, 1008, KEYED_WHEEL);
    expect(takeWheelStep(bank).step).toBe(1);
  });

  it('reversing direction answers at once instead of paying off the old bank', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 90, 1000, KEYED_WHEEL);
    bank = foldWheelEvent(bank, -100, 1008, KEYED_WHEEL);
    const taken = takeWheelStep(bank);
    expect(taken.step).toBe(-1);
    expect(taken.bank.distance).toBe(0);
  });
});

describe('takeWheelStep leaves the remainder banked', () => {
  it('spends exactly one notch per call', () => {
    const bank = foldWheelEvent(emptyWheelBank(), 250, 1000, KEYED_WHEEL);
    const first = takeWheelStep(bank);
    expect(first.step).toBe(1);
    expect(first.bank.distance).toBe(150);
    const second = takeWheelStep(first.bank);
    expect(second.step).toBe(1);
    expect(second.bank.distance).toBe(50);
    expect(takeWheelStep(second.bank).step).toBe(0);
  });
});

describe('createWheelZoom spends one step per frame', () => {
  // A queueing `requestAnimationFrame`. The shared stub in `test-setup.ts` runs
  // the callback at once, which would hide the cap entirely.
  let frames: FrameRequestCallback[] = [];
  let realRaf: typeof requestAnimationFrame;
  let realCancel: typeof cancelAnimationFrame;

  function flushFrame(): void {
    const due = frames;
    frames = [];
    for (const cb of due) cb(0);
  }

  beforeEach(() => {
    realRaf = globalThis.requestAnimationFrame;
    realCancel = globalThis.cancelAnimationFrame;
    frames = [];
    (globalThis as { requestAnimationFrame: unknown }).requestAnimationFrame =
      (cb: FrameRequestCallback) => { frames.push(cb); return frames.length; };
    (globalThis as { cancelAnimationFrame: unknown }).cancelAnimationFrame = () => {};
  });

  afterEach(() => {
    (globalThis as { requestAnimationFrame: unknown }).requestAnimationFrame = realRaf;
    (globalThis as { cancelAnimationFrame: unknown }).cancelAnimationFrame = realCancel;
  });

  it('a burst of notches inside one frame moves the scale once', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    // Ten full notches, all before the renderer gets a turn. Each one used to
    // apply straight away, and the layout it forced is what wedged the tab.
    for (let i = 0; i < 10; i++) zoom.push(-100, 0, 1000 + i, KEYED_WHEEL);
    expect(steps).toEqual([]);
    flushFrame();
    expect(steps).toEqual([1]);
  });

  it('the rest drains over the following frames', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    for (let i = 0; i < 3; i++) zoom.push(-100, 0, 1000 + i, KEYED_WHEEL);
    flushFrame();
    flushFrame();
    flushFrame();
    expect(steps).toEqual([1, 1, 1]);
  });

  it('settles instead of scheduling for ever', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    zoom.push(-100, 0, 1000, KEYED_WHEEL);
    for (let i = 0; i < 5; i++) flushFrame();
    expect(steps).toEqual([1]);
    expect(frames).toEqual([]);
  });

  it('a sub-notch pinch leaves the scale alone', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    for (let i = 0; i < 20; i++) zoom.push(-4, 0, 1000 + i * 8, PINCH);
    flushFrame();
    flushFrame();
    expect(steps).toEqual([]);
  });

  it('a slow tick of a keyed wheel still steps once', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    zoom.push(-4, 0, 1000, KEYED_WHEEL);
    flushFrame();
    flushFrame();
    expect(steps).toEqual([1]);
  });
});

/** The case above is exactly the one the panel can time out under, and only the
 *  listener wires the two together. A source scan, because a frame-accurate
 *  pinch against the real linger timer asserts one call through three clocks. */
describe('the wheel handler keeps the panel alive', () => {
  it('renews the linger on every event, not only on a step', () => {
    const source = readFileSync(
      new URL('../scaleWheelListener.ts', import.meta.url), 'utf8',
    );
    const handler = source.slice(
      source.indexOf('function handleWheel'),
      source.indexOf('document.addEventListener(\'wheel\''),
    );
    expect(handler).toContain('renewScaleModalLinger()');
    expect(handler.indexOf('renewScaleModalLinger()'))
      .toBeLessThan(handler.indexOf('zoom.push'));
  });
});
