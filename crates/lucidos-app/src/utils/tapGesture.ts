import { TAP_MOVE_THRESHOLD_PX, cancelledPressWasATap } from '@lucidos/tap-press';

export interface TapPointer {
  screenX: number;
  screenY: number;
}

interface TapState {
  startX: number;
  startY: number;
  movedPx: number;
  canceled: boolean;
}

export function createTapGate() {
  let state: TapState | null = null;
  let aborted = false;

  function settle(): number | null {
    const press = state;
    state = null;
    if (press === null || !press.canceled) return null;
    return Math.round(press.movedPx);
  }

  return {
    down(p: TapPointer): void {
      state = { startX: p.screenX, startY: p.screenY, movedPx: 0, canceled: false };
      aborted = false;
    },
    move(p: TapPointer): void {
      if (!state) return;
      state.movedPx = Math.max(
        state.movedPx,
        Math.abs(p.screenX - state.startX),
        Math.abs(p.screenY - state.startY),
      );
      if (state.movedPx > TAP_MOVE_THRESHOLD_PX) state.canceled = true;
    },
    cancel(): void {
      state = null;
      aborted = true;
    },
    spend(): void {
      state = null;
    },
    wasAborted(): boolean {
      return aborted;
    },
    isTap(): boolean {
      return settle() === null;
    },
    tapRejection(): number | null {
      return settle();
    },
  };
}

const TOUCH_CLICK_WINDOW_MS = 500;

export type PressOutcome = 'served' | 'swallowed' | 'refused' | 'aborted';

let lastPress: { outcome: PressOutcome; at: number } | null = null;

export function notePressOutcome(outcome: PressOutcome, now: number = Date.now()): void {
  lastPress = { outcome, at: now };
}

export function takePressOutcome(withinMs: number, now: number = Date.now()): PressOutcome | null {
  const press = lastPress;
  lastPress = null;
  if (press === null || now - press.at > withinMs) return null;
  return press.outcome;
}

function swallowedSince(since: number): boolean {
  return lastPress !== null && lastPress.outcome === 'swallowed' && lastPress.at >= since;
}

export interface TouchEventLike {
  preventDefault(): void;
}

interface TouchContact {
  identifier: number;
  screenX: number;
  screenY: number;
}

export interface ScrollNode {
  scrollTop: number;
  scrollLeft: number;
  parentElement: ScrollNode | null;
}

export interface TouchPressLike {
  changedTouches: ArrayLike<TouchContact>;
  touches: ArrayLike<unknown>;
  currentTarget?: ScrollNode | null;
  timeStamp?: number;
}

interface ScrollMark {
  node: ScrollNode;
  top: number;
  left: number;
}

function scrollMarksAbove(el: ScrollNode | null | undefined): ScrollMark[] {
  const marks: ScrollMark[] = [];
  for (let node = el?.parentElement ?? null; node?.parentElement; node = node.parentElement) {
    marks.push({ node, top: node.scrollTop, left: node.scrollLeft });
  }
  return marks;
}

function scrolledSince(marks: ScrollMark[]): boolean {
  return marks.some(m => m.node.scrollTop !== m.top || m.node.scrollLeft !== m.left);
}

function contactOf(e: TouchPressLike, identifier: number): TouchContact | null {
  for (let i = 0; i < e.changedTouches.length; i++) {
    if (e.changedTouches[i].identifier === identifier) return e.changedTouches[i];
  }
  return null;
}

export interface ActivationGate {
  pass(): boolean;
  spend(): void;
  aborted?(): boolean;
}

export interface TouchActivateOptions {
  enabled?: () => boolean;
  gate?: ActivationGate;
  destructive?: () => boolean;
  shownAt?: () => number | null;
  now?: () => number;
  pageVisible?: () => boolean;
}

export const CANCELLED_TAP_SETTLE_MS = 400;

export const FACE_SEEN_MS = 200;

export function pressPredatesFace(startedAt: number | null, shownAt: number | null): boolean {
  if (startedAt === null) return false;
  if (shownAt === null) return true;
  return startedAt < shownAt + FACE_SEEN_MS;
}

export function touchActivated(action: () => void, opts: TouchActivateOptions = {}) {
  const enabled = opts.enabled ?? (() => true);
  const destructive = opts.destructive ?? (() => false);
  const clock = opts.now ?? Date.now;
  const pageVisible = opts.pageVisible ?? (() => document.visibilityState === 'visible');
  let lastTouchAt: number | null = null;
  let press: {
    identifier: number;
    startX: number;
    startY: number;
    at: number;
    movedPx: number;
    fingers: number;
    destructive: boolean;
    startedAt: number | null;
    served: boolean;
    scrollMarks: ScrollMark[];
  } | null = null;
  function liftMayServe(p: typeof press): boolean {
    if (p?.served) return false;
    if (p && p.movedPx > TAP_MOVE_THRESHOLD_PX && scrolledSince(p.scrollMarks)) return false;
    if (p && opts.shownAt && destructive() && pressPredatesFace(p.startedAt, opts.shownAt())) {
      return false;
    }
    return (p?.destructive ?? false) === destructive();
  }
  function refuse(p: typeof press): void {
    opts.gate?.spend();
    notePressOutcome(p?.served ? 'served' : 'refused');
  }
  function travel(e: TouchPressLike): void {
    if (!press) return;
    const t = contactOf(e, press.identifier);
    if (!t) return;
    press.movedPx = Math.max(
      press.movedPx,
      Math.abs(t.screenX - press.startX),
      Math.abs(t.screenY - press.startY),
    );
  }
  return {
    onTouchStart(e: TouchPressLike): void {
      const t = e.changedTouches[0];
      press = t ? {
        identifier: t.identifier,
        startX: t.screenX,
        startY: t.screenY,
        at: clock(),
        movedPx: 0,
        fingers: e.touches.length,
        destructive: destructive(),
        startedAt: typeof e.timeStamp === 'number' ? e.timeStamp : null,
        served: false,
        scrollMarks: scrollMarksAbove(e.currentTarget),
      } : null;
    },
    onTouchMove(e: TouchPressLike): void {
      if (!press) return;
      press.fingers = Math.max(press.fingers, e.touches.length);
      travel(e);
    },
    onTouchCancel(e: TouchPressLike): void {
      travel(e);
      const p = press;
      press = null;
      if (!p || !contactOf(e, p.identifier)) return;
      if (!enabled()) {
        notePressOutcome('refused');
        return;
      }
      if (!liftMayServe(p)) {
        refuse(p);
        return;
      }
      if (swallowedSince(p.at)) return;
      const tap = cancelledPressWasATap({
        heldMs: clock() - p.at,
        movedPx: p.movedPx,
        fingers: p.fingers,
        fingersLeft: e.touches.length,
      });
      if (!tap) return;
      opts.gate?.spend();
      notePressOutcome('served');
      if (!destructive()) {
        action();
        return;
      }
      setTimeout(() => {
        if (pageVisible() && enabled() && destructive()) action();
      }, CANCELLED_TAP_SETTLE_MS);
    },
    onTouchEnd(e: TouchEventLike & Partial<TouchPressLike>): void {
      if (e.changedTouches && e.touches) travel({ changedTouches: e.changedTouches, touches: e.touches });
      const p = press;
      press = null;
      if (!enabled()) {
        notePressOutcome('refused');
        return;
      }
      lastTouchAt = clock();
      e.preventDefault();
      if (!liftMayServe(p)) {
        refuse(p);
        return;
      }
      if (destructive()) {
        if (opts.gate?.aborted?.()) {
          notePressOutcome('aborted');
          return;
        }
        if (!(opts.gate?.pass() ?? true)) return;
      } else {
        opts.gate?.spend();
      }
      notePressOutcome('served');
      action();
    },
    onClick(): void {
      if (lastTouchAt !== null && clock() - lastTouchAt < TOUCH_CLICK_WINDOW_MS) {
        lastTouchAt = null;
        return;
      }
      if (opts.gate && !opts.gate.pass()) return;
      if (press) press.served = true;
      action();
    },
  };
}
