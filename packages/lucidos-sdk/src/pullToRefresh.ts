/**
 * Pull to refresh: one gesture model for the host shell and for app iframes.
 *
 * The host tracks pulls on its content pane. An app frame captures every touch
 * it covers, so the SDK bundle tracks pulls inside the frame and posts them up.
 * The host draws the affordance and runs the refresh either way, so this module
 * draws nothing.
 *
 * Every listener is passive and nothing here calls `preventDefault()`: a pull
 * must never cost the page its native scroll. A gesture the page already
 * claimed is left alone: a handler that called `preventDefault()`, or an
 * element whose `touch-action` keeps the vertical pan for itself. That is how
 * an app with its own touch handling opts out.
 */

/** How far a pull must travel, in CSS px, before a release refreshes. The
 *  host's arrow drops this far and no further. */
export const PULL_REFRESH_THRESHOLD_PX = 64;

/** The finger moves twice as far as the pull travels, which reads as resistance. */
const PULL_RESISTANCE = 0.5;

/** Movement below this is too small to say which way the drag is going. */
const DIRECTION_SLOP_PX = 8;

/** Posted by an app frame with the current `travel` while a pull is live. */
export const APP_PULL_MESSAGE_TYPE = 'lucidos:app:pull';

/** Posted by an app frame when a pull is released past the threshold. */
export const APP_REFRESH_MESSAGE_TYPE = 'lucidos:app:refresh';

/** The pull's travel for a finger that has moved `dy` px down. Unbounded: the
 *  arrow keeps turning for as long as the finger keeps pulling. */
export function pullTravel(dy: number): number {
  return Math.max(0, dy * PULL_RESISTANCE);
}

/** Whether a release at this travel refreshes. */
export function releaseRefreshes(travel: number): boolean {
  return travel >= PULL_REFRESH_THRESHOLD_PX;
}

export type DragKind = 'undecided' | 'pull' | 'other';

/** What a drag is, once it has moved far enough to tell. Only a mostly
 *  vertical, downward drag is a pull. A sideways one is a pane swipe or a
 *  carousel, and an upward one is an ordinary scroll. */
export function classifyDrag(dx: number, dy: number): DragKind {
  if (Math.abs(dx) < DIRECTION_SLOP_PX && Math.abs(dy) < DIRECTION_SLOP_PX) return 'undecided';
  return dy > 0 && dy > Math.abs(dx) ? 'pull' : 'other';
}

/** The part of an element the arming check reads. */
export interface ScrollNode {
  scrollTop: number;
  parentElement: ScrollNode | null;
}

/** Whether a computed `touch-action` keeps the vertical pan from the browser.
 *  A canvas, a map or a drag handle declares `none` or a horizontal pan. It
 *  then handles the drag itself, often through Pointer Events, which never
 *  cancel the touch. Such an element owns the gesture, so it is no pull. */
export function claimsVerticalPan(touchAction: string): boolean {
  if (touchAction === '' || touchAction === 'auto' || touchAction === 'manipulation') return false;
  return !touchAction.split(/\s+/).includes('pan-y');
}

/** Whether anything from `target` up to the root claims the vertical pan. */
function panClaimedAbove(target: ScrollNode | null): boolean {
  if (typeof getComputedStyle !== 'function' || typeof Element === 'undefined') return false;
  for (let node = target; node; node = node.parentElement) {
    if (node instanceof Element && claimsVerticalPan(getComputedStyle(node).touchAction)) return true;
  }
  return false;
}

/** Whether anything from `target` up to the root is scrolled down. A pull
 *  arms only at the very top, so a finger inside a scrolled inner list is
 *  scrolling that list back up, not pulling. It arms the moment the scroll
 *  reaches the top, so a scroll up that runs into it and carries on pulls. */
export function scrolledAbove(target: ScrollNode | null): boolean {
  for (let node = target; node; node = node.parentElement) {
    if (node.scrollTop > 0) return true;
  }
  return false;
}

export interface PullHandlers {
  /** The pull's travel moved. Called with 0 when a pull is abandoned. */
  onPull: (travel: number) => void;
  /** A pull ended past the threshold. */
  onRefresh: () => void;
}

interface LiveDrag {
  /** Where the pull is measured from: the touch point, or the point the scroll
   *  reached the top at. */
  x: number;
  y: number;
  kind: DragKind;
  travel: number;
  target: ScrollNode | null;
  /** False while something above the target is scrolled down. */
  atTop: boolean;
}

function onlyTouch(e: Event): Touch | null {
  const touches = (e as TouchEvent).touches;
  return touches && touches.length === 1 ? touches[0] : null;
}

/** Track pulls on `surface`. Returns the cleanup that removes the listeners. */
export function trackPullToRefresh(surface: EventTarget, handlers: PullHandlers): () => void {
  let drag: LiveDrag | null = null;

  const abandon = () => {
    if (drag?.kind === 'pull') handlers.onPull(0);
    drag = null;
  };

  const onStart = (e: Event) => {
    // A pull whose touchend never arrived (its node left the DOM) ends here.
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
      // Only a finger below where it landed can have scrolled up to the top,
      // so an ordinary scroll down never walks the ancestors.
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

/**
 * Install pull tracking in an app frame. Call once on SDK load. Posts the
 * travel and the release to the host, which draws and refreshes.
 *
 * No-op when there is no parent window: the SDK loaded at the top level has no
 * host to refresh it.
 */
export function installAppPullToRefresh(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  const post = (message: object) => window.parent.postMessage(message, '*');
  return trackPullToRefresh(window, {
    onPull: (travel) => post({ type: APP_PULL_MESSAGE_TYPE, travel }),
    onRefresh: () => post({ type: APP_REFRESH_MESSAGE_TYPE }),
  });
}
