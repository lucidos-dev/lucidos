import { computed, signal, type ReadonlySignal } from '@preact/signals';
import { transcriptScrolls } from './transcriptScrolls';
import { isReducedMotion } from '../../utils/motion';
import { postClientLog } from '../../utils/clientLog';
import { markBoot } from '../../utils/bootTiming';
import { isScrollKeyPress, nowMs } from '../../utils/scrollActivity';
import {
  TOUCH_COAST_IDLE_MS,
  TOUCH_COAST_MAX_MS,
  attachReaderInput,
  isScrollHeld,
  onScrollHoldReleased,
  readerGestureActive,
  readerGestureForTest,
  readerGestureSince,
  readerScrollOwed,
  retireReaderMovement,
  scrollHoldReleased,
} from './readerInput';
import {
  type NavScrollKind,
  forgetLeftPosition,
  holdPosition,
  holdsReaderOn,
  isWhereWeHeldIt,
  isWhereWeLastScrolledIt,
  lastWriteKind,
  onRebasedScroll,
  writeLedgerScroll,
  writeCount,
  wroteWithinEventWindow,
} from './scrollLedger';
import { isMobile } from '../../utils/viewport';
import { isImageRetryPending } from '../../utils/markdownImageRetry';
import { applyNavFocus, clearNavFocus, navFocusElement } from '../shared/focusMarker';
import { isElementVisible, unmeasurableBox } from '../../utils/elementVisibility';
import { EVENT_RESOLVE_DEADLINE_MS } from './deepLinkBudget';
import { readLineBlock } from './scrollAnchor';
import type { DeepLinkTarget } from '../../store/thread-events/exchange-anchors';

export const awayFromBottom = signal(false);
export const notAtTop = signal(false);
export const deepLinkTarget = signal<DeepLinkTarget | null>(null);
export const scrolledFromTop = signal(false);

let _activeScrollElement: HTMLElement | null = null;

export function setActiveScrollElement(el: HTMLElement | null) {
  _activeScrollElement = el;
}

function findVisibleThreadContent(): HTMLElement | null {
  if (typeof document === 'undefined' || !document.querySelectorAll) return null;
  const elements = document.querySelectorAll('.thread-content');
  for (const el of elements) {
    if (isElementVisible(el as HTMLElement)) return el as HTMLElement;
  }
  return null;
}

export function getActiveScrollElement(): HTMLElement | null {
  return _activeScrollElement;
}

function resolveTarget(): HTMLElement | null {
  let el = _activeScrollElement;
  if (el && !isElementVisible(el)) el = null;
  return el ?? findVisibleThreadContent();
}

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

const SEND_PACE: ScrollPace = { minMs: 450, ease: easeInOutCubic };

let _scrollAnimRaf: number | null = null;
let _heldAnim = false;
type HeldGlide = 'ride' | 'landing';
let _heldAnimTarget: HeldGlide | null = null;
const _rideGliding = signal(false);
let _heldGlideFrozenTop: number | null = null;
let _shiftTweenStart: ((delta: number) => void) | null = null;
let _continueTweenTo: ((target: number) => void) | null = null;
let _endedTweens = 0;
function endScrollAnim() {
  _scrollAnimRaf = null;
  _heldAnim = false;
  _heldAnimTarget = null;
  _rideGliding.value = false;
  _heldGlideFrozenTop = null;
  _shiftTweenStart = null;
  _continueTweenTo = null;
  _endedTweens++;
}
function cancelScrollAnim() {
  if (_scrollAnimRaf !== null) cancelAnimationFrame(_scrollAnimRaf);
  endScrollAnim();
  cancelRideSettle();
}

export function markNavigationScroll(el: HTMLElement, top: number) {
  writeLedgerScroll(el, top, 'placement');
}

export function markAnchorScroll(el: HTMLElement, top: number): void {
  const before = el.scrollTop;
  writeLedgerScroll(el, top, 'anchor');
  if (_shiftTweenStart && el === resolveTarget()) _shiftTweenStart(el.scrollTop - before);
}

export function markRevealScroll(el?: HTMLElement | null): void {
  const target = el ?? resolveTarget();
  if (!target) return;
  markNavigationScroll(target, target.scrollTop);
}

function isNavigationScroll(el?: HTMLElement | null): boolean {
  if (_scrollAnimRaf !== null) return true;
  return wroteWithinEventWindow(el);
}

function isPlacementScroll(el: HTMLElement, navigation: boolean): boolean {
  const kind = lastWriteKind(el);
  if (kind === 'held' || kind === 'carry') return false;
  if (kind === 'anchor' && rideGlideInFlight()) return false;
  return navigation;
}

function isAnchorScroll(el: HTMLElement): boolean {
  return lastWriteKind(el) === 'anchor' && isNavigationScroll(el);
}

export function isOtherNavigationScroll(el: HTMLElement): boolean {
  if (_scrollAnimRaf !== null) return true;
  return lastWriteKind(el) !== 'anchor' && isNavigationScroll(el);
}

type FollowState = 'off' | 'riding' | 'parked';
const _follow = signal<FollowState>('off');

export const followingLiveEdge: ReadonlySignal<boolean> = computed(() => _follow.value !== 'off');

export const showScrollToBottom: ReadonlySignal<boolean> = computed(
  () => awayFromBottom.value && !_pendingLanding.value?.on && !_rideGliding.value,
);

let _transcriptLive = false;

export function setTranscriptLive(live: boolean): void {
  const waking = live && !_transcriptLive;
  _transcriptLive = live;
  if (!waking) return;
  const el = resolveTarget();
  if (el) wakeParkedRide(el);
}

function wakeParkedRide(el: HTMLElement): boolean {
  if (_follow.value !== 'parked' || !_transcriptLive) return false;
  if (hasPendingEventScroll()) return false;
  unparkTheRide(el);
  rideToLiveEdge(el);
  return true;
}

function unparkTheRide(el: HTMLElement): void {
  if (_follow.value !== 'parked') return;
  _follow.value = 'riding';
  holdPosition(el);
  _rideShortReported = false;
  for (const listener of _followRideListeners) listener();
}

const FOLLOW_SEED_KEY = 'lucidos-follow-live-edge';

export function followSeedFromStored(raw: string | null): boolean {
  return raw !== 'false';
}

function readFollowSeed(): boolean {
  if (typeof localStorage === 'undefined') return followSeedFromStored(null);
  try {
    return followSeedFromStored(localStorage.getItem(FOLLOW_SEED_KEY));
  } catch {
    return followSeedFromStored(null);
  }
}

const _followSeed = signal(readFollowSeed());

export const followLiveEdgeSeed: ReadonlySignal<boolean> = _followSeed;

function recordFollowSeed(on: boolean): void {
  _followSeed.value = on;
  if (typeof localStorage === 'undefined') return;
  try {
    localStorage.setItem(FOLLOW_SEED_KEY, String(on));
  } catch {  }
}

export function applyFollowSeed(el: HTMLElement, from: FollowResumeFrom = 'live-edge'): boolean {
  if (!_followSeed.value) return false;
  resumeFollowingBottom(el, from);
  return true;
}

const _followRideListeners = new Set<() => void>();

export function onFollowRideStarted(listener: () => void): () => void {
  _followRideListeners.add(listener);
  return () => { _followRideListeners.delete(listener); };
}

type ResolvedTurn = { turn: HTMLElement; drawnAtSubmit: number };

type TurnResolver = (el: HTMLElement) => ResolvedTurn | null;

const _pendingLanding = signal<{
  resolveTurn: TurnResolver;
  holds: boolean;
  aim?: LandingAim;
  on: ResolvedTurn | null;
  at: number;
} | null>(null);

type LandingAim = (el: HTMLElement, target: HTMLElement) => void;

let _rideArrival: { resolveTurn: TurnResolver; at: number } | null = null;

let _cardAwaitingConfirm: { cardId: string; resolveTurn: TurnResolver; at: number } | null = null;

let _nextCardWatch: { seen: string | null; at: number } | null = null;

const LANDING_ADDRESSABLE_MS = 1000;
const LANDING_ROUND_TRIP_MS = 8000;
const LANDING_AGENT_START_MS = 60_000;

export const DRAWN_ROW_SELECTOR = '.response-content > :not(.disclosure), '
  + '.response-content > .disclosure > .disclosure-body > :not(.response-elision):not([data-thinking-row])';

function drawnRows(turn: HTMLElement): ArrayLike<HTMLElement> {
  if (typeof turn.querySelectorAll !== 'function') return [];
  return turn.querySelectorAll<HTMLElement>(DRAWN_ROW_SELECTOR);
}

function releaseRestTop(el: HTMLElement, row: HTMLElement | undefined): number {
  const edge = liveEdgeTop(el);
  const lineBottom = row ? firstLineBottom(row) : null;
  if (lineBottom === null) return edge;
  const lineBottomTop = lineBottom - el.getBoundingClientRect().top + el.scrollTop;
  return Math.min(edge, Math.max(el.scrollTop, lineBottomTop + bottomPadding(el) - el.clientHeight));
}

function firstLineBottom(row: HTMLElement): number | null {
  if (typeof row.getBoundingClientRect !== 'function') return null;
  return firstCharacterRect(row)?.bottom ?? row.getBoundingClientRect().bottom;
}

function firstCharacterRect(row: HTMLElement): DOMRect | null {
  if (typeof document === 'undefined' || !(row instanceof Node)) return null;
  const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    const at = node.textContent?.search(/\S/) ?? -1;
    if (at < 0) continue;
    const range = document.createRange();
    range.setStart(node, at);
    range.setEnd(node, at + 1);
    const rect = range.getClientRects()[0];
    if (rect) return rect;
  }
  return null;
}

function bottomPadding(el: HTMLElement): number {
  return typeof getComputedStyle === 'function' ? (parseFloat(getComputedStyle(el).paddingBottom) || 0) : 0;
}

function turnIsQueued(turn: HTMLElement): boolean {
  return typeof turn.querySelector === 'function'
    && turn.querySelector('.exchange-status-queued') !== null;
}

function liveEdgeTop(el: HTMLElement): number {
  return Math.max(0, el.scrollHeight - el.clientHeight);
}

function clampToScrollRange(el: HTMLElement, top: number): number {
  return Math.max(0, Math.min(top, liveEdgeTop(el)));
}

const RESTING_SLACK_PX = 2;

function isLiveEdgeTop(el: HTMLElement, top: number): boolean {
  return top >= liveEdgeTop(el) - RESTING_SLACK_PX;
}

function isAtLiveEdge(el: HTMLElement): boolean {
  return isLiveEdgeTop(el, el.scrollTop);
}

function isScrollable(el: HTMLElement): boolean {
  return transcriptScrolls(el);
}


function markHeldScroll(el: HTMLElement, top: number) {
  writeHeldScroll(el, top, 'held');
}

function writeHeldScroll(el: HTMLElement, top: number, kind: 'held' | 'carry') {
  writeLedgerScroll(el, top, kind);
  holdPosition(el);
  rideSettlesOnTheEdge();
}

function markCarryScroll(el: HTMLElement, top: number) {
  writeHeldScroll(el, top, 'carry');
}

function armFollowOn(el: HTMLElement | null) {
  const wasArmed = _follow.value !== 'off';
  _follow.value = 'riding';
  holdPosition(el);
  _rideShortReported = false;
  forgetSubmits();
  if (!wasArmed) for (const listener of _followRideListeners) listener();
}

export type FollowResumeFrom = 'live-edge' | 'in-place';

export function resumeFollowingBottom(el: HTMLElement, from: FollowResumeFrom = 'live-edge'): void {
  if (from === 'in-place') {
    if (_follow.value !== 'off') return;
    if (deepLinkLandedOffLiveEdge()) return;
    if (deepLinkHasResolved()) {
      armFollowOn(el);
      rideToLiveEdge(el);
      return;
    }
    armFollowOn(isAtLiveEdge(el) ? el : null);
    return;
  }
  armFollowOn(el);
  markHeldScroll(el, liveEdgeTop(el));
}

export interface ScrollClass {
  navigation: boolean;
  kind: NavScrollKind | null;
  page: { navigation: boolean; kind: NavScrollKind | null };
  onLastWrite: boolean;
  onHeld: boolean;
  atEdge: boolean;
  gesture: boolean;
  placement: boolean;
  followSurvives: boolean;
}

export function classifyScroll(el?: HTMLElement | null): ScrollClass {
  const pageNavigation = isNavigationScroll();
  const page = { navigation: pageNavigation, kind: pageNavigation ? lastWriteKind() : null };
  if (!el) {
    return {
      ...page, page, onLastWrite: false, onHeld: false, atEdge: false,
      gesture: false, placement: false, followSurvives: false,
    };
  }
  const navigation = isNavigationScroll(el);
  const kind = navigation ? lastWriteKind(el) : null;
  const atEdge = isAtLiveEdge(el);
  const onHeld = isWhereWeHeldIt(el);
  const gesture = readerGestureActive(el);
  const placement = isPlacementScroll(el, navigation);
  const followSurvives = _follow.value === 'riding'
    && (onHeld || !scrollLeavesTheRide(atEdge, true, gesture, placement));
  return {
    navigation, kind, page, onLastWrite: isWhereWeLastScrolledIt(el), onHeld, atEdge,
    gesture, placement, followSurvives,
  };
}

function scrollLeavesTheRide(atEdge: boolean, tookOver: boolean, gesture: boolean, placement: boolean): boolean {
  return !atEdge && tookOver && (gesture || placement);
}

function leaveTheRideByScroll(): void {
  if (_transcriptLive) stopFollowingBottom();
  else parkTheRide();
}

export function stopFollowingBottom() {
  leaveTheRide('off');
}

function parkTheRide() {
  leaveTheRide('parked');
}

function leaveTheRide(next: 'off' | 'parked') {
  cancelRideSettle();
  if (_heldAnim) cancelScrollAnim();
  _follow.value = next;
  holdPosition(null);
  forgetSubmits();
}

function forgetSubmits() {
  _pendingLanding.value = null;
  _rideArrival = null;
  _cardAwaitingConfirm = null;
  _nextCardWatch = null;
}

function cancelLanding() {
  if (landingGlideInFlight()) cancelScrollAnim();
  forgetSubmits();
}

function landingGlideInFlight(): boolean {
  return _heldAnim && _heldAnimTarget === 'landing';
}

function dropLapsedLanding(): void {
  const landing = _pendingLanding.value;
  if (!landing) return;
  const deadline = landing.on ? LANDING_AGENT_START_MS
    : landing.holds ? LANDING_ADDRESSABLE_MS : LANDING_ROUND_TRIP_MS;
  if (nowMs() - landing.at >= deadline) _pendingLanding.value = null;
}

function landingInFlight(): boolean {
  return _cardAwaitingConfirm !== null || _pendingLanding.value !== null
    || landingGlideInFlight() || _nextCardWatch !== null;
}

function carryHeldScroll(el: HTMLElement): void {
  if (holdsReaderOn(el)) holdPosition(el);
}

export function holdAcrossRelayout(el: HTMLElement, top: number): void {
  markAnchorScroll(el, top);
  carryHeldScroll(el);
}

export function honourAnchoredMutation(el: HTMLElement): void {
  _arrivalHold = null;
  carryHeldScroll(el);
  if (_follow.value === 'riding' && !isAtLiveEdge(el) && !rideGlideInFlight()) stopFollowingBottom();
}

function rideGlideInFlight(): boolean {
  return _heldAnim && _heldAnimTarget === 'ride';
}


export { isWhereWeLastScrolledIt, onRebasedScroll };
export type { NavScrollKind };

export {
  TOUCH_COAST_IDLE_MS,
  TOUCH_COAST_MAX_MS,
  isScrollHeld,
  onScrollHoldReleased,
  readerGestureForTest,
  readerGestureSince,
  scrollHoldReleased,
};

function liesInFullView(node: Node | null, container: HTMLElement): boolean {
  const box = node as HTMLElement | null;
  if (typeof box?.getBoundingClientRect !== 'function') return false;
  const r = box.getBoundingClientRect();
  const c = container.getBoundingClientRect();
  return r.height > 0 && r.top >= c.top && r.bottom <= c.bottom && r.left >= c.left && r.right <= c.right;
}

function attachReaderGestures(el: HTMLElement): () => void {
  if (typeof el.addEventListener !== 'function') return () => {};
  const detachInput = attachReaderInput(el);
  const onDescendantKey = (e: KeyboardEvent) => {
    if (e.target === el || !isScrollKeyPress(e)) return;
    _arrivalHold = null;
    markRevealScroll(el);
  };
  const onFocusIn = (e: FocusEvent) => {
    const to = e.target as Node | null;
    if (_arrivalHold && to !== el && !_arrivalHold.target.contains(to)) _arrivalHold = null;
    if (to === el || liesInFullView(to, el)) return;
    markRevealScroll(el);
  };

  el.addEventListener('keydown', onDescendantKey);
  el.addEventListener('focusin', onFocusIn, { passive: true });
  return () => {
    el.removeEventListener('keydown', onDescendantKey);
    el.removeEventListener('focusin', onFocusIn);
    detachInput();
  };
}

function lastUserMessage(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const panels = el.querySelectorAll<HTMLElement>('.initiator-panel-user');
  const last = panels[panels.length - 1];
  return last && isElementVisible(last) ? last : null;
}

function lastUserTurn(el: HTMLElement): HTMLElement | null {
  const panel = lastUserMessage(el);
  return (panel?.closest?.(TURN_SELECTOR) as HTMLElement | null) ?? null;
}

function lastTurn(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const turns = el.querySelectorAll<HTMLElement>(TURN_SELECTOR);
  const last = turns[turns.length - 1];
  return last && isElementVisible(last) ? last : null;
}

function cardTurn(el: HTMLElement, bodySelector: string, attr: string, value: string): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  for (const body of el.querySelectorAll<HTMLElement>(bodySelector)) {
    if (body.getAttribute?.(attr) !== value) continue;
    if (isElementVisible(body)) return (body.closest?.(TURN_SELECTOR) as HTMLElement | null) ?? null;
  }
  return null;
}

const DEAD_QUESTION_CLASSES = [
  'question-body-answered',
  'question-body-terminated',
  'permission-body-answered',
  'permission-body-terminated',
];

const QUESTION_CARD = '.question-body, .permission-body[data-tool-use-id]';

function answerableQuestionId(el: HTMLElement): string | null {
  return answerableQuestionBody(el)?.getAttribute?.('data-tool-use-id') ?? null;
}

function answerableQuestionBody(el: HTMLElement): HTMLElement | null {
  if (typeof el.querySelectorAll !== 'function') return null;
  const bodies = el.querySelectorAll<HTMLElement>(QUESTION_CARD);
  const newest = bodies[bodies.length - 1];
  if (!newest) return null;
  if (DEAD_QUESTION_CLASSES.some((c) => newest.classList?.contains(c))) return null;
  return newest;
}

function asksAgain(el: HTMLElement, turn: HTMLElement): boolean {
  const body = answerableQuestionBody(el);
  return body !== null && body.closest?.(TURN_SELECTOR) !== turn;
}

const TURN_LANDING_FALLBACK_GAP_PX = 8;

function turnLandingClearancePx(turn: HTMLElement): number {
  if (typeof getComputedStyle !== 'function') return TURN_LANDING_FALLBACK_GAP_PX;
  const px = parseFloat(getComputedStyle(turn).scrollMarginTop);
  return Number.isFinite(px) && px > 0 ? px : TURN_LANDING_FALLBACK_GAP_PX;
}


function snapToLiveEdge(el: HTMLElement, frozenTop = liveEdgeTop(el)): void {
  cancelScrollAnim();
  markHeldScroll(el, frozenTop);
  syncAwayFromBottom();
}

function glideToLiveEdge(el: HTMLElement, owner: HeldGlide, freezeAt?: number, ridePace = NAVIGATION_PACE): void {
  if (_heldAnim && _heldAnimTarget === owner) {
    if (freezeAt !== undefined) freezeLandingGlide(freezeAt);
    return;
  }
  if (isReducedMotion()) {
    snapToLiveEdge(el, freezeAt);
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
  }, markHeldScroll, owner === 'landing' ? SEND_PACE : ridePace);
  _heldAnimTarget = owner;
  _rideGliding.value = owner === 'ride';
  if (freezeAt !== undefined) freezeLandingGlide(freezeAt);
}

function freezeLandingGlide(top: number): void {
  if (!landingGlideInFlight() || _heldGlideFrozenTop !== null) return;
  _heldGlideFrozenTop = top;
  _continueTweenTo?.(top);
}

function rideToLiveEdge(el: HTMLElement): void {
  if (!isAtLiveEdge(el)) { glideToLiveEdge(el, 'ride'); return; }
  cancelScrollAnim();
  syncAwayFromBottom();
}

function followSubmit(resolveTurn: TurnResolver, holds = true, aim?: LandingAim): void {
  _arrivalHold = null;
  retireReaderMovement();
  _cardAwaitingConfirm = null;
  _nextCardWatch = null;
  const el = resolveTarget();
  if (!el) return;
  if (_follow.value !== 'off') {
    unparkTheRide(el);
    _rideArrival = resolveTurn(el) ? null : { resolveTurn, at: nowMs() };
    if (!isAtLiveEdge(el)) glideToLiveEdge(el, 'ride', undefined, SEND_PACE);
    return;
  }
  dropLapsedLanding();
  if (_pendingLanding.value?.holds && !_pendingLanding.value.on) return;
  holdPosition(el);
  _pendingLanding.value = { resolveTurn, holds, aim, on: null, at: nowMs() };
  honourLanding(el);
}

function awaitsNewTurn(newest: (el: HTMLElement) => HTMLElement | null): TurnResolver {
  const el = resolveTarget();
  const before = el ? newest(el) : null;
  return (c) => {
    const now = newest(c);
    return now && now !== before ? { turn: now, drawnAtSubmit: 0 } : null;
  };
}

function landsOnCard(bodySelector: string, attr: string, value: string): TurnResolver {
  return (el) => {
    const turn = cardTurn(el, bodySelector, attr, value);
    return turn ? { turn, drawnAtSubmit: drawnRows(turn).length } : null;
  };
}

export function followSentMessage(): void {
  const el = resolveTarget();
  const answering = el ? answerableQuestionId(el) : null;
  followSubmit(answering ? landsOnQuestionCard(answering) : awaitsNewTurn(lastUserTurn));
}

export function followAnsweredQuestion(toolUseId: string): void {
  followCardOnceConfirmed(toolUseId, landsOnQuestionCard(toolUseId));
}

function landsOnQuestionCard(toolUseId: string): TurnResolver {
  return landsOnCard(QUESTION_CARD, 'data-tool-use-id', toolUseId);
}

export function followResolvedPermission(requestId: string): void {
  followCardOnceConfirmed(requestId, landsOnCard('.permission-body', 'data-request-id', requestId));
}

function followCardOnceConfirmed(cardId: string, resolveTurn: TurnResolver): void {
  retireReaderMovement();
  cancelLanding();
  _cardAwaitingConfirm = { cardId, resolveTurn, at: nowMs() };
  holdPosition(resolveTarget());
}

export function followConfirmedCard(cardId: string): void {
  const awaiting = _cardAwaitingConfirm;
  if (awaiting?.cardId !== cardId) return;
  _cardAwaitingConfirm = null;
  if (nowMs() - awaiting.at >= LANDING_ROUND_TRIP_MS) return;
  followSubmit(awaiting.resolveTurn);
}

export function followContinuedThread(): void {
  followSubmit(awaitsNewTurn(lastTurn));
}

export function followCanceledTurn(toolUseId?: string): void {
  followSubmit(toolUseId ? landsOnQuestionCard(toolUseId) : awaitsNewTurn(lastTurn), false);
}

export function followSideQuestion(): void {
  const el = resolveTarget();
  const before = new Set(el ? sideQuestionCards(el).map((card) => card.getAttribute('data-side-question-id')) : []);
  followSubmit((c) => {
    const card = sideQuestionCards(c).find((node) =>
      !before.has(node.getAttribute('data-side-question-id')) && isElementVisible(node));
    return card ? { turn: card, drawnAtSubmit: 0 } : null;
  }, false, landOnStart);
}

function sideQuestionCards(el: HTMLElement): HTMLElement[] {
  if (typeof el.querySelectorAll !== 'function') return [];
  return Array.from(el.querySelectorAll<HTMLElement>('[data-side-question-id]'));
}

function landOnStart(el: HTMLElement, target: HTMLElement): void {
  const onLine = landingTargetOf(target);
  const targetOf = (c: HTMLElement) => Math.min(liveEdgeTop(c), onLine(c));
  if (targetOf(el) <= el.scrollTop + 1) return;
  if (isReducedMotion()) {
    cancelScrollAnim();
    markNavigationScroll(el, targetOf(el));
    syncAwayFromBottom();
    return;
  }
  animateScroll(targetOf, syncAwayFromBottom);
}

function landAtLiveEdge(el: HTMLElement, freezeAt?: number): void {
  const resting = freezeAt === undefined ? isAtLiveEdge(el) : el.scrollTop >= freezeAt - RESTING_SLACK_PX;
  if (resting) {
    if (freezeAt !== undefined) freezeLandingGlide(freezeAt);
    return;
  }
  glideToLiveEdge(el, 'landing', freezeAt);
}

function honourLanding(el: HTMLElement): void {
  dropLapsedLanding();
  const landing = _pendingLanding.value;
  if (!landing) return;
  let on = landing.on;
  if (!on) {
    const found = landing.resolveTurn(el);
    if (!found) return;
    if (!landing.holds) {
      _pendingLanding.value = null;
      if (landing.aim) landing.aim(el, found.turn);
      else landAtLiveEdge(el, liveEdgeTop(el));
      return;
    }
    on = found;
    _pendingLanding.value = { ...landing, on };
  }
  const { turn, drawnAtSubmit } = on;
  if (turn.isConnected === false) { _pendingLanding.value = null; return; }
  const rows = drawnRows(turn);
  const asks = asksAgain(el, turn);
  const ending = turnIsQueued(turn) || rows.length !== drawnAtSubmit || asks;
  if (!ending) { landAtLiveEdge(el); return; }
  landAtLiveEdge(el, releaseRestTop(el, asks ? undefined : rows[drawnAtSubmit]));
  _pendingLanding.value = null;
  _nextCardWatch = { seen: answerableQuestionId(el), at: nowMs() };
}

function landOnNextCard(el: HTMLElement): boolean {
  const watch = _nextCardWatch;
  if (!watch) return false;
  if (nowMs() - watch.at >= LANDING_ADDRESSABLE_MS) { _nextCardWatch = null; return false; }
  const card = answerableQuestionId(el);
  if (card === null || card === watch.seen) return false;
  _nextCardWatch = null;
  if (landingGlideInFlight()) {
    _heldGlideFrozenTop = liveEdgeTop(el);
    _continueTweenTo?.(_heldGlideFrozenTop);
  } else {
    landAtLiveEdge(el, liveEdgeTop(el));
  }
  return true;
}

function keepTheLiveEdge(el: HTMLElement): boolean {
  if (_follow.value !== 'riding') return false;
  if (_scrollAnimRaf !== null || hasPendingEventScroll()) return false;
  if (isAnchorScroll(el)) return false;
  const owed = readerScrollOwed(el);
  if (owed > 0) {
    retryTheLiveEdgeAfter(el, owed);
    return false;
  }
  markCarryScroll(el, liveEdgeTop(el));
  return true;
}

let _owedScrollRetry: ReturnType<typeof setTimeout> | null = null;

function retryTheLiveEdgeAfter(el: HTMLElement, ms: number) {
  if (_owedScrollRetry !== null) clearTimeout(_owedScrollRetry);
  _owedScrollRetry = setTimeout(() => {
    _owedScrollRetry = null;
    if (el.isConnected !== false) keepTheLiveEdge(el);
  }, ms);
}

function honourGrowth(el: HTMLElement): void {
  if (_pendingLanding.value) {
    if (_scrollAnimRaf !== null && !landingGlideInFlight()) return;
    honourLanding(el);
    return;
  }
  if ((_scrollAnimRaf === null || landingGlideInFlight()) && landOnNextCard(el)) return;
  if (holdTheArrival(el)) return;
  if (wakeParkedRide(el)) return;
  if (glideToSubmittedTurn(el)) return;
  keepTheLiveEdge(el);
}

function glideToSubmittedTurn(el: HTMLElement): boolean {
  const arrival = _rideArrival;
  if (!arrival) return false;
  if (_follow.value !== 'riding' || nowMs() - arrival.at >= LANDING_ADDRESSABLE_MS) {
    _rideArrival = null;
    return false;
  }
  if (!arrival.resolveTurn(el)) return false;
  _rideArrival = null;
  if (_scrollAnimRaf !== null || isAtLiveEdge(el)) return false;
  glideToLiveEdge(el, 'ride', undefined, SEND_PACE);
  return true;
}

let _arrivalHold: {
  target: HTMLElement;
  relTop: number | null;
  fallback: HTMLElement | null;
  fallbackRelTop: number | null;
  quietUntil: number;
  endsBy: number;
  imagesLoading: boolean;
  onSettled?: () => void;
} | null = null;

function viewportTopOf(target: HTMLElement, c: HTMLElement): number {
  return target.getBoundingClientRect().top - c.getBoundingClientRect().top;
}

function imageAboveStillLoading(target: HTMLElement, c: HTMLElement): boolean {
  for (const img of c.querySelectorAll?.('img') ?? []) {
    if (img.loading === 'lazy' || (img.complete && !isImageRetryPending(img))) continue;
    if (img.compareDocumentPosition(target) & img.DOCUMENT_POSITION_FOLLOWING) return true;
  }
  return false;
}

function armArrivalHold(target: HTMLElement, fallback: HTMLElement | null = null): void {
  const now = nowMs();
  _arrivalHold = {
    target,
    relTop: null,
    fallback,
    fallbackRelTop: null,
    quietUntil: now + ARRIVAL_HOLD_QUIET_MS,
    endsBy: now + ARRIVAL_HOLD_MAX_MS,
    imagesLoading: false,
  };
}
function settleArrivalHold(target: HTMLElement, c: HTMLElement | null = resolveTarget()): void {
  if (!c || _arrivalHold?.target !== target) return;
  _arrivalHold.relTop = viewportTopOf(target, c);
  _arrivalHold.fallbackRelTop = _arrivalHold.fallback ? viewportTopOf(_arrivalHold.fallback, c) : null;
  _arrivalHold.quietUntil = nowMs() + ARRIVAL_HOLD_QUIET_MS;
  _arrivalHold.imagesLoading = imageAboveStillLoading(target, c);
}

export function holdRestoredPlace(c: HTMLElement, target: HTMLElement, onSettled?: () => void, fallback: HTMLElement | null = null): void {
  armArrivalHold(target, fallback === target ? null : fallback);
  if (_arrivalHold) _arrivalHold.onSettled = onSettled;
  settleArrivalHold(target, c);
}

function fallBackIfReplaced(hold: NonNullable<typeof _arrivalHold>): void {
  const { fallback, fallbackRelTop } = hold;
  if (hold.target.isConnected !== false || !fallback || fallback.isConnected === false || fallbackRelTop === null) return;
  hold.target = fallback;
  hold.relTop = fallbackRelTop;
  hold.fallback = null;
}

function holdTheArrival(el: HTMLElement): boolean {
  const hold = _arrivalHold;
  if (!hold) return false;
  const now = nowMs();
  fallBackIfReplaced(hold);
  const taken = hold.target.isConnected === false || _follow.value === 'riding';
  if (taken) {
    _arrivalHold = null;
    return false;
  }
  if (now <= hold.endsBy && _scrollAnimRaf === null && hold.relTop !== null) {
    const shift = viewportTopOf(hold.target, el) - hold.relTop;
    if (Math.abs(shift) > 1) {
      const before = el.scrollTop;
      markAnchorScroll(el, before + shift);
      if (el.scrollTop !== before) hold.quietUntil = now + ARRIVAL_HOLD_QUIET_MS;
    }
    if (hold.fallback) hold.fallbackRelTop = viewportTopOf(hold.fallback, el);
  }
  const imagesLoading = imageAboveStillLoading(hold.target, el);
  const settled = now >= hold.quietUntil && !hold.imagesLoading && !imagesLoading;
  if (settled || now >= hold.endsBy) {
    _arrivalHold = null;
    hold.onSettled?.();
    return false;
  }
  hold.imagesLoading = imagesLoading;
  return true;
}

let _rideSettleRaf: number | null = null;

let _rideShortReported = false;

function cancelRideSettle(): void {
  if (_rideSettleRaf !== null) cancelAnimationFrame(_rideSettleRaf);
  _rideSettleRaf = null;
}

function rideSettlesOnTheEdge(): void {
  if (typeof requestAnimationFrame !== 'function') return;
  cancelRideSettle();
  _rideSettleRaf = requestAnimationFrame(settleTheRide);
}

function settleTheRide(): void {
  _rideSettleRaf = null;
  const el = resolveTarget();
  if (!el || isAtLiveEdge(el)) return;
  if (_follow.value !== 'riding') return;
  if (_scrollAnimRaf !== null || _pendingLanding.value || hasPendingEventScroll()) return;
  if (!isWhereWeHeldIt(el)) return;
  const edge = liveEdgeTop(el);
  const short = Math.round(edge - el.scrollTop);
  markCarryScroll(el, edge);
  cancelRideSettle();
  syncAwayFromBottom();
  reportRideLandedShort(short, edge, el.clientHeight);
}

/** Say ONCE per ride that a write came to rest short, and by how much.
 *
 *  A diagnostic, registered in `docs/temporary-measures.md`. The strand it
 *  chases has been reported three times, and its trigger is still not measured
 *  on the device it happens on. The correction above hides it from the reader.
 *  Without this line a fourth round would start from nothing again.
 *
 *  Best-effort telemetry (`.claude/rules/frontend.md`). It runs without user
 *  intent, the reader is already served by the write above, and
 *  `postClientLog` is fire-and-forget. Three numbers and no user content. */
function reportRideLandedShort(short: number, edge: number, view: number): void {
  if (_rideShortReported) return;
  _rideShortReported = true;
  postClientLog('follow', 'short', { short, edge, view });
}

function animateScroll(
  targetOf: (el: HTMLElement) => number,
  onDone?: () => void,
  mark: (el: HTMLElement, top: number) => void = markNavigationScroll,
  pace: ScrollPace = NAVIGATION_PACE,
) {
  cancelScrollAnim();
  const endedBefore = _endedTweens;
  _heldAnim = mark === markHeldScroll;
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

function landingTargetOf(el: HTMLElement): (c: HTMLElement) => number {
  const marginTop =
    typeof getComputedStyle === 'function'
      ? (parseFloat(getComputedStyle(el).scrollMarginTop) || 0)
      : 0;
  return (c: HTMLElement) =>
    typeof c.getBoundingClientRect === 'function' && typeof el.getBoundingClientRect === 'function'
      ? el.getBoundingClientRect().top - c.getBoundingClientRect().top + c.scrollTop - marginTop
      : 0;
}

function smoothScrollToElement(el: HTMLElement, onDone?: () => void): void {
  const targetOf = landingTargetOf(el);
  if (isReducedMotion()) {
    cancelScrollAnim();
    const c = resolveTarget();
    if (c) markNavigationScroll(c, Math.max(0, targetOf(c)));
    onDone?.();
    return;
  }
  animateScroll(targetOf, onDone);
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

export function setFollowLiveEdge(on: boolean): void {
  recordFollowSeed(on);
  if (!on) { stopFollowingBottom(); return; }
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el) return;
  armFollowOn(el);
  rideToLiveEdge(el);
}

export function scrollToBottom() {
  clearPendingEventScroll();
  cancelScrollAnim();

  const target = resolveTarget();
  if (target) markNavigationScroll(target, target.scrollHeight);
  syncAwayFromBottom();
}

function syncAwayFromBottom() {
  const el = resolveTarget();
  if (!el) return;
  awayFromBottom.value = !isAtLiveEdge(el);
}

export { EVENT_RESOLVE_DEADLINE_MS };
export const EVENT_RESOLVE_MAX_WAIT_MS = 20000;
const ARRIVAL_HOLD_QUIET_MS = EVENT_RESOLVE_DEADLINE_MS;
const ARRIVAL_HOLD_MAX_MS = EVENT_RESOLVE_MAX_WAIT_MS;
const SCROLL_SETTLE_FALLBACK_MS = 1000;

let _pendingEventScrollClaim: object | null = null;

export function hasPendingEventScroll(): boolean {
  return _pendingEventScrollClaim !== null;
}

const _deepLinkClaimListeners = new Set<() => void>();

export function onDeepLinkClaimed(listener: () => void): () => void {
  _deepLinkClaimListeners.add(listener);
  return () => { _deepLinkClaimListeners.delete(listener); };
}

const _deepLinkResolvedListeners = new Set<() => void>();

export function onDeepLinkResolved(listener: () => void): () => void {
  _deepLinkResolvedListeners.add(listener);
  return () => { _deepLinkResolvedListeners.delete(listener); };
}

let _pendingEventScrollResolved = false;

export function deepLinkHasResolved(): boolean {
  return _pendingEventScrollClaim !== null && _pendingEventScrollResolved;
}

let _pendingEventScrollLandedOffEdge = false;

function deepLinkLandedOffLiveEdge(): boolean {
  return _pendingEventScrollLandedOffEdge;
}

let _headerPinnedForScroll = false;
let _headerPinTimer: ReturnType<typeof setTimeout> | null = null;
const HEADER_PIN_MS = 800;

export function isHeaderPinnedForScroll(): boolean {
  return _headerPinnedForScroll;
}

function pinHeaderForScroll(): void {
  _headerPinnedForScroll = true;
  if (_headerPinTimer) clearTimeout(_headerPinTimer);
  _headerPinTimer = setTimeout(() => {
    _headerPinnedForScroll = false;
    _headerPinTimer = null;
  }, HEADER_PIN_MS);
}

function scrollToSelectorAndPulse(
  linkTarget: DeepLinkTarget,
  selector: () => string,
  addressedBy: string,
  preferLast = false,
  pulseTarget?: string | ((target: HTMLElement) => HTMLElement | null),
  opts?: DeepLinkOptions,
): void {
  if (typeof document === 'undefined' || !document.querySelectorAll) return;

  let resolved = false;
  let deadlineTimer: ReturnType<typeof setTimeout> | null = null;
  let observer: MutationObserver | null = null;
  let boxObserver: ResizeObserver | null = null;
  const watchedBoxes = new Set<Element>();
  const claim = {};
  const startedAt = nowMs();
  let reported = false;
  let pastFirstLook = false;

  _pendingEventScrollClaim = claim;
  _pendingEventScrollResolved = false;
  _pendingEventScrollLandedOffEdge = false;
  for (const listener of _deepLinkClaimListeners) listener();
  deepLinkTarget.value = linkTarget;
  _arrivalHold = null;

  const releaseClaim = () => {
    if (_pendingEventScrollClaim === claim) {
      _pendingEventScrollClaim = null;
      _pendingEventScrollResolved = false;
      _pendingEventScrollLandedOffEdge = false;
      deepLinkTarget.value = null;
    }
  };

  const stopObserving = () => {
    if (observer) {
      observer.disconnect();
      observer = null;
    }
    if (boxObserver) {
      boxObserver.disconnect();
      boxObserver = null;
      watchedBoxes.clear();
    }
  };

  const stopWatching = () => {
    stopObserving();
    if (deadlineTimer !== null) {
      clearTimeout(deadlineTimer);
      deadlineTimer = null;
    }
  };

  const reportOutcome = (
    outcome: 'landed' | 'landed-after-wait' | 'superseded' | 'unresolved',
    matches: ArrayLike<HTMLElement>,
    landing?: { container: HTMLElement | null; fromTop: number; toTop: number },
  ) => {
    if (reported) return;
    reported = true;
    if (outcome === 'landed' || outcome === 'landed-after-wait') markBoot('landed');
    let visible = 0;
    for (let i = 0; i < matches.length; i++) if (isElementVisible(matches[i])) visible++;
    postClientLog('deeplink', 'outcome', {
      outcome,
      addressed_by: addressedBy,
      waited_ms: Math.round(nowMs() - startedAt),
      dom_matches: matches.length,
      visible_matches: visible,
      had_container: landing ? !!landing.container : !!resolveTarget(),
      from_top: landing ? Math.round(landing.fromTop) : null,
      to_top: landing ? Math.round(landing.toTop) : null,
    });
  };

  const watchForABox = (matches: ArrayLike<HTMLElement>) => {
    if (typeof ResizeObserver !== 'function') return;
    for (let i = 0; i < matches.length; i++) {
      const blocked = unmeasurableBox(matches[i]);
      if (!blocked || watchedBoxes.has(blocked)) continue;
      boxObserver ??= new ResizeObserver(() => tryResolve());
      watchedBoxes.add(blocked);
      boxObserver.observe(blocked);
    }
  };

  const holdClaimUntilScrollSettles = () => {
    const container = resolveTarget();
    let settleTimer: ReturnType<typeof setTimeout> | null = null;
    const finish = () => {
      if (settleTimer !== null) {
        clearTimeout(settleTimer);
        settleTimer = null;
      }
      container?.removeEventListener?.('scrollend', finish);
      releaseClaim();
    };
    container?.addEventListener?.('scrollend', finish, { once: true });
    settleTimer = setTimeout(finish, SCROLL_SETTLE_FALLBACK_MS);
  };

  let lookedFor = '';
  const tryResolve = () => {
    if (resolved) return;
    lookedFor = selector();
    const matches = document.querySelectorAll<HTMLElement>(lookedFor);
    let target: HTMLElement | null = null;
    for (const el of matches) {
      if (isElementVisible(el)) {
        target = el;
        if (!preferLast) break;
      }
    }
    if (!target) {
      watchForABox(matches);
      return;
    }
    resolved = true;
    stopObserving();

    pinHeaderForScroll();
    if (typeof document !== 'undefined' && document.dispatchEvent) {
      document.dispatchEvent(new Event('reveal-mobile-bars'));
    }
    const container = resolveTarget();
    const landingTop = container ? landingTargetOf(target)(container) : 0;
    const landsOnTheEdge = !!container && isLiveEdgeTop(container, landingTop);
    reportOutcome(pastFirstLook ? 'landed-after-wait' : 'landed', matches, {
      container,
      fromTop: container ? container.scrollTop : 0,
      toTop: container ? clampToScrollRange(container, landingTop) : 0,
    });
    if (container && landsOnTheEdge && _follow.value !== 'off') {
      unparkTheRide(container);
      rideToLiveEdge(container);
    } else {
      stopFollowingBottom();
      armArrivalHold(target);
      smoothScrollToElement(target, () => settleArrivalHold(target));
    }
    _pendingEventScrollLandedOffEdge = !landsOnTheEdge;
    if (_pendingEventScrollClaim === claim) {
      _pendingEventScrollResolved = true;
      for (const listener of _deepLinkResolvedListeners) listener();
    }
    const pulseChild =
      typeof pulseTarget === 'string'
        ? (target.querySelector?.(pulseTarget) as HTMLElement | null)
        : pulseTarget?.(target) ?? null;
    const pulseEl = pulseTarget ? pulseChild ?? target : target;
    applyNavFocus(pulseEl, { settleGuard: hasPendingEventScroll });
  };

  tryResolve();
  pastFirstLook = true;
  if (resolved) {
    holdClaimUntilScrollSettles();
    return;
  }

  observer = new MutationObserver((records) => {
    const current = selector();
    if (current !== lookedFor) {
      tryResolve();
      return;
    }
    for (const record of records) {
      if (record.type === 'attributes') {
        tryResolve();
        return;
      }
      for (const node of record.addedNodes) {
        if (node.nodeType !== 1) continue;
        const el = node as Element;
        if (el.matches(current) || el.querySelector(current)) {
          tryResolve();
          return;
        }
      }
    }
  });
  observer.observe(document.body, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: [addressedBy],
  });
  const armDeadline = () => {
    deadlineTimer = setTimeout(() => {
      const wasOurs = _pendingEventScrollClaim === claim;
      const mayWait = nowMs() - startedAt < EVENT_RESOLVE_MAX_WAIT_MS;
      if (!resolved && wasOurs && mayWait && opts?.stillArriving?.()) {
        armDeadline();
        return;
      }
      stopWatching();
      releaseClaim();
      if (resolved || !wasOurs) {
        if (!resolved) {
          reportOutcome('superseded', document.querySelectorAll<HTMLElement>(selector()));
        }
        return;
      }
      reportOutcome('unresolved', document.querySelectorAll<HTMLElement>(selector()));
      const c = resolveTarget();
      if (c && _follow.value === 'riding' && !isAtLiveEdge(c)) stopFollowingBottom();
      else if (c) wakeParkedRide(c);
      opts?.onUnresolved?.();
    }, EVENT_RESOLVE_DEADLINE_MS);
  };
  armDeadline();
}

export interface DeepLinkOptions {
  onUnresolved?: () => void;
  stillArriving?: () => boolean;
}

export interface EventDeepLinkOptions extends DeepLinkOptions {
  anchorFor?: () => string | null;
}

export function scrollToEventAndPulse(eventId: string, opts?: EventDeepLinkOptions): void {
  if (!eventId) return;
  const anchor = () => opts?.anchorFor?.() ?? eventId;
  scrollToSelectorAndPulse(
    { kind: 'event', eventId },
    () => `[data-event-id="${CSS.escape(anchor())}"]`,
    'data-event-id',
    false,
    (target) =>
      (target.matches?.('.chat-exchange')
        ? target.querySelector?.(anchor() === eventId ? '.initiator-panel' : '.response-panel')
        : null) as HTMLElement | null,
    opts,
  );
}

const CHANGE_RESOLUTION_INITIATOR =
  '.initiator-panel-change-applied,.initiator-panel-change-discarded,.initiator-panel-change-reverted,.initiator-panel-change-failed';
export function scrollToChangeAndPulse(changeId: string, opts?: DeepLinkOptions): void {
  if (!changeId) return;
  const selector = `[data-change-id="${CSS.escape(changeId)}"]`;
  scrollToSelectorAndPulse(
    { kind: 'change', changeId },
    () => selector,
    'data-change-id',
    true,
    (target) =>
      (target.querySelector?.(CHANGE_RESOLUTION_INITIATOR)
        ? target.querySelector?.('.initiator-panel')
        : target.querySelector?.('.response-panel')) as HTMLElement | null,
    opts,
  );
}

export function clearPendingEventScroll(): void {
  _arrivalHold = null;
  _pendingEventScrollClaim = null;
  _pendingEventScrollResolved = false;
  _pendingEventScrollLandedOffEdge = false;
  deepLinkTarget.value = null;
  clearNavFocus();
}

const TURN_SELECTOR = '.chat-exchange';
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

export function makeScrollObservers(el: HTMLElement) {
  const detachGestures = attachReaderGestures(el);
  function isAtTop() {
    return el.scrollTop <= 80;
  }
  function isVisuallyAtTop() {
    return el.scrollTop <= 2;
  }
  function syncNotAtTop() {
    notAtTop.value = isScrollable(el) && !isAtTop();
  }
  function syncScrolledFromTop() {
    scrolledFromTop.value = isScrollable(el) && !isVisuallyAtTop();
  }

  let lastWidth = el.clientWidth;
  let lastHeight = el.clientHeight;
  let anchorChild: HTMLElement | null = null;
  let anchorRelTop = 0;
  let lineTurn: HTMLElement | null = null;
  let lineTurnRelTop = 0;

  function viewportTop() {
    return el.getBoundingClientRect().top;
  }

  function recordAnchor() {
    const line = readLineBlock(el);
    anchorChild = line?.block ?? null;
    anchorRelTop = line?.relTop ?? 0;
    lineTurn = line?.turn ?? null;
    lineTurnRelTop = line?.turnRelTop ?? 0;
  }

  function restoreAfterReflow() {
    const [child, relTop] = anchorChild?.isConnected !== false ? [anchorChild, anchorRelTop] : [lineTurn, lineTurnRelTop];
    if (!child || child.isConnected === false) return;
    const shift = (child.getBoundingClientRect().top - viewportTop()) - relTop;
    if (shift === 0) return;
    holdAcrossRelayout(el, el.scrollTop + shift);
  }

  function onScroll(scroll: ScrollClass = classifyScroll(el)): ScrollClass {
    if (!isElementVisible(el)) return scroll;
    const writesBefore = writeCount(el);
    const followBefore = _follow.value;
    const tweenBefore = _scrollAnimRaf !== null;
    const { atEdge, gesture, placement } = scroll;
    const tookOver = !scroll.onHeld;
    const anchored = scroll.kind === 'anchor';
    const whereWePutIt = scroll.onLastWrite;
    forgetLeftPosition(el);
    if (gesture && !whereWePutIt) _arrivalHold = null;
    const readerMoved = tookOver && (gesture || (placement && !anchored));
    if (_follow.value === 'riding') {
      if (scrollLeavesTheRide(atEdge, tookOver, gesture, placement)) leaveTheRideByScroll();
      else if (!atEdge && tookOver) keepTheLiveEdge(el);
    } else if (_follow.value === 'parked') {
      if (atEdge && (gesture || placement)) unparkTheRide(el);
      else if (readerMoved && landingInFlight()) cancelLanding();
    } else if (readerMoved && landingInFlight()) {
      cancelLanding();
    }
    syncNotAtTop();
    syncScrolledFromTop();
    awayFromBottom.value = !isAtLiveEdge(el);
    recordAnchor();
    const acted = writeCount(el) !== writesBefore
      || _follow.value !== followBefore
      || (_scrollAnimRaf !== null) !== tweenBefore;
    return acted ? classifyScroll(el) : scroll;
  }
  function onResize() {
    if (!isElementVisible(el)) return;
    const width = el.clientWidth;
    const height = el.clientHeight;
    const boxChanged = width !== lastWidth || height !== lastHeight;
    const reflowed = width !== lastWidth;
    lastWidth = width;
    lastHeight = height;
    const scrollOwed = _follow.value === 'riding' && readerScrollOwed(el) > 0;
    if (!scrollOwed && !(boxChanged && keepTheLiveEdge(el)) && reflowed) {
      restoreAfterReflow();
    }
    honourGrowth(el);
    syncNotAtTop();
    syncScrolledFromTop();
    awayFromBottom.value = !isAtLiveEdge(el);
    recordAnchor();
  }
  return { onScroll, onResize, detachGestures };
}
