// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { createElement, render } from 'preact';
import { act } from 'preact/test-utils';
import { LONG_PRESS_DELAY_MS } from './useLongPress';
import { makeRowDrag, rowUnder, useRowDrag, type RowDragCallbacks } from './useRowDrag';

/** A stand-in for `document` that records listeners and replays events. */
function fakeSurface() {
  const listeners = new Map<string, Set<(e: unknown) => void>>();
  return {
    addEventListener: (type: string, fn: (e: unknown) => void) => {
      listeners.set(type, (listeners.get(type) ?? new Set()).add(fn));
    },
    removeEventListener: (type: string, fn: (e: unknown) => void) => listeners.get(type)?.delete(fn),
    fire: (type: string, event: object) => listeners.get(type)?.forEach((fn) => fn(event)),
    count: () => [...listeners.values()].reduce((n, set) => n + set.size, 0),
  } as unknown as Pick<Document, 'addEventListener' | 'removeEventListener'> & {
    fire(type: string, event: object): void;
    count(): number;
  };
}

const at = (clientY: number) => ({ button: 0, clientX: 0, clientY, currentTarget: document.createElement('div') }) as unknown as PointerEvent;
const rowAt = (y: number) => Math.min(3, Math.max(0, Math.floor(y / 50)));

describe('makeRowDrag', () => {
  let surface: ReturnType<typeof fakeSurface>;
  let callbacks: { [K in keyof RowDragCallbacks]: Mock<RowDragCallbacks[K]> };

  beforeEach(() => {
    vi.useFakeTimers();
    surface = fakeSurface();
    callbacks = {
      onLift: vi.fn<RowDragCallbacks['onLift']>(),
      onMove: vi.fn<RowDragCallbacks['onMove']>(),
      onDrop: vi.fn<RowDragCallbacks['onDrop']>(),
      onCancel: vi.fn<RowDragCallbacks['onCancel']>(),
    };
  });
  afterEach(() => vi.useRealTimers());

  it('a swipe that travels before the hold fires never lifts, so the page scrolls', () => {
    const drag = makeRowDrag(surface, rowAt, callbacks);
    drag.onPointerDown(at(60));
    drag.onPointerMove(at(110));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS * 2);
    drag.onPointerUp(at(110));
    expect(callbacks.onLift).not.toHaveBeenCalled();
    expect(callbacks.onDrop).not.toHaveBeenCalled();
    expect(surface.count()).toBe(0);
  });

  it('a hold lifts the row under the press, and the release drops it once', () => {
    const drag = makeRowDrag(surface, rowAt, callbacks);
    drag.onPointerDown(at(60));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(callbacks.onLift).toHaveBeenCalledWith(1);
    surface.fire('pointermove', at(110));
    surface.fire('pointermove', at(160));
    expect(callbacks.onMove).toHaveBeenLastCalledWith({ from: 1, to: 3, dy: 100 });
    surface.fire('pointerup', at(160));
    expect(callbacks.onDrop).toHaveBeenCalledTimes(1);
    expect(callbacks.onDrop).toHaveBeenCalledWith({ from: 1, to: 3, dy: 100 });
    expect(surface.count()).toBe(0);
  });

  it('a press on the grip lifts at once, with no hold', () => {
    const drag = makeRowDrag(surface, rowAt, callbacks);
    drag.liftNow(at(60));
    expect(callbacks.onLift).toHaveBeenCalledWith(1);
    surface.fire('pointermove', at(10));
    surface.fire('pointerup', at(10));
    expect(callbacks.onDrop).toHaveBeenCalledWith({ from: 1, to: 0, dy: -50 });
    expect(surface.count()).toBe(0);
  });

  it('keeps the row under a stationary pointer while scrolling and updates the drop target', () => {
    let scroll = 0;
    const drag = makeRowDrag(surface, rowAt, callbacks, () => scroll);
    drag.liftNow(at(60));
    scroll = 100;
    surface.fire('scroll', {});
    expect(callbacks.onMove).toHaveBeenLastCalledWith({ from: 1, to: 3, dy: 100 });
    scroll = -50;
    surface.fire('scroll', {});
    expect(callbacks.onMove).toHaveBeenLastCalledWith({ from: 1, to: 0, dy: -50 });
    surface.fire('pointermove', at(90));
    expect(callbacks.onMove).toHaveBeenLastCalledWith({ from: 1, to: 0, dy: -20 });
    surface.fire('pointerup', at(90));
    expect(callbacks.onDrop).toHaveBeenCalledWith({ from: 1, to: 0, dy: -20 });
    expect(surface.count()).toBe(0);
    surface.fire('scroll', {});
    expect(callbacks.onMove).toHaveBeenCalledTimes(4);
  });

  it('the grip ignores a button other than the primary one', () => {
    const drag = makeRowDrag(surface, rowAt, callbacks);
    drag.liftNow({ ...at(60), button: 2 } as PointerEvent);
    expect(callbacks.onLift).not.toHaveBeenCalled();
    expect(surface.count()).toBe(0);
  });

  it('swallows the click paired with the release, even when the row never saw it', () => {
    const drag = makeRowDrag(surface, rowAt, callbacks);
    drag.onPointerDown(at(60));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    surface.fire('pointerup', at(60));
    const click = { preventDefault: vi.fn(), stopPropagation: vi.fn() } as unknown as MouseEvent;
    drag.onClick(click);
    expect(click.preventDefault).toHaveBeenCalled();
  });

  it('a cancelled touch drops nothing and clears the listeners', () => {
    const drag = makeRowDrag(surface, rowAt, callbacks);
    drag.onPointerDown(at(60));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    expect(drag.dragging()).toBe(true);
    surface.fire('pointercancel', {});
    expect(callbacks.onCancel).toHaveBeenCalledTimes(1);
    expect(callbacks.onDrop).not.toHaveBeenCalled();
    expect(drag.dragging()).toBe(false);
    expect(surface.count()).toBe(0);
  });
});

describe('rowUnder', () => {
  const rows = [{ index: 0, bottom: 40 }, { index: 2, bottom: 80 }, { index: 3, bottom: 120 }];

  it('names the row whose band holds the pointer, clamped at both ends', () => {
    expect(rowUnder(rows, -10)).toBe(0);
    expect(rowUnder(rows, 39)).toBe(0);
    expect(rowUnder(rows, 40)).toBe(2);
    expect(rowUnder(rows, 119)).toBe(3);
    expect(rowUnder(rows, 500)).toBe(3);
  });
});

describe('useRowDrag scroll geometry', () => {
  it('captures ancestor and list scrolling, keeps the held row anchored, and drops on the scrolled target', () => {
    const host = document.createElement('div');
    document.body.append(host);
    const drop = vi.fn();
    let latest: ReturnType<typeof useRowDrag>;
    function Harness() {
      latest = useRowDrag(drop, () => true);
      return createElement('div', { ref: latest.listRef }, [0, 1, 2, 3].map(index =>
        createElement('div', { 'data-row-index': index }, String(index))));
    }
    try {
      act(() => render(createElement(Harness, {}), host));
      const list = host.firstElementChild as HTMLElement;
      let top = 0;
      vi.spyOn(list, 'getBoundingClientRect').mockImplementation(() => ({ top }) as DOMRect);
      list.querySelectorAll<HTMLElement>('[data-row-index]').forEach((row, index) => {
        vi.spyOn(row, 'getBoundingClientRect').mockImplementation(() =>
          ({ top: top - list.scrollTop + index * 50, bottom: top - list.scrollTop + (index + 1) * 50 }) as DOMRect);
      });
      act(() => latest.gripProps.onPointerDown(new MouseEvent('pointerdown', { clientY: 60 }) as PointerEvent));
      top = -50;
      act(() => { host.dispatchEvent(new Event('scroll')); });
      expect(latest!.drag).toEqual({ from: 1, to: 2, dy: 50 });
      list.scrollTop = 50;
      act(() => { list.dispatchEvent(new Event('scroll')); });
      expect(latest!.drag).toEqual({ from: 1, to: 3, dy: 100 });
      list.scrollTop = 200;
      act(() => { list.dispatchEvent(new Event('scroll')); });
      expect(latest!.drag).toEqual({ from: 1, to: 3, dy: 100 });
      top = 200;
      list.scrollTop = 0;
      act(() => { host.dispatchEvent(new Event('scroll')); });
      expect(latest!.drag).toEqual({ from: 1, to: 0, dy: -50 });
      top = -50;
      list.scrollTop = 50;
      act(() => { document.dispatchEvent(new MouseEvent('pointerup', { clientY: 60 })); });
      expect(drop).toHaveBeenCalledWith(1, 3);
      expect(latest!.drag).toBeNull();
    } finally {
      act(() => render(null, host));
      host.remove();
    }
  });
});
