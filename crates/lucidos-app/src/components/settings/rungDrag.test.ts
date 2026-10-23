import { afterEach, beforeEach, describe, expect, it, vi, type Mock } from 'vitest';
import { LONG_PRESS_DELAY_MS } from '../../hooks/useLongPress';
import { makeRungDrag, type RungDragCallbacks } from './rungDrag';

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

const at = (clientY: number) => ({ button: 0, clientX: 0, clientY, currentTarget: {} }) as unknown as PointerEvent;
const rowAt = (y: number) => Math.min(3, Math.max(0, Math.floor(y / 50)));

describe('makeRungDrag', () => {
  let surface: ReturnType<typeof fakeSurface>;
  let callbacks: { [K in keyof RungDragCallbacks]: Mock<RungDragCallbacks[K]> };

  beforeEach(() => {
    vi.useFakeTimers();
    surface = fakeSurface();
    callbacks = {
      onLift: vi.fn<RungDragCallbacks['onLift']>(),
      onMove: vi.fn<RungDragCallbacks['onMove']>(),
      onDrop: vi.fn<RungDragCallbacks['onDrop']>(),
      onCancel: vi.fn<RungDragCallbacks['onCancel']>(),
    };
  });
  afterEach(() => vi.useRealTimers());

  it('a swipe that travels before the hold fires never lifts, so the page scrolls', () => {
    const drag = makeRungDrag(surface, rowAt, callbacks);
    drag.onPointerDown(at(60));
    drag.onPointerMove(at(110));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS * 2);
    drag.onPointerUp(at(110));
    expect(callbacks.onLift).not.toHaveBeenCalled();
    expect(callbacks.onDrop).not.toHaveBeenCalled();
    expect(surface.count()).toBe(0);
  });

  it('a hold lifts the rung under the press, and the release drops it once', () => {
    const drag = makeRungDrag(surface, rowAt, callbacks);
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
    const drag = makeRungDrag(surface, rowAt, callbacks);
    drag.liftNow(at(60));
    expect(callbacks.onLift).toHaveBeenCalledWith(1);
    surface.fire('pointermove', at(10));
    surface.fire('pointerup', at(10));
    expect(callbacks.onDrop).toHaveBeenCalledWith({ from: 1, to: 0, dy: -50 });
    expect(surface.count()).toBe(0);
  });

  it('the grip ignores a button other than the primary one', () => {
    const drag = makeRungDrag(surface, rowAt, callbacks);
    drag.liftNow({ ...at(60), button: 2 } as PointerEvent);
    expect(callbacks.onLift).not.toHaveBeenCalled();
    expect(surface.count()).toBe(0);
  });

  it('swallows the click paired with the release, even when the row never saw it', () => {
    const drag = makeRungDrag(surface, rowAt, callbacks);
    drag.onPointerDown(at(60));
    vi.advanceTimersByTime(LONG_PRESS_DELAY_MS);
    surface.fire('pointerup', at(60));
    const click = { preventDefault: vi.fn(), stopPropagation: vi.fn() } as unknown as MouseEvent;
    drag.onClick(click);
    expect(click.preventDefault).toHaveBeenCalled();
  });

  it('a cancelled touch drops nothing and clears the listeners', () => {
    const drag = makeRungDrag(surface, rowAt, callbacks);
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
