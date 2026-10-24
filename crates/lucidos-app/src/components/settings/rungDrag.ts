import { useEffect, useRef, useState } from 'preact/hooks';
import { makeLongPressHandlers, type LongPressHandlers } from '../../hooks/useLongPress';
import { rowUnder } from './intelligenceLadder';

/** A rung in flight: the row it was lifted from, the row it would drop on, and
 *  how far the finger has travelled down the page since the lift. */
export interface RungDrag {
  from: number;
  to: number;
  dy: number;
}

/** What a drag reports. */
export interface RungDragCallbacks {
  onLift(from: number): void;
  onMove(drag: RungDrag): void;
  /** The release, once per drag. */
  onDrop(drag: RungDrag): void;
  onCancel(): void;
}

/** The slice of `document` a drag listens on after the lift. */
type DragSurface = Pick<Document, 'addEventListener' | 'removeEventListener'>;

/** Press and drag: a hold lifts the rung under the pointer, or `liftNow` lifts
 *  it at once from the grip, and the pointer then carries it until release. A
 *  swipe that travels before the hold fires never lifts, so it scrolls the page.
 *
 *  The hold is {@link makeLongPressHandlers}. After the lift the drag listens
 *  on `surface`, not the row, so a pointer that leaves the row keeps the rung.
 *  `rowAt` maps a pointer height to a row index. Hook-free so it tests with
 *  fake timers. */
export function makeRungDrag(
  surface: DragSurface,
  rowAt: (y: number) => number,
  callbacks: RungDragCallbacks,
): LongPressHandlers & { liftNow(e: PointerEvent): void; dragging(): boolean } {
  let drag: RungDrag | null = null;
  let startY = 0;
  let fromIndex = 0;

  const follow = (e: PointerEvent) => {
    if (!drag) return;
    drag = { ...drag, to: rowAt(e.clientY), dy: e.clientY - startY };
    callbacks.onMove(drag);
  };
  const stop = () => {
    surface.removeEventListener('pointermove', follow as EventListener);
    surface.removeEventListener('pointerup', release as EventListener);
    surface.removeEventListener('pointercancel', abandon);
    drag = null;
  };
  // A mouse can leave the row, so the row may never see the release. These
  // also tell the hold machine, which arms or clears the guard on the click.
  const release = (e: PointerEvent) => {
    follow(e);
    const landed = drag;
    stop();
    press.onPointerUp(e);
    if (landed) callbacks.onDrop(landed);
  };
  const abandon = (e?: Event) => {
    stop();
    press.onPointerCancel(e as PointerEvent);
    callbacks.onCancel();
  };

  const lift = () => {
    drag = { from: fromIndex, to: fromIndex, dy: 0 };
    surface.addEventListener('pointermove', follow as EventListener);
    surface.addEventListener('pointerup', release as EventListener);
    surface.addEventListener('pointercancel', abandon);
    callbacks.onLift(fromIndex);
  };
  const press = makeLongPressHandlers(lift, () => {}, { firesWhileHeld: true });
  const begin = (e: PointerEvent) => {
    startY = e.clientY;
    fromIndex = rowAt(e.clientY);
  };

  return {
    ...press,
    onPointerDown(e) {
      begin(e);
      press.onPointerDown(e);
    },
    /** A press on the grip: the rung lifts at once, with no hold. */
    liftNow(e: PointerEvent) {
      if (e.button !== 0 || drag) return;
      begin(e);
      lift();
    },
    cancel() {
      press.cancel();
      if (drag) abandon();
    },
    dragging: () => drag !== null,
  };
}

/** Whether the ladder is in its compact layout, which the stylesheet decides
 *  with a container query and publishes as a custom property on each row. The
 *  hook reads that, so the breakpoint lives in the CSS alone. */
function isCompact(row: Element): boolean {
  return getComputedStyle(row).getPropertyValue('--ladder-compact').trim() === '1';
}

/** Drag reorder for one ladder's rows. The wide layout drags by the grip
 *  (`gripProps`), and the compact layout by holding the row (`rowProps`). Rows
 *  carry `data-rung-index`, and `listRef` goes on the element holding them. */
export function useRungDrag(onDrop: (from: number, to: number) => void) {
  const listRef = useRef<HTMLDivElement>(null);
  const [drag, setDrag] = useState<RungDrag | null>(null);
  const dropRef = useRef(onDrop);
  dropRef.current = onDrop;

  // The lifted row is translated with the finger, so its own box would always
  // hold the pointer. The rows are measured once at the lift, before it moves.
  const measured = useRef<{ index: number; bottom: number }[] | null>(null);
  const measure = () => [...(listRef.current?.querySelectorAll<HTMLElement>('[data-rung-index]') ?? [])]
    .map((el) => ({ index: Number(el.dataset.rungIndex), bottom: el.getBoundingClientRect().bottom }));
  const rowAt = (y: number) => {
    const rows = measured.current ?? measure();
    return rows.length ? rowUnder(rows, y) : 0;
  };
  const handlersRef = useRef<ReturnType<typeof makeRungDrag>>();
  handlersRef.current ??= makeRungDrag(document, (y) => rowAt(y), {
    onLift: (from) => {
      measured.current = measure();
      setDrag({ from, to: from, dy: 0 });
    },
    onMove: setDrag,
    onDrop: (landed) => {
      measured.current = null;
      setDrag(null);
      dropRef.current(landed.from, landed.to);
    },
    onCancel: () => {
      measured.current = null;
      setDrag(null);
    },
  });
  const handlers = handlersRef.current;

  // iOS honours `preventDefault` on `touchmove` only from a listener that
  // exists when the touch starts. So this one stays for the list's life and
  // acts only while a rung is lifted.
  useEffect(() => {
    const list = listRef.current;
    if (!list) return undefined;
    const holdPage = (e: TouchEvent) => { if (handlers.dragging()) e.preventDefault(); };
    list.addEventListener('touchmove', holdPage, { passive: false });
    return () => list.removeEventListener('touchmove', holdPage);
  }, [handlers]);
  useEffect(() => () => handlers.cancel(), [handlers]);

  const rowProps = {
    onPointerDown: (e: PointerEvent) => {
      const row = e.currentTarget as HTMLElement;
      // A press on a control belongs to the control.
      if (!isCompact(row) || (e.target as Element).closest('button, label')) return;
      handlers.onPointerDown(e);
    },
    onPointerMove: handlers.onPointerMove,
    onPointerUp: handlers.onPointerUp,
    onPointerLeave: handlers.onPointerLeave,
    onPointerCancel: handlers.onPointerCancel,
    // A touch hold raises the browser's own menu on some platforms.
    onContextMenu: (e: MouseEvent) => { if (isCompact(e.currentTarget as Element)) e.preventDefault(); },
    onClick: handlers.onClick,
  };
  const gripProps = {
    onPointerDown: (e: PointerEvent) => {
      // Cancelling the press keeps the browser from selecting text as it moves.
      e.preventDefault();
      handlers.liftNow(e);
    },
  };
  return { listRef, drag, rowProps, gripProps };
}
