
import { postToHost } from './_bridge';

export const PULL_REFRESH_THRESHOLD_PX = 64;

const PULL_RESISTANCE = 0.5;

const DIRECTION_SLOP_PX = 8;

export const APP_PULL_MESSAGE_TYPE = 'lucidos:app:pull';

export const APP_REFRESH_MESSAGE_TYPE = 'lucidos:app:refresh';

export function pullTravel(dy: number): number {
  return Math.max(0, dy * PULL_RESISTANCE);
}

export function releaseRefreshes(travel: number): boolean {
  return travel >= PULL_REFRESH_THRESHOLD_PX;
}

export type DragKind = 'undecided' | 'pull' | 'other';

export function classifyDrag(dx: number, dy: number): DragKind {
  if (Math.abs(dx) < DIRECTION_SLOP_PX && Math.abs(dy) < DIRECTION_SLOP_PX) return 'undecided';
  return dy > 0 && dy > Math.abs(dx) ? 'pull' : 'other';
}

export interface ScrollNode {
  scrollTop: number;
  parentElement: ScrollNode | null;
}

export function claimsVerticalPan(touchAction: string): boolean {
  if (touchAction === '' || touchAction === 'auto' || touchAction === 'manipulation') return false;
  return !touchAction.split(/\s+/).includes('pan-y');
}

function panClaimedAbove(target: ScrollNode | null): boolean {
  if (typeof getComputedStyle !== 'function' || typeof Element === 'undefined') return false;
  for (let node = target; node; node = node.parentElement) {
    if (node instanceof Element && claimsVerticalPan(getComputedStyle(node).touchAction)) return true;
  }
  return false;
}

export function scrolledAbove(target: ScrollNode | null): boolean {
  for (let node = target; node; node = node.parentElement) {
    if (node.scrollTop > 0) return true;
  }
  return false;
}

export interface PullHandlers {
  onPull: (travel: number) => void;
  onRefresh: () => void;
}

interface LiveDrag {
  x: number;
  y: number;
  kind: DragKind;
  travel: number;
  target: ScrollNode | null;
  atTop: boolean;
}

function onlyTouch(e: Event): Touch | null {
  const touches = (e as TouchEvent).touches;
  return touches && touches.length === 1 ? touches[0] : null;
}

export function trackPullToRefresh(surface: EventTarget, handlers: PullHandlers): () => void {
  let drag: LiveDrag | null = null;

  const abandon = () => {
    if (drag?.kind === 'pull') handlers.onPull(0);
    drag = null;
  };

  const onStart = (e: Event) => {
    abandon();
    const touch = onlyTouch(e);
    const target = e.target as ScrollNode | null;
    if (!touch || e.defaultPrevented || panClaimedAbove(target)) return;
    drag = { x: touch.clientX, y: touch.clientY, kind: 'undecided', travel: 0, target, atTop: !scrolledAbove(target) };
  };

  const onMove = (e: Event) => {
    if (!drag) return;
    const touch = onlyTouch(e);
    if (!touch || e.defaultPrevented) {
      abandon();
      return;
    }
    if (!drag.atTop) {
      if (touch.clientY <= drag.y || scrolledAbove(drag.target)) return;
      drag.atTop = true;
      drag.x = touch.clientX;
      drag.y = touch.clientY;
      return;
    }
    const dx = touch.clientX - drag.x;
    const dy = touch.clientY - drag.y;
    if (drag.kind === 'undecided') {
      drag.kind = classifyDrag(dx, dy);
      if (drag.kind === 'other') {
        drag = null;
        return;
      }
      if (drag.kind === 'undecided') return;
    }
    drag.travel = pullTravel(dy);
    handlers.onPull(drag.travel);
  };

  const onEnd = () => {
    const refresh = drag?.kind === 'pull' && releaseRefreshes(drag.travel);
    abandon();
    if (refresh) handlers.onRefresh();
  };

  const options: AddEventListenerOptions = { passive: true };
  surface.addEventListener('touchstart', onStart, options);
  surface.addEventListener('touchmove', onMove, options);
  surface.addEventListener('touchend', onEnd, options);
  surface.addEventListener('touchcancel', abandon, options);
  return () => {
    surface.removeEventListener('touchstart', onStart, options);
    surface.removeEventListener('touchmove', onMove, options);
    surface.removeEventListener('touchend', onEnd, options);
    surface.removeEventListener('touchcancel', abandon, options);
  };
}

export function installAppPullToRefresh(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  return trackPullToRefresh(window, {
    onPull: (travel) => postToHost({ type: APP_PULL_MESSAGE_TYPE, travel }),
    onRefresh: () => postToHost({ type: APP_REFRESH_MESSAGE_TYPE }),
  });
}
