import { useEffect, useRef, useState } from 'preact/hooks';
import { makeLongPressHandlers, type LongPressHandlers } from './useLongPress';

/** A row in flight: the row it was lifted from, the row it would drop on, and
 *  the translation keeping it under the pointer, including scroll since lift. */
export interface RowDrag {
  from: number;
  to: number;
  dy: number;
}

/** What a drag reports. */
export interface RowDragCallbacks {
  onLift(from: number): void;
  onMove(drag: RowDrag): void;
  /** The release, once per drag. */
  onDrop(drag: RowDrag): void;
  onCancel(): void;
}

/** The slice of `document` a drag listens on after the lift. */
type DragSurface = Pick<Document, 'addEventListener' | 'removeEventListener'>;

/** The row a drag at `y` is over: the first whose bottom edge is below `y`,
 *  else the last. `rows` run top to bottom. */
export function rowUnder(rows: readonly { index: number; bottom: number }[], y: number): number {
  return (rows.find((row) => y < row.bottom) ?? rows[rows.length - 1]).index;
}

/** Press and drag: a hold lifts the row under the pointer, or `liftNow` lifts
 *  it at once from the grip, and the pointer then carries it until release. A
 *  swipe that travels before the hold fires never lifts, so it scrolls the page.
 *
 *  The hold is {@link makeLongPressHandlers}. After the lift the drag listens
 *  on `surface`, not the row, so a pointer that leaves the row keeps it.
 *  `rowAt` maps a pointer height to a row index. Hook-free so it tests with
 *  fake timers. `scrollOffset` reports how far the list has scrolled since lift. */
export function makeRowDrag(
  surface: DragSurface,
  rowAt: (y: number) => number,
  callbacks: RowDragCallbacks,
  scrollOffset: () => number = () => 0,
): LongPressHandlers & { liftNow(e: PointerEvent): void; dragging(): boolean } {
  let drag: RowDrag | null = null;
  let startY = 0;
  let fromIndex = 0;
  let pointerY = 0;

  const refresh = () => {
    if (!drag) return;
    const y = pointerY + scrollOffset();
    drag = { ...drag, to: rowAt(y), dy: y - startY };
    callbacks.onMove(drag);
  };
  const follow = (e: PointerEvent) => {
    pointerY = e.clientY;
    refresh();
  };
  const stop = () => {
    surface.removeEventListener('pointermove', follow as EventListener);
    surface.removeEventListener('pointerup', release as EventListener);
    surface.removeEventListener('pointercancel', abandon);
    surface.removeEventListener('scroll', refresh, true);
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
    // Scroll does not bubble; capture nested scrollers as well as the page.
    surface.addEventListener('scroll', refresh, true);
    callbacks.onLift(fromIndex);
  };
  const press = makeLongPressHandlers(lift, () => {});
  const begin = (e: PointerEvent) => {
    pointerY = startY = e.clientY;
    fromIndex = rowAt(e.clientY);
  };

  return {
    ...press,
    onPointerDown(e) {
      begin(e);
      press.onPointerDown(e);
    },
    /** A press on the grip: the row lifts at once, with no hold. */
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

/** Drag reorder for one list's rows: by the grip (`gripProps`), or by holding
 *  the row (`rowProps`) where `holdToDrag(row)` says so. Rows carry
 *  `data-row-index`, and `listRef` goes on the element holding them. */
export function useRowDrag<List extends HTMLElement = HTMLDivElement>(
  onDrop: (from: number, to: number) => void,
  holdToDrag: (row: Element) => boolean,
) {
  // State, not a ref object: the touch listener below must move with the element.
  const [list, listRef] = useState<List | null>(null);
  const listNow = useRef(list);
  listNow.current = list;
  const [drag, setDrag] = useState<RowDrag | null>(null);
  const dropRef = useRef(onDrop);
  dropRef.current = onDrop;

  // The lifted row is translated with the finger, so its own box would always
  // hold the pointer. The rows are measured once at the lift, before it moves.
  const listOrigin = () => {
    const el = listNow.current;
    return el ? el.getBoundingClientRect().top - el.scrollTop : 0;
  };
  const liftOrigin = useRef(0);
  const measured = useRef<{ index: number; top: number; bottom: number }[] | null>(null);
  const measure = () => [...(listNow.current?.querySelectorAll<HTMLElement>('[data-row-index]') ?? [])]
    .map((el) => {
      const { top, bottom } = el.getBoundingClientRect();
      return { index: Number(el.dataset.rowIndex), top, bottom };
    });
  const rowAt = (y: number) => {
    const rows = measured.current ?? measure();
    return rows.length ? rowUnder(rows, y) : 0;
  };
  const handlersRef = useRef<ReturnType<typeof makeRowDrag>>();
  handlersRef.current ??= makeRowDrag(document, (y) => rowAt(y), {
    onLift: (from) => {
      measured.current = measure();
      liftOrigin.current = listOrigin();
      setDrag({ from, to: from, dy: 0 });
    },
    onMove: (moving) => {
      const rows = measured.current!;
      const source = rows.find(row => row.index === moving.from);
      // Transformed rows contribute to scroll overflow. Keep the lifted box
      // inside the original list so scrolling cannot grow its own range.
      const dy = source ? Math.max(rows[0].top - source.top,
        Math.min(rows[rows.length - 1].bottom - source.bottom, moving.dy)) : moving.dy;
      setDrag({ ...moving, dy });
    },
    onDrop: (landed) => {
      measured.current = null;
      setDrag(null);
      dropRef.current(landed.from, landed.to);
    },
    onCancel: () => {
      measured.current = null;
      setDrag(null);
    },
  }, () => liftOrigin.current - listOrigin());
  const handlers = handlersRef.current;

  // iOS honours `preventDefault` on `touchmove` only from a listener that
  // exists when the touch starts. So this one stays for the list's life and
  // acts only while a row is lifted. A list drawn after a load mounts later
  // than the hook, so the listener follows the element, not the first render.
  useEffect(() => {
    if (!list) return undefined;
    const holdPage = (e: TouchEvent) => { if (handlers.dragging()) e.preventDefault(); };
    list.addEventListener('touchmove', holdPage, { passive: false });
    return () => list.removeEventListener('touchmove', holdPage);
  }, [list, handlers]);
  useEffect(() => () => handlers.cancel(), [handlers]);

  const rowProps = {
    onPointerDown: (e: PointerEvent) => {
      const row = e.currentTarget as HTMLElement;
      // A press on a control belongs to the control.
      if (!holdToDrag(row) || (e.target as Element).closest('button, label')) return;
      handlers.onPointerDown(e);
    },
    onPointerMove: handlers.onPointerMove,
    onPointerUp: handlers.onPointerUp,
    onPointerLeave: handlers.onPointerLeave,
    onPointerCancel: handlers.onPointerCancel,
    // A touch hold raises the browser's own menu on some platforms.
    onContextMenu: (e: MouseEvent) => { if (holdToDrag(e.currentTarget as Element)) e.preventDefault(); },
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
