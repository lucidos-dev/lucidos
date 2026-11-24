import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';

import {
  TOUCH_COAST_IDLE_MS,
  TOUCH_COAST_MAX_MS,
  attachReaderInput,
  isScrollHeld,
  onScrollHoldReleased,
  readerGestureActive,
  readerGestureForTest,
  readerGestureSince,
  retireReaderMovement,
} from '../readerInput';
import { USER_SCROLL_WINDOW_MS, nowMs } from '../../../utils/scrollActivity';

describe('the reader-input state', () => {
  let windowListeners: Record<string, Function[]>;
  let realWindow: unknown;

  function makeContainer() {
    const own: Record<string, Function[]> = {};
    const el: any = {
      clientWidth: 800,
      addEventListener: (type: string, fn: Function) => { (own[type] ??= []).push(fn); },
      removeEventListener: (type: string, fn: Function) => {
        own[type] = (own[type] ?? []).filter((f) => f !== fn);
      },
      fire(type: string, event: Record<string, unknown> = {}) {
        for (const fn of own[type] ?? []) fn({ target: el, button: 0, buttons: 1, offsetX: 10, ...event });
      },
      listenerCount: () => Object.values(own).reduce((n, fns) => n + fns.length, 0),
    };
    return el;
  }

  function fireWindow(type: string, event: Record<string, unknown> = {}) {
    for (const fn of windowListeners[type] ?? []) fn(event);
  }

  beforeEach(() => {
    vi.useFakeTimers();
    windowListeners = {};
    realWindow = (globalThis as any).window;
    (globalThis as any).window = {
      addEventListener: (type: string, fn: Function) => { (windowListeners[type] ??= []).push(fn); },
      removeEventListener: (type: string, fn: Function) => {
        windowListeners[type] = (windowListeners[type] ?? []).filter((f) => f !== fn);
      },
    };
  });

  afterEach(() => {
    readerGestureForTest(null, false);
    vi.clearAllTimers();
    vi.useRealTimers();
    (globalThis as any).window = realWindow;
  });

  it('is idle for a container nothing has touched', () => {
    const el = makeContainer();
    expect(isScrollHeld(el)).toBe(false);
    expect(readerGestureActive(el)).toBe(false);
  });

  it('ends a wheel or a key by its tail, and neither holds', () => {
    const el = makeContainer();
    attachReaderInput(el);
    el.fire('wheel');
    el.fire('keydown', { key: 'PageDown' });
    expect(isScrollHeld(el)).toBe(false);
    vi.advanceTimersByTime(USER_SCROLL_WINDOW_MS - 1);
    expect(readerGestureActive(el)).toBe(true);
    vi.advanceTimersByTime(1);
    expect(readerGestureActive(el)).toBe(false);
  });

  it('holds a touch while the finger is down, then for its coast', () => {
    const el = makeContainer();
    attachReaderInput(el);
    el.fire('touchmove');
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
    fireWindow('touchend', { touches: [] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS - 1);
    expect(isScrollHeld(el)).toBe(true);
    vi.advanceTimersByTime(1);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('keeps a touch the reader has not ended when a wheel or a key arrives', () => {
    const el = makeContainer();
    attachReaderInput(el);
    el.fire('touchmove');
    el.fire('wheel');
    el.fire('keydown', { key: 'End' });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
  });

  it('holds on a primary press of the own box, and only moves on a gutter press', () => {
    const el = makeContainer();
    attachReaderInput(el);
    const before = nowMs();
    vi.advanceTimersByTime(10);
    el.fire('pointerdown');
    expect(isScrollHeld(el)).toBe(true);
    expect(readerGestureSince(el, before)).toBe(true);
    fireWindow('pointerup');
    expect(isScrollHeld(el)).toBe(false);
    expect(readerGestureSince(el, before)).toBe(false);

    el.fire('pointerdown', { button: 1, offsetX: el.clientWidth + 6 });
    expect(isScrollHeld(el)).toBe(false);
    expect(readerGestureSince(el, before)).toBe(true);
  });

  it('keeps a scrollbar hold through a second press, until the release', () => {
    const el = makeContainer();
    const released: unknown[] = [];
    const unsubscribe = onScrollHoldReleased((held) => released.push(held));
    attachReaderInput(el);
    el.fire('pointerdown');
    el.fire('pointerdown', { button: 1, offsetX: el.clientWidth + 6 });
    expect(isScrollHeld(el)).toBe(true);
    expect(released).toEqual([]);
    fireWindow('pointerup');
    unsubscribe();
    expect(isScrollHeld(el)).toBe(false);
    expect(released).toEqual([el]);
  });

  it('turns a coast into a scrollbar hold on a primary press, and waits for its release', () => {
    const el = makeContainer();
    const released: unknown[] = [];
    const unsubscribe = onScrollHoldReleased((held) => released.push(held));
    attachReaderInput(el);
    el.fire('touchmove');
    fireWindow('touchend', { touches: [] });
    el.fire('pointerdown');
    vi.advanceTimersByTime(TOUCH_COAST_MAX_MS);
    expect(isScrollHeld(el)).toBe(true);
    expect(released).toEqual([]);
    fireWindow('pointerup');
    unsubscribe();
    expect(released).toEqual([el]);
  });

  it('keeps a coast through a press that holds nothing', () => {
    const el = makeContainer();
    attachReaderInput(el);
    el.fire('touchmove');
    fireWindow('touchend', { touches: [] });
    el.fire('pointerdown', { button: 1, offsetX: el.clientWidth + 6 });
    fireWindow('pointerup');
    expect(isScrollHeld(el)).toBe(true);
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('ends a coast on time when a touch lifts in another pane', () => {
    const a = makeContainer();
    const b = makeContainer();
    attachReaderInput(a);
    attachReaderInput(b);
    a.fire('touchmove');
    fireWindow('touchend', { touches: [] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS - 1);
    fireWindow('touchend', { touches: [] });
    vi.advanceTimersByTime(1);
    expect(isScrollHeld(a)).toBe(false);
    expect(isScrollHeld(b)).toBe(false);
  });

  it('starts the coast when its own finger lifts, whatever rests in another pane', () => {
    const el = makeContainer();
    attachReaderInput(el);
    const mine = { identifier: 4 };
    const resting = { identifier: 9 };
    el.fire('touchmove', { changedTouches: [mine], touches: [mine, resting] });
    fireWindow('touchend', { changedTouches: [mine], touches: [resting] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('keeps the touch while one of its own two fingers is still down', () => {
    const el = makeContainer();
    attachReaderInput(el);
    const first = { identifier: 4 };
    const second = { identifier: 5 };
    el.fire('touchmove', { changedTouches: [first], touches: [first, second] });
    el.fire('touchmove', { changedTouches: [second], touches: [first, second] });
    fireWindow('touchend', { changedTouches: [first], touches: [second] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
    fireWindow('touchend', { changedTouches: [second], touches: [] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('forgets a lifted finger, so its identifier reused elsewhere is not its own', () => {
    const el = makeContainer();
    attachReaderInput(el);
    const first = { identifier: 4 };
    const second = { identifier: 5 };
    el.fire('touchmove', { changedTouches: [first, second], touches: [first, second] });
    fireWindow('touchend', { changedTouches: [first], touches: [second] });
    const reused = { identifier: 4 };
    fireWindow('touchend', { changedTouches: [second], touches: [reused] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('keeps the touch for a finger resting on the same container', () => {
    const el = makeContainer();
    const row = {};
    el.contains = (node: unknown) => node === row;
    attachReaderInput(el);
    const mine = { identifier: 4, target: row };
    const resting = { identifier: 5, target: row };
    el.fire('touchmove', { changedTouches: [mine], touches: [mine, resting] });
    fireWindow('touchend', { changedTouches: [mine], touches: [resting] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
  });

  it('keeps the touch for a finger placed on the container after the last move', () => {
    const el = makeContainer();
    const row = {};
    el.contains = (node: unknown) => node === row;
    attachReaderInput(el);
    const mine = { identifier: 4, target: row };
    const late = { identifier: 5, target: row };
    el.fire('touchmove', { changedTouches: [mine], touches: [mine] });
    fireWindow('touchend', { changedTouches: [mine], touches: [late] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
    fireWindow('touchend', { changedTouches: [late], touches: [] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('keeps the touch for a late finger whose row unmounted before the lift', () => {
    const el = makeContainer();
    const row = {};
    const lateRow = {};
    let mounted = true;
    el.contains = (node: unknown) => node === row || (mounted && node === lateRow);
    attachReaderInput(el);
    const mine = { identifier: 4, target: row };
    const late = { identifier: 5, target: lateRow };
    el.fire('touchmove', { changedTouches: [mine], touches: [mine] });
    el.fire('touchstart', { changedTouches: [late], touches: [mine, late] });
    mounted = false;
    fireWindow('touchend', { changedTouches: [mine], touches: [late] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
  });

  it('holds nothing for a finger that only lands', () => {
    const el = makeContainer();
    attachReaderInput(el);
    const tap = { identifier: 4, target: el };
    el.fire('touchstart', { changedTouches: [tap], touches: [tap] });
    expect(isScrollHeld(el)).toBe(false);
  });

  it('does not take a reused identifier on another target for a finger whose lift it missed', () => {
    const el = makeContainer();
    const row = {};
    el.contains = (node: unknown) => node === row;
    attachReaderInput(el);
    const mine = { identifier: 4, target: row };
    const resting = { identifier: 5, target: row };
    el.fire('touchmove', { changedTouches: [mine], touches: [mine, resting] });
    const elsewhere = { identifier: 5, target: {} };
    fireWindow('touchend', { changedTouches: [mine], touches: [elsewhere] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('keeps the touch when its lift never arrives, as under an unmounted row', () => {
    const el = makeContainer();
    attachReaderInput(el);
    const mine = { identifier: 4 };
    const other = { identifier: 9 };
    el.fire('touchmove', { changedTouches: [mine], touches: [mine] });
    fireWindow('touchend', { changedTouches: [other], touches: [mine] });
    vi.advanceTimersByTime(TOUCH_COAST_IDLE_MS);
    expect(isScrollHeld(el)).toBe(true);
    vi.advanceTimersByTime(TOUCH_COAST_MAX_MS);
    expect(isScrollHeld(el)).toBe(false);
  });

  it('keeps one state per container, so one pane never speaks for another', () => {
    const a = makeContainer();
    const b = makeContainer();
    attachReaderInput(a);
    attachReaderInput(b);
    a.fire('touchmove');
    b.fire('wheel');
    expect(isScrollHeld(a)).toBe(true);
    expect(isScrollHeld(b)).toBe(false);
    expect(readerGestureActive(b)).toBe(true);
  });

  it('closes every movement tail on a retire, and leaves a hold alone', () => {
    const el = makeContainer();
    attachReaderInput(el);
    el.fire('touchmove');
    retireReaderMovement();
    expect(readerGestureActive(el)).toBe(false);
    expect(isScrollHeld(el)).toBe(true);
  });

  it('releases a hold on teardown, announces it once, and leaves no listener', () => {
    const el = makeContainer();
    const released: unknown[] = [];
    const unsubscribe = onScrollHoldReleased((held) => released.push(held));
    const detach = attachReaderInput(el);
    el.fire('touchmove');
    detach();
    unsubscribe();
    expect(released).toEqual([el]);
    expect(isScrollHeld(el)).toBe(false);
    expect(el.listenerCount()).toBe(0);
    expect(Object.values(windowListeners).flat()).toEqual([]);
  });
});
