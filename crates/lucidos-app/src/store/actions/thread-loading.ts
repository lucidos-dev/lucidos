import { threadMap, focusedThreadId, setFocusedThread, showToast, threadsLoaded, generatedTitleIds, threadHasMore, threadLoadingMore, archiveThreadCount, ALL_CHANNELS, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, filterFacets, codingAgentSessionVersion, engineRestarting, archivingThreadIds, CODING_AGENT_CHANNEL, threadChannelToFilterSource, type ThreadFilterSource } from '../store';
import { threadPassesChannelFilter } from '../threadFilter';
import { handleEvent, isChannelDefiningEvent, PENDING_TITLE_PLACEHOLDER, applyAggregateToMeta, createdKey, type ThreadAggregate, type ThreadState, type ThreadEvent, type ThreadMeta, type ThreadStatus } from '../thread-events';
import { bumpThreadEvents } from '../threadActivity';
import { recordPerfSample } from '../../utils/perfQueue';
import { markThreadOpenStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { applyDraftBatch, setDraft, clearDraft, type ComposeDraft } from '../composeDrafts';
import { fetchThreads, fetchThreadEvents, fetchOlderThreads, fetchFilterFacets, fetchArchivedCount } from '../../api/threads';
import type { ThreadSummary, ThreadEventRow } from '../../api/threads';
import { isTransportError } from '../../api/client';
import { toFailed } from '../types';
import { errorDetail, isAbortError } from '../../utils/errorDetail';
import { postClientLog } from '../../utils/liveness';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { pendingComposePuts, composeEditedAt, composePutSettledAt, hasLocalDraftEdit } from './compose';

/** Buffer for batched compose draft writes during loadAllThreads — hundreds
 *  of threads through the upsertThread loop should land in one signal write,
 *  not N. `null` means clear the entry. Caller flushes via `applyDraftBatch`. */
type DraftBatch = Map<string, ComposeDraft | null>;

/** Per-thread timestamp of the last local archive-flip (Date.now()). Mirrors
 *  composeEditedAt: upsertThread consults this and skips overwriting `section`
 *  + `codingAgentProposed` when the GET went out before the flip. Without it,
 *  a stale GET (e.g. resyncLoadedThreads' /api/v1/threads fired before the user
 *  clicked Archive) whose response lands AFTER the optimistic flip silently
 *  overwrites section back to 'inbox' — the row flickers back into Review
 *  until the SSE ThreadArchived event finally confirms the move. Stays set
 *  forever (no expiry); legitimate fresh GETs capture requestStartedAt AFTER
 *  the last flip and the guard naturally lets them through. Stamped per
 *  cascade member by handleArchiveThread (threads.ts). */
export const sectionMutatedAt = new Map<string, number>();

function makeThreadState(info: ThreadSummary, saved: boolean, batch?: DraftBatch): ThreadState {
  stageDraftFromApi(info, batch);
  return {
    meta: {
      id: info.thread_id,
      title: info.title || PENDING_TITLE_PLACEHOLDER,
      channel: info.channel as ThreadMeta['channel'],
      initiator: info.initiator,
      saved,
      createdAt: info.created_at || new Date().toISOString(),
      updatedAt: info.last_activity || info.created_at || new Date().toISOString(),
      // Absent (legacy/test mocks) → fall back to last_activity, matching
      // recencyKey's fallback so the Saved-section sort key stays coherent.
      // (Archive sorts + pages by createdAt now, not this field.)
      lastUserAction: info.last_user_action || info.last_activity || info.created_at || new Date().toISOString(),
      lastAgentAction: info.last_agent_action || info.last_activity || info.created_at || new Date().toISOString(),
      status: (info.status as ThreadStatus) || 'idle',
      messageCount: info.message_count || 0,
      section: (info.section as ThreadMeta['section']) || 'archived',
      activeChildrenCount: info.active_children_count || 0,
      totalChildrenCount: info.total_children_count || 0,
      blockingDescendantCount: info.blocking_descendant_count || 0,
      attentionDescendantCount: info.attention_descendant_count || 0,
      codingAgentHasDiff: info.coding_agent_has_diff || false,
      codingAgentProposed: info.coding_agent_proposed || false,
      codingAgentRequiresRestart: info.coding_agent_requires_restart || false,
      codingAgentIsExternalRepo: info.coding_agent_is_external_repo || false,
      codingAgentApplying: info.coding_agent_applying || false,
      lastRevivedAt: info.last_revived_at || '',
      parentThreadId: info.parent_thread_id || undefined,
      parentThreadTitle: info.parent_thread_title || undefined,
      triggerId: info.trigger_id || undefined,
      triggerName: info.trigger_name || undefined,
      repoId: info.cc_repo_id || undefined,
      repoName: info.cc_repo_name || undefined,
      codingAgentKind: info.coding_agent_kind || undefined,
      codingAgentFolder: info.coding_agent_folder || undefined,
      codingAgent: info.coding_agent || undefined,
      state: info.state,
      latestTodoList: null,
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/** Stage a draft write from API metadata. Pushes into `batch` when provided
 *  (loadAllThreads' single-flush path); otherwise writes the signal directly
 *  (single-thread upserts from search/link bootstrapping). Empty server-side
 *  drafts clear the entry instead of populating it — `EMPTY_DRAFT` covers
 *  reads, and an empty entry would only inflate the Map for no reason. */
function stageDraftFromApi(info: ThreadSummary, batch?: DraftBatch): void {
  const text = info.compose_text || '';
  // The backend column is still named `compose_images` (Phase 5 cleanup
  // will rename it); post-migration the JSONB array contains hash strings.
  const image_hashes = info.compose_images || [];
  const mode = info.compose_mode ?? null;
  const isEmpty = text === '' && image_hashes.length === 0 && mode === null;
  // A bulk loadAllThreads/upsert snapshot must NEVER clear a non-empty draft
  // the user typed ON THIS DEVICE. The compose PUT is debounced and can fail or
  // time out under host contention; `composePutSettledAt` is then stamped (even
  // on failure) and a later resync's GET — fired AFTER that stamp, reading the
  // server before our text ever committed — returns an empty compose. None of
  // the timing guards in `upsertThread` (typing-here / pendingComposePuts /
  // composeEditedAt / composePutSettledAt) catch that post-settle stale-empty
  // read, so without this it blanks the just-typed draft (the value='' face of
  // mobile-webkit drafts.spec.ts:65 — and a real way a transient sync failure
  // silently drops an unsent draft). Gate on `composeEditedAt.has` so this only
  // protects locally-authored drafts: a server-ORIGINATED draft (cross-device,
  // never edited here) can still be cleared by a snapshot, and a peer's clear
  // still flows through the SSE `ThreadComposeChanged` path. The kept draft is
  // local-view only — it schedules no PUT, so it never resurrects server-side
  // unless the user resumes editing it.
  if (isEmpty && hasLocalDraftEdit(info.thread_id)) {
    return;
  }
  if (batch) {
    batch.set(info.thread_id, isEmpty ? null : { text, image_hashes, mode });
    return;
  }
  if (isEmpty) clearDraft(info.thread_id);
  else setDraft(info.thread_id, { text, image_hashes, mode });
}

/** Insert or update a thread in the map from API metadata. Exported for testing.
 *
 *  `requestStartedAt` (Date.now() ms) is the moment the originating GET went
 *  out. The compose-fields overwrite is skipped if a local edit happened
 *  AT OR AFTER the request started — without this, a slow GET issued before
 *  the user's photo attach but whose response lands AFTER pushNow's PUT
 *  clears `pendingComposePuts` silently overwrites the optimistic image with
 *  the server's pre-PUT snapshot. Default `Number.MAX_SAFE_INTEGER` is the
 *  "infinitely fresh" sentinel for synthetic callers (tests, code paths that
 *  don't gate on a real GET) so the staleness check is always false. */
export function upsertThread(
  map: Map<string, ThreadState>,
  info: ThreadSummary,
  saved: boolean,
  requestStartedAt: number = Number.MAX_SAFE_INTEGER,
  draftBatch?: DraftBatch,
): void {
  if (!map.has(info.thread_id)) {
    map.set(info.thread_id, makeThreadState(info, saved, draftBatch));
  } else {
    const existing = map.get(info.thread_id)!;
    if (saved) existing.meta.saved = true;
    if (info.title && info.title !== PENDING_TITLE_PLACEHOLDER && !generatedTitleIds.has(info.thread_id)) existing.meta.title = info.title;
    if (info.created_at) existing.meta.createdAt = info.created_at;
    // Snapshot the live (pre-overlay) last_activity so the status guard below
    // can spot a stale GET. `apiTime` (this snapshot), `existing.meta.updatedAt`
    // (last applied live event) and the per-thread refresh's currentAggregate
    // are all the SAME monotonic thread_summaries.last_activity column read at
    // different times — so an older `apiTime` means this GET fired before a
    // live event we've already applied.
    const liveUpdatedAt = existing.meta.updatedAt;
    const apiTime = info.last_activity || info.created_at;
    // ISO-8601 UTC timestamps from the backend (`...Z`) are lexicographically
    // ordered the same way they're chronologically ordered — fixed-width
    // YYYY-MM-DDTHH:MM:SS.fffZ with the same timezone suffix. A string `>`
    // is the cheapest valid "newer than" test; no Date parse needed.
    if (apiTime && apiTime > existing.meta.updatedAt) existing.meta.updatedAt = apiTime;
    // Advance the attributed-recency fields monotonically too (same stale-GET
    // guard as updatedAt): a slow GET must never regress the drawer sort key.
    // Set outright if unset (optional field), else only move forward.
    if (info.last_user_action && (!existing.meta.lastUserAction || info.last_user_action > existing.meta.lastUserAction)) existing.meta.lastUserAction = info.last_user_action;
    if (info.last_agent_action && (!existing.meta.lastAgentAction || info.last_agent_action > existing.meta.lastAgentAction)) existing.meta.lastAgentAction = info.last_agent_action;
    if (info.channel) existing.meta.channel = info.channel as ThreadMeta['channel'];
    if (info.initiator) existing.meta.initiator = info.initiator;
    // Update status from the snapshot — backend-authoritative, but only when
    // this GET isn't stale. A resync GET (loadAllThreads on SSE reconnect /
    // visibility resume) that fired while the thread was `running` and landed
    // AFTER live SSE applied the terminal `ResponseGenerated` (status='idle')
    // would otherwise clobber idle → running — the dot stuck on "running" until
    // reload. `apiTime < liveUpdatedAt` means exactly that. Mirrors the
    // monotonic stale-GET guards on updatedAt / lastUserAction above.
    const statusSnapshotStale = !!apiTime && !!liveUpdatedAt && apiTime < liveUpdatedAt;
    if (info.status && !statusSnapshotStale) existing.meta.status = info.status as ThreadStatus;
    if (info.message_count) existing.meta.messageCount = info.message_count;
    // Skip section + codingAgentProposed when a local archive-flip happened
    // AT OR AFTER this GET went out — its snapshot is stale by definition.
    // See `sectionMutatedAt` for the full race description.
    const sectionEditedSinceRequest = (sectionMutatedAt.get(info.thread_id) ?? 0) >= requestStartedAt;
    if (!sectionEditedSinceRequest) {
      if (info.section) existing.meta.section = info.section as ThreadMeta['section'];
      existing.meta.codingAgentProposed = info.coding_agent_proposed || false;
    }
    existing.meta.activeChildrenCount = info.active_children_count || 0;
    existing.meta.totalChildrenCount = info.total_children_count || 0;
    existing.meta.blockingDescendantCount = info.blocking_descendant_count || 0;
    existing.meta.attentionDescendantCount = info.attention_descendant_count || 0;
    // Update coding-agent state fields from API
    existing.meta.codingAgentHasDiff = info.coding_agent_has_diff || false;
    existing.meta.codingAgentRequiresRestart = info.coding_agent_requires_restart || false;
    existing.meta.codingAgentIsExternalRepo = info.coding_agent_is_external_repo || false;
    existing.meta.codingAgentApplying = info.coding_agent_applying || false;
    if (info.last_revived_at) existing.meta.lastRevivedAt = info.last_revived_at;
    if (info.parent_thread_id) existing.meta.parentThreadId = info.parent_thread_id;
    if (info.parent_thread_title) existing.meta.parentThreadTitle = info.parent_thread_title;
    if (info.trigger_id) existing.meta.triggerId = info.trigger_id;
    if (info.trigger_name) existing.meta.triggerName = info.trigger_name;
    if (info.cc_repo_id) existing.meta.repoId = info.cc_repo_id;
    if (info.cc_repo_name) existing.meta.repoName = info.cc_repo_name;
    if (info.coding_agent_kind) existing.meta.codingAgentKind = info.coding_agent_kind;
    if (info.coding_agent_folder) existing.meta.codingAgentFolder = info.coding_agent_folder;
    if (info.coding_agent) existing.meta.codingAgent = info.coding_agent;
    // Refresh compose state from API. Without this, an SSE skeleton stuck at
    // state='composing' (because MessageReceived was dropped) stays invisible
    // forever — categorizeThreads skips composing rows, so the thread never
    // surfaces in any drawer section even after the projection moved on.
    //
    // Skip the compose fields under three conditions:
    //   1. User is mid-edit on this thread's textarea (focus + matching id).
    //   2. A debounced/in-flight PUT covers the value the API would clobber.
    //   3. A local edit happened AFTER this GET went out — its response is
    //      stale wrt compose by definition. Without this, picker dismissal +
    //      slow loadAllThreads + fast PUT races to overwrite the optimistic
    //      image (preview appears, then disappears).
    existing.meta.state = info.state;
    const isFocusedThread = info.thread_id === focusedThreadId.value;
    const userIsTypingHere = isFocusedThread && isComposeFocusedHere(info.thread_id);
    // `>=` because both timestamps come from Date.now() (1ms resolution) — a
    // request fired in the same millisecond as the edit can race ahead of the
    // edit's PUT and would otherwise pass the guard.
    const editedSinceRequest = (composeEditedAt.get(info.thread_id) ?? 0) >= requestStartedAt;
    // Inverse-order hole: the edit predates this GET (so editedSinceRequest is
    // false) but the debounced PUT settled AT OR AFTER the GET went out, so the
    // server snapshot in this response was read before the PUT committed and is
    // stale. pendingComposePuts no longer covers it — it cleared when the PUT
    // settled, which is exactly the moment composePutSettledAt records. Skip the
    // overwrite. See `composePutSettledAt` (fixes drafts.spec.ts:65 blank-draft).
    const putSettledSinceRequest = (composePutSettledAt.get(info.thread_id) ?? 0) >= requestStartedAt;
    if (!userIsTypingHere && !pendingComposePuts.has(info.thread_id) && !editedSinceRequest && !putSettledSinceRequest) {
      stageDraftFromApi(info, draftBatch);
    }
  }
}

/** Thread IDs loaded ONLY as a family extension (ancestor/descendant of a
 *  paginated thread). Excluded from the `loadOlderThreads` cursor so an
 *  eagerly-loaded family member from way back in history doesn't advance
 *  the pagination cursor past every thread between "now" and itself. An
 *  ID is removed from this set when natural pagination later returns it
 *  as a base thread — at that point it's no longer family-only. */
const familyExtensionIds = new Set<string>();

/** Test-only reset hook. Vitest's per-test `beforeEach` mounts a fresh
 *  threadMap; module-level state needs to follow. Production code never
 *  calls this. */
export function _clearFamilyExtensionIdsForTest(): void {
  familyExtensionIds.clear();
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

/** Fetch the complete set of selectable filter facets (every trigger/repo/app
 *  that has a thread) so the drawer "Show" dropdown lists them all, not just
 *  facets present in the loaded window. Best-effort: a failure leaves the
 *  dropdown seeded from loaded threads + registries (the prior behavior). */
export async function loadFilterFacets(): Promise<void> {
  if (filterFacets.value.status !== 'loaded') filterFacets.value = { status: 'loading' };
  try {
    filterFacets.value = { status: 'loaded', data: await fetchFilterFacets() };
  } catch (e) {
    filterFacets.value = toFailed(e);
  }
}

async function loadAllThreadsInner(): Promise<void> {
  const focused = focusedThreadId.value;

  // Capture BEFORE the GET fires so a local edit that happens between this
  // line and the response landing reliably wins (composeEditedAt > requestAt).
  const requestStartedAt = Date.now();
  const response = await fetchThreads(focused || undefined);

  const map = threadMap.value;
  const activeSet = new Set(response.active);
  const draftBatch: DraftBatch = new Map();

  // Build/update thread states from metadata
  for (const info of response.saved) {
    upsertThread(map, info, true, requestStartedAt, draftBatch);
    familyExtensionIds.delete(info.thread_id);
  }
  for (const info of [...response.active_threads, ...response.archive, ...response.composing]) {
    upsertThread(map, info, false, requestStartedAt, draftBatch);
    familyExtensionIds.delete(info.thread_id);
  }
  // Focused thread may be too old for recent list, not saved, not active
  if (response.focused_thread) {
    upsertThread(map, response.focused_thread, false, requestStartedAt, draftBatch);
    familyExtensionIds.delete(response.focused_thread.thread_id);
  }
  // Family members of the loaded set that weren't already loaded. The
  // drawer's family-aware sort needs every member present in threadMap to
  // nest them under their parent — but these threads MUST stay out of the
  // pagination cursor (see `familyExtensionIds`) since their own
  // `last_activity` can be arbitrarily old. The `?? []` keeps the legacy
  // `(fetchThreads as any).mockResolvedValue({...})` test mocks (which
  // skip optional fields) green — "no family extension" is the field's
  // correct semantic default, not a value-masking fallback.
  for (const info of response.family_threads ?? []) {
    upsertThread(map, info, false, requestStartedAt, draftBatch);
    familyExtensionIds.add(info.thread_id);
  }

  // Ghost focused-thread: the persisted focusedThreadId references a thread
  // the backend doesn't know about (deleted, never committed because the user
  // never sent, or wrong workspace). Without this clear, loadThreadEvents is
  // gated on threadInMap and never fires — the UI shows an indefinite spinner
  // that turns into "Taking too long? Tap to reload" after 8s, and the same
  // ghost id survives every reload until the user manually picks another
  // thread. The fetchThreads request above already passed `focused` as a hint
  // so the backend gets a chance to include it via `response.focused_thread`;
  // if the id is still missing from the map after every upsert, the thread
  // does not exist server-side.
  //
  // The `focusedThreadId.peek() === focused` guard prevents wiping a fresh
  // user navigation that landed during the await: if the user tapped a real
  // thread between the captured-`focused` and now, peek() returns that new id
  // and we leave it alone. Without the guard, a workspace with a ghost id in
  // localStorage would snap any concurrent navigation back to the compose
  // screen.
  let ghostFocusedThread: string | null = null;
  if (focused && !map.has(focused) && focusedThreadId.peek() === focused) {
    ghostFocusedThread = focused;
    setFocusedThread(null);
    postClientLog('lifecycle', 'cleared_ghost_focus', { thread_id: focused });
  }

  applyDraftBatch(draftBatch);
  threadMap.value = new Map(map);
  threadsLoaded.value = true;
  // Collapsed Archive badge total. The inline `archive_count` is the UNFILTERED
  // pile size — correct for the common no-filter drawer and instant (no extra
  // round-trip). When a drawer filter is active (it persists across reloads via
  // localStorage), that global total is wrong, so re-fetch the filter-scoped
  // count. `?? 0` degrades gracefully when an older engine / test mock omits it.
  const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();
  if (sources || triggerIds || repoIds || appIds) {
    void refreshArchivedCount();
  } else {
    archiveThreadCount.value = response.archive_count ?? 0;
  }

  // Refresh the complete filter-facet set (fire-and-forget) so the drawer
  // "Show" dropdown reflects newly-created / archived threads. Best-effort:
  // the option lists still work from loaded threads if this fails.
  void loadFilterFacets();

  // Load events for focused, active, and saved threads; others load lazily
  // on focus. Saved threads are always few and need events for correct
  // drawer status (e.g. waiting coding-agent threads after engine restart).
  const loads: Promise<void>[] = [];
  if (focused && focused !== ghostFocusedThread) loads.push(loadThreadEvents(focused));
  for (const t of map.values()) {
    if (t.meta.id !== focused && (activeSet.has(t.meta.id) || t.meta.saved)) {
      loads.push(loadThreadEvents(t.meta.id));
    }
  }
  if (loads.length > 0) await Promise.all(loads);

  // Bump so CodingAgentControlMenu re-fetches commands after reload (SSE doesn't replay).
  if (focused) {
    const focusedThread = map.get(focused);
    if (focusedThread?.meta.channel === CODING_AGENT_CHANNEL) {
      codingAgentSessionVersion.value++;
    }
  }
}

/** Ensure a thread exists in threadMap, bootstrapping from metadata if needed, then load its events. */
export async function ensureThreadInMap(info: ThreadSummary): Promise<void> {
  const map = threadMap.value;
  if (!map.has(info.thread_id)) {
    upsertThread(map, info, false);
    threadMap.value = new Map(map);
  }
  await loadThreadEvents(info.thread_id);
}

/** Fetch a thread's metadata by ID and add it to the map.
 *
 *  Used by thread-link clicks (and any other flow that knows only the ID,
 *  not the metadata) so the link works even when the thread is too old to be
 *  in the loaded thread list — e.g. an archived thread beyond the per-source
 *  Archive window. Returns true if the thread is now in the map, false if
 *  the API has no record of this ID. Network/HTTP failures propagate so the
 *  caller can surface the real error instead of a generic "not found". */
export async function ensureThreadByIdInMap(threadId: string): Promise<boolean> {
  if (threadMap.value.has(threadId)) return true;
  const requestStartedAt = Date.now();
  const response = await fetchThreads(threadId);
  // The backend returns the focused thread separately when it isn't already
  // in saved/active/archive. When it IS already there, we still need to
  // pluck it out of those lists.
  const info =
    response.focused_thread
    ?? response.saved.find(t => t.thread_id === threadId)
    ?? response.active_threads.find(t => t.thread_id === threadId)
    ?? response.archive.find(t => t.thread_id === threadId);
  if (!info) return false;
  const isSaved = response.saved.some(t => t.thread_id === threadId);
  const map = threadMap.value;
  upsertThread(map, info, isSaved, requestStartedAt);
  threadMap.value = new Map(map);
  return true;
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

  // Perf: stamp the open-start for the `thread-render` mark — the moment the
  // focused thread's real event load begins (we're past the eventsLoaded
  // early-return). ThreadView reads + clears it on first content render to
  // measure open→paint. Gated to the focused thread so loadAllThreads' eager
  // loads of non-focused active/saved threads (which never render) don't leave
  // stale marks that would later fire a multi-minute renderMs. Covers both the
  // click case (focusThread sets focusedThreadId before calling this) and
  // cold-start (loadAllThreads loads the already-focused thread). See
  // utils/threadOpenMarks.ts + utils/perfQueue.ts. Fire-and-forget telemetry.
  if (threadId === focusedThreadId.value) markThreadOpenStart(threadId, currentPerfBaseline());

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
        // Perf instrumentation: split the open cost into transfer (fetchMs) vs
        // store (applyMs), correlated with event count, so we can see which half
        // dominates on a big coding-agent thread. Fire-and-forget; the grouping
        // half is measured separately in ThreadView. See utils/perfQueue.ts.
        const fetchStart = performance.now();
        const snapshot = await fetchThreadEvents(threadId);
        const fetchMs = performance.now() - fetchStart;
        // Re-read from current map — the map reference may have changed
        // during the async fetch (other threads loaded/updated).
        const current = threadMap.value.get(threadId);
        if (!current) return;
        const applyStart = performance.now();
        applyEventRows(threadMap.value, threadId, current, snapshot.events, snapshot.currentAggregate);
        const applyMs = performance.now() - applyStart;
        current.eventsLoaded = true;
        threadMap.value = new Map(threadMap.value);
        recordPerfSample('thread-load', {
          threadId,
          eventCount: snapshot.events.length,
          channel: current.meta.channel,
          fetchMs: Math.round(fetchMs),
          applyMs: Math.round(applyMs),
        });
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

/** Fetch only new events since the thread's last DB-loaded sequence and append them.
 *
 *  Retries once on transient wake failures — DOMException (TimeoutError or
 *  AbortError) AND transport errors (TypeError "Load failed"/"Failed to fetch").
 *  The runResumeSync / resyncLoadedThreads paths fan this out across every
 *  loaded thread on SSE reconnect, iOS PWA wake, or right after an engine
 *  restart. iOS Safari cancels in-flight fetches mid-flight (suspend/resume,
 *  lifecycle, network change) and fails the first request on a stale HTTP/2
 *  connection — both succeed on retry. Without the retry the user gets one
 *  toast per affected thread — dozens at once in a single wake cycle. Mirrors
 *  the retry-on-TimeoutError pattern in refreshChangesState. */
export async function refreshThreadEvents(threadId: string): Promise<void> {
  if (engineRestarting.value) return;
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (!thread || !thread.eventsLoaded) return;

  try {
    const snapshot = await fetchThreadEvents(threadId, thread.lastDbSeq || undefined)
      .catch(err => {
        if (!(err instanceof DOMException) && !isTransportError(err)) throw err;
        return fetchThreadEvents(threadId, thread.lastDbSeq || undefined);
      });
    // Always apply currentAggregate even when no new events arrived — the
    // snapshot may have advanced (e.g. status flipped) since the last refresh.
    if (snapshot.events.length > 0 || snapshot.currentAggregate) {
      applyEventRows(map, threadId, thread, snapshot.events, snapshot.currentAggregate);
      threadMap.value = new Map(threadMap.value);
    }
  } catch (err) {
    if (engineRestarting.value) return;
    // Telemetry carve-out (.claude/rules/frontend.md): this refresh is
    // background (resyncLoadedThreads / runResumeSync on wake, SSE reconnect,
    // or right after an engine restart), never user-initiated. On the iOS
    // suspend/resume boundary WebKit aborts the in-flight fetch (AbortError)
    // and the freshly-resumed page's stale HTTP/2 connection fails the request
    // at the transport layer (TypeError "Load failed") — sometimes both the
    // original AND the retry. The engine is reachable and SSE delivers any
    // events we miss, so a toast per affected thread is a dozen unactionable
    // errors at once. Self-recovery: the next SSE event (constant on any active
    // thread) re-syncs via handleThreadEvent, or the next resync cycle (engine
    // wake-up, SSE reconnect) refreshes successfully once the connection has
    // settled. Genuine errors (5xx, parse, TimeoutError) still toast.
    if (isAbortError(err) || isTransportError(err)) {
      console.warn(`[ThreadLoading] refresh failed transiently for ${threadId} (iOS PWA wake / engine restart); SSE will recover`, err);
      return;
    }
    console.warn(`[ThreadLoading] Failed to refresh events for ${threadId}:`, err);
    const title = threadMap.value.get(threadId)?.meta.title || threadId.slice(0, 8);
    showToast(`Failed to refresh thread events for "${title}": ${errorDetail(err)}`, 'error');
  }
}

/** Re-arm pagination and eagerly fetch the first page of threads matching the
 *  newly-applied filter.
 *
 *  The drawer's IntersectionObserver sentinel cannot be relied on to populate a
 *  freshly-filtered view: its fill loop is suppressed while the Archive section
 *  is collapsed (`archivePaginationAllowed`) and only re-fires on a scroll
 *  transition, so selecting a facet whose matches are all archived/old would
 *  strand the user on "No threads" until they manually scrolled. The drawer calls this
 *  whenever the channel / trigger / repo / app selection changes so that
 *  "select a facet → see its threads" is deterministic. `loadOlderThreads`
 *  self-guards against concurrent calls and falls back to a now()-cursor when
 *  no loaded thread matches, so this is safe to fire on every filter change. */
export async function reloadAfterFilterChange(): Promise<void> {
  // Different filter = different cursor space; clear any stale "no more" from
  // the previous selection before fetching, or loadOlderThreads early-returns.
  threadHasMore.value = true;
  // Re-fetch the filter-scoped Archive badge total so it reflects the new
  // selection immediately (stable, server-sourced — not the loaded count).
  void refreshArchivedCount();
  await loadOlderThreads();
}

/** Channel/facet filter params for the older-threads + archived-count APIs,
 *  read from the live drawer-filter signals. `sources` is undefined when every
 *  channel is selected (no narrowing); each facet array is undefined when its
 *  selection is empty. Shared by `loadOlderThreads` (pagination cursor space)
 *  and `refreshArchivedCount` (badge total) so both target the identical set. */
export function currentThreadFilterParams(): {
  sources: ThreadFilterSource[] | undefined;
  triggerIds: string[] | undefined;
  repoIds: string[] | undefined;
  appIds: string[] | undefined;
} {
  const filter = threadChannelFilter.value;
  const isFiltered = filter.size < ALL_CHANNELS.length;
  const triggerIdSet = selectedTriggerIds.value;
  const repoIdSet = selectedRepoIds.value;
  const appIdSet = selectedAppIds.value;
  return {
    sources: isFiltered ? [...filter].map(threadChannelToFilterSource) : undefined,
    triggerIds: triggerIdSet.size > 0 ? [...triggerIdSet] : undefined,
    repoIds: repoIdSet.size > 0 ? [...repoIdSet] : undefined,
    appIds: appIdSet.size > 0 ? [...appIdSet] : undefined,
  };
}

/** Refresh `archiveThreadCount` — the collapsed Archive badge total — so it
 *  reflects the ACTIVE drawer filter and stays stable regardless of how many
 *  rows are paginated in (the badge reads this signal directly; it must NOT
 *  change as the user scrolls or collapses/expands the section). Fetches the
 *  true server-side count of archived (unsaved) threads matching the current
 *  channel/trigger/repo/app selection.
 *
 *  Best-effort: on failure the badge keeps its previous value and the next
 *  filter change / reload re-fetches. It's an informational count, not a
 *  blocking fetch — the rows themselves still load via pagination — so a
 *  transient miss must not pop a toast (per the frontend telemetry carve-out). */
export async function refreshArchivedCount(): Promise<void> {
  // Empty channel filter = nothing shown by intent → the archive is empty.
  if (threadChannelFilter.value.size === 0) {
    archiveThreadCount.value = 0;
    return;
  }
  const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();
  try {
    archiveThreadCount.value = await fetchArchivedCount(sources, triggerIds, repoIds, appIds);
  } catch (err) {
    console.warn('[threads] archived-count refresh failed:', errorDetail(err));
  }
}

/** Load older threads for infinite scroll. Self-guards against concurrent calls.
 *  Passes channel + trigger-id filters to the API so pagination targets only
 *  matching threads. */
export async function loadOlderThreads(): Promise<void> {
  if (threadLoadingMore.value || !threadHasMore.value) return;
  // Empty filter = nothing visible by intent; never fetch.
  if (threadChannelFilter.value.size === 0) {
    threadHasMore.value = false;
    return;
  }
  threadLoadingMore.value = true;

  try {
    const map = threadMap.value;
    const filter = threadChannelFilter.value;
    const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();
    const triggerIdSet = selectedTriggerIds.value;
    const repoIdSet = selectedRepoIds.value;
    const appIdSet = selectedAppIds.value;

    let oldestTime: string | null = null;
    for (const t of map.values()) {
      if (t.meta.saved) continue;
      // `t.meta.section` is the raw `archive_state` ('archived'/'inbox'). The
      // cursor tracks the Archive pile, which `get_recent_threads` returns as a
      // single contiguous `created_at DESC` window — no out-of-window injection
      // (the actionable/proposed bypasses were removed), so every loaded archived
      // row is a contiguous member and there are no outliers to special-case.
      if (t.meta.section !== 'archived') continue;
      // Family-extension threads are loaded eagerly so the drawer can nest
      // them under their parent, but their own `last_activity` can be
      // arbitrarily old. Letting one drive the cursor would jump natural
      // pagination over every intervening thread.
      if (familyExtensionIds.has(t.meta.id)) continue;
      // Same predicate the display uses, so the cursor is the oldest loaded
      // thread that actually matches the active filter (channel + trigger +
      // repo/app union) — never drifts from what's shown.
      if (!threadPassesChannelFilter(t, filter, triggerIdSet, repoIdSet, appIdSet)) continue;
      // Cursor on created_at — the column the backend `get_older_threads`
      // pages by — so the cursor axis matches the Archive sort axis (else
      // pagination skips or repeats rows). Same fallback chain as `byCreated`.
      const key = createdKey(t);
      if (!oldestTime || key < oldestTime) oldestTime = key;
    }

    // Empty filter result on loaded threads — fall back to now() so the server
    // can find matches in history. Without this, a freshly-applied filter
    // permanently halts with "no more" even though matches exist on disk.
    if (!oldestTime) {
      // No channel narrowing (sources undefined ⇔ all channels) and no facet
      // selection → genuinely nothing left to page. With a filter, fall through
      // to the now()-cursor so the server can find matches deeper in history.
      if (!sources && !triggerIds && !repoIds && !appIds) {
        threadHasMore.value = false;
        return;
      }
      oldestTime = new Date().toISOString();
    }

    const response = await fetchOlderThreads(oldestTime, 15, sources, triggerIds, repoIds, appIds);
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
      // Promote: a thread previously loaded as a family extension that now
      // shows up in natural pagination is no longer family-only and should
      // contribute to the cursor on subsequent calls.
      familyExtensionIds.delete(info.thread_id);
    }
    for (const info of response.family_threads ?? []) {
      if (!map.has(info.thread_id)) {
        upsertThread(map, info, false);
        familyExtensionIds.add(info.thread_id);
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
  currentAggregate: ThreadAggregate | null,
): void {
  for (const row of rows) {
    const event = { type: row.event_type, ...row.payload } as ThreadEvent;
    handleEvent(map, threadId, row.sequence, event, row.created, row.event_id);
    if (row.sequence > thread.lastDbSeq) thread.lastDbSeq = row.sequence;

    if ((row.event_type === 'ThreadTitleGenerated' || row.event_type === 'ThreadTitleRenamed') && row.payload.title) {
      thread.meta.title = row.payload.title as string;
      generatedTitleIds.add(threadId);
    }
    if (isChannelDefiningEvent(row.event_type) && row.payload.channel) {
      thread.meta.channel = row.payload.channel as ThreadMeta['channel'];
    }
  }
  // Backend snapshot is the source of truth for meta — overlay last so any
  // per-event mutations during replay don't leak through to thread.meta.
  if (currentAggregate) {
    // Staleness guard (mirrors upsertThread): `currentAggregate.lastActivity`
    // and `thread.meta.updatedAt` are the SAME monotonic
    // thread_summaries.last_activity column read at different times. A snapshot
    // fetched before a live event we've already applied is stale — applying it
    // would regress status (running clobbering a live idle — the dot stuck on
    // "running" until reload), updatedAt, and counts back in time. Skip the
    // overlay entirely; the fresher live SSE state stands. New event rows above
    // are folded in first and advance updatedAt, so a refresh that brought
    // genuinely-new work is never misclassified as stale.
    const snapshotStale = !!currentAggregate.lastActivity && !!thread.meta.updatedAt
      && currentAggregate.lastActivity < thread.meta.updatedAt;
    if (!snapshotStale) {
      applyAggregateToMeta(thread.meta, currentAggregate);
      // Same archive race guard as the SSE path in thread-sync.ts: a replay
      // initiated before the user's Archive click ships a pre-archive
      // aggregate that would otherwise revert the optimistic flip.
      if (archivingThreadIds.value.has(threadId)) {
        thread.meta.section = 'archived';
        thread.meta.codingAgentProposed = false;
      }
    }
  }

  // Ring the per-thread bell once after the batch replay so subscribers to
  // this thread's `events` / `streamingBuffer` recompute (focused
  // ChatExchange, activeStreamingBuffer, activeExchanges). Cheaper than
  // calling per-row. The caller separately writes `threadMap` to fire wide
  // meta subscribers. The aggregate-only branch (`rows=[]` +
  // `currentAggregate`, hit by `refreshThreadEvents` when the snapshot
  // delivered no new events but the aggregate moved meta) deliberately does
  // NOT bump: `computeExchanges` reads only `meta.channel` + events +
  // pendingUserMessages, and `meta.channel` is the only `meta` field a bump
  // subscriber would read — but `channel` only changes via the per-row
  // `isChannelDefiningEvent` branch above, which requires `rows.length > 0`.
  // So aggregate-only refreshes wake no useful work via the bump path.
  if (rows.length > 0) bumpThreadEvents(threadId);
}
