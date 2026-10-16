import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import { EASING_DISCLOSURE, DISCLOSURE_MIN_MS } from '../../utils/disclosureMotion';
import { scaledDurationMs } from '../../utils/motion';
import { moveTile } from './boardArrangement';

/** Pointer travel before a press on a tile's shield becomes a drag. */
export const TILE_DRAG_THRESHOLD_PX = 6;

interface Drag {
  key: string;
  pointerId: number;
  /** Where the pointer holds the tile, from its top left corner. */
  grabX: number;
  grabY: number;
  startX: number;
  startY: number;
  x: number;
  y: number;
  moved: boolean;
  /** The tile the last swap moved past. The pointer must leave it before it
   *  can swap again, or a slide still under the pointer swaps straight back. */
  passed: string | null;
}

/** Slide each element from where it was to where it is now. */
function slideFrom(el: HTMLElement, from: DOMRect, to: DOMRect): void {
  const dx = from.left - to.left;
  const dy = from.top - to.top;
  const ms = scaledDurationMs(DISCLOSURE_MIN_MS);
  if ((Math.abs(dx) < 1 && Math.abs(dy) < 1) || ms <= 0 || typeof el.animate !== 'function') return;
  el.animate(
    [{ transform: `translate(${dx}px, ${dy}px)` }, { transform: 'none' }],
    { duration: ms, easing: EASING_DISCLOSURE },
  );
}

/** The tile under a point, other than the one being dragged. */
function tileUnder(x: number, y: number, dragged: string, known: Map<string, HTMLElement>): string | null {
  if (typeof document.elementsFromPoint !== 'function') return null;
  for (const el of document.elementsFromPoint(x, y)) {
    const key = (el as HTMLElement).closest<HTMLElement>('[data-home-tile]')?.dataset.homeTile;
    if (key && key !== dragged && known.has(key)) return key;
  }
  return null;
}

/** Drag to reorder the Home board's tiles in edit mode (ADR 0432).
 *
 *  The dragged tile follows the pointer on a transform, and the others reflow
 *  live through CSS `order`, sliding to their new places. No element ever
 *  moves in the DOM, so no frame reloads. Only a drop commits: `onDrop` runs
 *  once per drag, never per pointer move. `moveBy` is the keyboard's Alt plus
 *  an arrow, one place per press. */
export function useBoardDrag({ order, enabled, onDrop }: {
  order: readonly string[];
  enabled: boolean;
  onDrop: (order: string[]) => void;
}) {
  const elements = useRef(new Map<string, HTMLElement>());
  const drag = useRef<Drag | null>(null);
  const before = useRef<Map<string, DOMRect> | null>(null);
  const [draft, setDraft] = useState<string[] | null>(null);
  const draftRef = useRef<string[] | null>(null);
  const [dragging, setDragging] = useState<string | null>(null);
  const shown = draft ?? order;

  const register = useCallback((key: string, el: HTMLElement | null) => {
    if (el) elements.current.set(key, el);
    else elements.current.delete(key);
  }, []);

  const snapshot = () => {
    before.current = new Map([...elements.current].map(([k, el]) => [k, el.getBoundingClientRect()]));
  };

  /** Keep the dragged tile under the pointer from wherever its slot now is. */
  const follow = () => {
    const d = drag.current;
    const el = d?.moved ? elements.current.get(d.key) : undefined;
    if (!d || !el) return;
    el.style.transform = '';
    const slot = el.getBoundingClientRect();
    el.style.transform = `translate(${d.x - d.grabX - slot.left}px, ${d.y - d.grabY - slot.top}px)`;
  };

  useLayoutEffect(() => {
    const previous = before.current;
    before.current = null;
    if (previous) {
      for (const [key, el] of elements.current) {
        const from = previous.get(key);
        if (from && key !== drag.current?.key) slideFrom(el, from, el.getBoundingClientRect());
      }
    }
    follow();
  }, [shown.join('\n')]);

  const setDraftOrder = (next: string[] | null) => {
    draftRef.current = next;
    setDraft(next);
  };

  const finish = (pointerId: number, commit: boolean) => {
    const d = drag.current;
    if (!d || d.pointerId !== pointerId) return;
    drag.current = null;
    const el = elements.current.get(d.key);
    if (el && d.moved) {
      const held = el.getBoundingClientRect();
      el.style.transform = '';
      slideFrom(el, held, el.getBoundingClientRect());
    }
    setDragging(null);
    const next = draftRef.current;
    setDraftOrder(null);
    if (commit && d.moved && next && next.join('\n') !== order.join('\n')) onDrop(next);
  };

  const shield = (key: string) => ({
    onPointerDown(e: PointerEvent) {
      const el = elements.current.get(key);
      if (!enabled || !e.isPrimary || e.button !== 0 || !el) return;
      (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
      const box = el.getBoundingClientRect();
      drag.current = {
        key,
        pointerId: e.pointerId,
        grabX: e.clientX - box.left,
        grabY: e.clientY - box.top,
        startX: e.clientX,
        startY: e.clientY,
        x: e.clientX,
        y: e.clientY,
        moved: false,
        passed: null,
      };
    },
    onPointerMove(e: PointerEvent) {
      const d = drag.current;
      if (!d || d.pointerId !== e.pointerId) return;
      d.x = e.clientX;
      d.y = e.clientY;
      if (!d.moved) {
        if (Math.hypot(e.clientX - d.startX, e.clientY - d.startY) < TILE_DRAG_THRESHOLD_PX) return;
        d.moved = true;
        setDragging(key);
      }
      follow();
      const target = tileUnder(e.clientX, e.clientY, key, elements.current);
      if (target !== d.passed) d.passed = null;
      if (!target || target === d.passed) return;
      const current = draftRef.current ?? [...order];
      const next = moveTile(current, key, current.indexOf(target));
      if (next.join('\n') === current.join('\n')) return;
      d.passed = target;
      snapshot();
      setDraftOrder(next);
    },
    onPointerUp: (e: PointerEvent) => finish(e.pointerId, true),
    onPointerCancel: (e: PointerEvent) => finish(e.pointerId, false),
    onLostPointerCapture: (e: PointerEvent) => finish(e.pointerId, false),
  });

  // Leaving edit mode mid-drag drops the drag, uncommitted.
  useEffect(() => {
    if (!enabled && drag.current) finish(drag.current.pointerId, false);
  }, [enabled]);

  const moveBy = (key: string, delta: number) => {
    const at = order.indexOf(key);
    if (at < 0) return;
    const next = moveTile(order, key, at + delta);
    if (next.join('\n') === order.join('\n')) return;
    snapshot();
    onDrop(next);
  };

  return { shown, dragging, register, shield, moveBy };
}
