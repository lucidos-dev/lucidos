import { readSavedScroll, threadScrollKey } from '../savedScroll';
import { readerHasReached } from '../thread-events/exchange-anchors';
import { effect, signal, untracked } from '@preact/signals';
import { ApiError } from '../../api/client';
import { fetchEventLocation } from '../../api/threads';
import { EVENT_RESOLVE_DEADLINE_MS } from '../../components/chat/deepLinkBudget';
import { UNANCHORABLE_ASYNC_EVENTS, landingTarget, computeExchanges, deepLinkAnchorInThread } from '../thread-events';
import type { LandingGroup, LandingTarget } from '../thread-events';
import { showToast, threadMap, focusedThreadId, effectiveThreadStatus, changeAwaitsApply, drawerGrouping, selectedOngoingGroup, threadSearchQuery, mobileView } from '../store';
import type { DrawerGrouping, OngoingGroup } from '../store';
import { drawnSplitRatio } from '../../components/layout/drawnLayout';
import { viewportIsMobile } from '../../utils/viewport';
import { threadPaneOnScreen } from './seen-target';
import { errorDetail } from '../../utils/errorDetail';
import { ensureThreadByIdInMap, ensureWholeThreadLoaded, loadThreadEvents, threadEventsStillArriving } from './thread-loading';
import { focusThread, landOnEvent, type FocusThreadOptions } from './threads';

const NOT_IN_A_CONVERSATION =
  'That event is a workspace event, not part of any conversation, so there is nowhere to open it.';

const NOTHING_TO_LAND_ON =
  'That event is not drawn anywhere in its thread, so there is nothing to jump to.';

export type EventTarget =
  | { kind: 'nowhere'; note: string }
  | { kind: 'anchored'; threadId: string; anchor: string }
  | { kind: 'unloaded'; threadId: string };

const owningThreads = new Map<string, string | null>();

const owningThreadRequests = new Map<string, Promise<string | null>>();

async function owningThreadId(eventId: string): Promise<string | null> {
  const openId = focusedThreadId.value;
  const open = openId ? threadMap.value.get(openId) : null;
  if (open) {
    for (const event of open.events.values()) {
      if (event._eventId === eventId) return openId;
    }
  }
  const cached = owningThreads.get(eventId);
  if (cached !== undefined) return cached;
  const inFlight = owningThreadRequests.get(eventId);
  if (inFlight) return inFlight;
  const request = fetchEventLocation(eventId)
    .then(location => {
      owningThreads.set(eventId, location.thread_id);
      return location.thread_id;
    })
    .finally(() => {
      owningThreadRequests.delete(eventId);
    });
  owningThreadRequests.set(eventId, request);
  return request;
}

function anchorInLoadedThread(threadId: string, eventId: string): string | null {
  return deepLinkAnchorInThread(threadMap.value.get(threadId), eventId);
}

function loadedThreadHolds(threadId: string, eventId: string): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread?.eventsLoaded) return false;
  for (const event of thread.events.values()) {
    if (event._eventId === eventId) return true;
  }
  return false;
}

export async function resolveEventTarget(eventId: string): Promise<EventTarget> {
  const threadId = await owningThreadId(eventId);
  if (!threadId) return { kind: 'nowhere', note: NOT_IN_A_CONVERSATION };
  if (!loadedThreadHolds(threadId, eventId)) return { kind: 'unloaded', threadId };
  const anchor = anchorInLoadedThread(threadId, eventId);
  if (anchor) return { kind: 'anchored', threadId, anchor };
  if (threadMap.value.get(threadId)?.hasOlderEvents) return { kind: 'unloaded', threadId };
  return { kind: 'nowhere', note: NOTHING_TO_LAND_ON };
}

export function jumpableEventId(
  eventId: string | undefined,
  eventType: string | undefined,
): string | undefined {
  if (eventType && UNANCHORABLE_ASYNC_EVENTS.has(eventType)) return undefined;
  return eventId;
}

const eventTargetVerdicts = signal<ReadonlyMap<string, boolean>>(new Map());

export function eventHasTarget(eventId: string | undefined): boolean {
  return !!eventId && eventTargetVerdicts.value.get(eventId) === true;
}

const verdictsInFlight = new Set<string>();

function recordVerdict(eventId: string, hasTarget: boolean): void {
  const next = new Map(eventTargetVerdicts.value);
  next.set(eventId, hasTarget);
  eventTargetVerdicts.value = next;
}

export function ensureEventTargetResolved(eventId: string | undefined): void {
  if (!eventId) return;
  if (eventTargetVerdicts.value.has(eventId) || verdictsInFlight.has(eventId)) return;
  verdictsInFlight.add(eventId);
  void resolveEventTarget(eventId)
    .then(target => recordVerdict(eventId, target.kind !== 'nowhere'))
    .catch(e => {
      if (e instanceof ApiError && e.httpCode === 404) recordVerdict(eventId, false);
    })
    .finally(() => verdictsInFlight.delete(eventId));
}

export function _resetEventTargetCacheForTesting(): void {
  owningThreads.clear();
  owningThreadRequests.clear();
  verdictsInFlight.clear();
  eventTargetVerdicts.value = new Map();
}

function reportNoTarget(eventId: string, note: string): void {
  recordVerdict(eventId, false);
  showToast(note, 'warning');
}

export async function showEventWhereItLives(eventId: string): Promise<void> {
  if (!eventId) return;
  try {
    const target = await resolveEventTarget(eventId);
    if (target.kind === 'nowhere') {
      reportNoTarget(eventId, target.note);
      return;
    }
    if (target.kind === 'anchored') {
      focusThread(target.threadId, { targetEventId: target.anchor });
      return;
    }
    if (!(await ensureThreadByIdInMap(target.threadId))) {
      reportNoTarget(eventId, 'The thread that event happened in no longer exists.');
      return;
    }
    await awaitThreadEvents(target.threadId);
    let anchor = anchorInLoadedThread(target.threadId, eventId);
    if (!anchor && threadMap.value.get(target.threadId)?.hasOlderEvents) {
      await ensureWholeThreadLoaded(target.threadId);
      anchor = anchorInLoadedThread(target.threadId, eventId);
    }
    if (anchor) {
      focusThread(target.threadId, { targetEventId: anchor });
      return;
    }
    if (loadedThreadHolds(target.threadId, eventId)) {
      reportNoTarget(eventId, NOTHING_TO_LAND_ON);
      return;
    }
    focusThread(target.threadId, { targetEventId: eventId });
  } catch (e) {
    if (e instanceof ApiError && e.httpCode === 404) {
      reportNoTarget(eventId, 'That event is no longer in the event store.');
      return;
    }
    showToast(`Could not open that event: ${errorDetail(e)}`, 'error');
  }
}

export function openOngoingThread(threadId: string, group: LandingGroup): void {
  focusThread(threadId);
  landOrSay(threadId, group);
}

function landOrSay(threadId: string, group: LandingGroup): void {
  landOnTarget(threadId, group).catch(err => {
    showToast(`Could not open what needs your attention: ${errorDetail(err)}`, 'error');
  });
}

function conversationPaneOnScreen(): boolean {
  return threadPaneOnScreen({
    mobile: viewportIsMobile.value,
    mobileView: mobileView.value,
    drawnSplitRatio: drawnSplitRatio.value,
  });
}

export function landingGroupFor(grouping: DrawerGrouping, selected: OngoingGroup): LandingGroup | null {
  if (grouping !== 'ongoing') return null;
  return selected === 'blocked' || selected === 'review' ? selected : null;
}

export function openListedThread(threadId: string, options?: Pick<FocusThreadOptions, 'revealPane'>): void {
  const searching = threadSearchQuery.value.trim().length > 0;
  const landing = searching ? null : landingGroupFor(drawerGrouping.value, selectedOngoingGroup.value);
  focusThread(threadId, options);
  if (!landing) return;
  if (options?.revealPane !== false || conversationPaneOnScreen()) {
    landOrSay(threadId, landing);
    return;
  }
  const dispose = effect(() => {
    if (focusedThreadId.value !== threadId) {
      queueMicrotask(() => dispose());
      return;
    }
    if (!conversationPaneOnScreen()) return;
    queueMicrotask(() => dispose());
    untracked(() => landOrSay(threadId, landing));
  });
}

async function landOnTarget(threadId: string, group: LandingGroup): Promise<void> {
  const stillOpen = () => focusedThreadId.value === threadId;
  if (threadEventsStillArriving(threadId)) {
    await awaitThreadEvents(threadId);
    if (!stillOpen() || threadEventsStillArriving(threadId)) return;
  }
  let target = landingTargetIn(threadId, group);
  if (target?.kind === 'card-not-loaded' && threadMap.value.get(threadId)?.hasOlderEvents) {
    await ensureWholeThreadLoaded(threadId);
    if (!stillOpen()) return;
    target = landingTargetIn(threadId, group);
  }
  const thread = threadMap.value.get(threadId);
  if (target && thread && readerHasReached(computeExchanges(thread), target, readSavedScroll(threadScrollKey(threadId)))) return;
  const eventId = target?.kind === 'card-not-loaded' ? target.newestTurnId : target?.eventId;
  if (eventId) landOnEvent(threadId, eventId);
}

function landingTargetIn(threadId: string, group: LandingGroup): LandingTarget | null {
  const thread = threadMap.value.get(threadId);
  if (!thread?.eventsLoaded) return null;
  const changeReady = group === 'review' && changeAwaitsApply(thread.meta);
  return landingTarget(computeExchanges(thread), effectiveThreadStatus(thread), group, changeReady);
}

async function awaitThreadEvents(threadId: string): Promise<void> {
  await loadThreadEvents(threadId);
  if (!threadEventsStillArriving(threadId)) return;
  await new Promise<void>(resolve => {
    let dispose: (() => void) | undefined;
    let settled = false;
    const finish = () => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      dispose?.();
      resolve();
    };
    const timer = setTimeout(finish, EVENT_RESOLVE_DEADLINE_MS);
    dispose = effect(() => {
      void threadMap.value;
      queueMicrotask(() => {
        if (!threadEventsStillArriving(threadId)) finish();
      });
    });
  });
}
