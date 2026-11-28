import { type TemporaryOwner, scrollPosition } from './scrollOwner';
import { signal } from '@preact/signals';
import { cancelRideSettle, markHeldScroll, stopFollowingBottom } from './scrollRide';
import { writeLedgerScroll, wroteWithinEventWindow, lastWriteKind } from './scrollLedger';
import {
  resolveTarget,
  liveEdgeTop,
  syncAwayFromBottom,
  awayFromBottom,
  turnLandingClearancePx,
  isLiveEdgeTop,
  getActiveScrollElement,
} from './scrollSurface';
import { isReducedMotion } from '../../utils/motion';
import { honourLanding } from './scrollLanding';
import { clearPendingEventScroll } from './scrollLink';
import { rereadReadingLine } from './scrollHold';
import { isMobile } from '../../utils/viewport';
import { isElementVisible } from '../../utils/elementVisibility';
import { navFocusElement, applyNavFocus } from '../shared/focusMarker';

const SCROLL_MIN_MS = 240;

const SCROLL_MAX_MS = 760;

const SCROLL_PX_PER_MS = 6.5;

const SCROLL_FRAME_MS = 1000 / 60;

const easeOutCubic = (t: number) => 1 - Math.pow(1 - t, 3);

const easeInOutCubic = (t: number) => (t < 0.5 ? 4 * t * t * t : 1 - Math.pow(-2 * t + 2, 3) / 2);

interface ScrollPace {
  minMs: number;
  ease: (t: number) => number;
}

const NAVIGATION_PACE: ScrollPace = { minMs: SCROLL_MIN_MS, ease: easeOutCubic };

export const SEND_PACE: ScrollPace = { minMs: 450, ease: easeInOutCubic };

let _scrollAnimRaf: number | null = null;

export const scrollAnimating = () => _scrollAnimRaf !== null;

let tweenWriter: TemporaryOwner | null = null;

type HeldGlide = 'ride' | 'landing';

const heldGlide = signal<HeldGlide | null>(null);

export const heldGlideInFlight = () => heldGlide.value !== null;

let _heldGlideFrozenTop: number | null = null;

let _shiftTweenStart: ((delta: number) => void) | null = null;

let _continueTweenTo: ((target: number) => void) | null = null;

let _endedTweens = 0;

function endScrollAnim() {
  _scrollAnimRaf = null;
  if (tweenWriter) scrollPosition.transition({ type: 'finish', owner: tweenWriter });
  tweenWriter = null;
  heldGlide.value = null;
  _heldGlideFrozenTop = null;
  _shiftTweenStart = null;
  _continueTweenTo = null;
  _endedTweens++;
}

export function cancelScrollAnim() {
  if (_scrollAnimRaf !== null) cancelAnimationFrame(_scrollAnimRaf);
  endScrollAnim();
  cancelRideSettle();
}

export function markNavigationScroll(el: HTMLElement, top: number) {
  const before = el.scrollTop;
  const owner = scrollPosition.owner();
  if (owner.kind === 'reader' || owner.kind === 'ride') {
    scrollPosition.withWriter({ kind: 'navigation' }, () => writeLedgerScroll(el, top, 'placement'));
  } else {
    writeLedgerScroll(el, top, 'placement');
  }
  const firesNoScrollEvent = el.scrollTop === before;
  if (firesNoScrollEvent) rereadReadingLine(el);
}

export function markAnchorScroll(el: HTMLElement, top: number): void {
  scrollPosition.withWriter({ kind: 'hold' }, () => {
    const before = el.scrollTop;
    writeLedgerScroll(el, top, 'anchor');
    if (_shiftTweenStart && el === resolveTarget()) _shiftTweenStart(el.scrollTop - before);
  });
}

export function markRevealScroll(el?: HTMLElement | null): void {
  const target = el ?? resolveTarget();
  if (!target) return;
  markNavigationScroll(target, target.scrollTop);
}

export function isNavigationScroll(el?: HTMLElement | null): boolean {
  if (_scrollAnimRaf !== null) return true;
  return wroteWithinEventWindow(el);
}

export function isPlacementScroll(el: HTMLElement, navigation: boolean): boolean {
  const kind = lastWriteKind(el);
  if (kind === 'held' || kind === 'carry') return false;
  if (kind === 'anchor' && rideGlideInFlight()) return false;
  return navigation;
}

export function isAnchorScroll(el: HTMLElement): boolean {
  return lastWriteKind(el) === 'anchor' && isNavigationScroll(el);
}

export function isOtherNavigationScroll(el: HTMLElement): boolean {
  if (_scrollAnimRaf !== null) return true;
  return lastWriteKind(el) !== 'anchor' && isNavigationScroll(el);
}

export function landingGlideInFlight(): boolean {
  return heldGlide.value === 'landing';
}

export function rideGlideInFlight(): boolean {
  return heldGlide.value === 'ride';
}

function snapToLiveEdge(el: HTMLElement, frozenTop = liveEdgeTop(el)): void {
  cancelScrollAnim();
  markHeldScroll(el, frozenTop);
  syncAwayFromBottom();
}

export function glideToLiveEdge(el: HTMLElement, owner: HeldGlide, freezeAt?: number, ridePace = NAVIGATION_PACE): void {
  if (heldGlide.value === owner) {
    if (freezeAt !== undefined) freezeLandingGlide(freezeAt);
    return;
  }
  if (isReducedMotion()) {
    if (owner === 'landing') {
      scrollPosition.withWriter({ kind: 'landing' }, () => snapToLiveEdge(el, freezeAt));
    } else {
      snapToLiveEdge(el, freezeAt);
    }
    return;
  }
  animateScroll((c) => {
    if (owner === 'landing') honourLanding(c);
    return _heldGlideFrozenTop ?? liveEdgeTop(c);
  }, () => {
    syncAwayFromBottom();
    if (owner !== 'landing') return;
    const cur = resolveTarget();
    if (cur) honourLanding(cur);
  }, markHeldScroll, owner === 'landing' ? SEND_PACE : ridePace, owner === 'landing' ? { kind: 'landing' } : null);
  heldGlide.value = owner;
  if (freezeAt !== undefined) freezeLandingGlide(freezeAt);
}

export function freezeLandingGlide(top: number): void {
  if (!landingGlideInFlight() || _heldGlideFrozenTop !== null) return;
  retargetLandingGlide(top);
}

export function retargetLandingGlide(top: number): void {
  _heldGlideFrozenTop = top;
  _continueTweenTo?.(top);
}

export function animateScroll(
  targetOf: (el: HTMLElement) => number,
  onDone?: () => void,
  mark: (el: HTMLElement, top: number) => void = markNavigationScroll,
  pace: ScrollPace = NAVIGATION_PACE,
  writer: TemporaryOwner | null = { kind: 'navigation' },
) {
  cancelScrollAnim();
  const endedBefore = _endedTweens;
  tweenWriter = writer?.kind === 'link'
    ? scrollPosition.landLink(writer.claim, false)
    : writer;
  if (writer && writer.kind !== 'link') scrollPosition.transition({ type: 'take', owner: writer });
  let started = false;
  let start = 0;
  let startTime = 0;
  let duration = pace.minMs;
  let lastTop = 0;
  let lastEase = 0;
  _shiftTweenStart = (delta) => { if (started) { start += delta; lastTop += delta; } };
  _continueTweenTo = (target) => {
    if (started && lastEase < 1) start = (lastTop - target * lastEase) / (1 - lastEase);
  };
  const step = (now: number) => {
    const cur = resolveTarget();
    if (!cur) { endScrollAnim(); return; }
    if (!started) {
      started = true;
      start = cur.scrollTop;
      lastTop = start;
      startTime = now - SCROLL_FRAME_MS;
      const distance = Math.abs(targetOf(cur) - start);
      duration = Math.min(SCROLL_MAX_MS, Math.max(pace.minMs, distance / SCROLL_PX_PER_MS));
    }
    const target = targetOf(cur);
    if (_endedTweens !== endedBefore) return;
    const t = Math.min(1, (now - startTime) / duration);
    if (t >= 1) {
      mark(cur, target);
      endScrollAnim();
      onDone?.();
      return;
    }
    lastEase = pace.ease(t);
    lastTop = start + (target - start) * lastEase;
    mark(cur, lastTop);
    _scrollAnimRaf = requestAnimationFrame(step);
  };
  _scrollAnimRaf = requestAnimationFrame(step);
}

export function landingTargetOf(el: HTMLElement): (c: HTMLElement) => number {
  const marginTop =
    typeof getComputedStyle === 'function'
      ? (parseFloat(getComputedStyle(el).scrollMarginTop) || 0)
      : 0;
  return (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function' && typeof el.getBoundingClientRect === 'function'
      ? el.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - marginTop
      : 0;
}

export function smoothScrollToElement(el: HTMLElement, writer: Extract<TemporaryOwner, { kind: 'link' }>, onDone?: () => void): void {
  const targetOf = landingTargetOf(el);
  if (isReducedMotion()) {
    cancelScrollAnim();
    const c = resolveTarget();
    if (c) {
      const link = scrollPosition.landLink(writer.claim, false);
      try {
        markNavigationScroll(c, Math.max(0, targetOf(c)));
      } finally {
        scrollPosition.transition({ type: 'finish', owner: link });
      }
    }
    onDone?.();
    return;
  }
  animateScroll(targetOf, onDone, markNavigationScroll, NAVIGATION_PACE, writer);
}

export function scrollToTop() {
  clearPendingEventScroll();
  awayFromBottom.value = true;

  const el = resolveTarget();
  if (!el) return;

  stopFollowingBottom();

  if (isReducedMotion()) {
    cancelScrollAnim();
    markNavigationScroll(el, 0);
    _scrollAnimRaf = requestAnimationFrame(() => {
      const t = resolveTarget();
      if (t) markNavigationScroll(t, 0);
      _scrollAnimRaf = null;
    });
    return;
  }

  animateScroll(() => 0);
}

export function scrollToBottomAnimated() {
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el || isReducedMotion()) { scrollToBottom(); return; }
  animateScroll(
    liveEdgeTop,
    syncAwayFromBottom,
  );
}

export function scrollToBottom() {
  clearPendingEventScroll();
  cancelScrollAnim();

  const target = resolveTarget();
  if (target) markNavigationScroll(target, target.scrollHeight);
  syncAwayFromBottom();
}

export const TURN_SELECTOR = '.chat-exchange';

const TURN_NAV_THRESHOLD_SLACK_PX = 4;

export function pickTurnIndex(
  tops: number[], scrollTop: number, direction: 1 | -1, threshold: number,
): number | null {
  if (tops.length === 0) return null;
  if (direction === 1) {
    for (let i = 0; i < tops.length; i++) {
      if (tops[i] > scrollTop + threshold) return i;
    }
    return null;
  }
  for (let i = tops.length - 1; i >= 0; i--) {
    if (tops[i] < scrollTop - threshold) return i;
  }
  return null;
}

export function pickTurnTarget(
  anchorIdx: number,
  tops: number[],
  scrollTop: number,
  direction: 1 | -1,
  threshold: number,
): number | null {
  if (anchorIdx >= 0) {
    const next = anchorIdx + direction;
    return next >= 0 && next < tops.length ? next : null;
  }
  return pickTurnIndex(tops, scrollTop, direction, threshold);
}

export function stepThreadTurn(direction: 1 | -1): HTMLElement | null {
  const el = resolveTarget();
  if (!el) return null;

  const focused = isMobile() ? null : el;
  focused?.focus({ preventScroll: true });

  const turns = Array.from(el.querySelectorAll<HTMLElement>(TURN_SELECTOR)).filter(isElementVisible);
  if (turns.length === 0) return focused;
  const gap = turnLandingClearancePx(turns[0]);
  const containerTop = el.getBoundingClientRect().top;
  const tops = turns.map((t) => t.getBoundingClientRect().top - containerTop + el.scrollTop);
  const markedTurn = navFocusElement()?.closest?.(TURN_SELECTOR) as HTMLElement | null;
  const anchorIdx = markedTurn ? turns.indexOf(markedTurn) : -1;
  const idx = pickTurnTarget(anchorIdx, tops, el.scrollTop + gap, direction, TURN_NAV_THRESHOLD_SLACK_PX);
  if (idx === null) return focused;
  const turn = turns[idx];

  clearPendingEventScroll();

  const targetOf = (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function'
      ? turn.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - gap
      : 0;

  awayFromBottom.value = !isLiveEdgeTop(el, targetOf(el));

  if (awayFromBottom.value) stopFollowingBottom();

  applyNavFocus(turn);

  if (isReducedMotion()) {
    cancelScrollAnim();
    markNavigationScroll(el, Math.max(0, targetOf(el)));
    return focused;
  }
  animateScroll(targetOf);
  return focused;
}

export type TurnCollapseKind = 'response' | 'initiator';

export function parseNavigatedTurn(
  threadId: string | null,
  userSeqAttr: string | null,
  kind: string | null,
): { threadId: string; userSeq: number; kind: TurnCollapseKind } | null {
  if (!threadId) return null;
  if (kind !== 'response' && kind !== 'initiator') return null;
  if (userSeqAttr === null || userSeqAttr.trim() === '') return null;
  const userSeq = Number(userSeqAttr);
  if (!Number.isInteger(userSeq)) return null;
  return { threadId, userSeq, kind };
}

export function isEventInViewport(eventId: string): boolean {
  if (!eventId || typeof document === 'undefined' || !document.querySelectorAll) return false;
  const matches = document.querySelectorAll<HTMLElement>(`[data-event-id="${CSS.escape(eventId)}"]`);
  for (const el of matches) {
    if (isElementOnScreen(el)) return true;
  }
  return false;
}

export function isReplyEndInViewport(threadId: string): boolean {
  const last = lastExchangeOf(threadId);
  if (!last || !isElementVisible(last)) return false;
  const bounds = visibleBand();
  const r = last.getBoundingClientRect();
  return r.bottom > bounds.top && r.bottom <= bounds.bottom
    && r.right > bounds.left && r.left < bounds.right;
}

export function lastExchangeOf(threadId: string): HTMLElement | null {
  if (!threadId || typeof document === 'undefined' || !document.querySelectorAll) return null;
  const all = document.querySelectorAll<HTMLElement>(`.chat-exchange[data-thread-id="${CSS.escape(threadId)}"]`);
  return all.length > 0 ? all[all.length - 1] : null;
}

export function isElementOnScreen(el: HTMLElement): boolean {
  if (!isElementVisible(el)) return false;
  const bounds = visibleBand();
  const r = el.getBoundingClientRect();
  return r.bottom > bounds.top && r.top < bounds.bottom
    && r.right > bounds.left && r.left < bounds.right;
}

function visibleBand(): { top: number; bottom: number; left: number; right: number } {
  const scroller = getActiveScrollElement();
  return scroller && isElementVisible(scroller)
    ? scroller.getBoundingClientRect()
    : {
        top: 0,
        bottom: window.innerHeight || document.documentElement.clientHeight,
        left: 0,
        right: window.innerWidth || document.documentElement.clientWidth,
      };
}
