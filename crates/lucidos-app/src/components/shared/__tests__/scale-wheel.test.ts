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

/** Feed a stream of events and count the steps they buy, draining greedily.
 *  The per-frame cap lives in the component; here every banked step is taken,
 *  which is what says how many the gesture ASKED for. */
function stepsFor(deltas: number[], deltaMode = 0, gapMs = 8): number {
  let bank: WheelBank = emptyWheelBank();
  let at = 1000;
  let steps = 0;
  for (const d of deltas) {
    at += gapMs;
    bank = foldWheelEvent(bank, wheelZoomDistance(d, deltaMode), at);
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
    expect(stepsFor([-3], 1)).toBe(1);
  });

  it('three notches are three steps', () => {
    expect(stepsFor([-100, -100, -100])).toBe(3);
  });
});

describe('a pinch spends distance, not events', () => {
  it('twenty small-delta events are not twenty steps', () => {
    // A macOS trackpad pinch, roughly. Twenty events carrying 4px each are 80px
    // of travel, which is less than one notch.
    expect(stepsFor(Array(20).fill(-4))).toBe(0);
  });

  it('the steps track the distance travelled', () => {
    // 50 events of 8px is 400px, which is four notches.
    expect(stepsFor(Array(50).fill(-8))).toBe(4);
  });
});

describe('the bank belongs to one gesture', () => {
  it('an idle gap drops what the previous gesture banked', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 90, 1000);
    bank = foldWheelEvent(bank, 90, 1000 + WHEEL_GESTURE_GAP_MS + 1);
    // Without the reset the two 90s would total 180 and buy a step.
    expect(takeWheelStep(bank).step).toBe(0);
    expect(bank.distance).toBe(90);
  });

  it('a continuous gesture keeps banking', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 90, 1000);
    bank = foldWheelEvent(bank, 90, 1008);
    expect(takeWheelStep(bank).step).toBe(1);
  });

  it('reversing direction answers at once instead of paying off the old bank', () => {
    let bank = foldWheelEvent(emptyWheelBank(), 90, 1000);
    bank = foldWheelEvent(bank, -100, 1008);
    const taken = takeWheelStep(bank);
    expect(taken.step).toBe(-1);
    expect(taken.bank.distance).toBe(0);
  });
});

describe('takeWheelStep leaves the remainder banked', () => {
  it('spends exactly one notch per call', () => {
    const bank = foldWheelEvent(emptyWheelBank(), 250, 1000);
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
    for (let i = 0; i < 10; i++) zoom.push(-100, 0, 1000 + i);
    expect(steps).toEqual([]);
    flushFrame();
    expect(steps).toEqual([1]);
  });

  it('the rest drains over the following frames', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    for (let i = 0; i < 3; i++) zoom.push(-100, 0, 1000 + i);
    flushFrame();
    flushFrame();
    flushFrame();
    expect(steps).toEqual([1, 1, 1]);
  });

  it('settles instead of scheduling for ever', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    zoom.push(-100, 0, 1000);
    for (let i = 0; i < 5; i++) flushFrame();
    expect(steps).toEqual([1]);
    expect(frames).toEqual([]);
  });

  it('a sub-notch pinch asks for no step at all', () => {
    const steps: number[] = [];
    const zoom = createWheelZoom(d => steps.push(d));
    for (let i = 0; i < 20; i++) zoom.push(-4, 0, 1000 + i * 8);
    flushFrame();
    expect(steps).toEqual([]);
  });
});

/** The case above is exactly the one the panel can time out under, and only the
 *  component wires the two together. A source scan, because mounting the panel
 *  drags in the whole overlay stack to assert one call. */
describe('the wheel handler keeps the panel alive', () => {
  it('renews the linger on every event, not only on a step', () => {
    const source = readFileSync(
      new URL('../ScaleModal.tsx', import.meta.url), 'utf8',
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
