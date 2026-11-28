import { useEffect, useRef, useState } from 'preact/hooks';
import { makeLongPressHandlers, type LongPressHandlers } from './useLongPress';

export interface RowDrag {
  from: number;
  to: number;
  dy: number;
}

export interface RowDragCallbacks {
  onLift(from: number): void;
  onMove(drag: RowDrag): void;
  onDrop(drag: RowDrag): void;
  onCancel(): void;
}

type DragSurface = Pick<Document, 'addEventListener' | 'removeEventListener'>;

export function rowUnder(rows: readonly { index: number; bottom: number }[], y: number): number {
  return (rows.find((row) => y < row.bottom) ?? rows[rows.length - 1]).index;
}

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

export function useRowDrag<List extends HTMLElement = HTMLDivElement>(
  onDrop: (from: number, to: number) => void,
  holdToDrag: (row: Element) => boolean,
) {
  const [list, listRef] = useState<List | null>(null);
  const listNow = useRef(list);
  listNow.current = list;
  const [drag, setDrag] = useState<RowDrag | null>(null);
  const dropRef = useRef(onDrop);
  dropRef.current = onDrop;

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
      if (!holdToDrag(row) || (e.target as Element).closest('button, label')) return;
      handlers.onPointerDown(e);
    },
    onPointerMove: handlers.onPointerMove,
    onPointerUp: handlers.onPointerUp,
    onPointerLeave: handlers.onPointerLeave,
    onPointerCancel: handlers.onPointerCancel,
    onContextMenu: (e: MouseEvent) => { if (holdToDrag(e.currentTarget as Element)) e.preventDefault(); },
    onClick: handlers.onClick,
  };
  const gripProps = {
    onPointerDown: (e: PointerEvent) => {
      e.preventDefault();
      handlers.liftNow(e);
    },
  };
  return { listRef, drag, rowProps, gripProps };
}
