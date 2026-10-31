/**
 * The link press guard: a swipe that starts on a link never opens it.
 *
 * iOS fires `click` after a short drag that stays under its ~10 px cancel
 * threshold. So a swipe that began on a link can open the page, in Safari or
 * in the in-app web view. The guard records each touch itself. It cancels the
 * click of a touch that was not a tap, before any link handler sees it.
 *
 * It rules on links only (`a[href]`), and only on a click close after its
 * touch. An old swipe never vetoes a mouse click or a keyboard Enter.
 */

import { cancelledPressWasATap } from './tapPress';

/** How long after a touch ends a click counts as that touch's synthetic twin. */
export const LINK_CLICK_AFTER_TOUCH_MS = 500;

/** What the guard knew when it cancelled a link click. */
export interface SwallowedLinkClick {
  href: string;
  movedPx: number;
  fingers: number;
  heldMs: number;
}

export interface LinkPressGuardOptions {
  /** Called once per cancelled click, for the breadcrumb. */
  onSwallowed?: (info: SwallowedLinkClick) => void;
  /** Injected clock, so the click window is testable. */
  now?: () => number;
}

interface TouchContact {
  identifier: number;
  screenX: number;
  screenY: number;
}

/** What the guard reads off a touch event, in screen space like the tap gates. */
interface TouchLike {
  changedTouches: ArrayLike<TouchContact>;
  touches: ArrayLike<unknown>;
}

interface Press {
  identifier: number;
  startX: number;
  startY: number;
  startedAt: number;
  movedPx: number;
  fingers: number;
}

interface Verdict {
  tap: boolean;
  movedPx: number;
  fingers: number;
  heldMs: number;
  endedAt: number;
}

function contactOf(e: TouchLike, identifier: number): TouchContact | null {
  for (let i = 0; i < e.changedTouches.length; i++) {
    if (e.changedTouches[i].identifier === identifier) return e.changedTouches[i];
  }
  return null;
}

/** Install the guard on `target` and return the uninstall. The default,
 *  `window`, captures ahead of every `document` listener, whatever their order. */
export function installLinkPressGuard(
  target: EventTarget = window,
  opts: LinkPressGuardOptions = {},
): () => void {
  const clock = opts.now ?? Date.now;
  let press: Press | null = null;
  let verdict: Verdict | null = null;

  function travel(p: Press, e: TouchLike): void {
    const c = contactOf(e, p.identifier);
    if (!c) return;
    p.movedPx = Math.max(p.movedPx, Math.abs(c.screenX - p.startX), Math.abs(c.screenY - p.startY));
  }

  function onStart(raw: Event): void {
    const e = raw as unknown as TouchLike;
    if (press && e.touches.length > 1) {
      press.fingers = Math.max(press.fingers, e.touches.length);
      return;
    }
    const t = e.changedTouches[0];
    verdict = null;
    press = t
      ? { identifier: t.identifier, startX: t.screenX, startY: t.screenY, startedAt: clock(), movedPx: 0, fingers: e.touches.length }
      : null;
  }

  function onMove(raw: Event): void {
    const e = raw as unknown as TouchLike;
    if (!press) return;
    press.fingers = Math.max(press.fingers, e.touches.length);
    travel(press, e);
  }

  // A cancel counts like a lift: iOS can take a press mid-swipe, and the click
  // that follows a drag still arrives on some paths.
  function onEnd(raw: Event): void {
    const e = raw as unknown as TouchLike;
    const p = press;
    if (!p || !contactOf(e, p.identifier)) return;
    travel(p, e);
    press = null;
    const endedAt = clock();
    const heldMs = endedAt - p.startedAt;
    verdict = {
      tap: cancelledPressWasATap({ heldMs, movedPx: p.movedPx, fingers: p.fingers, fingersLeft: e.touches.length }),
      movedPx: Math.round(p.movedPx),
      fingers: p.fingers,
      heldMs,
      endedAt,
    };
  }

  function onClick(e: Event): void {
    // Consuming, so a stale swipe never rules on a later click.
    const v = verdict;
    verdict = null;
    if (!v || v.tap || clock() - v.endedAt > LINK_CLICK_AFTER_TOUCH_MS) return;
    const anchor = (e.target as Element | null)?.closest?.('a[href]');
    if (!anchor) return;
    e.preventDefault();
    e.stopPropagation();
    e.stopImmediatePropagation();
    opts.onSwallowed?.({
      href: anchor.getAttribute('href') ?? '',
      movedPx: v.movedPx,
      fingers: v.fingers,
      heldMs: v.heldMs,
    });
  }

  const listeners: Array<[string, (e: Event) => void]> = [
    ['touchstart', onStart],
    ['touchmove', onMove],
    ['touchend', onEnd],
    ['touchcancel', onEnd],
    ['click', onClick],
  ];
  // The touch listeners are passive and the click one is not: it cancels.
  for (const [type, fn] of listeners) {
    target.addEventListener(type, fn, { capture: true, passive: type !== 'click' });
  }
  return () => {
    for (const [type, fn] of listeners) target.removeEventListener(type, fn, { capture: true });
  };
}
