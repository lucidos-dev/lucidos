import { signal } from '@preact/signals';
import { type DeepLinkTarget } from '../../store/thread-events/exchange-anchors';
import { scrollPosition } from './scrollOwner';
import { nowMs } from '../../utils/scrollActivity';
import { recordReadingLine } from './scrollHold';
import { markBoot } from '../../utils/bootTiming';
import { isElementVisible, unmeasurableBox } from '../../utils/elementVisibility';
import { postClientLog } from '../../utils/clientLog';
import { resolveTarget, isLiveEdgeTop, clampToScrollRange, isAtLiveEdge } from './scrollSurface';
import { landingTargetOf, smoothScrollToElement } from './scrollNavigation';
import { followState, unparkTheRide, rideToLiveEdge, stopFollowingBottom, wakeParkedRide } from './scrollRide';
import { applyNavFocus, clearNavFocus } from '../shared/focusMarker';
import { EVENT_RESOLVE_DEADLINE_MS, EVENT_RESOLVE_MAX_WAIT_MS } from './deepLinkBudget';

export const deepLinkTarget = signal<DeepLinkTarget | null>(null);

const SCROLL_SETTLE_FALLBACK_MS = 1000;

export function hasPendingEventScroll(): boolean {
  return scrollPosition.claim() !== null;
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

export function deepLinkHasResolved(): boolean {
  return scrollPosition.claimHasLanded();
}

export function deepLinkLandedOffLiveEdge(): boolean {
  return scrollPosition.linkLandedOffEdge();
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

  scrollPosition.transition({ type: 'claim-link', claim });
  for (const listener of _deepLinkClaimListeners) listener();
  deepLinkTarget.value = linkTarget;

  const releaseClaim = () => {
    if (scrollPosition.claim() === claim) {
      scrollPosition.transition({ type: 'release-link', claim });
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
    if (container && landsOnTheEdge && followState() !== 'off') {
      unparkTheRide(container);
      rideToLiveEdge(container);
    } else {
      stopFollowingBottom();
      smoothScrollToElement(target, { kind: 'link', claim }, () => {
        if (container) recordReadingLine(container, target);
      });
    }
    scrollPosition.transition({ type: 'link-landed', claim, atEdge: landsOnTheEdge });
    if (scrollPosition.claim() === claim) {
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
      const wasOurs = scrollPosition.claim() === claim;
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
      if (c && followState() === 'riding' && !isAtLiveEdge(c)) stopFollowingBottom();
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
  scrollPosition.transition({ type: 'release-link' });
  deepLinkTarget.value = null;
  clearNavFocus();
}
