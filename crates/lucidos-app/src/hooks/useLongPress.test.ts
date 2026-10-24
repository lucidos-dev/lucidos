// @vitest-environment jsdom
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { makeLongPressHandlers, type LongPressCallback, type LongPressHandlers } from './useLongPress';

const DELAY = 450;

function pointerDown(over: Partial<{ button: number; clientX: number; clientY: number; pointerId: number }> = {}, target: object = document.createElement('div')): PointerEvent {
  return { button: 0, clientX: 0, clientY: 0, pointerId: 1, timeStamp: 0, currentTarget: target, ...over } as unknown as PointerEvent;
}
/** A document-level event at `timeStamp` ms after the press went down at 0. */
function docEvent(type: string, timeStamp: number, fields: object): Event {
  const e = Object.assign(new Event(type), fields);
  return Object.defineProperty(e, 'timeStamp', { value: timeStamp });
}
/** Lift pointer `pointerId` at `timeStamp`, as the browser dispatches it to the document. */
function liftAt(timeStamp: number, pointerId = 1): void {
  document.dispatchEvent(docEvent('pointerup', timeStamp, { pointerId }));
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

  it('a press whose lift never arrives fires nothing', () => {
    // iOS can take a gesture and hand the page no pointerup, touchend or
    // touchcancel. A silent glass is not a held finger.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    vi.advanceTimersByTime(10 * DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('the next tap elsewhere never rules on a press whose lift was lost', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('pointerdown', 5000, { pointerId: 7 }));
    document.dispatchEvent(docEvent('touchend', 5080, { touches: [] }));
    liftAt(5080, 7);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a finger resting past the delay stops the target selecting text until the next press', () => {
    // iOS starts selecting under a resting finger and cancels the gesture,
    // so the hold would never lift into a menu.
    const target = document.createElement('div');
    target.style.setProperty('user-select', 'text');
    h.onPointerDown(pointerDown({}, target));
    vi.advanceTimersByTime(DELAY - 1);
    expect(target.style.getPropertyValue('user-select')).toBe('text');
    vi.advanceTimersByTime(1);
    expect(target.style.getPropertyValue('user-select')).toBe('none');
    liftAt(DELAY + 100);
    expect(target.style.getPropertyValue('user-select')).toBe('none');
    document.dispatchEvent(docEvent('pointerdown', 2000, { pointerId: 2 }));
    expect(target.style.getPropertyValue('user-select')).toBe('text');
  });

  it('a quick tap fires onClick, not onLongPress', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(100);
    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onClick).toHaveBeenCalledTimes(1);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a tap whose button is swapped out mid-press still ends the hold', () => {
    // A button swapped out by its own tap never sees its pointerup.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    vi.advanceTimersByTime(100);
    liftAt(100, 7);
    liftAt(DELAY, 7);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it.each(['touchend', 'touchcancel'])('a %s that leaves no finger on the glass ends the hold', (type) => {
    // A tap's touchend can disable its button, and WebKit then never dispatches the pointerup.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent(type, 100, { touches: [] }));
    liftAt(DELAY, 7);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a touch lift with a finger still down does not end the hold', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('touchend', 100, { touches: [{}] }));
    liftAt(DELAY, 7);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('another pointer lifting does not end the hold', () => {
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('pointercancel', 100, { pointerId: 8 }));
    liftAt(DELAY, 7);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('a press held still past the delay fires nothing until it lifts', () => {
    h.onPointerDown(pointerDown());
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
    liftAt(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('a lift just short of the delay is a tap', () => {
    h.onPointerDown(pointerDown());
    liftAt(DELAY - 1);
    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onLongPress).not.toHaveBeenCalled();
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('the last finger leaving the glass past the delay fires the hold', () => {
    // WebKit may dispatch the touchend before the pointerup, or no pointerup.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent('touchend', DELAY, { touches: [] }));
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it.each(['pointercancel', 'touchcancel'])('a %s past the delay is never a hold', (type) => {
    // A cancel is the system taking the gesture.
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    document.dispatchEvent(docEvent(type, 10 * DELAY, { pointerId: 7, touches: [] }));
    liftAt(10 * DELAY, 7);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('the lift is ruled at document capture, before the button hears it', () => {
    // A hold's release must not also run the button's own lift action.
    const button = document.createElement('button');
    document.body.append(button);
    const order: string[] = [];
    onLongPress.mockImplementation(() => { order.push('hold'); });
    button.addEventListener('touchend', () => order.push('button'));
    h.onPointerDown(pointerDown({ pointerId: 7 }));
    button.dispatchEvent(Object.defineProperty(
      Object.assign(new Event('touchend', { bubbles: true }), { touches: [] }), 'timeStamp', { value: DELAY },
    ));
    expect(order).toEqual(['hold', 'button']);
    button.remove();
  });

  it('a caller forwarding a late lift is ruled from that lift\'s time', () => {
    h.onPointerDown(pointerDown());
    h.onPointerUp({ timeStamp: DELAY } as PointerEvent);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('holding past the delay fires onLongPress and swallows the paired click', () => {
    const target = document.createElement('button');
    h.onPointerDown(pointerDown({}, target));
    liftAt(DELAY);
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
    liftAt(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();

    h.onPointerUp({} as PointerEvent);
    h.onClick(clickEvent());
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('a small jitter within the tolerance keeps the hold alive', () => {
    h.onPointerDown(pointerDown());
    h.onPointerMove(pointerMove(5));
    liftAt(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
  });

  it('right-click opens via contextmenu, at the pointer, without starting a hold timer', () => {
    const target = { id: 'btn' };
    h.onContextMenu(contextMenuEvent(target, 120, 340));
    expect(onLongPress).toHaveBeenCalledWith(target, { x: 120, y: 340 });
  });

  it('a right-button pointerdown never arms a hold', () => {
    h.onPointerDown(pointerDown({ button: 2 }));
    liftAt(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('the click suppressor auto-disarms after the fuse so a later tap still navigates', () => {
    h.onPointerDown(pointerDown());
    liftAt(DELAY);                   // long-press fires, arm set
    h.onPointerUp({} as PointerEvent); // arms the disarm fuse
    vi.advanceTimersByTime(1000);    // fuse elapses

    h.onClick(clickEvent());          // unrelated later click is NOT swallowed
    expect(onClick).toHaveBeenCalledTimes(1);
  });

  it('cancel() drops a pending hold so a late lift never fires it (unmount mid-press)', () => {
    h.onPointerDown(pointerDown());
    h.cancel();
    liftAt(DELAY);
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
    liftAt(DELAY);
    expect(onLongPress).not.toHaveBeenCalled();
  });

  it('a drag hold fires while held, before any lift', () => {
    const drag = makeLongPressHandlers(onLongPress, onClick, { firesWhileHeld: true });
    drag.onPointerDown(pointerDown());
    vi.advanceTimersByTime(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
    liftAt(DELAY);
    expect(onLongPress).toHaveBeenCalledTimes(1);
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
