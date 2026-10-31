// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { makeLongPressHandlers, type LongPressCallback, type LongPressHandlers } from './useLongPress';

const DELAY = 450;

function pointerDown(over: Partial<{ button: number; clientX: number; clientY: number; pointerId: number; pointerType: string }> = {}, target: object = document.createElement('div')): PointerEvent {
  return { button: 0, clientX: 0, clientY: 0, pointerId: 1, timeStamp: 0, currentTarget: target, ...over } as unknown as PointerEvent;
}
/** A document-level event, as the browser dispatches a lift to the document. */
function docEvent(type: string, fields: object): Event {
  return Object.assign(new Event(type), fields);
}
function lift(pointerId = 1): void {
  document.dispatchEvent(docEvent('pointerup', { pointerId }));
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
function contextMenuEvent(target: object = {}, clientX = 0, clientY = 0) {
  return { preventDefault: vi.fn(), currentTarget: target, clientX, clientY } as unknown as MouseEvent & {
    preventDefault: ReturnType<typeof vi.fn>;
  };
}

describe('makeLongPressHandlers', () => {
  let onLongPress: ReturnType<typeof vi.fn<LongPressCallback>>;
  let onClick: ReturnType<typeof vi.fn<() => void>>;
  let h: LongPressHandlers;

  beforeEach(() => {
    vi.useFakeTimers();
    onLongPress = vi.fn<LongPressCallback>();
    onClick = vi.fn<() => void>();
    h = makeLongPressHandlers(onLongPress, onClick);
  });
  afterEach(() => {
    vi.useRealTimers();
  });

  it('a hold fires at the delay with the finger still down, before any lift', () => {
    const target = document.createElement('button');
    h.onPointerDown(pointerDown({}, target));
    vi.advanceTimersByTime(DELAY - 1);
    expect(onLongPress).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onLongPress).toHaveBeenCalledOnce();
    expect(onLongPress).toHaveBeenCalledWith(target);
  });

  it('a hold fires once, and its lift does not fire it again', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(2 * DELAY);
    lift();
    h.onPointerUp({} as PointerEvent);
    expect(onLongPress).toHaveBeenCalledOnce();
  });

  it("a touch press turns off the target's native long-press at once, until the next press", () => {
    // iOS shows its own selection or link menu while the finger rests,
    // and a selection also cancels the gesture.
    const target = document.createElement('div');
    target.style.setProperty('user-select', 'text');
    h.onPointerDown(pointerDown({}, target));
    expect(target.style.getPropertyValue('user-select')).toBe('none');
    expect(target.style.getPropertyValue('-webkit-user-select')).toBe('none');
    vi.advanceTimersByTime(DELAY + 100);
    lift();
    expect(target.style.getPropertyValue('user-select')).toBe('none');
    document.dispatchEvent(docEvent('pointerdown', { pointerId: 2 }));
    expect(target.style.getPropertyValue('user-select')).toBe('text');
    expect(target.style.getPropertyValue('-webkit-user-select')).toBe('');
  });

  it('a mouse press leaves selection alone', () => {
    const target = document.createElement('div');
    h.onPointerDown(pointerDown({ pointerType: 'mouse' }, target));
    vi.advanceTimersByTime(DELAY);
    expect(target.style.getPropertyValue('user-select')).toBe('');
  });

  it('a hold that fires ticks, and a quick tap does not', () => {
    const vibrate = vi.fn();
    Object.defineProperty(navigator, 'vibrate', { value: vibrate, configurable: true });
    try {
      h.onPointerDown(pointerDown());
      vi.advanceTimersByTime(DELAY - 1);
      lift();
      vi.advanceTimersByTime(DELAY);
      expect(vibrate).not.toHaveBeenCalled();
      h.onPointerDown(pointerDown());
      vi.advanceTimersByTime(DELAY);
      expect(onLongPress).toHaveBeenCalledTimes(1);
      expect(vibrate).toHaveBeenCalledTimes(1);
    } finally {
      delete (navigator as { vibrate?: unknown }).vibrate;
    }
  });

  it('a quick tap fires onClick, not onLongPress', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(100);
    h.onPointerUp({} as PointerEvent);
    vi.advanceTimersByTime(DELAY);
    h.onClick(clickEvent());
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a tap whose button is swapped out mid-press still ends the hold', () => {
    // A button swapped out by its own tap never sees its pointerup.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    vi.advanceTimersByTime(100);
    lift(7);
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it.each(['touchend', 'touchcancel'])('a %s that leaves no finger on the glass ends the hold', (type) => {
    // A tap's touchend can disable its button, and WebKit then never dispatches the pointerup.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent(type, { touches: [] }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a touch lift with a finger still down does not end the hold', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('touchend', { touches: [{}] }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('another pointer lifting does not end the hold', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('pointercancel', { pointerId: 8 }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('a pointercancel before the delay is never a hold', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('pointercancel', { pointerId: 7 }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a new press elsewhere ends a press whose lift was lost', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    vi.advanceTimersByTime(DELAY - 100);
    document.dispatchEvent(docEvent('pointerdown', { pointerId: 8 }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a lift just short of the delay is a tap', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(DELAY - 1);
    lift();
    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onLongPress).not.toHaveBeenCalled();
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('holding past the delay fires onLongPress and swallows the paired click', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);

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

  it('right-click opens via contextmenu, at the pointer, without starting a hold timer', () => {
    const target = { id: 'btn' };
    h.onContextMenu(contextMenuEvent(target, 120, 340));
    expect(onLongPress).toHaveBeenCalledWith(target, { x: 120, y: 340 });
  });

  it('a right-button pointerdown never arms a hold', () => {
    h.onPointerDown(pointerDown({ button: 2 }));
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('the click suppressor auto-disarms after the fuse so a later tap still navigates', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(DELAY);     // long-press fires, arm set
    h.onPointerUp({} as PointerEvent); // arms the disarm fuse
    vi.advanceTimersByTime(1000);      // fuse elapses

    h.onClick(clickEvent());           // unrelated later click is NOT swallowed
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('cancel() drops a pending hold so its timer never fires it (unmount mid-press)', () => {
    h.onPointerDown(pointerDown());
    h.cancel();
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  /** Capture would retarget the paired click to this element, so a press
   *  dragged off the control and released would fire its action. */
  it('never captures the pointer', () => {
    const setPointerCapture = vi.fn();
    h.onPointerDown(pointerDown({ pointerId: 7 }, Object.assign(document.createElement('div'), { setPointerCapture })));
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
