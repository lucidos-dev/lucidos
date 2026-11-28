
import { isTextEntryField } from './textEntry';
import { postToHost } from './_bridge';

const LOCK_THRESHOLD = 8;
const MIN_SWIPE_DISTANCE = 30;
const FAST_SWIPE_VELOCITY = 0.3;

export const APP_SWIPE_MESSAGE_TYPE = 'lucidos:app:swipe';

export const APP_SWIPE_END_MESSAGE_TYPE = 'lucidos:app:swipe-end';

export class SwipeTouch {
  private _startX = 0;
  private _startY = 0;
  private _startTime = 0;
  private _tracking = false;
  private _direction: 'horizontal' | 'vertical' | null = null;
  private _dx = 0;
  private _lockSign = 0;

  start(x: number, y: number): void {
    this._startX = x;
    this._startY = y;
    this._startTime = Date.now();
    this._tracking = true;
    this._direction = null;
    this._dx = 0;
    this._lockSign = 0;
  }

  move(x: number, y: number): number | null {
    if (!this._tracking) return null;

    const dx = x - this._startX;
    const dy = y - this._startY;

    if (this._direction === null) {
      if (Math.abs(dx) < LOCK_THRESHOLD && Math.abs(dy) < LOCK_THRESHOLD) return null;
      this._direction = Math.abs(dx) >= Math.abs(dy) ? 'horizontal' : 'vertical';
      this._lockSign = Math.sign(dx);
    }

    if (this._direction === 'vertical') return null;

    const adjusted = dx - this._lockSign * LOCK_THRESHOLD;
    this._dx = adjusted;
    return adjusted;
  }

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

  get isHorizontal(): boolean {
    return this._tracking && this._direction === 'horizontal';
  }

  cancel(): void {
    this._tracking = false;
    this._direction = null;
    this._dx = 0;
    this._lockSign = 0;
  }
}

export function claimsHorizontalPan(touchAction: string): boolean {
  if (touchAction === '' || touchAction === 'auto' || touchAction === 'manipulation') return false;
  return !touchAction.split(/\s+/).includes('pan-x');
}

function keepsVerticalPan(touchAction: string): boolean {
  return touchAction.split(/\s+/).some((v) => v === 'pan-y' || v === 'pan-up' || v === 'pan-down');
}

function pageClaimsHorizontalPan(touchAction: string): boolean {
  return claimsHorizontalPan(touchAction) && !keepsVerticalPan(touchAction);
}

export interface DragNode {
  localName?: string;
  type?: string;
  scrollWidth?: number;
  clientWidth?: number;
  parentElement: DragNode | null;
}

export type StyleOf = (node: DragNode) => { overflowX: string; touchAction: string } | null;

export function horizontalDragClaimed(target: DragNode | null, styleOf: StyleOf): boolean {
  for (let node = target; node; node = node.parentElement) {
    if (node.localName === 'input' && node.type === 'range') return true;
    const style = styleOf(node);
    if (!style) continue;
    const isPageRoot = node.localName === 'html' || node.localName === 'body';
    const claims = isPageRoot ? pageClaimsHorizontalPan : claimsHorizontalPan;
    if (claims(style.touchAction)) return true;
    const overflows = (node.scrollWidth ?? 0) > (node.clientWidth ?? 0);
    if (overflows && (style.overflowX === 'auto' || style.overflowX === 'scroll')) return true;
  }
  return false;
}

const computedStyleOf: StyleOf = (node) => {
  if (typeof getComputedStyle !== 'function' || typeof Element === 'undefined') return null;
  return node instanceof Element ? getComputedStyle(node) : null;
};

function textFieldFocused(): boolean {
  const el = typeof document === 'undefined' ? null : document.activeElement;
  if (!el) return false;
  return isTextEntryField(el) || (el as HTMLElement).isContentEditable === true;
}

export interface PaneSwipeHandlers {
  onDrag: (dx: number) => void;
  onRelease: (paneDelta: number) => void;
}

export interface PaneSwipeEnv {
  paneWidth: () => number;
  textFieldFocused: () => boolean;
  styleOf: StyleOf;
  pageScrollsSideways: () => boolean;
}

export interface PageNode {
  scrollingElement: { scrollWidth: number; clientWidth: number } | null;
  documentElement: Element | null;
  body: Element | null;
}

export function pageScrollsSideways(doc: PageNode, overflowXOf: (el: Element) => string): boolean {
  const root = doc.scrollingElement;
  if (!root || root.scrollWidth <= root.clientWidth) return false;
  return [doc.documentElement, doc.body].every((el) => !el || !/^(hidden|clip)$/.test(overflowXOf(el)));
}

export interface TouchTail {
  move: (e: Event) => void;
  end: (e: Event) => void;
  cancel: (e: Event) => void;
}

export function followStrandedTouch(start: Event, surface: EventTarget, tail: TouchTail): () => void {
  const node = start.target as Partial<EventTarget> | null;
  if (!node || node === surface || typeof node.addEventListener !== 'function') return () => {};
  const stranded = (fn: (e: Event) => void) => (e: Event) => {
    if (e.isTrusted && !e.composedPath().includes(surface)) fn(e);
  };
  const listeners: Array<[string, (e: Event) => void, AddEventListenerOptions]> = [
    ['touchmove', stranded(tail.move), { passive: false }],
    ['touchend', stranded(tail.end), { passive: true }],
    ['touchcancel', stranded(tail.cancel), { passive: true }],
  ];
  for (const [type, fn, options] of listeners) node.addEventListener!(type, fn, options);
  return () => {
    for (const [type, fn, options] of listeners) node.removeEventListener!(type, fn, options);
  };
}

function onlyTouch(e: Event): Touch | null {
  const touches = (e as TouchEvent).touches;
  return touches && touches.length === 1 ? touches[0] : null;
}

function screenPoint(t: Touch): [number, number] {
  return [t.screenX, t.screenY];
}

export function trackPaneSwipe(
  surface: EventTarget,
  handlers: PaneSwipeHandlers,
  env: PaneSwipeEnv,
): () => void {
  const touch = new SwipeTouch();
  let unfollow = () => {};

  const finish = () => {
    touch.cancel();
    unfollow();
    unfollow = () => {};
  };

  const abandon = () => {
    if (touch.isHorizontal) handlers.onRelease(0);
    finish();
  };

  const onStart = (e: Event) => {
    abandon();
    const t = onlyTouch(e);
    if (!t || e.defaultPrevented || env.textFieldFocused()) return;
    if (env.pageScrollsSideways() || horizontalDragClaimed(e.target as DragNode | null, env.styleOf)) return;
    touch.start(...screenPoint(t));
    unfollow = followStrandedTouch(e, surface, { move: onMove, end: onEnd, cancel: abandon });
  };

  const onMove = (e: Event) => {
    const t = onlyTouch(e);
    if (!t || e.defaultPrevented) {
      abandon();
      return;
    }
    const dx = touch.move(...screenPoint(t));
    if (dx === null) return;
    if (!e.cancelable) {
      abandon();
      return;
    }
    e.preventDefault();
    handlers.onDrag(dx);
  };

  const onEnd = () => {
    if (touch.isHorizontal) handlers.onRelease(touch.end(env.paneWidth()));
    finish();
  };

  const passive: AddEventListenerOptions = { passive: true };
  const active: AddEventListenerOptions = { passive: false };
  const trusted = (fn: (e: Event) => void) => (e: Event) => { if (e.isTrusted) fn(e); };
  const listeners: Array<[string, (e: Event) => void, AddEventListenerOptions]> = [
    ['touchstart', trusted(onStart), passive],
    ['touchmove', trusted(onMove), active],
    ['touchend', trusted(onEnd), passive],
    ['touchcancel', trusted(abandon), passive],
    ['pagehide', trusted(abandon), passive],
  ];
  for (const [type, fn, options] of listeners) surface.addEventListener(type, fn, options);
  return () => {
    for (const [type, fn, options] of listeners) surface.removeEventListener(type, fn, options);
    finish();
  };
}

export function installFramePaneSwipe(
  win: Window,
  handlers: PaneSwipeHandlers,
  { yieldToWidePage, hostPansSideways = () => false }: { yieldToWidePage: boolean; hostPansSideways?: () => boolean },
): () => void {
  return trackPaneSwipe(win, handlers, {
    paneWidth: () => win.innerWidth,
    textFieldFocused,
    styleOf: computedStyleOf,
    pageScrollsSideways: () => hostPansSideways() || (yieldToWidePage
      && pageScrollsSideways(win.document, (el) => win.getComputedStyle(el).overflowX)),
  });
}

export function installAppPaneSwipe(hostPansSideways?: () => boolean): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  return installFramePaneSwipe(window, {
    onDrag: (dx) => postToHost({ type: APP_SWIPE_MESSAGE_TYPE, dx }),
    onRelease: (paneDelta) => postToHost({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta }),
  }, { yieldToWidePage: false, hostPansSideways });
}
