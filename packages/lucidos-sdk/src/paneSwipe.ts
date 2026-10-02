/**
 * The pane swipe: one gesture model for the host shell and for app iframes.
 *
 * The host tracks sideways drags on its mobile swipe container. An app frame
 * captures every touch it covers, so the SDK bundle tracks drags inside the
 * frame and posts them up. The host moves the panes either way, so this module
 * moves nothing.
 *
 * Vertical scrolling stays native. The frame cancels a `touchmove` only once
 * the drag has locked horizontal, as the host does. A gesture the app already
 * claimed is left alone, which is how an app with its own sideways drag opts
 * out (see `horizontalDragClaimed`).
 */

import { isTextEntryField } from './textEntry';

// Direction lock threshold (px) — below this, direction is undecided
const LOCK_THRESHOLD = 8;
// Minimum distance for a fast swipe (px)
const MIN_SWIPE_DISTANCE = 30;
// Minimum velocity for a fast swipe (px/ms)
const FAST_SWIPE_VELOCITY = 0.3;

/** Posted by an app frame with the drag's `dx` while a sideways drag is live. */
export const APP_SWIPE_MESSAGE_TYPE = 'lucidos:app:swipe';

/** Posted by an app frame when a sideways drag ends, with its `paneDelta`. */
export const APP_SWIPE_END_MESSAGE_TYPE = 'lucidos:app:swipe-end';

/** Pure touch gesture handler for horizontal pane swiping.
 *  No DOM dependencies — fully testable. */
export class SwipeTouch {
  private _startX = 0;
  private _startY = 0;
  private _startTime = 0;
  private _tracking = false;
  private _direction: 'horizontal' | 'vertical' | null = null;
  private _dx = 0;
  /** Which way the finger was going when the direction locked. Frozen there,
   *  because the threshold compensation below is one fixed offset for the whole
   *  gesture. Re-reading the sign per frame flips the offset the moment the
   *  finger drags back past its start, which reports +7 for a 1px move LEFT. */
  private _lockSign = 0;

  /** Record start of a new touch. */
  start(x: number, y: number): void {
    this._startX = x;
    this._startY = y;
    this._startTime = Date.now();
    this._tracking = true;
    this._direction = null;
    this._dx = 0;
    this._lockSign = 0;
  }

  /** Process a touch move. Returns the horizontal delta (px) if tracking
   *  horizontally, or null if vertical/undecided/not tracking. */
  move(x: number, y: number): number | null {
    if (!this._tracking) return null;

    const dx = x - this._startX;
    const dy = y - this._startY;

    if (this._direction === null) {
      if (Math.abs(dx) < LOCK_THRESHOLD && Math.abs(dy) < LOCK_THRESHOLD) return null;
      this._direction = Math.abs(dx) >= Math.abs(dy) ? 'horizontal' : 'vertical';
      // A horizontal lock needs `|dx| >= |dy|` and one of them past the
      // threshold, so `|dx| >= LOCK_THRESHOLD` here and the sign is decided.
      this._lockSign = Math.sign(dx);
    }

    if (this._direction === 'vertical') return null;

    // Subtract the lock threshold so visible drag starts from 0 instead of
    // jumping ±LOCK_THRESHOLD on the first frame after lock.
    const adjusted = dx - this._lockSign * LOCK_THRESHOLD;
    this._dx = adjusted;
    return adjusted;
  }

  /** End the touch. Returns the pane delta: -1 (prev), 0 (snap back), +1 (next).
   *  Only non-zero when the gesture was a confirmed horizontal swipe. */
  end(paneWidth: number): number {
    if (!this._tracking || this._direction !== 'horizontal') {
      this._tracking = false;
      return 0;
    }

    this._tracking = false;
    const elapsed = Math.max(1, Date.now() - this._startTime);
    const velocity = Math.abs(this._dx) / elapsed;

    const fastSwipe = velocity > FAST_SWIPE_VELOCITY && Math.abs(this._dx) > MIN_SWIPE_DISTANCE;
    const farDrag = Math.abs(this._dx) > paneWidth / 3;

    if (fastSwipe || farDrag) {
      return this._dx > 0 ? -1 : 1;
    }

    return 0;
  }

  /** Whether a horizontal swipe is currently being tracked. */
  get isHorizontal(): boolean {
    return this._tracking && this._direction === 'horizontal';
  }

  /** Cancel tracking without producing a result. */
  cancel(): void {
    this._tracking = false;
    this._direction = null;
    this._dx = 0;
    this._lockSign = 0;
  }
}

/** Whether a computed `touch-action` keeps the horizontal pan from the browser.
 *  A carousel declares `pan-y` and a canvas `none`, then handles the sideways
 *  drag itself. Such an element owns the gesture, so it is no pane swipe. */
export function claimsHorizontalPan(touchAction: string): boolean {
  if (touchAction === '' || touchAction === 'auto' || touchAction === 'manipulation') return false;
  return !touchAction.split(/\s+/).includes('pan-x');
}

/** The part of an element the claim check reads. */
export interface DragNode {
  localName?: string;
  type?: string;
  scrollWidth?: number;
  clientWidth?: number;
  parentElement: DragNode | null;
}

/** The computed styles the claim check reads, or null where none apply. */
export type StyleOf = (node: DragNode) => { overflowX: string; touchAction: string } | null;

/** Whether anything from `target` up to the root owns a sideways drag: a range
 *  input's knob, a `touch-action` claim, or an element that scrolls sideways. */
export function horizontalDragClaimed(target: DragNode | null, styleOf: StyleOf): boolean {
  for (let node = target; node; node = node.parentElement) {
    if (node.localName === 'input' && node.type === 'range') return true;
    const style = styleOf(node);
    if (!style) continue;
    if (claimsHorizontalPan(style.touchAction)) return true;
    const overflows = (node.scrollWidth ?? 0) > (node.clientWidth ?? 0);
    if (overflows && (style.overflowX === 'auto' || style.overflowX === 'scroll')) return true;
  }
  return false;
}

/** The live computed style of a real element. */
const computedStyleOf: StyleOf = (node) => {
  if (typeof getComputedStyle !== 'function' || typeof Element === 'undefined') return null;
  return node instanceof Element ? getComputedStyle(node) : null;
};

/** Whether the user is typing in this document, so a drag must not navigate. */
function textFieldFocused(): boolean {
  const el = typeof document === 'undefined' ? null : document.activeElement;
  if (!el) return false;
  return isTextEntryField(el) || (el as HTMLElement).isContentEditable === true;
}

export interface PaneSwipeHandlers {
  /** A sideways drag moved by `dx` px from where it locked. */
  onDrag: (dx: number) => void;
  /** A sideways drag ended: -1 (prev), 0 (snap back) or +1 (next). */
  onRelease: (paneDelta: number) => void;
}

export interface PaneSwipeEnv {
  /** The width a far drag is measured against. */
  paneWidth: () => number;
  textFieldFocused: () => boolean;
  styleOf: StyleOf;
}

function onlyTouch(e: Event): Touch | null {
  const touches = (e as TouchEvent).touches;
  return touches && touches.length === 1 ? touches[0] : null;
}

/** Screen coordinates, never client ones. The host moves the frame WITH the
 *  drag, so a client coordinate loses each step the panes already moved. The
 *  panes then jitter or crawl behind the finger. */
function screenPoint(t: Touch): [number, number] {
  return [t.screenX, t.screenY];
}

/** Track sideways drags on `surface`. Returns the cleanup that removes the
 *  listeners. Every live drag ends in exactly one `onRelease`. */
export function trackPaneSwipe(
  surface: EventTarget,
  handlers: PaneSwipeHandlers,
  env: PaneSwipeEnv,
): () => void {
  const touch = new SwipeTouch();

  const abandon = () => {
    if (touch.isHorizontal) handlers.onRelease(0);
    touch.cancel();
  };

  const onStart = (e: Event) => {
    // A drag whose touchend never arrived (its node left the DOM) ends here.
    abandon();
    const t = onlyTouch(e);
    if (!t || e.defaultPrevented || env.textFieldFocused()) return;
    if (horizontalDragClaimed(e.target as DragNode | null, env.styleOf)) return;
    touch.start(...screenPoint(t));
  };

  const onMove = (e: Event) => {
    const t = onlyTouch(e);
    if (!t || e.defaultPrevented) {
      abandon();
      return;
    }
    const dx = touch.move(...screenPoint(t));
    if (dx === null) return;
    // An uncancelable move means the browser already owns a native pan. A pane
    // swipe now would move the panes and scroll the page at once.
    if (!e.cancelable) {
      abandon();
      return;
    }
    // Stops the page drifting vertically under a sideways drag.
    e.preventDefault();
    handlers.onDrag(dx);
  };

  const onEnd = () => {
    if (touch.isHorizontal) handlers.onRelease(touch.end(env.paneWidth()));
    touch.cancel();
  };

  const passive: AddEventListenerOptions = { passive: true };
  const active: AddEventListenerOptions = { passive: false };
  surface.addEventListener('touchstart', onStart, passive);
  surface.addEventListener('touchmove', onMove, active);
  surface.addEventListener('touchend', onEnd, passive);
  surface.addEventListener('touchcancel', abandon, passive);
  // A frame that reloads mid-drag never sends its touchend.
  surface.addEventListener('pagehide', abandon, passive);
  return () => {
    surface.removeEventListener('touchstart', onStart, passive);
    surface.removeEventListener('touchmove', onMove, active);
    surface.removeEventListener('touchend', onEnd, passive);
    surface.removeEventListener('touchcancel', abandon, passive);
    surface.removeEventListener('pagehide', abandon, passive);
  };
}

/**
 * Install pane-swipe tracking in an app frame. Call once on SDK load. Posts
 * the drag and the release to the host, which moves the panes.
 *
 * No-op when there is no parent window: the SDK loaded at the top level has no
 * host to swipe.
 */
export function installAppPaneSwipe(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  const post = (message: object) => window.parent.postMessage(message, '*');
  return trackPaneSwipe(
    window,
    {
      onDrag: (dx) => post({ type: APP_SWIPE_MESSAGE_TYPE, dx }),
      onRelease: (paneDelta) => post({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta }),
    },
    // The app frame is full-bleed in its pane, so its width is the pane's.
    { paneWidth: () => window.innerWidth, textFieldFocused, styleOf: computedStyleOf },
  );
}
