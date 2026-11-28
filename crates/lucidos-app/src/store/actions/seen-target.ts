
import { effect, untracked } from '@preact/signals';
import {
  activeMenuItem,
  focusedThreadId,
  mobileView,
  panelOverlay,
  settingsSubview,
  readRequestStanding,
  threadMap,
  unreadNotifications,
} from '../store';
import type { MobileView, PanelOverlay, SettingsSubview } from '../store';
import { drawnSplitRatio } from '../../components/layout/drawnLayout';
import type { MenuItem } from '../types';
import { isInViewport, isReplyEndInViewport, lastExchangeOf, viewportIsMobile } from '../../utils/viewport';
import { markReplySeen } from '../../api/threads';
import { postClientLog } from '../../utils/clientLog';
import { isPageActive } from '../../utils/pageActive';
import { onPageHide, onPageWake } from '../../utils/pageVisit';
import { markReadOptimistic } from './notifications';
import {
  appVisitKey,
  fileVisitKey,
  notificationTarget,
  panelVisitKey,
  settingsVisitKey,
  threadVisitKey,
  triggerVisitKey,
} from './visitKeys';
import type { SeenTarget } from './visitKeys';

export const SEEN_DWELL_MS = 1000;


export interface VisitLocation {
  focusedThreadId: string | null;
  overlay: PanelOverlay;
  activeMenuItem: MenuItem;
  settingsSubview: SettingsSubview;
  mobile: boolean;
  mobileView: MobileView;
  drawnSplitRatio: number;
}

export function threadPaneOnScreen(loc: Pick<VisitLocation, 'mobile' | 'mobileView' | 'drawnSplitRatio'>): boolean {
  return loc.mobile ? loc.mobileView === 'thread' : loc.drawnSplitRatio > 0;
}

export function contentPaneOnScreen(loc: VisitLocation): boolean {
  return loc.mobile ? loc.mobileView === 'content' : loc.drawnSplitRatio < 1;
}

function contentVisitKey(loc: VisitLocation): string | null {
  const overlay = loc.overlay;
  if (overlay) {
    if (overlay.type === 'app-ui') return appVisitKey(overlay.app.id);
    if (overlay.type === 'file-preview') return fileVisitKey(overlay.path);
    if (overlay.type === 'form' && overlay.form.type === 'trigger' && overlay.form.triggerId) {
      return triggerVisitKey(overlay.form.triggerId);
    }
    return null;
  }
  if (loc.activeMenuItem === 'settings') return settingsVisitKey(loc.settingsSubview);
  return panelVisitKey(loc.activeMenuItem);
}

export function visitedKeys(loc: VisitLocation): string[] {
  const keys: string[] = [];
  if (loc.focusedThreadId && threadPaneOnScreen(loc)) {
    keys.push(threadVisitKey(loc.focusedThreadId));
  }
  if (contentPaneOnScreen(loc)) {
    const key = contentVisitKey(loc);
    if (key) keys.push(key);
  }
  return keys;
}


export interface SeenWaits {
  since: Map<string, number>;
  done: Set<string>;
}

export function emptySeenWaits(): SeenWaits {
  return { since: new Map(), done: new Set() };
}

export function sampleSeen(
  waits: SeenWaits,
  now: number,
  satisfied: readonly string[],
  dwellMs: number = SEEN_DWELL_MS,
): string[] {
  const live = new Set(satisfied);
  for (const id of [...waits.since.keys()]) {
    if (!live.has(id)) waits.since.delete(id);
  }
  const completed: string[] = [];
  for (const id of live) {
    if (waits.done.has(id)) continue;
    const started = waits.since.get(id);
    if (started === undefined) {
      waits.since.set(id, now);
      continue;
    }
    if (now - started >= dwellMs) {
      waits.since.delete(id);
      waits.done.add(id);
      completed.push(id);
    }
  }
  return completed;
}

export function pruneSeenWaits(waits: SeenWaits, keep: ReadonlySet<string>): void {
  for (const id of [...waits.since.keys()]) if (!keep.has(id)) waits.since.delete(id);
  for (const id of [...waits.done]) if (!keep.has(id)) waits.done.delete(id);
}

export function nextDwellDueIn(
  waits: SeenWaits,
  now: number,
  dwellMs: number = SEEN_DWELL_MS,
): number | null {
  let earliest: number | null = null;
  for (const started of waits.since.values()) {
    if (earliest === null || started < earliest) earliest = started;
  }
  if (earliest === null) return null;
  return Math.max(0, earliest + dwellMs - now);
}


interface ReplyEndTarget {
  kind: 'reply-end';
  threadId: string;
}

interface WatchedTarget {
  id: string;
  target: SeenTarget | ReplyEndTarget;
  onSeen: () => void;
}

const waits = emptySeenWaits();
interface Arrival {
  places: Set<string>;
  seenUnread: boolean;
}

const arrivals = new Map<string, Arrival>();

export const MAX_HELD_ARRIVALS = 100;
let watched: WatchedTarget[] = [];
let observer: IntersectionObserver | null = null;
let observed: Element[] = [];
let renderWatch: MutationObserver | null = null;
let renderWatchTimer: ReturnType<typeof setTimeout> | null = null;
let dwellTimer: ReturnType<typeof setTimeout> | null = null;
let cancelCoalesced: (() => void) | null = null;
let installed = false;

export function noteArrivalWhilePresent(notificationId: string): void {
  if (!isPageActive()) return;
  arrivals.set(notificationId, { places: new Set(visitedKeys(currentLocation())), seenUnread: false });
  for (const oldest of arrivals.keys()) {
    if (arrivals.size <= MAX_HELD_ARRIVALS) break;
    arrivals.delete(oldest);
  }
}

export function noteArrivalWhileAway(notificationId: string): void {
  arrivals.delete(notificationId);
}

function narrowArrivals(places: ReadonlySet<string>): void {
  for (const [id, arrival] of arrivals) {
    for (const key of arrival.places) if (!places.has(key)) arrival.places.delete(key);
    if (arrival.places.size === 0) arrivals.delete(id);
  }
}

function pruneArrivals(unread: ReadonlySet<string>): void {
  for (const [id, arrival] of arrivals) {
    if (unread.has(id)) arrival.seenUnread = true;
    else if (arrival.seenUnread) arrivals.delete(id);
  }
}

function currentLocation(): VisitLocation {
  return {
    focusedThreadId: focusedThreadId.value,
    overlay: panelOverlay.value,
    activeMenuItem: activeMenuItem.value,
    settingsSubview: settingsSubview.value,
    mobile: viewportIsMobile.value,
    mobileView: mobileView.value,
    drawnSplitRatio: drawnSplitRatio.value,
  };
}

function satisfiedIds(loc: VisitLocation, places: ReadonlySet<string>): string[] {
  const onThreadPane = threadPaneOnScreen(loc);
  const ids: string[] = [];
  for (const { id, target } of watched) {
    if (target.kind === 'place') {
      if (places.has(target.key) && !arrivals.get(id)?.places.has(target.key)) ids.push(id);
      continue;
    }
    if (!onThreadPane || loc.focusedThreadId !== target.threadId) continue;
    const inBand = target.kind === 'event'
      ? isInViewport(target.eventId)
      : isReplyEndInViewport(target.threadId);
    if (inBand) ids.push(id);
  }
  return ids;
}

function readRequestWatchId(threadId: string): string {
  return `read-request:${threadId}`;
}

function rearm(): void {
  watched = [];
  const keep = new Set<string>();
  const set = unreadNotifications.value;
  const notificationsKnown = set.status === 'loaded';
  if (notificationsKnown) {
    for (const notification of set.data) {
      keep.add(notification.id);
      const target = notificationTarget(notification.tap);
      if (target) watched.push({ id: notification.id, target, onSeen: () => markReadOptimistic(notification.id) });
    }
  }
  for (const thread of threadMap.value.values()) {
    if (!readRequestStanding(thread)) continue;
    const threadId = thread.meta.id;
    const id = readRequestWatchId(threadId);
    keep.add(id);
    watched.push({ id, target: { kind: 'reply-end', threadId }, onSeen: () => reportReplySeen(threadId) });
  }
  if (notificationsKnown) {
    pruneSeenWaits(waits, keep);
    pruneArrivals(keep);
  }
}

function endVisitsIfAway(): void {
  if (!isPageActive()) arrivals.clear();
}

function reportReplySeen(threadId: string): void {
  const version = threadMap.value.get(threadId)?.meta.summaryVersion;
  if (version === undefined) return;
  markReplySeen(threadId, version).catch((e: unknown) => {
    postClientLog('seen-target', 'reporting a read reply failed', { threadId, error: String(e) });
    waits.done.delete(readRequestWatchId(threadId));
    scheduleResample();
  });
}

function clearDwellTimer(): void {
  if (dwellTimer !== null) clearTimeout(dwellTimer);
  dwellTimer = null;
}

export const RENDER_WATCH_MS = 5000;

function clearRenderWatch(): void {
  renderWatch?.disconnect();
  renderWatch = null;
  if (renderWatchTimer !== null) clearTimeout(renderWatchTimer);
  renderWatchTimer = null;
}

function disarmViewportObserver(): void {
  observer?.disconnect();
  observer = null;
  observed = [];
  clearRenderWatch();
}

function armRenderWatch(missing: boolean): void {
  if (!missing) {
    clearRenderWatch();
    return;
  }
  if (renderWatch) return;
  if (typeof MutationObserver !== 'function' || typeof document === 'undefined') return;
  if (!document.body) return;
  renderWatch = new MutationObserver(scheduleResample);
  renderWatch.observe(document.body, { childList: true, subtree: true });
  renderWatchTimer = setTimeout(clearRenderWatch, RENDER_WATCH_MS);
}

function sameElements(a: readonly Element[], b: readonly Element[]): boolean {
  return a.length === b.length && a.every((el, i) => el === b[i]);
}

function armViewportObserver(): void {
  if (typeof IntersectionObserver !== 'function' || typeof document === 'undefined') return;
  const els: Element[] = [];
  let missing = false;
  for (const { target } of watched) {
    if (target.kind === 'place') continue;
    if (focusedThreadId.value !== target.threadId) continue;
    if (target.kind === 'reply-end') {
      const last = lastExchangeOf(target.threadId);
      if (last) els.push(last);
      else missing = true;
      continue;
    }
    const found = document.querySelectorAll(`[data-event-id="${CSS.escape(target.eventId)}"]`);
    if (found.length === 0) missing = true;
    for (const el of found) els.push(el);
  }
  if (!sameElements(els, observed)) {
    observer?.disconnect();
    observer = null;
    observed = els;
    if (els.length > 0) {
      observer = new IntersectionObserver(scheduleResample);
      for (const el of els) observer.observe(el);
    }
  }
  armRenderWatch(missing);
}

export function resampleSeenTargets(): void {
  clearDwellTimer();
  if (!isPageActive()) {
    waits.since.clear();
    endVisitsIfAway();
    return;
  }
  const loc = currentLocation();
  const places = new Set(visitedKeys(loc));
  narrowArrivals(places);
  if (watched.length === 0) {
    disarmViewportObserver();
    return;
  }
  armViewportObserver();
  const now = Date.now();
  const completed = new Set(sampleSeen(waits, now, satisfiedIds(loc, places)));
  for (const { id, onSeen } of watched) if (completed.has(id)) onSeen();
  const due = nextDwellDueIn(waits, now);
  if (due !== null) dwellTimer = setTimeout(resampleSeenTargets, due);
}

function clearCoalesced(): void {
  if (cancelCoalesced) cancelCoalesced();
  cancelCoalesced = null;
}

function standDown(): void {
  clearCoalesced();
  clearDwellTimer();
  waits.since.clear();
  endVisitsIfAway();
}

function scheduleResample(): void {
  if (cancelCoalesced) return;
  const run = () => {
    cancelCoalesced = null;
    resampleSeenTargets();
  };
  if (typeof requestAnimationFrame === 'function') {
    const frame = requestAnimationFrame(run);
    cancelCoalesced = () => cancelAnimationFrame(frame);
  } else {
    const timer = setTimeout(run, 0);
    cancelCoalesced = () => clearTimeout(timer);
  }
}

export function installSeenTargetWatch(): void {
  if (installed) return;
  installed = true;

  effect(() => {
    currentLocation();
    untracked(scheduleResample);
  });
  effect(() => {
    unreadNotifications.value;
    untracked(() => {
      rearm();
      scheduleResample();
    });
  });
  effect(() => {
    threadMap.value;
    untracked(() => {
      rearm();
      scheduleResample();
    });
  });

  if (typeof document !== 'undefined' && typeof document.addEventListener === 'function') {
    document.addEventListener('scroll', scheduleResample, { capture: true, passive: true });
  }
  if (typeof window !== 'undefined' && typeof window.addEventListener === 'function') {
    window.addEventListener('resize', scheduleResample, { passive: true });
    window.addEventListener('blur', standDown);
    window.addEventListener('focus', scheduleResample);
  }
  onPageHide(standDown);
  onPageWake(scheduleResample);
}

export function _resetSeenTargetWatchForTesting(): void {
  clearCoalesced();
  clearDwellTimer();
  disarmViewportObserver();
  waits.since.clear();
  waits.done.clear();
  arrivals.clear();
  watched = [];
}

export function _heldArrivalCountForTesting(): number {
  return arrivals.size;
}
