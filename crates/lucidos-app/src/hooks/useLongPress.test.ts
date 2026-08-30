import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { makeLongPressHandlers, type LongPressHandlers } from './useLongPress';

const DELAY = 450;

function pointerDown(over: Partial<{ button: number; clientX: number; clientY: number; pointerId: number }> = {}, target: object = {}): PointerEvent {
  return { button: 0, clientX: 0, clientY: 0, pointerId: 1, currentTarget: target, ...over } as unknown as PointerEvent;
}
function pointerMove(clientX: number, clientY = 0): PointerEvent {
  return { clientX, clientY } as unknown as PointerEvent;
}
function clickEvent() {
  return { preventDefault: vi.fn(), stopPropagation: vi.fn() } as unknown as MouseEvent & {
    preventDefault: ReturnType<typeof vi.fn>;
    stopPropagation: ReturnType<typeof vi.fn>;
  };
}
function contextMenuEvent(target: object = {}) {
  return { preventDefault: vi.fn(), currentTarget: target } as unknown as MouseEvent & {
    preventDefault: ReturnType<typeof vi.fn>;
  };
}

describe('makeLongPressHandlers', () => {
  let onLongPress: ReturnType<typeof vi.fn<(target: HTMLElement) => void>>;
  let onClick: ReturnType<typeof vi.fn<() => void>>;
  let h: LongPressHandlers;

  beforeEach(() => {
    vi.useFakeTimers();
    onLongPress = vi.fn<(target: HTMLElement) => void>();
    onClick = vi.fn<() => void>();
    h = makeLongPressHandlers(onLongPress, onClick);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('a quick tap fires onClick, not onLongPress', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(100);
    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('holding past the delay fires onLongPress and swallows the paired click', () => {
    const target = { id: 'btn' };
    h.onPointerDown(pointerDown({}, target));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    expect(onLongPress).toHaveBeenCalledWith(target);

    h.onPointerUp({} as PointerEvent);
    const click = clickEvent();
    h.onClick(click);
    // The click the browser pairs with the hold is swallowed — step nav must not run.
    expect(onClick).not.toHaveBeenCalled();
    expect(click.preventDefault).toHaveBeenCalled();
    expect(click.stopPropagation).toHaveBeenCalled();
  });

  it('moving beyond the tolerance cancels the pending hold', () => {
    h.onPointerDown(pointerDown());
    h.onPointerMove(pointerMove(50));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();

    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('a small jitter within the tolerance keeps the hold alive', () => {
    h.onPointerDown(pointerDown());
    h.onPointerMove(pointerMove(5));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('right-click opens via contextmenu without starting a hold timer', () => {
    const target = { id: 'btn' };
    h.onContextMenu(contextMenuEvent(target));
    expect(onLongPress).toHaveBeenCalledWith(target);
  });

  it('a right-button pointerdown never arms a hold', () => {
    h.onPointerDown(pointerDown({ button: 2 }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('the click suppressor auto-disarms after the fuse so a later tap still navigates', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(DELAY);   // long-press fires, arm set
    h.onPointerUp({} as PointerEvent); // arms the disarm fuse
    vi.advanceTimersByTime(1000);    // fuse elapses

    h.onClick(clickEvent());          // unrelated later click is NOT swallowed
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('cancel() clears a pending hold so the timer never fires (unmount mid-press)', () => {
    h.onPointerDown(pointerDown());
    h.cancel();
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  /** Capture would retarget the paired click to this element, so a press
   *  dragged off the control and released would fire its action. */
  it('never captures the pointer', () => {
    const setPointerCapture = vi.fn();
    h.onPointerDown(pointerDown({ pointerId: 7 }, { setPointerCapture }));
    expect(setPointerCapture).not.toHaveBeenCalled();
  });

  /** Leaving the element cancels the hold, which capture would suppress. */
  it('a pointer that leaves the element cancels the hold', () => {
    h.onPointerDown(pointerDown());
    h.onPointerLeave({} as PointerEvent);
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a fresh press clears a stranded arm from a right-click that had no paired click', () => {
    h.onContextMenu(contextMenuEvent()); // arms (touch may pair a click; mouse won't)
    // New unrelated gesture before the fuse elapses:
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(100);
    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onClick).toHaveBeenCalledTimes(1);
  });
});
