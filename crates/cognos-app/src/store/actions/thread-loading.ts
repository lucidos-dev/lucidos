import { threadMap, focusedThreadId, showToast, threadsLoaded, generatedTitleIds, threadHasMore, threadLoadingMore, ALL_CHANNELS, type ThreadChannel, threadChannelFilter, ccSessionVersion, engineRestarting } from '../store';
import { handleEvent, isChannelDefiningEvent, type ThreadState, type ThreadEvent, type ThreadMeta, type ThreadStatus } from '../thread-events';
import { fetchThreads, fetchThreadEvents, fetchOlderThreads } from '../../api/threads';
import type { ThreadInfo, ThreadEventRow } from '../../api/threads';
import { errorDetail } from '../../utils/errorDetail';

function makeThreadState(info: ThreadInfo, pinned: boolean): ThreadState {
  return {
    meta: {
      id: info.thread_id,
      title: info.title || '...',
      channel: info.channel as ThreadMeta['channel'],
      initiator: info.initiator,
      pinned,
      createdAt: info.created_at || new Date().toISOString(),
      updatedAt: info.last_activity || info.created_at || new Date().toISOString(),
      unread: false,
      status: (info.status as ThreadStatus) || 'idle',
      messageCount: info.message_count || 0,
      section: (info.section as ThreadMeta['section']) || 'default',
      activeChildrenCount: info.active_children_count || 0,
      totalChildrenCount: info.total_children_count || 0,
      ccHasChanges: info.cc_has_changes || false,
      ccRequiresRestart: info.cc_requires_restart || false,
      ccIsExternalRepo: info.cc_is_external_repo || false,
      ccApplying: info.cc_applying || false,
      lastRevivedAt: info.last_revived_at || '',
      parentThreadId: info.parent_thread_id || undefined,
      parentThreadTitle: info.parent_thread_title || undefined,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/** Insert or update a thread in the map from API metadata. Exported for testing. */
export function upsertThread(map: Map<string, ThreadState>, info: ThreadInfo, pinned: boolean): void {
  if (!map.has(info.thread_id)) {
    map.set(info.thread_id, makeThreadState(info, pinned));
  } else {
    const existing = map.get(info.thread_id)!;
    if (pinned) existing.meta.pinned = true;
    if (info.title && info.title !== '...' && !generatedTitleIds.has(info.thread_id)) existing.meta.title = info.title;
    if (info.created_at) existing.meta.createdAt = info.created_at;
    const apiTime = info.last_activity || info.created_at;
    if (apiTime && apiTime > existing.meta.updatedAt) existing.meta.updatedAt = apiTime;
    if (info.channel) existing.meta.channel = info.channel as ThreadMeta['channel'];
    if (info.initiator) existing.meta.initiator = info.initiator;
    // Update status from API — the backend is authoritative.
    if (info.status) existing.meta.status = info.status as ThreadStatus;
    if (info.message_count) existing.meta.messageCount = info.message_count;
    if (info.section) existing.meta.section = info.section as ThreadMeta['section'];
    existing.meta.activeChildrenCount = info.active_children_count || 0;
    existing.meta.totalChildrenCount = info.total_children_count || 0;
    // Update CC state fields from API
    existing.meta.ccHasChanges = info.cc_has_changes || false;
    existing.meta.ccRequiresRestart = info.cc_requires_restart || false;
    existing.meta.ccIsExternalRepo = info.cc_is_external_repo || false;
    existing.meta.ccApplying = info.cc_applying || false;
    if (info.last_revived_at) existing.meta.lastRevivedAt = info.last_revived_at;
    if (info.parent_thread_id) existing.meta.parentThreadId = info.parent_thread_id;
    if (info.parent_thread_title) existing.meta.parentThreadTitle = info.parent_thread_title;
  }
}

/** Prevents duplicate concurrent loadAllThreads calls (e.g. 5s health poll + resume). */
let loadingAll = false;

/** Load thread list metadata, then lazy-load events by priority. */
export async function loadAllThreads(): Promise<void> {
  if (loadingAll || engineRestarting.value) return;
  loadingAll = true;

  try {
    await loadAllThreadsInner();
  } finally {
    loadingAll = false;
  }
}

async function loadAllThreadsInner(): Promise<void> {
  const focused = focusedThreadId.value;

  const response = await fetchThreads(focused || undefined);

  const map = threadMap.value;
  const activeSet = new Set(response.active);

  // Build/update thread states from metadata
  for (const info of response.pinned) {
    upsertThread(map, info, true);
  }
  for (const info of [...response.active_threads, ...response.history]) {
    upsertThread(map, info, false);
  }
  // Focused thread may be too old for recent list, not pinned, not active
  if (response.focused_thread) {
    upsertThread(map, response.focused_thread, false);
  }

  threadMap.value = new Map(map);
  threadsLoaded.value = true;

  // Load events for focused, active, and pinned threads; others load lazily
  // on focus. Pinned threads are always few and need events for correct
  // drawer status (e.g. waiting CC threads after engine restart).
  const loads: Promise<void>[] = [];
  if (focused) loads.push(loadThreadEvents(focused));
  for (const t of map.values()) {
    if (t.meta.id !== focused && (activeSet.has(t.meta.id) || t.meta.pinned)) {
      loads.push(loadThreadEvents(t.meta.id));
    }
  }
  if (loads.length > 0) await Promise.all(loads);

  // Bump so CCControlMenu re-fetches commands after reload (SSE doesn't replay).
  if (focused) {
    const focusedThread = map.get(focused);
    if (focusedThread?.meta.channel === 'claude_code') {
      ccSessionVersion.value++;
    }
  }
}

/** Ensure a thread exists in threadMap, bootstrapping from metadata if needed, then load its events. */
export async function ensureThreadInMap(info: ThreadInfo): Promise<void> {
  const map = threadMap.value;
  if (!map.has(info.thread_id)) {
    upsertThread(map, info, false);
    threadMap.value = new Map(map);
  }
  await loadThreadEvents(info.thread_id);
}

/** Tracks threads with an in-flight load to prevent duplicate concurrent fetches. */
const loadingThreads = new Set<string>();

/** Tracks threads that have already been force-retried by the watchdog.
 *  Prevents infinite retry loops on persistent failures.
 *  Cleared on resume so threads can be retried after iOS suspend/resume. */
const forcedRetries = new Set<string>();

/** Clear forced-retry tracking so the watchdog can retry threads again.
 *  Called on iOS PWA resume — stale retry caps prevent recovery after
 *  suspend/resume cycles where transient failures may have cleared.
 *  Also clears loadingThreads — on iOS, setTimeout timers can be paused
 *  or cleared during suspension, causing in-flight fetches to hang forever
 *  and permanently block those threads from loading. */
export function clearForcedRetries(): void {
  forcedRetries.clear();
  loadingThreads.clear();
}

/** Force-retry event loading for a stuck thread. Used by the watchdog in
 *  ThreadView when a thread has no content for >2 seconds.
 *  - Skips if this thread was already force-retried (prevents infinite loop)
 *  - Clears loadingThreads for this thread — the watchdog fires BECAUSE
 *    loading is stuck, so the in-flight fetch is likely hung (iOS suspension
 *    can pause setTimeout timers, leaving fetch Promises unresolved forever).
 *    A duplicate fetch is safe: applyEventRows is idempotent (checks lastDbSeq).
 *  - Resets eventsLoaded so loadThreadEvents doesn't early-return */
export function forceRetryThreadEvents(threadId: string): void {
  if (forcedRetries.has(threadId)) return;
  forcedRetries.add(threadId);
  loadingThreads.delete(threadId);
  const thread = threadMap.value.get(threadId);
  if (thread) {
    thread.eventsLoaded = false;
    thread.eventsLoadFailed = false;
  }
  loadThreadEvents(threadId);
}

/** Load events for a single thread from the snapshot endpoint.
 *  Retries up to 2 times with exponential backoff on failure. */
export async function loadThreadEvents(threadId: string): Promise<void> {
  if (engineRestarting.value) return;
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.eventsLoaded) return;
  if (loadingThreads.has(threadId)) return;
  loadingThreads.add(threadId);

  // Clear any prior failure so the UI shows the loading state again
  if (thread.eventsLoadFailed) {
    thread.eventsLoadFailed = false;
    threadMap.value = new Map(threadMap.value);
  }

  const MAX_RETRIES = 2;
  const BASE_DELAY = 1000;

  try {
    for (let attempt = 0; attempt <= MAX_RETRIES; attempt++) {
      try {
        const rows = await fetchThreadEvents(threadId);
        // Re-read from current map — the map reference may have changed
        // during the async fetch (other threads loaded/updated).
        const current = threadMap.value.get(threadId);
        if (!current) return;
        applyEventRows(threadMap.value, threadId, current, rows, true);
        current.eventsLoaded = true;
        threadMap.value = new Map(threadMap.value);
        return;
      } catch (err) {
        if (attempt < MAX_RETRIES) {
          // Retry all errors including timeouts — iOS Safari PWA can cause
          // transient AbortErrors during suspend/resume that succeed on retry.
          await new Promise(r => setTimeout(r, BASE_DELAY * (attempt + 1)));
        } else {
          if (engineRestarting.value) return;
          console.warn(`[ThreadLoading] Failed to load events for ${threadId} after ${MAX_RETRIES + 1} attempts:`, err);
          const title = threadMap.value.get(threadId)?.meta.title || threadId.slice(0, 8);
          showToast(`Failed to load thread events for "${title}": ${errorDetail(err)}`, 'error');
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
    loadingThreads.delete(threadId);
  }
}

/** Fetch only new events since the thread's last DB-loaded sequence and append them. */
export async function refreshThreadEvents(threadId: string): Promise<void> {
  if (engineRestarting.value) return;
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (!thread || !thread.eventsLoaded) return;

  try {
    const rows = await fetchThreadEvents(threadId, thread.lastDbSeq || undefined);
    if (rows.length > 0) {
      applyEventRows(map, threadId, thread, rows);
      threadMap.value = new Map(threadMap.value);
    }
  } catch (err) {
    if (engineRestarting.value) return;
    console.warn(`[ThreadLoading] Failed to refresh events for ${threadId}:`, err);
    const title = threadMap.value.get(threadId)?.meta.title || threadId.slice(0, 8);
    showToast(`Failed to refresh thread events for "${title}": ${errorDetail(err)}`, 'error');
  }
}

/** Load older threads for infinite scroll. Self-guards against concurrent calls.
 *  Passes the active source filter to the API so only matching threads are returned. */
export async function loadOlderThreads(): Promise<void> {
  if (threadLoadingMore.value || !threadHasMore.value) return;
  threadLoadingMore.value = true;

  try {
    const map = threadMap.value;
    const filter = threadChannelFilter.value;
    const isFiltered = filter.size < ALL_CHANNELS.length;
    const sources = isFiltered ? [...filter] : undefined;

    // Find the oldest updatedAt among non-pinned history threads
    // that match the current filter, so the cursor skips over filtered-out threads.
    let oldestTime: string | null = null;

    for (const t of map.values()) {
      if (t.meta.pinned) continue;
      if (isFiltered && !filter.has(t.meta.channel as ThreadChannel)) continue;
      if (!oldestTime || t.meta.updatedAt < oldestTime) {
        oldestTime = t.meta.updatedAt;
      }
    }

    if (!oldestTime) {
      threadHasMore.value = false;
      return;
    }

    const response = await fetchOlderThreads(oldestTime, 15, sources);
    if (response.threads.length === 0) {
      threadHasMore.value = false;
      return;
    }

    let added = 0;
    for (const info of response.threads) {
      if (!map.has(info.thread_id)) {
        upsertThread(map, info, false);
        added++;
      }
    }

    // If server returned results but all were duplicates, treat as exhausted
    // to prevent infinite fetch loops with the same cursor.
    if (added === 0) {
      threadHasMore.value = false;
      return;
    }

    threadHasMore.value = response.has_more;
    threadMap.value = new Map(map);
  } catch (err) {
    showToast(`Failed to load more threads: ${errorDetail(err)}`, 'error');
  } finally {
    threadLoadingMore.value = false;
  }
}

function applyEventRows(
  map: Map<string, ThreadState>,
  threadId: string,
  thread: ThreadState,
  rows: ThreadEventRow[],
  initialLoad = false,
): void {
  // On initial load, preserve API-authoritative fields — thread_summaries is the
  // source of truth. Event replay can override these with stale values (e.g., a
  // CodingAgentPromptSent that set status='running' but the session crashed without
  // a terminal event). On incremental refresh, events ARE the truth — don't override.
  const apiSection = initialLoad ? thread.meta.section : null;
  const apiStatus = initialLoad ? thread.meta.status : null;
  const apiChannel = initialLoad ? thread.meta.channel : null;
  for (const row of rows) {
    const event = { type: row.event_type, ...row.payload } as ThreadEvent;
    handleEvent(map, threadId, row.sequence, event, row.created);
    if (row.sequence > thread.lastDbSeq) thread.lastDbSeq = row.sequence;

    if ((row.event_type === 'ThreadTitleGenerated' || row.event_type === 'ThreadTitleRenamed') && row.payload.title) {
      thread.meta.title = row.payload.title as string;
      generatedTitleIds.add(threadId);
    }
    if (isChannelDefiningEvent(row.event_type) && row.payload.channel) {
      thread.meta.channel = row.payload.channel as ThreadMeta['channel'];
    }
  }
  // Restore API-authoritative fields on initial load only.
  if (apiSection !== null) thread.meta.section = apiSection;
  if (apiStatus !== null) thread.meta.status = apiStatus;
  if (apiChannel !== null) thread.meta.channel = apiChannel;
}
