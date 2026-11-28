import { scrollPosition } from './scrollOwner';
import { type ReadonlySignal, computed, signal } from '@preact/signals';
import { resolveTarget, isAtLiveEdge, liveEdgeTop, syncAwayFromBottom } from './scrollSurface';
import {
  hasPendingEventScroll,
  deepLinkLandedOffLiveEdge,
  deepLinkHasResolved,
  clearPendingEventScroll,
} from './scrollLink';
import { holdPosition, writeLedgerScroll, isWhereWeHeldIt } from './scrollLedger';
import { forgetSubmits, hasPendingLanding } from './scrollLanding';
import {
  heldGlideInFlight,
  cancelScrollAnim,
  glideToLiveEdge,
  scrollAnimating,
  isAnchorScroll,
} from './scrollNavigation';
import { readerScrollOwed } from './readerInput';
import { postClientLog } from '../../utils/clientLog';

type FollowState = 'off' | 'riding' | 'parked';

export function followState(): FollowState {
  if (!scrollPosition.armed()) return 'off';
  return scrollPosition.ridePending() ? 'riding' : 'parked';
}

export const followingLiveEdge: ReadonlySignal<boolean> = computed(() => followState() !== 'off');

let _transcriptLive = false;

export const transcriptIsLive = () => _transcriptLive;

export function setTranscriptLive(live: boolean): void {
  const waking = live && !_transcriptLive;
  _transcriptLive = live;
  if (!waking) return;
  const el = resolveTarget();
  if (el) wakeParkedRide(el);
}

export function wakeParkedRide(el: HTMLElement): boolean {
  if (followState() !== 'parked' || !_transcriptLive) return false;
  if (hasPendingEventScroll()) return false;
  unparkTheRide(el);
  rideToLiveEdge(el);
  return true;
}

export function unparkTheRide(el: HTMLElement): void {
  if (followState() !== 'parked') return;
  scrollPosition.transition({ type: 'resume' });
  resumedRide(el);
}

export function resumedRide(el: HTMLElement): void {
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

export function markHeldScroll(el: HTMLElement, top: number) {
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
  const wasArmed = followState() !== 'off';
  scrollPosition.transition({ type: 'arm' });
  holdPosition(el);
  _rideShortReported = false;
  forgetSubmits();
  if (!wasArmed) for (const listener of _followRideListeners) listener();
}

export type FollowResumeFrom = 'live-edge' | 'in-place';

export function resumeFollowingBottom(el: HTMLElement, from: FollowResumeFrom = 'live-edge'): void {
  if (from === 'in-place') {
    if (followState() !== 'off') return;
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

export function stopFollowingBottom() {
  leaveTheRide('off');
}

export function leaveTheRide(next?: 'off' | 'parked') {
  cancelRideSettle();
  if (heldGlideInFlight()) cancelScrollAnim();
  if (next) scrollPosition.transition({ type: next === 'off' ? 'disarm' : 'park' });
  holdPosition(null);
  forgetSubmits();
}

export function rideToLiveEdge(el: HTMLElement): void {
  if (!isAtLiveEdge(el)) { glideToLiveEdge(el, 'ride'); return; }
  cancelScrollAnim();
  syncAwayFromBottom();
}

function canCarryTheRide(): boolean {
  return scrollPosition.canRide() && !scrollAnimating();
}

export function keepTheLiveEdge(el: HTMLElement): boolean {
  if (!canCarryTheRide()) return false;
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

let _rideSettleRaf: number | null = null;

let _rideShortReported = false;

export function cancelRideSettle(): void {
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
  if (!canCarryTheRide() || hasPendingLanding()) return;
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

export function setFollowLiveEdge(on: boolean): void {
  recordFollowSeed(on);
  if (!on) { stopFollowingBottom(); return; }
  clearPendingEventScroll();
  const el = resolveTarget();
  if (!el) return;
  armFollowOn(el);
  rideToLiveEdge(el);
}
