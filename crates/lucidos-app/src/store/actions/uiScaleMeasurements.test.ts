import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

/** Counts the measurement passes `applyUiScale` owes.
 *
 *  `publishScrollbarGutter` and `clampThreadDrawerWidth` run together in one
 *  pass, so counting the first counts both. Mocked rather than spied, because
 *  `preferences.ts` binds the import directly. */
const passes = vi.hoisted(() => ({ count: 0 }));
vi.mock('../../utils/scrollbarGutter', () => ({
  publishScrollbarGutter: () => { passes.count += 1; return 0; },
}));

import { applyUiScale } from './preferences';

/** A `requestAnimationFrame` that QUEUES, unlike the synchronous one in
 *  `test-setup.ts`. Coalescing is invisible against a stub that runs the
 *  callback before the next caller arrives. */
let frame: FrameRequestCallback[] = [];
let realRaf: typeof requestAnimationFrame;
/** What the root element's inline custom properties hold. The shared stub in
 *  `test-setup.ts` swallows every write, so a reader needs its own. */
let inlineProps: Record<string, string>;
let realStyle: CSSStyleDeclaration;

function flushFrame(): void {
  const due = frame;
  frame = [];
  for (const cb of due) cb(0);
}

describe('applyUiScale measures once per frame', () => {
  beforeEach(() => {
    realRaf = globalThis.requestAnimationFrame;
    frame = [];
    (globalThis as { requestAnimationFrame: unknown }).requestAnimationFrame =
      (cb: FrameRequestCallback) => { frame.push(cb); return frame.length; };
    inlineProps = {};
    realStyle = document.documentElement.style;
    Object.defineProperty(document.documentElement, 'style', {
      configurable: true,
      value: {
        setProperty: (k: string, v: string) => { inlineProps[k] = v; },
        getPropertyValue: (k: string) => inlineProps[k] ?? '',
        removeProperty: (k: string) => { delete inlineProps[k]; },
      },
    });
    passes.count = 0;
    localStorage.clear();
  });

  afterEach(() => {
    // Leave no pass owed: the flag lives in the module, so an unflushed frame
    // would silently swallow the next case's first schedule.
    flushFrame();
    (globalThis as { requestAnimationFrame: unknown }).requestAnimationFrame = realRaf;
    Object.defineProperty(document.documentElement, 'style', {
      configurable: true,
      value: realStyle,
    });
  });

  it('a burst of calls costs one pass, not one per call', () => {
    // A held zoom key or a trackpad pinch, arriving inside one frame. Each call
    // used to force its own full layout, which is what wedged the tab.
    for (let i = 0; i < 12; i++) applyUiScale(100 + i);
    expect(passes.count).toBe(0);
    flushFrame();
    expect(passes.count).toBe(1);
  });

  it('the next frame is measured again', () => {
    applyUiScale(125);
    flushFrame();
    applyUiScale(150);
    flushFrame();
    expect(passes.count).toBe(2);
  });

  it('writes the scale itself synchronously, so the paint is never a frame late', () => {
    applyUiScale(125);
    expect(localStorage.getItem('lucidos-ui-scale')).toBe('125');
    expect(inlineProps['--user-ui-scale']).toBe('125%');
  });

  it('measures with no requestAnimationFrame at all', () => {
    (globalThis as { requestAnimationFrame: unknown }).requestAnimationFrame = undefined;
    applyUiScale(175);
    expect(passes.count).toBe(1);
  });
});
