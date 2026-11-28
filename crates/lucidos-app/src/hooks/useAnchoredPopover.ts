import { useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { JSX } from 'preact';
import { clampWithin, readableVisualViewport, safeAreaTopPx } from '../utils/dom';
import { ANCHOR_GAP } from './anchorGap';
import { notePressOutcome } from '../utils/tapGesture';
import { primaryPointerIsDown } from '../utils/pointerPress';
import { SURFACE_STEP_ATTR, boxOf, startStepMorph, stepSignature, type Box, type StepMorph } from './panelStepMorph';

export interface AnchorPosition {
  top: number;
  left: number;
  placement: 'bottom-start' | 'top-start';
  maxWidth: number;
  maxHeight: number;
}

const MIN_PANEL_ROOM_PX = 96;

const CLAMP_MARGIN = 8;


export type AnchorAlign = 'start' | 'end' | 'center' | 'container-center';

export type AnchorBox = Pick<HTMLElement, 'getBoundingClientRect'>;

export interface ViewportPoint {
  x: number;
  y: number;
}

export function pointAnchor({ x, y }: ViewportPoint): AnchorBox {
  const rect = { x, y, left: x, right: x, top: y, bottom: y, width: 0, height: 0 };
  return { getBoundingClientRect: () => ({ ...rect, toJSON: () => rect }) };
}

export function computeAnchorPosition(
  anchor: AnchorBox,
  panelHeight: number,
  panelWidth: number,
  container?: HTMLElement | null,
  align: AnchorAlign = 'start',
  topInset = 0,
): AnchorPosition {
  const rect = anchor.getBoundingClientRect();
  const vv = readableVisualViewport();
  const viewTop = (vv?.offsetTop ?? 0) + topInset;
  const viewBottom = vv ? vv.offsetTop + vv.height : window.innerHeight;
  const roomBelow = viewBottom - CLAMP_MARGIN - (rect.bottom + ANCHOR_GAP);
  const roomAbove = rect.top - ANCHOR_GAP - (viewTop + CLAMP_MARGIN);
  const wantBelow = panelHeight <= roomBelow || roomBelow >= roomAbove;
  const desiredTop = wantBelow ? rect.bottom + ANCHOR_GAP : rect.top - panelHeight - ANCHOR_GAP;
  const top = clampWithin(desiredTop, panelHeight, viewTop, viewBottom, CLAMP_MARGIN);
  const placement: AnchorPosition['placement'] = wantBelow ? 'bottom-start' : 'top-start';
  const maxHeight = Math.max(wantBelow ? roomBelow : roomAbove, Math.min(panelHeight, MIN_PANEL_ROOM_PX));
  const bounds = container?.getBoundingClientRect();
  const boundsLeft = bounds?.left ?? 0;
  const boundsRight = bounds?.right ?? window.innerWidth;
  const desiredLeft = {
    start: rect.left,
    end: rect.right - panelWidth,
    center: (rect.left + rect.right - panelWidth) / 2,
    'container-center': (boundsLeft + boundsRight - panelWidth) / 2,
  }[align];
  const left = clampWithin(desiredLeft, panelWidth, boundsLeft, boundsRight, CLAMP_MARGIN);
  const containerFit = boundsRight - boundsLeft - 2 * CLAMP_MARGIN;
  const maxWidth = containerFit > 0 ? containerFit : Math.max(0, window.innerWidth - 2 * CLAMP_MARGIN);
  return { top, left, placement, maxWidth, maxHeight };
}

const TOAST_LAYER_SELECTOR = '.toast-container';

export function isOutsidePointerTarget(
  target: Node,
  panel: HTMLElement | null,
  anchor: HTMLElement | null,
): boolean {
  if (panel?.contains(target)) return false;
  if (anchor?.contains(target)) return false;
  if (isInToastLayer(target)) return false;
  return true;
}

function isInToastLayer(target: Node): boolean {
  const el = target.nodeType === 1 ? (target as Element) : target.parentElement;
  return !!el?.closest?.(TOAST_LAYER_SELECTOR);
}

export function useAnchoredPosition(
  anchor: AnchorBox | null,
  panelRef: { current: HTMLElement | null },
  containerSelector?: string,
  align: AnchorAlign = 'start',
): AnchorPosition | null {
  const [pos, setPos] = useState<AnchorPosition | null>(null);
  const measureNow = useRef<(() => void) | null>(null);
  useLayoutEffect(() => {
    if (!anchor) {
      setPos(null);
      return;
    }
    const container = containerSelector && 'closest' in anchor
      ? (anchor as HTMLElement).closest<HTMLElement>(containerSelector)
      : null;
    let rafId: number | null = null;
    let drawn: Box | null = null;
    let morph: StepMorph | null = null;
    let morphFrame: number | null = null;
    const measure = () => {
      const panel = panelRef.current;
      if (!panel || morph) return;
      drawn = boxOf(panel);
      const next = computeAnchorPosition(
        anchor, naturalPanelHeight(panel), panel.offsetWidth, container, align, safeAreaTopPx(),
      );
      setPos(prev =>
        prev &&
        prev.top === next.top &&
        prev.left === next.left &&
        prev.placement === next.placement &&
        prev.maxWidth === next.maxWidth &&
        prev.maxHeight === next.maxHeight
          ? prev
          : next,
      );
    };
    const recompute = () => {
      rafId = null;
      measure();
    };
    const schedule = () => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(recompute);
    };
    const onScroll = (e: Event) => {
      if (e.target instanceof Node && panelRef.current?.contains(e.target)) return;
      schedule();
    };
    measureNow.current = measure;
    measure();
    window.addEventListener('scroll', onScroll, { capture: true, passive: true });
    window.addEventListener('resize', schedule);
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', schedule);
      vv.addEventListener('scroll', schedule);
    }
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(schedule);
    if (ro && panelRef.current) ro.observe(panelRef.current);
    const movingAnchor = 'addEventListener' in anchor ? (anchor as HTMLElement) : null;
    movingAnchor?.addEventListener('transitionend', schedule);
    let steps = panelRef.current ? stepSignature(panelRef.current) : '';
    let morphFrom: Box | null = null;
    const queueMorph = (from: Box) => {
      morphFrom ??= from;
      if (morphFrame !== null) return;
      morphFrame = requestAnimationFrame(() => {
        morphFrame = null;
        const panel = panelRef.current;
        const start = morphFrom;
        morphFrom = null;
        if (!panel || !start) return;
        morph = startStepMorph(panel, start, () => {
          morph = null;
          measure();
        });
      });
    };
    const onContentChanged = () => {
      const panel = panelRef.current;
      if (!panel) return;
      const next = stepSignature(panel);
      const stepped = steps !== '' && next !== '' && next !== steps;
      steps = next;
      if (stepped) {
        const from = morph ? boxOf(panel) : drawn;
        morph?.cancel();
        morph = null;
        if (from) queueMorph(from);
      }
      measure();
    };
    const mo = typeof MutationObserver === 'undefined' ? null : new MutationObserver(onContentChanged);
    if (mo && panelRef.current) {
      mo.observe(panelRef.current, {
        childList: true,
        subtree: true,
        characterData: true,
        attributeFilter: [SURFACE_STEP_ATTR],
      });
    }
    return () => {
      measureNow.current = null;
      if (rafId !== null) cancelAnimationFrame(rafId);
      if (morphFrame !== null) cancelAnimationFrame(morphFrame);
      morph?.cancel();
      ro?.disconnect();
      mo?.disconnect();
      movingAnchor?.removeEventListener('transitionend', schedule);
      window.removeEventListener('scroll', onScroll, true);
      window.removeEventListener('resize', schedule);
      if (vv) {
        vv.removeEventListener('resize', schedule);
        vv.removeEventListener('scroll', schedule);
      }
    };
  }, [anchor, panelRef, containerSelector, align]);
  useLayoutEffect(() => {
    measureNow.current?.();
  }, [pos]);
  return pos;
}

export function naturalPanelHeight(panel: HTMLElement): number {
  const room = panel.style.getPropertyValue('--anchor-room');
  if (!room) return panel.offsetHeight;
  const { scrollTop } = panel;
  panel.style.removeProperty('--anchor-room');
  const height = panel.offsetHeight;
  panel.style.setProperty('--anchor-room', room);
  panel.scrollTop = scrollTop;
  return height;
}

export function anchoredPanelStyle(pos: AnchorPosition | null): JSX.CSSProperties {
  return pos
    ? { top: `${pos.top}px`, left: `${pos.left}px`, maxWidth: `${pos.maxWidth}px` }
    : { visibility: 'hidden' };
}

export function installPairedSwallow(arming?: Event): void {
  if (typeof document === 'undefined') return;
  let done = false;
  let fuse: ReturnType<typeof setTimeout>;
  function teardown() {
    if (done) return;
    done = true;
    document.removeEventListener('touchend', swallow, true);
    document.removeEventListener('click', swallow, true);
    document.removeEventListener('touchcancel', claimCancel, true);
    document.removeEventListener('pointercancel', claimCancel, true);
    document.removeEventListener('pointerdown', onNewGesture, true);
    clearTimeout(fuse);
  }
  function swallow(e: Event) {
    notePressOutcome('swallowed');
    e.stopPropagation();
    e.preventDefault();
    teardown();
  }
  function claimCancel() {
    notePressOutcome('swallowed');
    teardown();
  }
  function onNewGesture(e: Event) {
    if (e === arming) return;
    if ((e as PointerEvent).isPrimary === false) return;
    teardown();
  }
  document.addEventListener('touchend', swallow, { capture: true, passive: false });
  document.addEventListener('click', swallow, true);
  document.addEventListener('touchcancel', claimCancel, { capture: true });
  document.addEventListener('pointercancel', claimCancel, { capture: true });
  document.addEventListener('pointerdown', onNewGesture, { capture: true });
  fuse = setTimeout(teardown, 1500);
}

export function makeDismissHandlers(
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void | boolean,
  onArm?: (arming: Event) => void,
  isTop: () => boolean = () => true,
  openedUnderPress = false,
  dismissOnToastTap = false,
): {
  onPointerDown(e: PointerEvent): void;
  onTouchEnd(e: TouchEvent): void;
  onCancel(): void;
  onClickCapture(e: MouseEvent): void;
  onClickBubble(e: MouseEvent): void;
  onKey(e: KeyboardEvent): void;
} {
  let suppressNextClick = false;
  let insideClick: MouseEvent | null = null;
  let awaitingPairedClick = false;
  let owedToOpeningPress = openedUnderPress;
  let pressBeganInside = false;
  return {
    onPointerDown(e) {
      if (e.isPrimary !== false) owedToOpeningPress = false;
      if (dismissOnToastTap && isInToastLayer(e.target as Node)) {
        if (isTop()) onDismiss();
        return;
      }
      const outside = isOutsidePointerTarget(e.target as Node, panelRef.current, anchor);
      if (e.isPrimary !== false) pressBeganInside = !outside && e.button === 0;
      if (!outside) return;
      if (e.button === 0) awaitingPairedClick = true;
      if (!isTop()) return;
      const dismissed = onDismiss();
      if (e.button === 0 && dismissed !== false) {
        suppressNextClick = true;
        onArm?.(e);
      }
    },
    onTouchEnd(e) {
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) return;
      awaitingPairedClick = false;
      if (!suppressNextClick) return;
      suppressNextClick = false;
      notePressOutcome('swallowed');
      e.stopPropagation();
      e.preventDefault();
    },
    onCancel() {
      awaitingPairedClick = false;
      suppressNextClick = false;
      owedToOpeningPress = false;
      pressBeganInside = false;
    },
    onClickCapture(e) {
      const paired = awaitingPairedClick;
      awaitingPairedClick = false;
      const beganInside = pressBeganInside;
      pressBeganInside = false;
      const opening = owedToOpeningPress;
      owedToOpeningPress = false;
      if (suppressNextClick) {
        suppressNextClick = false;
        e.stopPropagation();
        e.preventDefault();
        return;
      }
      if (!isOutsidePointerTarget(e.target as Node, panelRef.current, anchor)) {
        insideClick = e;
        setTimeout(() => { if (insideClick === e) insideClick = null; }, 0);
        return;
      }
      if (insideClick) return;
      if (paired) return;
      if (opening) return;
      if (beganInside) return;
      if (!isTop()) return;
      const dismissed = onDismiss();
      if (dismissed !== false) {
        e.stopPropagation();
        e.preventDefault();
      }
    },
    onClickBubble(e) {
      if (insideClick === e) insideClick = null;
    },
    onKey(e) {
      if (e.key === 'Escape' && isTop()) onDismiss();
    },
  };
}

export function useDismissOnOutside(
  isOpen: boolean,
  panelRef: { current: HTMLElement | null },
  anchor: HTMLElement | null,
  onDismiss: () => void | boolean,
  isTop?: () => boolean,
  dismissOnToastTap = false,
): void {
  const dismissRef = useRef(onDismiss);
  dismissRef.current = onDismiss;
  const isTopRef = useRef(isTop);
  isTopRef.current = isTop;
  useLayoutEffect(() => {
    if (!isOpen) return;
    const handlers = makeDismissHandlers(
      panelRef,
      anchor,
      () => dismissRef.current(),
      installPairedSwallow,
      () => isTopRef.current?.() ?? true,
      primaryPointerIsDown(),
      dismissOnToastTap,
    );
    document.addEventListener('pointerdown', handlers.onPointerDown, true);
    document.addEventListener('touchend', handlers.onTouchEnd, { capture: true, passive: false });
    document.addEventListener('click', handlers.onClickCapture, true);
    document.addEventListener('click', handlers.onClickBubble);
    document.addEventListener('keydown', handlers.onKey);
    document.addEventListener('pointercancel', handlers.onCancel, true);
    document.addEventListener('touchcancel', handlers.onCancel, true);
    return () => {
      document.removeEventListener('pointerdown', handlers.onPointerDown, true);
      document.removeEventListener('touchend', handlers.onTouchEnd, true);
      document.removeEventListener('click', handlers.onClickCapture, true);
      document.removeEventListener('click', handlers.onClickBubble);
      document.removeEventListener('keydown', handlers.onKey);
      document.removeEventListener('pointercancel', handlers.onCancel, true);
      document.removeEventListener('touchcancel', handlers.onCancel, true);
    };
  }, [isOpen, panelRef, anchor, dismissOnToastTap]);
}
