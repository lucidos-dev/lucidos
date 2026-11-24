import { isWebKit } from './platform';
import { deepLinkHasResolved, hasPendingEventScroll } from '../components/chat/scrollState';
import { isUserScrolling } from './scrollActivity';
import { OPEN_REPAINT_BURST_DELAYS_MS } from './repaintBurst';


export const SCROLLER_PINNED_ATTR = 'data-scroller-pinned';

export const PINNED_SHIFT_PROP = '--repaint-nudge-shift';

interface InFlightToggle {
  raf1: number;
  raf2: number | undefined;
  prev: string;
  nudgeScroll: boolean;
  restoreTop: number | undefined;
  nudgedTop: number | undefined;
  pinned: HTMLElement[];
}
const inFlight = new WeakMap<object, InFlightToggle>();

let lastNudgeAt = -Infinity;

export const NUDGE_EVENT_WINDOW_MS = 64;

export function isRepaintNudging(now: number = performance.now()): boolean {
  return now - lastNudgeAt < NUDGE_EVENT_WINDOW_MS;
}

function writeNudgedScrollTop(el: HTMLElement, top: number) {
  lastNudgeAt = performance.now();
  el.scrollTop = top;
}

function nudgedTransform(prev: string, entry: InFlightToggle): string {
  const delta = nudgeDelta(entry);
  const compensation = delta === 0 ? '' : `translateY(${delta}px) `;
  return `${compensation}${prev ? `${prev} ` : ''}translateZ(0.1px)`;
}

function nudgeDelta(entry: InFlightToggle): number {
  return entry.nudgedTop === undefined ? 0 : entry.nudgedTop - entry.restoreTop!;
}

function publishPinnedShift(el: HTMLElement, entry: InFlightToggle) {
  const delta = nudgeDelta(entry);
  if (delta === 0) return;
  entry.pinned = Array.from(el.children).filter(
    child => child.hasAttribute(SCROLLER_PINNED_ATTR),
  ) as HTMLElement[];
  for (const pinned of entry.pinned) pinned.style.setProperty(PINNED_SHIFT_PROP, `${delta}px`);
}

function clearPinnedShift(entry: InFlightToggle) {
  for (const pinned of entry.pinned) pinned.style.removeProperty(PINNED_SHIFT_PROP);
  entry.pinned = [];
}

function restoreNudge(el: HTMLElement, entry: InFlightToggle) {
  if (entry.nudgedTop !== undefined && el.scrollTop === entry.nudgedTop) {
    writeNudgedScrollTop(el, entry.restoreTop!);
  }
}

export function repaintNudgeShift(el: HTMLElement): number {
  const entry = inFlight.get(el);
  if (entry?.nudgedTop === undefined || el.scrollTop !== entry.nudgedTop) return 0;
  return nudgeDelta(entry);
}

export function settledScrollTop(el: HTMLElement): number {
  return el.scrollTop - repaintNudgeShift(el);
}

function deepLinkSeekingTarget(): boolean {
  return hasPendingEventScroll() && !deepLinkHasResolved();
}

export function forceWebKitRepaint(el: HTMLElement | null | undefined): (() => void) | undefined {
  if (!el?.isConnected || !isWebKit()) return;

  const existing = inFlight.get(el);
  const prev = existing ? existing.prev : el.style.transform;
  const nudgeScroll = existing
    ? existing.nudgeScroll
    : el.scrollHeight > el.clientHeight && !deepLinkSeekingTarget() && !isUserScrolling();

  if (existing) {
    cancelAnimationFrame(existing.raf1);
    if (existing.raf2 !== undefined) cancelAnimationFrame(existing.raf2);
    if (el.style.transform !== prev) el.style.transform = prev;
    clearPinnedShift(existing);
    restoreNudge(el, existing);
  }

  const entry: InFlightToggle = { raf1: 0, raf2: undefined, prev, nudgeScroll, restoreTop: undefined, nudgedTop: undefined, pinned: [] };
  inFlight.set(el, entry);
  const done = () => { if (inFlight.get(el) === entry) inFlight.delete(el); };
  entry.raf1 = requestAnimationFrame(() => {
    if (!el.isConnected) { done(); return; }
    if (nudgeScroll && !isUserScrolling()) {
      const live = el.scrollTop;
      entry.restoreTop = live;
      entry.nudgedTop = live > 0 ? live - 1 : live + 1;
    }
    el.style.transform = nudgedTransform(prev, entry);
    publishPinnedShift(el, entry);
    if (entry.nudgedTop !== undefined) writeNudgedScrollTop(el, entry.nudgedTop);
    void el.offsetHeight;
    entry.raf2 = requestAnimationFrame(() => {
      if (el.isConnected) {
        el.style.transform = prev;
        clearPinnedShift(entry);
        restoreNudge(el, entry);
      }
      done();
    });
  });
  return () => {
    cancelAnimationFrame(entry.raf1);
    if (entry.raf2 !== undefined) cancelAnimationFrame(entry.raf2);
    if (el.style.transform !== prev) el.style.transform = prev;
    clearPinnedShift(entry);
    restoreNudge(el, entry);
    done();
  };
}

export function forceWebKitRepaintBurst(el: HTMLElement | null | undefined): (() => void) | undefined {
  if (!el?.isConnected || !isWebKit()) return;
  let immediateCleanup: (() => void) | undefined;
  const timers: Array<ReturnType<typeof setTimeout>> = [];
  for (const delay of OPEN_REPAINT_BURST_DELAYS_MS) {
    if (delay === 0) {
      immediateCleanup = forceWebKitRepaint(el);
    } else {
      timers.push(setTimeout(() => forceWebKitRepaint(el), delay));
    }
  }
  return () => {
    immediateCleanup?.();
    for (const t of timers) clearTimeout(t);
    timers.length = 0;
  };
}

export interface RepaintThrottle {
  request(now: number, fire: () => void): void;
  cancel(): void;
}

export function createRepaintThrottle(intervalMs: number): RepaintThrottle {
  let last = -Infinity;
  let trailingTimer: ReturnType<typeof setTimeout> | null = null;
  const clearTrailing = () => {
    if (trailingTimer !== null) {
      clearTimeout(trailingTimer);
      trailingTimer = null;
    }
  };
  return {
    request(now: number, fire: () => void) {
      clearTrailing();
      const elapsed = now - last;
      if (elapsed >= intervalMs) {
        last = now;
        fire();
        return;
      }
      const wait = intervalMs - elapsed;
      trailingTimer = setTimeout(() => {
        trailingTimer = null;
        last = now + wait;
        fire();
      }, wait);
    },
    cancel: clearTrailing,
  };
}
