import { type ReadonlySignal, computed, signal } from '@preact/signals';
import {
  awayFromBottom,
  liveEdgeTop,
  resolveTarget,
  isAtLiveEdge,
  syncAwayFromBottom,
  RESTING_SLACK_PX,
} from './scrollSurface';
import {
  rideGlideInFlight,
  landingGlideInFlight,
  cancelScrollAnim,
  TURN_SELECTOR,
  glideToLiveEdge,
  SEND_PACE,
  landingTargetOf,
  markNavigationScroll,
  animateScroll,
  freezeLandingGlide,
  retargetLandingGlide,
  scrollAnimating,
} from './scrollNavigation';
import { nowMs } from '../../utils/scrollActivity';
import { isElementVisible } from '../../utils/elementVisibility';
import { retireReaderMovement } from './readerInput';
import { followState, unparkTheRide } from './scrollRide';
import { holdPosition } from './scrollLedger';
import { isReducedMotion } from '../../utils/motion';

export const showScrollToBottom: ReadonlySignal<boolean> = computed(
  () => awayFromBottom.value && !_pendingLanding.value?.on && !rideGlideInFlight(),
);

type ResolvedTurn = { turn: HTMLElement; drawnAtSubmit: number };

type TurnResolver = (el: HTMLElement) => ResolvedTurn | null;

const _pendingLanding = signal<{
  resolveTurn: TurnResolver;
  holds: boolean;
  aim?: LandingAim;
  on: ResolvedTurn | null;
  at: number;
} | null>(null);

export const hasPendingLanding = () => _pendingLanding.value !== null;

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

export function forgetSubmits() {
  _pendingLanding.value = null;
  _rideArrival = null;
  _cardAwaitingConfirm = null;
  _nextCardWatch = null;
}

export function cancelLanding() {
  if (landingGlideInFlight()) cancelScrollAnim();
  forgetSubmits();
}

function dropLapsedLanding(): void {
  const landing = _pendingLanding.value;
  if (!landing) return;
  const deadline = landing.on ? LANDING_AGENT_START_MS
    : landing.holds ? LANDING_ADDRESSABLE_MS : LANDING_ROUND_TRIP_MS;
  if (nowMs() - landing.at >= deadline) _pendingLanding.value = null;
}

export function landingInFlight(): boolean {
  return _cardAwaitingConfirm !== null || _pendingLanding.value !== null
    || landingGlideInFlight() || _nextCardWatch !== null;
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

function followSubmit(resolveTurn: TurnResolver, holds = true, aim?: LandingAim): void {
  retireReaderMovement();
  _cardAwaitingConfirm = null;
  _nextCardWatch = null;
  const el = resolveTarget();
  if (!el) return;
  if (followState() !== 'off') {
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

export function honourLanding(el: HTMLElement): void {
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

export function landOnNextCard(el: HTMLElement): boolean {
  const watch = _nextCardWatch;
  if (!watch) return false;
  if (nowMs() - watch.at >= LANDING_ADDRESSABLE_MS) { _nextCardWatch = null; return false; }
  const card = answerableQuestionId(el);
  if (card === null || card === watch.seen) return false;
  _nextCardWatch = null;
  if (landingGlideInFlight()) {
    retargetLandingGlide(liveEdgeTop(el));
  } else {
    landAtLiveEdge(el, liveEdgeTop(el));
  }
  return true;
}

export function glideToSubmittedTurn(el: HTMLElement): boolean {
  const arrival = _rideArrival;
  if (!arrival) return false;
  if (followState() !== 'riding' || nowMs() - arrival.at >= LANDING_ADDRESSABLE_MS) {
    _rideArrival = null;
    return false;
  }
  if (!arrival.resolveTurn(el)) return false;
  _rideArrival = null;
  if (scrollAnimating() || isAtLiveEdge(el)) return false;
  glideToLiveEdge(el, 'ride', undefined, SEND_PACE);
  return true;
}
