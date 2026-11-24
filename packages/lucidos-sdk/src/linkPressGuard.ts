
import { cancelledPressWasATap } from './tapPress';

export const LINK_CLICK_AFTER_TOUCH_MS = 500;

export interface SwallowedLinkClick {
  href: string;
  movedPx: number;
  fingers: number;
  heldMs: number;
}

export interface LinkTouch extends SwallowedLinkClick {
  tap: boolean;
}

export interface LinkPressGuardOptions {
  onLinkTouch?: (info: LinkTouch) => void;
  onSwallowed?: (info: SwallowedLinkClick) => void;
  now?: () => number;
}

interface TouchContact {
  identifier: number;
  screenX: number;
  screenY: number;
}

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
    const anchor = (raw.target as Element | null)?.closest?.('a[href]');
    if (anchor) {
      opts.onLinkTouch?.({
        href: anchor.getAttribute('href') ?? '',
        tap: verdict.tap,
        movedPx: verdict.movedPx,
        fingers: verdict.fingers,
        heldMs,
      });
    }
  }

  function onClick(e: Event): void {
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
  for (const [type, fn] of listeners) {
    target.addEventListener(type, fn, { capture: true, passive: type !== 'click' });
  }
  return () => {
    for (const [type, fn] of listeners) target.removeEventListener(type, fn, { capture: true });
  };
}
