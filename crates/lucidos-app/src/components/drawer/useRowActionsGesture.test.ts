import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { makeLongPressHandlers, type LongPressCallback } from '../../hooks/useLongPress';
import { rowGestureHandlers } from './useRowActionsGesture';

const row = { closest: () => null };
const pinButton = { closest: (sel: string) => (sel === 'button' ? {} : null) };

function pointerDown(target: object = row, button = 0): PointerEvent {
  return { button, clientX: 0, clientY: 0, currentTarget: row, target } as unknown as PointerEvent;
}
function contextMenu(target: object = row, clientX = 40, clientY = 60, altKey = false) {
  return { preventDefault: vi.fn(), currentTarget: row, target, clientX, clientY, altKey } as unknown as MouseEvent & {
    preventDefault: ReturnType<typeof vi.fn>;
  };
}
function click() {
  return { preventDefault: vi.fn(), stopPropagation: vi.fn() } as unknown as MouseEvent;
}

describe('rowGestureHandlers', () => {
  let open: ReturnType<typeof vi.fn<LongPressCallback>>;
  let tap: ReturnType<typeof vi.fn<() => void>>;
  let prefetch: ReturnType<typeof vi.fn<() => void>>;

  beforeEach(() => {
    vi.useFakeTimers();
    open = vi.fn<LongPressCallback>();
    tap = vi.fn<() => void>();
    prefetch = vi.fn<() => void>();
  });
  afterEach(() => vi.useRealTimers());

  const build = (mobile: boolean) => rowGestureHandlers({
    mobile,
    enabled: true,
    press: makeLongPressHandlers(open, tap),
    onPress: prefetch,
  });

  describe('desktop', () => {
    it('a right-click opens the row menu at the pointer and claims the event', () => {
      const h = build(false);
      const e = contextMenu(row, 40, 60);
      h.onPointerDown!(pointerDown(row, 2));
      h.onContextMenu!(e);
      expect(open).toHaveBeenCalledWith(row, { x: 40, y: 60 });
      expect(e.preventDefault).toHaveBeenCalled();
    });

    it('a ctrl-click opens the menu and never focuses the thread', () => {
      const h = build(false);
      h.onPointerDown!(pointerDown());
      h.onContextMenu!(contextMenu());
      h.onClick!(click());
      expect(open).toHaveBeenCalledTimes(1);
      expect(tap).not.toHaveBeenCalled();
    });

    it('the next plain click after a right-click still focuses the thread', () => {
      const h = build(false);
      h.onPointerDown!(pointerDown(row, 2));
      h.onContextMenu!(contextMenu());
      h.onPointerDown!(pointerDown());
      h.onClick!(click());
      expect(tap).toHaveBeenCalledTimes(1);
    });

    it('a held press never opens the menu', () => {
      const h = build(false);
      h.onPointerDown!(pointerDown());
      vi.advanceTimersByTime(2000);
      expect(open).not.toHaveBeenCalled();
      expect(h.onPointerMove).toBeUndefined();
    });

    it('a right-click on an inline control is left to that control', () => {
      const h = build(false);
      const e = contextMenu(pinButton);
      h.onContextMenu!(e);
      expect(open).not.toHaveBeenCalled();
      expect(e.preventDefault).not.toHaveBeenCalled();
    });

    it('Option+right-click is left to the native menu', () => {
      const h = build(false);
      const e = contextMenu(row, 40, 60, true);
      h.onContextMenu!(e);
      expect(open).not.toHaveBeenCalled();
      expect(e.preventDefault).not.toHaveBeenCalled();
    });

    it('every press prefetches, a right-click included', () => {
      const h = build(false);
      h.onPointerDown!(pointerDown(row, 2));
      expect(prefetch).toHaveBeenCalledTimes(1);
    });
  });

  describe('mobile', () => {
    it('a hold opens the menu and swallows its paired click', () => {
      const h = build(true);
      h.onPointerDown!(pointerDown());
      vi.advanceTimersByTime(450);
      h.onPointerUp!(pointerDown());
      h.onClick!(click());
      expect(open).toHaveBeenCalledWith(row);
      expect(tap).not.toHaveBeenCalled();
    });

    it('a hold that starts on an inline control is left to it', () => {
      const h = build(true);
      h.onPointerDown!(pointerDown(pinButton));
      vi.advanceTimersByTime(450);
      expect(open).not.toHaveBeenCalled();
      expect(prefetch).toHaveBeenCalledTimes(1);
    });
  });

  it('a skeleton row answers nothing', () => {
    const h = rowGestureHandlers({ mobile: false, enabled: false, press: makeLongPressHandlers(open, tap) });
    expect(h).toEqual({});
  });
});
