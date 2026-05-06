import { useEffect, useState } from 'preact/hooks';
import { clampLeftWithin } from '../utils/dom';

export interface AnchorPosition {
  top: number;
  left: number;
  placement: 'bottom-start' | 'top-start';
}

/** Compute a fixed-positioned popover offset relative to an anchor element.
 *  Defaults to placing the popover *below* the anchor; flips to *above* when
 *  there isn't enough vertical room. Horizontally clamps `left` so the panel
 *  stays inside `container` (or the viewport when no container is given) —
 *  necessary on narrow viewports where an anchor near the right edge would
 *  otherwise push the panel off-screen, and to keep the popover visually
 *  contained within its originating pane. Returns viewport-coordinate offsets
 *  ready for `style.top` / `style.left`. */
export function computeAnchorPosition(
  anchor: HTMLElement,
  panelHeight: number,
  panelWidth: number,
  container?: HTMLElement | null,
): AnchorPosition {
  const rect = anchor.getBoundingClientRect();
  const wantBelow = rect.bottom + panelHeight + 8 <= window.innerHeight;
  const top = wantBelow ? rect.bottom + 4 : rect.top - panelHeight - 4;
  const placement: AnchorPosition['placement'] = wantBelow ? 'bottom-start' : 'top-start';
  const bounds = container?.getBoundingClientRect();
  const left = clampLeftWithin(
    rect.left,
    panelWidth,
    bounds?.left ?? 0,
    bounds?.right ?? window.innerWidth,
  );
  return { top, left, placement };
}

/** Decide whether a pointerdown should dismiss the popover. Clicks on the panel
 *  itself or on the anchor element are kept inside — the anchor is excluded so
 *  re-clicking it can toggle the popover via the caller's click handler instead
 *  of being eaten by this dismiss handler firing first. */
export function isOutsidePointerTarget(
  target: Node,
  panel: HTMLElement | null,
  anchor: HTMLElement | null,
): boolean {
  if (panel?.contains(target)) return false;
  if (anchor?.contains(target)) return false;
  return true;
}

/** Track an anchored popover's position and keep it pinned to the anchor as the
 *  page scrolls or resizes. Returns the current viewport offsets, or `null`
 *  when the popover is closed (`anchor === null`).
 *
 *  rAF-coalesced + equality-guarded so a fast scroll burst produces at most one
 *  recompute per frame and no re-render when the anchor's screen position
 *  hasn't actually changed (common during inertia scroll where anchor and
 *  scroll container move together). Passive scroll listener so we don't block
 *  the chat's auto-scroll. */
export function useAnchoredPosition(
  anchor: HTMLElement | null,
  panelRef: { current: HTMLElement | null },
  containerSelector?: string,
): AnchorPosition | null {
  const [pos, setPos] = useState<AnchorPosition | null>(null);
  useEffect(() => {
    if (!anchor) {
      setPos(null);
      return;
    }
    const container = containerSelector ? anchor.closest<HTMLElement>(containerSelector) : null;
    let rafId: number | null = null;
    const recompute = () => {
      rafId = null;
      const panel = panelRef.current;
      if (!panel) return;
      const next = computeAnchorPosition(anchor, panel.offsetHeight, panel.offsetWidth, container);
      setPos(prev =>
        prev && prev.top === next.top && prev.left === next.left && prev.placement === next.placement
          ? prev
          : next,
      );
    };
    const schedule = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(recompute);
    };
    recompute();
    // Capture-phase: the chat pane is its own scroll container, not window — bubbling
    // scrolls would never reach a non-capture window listener.
    window.addEventListener('scroll', schedule, { capture: true, passive: true });
    window.addEventListener('resize', schedule);
    return () => {
      if (rafId !== null) cancelAnimationFrame(rafId);
      window.removeEventListener('scroll', schedule, true);
      window.removeEventListener('resize', schedule);
    };
  }, [anchor, panelRef, containerSelector]);
  return pos;
}

/** Wires "dismiss on click-outside / Escape" for an anchored popover. Scroll and
 *  resize do NOT dismiss — pair with `useAnchoredPosition` if the popover should
 *  follow its anchor through page scrolling. Caller owns open/close state. */
export function useDismissOnOutside(
  isOpen: boolean,
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void,
): void {
  useEffect(() => {
    if (!isOpen) return;
    const onPointerDown = (e: PointerEvent) => {
      if (isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) onDismiss();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onDismiss();
    };
    document.addEventListener('pointerdown', onPointerDown, true);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true);
      document.removeEventListener('keydown', onKey);
    };
  }, [isOpen, panelRef, anchor, onDismiss]);
}
