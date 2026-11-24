import { threadMap, awaitedThreadId, focusedThreadId, setFocusedThread, showToast, removeToast, connectionStatus, threadsLoaded, threadListFetched, generatedTitleIds, threadHasMore, threadLoadingMore, archiveThreadCount, ALL_CHANNELS, filterFacets, codingAgentSessionVersion, engineRestarting, CODING_AGENT_CHANNEL, toasts, THREAD_EVENTS_LOAD_TOAST_KEY, THREAD_EVENTS_REFRESH_TOAST_KEY, THREAD_EVENTS_FETCH_CONCURRENCY, THREAD_EVENTS_PREFETCH_LIMIT, threadChannelToFilterSource, type ThreadFilterSource } from '../store';
import { appliedThreadFilter, type ThreadFilterSelection } from '../appliedThreadFilter';
import { threadPassesChannelFilter } from '../threadFilter';
import { settleDeliveredUnsentMessage } from './sendSettlement';
import { noteImageLanded } from '../landedImages';
import { liftRowIntent, overlayRowIntent } from '../rowIntents';
import { handleEvent, isCallerUtterance, isChannelDefiningEvent, offerCallerUtterance, PENDING_TITLE_PLACEHOLDER, applyAggregateToThread, firstSendUnsettled, applySummaryVersion, isSummaryCurrent, createdKey, isExcludedFromSections, type ThreadAggregate, type ThreadState, type ThreadEvent, type StoredEvent, type ThreadMeta } from '../thread-events';
import { bumpThreadEvents } from '../threadActivity';
import { recordPerfSample } from '../../utils/perfQueue';
import { runWithConcurrency } from '../../utils/concurrentPool';
import { markThreadOpenStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { applyDraftBatch, type ComposeDraft } from '../composeDrafts';
import { fetchThreads, fetchThreadById, fetchThreadEvents, fetchOlderThreads, fetchFilterFacets, fetchArchivedCount } from '../../api/threads';
import type { ThreadSummary, ThreadEventRow, ThreadEventsSnapshot } from '../../api/threads';
import { isTransientFetchError, retryTransient } from '../../api/client';
import { setLoadingIfFresh, toFailed } from '../types';
import { loadSlice, type SliceOf } from '../bootstrapSliceLoads';
import { errorDetail } from '../../utils/errorDetail';
import { postClientLog } from '../../utils/liveness';
import { acceptComposeReport, parseComposeMode } from '../composeSync';

type DraftBatch = Map<string, ComposeDraft | null>;

function makeThreadState(info: ThreadSummary, saved: boolean, batch?: DraftBatch): ThreadState {
  stageDraftFromApi(info, batch);
  return {
    meta: {
      id: info.thread_id,
      title: info.title || PENDING_TITLE_PLACEHOLDER,
      channel: info.channel as ThreadMeta['channel'],
      initiator: info.initiator,
      saved,
      ...(info.home ? { home: true as const } : {}),
      createdAt: info.created_at || new Date().toISOString(),
      updatedAt: info.last_activity || info.created_at || new Date().toISOString(),
      lastUserAction: info.last_user_action || info.last_activity || info.created_at || new Date().toISOString(),
      lastAgentAction: info.last_agent_action || info.last_activity || info.created_at || new Date().toISOString(),
      status: info.status || 'idle',
      summaryVersion: info.summary_version,
      messageCount: info.message_count || 0,
      section: (info.section as ThreadMeta['section']) || 'archived',
      activeChildrenCount: info.active_children_count || 0,
      awaitedChildrenCount: info.awaited_children_count || 0,
      awaitedByParent: !!info.awaited_by_parent,
      totalChildrenCount: info.total_children_count || 0,
      blockingDescendantCount: info.blocking_descendant_count || 0,
      attentionDescendantCount: info.attention_descendant_count || 0,
      isStoppedChild: info.is_stopped_child === true,
      hold: info.hold ?? null,
      readRequested: info.read_requested === true,
      replySeenDevices: info.reply_seen_devices ?? [],
      applyInFlight: info.apply_in_flight === true,
      liveEventWaitCount: info.live_event_wait_count || 0,
      codingAgentChangeState: info.coding_agent_change_state,
      codingAgentIsExternalRepo: info.coding_agent_is_external_repo || false,
      lastRevivedAt: info.last_revived_at || '',
      parentThreadId: info.parent_thread_id || undefined,
      parentThreadTitle: info.parent_thread_title || undefined,
      successor: info.successor,
      successorOf: info.successor_of,
      triggerId: info.trigger_id || undefined,
      triggerName: info.trigger_name || undefined,
      repoId: info.cc_repo_id || undefined,
      repoName: info.cc_repo_name || undefined,
      codingAgentKind: info.coding_agent_kind || undefined,
      codingAgentFolder: info.coding_agent_folder || undefined,
      codingAgent: info.coding_agent || undefined,
      codingAgents: info.coding_agents,
      state: info.state,
      latestTodoList: null,
      liveEventWaits: info.live_event_waits ?? [],
      cacheKeepAliveStatus: info.cache_keep_alive_status ?? null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

function stageDraftFromApi(info: ThreadSummary, batch?: DraftBatch): void {
  acceptComposeReport(info.thread_id, {
    revision: info.summary_version ?? null,
    epoch: info.compose_epoch ?? null,
    restorations: info.compose_restorations ?? [],
    text: info.compose_text || '',
    imageHashes: info.compose_images || [],
    mode: parseComposeMode(info.compose_mode),
    selection: info.compose_selection ?? null,
  }, batch);
}

const deletedThreadIds = new Set<string>();

export function noteThreadDeleted(threadId: string): void {
  deletedThreadIds.add(threadId);
}

export function upsertThread(
  map: Map<string, ThreadState>,
  info: ThreadSummary,
  saved: boolean,
  draftBatch?: DraftBatch,
): void {
  if (deletedThreadIds.has(info.thread_id)) return;
  if (!map.has(info.thread_id)) {
    map.set(info.thread_id, makeThreadState(info, saved, draftBatch));
  } else {
    const existing = map.get(info.thread_id)!;
    if (info.title && info.title !== PENDING_TITLE_PLACEHOLDER && !generatedTitleIds.has(info.thread_id)) existing.meta.title = info.title;
    if (info.created_at) existing.meta.createdAt = info.created_at;
    const apiTime = info.last_activity || info.created_at;
    if (apiTime && apiTime > existing.meta.updatedAt) existing.meta.updatedAt = apiTime;
    if (info.last_user_action && (!existing.meta.lastUserAction || info.last_user_action > existing.meta.lastUserAction)) existing.meta.lastUserAction = info.last_user_action;
    if (info.last_agent_action && (!existing.meta.lastAgentAction || info.last_agent_action > existing.meta.lastAgentAction)) existing.meta.lastAgentAction = info.last_agent_action;
    if (info.channel) existing.meta.channel = info.channel as ThreadMeta['channel'];
    if (info.initiator) existing.meta.initiator = info.initiator;
    if (info.home) existing.meta.home = true;
    const summaryIsCurrent = applySummaryVersion(existing.meta, info.summary_version, info.status);
    const unsettledFirstSend = firstSendUnsettled(existing, info.state);
    if (summaryIsCurrent) {
      liftRowIntent(existing.meta);
      if (info.message_count) existing.meta.messageCount = info.message_count;
      if (info.saved !== undefined) existing.meta.saved = info.saved;
      else if (saved) existing.meta.saved = true;
      if (info.section) existing.meta.section = info.section as ThreadMeta['section'];
      existing.meta.codingAgentChangeState = info.coding_agent_change_state;
      existing.meta.activeChildrenCount = info.active_children_count || 0;
      existing.meta.awaitedChildrenCount = info.awaited_children_count || 0;
      existing.meta.awaitedByParent = !!info.awaited_by_parent;
      existing.meta.totalChildrenCount = info.total_children_count || 0;
      existing.meta.blockingDescendantCount = info.blocking_descendant_count || 0;
      existing.meta.attentionDescendantCount = info.attention_descendant_count || 0;
      existing.meta.isStoppedChild = info.is_stopped_child === true;
      existing.meta.hold = info.hold ?? null;
      existing.meta.readRequested = info.read_requested === true;
      existing.meta.replySeenDevices = info.reply_seen_devices ?? [];
      existing.meta.applyInFlight = info.apply_in_flight === true;
      existing.meta.liveEventWaitCount = info.live_event_wait_count || 0;
      if (info.live_event_waits) existing.meta.liveEventWaits = info.live_event_waits;
      existing.meta.cacheKeepAliveStatus = info.cache_keep_alive_status ?? null;
      existing.meta.codingAgentIsExternalRepo = info.coding_agent_is_external_repo || false;
      if (info.last_revived_at) existing.meta.lastRevivedAt = info.last_revived_at;
      if (info.parent_thread_id !== undefined) existing.meta.parentThreadId = info.parent_thread_id ?? undefined;
      if (info.parent_thread_title !== undefined) existing.meta.parentThreadTitle = info.parent_thread_title ?? undefined;
      if (info.successor) existing.meta.successor = info.successor;
      if (info.successor_of) existing.meta.successorOf = info.successor_of;
      if (info.trigger_id) existing.meta.triggerId = info.trigger_id;
      if (info.trigger_name) existing.meta.triggerName = info.trigger_name;
      if (info.cc_repo_id) existing.meta.repoId = info.cc_repo_id;
      if (info.cc_repo_name) existing.meta.repoName = info.cc_repo_name;
      if (info.coding_agent_kind) existing.meta.codingAgentKind = info.coding_agent_kind;
      if (info.coding_agent_folder) existing.meta.codingAgentFolder = info.coding_agent_folder;
      if (info.coding_agent) existing.meta.codingAgent = info.coding_agent;
      if (info.coding_agents) existing.meta.codingAgents = info.coding_agents;
      if (!unsettledFirstSend) existing.meta.state = info.state;
      overlayRowIntent(existing.meta);
    }
    if (existing.meta.state !== 'discarded' && !unsettledFirstSend) stageDraftFromApi(info, draftBatch);
  }
}

const familyExtensionIds = new Set<string>();

export function _clearFamilyExtensionIdsForTest(): void {
  familyExtensionIds.clear();
}

let loadingAll = false;
let loadAllAgain = false;

export async function loadAllThreads(): Promise<boolean> {
  if (engineRestarting.value) return false;
  if (loadingAll) {
    loadAllAgain = true;
    return false;
  }
  loadingAll = true;

  try {
    do {
      loadAllAgain = false;
      await loadAllThreadsInner();
    } while (loadAllAgain && !engineRestarting.value);
    return true;
  } finally {
    loadingAll = false;
  }
}

export const filterFacetsSlice: SliceOf<'filter_facets'> = {
  begin: () => setLoadingIfFresh(filterFacets),
  apply: (data) => { filterFacets.value = { status: 'loaded', data }; },
  fail: (e) => { filterFacets.value = toFailed(e); },
};

export function loadFilterFacets(): Promise<void> {
  return loadSlice('filter_facets', filterFacetsSlice, () => fetchFilterFacets());
}

export function reloadFilterFacetsIfLoaded(): void {
  if (filterFacets.value.status !== 'not-loaded') void loadFilterFacets();
}

const facetReloadsAsked = new Set<string>();

export function reloadFacetsForApps(ids: readonly string[]): void {
  const fresh = ids.filter(id => !facetReloadsAsked.has(id));
  if (fresh.length === 0) return;
  for (const id of fresh) facetReloadsAsked.add(id);
  void loadFilterFacets();
}

async function loadAllThreadsInner(): Promise<void> {
  const focused = focusedThreadId.value;

  const response = await fetchThreads(focused || undefined);

  const map = threadMap.value;
  const activeSet = new Set(response.active);
  const draftBatch: DraftBatch = new Map();

  for (const info of response.saved) {
    upsertThread(map, info, true, draftBatch);
    familyExtensionIds.delete(info.thread_id);
  }
  for (const info of [...response.active_threads, ...response.archive, ...response.composing]) {
    upsertThread(map, info, false, draftBatch);
    familyExtensionIds.delete(info.thread_id);
  }
  if (response.home_thread) {
    upsertThread(map, response.home_thread, false, draftBatch);
    familyExtensionIds.delete(response.home_thread.thread_id);
  }
  if (response.focused_thread) {
    upsertThread(map, response.focused_thread, false, draftBatch);
    familyExtensionIds.delete(response.focused_thread.thread_id);
  }
  for (const info of response.family_threads ?? []) {
    upsertThread(map, info, false, draftBatch);
    familyExtensionIds.add(info.thread_id);
  }

  let ghostFocusedThread: string | null = null;
  if (focused && !map.has(focused) && focusedThreadId.peek() === focused
      && awaitedThreadId.peek() !== focused) {
    ghostFocusedThread = focused;
    setFocusedThread(null);
    postClientLog('lifecycle', 'cleared_ghost_focus', { thread_id: focused });
  }

  applyDraftBatch(draftBatch);
  threadMap.value = new Map(map);
  threadsLoaded.value = true;
  threadListFetched.value = true;
  if (loadedFilterSelection === null) stampLoadedFilterSelection(appliedThreadFilter.value);
  const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();
  if (sources || triggerIds || repoIds || appIds) {
    void refreshArchivedCount();
  } else {
    archivedCountGeneration++;
    archiveThreadCount.value = response.archive_count ?? 0;
  }

  void loadFilterFacets();

  if (focused && focused !== ghostFocusedThread) await loadThreadEvents(focused);

  const prefetchIds = [...map.values()]
    .filter(t => t.meta.id !== focused && (activeSet.has(t.meta.id) || t.meta.saved))
    .sort((a, b) => Date.parse(b.meta.updatedAt) - Date.parse(a.meta.updatedAt))
    .slice(0, THREAD_EVENTS_PREFETCH_LIMIT)
    .map(t => t.meta.id);
  await runWithConcurrency(prefetchIds, THREAD_EVENTS_FETCH_CONCURRENCY, loadThreadEvents);

  if (focused) {
    const focusedThread = map.get(focused);
    if (focusedThread?.meta.channel === CODING_AGENT_CHANNEL) {
      codingAgentSessionVersion.value++;
    }
  }
}

export async function ensureThreadInMap(info: ThreadSummary): Promise<void> {
  const map = threadMap.value;
  if (!map.has(info.thread_id)) {
    upsertThread(map, info, false);
    threadMap.value = new Map(map);
  }
  await loadThreadEvents(info.thread_id);
}

export async function ensureThreadByIdInMap(threadId: string): Promise<boolean> {
  if (threadMap.value.has(threadId)) return true;
  const info = await fetchThreadById(threadId);
  if (!info) return false;
  const map = threadMap.value;
  upsertThread(map, info, info.saved === true);
  threadMap.value = new Map(map);
  return true;
}

let fetchAttemptSeq = 0;

const loadingThreads = new Map<string, { token: number; startedAt: number }>();

export function threadLoadInFlightMs(threadId: string): number | null {
  const attempt = loadingThreads.get(threadId);
  return attempt ? Date.now() - attempt.startedAt : null;
}

const refreshAttempts = new Map<string, number>();

const lastRefreshReport = new Map<string, number>();

const forcedRetries = new Set<string>();

export function threadEventsStillArriving(threadId: string): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.eventsLoadFailed) return false;
  return !thread.eventsLoaded
    || loadingThreads.has(threadId)
    || refreshAttempts.has(threadId)
    || historyReadInFlight.has(threadId);
}

let staleThreadEvents = new Set<string>();

let staleMarkedAtToken = 0;

export function markLoadedThreadsStale(): void {
  const next = new Set<string>();
  for (const [id, thread] of threadMap.value) {
    if (thread.eventsLoaded || loadingThreads.has(id)) next.add(id);
  }
  staleThreadEvents = next;
  staleMarkedAtToken = fetchAttemptSeq;
}

function clearStaleMark(threadId: string, attemptToken: number): void {
  if (staleMarkedAtToken >= attemptToken) return;
  staleThreadEvents.delete(threadId);
}

function mayCoalesceIntoLiveRefresh(threadId: string): boolean {
  const live = refreshAttempts.get(threadId);
  return live !== undefined && live > staleMarkedAtToken;
}

export function refreshStaleThreadEvents(threadId: string): void {
  if (!staleThreadEvents.has(threadId)) return;
  void refreshThreadEvents(threadId, { coalesce: true });
}

export function retryStaleOpenThreadEvents(threadId: string): boolean {
  if (!staleThreadEvents.has(threadId) || REFRESH_FAILURES.failing.has(threadId)) return false;
  refreshStaleThreadEvents(threadId);
  return true;
}

export function _resetStaleThreadEventsForTesting(): void {
  staleThreadEvents = new Set();
  staleMarkedAtToken = 0;
}

export function clearThreadFetchGuards(): void {
  forcedRetries.clear();
  loadingThreads.clear();
  refreshAttempts.clear();
}

export function forceRetryThreadEvents(threadId: string): void {
  if (forcedRetries.has(threadId)) return;
  if (engineRestarting.value) return;
  forcedRetries.add(threadId);
  restartThreadEventsLoad(threadId);
}

export function retryThreadEvents(threadId: string): void {
  if (engineRestarting.value) return;
  restartThreadEventsLoad(threadId);
}

function restartThreadEventsLoad(threadId: string): void {
  loadingThreads.delete(threadId);
  const thread = threadMap.value.get(threadId);
  if (thread) {
    thread.eventsLoaded = false;
    thread.eventsLoadFailed = false;
  }
  loadThreadEvents(threadId);
}

type ThreadEventsFailures = {
  key: string;
  failing: Map<string, string>;
  describe: (subject: string, detail: string) => string;
};

const REFRESH_FAILURES: ThreadEventsFailures = {
  key: THREAD_EVENTS_REFRESH_TOAST_KEY,
  failing: new Map(),
  describe: (subject, detail) => `Failed to refresh thread events for ${subject}: ${detail}`,
};

const LOAD_FAILURES: ThreadEventsFailures = {
  key: THREAD_EVENTS_LOAD_TOAST_KEY,
  failing: new Map(),
  describe: (subject, detail) => `Failed to load thread events for ${subject}: ${detail}`,
};

export function _resetThreadEventsFailuresForTesting(): void {
  REFRESH_FAILURES.failing.clear();
  LOAD_FAILURES.failing.clear();
}

export function forgetThreadEventsFailures(threadId: string): void {
  clearThreadEventsFailure(REFRESH_FAILURES, threadId);
  clearThreadEventsFailure(LOAD_FAILURES, threadId);
}

function dropDepartedThreads(surface: ThreadEventsFailures): void {
  for (const id of surface.failing.keys()) {
    if (!threadMap.value.has(id)) surface.failing.delete(id);
  }
}

function renderThreadEventsCard(surface: ThreadEventsFailures, raise: boolean): void {
  const ids = [...surface.failing.keys()];
  if (ids.length === 0) {
    removeToast(surface.key);
    return;
  }
  if (!raise && !toasts.value.some(t => t.key === surface.key)) return;
  const detail = surface.failing.get(ids[ids.length - 1])!;
  const only = ids.length === 1 ? threadMap.value.get(ids[0]) : undefined;
  const title = only && only.meta.title !== PENDING_TITLE_PLACEHOLDER ? only.meta.title : '';
  const subject = title ? `"${title}"` : `${ids.length} thread${ids.length === 1 ? '' : 's'}`;
  showToast(surface.describe(subject, detail), 'error', { key: surface.key });
}

function recordThreadEventsFailure(surface: ThreadEventsFailures, threadId: string, err: unknown): void {
  if (connectionStatus.value !== 'connected') return;
  surface.failing.delete(threadId);
  surface.failing.set(threadId, errorDetail(err));
  dropDepartedThreads(surface);
  renderThreadEventsCard(surface, true);
}

function clearThreadEventsFailure(surface: ThreadEventsFailures, threadId: string): void {
  const before = surface.failing.size;
  surface.failing.delete(threadId);
  dropDepartedThreads(surface);
  if (surface.failing.size !== before) renderThreadEventsCard(surface, false);
}

export async function loadThreadEvents(threadId: string): Promise<void> {
  if (engineRestarting.value) return;
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.eventsLoaded) return;
  if (loadingThreads.has(threadId)) return;
  const attemptToken = ++fetchAttemptSeq;
  loadingThreads.set(threadId, { token: attemptToken, startedAt: Date.now() });

  if (threadId === focusedThreadId.value) {
    markThreadOpenStart(threadId, { ...currentPerfBaseline(), warm: false });
  }

  if (thread.eventsLoadFailed) {
    thread.eventsLoadFailed = false;
    threadMap.value = new Map(threadMap.value);
  }

  const MAX_RETRIES = 2;
  const BASE_DELAY = 1000;

  try {
    for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
      try {
        const fetchStart = performance.now();
        const snapshot = await fetchThreadEvents(threadId, { limit: THREAD_EVENTS_PAGE_SIZE });
        const fetchMs = performance.now() - fetchStart;
        const current = threadMap.value.get(threadId);
        if (!current) return;
        const applyStart = performance.now();
        applyEventRows(threadMap.value, threadId, current, snapshot.events, snapshot.currentAggregate);
        const applyMs = performance.now() - applyStart;
        noteHistoryFloor(current, snapshot);
        if (snapshot.maxSequence !== undefined) {
          current.lastDbSeq = Math.max(current.lastDbSeq, snapshot.maxSequence);
        }
        current.eventsLoaded = true;
        current.eventsLoadFailed = false;
        threadMap.value = new Map(threadMap.value);
        recordPerfSample('thread-load', {
          threadId,
          eventCount: snapshot.events.length,
          channel: current.meta.channel,
          fetchMs: Math.round(fetchMs),
          applyMs: Math.round(applyMs),
          paged: current.hasOlderEvents,
        });
        lastRefreshReport.set(threadId, Math.max(lastRefreshReport.get(threadId) ?? 0, attemptToken));
        forgetThreadEventsFailures(threadId);
        clearStaleMark(threadId, attemptToken);
        if (threadId === focusedThreadId.value) refreshStaleThreadEvents(threadId);
        return;
      } catch (err) {
        if (attempt < MAX_RETRIES) {
          await new Promise(r => setTimeout(r, BASE_DELAY * (attempt + 1)));
        } else {
          if (engineRestarting.value) {
            const restarting = threadMap.value.get(threadId);
            if (restarting) {
              restarting.eventsLoadFailed = true;
              threadMap.value = new Map(threadMap.value);
            }
            return;
          }
          if (threadMap.value.get(threadId)?.eventsLoaded) return;
          console.warn(`[ThreadLoading] Failed to load events for ${threadId} after ${MAX_RETRIES + 1} attempts:`, err);
          if (!isTransientFetchError(err)) recordThreadEventsFailure(LOAD_FAILURES, threadId, err);
          const current = threadMap.value.get(threadId);
          if (current) {
            current.eventsLoadFailed = true;
            threadMap.value = new Map(threadMap.value);
          }
          return;
        }
      }
    }
  } finally {
    if (loadingThreads.get(threadId)?.token === attemptToken) loadingThreads.delete(threadId);
  }
}

export async function refreshThreadEvents(
  threadId: string,
  opts: { coalesce?: boolean } = {},
): Promise<boolean> {
  if (engineRestarting.value) return false;
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (!thread || !thread.eventsLoaded) return false;
  if (opts.coalesce === true && mayCoalesceIntoLiveRefresh(threadId)) return false;
  const attemptToken = ++fetchAttemptSeq;
  refreshAttempts.set(threadId, attemptToken);
  const mayReport = () => (lastRefreshReport.get(threadId) ?? 0) < attemptToken;
  const markReported = () => lastRefreshReport.set(threadId, attemptToken);

  try {
    const snapshot = await retryTransient(() => fetchThreadEvents(threadId, thread.lastDbSeq || undefined));
    const live = threadMap.value.get(threadId);
    if (live && (snapshot.events.length > 0 || snapshot.currentAggregate)) {
      applyEventRows(threadMap.value, threadId, live, snapshot.events, snapshot.currentAggregate);
      threadMap.value = new Map(threadMap.value);
    }
    clearStaleMark(threadId, attemptToken);
    if (mayReport()) {
      markReported();
      clearThreadEventsFailure(REFRESH_FAILURES, threadId);
    }
    return true;
  } catch (err) {
    if (engineRestarting.value) return false;
    if (isTransientFetchError(err)) {
      console.warn(`[ThreadLoading] refresh failed transiently for ${threadId} (iOS PWA wake / engine restart); the health poll retries`, err);
      return false;
    }
    console.warn(`[ThreadLoading] Failed to refresh events for ${threadId}:`, err);
    if (mayReport()) {
      markReported();
      recordThreadEventsFailure(REFRESH_FAILURES, threadId, err);
    }
    return false;
  } finally {
    if (refreshAttempts.get(threadId) === attemptToken) refreshAttempts.delete(threadId);
  }
}

export async function reloadAfterFilterChange(): Promise<void> {
  threadHasMore.value = true;
  void refreshArchivedCount();
  await loadOlderThreads();
}

let loadedFilterSelection: ThreadFilterSelection | null = null;

function stampLoadedFilterSelection(selection: ThreadFilterSelection): void {
  loadedFilterSelection = selection;
}

export function filterChangedSinceLoad(): boolean {
  return loadedFilterSelection !== appliedThreadFilter.value;
}

export function _clearLoadedFilterSelectionForTest(): void {
  loadedFilterSelection = null;
}

let olderThreadsCursor: { selection: ThreadFilterSelection; before: string } | null = null;

export function _clearOlderThreadsCursorForTest(): void {
  olderThreadsCursor = null;
}

function earlierCursor(a: string | null, b: string | null): string | null {
  if (!a) return b;
  if (!b) return a;
  return Date.parse(a) <= Date.parse(b) ? a : b;
}

export function currentThreadFilterParams(): {
  sources: ThreadFilterSource[] | undefined;
  triggerIds: string[] | undefined;
  repoIds: string[] | undefined;
  appIds: string[] | undefined;
} {
  const applied = appliedThreadFilter.value;
  const isFiltered = applied.channels.size < ALL_CHANNELS.length;
  return {
    sources: isFiltered ? [...applied.channels].map(threadChannelToFilterSource) : undefined,
    triggerIds: applied.triggerIds.size > 0 ? [...applied.triggerIds] : undefined,
    repoIds: applied.repoIds.size > 0 ? [...applied.repoIds] : undefined,
    appIds: applied.appIds.size > 0 ? [...applied.appIds] : undefined,
  };
}

export async function refreshArchivedCount(): Promise<void> {
  const generation = ++archivedCountGeneration;
  if (appliedThreadFilter.value.channels.size === 0) {
    archiveThreadCount.value = 0;
    return;
  }
  const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();
  try {
    const count = await fetchArchivedCount(sources, triggerIds, repoIds, appIds);
    if (generation === archivedCountGeneration) archiveThreadCount.value = count;
  } catch (err) {
    console.warn('[threads] archived-count refresh failed:', errorDetail(err));
  }
}

let archivedCountGeneration = 0;
let archivedCountRefresh: Promise<void> | null = null;
let archivedCountStale = false;

export function refreshArchivedCountAfterFlip(): void {
  archivedCountStale = true;
  if (archivedCountRefresh) return;
  archivedCountRefresh = (async () => {
    while (archivedCountStale) {
      archivedCountStale = false;
      await refreshArchivedCount();
    }
  })().finally(() => { archivedCountRefresh = null; });
}

export async function loadOlderThreads(): Promise<boolean> {
  if (threadLoadingMore.value || !threadHasMore.value) return false;
  const applied = appliedThreadFilter.value;
  if (applied.channels.size === 0) {
    threadHasMore.value = false;
    stampLoadedFilterSelection(applied);
    return false;
  }
  threadLoadingMore.value = true;

  try {
    const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();

    let oldestTime: string | null = null;
    for (const t of threadMap.value.values()) {
      if (t.meta.saved) continue;
      if (t.meta.section !== 'archived') continue;
      if (familyExtensionIds.has(t.meta.id)) continue;
      if (isExcludedFromSections(t)) continue;
      if (!threadPassesChannelFilter(t, applied.channels, applied.triggerIds, applied.repoIds, applied.appIds)) continue;
      const key = createdKey(t);
      if (!oldestTime || key < oldestTime) oldestTime = key;
    }

    if (olderThreadsCursor?.selection === applied) {
      oldestTime = earlierCursor(oldestTime, olderThreadsCursor.before);
    }

    if (!oldestTime) {
      if (!sources && !triggerIds && !repoIds && !appIds) {
        threadHasMore.value = false;
        stampLoadedFilterSelection(applied);
        return false;
      }
      oldestTime = new Date().toISOString();
    }

    const response = await fetchOlderThreads(oldestTime, 15, sources, triggerIds, repoIds, appIds);
    const map = threadMap.value;
    stampLoadedFilterSelection(applied);
    const stillCurrent = appliedThreadFilter.value === applied;
    if (response.threads.length === 0) {
      if (stillCurrent) threadHasMore.value = false;
      return true;
    }
    const pageOldest = response.threads[response.threads.length - 1].created_at;
    if (pageOldest) olderThreadsCursor = { selection: applied, before: pageOldest };

    let mapChanged = false;
    for (const info of response.threads) {
      if (!map.has(info.thread_id)) {
        upsertThread(map, info, false);
        mapChanged = true;
      }
      familyExtensionIds.delete(info.thread_id);
    }
    for (const info of response.family_threads ?? []) {
      if (!map.has(info.thread_id)) {
        upsertThread(map, info, false);
        familyExtensionIds.add(info.thread_id);
        mapChanged = true;
      }
    }

    if (stillCurrent) threadHasMore.value = response.has_more;
    if (mapChanged) threadMap.value = new Map(map);
    return true;
  } catch (err) {
    showToast(`Failed to load more threads: ${errorDetail(err)}`, 'error');
    return false;
  } finally {
    threadLoadingMore.value = false;
  }
}

export function _applyEventRowsForTest(
  map: Map<string, ThreadState>,
  thread: ThreadState,
  rows: ThreadEventRow[],
): void {
  applyEventRows(map, thread.meta.id, thread, rows, null);
}

export const THREAD_EVENTS_PAGE_SIZE = 400;

function noteHistoryFloor(thread: ThreadState, snapshot: ThreadEventsSnapshot): void {
  const oldest = snapshot.events[0];
  if (oldest) thread.historyFloor = { created: oldest.created, sequence: oldest.sequence };
  thread.hasOlderEvents = snapshot.hasMore === true;
  if (snapshot.pairStamps) thread.pairStamps = snapshot.pairStamps;
  if (snapshot.cardSizes) thread.cardSizes = snapshot.cardSizes;
}

function prependEventRows(thread: ThreadState, rows: ThreadEventRow[]): boolean {
  if (rows.length === 0) return false;
  const merged = new Map<number, StoredEvent>();
  for (const row of rows) {
    if (thread.events.has(row.sequence)) continue;
    merged.set(row.sequence, storedFromRow(row));
  }
  if (merged.size === 0) return false;
  for (const [seq, stored] of thread.events) merged.set(seq, stored);
  thread.events = merged;
  return true;
}

function storedFromRow(row: ThreadEventRow): StoredEvent {
  return {
    type: row.event_type,
    ...row.payload,
    created: row.created,
    ...(row.event_id ? { _eventId: row.event_id } : {}),
  } as StoredEvent;
}

const historyReadInFlight = new Map<string, Promise<void>>();

export function _resetHistoryReadsForTesting(): void {
  historyReadInFlight.clear();
}

export function threadHistoryReadInFlight(threadId: string): boolean {
  return historyReadInFlight.has(threadId);
}

async function afterHistoryRead(threadId: string, read: () => Promise<void>): Promise<void> {
  const prior = historyReadInFlight.get(threadId) ?? Promise.resolve();
  const run = prior.catch(() => {}).then(read);
  historyReadInFlight.set(threadId, run);
  try {
    await run;
  } finally {
    if (historyReadInFlight.get(threadId) === run) historyReadInFlight.delete(threadId);
  }
}

export type HistoryLandingGate = () => Promise<void>;

export async function loadOlderThreadEvents(
  threadId: string,
  pageSize: number = THREAD_EVENTS_PAGE_SIZE,
  landWhen?: HistoryLandingGate,
): Promise<boolean> {
  if (historyReadInFlight.has(threadId)) return false;
  let added = false;
  await afterHistoryRead(threadId, () => backfillOnePage(threadId, pageSize, landWhen).then(r => { added = r; }));
  return added;
}

async function backfillOnePage(threadId: string, pageSize: number, landWhen?: HistoryLandingGate): Promise<boolean> {
  const thread = threadMap.value.get(threadId);
  if (!thread || !thread.hasOlderEvents) return false;
  const floor = thread.historyFloor;
  if (!floor) return false;
  try {
    const fetchStart = performance.now();
    const snapshot = await retryTransient(() => fetchThreadEvents(threadId, {
      limit: pageSize,
      before: floor,
    }));
    const fetchMs = performance.now() - fetchStart;
    await landWhen?.();
    const current = threadMap.value.get(threadId);
    if (!current) return false;
    const applyStart = performance.now();
    const grew = prependEventRows(current, snapshot.events);
    const applyMs = performance.now() - applyStart;
    noteHistoryFloor(current, snapshot);
    threadMap.value = new Map(threadMap.value);
    bumpThreadEvents(threadId);
    recordPerfSample('thread-backfill', {
      threadId,
      eventCount: snapshot.events.length,
      fetchMs: Math.round(fetchMs),
      applyMs: Math.round(applyMs),
      hasMore: current.hasOlderEvents,
    });
    return grew;
  } catch {
    showToast('Could not load older messages. Scroll up again to retry.', 'error');
    return false;
  }
}

export async function ensureWholeThreadLoaded(threadId: string, landWhen?: HistoryLandingGate): Promise<boolean> {
  let added = false;
  await afterHistoryRead(threadId, () => loadWholeThread(threadId, landWhen).then(r => { added = r; }));
  return added;
}

async function loadWholeThread(threadId: string, landWhen?: HistoryLandingGate): Promise<boolean> {
  const thread = threadMap.value.get(threadId);
  if (!thread || !thread.hasOlderEvents) return false;
  try {
    const snapshot = await retryTransient(() => fetchThreadEvents(threadId));
    await landWhen?.();
    const current = threadMap.value.get(threadId);
    if (!current) return false;
    const grew = prependEventRows(current, snapshot.events);
    noteHistoryFloor(current, snapshot);
    threadMap.value = new Map(threadMap.value);
    bumpThreadEvents(threadId);
    return grew;
  } catch {
    showToast('Could not load the rest of this thread.', 'error');
    return false;
  }
}

function applyEventRows(
  map: Map<string, ThreadState>,
  threadId: string,
  thread: ThreadState,
  rows: ThreadEventRow[],
  currentAggregate: ThreadAggregate | null,
): void {
  const liveUtterances = thread.liveUtterances;
  const unclaimedUtterances = thread.unclaimedUtterances;
  const liveReply = thread.liveReply;
  for (const row of rows) {
    const event = { type: row.event_type, ...row.payload } as ThreadEvent;
    const handled = handleEvent(map, threadId, row.sequence, event, row.created, row.event_id);
    if (handled.retiredUnsentEventId) settleDeliveredUnsentMessage(handled.retiredUnsentEventId);
    if (event.type === 'ImageUploaded') noteImageLanded(threadId, event.hash);
    if (row.sequence > thread.lastDbSeq) thread.lastDbSeq = row.sequence;

    if ((row.event_type === 'ThreadTitleGenerated' || row.event_type === 'ThreadTitleRenamed') && row.payload.title) {
      thread.meta.title = row.payload.title as string;
      generatedTitleIds.add(threadId);
    }
    if (isChannelDefiningEvent(row.event_type) && row.payload.channel) {
      thread.meta.channel = row.payload.channel as ThreadMeta['channel'];
    }
  }
  thread.liveUtterances = liveUtterances;
  thread.unclaimedUtterances = unclaimedUtterances;
  thread.liveReply = liveReply;
  for (const row of rows) {
    if (!isCallerUtterance({ type: row.event_type, ...row.payload })) continue;
    offerCallerUtterance(thread, (row.payload as { text?: string }).text ?? '');
  }
  if (currentAggregate) {
    const current = isSummaryCurrent(thread.meta, currentAggregate.summaryVersion);
    if (current) liftRowIntent(thread.meta);
    applyAggregateToThread(thread, currentAggregate);
    if (current) overlayRowIntent(thread.meta);
  }

  if (rows.length > 0) bumpThreadEvents(threadId);
}
