import { threadMap, focusedThreadId, setFocusedThread, showToast, removeToast, connectionStatus, threadsLoaded, generatedTitleIds, threadHasMore, threadLoadingMore, archiveThreadCount, ALL_CHANNELS, filterFacets, codingAgentSessionVersion, engineRestarting, archivingThreadIds, CODING_AGENT_CHANNEL, toasts, THREAD_EVENTS_LOAD_TOAST_KEY, THREAD_EVENTS_REFRESH_TOAST_KEY, THREAD_EVENTS_FETCH_CONCURRENCY, threadChannelToFilterSource, type ThreadFilterSource } from '../store';
import { appliedThreadFilter, type ThreadFilterSelection } from '../appliedThreadFilter';
import { threadPassesChannelFilter } from '../threadFilter';
import { handleEvent, isChannelDefiningEvent, PENDING_TITLE_PLACEHOLDER, applyAggregateToMeta, createdKey, type ThreadAggregate, type ThreadState, type ThreadEvent, type ThreadMeta, type ThreadStatus } from '../thread-events';
import { bumpThreadEvents } from '../threadActivity';
import { recordPerfSample } from '../../utils/perfQueue';
import { runWithConcurrency } from '../../utils/concurrentPool';
import { markThreadOpenStart } from '../../utils/threadOpenMarks';
import { currentPerfBaseline } from '../../utils/renderPhaseTimers';
import { applyDraftBatch, setDraft, clearDraft, type ComposeDraft } from '../composeDrafts';
import { setComposeSelectionFromServer } from '../composeSelections';
import { fetchThreads, fetchThreadById, fetchThreadEvents, fetchOlderThreads, fetchFilterFacets, fetchArchivedCount } from '../../api/threads';
import type { ThreadSummary, ThreadEventRow } from '../../api/threads';
import { isTransientFetchError } from '../../api/client';
import { toFailed } from '../types';
import { errorDetail } from '../../utils/errorDetail';
import { postClientLog } from '../../utils/liveness';
import { isComposeFocusedHere } from '../../components/chat/promptFocus';
import { pendingComposePuts, composeEditedAt, composePutSettledAt, hasUnsentLocalDraft, clearSupersededDraft, noteServerDraft, noteComposeEpoch, noteServerComposeMode } from './compose';

/** Buffer for batched compose draft writes during loadAllThreads. Hundreds of
 *  threads through the upsertThread loop land in ONE signal write. `null`
 *  means clear the entry. The caller flushes via `applyDraftBatch`. */
type DraftBatch = Map<string, ComposeDraft | null>;

/** Per-thread timestamp of the last local archive-flip. Mirrors
 *  `composeEditedAt`: `upsertThread` consults it and skips overwriting
 *  `section` and `codingAgentProposed` when the GET went out before the flip.
 *  Without it, a stale GET landing after the optimistic flip overwrites
 *  section back to 'inbox'. The row then flickers into Review until the SSE
 *  event confirms the move.
 *
 *  Stays set forever, with no expiry: a legitimate fresh GET captures
 *  `requestStartedAt` after the last flip, so the guard lets it through.
 *  `handleArchiveThread` (threads.ts) stamps it per cascade member. */
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
      // Absent on a legacy row or test mock, so fall back to last_activity.
      // Matches `recencyKey`'s fallback, keeping the Saved-section sort key
      // coherent. Archive sorts and pages by createdAt rather than this.
      lastUserAction: info.last_user_action || info.last_activity || info.created_at || new Date().toISOString(),
      lastAgentAction: info.last_agent_action || info.last_activity || info.created_at || new Date().toISOString(),
      status: (info.status as ThreadStatus) || 'idle',
      messageCount: info.message_count || 0,
      section: (info.section as ThreadMeta['section']) || 'archived',
      activeChildrenCount: info.active_children_count || 0,
      totalChildrenCount: info.total_children_count || 0,
      blockingDescendantCount: info.blocking_descendant_count || 0,
      attentionDescendantCount: info.attention_descendant_count || 0,
      liveEventWaitCount: info.live_event_wait_count || 0,
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
      liveEventWaits: info.live_event_waits ?? [],
    },
    events: new Map(),
    streamingBuffer: '',
    eventsLoaded: false,
    eventsLoadFailed: false,
    lastDbSeq: 0,
    pendingUserMessages: [],
  };
}

/** True when a thread-summary snapshot carries no compose draft at all: the
 *  shape the backend leaves after clearing compose fields, meaning the shared
 *  draft was sent or discarded by somebody. ONE helper, so the upsert
 *  focus-gate and `stageDraftFromApi`'s clear decision cannot drift apart. If
 *  they disagreed, a focused draft could pass the gate and be re-staged. */
function composeSnapshotIsEmpty(info: ThreadSummary): boolean {
  return (info.compose_text || '') === '' &&
    (info.compose_images || []).length === 0 &&
    (info.compose_mode ?? null) === null;
}

function stageDraftFromApi(info: ThreadSummary, batch?: DraftBatch): void {
  const text = info.compose_text || '';
  // The backend column is named `compose_images`, but the JSONB array holds
  // hash strings.
  const image_hashes = info.compose_images || [];
  // The snapshot is the server stating what it holds. Record it BEFORE any
  // guard. The guard below may keep a local draft the server no longer has,
  // and that difference is what `serverDraft` exists to state. Callers
  // gate this whole function on the staleness guards, so a snapshot known to
  // predate a local write never lands here.
  noteServerDraft(info.thread_id, text, image_hashes);
  // The snapshot's *compose epoch*, on the same footing as the draft itself
  // and likewise recorded before the guard below.
  noteComposeEpoch(info.thread_id, info.compose_epoch);
  const mode = info.compose_mode ?? null;
  // The stored mode, same footing again. A write only carries a mode the engine
  // is not already known to hold.
  noteServerComposeMode(info.thread_id, mode);
  const isEmpty = composeSnapshotIsEmpty(info);
  // A bulk snapshot must NEVER clear a non-empty draft the user typed ON THIS
  // DEVICE. The compose PUT is debounced and can fail or time out under host
  // contention. `composePutSettledAt` is stamped even on failure. A later
  // resync's GET then fires after that stamp, reads the server before our text
  // committed, and returns an empty compose. None of the timing guards in
  // `upsertThread` catch that post-settle stale-empty read.
  //
  // Gated on `hasUnsentLocalDraft`, so it protects only locally-authored,
  // genuinely unsent work. A snapshot can still clear a server-ORIGINATED
  // draft, or one the history shows was submitted. A peer's clear still flows
  // through the SSE path. The kept draft is local-view only: it schedules no
  // PUT, so it never resurrects server-side unless the user resumes editing.
  if (isEmpty && hasUnsentLocalDraft(info.thread_id)) {
    return;
  }
  // Rehydrate the per-draft dropdown selection from the DB, the authoritative
  // store, so a reload restores the draft's picks. Placed past the local-edit
  // guard above so it cannot clobber a locally-edited draft. An absent
  // selection clears the local entry.
  setComposeSelectionFromServer(info.thread_id, info.compose_selection);
  if (batch) {
    batch.set(info.thread_id, isEmpty ? null : { text, image_hashes, mode });
    return;
  }
  if (isEmpty) clearDraft(info.thread_id);
  else setDraft(info.thread_id, { text, image_hashes, mode });
}

/** Insert or update a thread in the map from API metadata.
 *
 *  `requestStartedAt` is the moment the originating GET went out. The
 *  compose-fields overwrite is skipped when a local edit happened AT OR AFTER
 *  it. Without that guard, a slow GET issued before the user's photo attach
 *  can land after `pushNow`'s PUT clears `pendingComposePuts`. It then
 *  overwrites the optimistic image with the server's pre-PUT snapshot.
 *
 *  The default `Number.MAX_SAFE_INTEGER` is an "infinitely fresh" sentinel for
 *  a synthetic caller that gates on no real GET, making the check always
 *  false. */
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
    // Snapshot the live, pre-overlay last_activity so the status guard below
    // can spot a stale GET. `apiTime`, `existing.meta.updatedAt` and the
    // per-thread refresh's currentAggregate all read the SAME monotonic
    // thread_summaries.last_activity column at different times. An older
    // `apiTime` therefore means this GET fired before a live event we applied.
    const liveUpdatedAt = existing.meta.updatedAt;
    const apiTime = info.last_activity || info.created_at;
    // A backend ISO-8601 UTC timestamp is fixed-width with the same timezone
    // suffix, so lexicographic order IS chronological order. A string `>` is
    // the cheapest valid "newer than" test, with no Date parse.
    if (apiTime && apiTime > existing.meta.updatedAt) existing.meta.updatedAt = apiTime;
    // Advance the attributed-recency fields monotonically too, under the same
    // stale-GET guard: a slow GET must never regress the drawer sort key. Set
    // outright when unset, else only move forward.
    if (info.last_user_action && (!existing.meta.lastUserAction || info.last_user_action > existing.meta.lastUserAction)) existing.meta.lastUserAction = info.last_user_action;
    if (info.last_agent_action && (!existing.meta.lastAgentAction || info.last_agent_action > existing.meta.lastAgentAction)) existing.meta.lastAgentAction = info.last_agent_action;
    if (info.channel) existing.meta.channel = info.channel as ThreadMeta['channel'];
    if (info.initiator) existing.meta.initiator = info.initiator;
    // Status is backend-authoritative, but only when this GET is not stale. A
    // resync GET fired while the thread was `running` can land after live SSE
    // applied the terminal event. It would then clobber idle back to running,
    // sticking the dot until a reload. `apiTime < liveUpdatedAt` means exactly
    // that. Mirrors the monotonic stale-GET guards above.
    const statusSnapshotStale = !!apiTime && !!liveUpdatedAt && apiTime < liveUpdatedAt;
    if (info.status && !statusSnapshotStale) existing.meta.status = info.status as ThreadStatus;
    if (info.message_count) existing.meta.messageCount = info.message_count;
    // Skip section and codingAgentProposed when a local archive-flip happened
    // AT OR AFTER this GET went out, since the snapshot is then stale by
    // definition. See `sectionMutatedAt`.
    const sectionEditedSinceRequest = (sectionMutatedAt.get(info.thread_id) ?? 0) >= requestStartedAt;
    if (!sectionEditedSinceRequest) {
      if (info.section) existing.meta.section = info.section as ThreadMeta['section'];
      existing.meta.codingAgentProposed = info.coding_agent_proposed || false;
    }
    existing.meta.activeChildrenCount = info.active_children_count || 0;
    existing.meta.totalChildrenCount = info.total_children_count || 0;
    existing.meta.blockingDescendantCount = info.blocking_descendant_count || 0;
    existing.meta.attentionDescendantCount = info.attention_descendant_count || 0;
    existing.meta.liveEventWaitCount = info.live_event_wait_count || 0;
    // The *event wait* list, under the SAME staleness guard as status, and for
    // the same reason: both directions lose real state. A GET fired before a
    // wait was armed would blank it. One fired before its delivery would
    // resurrect a dead wait, countdown and all.
    //
    // The guard is exact here. All four `EventWait*` projection arms bump
    // `last_activity`, and all four are 'activity' in `EVENT_CLASSIFICATION`.
    // So an arm or a resolution applied over SSE has already advanced
    // `meta.updatedAt` past this snapshot's `apiTime`.
    //
    // Absence is not emptiness: an older engine or a partial test fixture
    // omits the field, and must leave a populated list alone. Only an explicit
    // `[]` clears it, which is the reported bug's repair.
    if (info.live_event_waits && !statusSnapshotStale) existing.meta.liveEventWaits = info.live_event_waits;
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
    // Refresh compose state from the API. Without this, an SSE skeleton stuck
    // at state='composing' stays invisible forever: `categorizeThreads` skips
    // a composing row, so the thread surfaces in no drawer section even after
    // the projection moved on.
    //
    // Three conditions skip the compose fields:
    //   1. The user is mid-edit on this thread's textarea, and ONLY for a
    //      NON-empty snapshot (see below).
    //   2. A debounced or in-flight PUT covers the value the API would clobber.
    //   3. A local edit happened AFTER this GET went out, so the response is
    //      stale with respect to compose by definition.
    existing.meta.state = info.state;
    const isFocusedThread = info.thread_id === focusedThreadId.value;
    // An EMPTY server snapshot genuinely means the shared draft was sent or
    // discarded by somebody, since the backend clears compose_text on those
    // events. Focus must NOT block that clear, or a synced-from-peer draft
    // ghosts in a focused-but-untyped textarea.
    //
    // Unsent locally-authored work is still protected, by the staleness guards
    // below AND `stageDraftFromApi`'s own empty-guard. A draft whose text has
    // since been submitted is not such work and clears. A NON-empty snapshot
    // keeps the focus guard, so a background refresh cannot move the cursor.
    const snapshotIsEmpty = composeSnapshotIsEmpty(info);
    const userIsTypingHere = isFocusedThread && isComposeFocusedHere(info.thread_id) && !snapshotIsEmpty;
    // `>=` because both timestamps come from `Date.now()`, at 1ms resolution.
    // A request fired in the same millisecond as the edit can race ahead of
    // the edit's PUT and would otherwise pass the guard.
    const editedSinceRequest = (composeEditedAt.get(info.thread_id) ?? 0) >= requestStartedAt;
    // The inverse-order hole. The edit predates this GET, so
    // `editedSinceRequest` is false, but the debounced PUT settled at or after
    // the GET went out. The server snapshot in this response was therefore
    // read before the PUT committed, and is stale. `pendingComposePuts` no
    // longer covers it, since it cleared at exactly the moment
    // `composePutSettledAt` records.
    const putSettledSinceRequest = (composePutSettledAt.get(info.thread_id) ?? 0) >= requestStartedAt;
    if (!userIsTypingHere && !pendingComposePuts.has(info.thread_id) && !editedSinceRequest && !putSettledSinceRequest) {
      stageDraftFromApi(info, draftBatch);
    }
  }
}

/** Thread IDs loaded ONLY as a family extension: an ancestor or descendant of
 *  a paginated thread. Excluded from the `loadOlderThreads` cursor. Otherwise
 *  an eagerly-loaded family member from far back in history advances the
 *  cursor past every thread between now and itself. An id leaves this set when
 *  natural pagination later returns it as a base thread. */
const familyExtensionIds = new Set<string>();

/** Test-only reset hook. Vitest's per-test `beforeEach` mounts a fresh
 *  threadMap, and module-level state has to follow. Production never calls
 *  this. */
export function _clearFamilyExtensionIdsForTest(): void {
  familyExtensionIds.clear();
}

/** Prevents duplicate concurrent loadAllThreads calls. */
let loadingAll = false;

/** Load thread list metadata, then lazy-load events by priority. Resolves TRUE
 *  when THIS call performed a load, FALSE when it declined.
 *
 *  The boolean is the point of the signature. "Resolved" alone cannot be read
 *  as "the list is now fresh", and `refreshThreadList` has to know the
 *  difference: it retracts its stale-list card on a landed refresh.
 *
 *  Two ways to decline, both FALSE rather than a rejection because nothing
 *  went wrong: the engine is on its way down, or another load is in flight.
 *
 *  A declining caller does NOT await the in-flight load, deliberately. WebKit
 *  routinely leaves a fetch hanging across an iOS suspension, and the
 *  request's own `AbortSignal.timeout` cannot save it, because that timer
 *  freezes with the page. Sharing the promise would turn one such fetch into
 *  something that blocks every later caller for the suspension's length.
 *  Releasing the guard early to escape that puts two loads in flight, where
 *  the older can land last and write stale `meta` over newer. Sharing safely
 *  needs a superseded-attempt token like `fetchAttemptSeq`. */
export async function loadAllThreads(): Promise<boolean> {
  if (loadingAll || engineRestarting.value) return false;
  loadingAll = true;

  try {
    await loadAllThreadsInner();
    return true;
  } finally {
    loadingAll = false;
  }
}

/** Fetch every selectable filter facet, so the drawer "Show" dropdown lists
 *  them all rather than only those in the loaded window. Best-effort: a
 *  failure leaves the dropdown seeded from loaded threads and registries. */
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
  // Family members of the loaded set that were not already loaded. The
  // drawer's family-aware sort needs every member present in threadMap to nest
  // them under their parent. They MUST stay out of the pagination cursor (see
  // `familyExtensionIds`), since their own `last_activity` can be arbitrarily
  // old. The `?? []` is the field's correct semantic default rather than a
  // value-masking fallback, and it keeps a mock that skips the field green.
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
  // thread. The fetchThreads request above already passed `focused` as a hint,
  // so the backend had a chance to include it via `response.focused_thread`.
  // An id still missing from the map after every upsert does not exist
  // server-side.
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
  // Cold boot only, and the condition is load-bearing. This call fetches the
  // recent window with NO filter params and never re-arms `threadHasMore`, so
  // it settles nothing about a selection. It is here purely so the drawer's
  // first mount is not read as a filter change.
  //
  // Stamping unconditionally would let any RESYNC swallow a filter change made
  // while the list was unmounted. Pagination is then armed for the old cursor
  // space with nothing left to notice (see `filterChangedSinceLoad`).
  if (loadedFilterSelection === null) stampLoadedFilterSelection(appliedThreadFilter.value);
  // Collapsed Archive badge total. The inline `archive_count` is the
  // UNFILTERED pile size: correct for the common no-filter drawer, and
  // instant. An active drawer filter, which persists across reloads, makes
  // that global total wrong, so re-fetch the filter-scoped count. `?? 0`
  // degrades gracefully when an older engine or a test mock omits it.
  const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();
  if (sources || triggerIds || repoIds || appIds) {
    void refreshArchivedCount();
  } else {
    archiveThreadCount.value = response.archive_count ?? 0;
  }

  // Refresh the filter-facet set, fire-and-forget, so the drawer "Show"
  // dropdown reflects newly-created and archived threads. Best-effort: the
  // option lists still work from loaded threads if this fails.
  void loadFilterFacets();

  // Load events for the focused, active and saved threads. Everything else
  // loads lazily on focus. The drawer does not need them: its status dot,
  // sections, badges and counts are all `meta`, which the response above just
  // refreshed. These are simply the threads the user is most likely to open
  // next, and pre-loading them is what makes that open instant.
  //
  // Pooled rather than fired at once (`THREAD_EVENTS_FETCH_CONCURRENCY`), with
  // the focused thread first so it claims a slot immediately. Still awaited as
  // a whole, so a caller ordering work after `loadAllThreads` is unaffected.
  const loadIds: string[] = [];
  if (focused && focused !== ghostFocusedThread) loadIds.push(focused);
  for (const t of map.values()) {
    if (t.meta.id !== focused && (activeSet.has(t.meta.id) || t.meta.saved)) {
      loadIds.push(t.meta.id);
    }
  }
  await runWithConcurrency(loadIds, THREAD_EVENTS_FETCH_CONCURRENCY, loadThreadEvents);

  // Bump so CodingAgentControlMenu re-fetches commands after a reload, which
  // SSE does not replay.
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
 *  For a thread-link click, or any flow that knows only the ID. The link then
 *  works even when the thread is too old to be in the loaded list. True means
 *  the thread is now in the map, false that the API has no record of this ID.
 *  A network or HTTP failure propagates, so the caller can surface the real
 *  error instead of a generic "not found".
 *
 *  Reads the BY-ID endpoint, NOT the grouped `GET /api/v1/threads`. The
 *  grouped one assembles five collections at once, which is hundreds of
 *  milliseconds of server time on a large workspace. This needs exactly one
 *  row out of it.
 *
 *  That cost sits on the notification-tap critical path. A tap navigating
 *  outside the loaded window blocks here with nothing on screen. On a cold
 *  push tap the map is always empty, since the deep link dispatches while
 *  `loadAllThreads` is still in flight. */
export async function ensureThreadByIdInMap(threadId: string): Promise<boolean> {
  if (threadMap.value.has(threadId)) return true;
  const requestStartedAt = Date.now();
  const info = await fetchThreadById(threadId);
  // null is the engine's 404, i.e. a real "no such thread" verdict. Anything
  // else threw, so the caller can tell "gone" from "could not ask" and retry
  // only the latter (see focusThreadOrBootstrapResult / landThreadHash).
  if (!info) return false;
  const map = threadMap.value;
  // `saved` rides on the summary itself here. The grouped endpoint conveys it
  // structurally instead, by membership in its `saved` array, which a single
  // row cannot. Both read `thread_summaries.is_saved`. It is absent only from
  // a test mock or an older engine, where unsaved is the honest reading: the
  // Saved section is opt-in, and the next grouped load reconciles either way.
  upsertThread(map, info, info.saved === true, requestStartedAt);
  threadMap.value = new Map(map);
  return true;
}

/** Monotonic token claimed by each in-flight fetch attempt, load or refresh.
 *  Ownership is per ATTEMPT rather than per thread because the entry can be
 *  dropped out from under a live attempt in two ways: `clearThreadFetchGuards`
 *  on resume, and `forceRetryThreadEvents`' deliberate override. Either can put
 *  a second attempt on the same thread. An attempt must not release, or act on,
 *  a slot that is no longer its own. */
let fetchAttemptSeq = 0;

/** Threads with an in-flight load, mapped to the attempt token that owns it.
 *  Prevents duplicate concurrent fetches, and lets a superseded attempt tell
 *  that its own outcome is stale. */
const loadingThreads = new Map<string, number>();

/** The newest refresh attempt claimed per thread, by token. EVERY attempt
 *  claims, and the claim does two jobs.
 *
 *  It decides whose OUTCOME counts. Several attempts can be live on one thread
 *  and they settle in any order, so the losing shape is an older one landing
 *  last. Its failure would raise a card a newer attempt just cleared, and its
 *  success would retract a card a newer failure earned. Only the newest claim
 *  reports. The rows are applied either way, being append-only and gated on
 *  `lastDbSeq`.
 *
 *  And it lets a BACKGROUND caller decline a duplicate (`{ coalesce: true }`).
 *  Three can target one thread at once: a wake's `runResumeSync`, an SSE
 *  `Lagged` firing `resyncLoadedThreads`, and the user opening a thread those
 *  two just marked stale. ONLY those three decline, and the restriction is
 *  load-bearing. Several callers use a refresh as read-after-write PROOF, and
 *  a call that resolves without fetching breaks them.
 *
 *  Cleared with the other guards on resume (`clearThreadFetchGuards`), so a
 *  fetch WebKit left hanging cannot block a thread forever. That reset is also
 *  how two coalescing attempts end up live at once. */
const refreshAttempts = new Map<string, number>();

/** Highest attempt token that has already REPORTED a refresh outcome for a
 *  thread. Distinct from the live claim above, which is released on settle
 *  whether or not the attempt reported anything. That one cannot answer whether
 *  a newer conclusion has landed. Monotonic and never reset: tokens only
 *  increase, so a stale mark can never wrongly admit an older attempt. */
const lastRefreshReport = new Map<string, number>();

/** Threads the watchdog has already force-retried, so a persistent failure
 *  cannot loop. Cleared on resume, so a thread can be retried after an iOS
 *  suspension. */
const forcedRetries = new Set<string>();

/** Are more of this thread's events still on their way?
 *
 *  A *deep link* asks it before calling its target missing. Absence proves
 *  nothing while the transcript is still arriving, and the load path routinely
 *  outruns a few seconds: `loadThreadEvents` retries twice behind a 1s then 2s
 *  backoff, ThreadView's watchdog force-retries a silent thread at 2s, and its
 *  own "Taking too long?" fuse waits 8s. A link that gave up first reported a
 *  change the reader could see on screen a moment later.
 *
 *  THREE terms, because a fetch in flight is only the commonest of them.
 *  `eventsLoaded` false covers the gaps BETWEEN attempts, where the retry
 *  backoff and the watchdog's restart leave no claim standing. The two claims
 *  cover the opposite shape, a thread already loaded and being caught up by a
 *  refresh. That is what an iOS PWA wake leaves on the thread the reader opens.
 *
 *  False for a thread that has left the map, and for one whose load gave up and
 *  said so. Nothing more is coming for either, so the caller may conclude. */
export function threadEventsStillArriving(threadId: string): boolean {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.eventsLoadFailed) return false;
  return !thread.eventsLoaded
    || loadingThreads.has(threadId)
    || refreshAttempts.has(threadId);
}

/** Threads whose events this device may have missed while it was not listening.
 *  See `docs/glossary.md` § "Stale thread events".
 *
 *  A wake, an SSE reopen or a `Lagged` MARKS rather than fetching, and only
 *  the thread the user OPENS is fetched. Nothing a background thread
 *  contributes to the drawer comes from its events: the status dot, the
 *  sections, the badges and the counts are all `meta`, which the single
 *  `loadAllThreads` request in the same sync point refreshes. Its events are
 *  read only once it is on screen, which is when the mark is consumed.
 *
 *  Deliberately NOT one of the guards `clearThreadFetchGuards` resets, and the
 *  distinction is easy to get wrong because both live here. That function runs
 *  at the TOP of `runResumeSync`, the very place the marks are set. Clearing
 *  them there would disable the whole mechanism silently.
 *
 *  Mutated without a paired `threadMap` signal write, unlike its neighbours,
 *  because nothing RENDERS off a mark. It is read imperatively at focus time
 *  to decide whether to issue a fetch. */
let staleThreadEvents = new Set<string>();

/** The value `fetchAttemptSeq` held when the marks were last raised, so that
 *  `staleMarkedAtToken < attemptToken` reads as "this fetch started AFTER the
 *  current gap opened". One number, because both rules that need it reduce to
 *  that same question and the attempt's own token is already the capture.
 *
 *  A fetch issued BEFORE a gap carries a snapshot that predates it, and that
 *  ordering is ordinary rather than a corner case. WebKit routinely leaves a
 *  fetch hanging across an iOS suspension, so any wake with a slow request
 *  outstanding produces it.
 *
 *  Such a fetch may not CLEAR the mark, since the gap it would claim to close
 *  is one it never covered. More sharply, it may not be COALESCED INTO either.
 *  A caller acting on the current mark and handed a pre-mark attempt gets no
 *  request at all. The thread it just opened is then stale with nothing in
 *  flight to fix it. Same discipline as `lastRefreshReport`. */
let staleMarkedAtToken = 0;

/** Record that every loaded thread may be behind.
 *
 *  Rebuilt from `threadMap` rather than added to, so a thread that has left
 *  the map cannot leave an entry behind. Both optimistic-send rollbacks delete
 *  a row carrying `eventsLoaded: true`. That shape accumulates in a long-lived
 *  PWA session, and it is why the failure maps next door need a
 *  `dropDepartedThreads` reconcile at all.
 *
 *  Only `eventsLoaded` threads are marked. A thread with no events is not
 *  behind, it is unloaded, which is the LOAD path's business.
 *  `refreshThreadEvents` would decline it anyway.
 *
 *  The FOCUSED thread is marked with the rest, even though its caller
 *  refreshes it at once. A landed fetch then stays the only thing that clears
 *  a mark. If that refresh fails, the mark survives and re-opening the thread
 *  retries it, rather than the thread waiting for the next sync point. */
export function markLoadedThreadsStale(): void {
  const next = new Set<string>();
  for (const [id, thread] of threadMap.value) {
    if (thread.eventsLoaded) next.add(id);
  }
  staleThreadEvents = next;
  staleMarkedAtToken = fetchAttemptSeq;
}

/** A fetch landed for this thread, so it is no longer behind, unless it started
 *  before the current gap opened (see `staleMarkedAtToken`). */
function clearStaleMark(threadId: string, attemptToken: number): void {
  if (staleMarkedAtToken >= attemptToken) return;
  staleThreadEvents.delete(threadId);
}

/** May a background caller be satisfied by the refresh already in flight for
 *  this thread, rather than issuing its own?
 *
 *  Only when that attempt started after the current gap opened. An older one
 *  cannot answer the caller's question: it will not clear the mark when it
 *  lands, and `clearStaleMark` refuses it for the same reason. Treating it as
 *  a duplicate spends the caller's turn on nothing and leaves the thread stale
 *  with no request covering it.
 *
 *  That is worst where it is least visible, on a thread the user just opened.
 *  `resyncLoadedThreads` marks without resetting the fetch guards, so a
 *  refresh outstanding from before the `Lagged` is still claiming the slot.
 *  `runResumeSync` happens to be immune, but relying on that would make this
 *  correct by accident. */
function mayCoalesceIntoLiveRefresh(threadId: string): boolean {
  const live = refreshAttempts.get(threadId);
  return live !== undefined && live > staleMarkedAtToken;
}

/** The user opened this thread: catch it up if a sync point marked it behind.
 *
 *  The consuming half of the mark, called from `focusThread` beside the
 *  `loadThreadEvents` that covers a thread with no events yet. Deliberately
 *  NOT folded into `loadThreadEvents` itself, tempting though its
 *  `eventsLoaded` early-return is. `loadAllThreadsInner` runs that function
 *  over every active and saved thread. A refresh hidden inside it would
 *  rebuild the fan-out this replaced, through the back door.
 *
 *  Coalesced, because a focus can land while the sync point's own refresh is
 *  still in flight, and because rapid navigation must not stack requests. That
 *  in-flight attempt clears the mark when it lands, but only if it started
 *  after the mark was raised, which is what `mayCoalesceIntoLiveRefresh`
 *  declines on.
 *
 *  Fire-and-forget: `refreshThreadEvents` never rejects and owns its own
 *  reporting, with one keyed card on a verdict and silence on anything
 *  transient. */
export function refreshStaleThreadEvents(threadId: string): void {
  if (!staleThreadEvents.has(threadId)) return;
  void refreshThreadEvents(threadId, { coalesce: true });
}

/** Test-only reset. Module state that outlives a vitest case; production never
 *  calls this. */
export function _resetStaleThreadEventsForTesting(): void {
  staleThreadEvents = new Set();
  staleMarkedAtToken = 0;
}

/** Reset every per-thread fetch guard so a resume starts clean.
 *
 *  Called on iOS PWA resume, where all three guards go stale the same way. A
 *  stale retry cap stops the watchdog retrying a thread whose transient failure
 *  has long since cleared. And WebKit can pause the timers behind an in-flight
 *  fetch during suspension, leaving its promise unresolved and its thread
 *  blocked by the in-flight guard. A duplicate fetch is safe either way,
 *  `applyEventRows` being idempotent against `lastDbSeq`.
 *
 *  Named for the guards rather than for `forcedRetries` alone, which is only
 *  one of the three it clears. Dropping an entry here is what can put a second
 *  attempt on a thread, which is what the per-attempt tokens are for. */
export function clearThreadFetchGuards(): void {
  forcedRetries.clear();
  loadingThreads.clear();
  refreshAttempts.clear();
}

/** Force-retry event loading for a stuck thread. Used by the watchdog in
 *  ThreadView when a thread has no content for >2 seconds.
 *  - Skips if this thread was already force-retried (prevents infinite loop)
 *  - Clears loadingThreads for this thread. The watchdog fires BECAUSE loading
 *    is stuck, so the in-flight fetch is likely hung: iOS suspension can pause
 *    setTimeout timers, leaving fetch Promises unresolved forever. A duplicate
 *    fetch is safe, applyEventRows being idempotent against lastDbSeq.
 *  - Resets eventsLoaded so loadThreadEvents doesn't early-return */
export function forceRetryThreadEvents(threadId: string): void {
  if (forcedRetries.has(threadId)) return;
  // `loadThreadEvents` bails at once while a Switch is in flight. Clearing the
  // flags below would leave the thread in neither resume collection AND spend
  // its one forced retry on nothing. The post-restart resume covers it.
  if (engineRestarting.value) return;
  forcedRetries.add(threadId);
  loadingThreads.delete(threadId);
  const thread = threadMap.value.get(threadId);
  if (thread) {
    thread.eventsLoaded = false;
    thread.eventsLoadFailed = false;
  }
  loadThreadEvents(threadId);
}

/** One of the two per-thread event fetches, paired with the single toast key it
 *  reports through and the copy for its card. See `docs/glossary.md`
 *  § "Thread-events failures" for why the map exists at all. */
type ThreadEventsFailures = {
  key: string;
  /** Threads currently failing this fetch with a VERDICT, the engine having
   *  answered and refused, each mapped to the reason it gave. A Map rather than
   *  a Set, so the card re-renders from whatever is still failing rather than
   *  freezing the count it was first raised with. A transient rejection never
   *  lands here. */
  failing: Map<string, string>;
  /** The card's copy. `subject` is already rendered as a quoted title or a
   *  thread count, so a surface only has to name its own verb. */
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

/** Test-only reset. The maps are module state that outlives a vitest case;
 *  production never calls this. */
export function _resetThreadEventsFailuresForTesting(): void {
  REFRESH_FAILURES.failing.clear();
  LOAD_FAILURES.failing.clear();
}

/** Forget both surfaces' record of a thread, and re-render whatever is left.
 *
 *  For the four moments where something OTHER than a fetch settling proves the
 *  failures over. `dropDepartedThreads` and the two outcome paths all run only
 *  when a later fetch settles, and each of these four can leave none to settle.
 *  The card would then stand for the life of the page.
 *
 *  Two are removals, where nothing will ever fetch the thread again:
 *  `sendMessage`'s optimistic-send rollback and `rollbackOptimistic` in
 *  `compose.ts`. Two are recoveries, where the thread lives on but its failures
 *  are provably over. Those are a full load succeeding, which carries no
 *  `after` and so subsumes a refresh, and the SSE handler clearing
 *  `eventsLoadFailed`. */
export function forgetThreadEventsFailures(threadId: string): void {
  clearThreadEventsFailure(REFRESH_FAILURES, threadId);
  clearThreadEventsFailure(LOAD_FAILURES, threadId);
}

/** Drop entries whose thread has left `threadMap` entirely, which both
 *  optimistic-send rollbacks do (`sendMessage` in chat.ts, `rollbackOptimistic`
 *  in compose.ts). Nothing will ever fetch such a thread again, so its entry
 *  would hold the card open forever, counting a thread the user cannot even see.
 *
 *  The BACKSTOP half of that cleanup. Reconciling where the map is read covers
 *  any future removal path that forgets to say so. The removal sites call
 *  `forgetThreadEventsFailures` directly, because this runs only when some
 *  later fetch settles, and a departed lone entry can leave none. */
function dropDepartedThreads(surface: ThreadEventsFailures): void {
  for (const id of surface.failing.keys()) {
    if (!threadMap.value.has(id)) surface.failing.delete(id);
  }
}

/** Render one surface's single card from whatever is failing RIGHT NOW, or
 *  retract it once nothing is. Called on every change to the map, so the count
 *  can never outlive the failures it describes: nine of ten threads recovering
 *  must not leave a card still claiming ten.
 *
 *  Names the thread when it is the only one failing, a title beating counting
 *  to one. Counts otherwise, a lone thread with no title yet included, since a
 *  user-facing string must never fall back to a raw thread id. The reason shown
 *  is the most recently recorded one, which on a fan-out is the same cause for
 *  every thread in the card.
 *
 *  `raise` is FALSE on the recovery path, which may only UPDATE a card already
 *  on screen and must never create one. Two ways it would otherwise emit a
 *  card at the wrong moment. `showToast` drops everything while
 *  `workspaceUnavailable()` holds, and a database outage is precisely that.
 *  Or the user simply dismissed the card. Either way the FIRST success
 *  afterwards would raise a fresh sticky error carrying a stale reason,
 *  counting down as the rest recover. */
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

/** Record a VERDICT failure for one thread. Both fetches fan out one request
 *  per loaded thread. The map is what makes ten failures one honest card
 *  instead of ten identical ones the user cannot act on.
 *
 *  Silent while the engine is unreachable, exactly as `loadUnreadNotifications`
 *  is. The debounced connection dot already reports an outage once. A card per
 *  thread on top of it is the same fact told N more times. The dot's hysteresis
 *  keeps it green through a brief blip, so a verdict during one is still
 *  surfaced. `'connecting'`, the signal's value before the first health probe
 *  lands, is deliberately included. Reachability is unconfirmed there, and the
 *  load path still flags the thread. */
function recordThreadEventsFailure(surface: ThreadEventsFailures, threadId: string, err: unknown): void {
  if (connectionStatus.value !== 'connected') return;
  // Delete first so a re-recorded thread moves to the END of the insertion
  // order: `Map.set` on an existing key keeps its original position, which would
  // leave the card showing an older thread's reason after this one's cause
  // changed.
  surface.failing.delete(threadId);
  surface.failing.set(threadId, errorDetail(err));
  dropDepartedThreads(surface);
  renderThreadEventsCard(surface, true);
}

/** A fetch landed, so this thread is no longer behind. Re-render from what is
 *  left, which retracts the card when this was the last one. Re-renders only
 *  when the map actually changed, so the ordinary case (nothing failing, every
 *  fetch succeeding) costs one empty loop and no toast churn. */
function clearThreadEventsFailure(surface: ThreadEventsFailures, threadId: string): void {
  const before = surface.failing.size;
  surface.failing.delete(threadId);
  dropDepartedThreads(surface);
  if (surface.failing.size !== before) renderThreadEventsCard(surface, false);
}

/** Load events for a single thread from the snapshot endpoint.
 *  Retries up to 2 times with exponential backoff on failure. */
export async function loadThreadEvents(threadId: string): Promise<void> {
  if (engineRestarting.value) return;
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.eventsLoaded) return;
  if (loadingThreads.has(threadId)) return;
  const attemptToken = ++fetchAttemptSeq;
  loadingThreads.set(threadId, attemptToken);

  // Perf: stamp the open-start for the `thread-render` mark, the moment the
  // focused thread's real event load begins. ThreadView reads and clears it on
  // first content render to measure open-to-paint. Gated to the focused thread,
  // so loadAllThreads' eager loads of non-focused threads leave no stale marks
  // that would later fire a multi-minute renderMs. Covers both the click case
  // and cold start. See utils/threadOpenMarks.ts. Fire-and-forget telemetry.
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
        // Perf instrumentation. Split the open cost into transfer (fetchMs) and
        // store (applyMs), correlated with event count, to show which half
        // dominates on a big coding-agent thread. Fire-and-forget. The grouping
        // half is measured separately in ThreadView. See utils/perfQueue.ts.
        const fetchStart = performance.now();
        const snapshot = await fetchThreadEvents(threadId);
        const fetchMs = performance.now() - fetchStart;
        // The rows are applied straight through, and nothing here may raise
        // the skeleton ahead of the delay gate. Doing so shows a loader
        // instantly and then blocks the paint with the fold, which is how it
        // gets reported. ADR 0081 carries the whole decision.
        //
        // Re-read from current map — the map reference may have changed
        // during the async fetch (other threads loaded/updated).
        const current = threadMap.value.get(threadId);
        if (!current) return;
        const applyStart = performance.now();
        applyEventRows(threadMap.value, threadId, current, snapshot.events, snapshot.currentAggregate);
        const applyMs = performance.now() - applyStart;
        current.eventsLoaded = true;
        // Cleared here, not only at claim time. Another attempt can have set it
        // while this one was in flight. `eventsLoaded && eventsLoadFailed` then
        // paints `ThreadView`'s failed empty state over a loaded thread, its
        // `emptyReason` testing the failure flag first.
        current.eventsLoadFailed = false;
        threadMap.value = new Map(threadMap.value);
        recordPerfSample('thread-load', {
          threadId,
          eventCount: snapshot.events.length,
          channel: current.meta.channel,
          fetchMs: Math.round(fetchMs),
          applyMs: Math.round(applyMs),
        });
        // Unconditional, unlike the refresh's ownership-gated clear: a load
        // SUCCEEDING is terminal. It just set `eventsLoaded`, so no card may go
        // on claiming this device never got the thread's history. BOTH
        // surfaces, because this fetch carries no `after` and returned the whole
        // snapshot. It strictly subsumes a refresh, and a refresh card really
        // does race a full load.
        //
        // Claiming the refresh high-water mark makes that subsumption hold over
        // TIME rather than at this instant. A refresh that started before this
        // load would otherwise still pass its report gate and re-raise the card
        // it just retracted. Monotone-safe: a refresh starting after this load
        // draws a higher token and still reports. `Math.max`, never a bare set,
        // since this load's token was drawn when it STARTED. Lowering the mark
        // would re-admit a third attempt sitting between them.
        lastRefreshReport.set(threadId, Math.max(lastRefreshReport.get(threadId) ?? 0, attemptToken));
        forgetThreadEventsFailures(threadId);
        // Same subsumption, applied to the stale mark: this snapshot carries no
        // `after`, so it holds everything a refresh would have brought and the
        // thread is no longer behind. Gated rather than unconditional, because a
        // mark raised while this load was in flight describes a gap this snapshot
        // may predate (see `staleMarkedAtToken`).
        clearStaleMark(threadId, attemptToken);
        return;
      } catch (err) {
        if (attempt < MAX_RETRIES) {
          // Retry all errors including timeouts — iOS Safari PWA can cause
          // transient AbortErrors during suspend/resume that succeed on retry.
          await new Promise(r => setTimeout(r, BASE_DELAY * (attempt + 1)));
        } else {
          if (engineRestarting.value) {
            // The load genuinely did not land, and the claim-time clear at the
            // top already dropped `eventsLoadFailed`. Leaving it false puts the
            // thread in NEITHER of `runResumeSync`'s collections and below the
            // SSE retraction hook. Nothing would fetch it again, and any card
            // it holds would have no retractor. Restore the honest state, so
            // the post-restart resume picks it up.
            const restarting = threadMap.value.get(threadId);
            if (restarting) {
              restarting.eventsLoadFailed = true;
              threadMap.value = new Map(threadMap.value);
            }
            return;
          }
          // Another attempt (the resume reset or `forceRetryThreadEvents` can
          // put a second one on this thread) already loaded it while this one
          // was in flight. This failure then describes a thread that is loaded
          // and rendering, so reporting it would raise a sticky card and
          // re-flag a healthy thread. Keyed on the terminal success rather than
          // on attempt order, because either order reaches the same state.
          if (threadMap.value.get(threadId)?.eventsLoaded) return;
          // Telemetry carve-out (.claude/rules/frontend.md) for the transient
          // case, same reasoning as `refreshThreadEvents` below. Three attempts
          // that all died without an answer say nothing about the engine.
          // `loadAllThreads` also fans this out across every active and saved
          // thread on boot and on every wake. The user is still told, by the
          // two surfaces that can act on it. `eventsLoadFailed` below paints
          // the focused thread's own failed empty state, and the resume sync
          // retries every thread carrying the flag. A verdict is different: the
          // engine answered and refused, so it reaches the user by card.
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
    // Only while this attempt still owns the slot (see `fetchAttemptSeq`).
    if (loadingThreads.get(threadId) === attemptToken) loadingThreads.delete(threadId);
  }
}

/** Fetch only new events since the thread's last DB-loaded sequence and append them.
 *
 *  Retries once on any `isTransientFetchError` rejection: a cancelled fetch,
 *  the client deadline, or a transport failure. The sync-point paths call this
 *  for the FOCUSED thread on an SSE reconnect, an iOS PWA wake or an engine
 *  restart. iOS Safari both cancels in-flight fetches and fails the first
 *  request on a stale HTTP/2 connection. Both succeed on retry, and this
 *  mirrors the retry pattern in refreshChangesState.
 *
 *  A failure that survives the retry is reported per the split in the catch
 *  below: silence for anything transient, one keyed card on a verdict.
 *
 *  Resolves to whether a snapshot actually LANDED. It never rejects, so a
 *  `.catch` on the call is dead code. A caller using a refresh as
 *  read-after-write proof must tell "the engine answered" from "we declined or
 *  gave up" before acting. `schedulePendingCleanup` is the one with teeth: it
 *  force-drops a pending message on the strength of this answer. */
export async function refreshThreadEvents(
  threadId: string,
  opts: { coalesce?: boolean } = {},
): Promise<boolean> {
  if (engineRestarting.value) return false;
  const map = threadMap.value;
  const thread = map.get(threadId);
  if (!thread || !thread.eventsLoaded) return false;
  // Decline only for a background caller, and only when the live attempt is new
  // enough to answer for it (see `mayCoalesceIntoLiveRefresh`). Every attempt
  // still CLAIMS the slot, because the claim is what decides whose outcome
  // counts (see `refreshAttempts`).
  if (opts.coalesce === true && mayCoalesceIntoLiveRefresh(threadId)) return false;
  const attemptToken = ++fetchAttemptSeq;
  refreshAttempts.set(threadId, attemptToken);
  /** May this attempt report? Only if nothing NEWER has reported already.
   *
   *  Gating on the live claim instead is wrong in both directions, because a
   *  claim is released on settle whether or not the attempt reported. Requiring
   *  the claim silences an older attempt forever once a newer one settles
   *  quietly (its transient arm, or the `engineRestarting` bail), swallowing a
   *  genuine verdict. Accepting a released claim lets an older failure card a
   *  thread a newer attempt just refreshed cleanly. The high-water mark
   *  expresses the rule, and is self-correcting: a newer attempt reporting
   *  later overwrites whatever this one concluded. */
  const mayReport = () => (lastRefreshReport.get(threadId) ?? 0) < attemptToken;
  const markReported = () => lastRefreshReport.set(threadId, attemptToken);

  try {
    const snapshot = await fetchThreadEvents(threadId, thread.lastDbSeq || undefined)
      .catch(err => {
        if (!isTransientFetchError(err)) throw err;
        return fetchThreadEvents(threadId, thread.lastDbSeq || undefined);
      });
    // Always apply currentAggregate even when no new events arrived — the
    // snapshot may have advanced (e.g. status flipped) since the last refresh.
    if (snapshot.events.length > 0 || snapshot.currentAggregate) {
      // Applied even when superseded: rows are append-only and gated on
      // `lastDbSeq`, and `applyEventRows` has its own aggregate staleness guard,
      // so a late snapshot can only add what is missing. Only the REPORTING is
      // ownership-gated, because that is what a superseded attempt gets wrong.
      applyEventRows(map, threadId, thread, snapshot.events, snapshot.currentAggregate);
      threadMap.value = new Map(threadMap.value);
    }
    // The thread is caught up, so drop its stale mark. Outside the ownership
    // gate below, deliberately: a mark is not a REPORT, and this snapshot was
    // applied whichever attempt fetched it, so even a superseded attempt has
    // genuinely closed the gap. Its own start-after-the-mark rule is the
    // narrower one that applies here (see `staleMarkedAtToken`).
    clearStaleMark(threadId, attemptToken);
    // A newer attempt may have failed since; retracting its card here would hide
    // a failure that is still true.
    if (mayReport()) {
      markReported();
      clearThreadEventsFailure(REFRESH_FAILURES, threadId);
    }
    return true;
  } catch (err) {
    if (engineRestarting.value) return false;
    // Telemetry carve-out (.claude/rules/frontend.md): this refresh is
    // BACKGROUND. It runs on a wake, an SSE reconnect, an engine restart, a
    // focus onto a stale thread, and behind a few read-after-write heals.
    // None of those is the user's click itself. The two that follow one each
    // toast their own failure on the next line, and the focus case already
    // painted the transcript.
    //
    // Every rejection that carries no ANSWER is suppressed. WebKit aborts the
    // in-flight fetch on the iOS suspend boundary, and a freshly-resumed
    // page's stale HTTP/2 connection fails at the transport layer. A dropped
    // tunnel drops packets rather than refusing, so the request hangs to the
    // client deadline: on this path the hang IS the outage's shape. A
    // sustained outage is the debounced connection dot's to report, once.
    //
    // Self-recovery: a refresh that did not land leaves the thread's stale
    // mark set, so re-opening it retries. Failing that, the next SSE event
    // re-syncs via handleThreadEvent, and the next sync point marks again.
    //
    // A VERDICT means the engine answered and refused, so it reaches the user
    // through one keyed card, however many threads are failing.
    if (isTransientFetchError(err)) {
      console.warn(`[ThreadLoading] refresh failed transiently for ${threadId} (iOS PWA wake / engine restart); SSE will recover`, err);
      return false;
    }
    console.warn(`[ThreadLoading] Failed to refresh events for ${threadId}:`, err);
    // A newer attempt may have already refreshed this thread cleanly. Raising a
    // card off this stale failure would report a thread that is up to date.
    if (mayReport()) {
      markReported();
      recordThreadEventsFailure(REFRESH_FAILURES, threadId, err);
    }
    return false;
  } finally {
    // Release only the claim this attempt made (see `fetchAttemptSeq`).
    if (refreshAttempts.get(threadId) === attemptToken) refreshAttempts.delete(threadId);
  }
}

/** Re-arm pagination and eagerly fetch the first page of threads matching the
 *  newly-applied filter.
 *
 *  The drawer's IntersectionObserver sentinel cannot populate a freshly-filtered
 *  view. Its fill loop is suppressed while the Archive section is collapsed
 *  (`archivePaginationAllowed`) and only re-fires on a scroll transition.
 *  Selecting a facet whose matches are all archived would strand the user on
 *  "No threads". The drawer calls this whenever the selection changes, so
 *  picking a facet deterministically shows its threads. `loadOlderThreads`
 *  self-guards against concurrent calls and falls back to a now()-cursor when
 *  no loaded thread matches, so this is safe on every filter change. */
export async function reloadAfterFilterChange(): Promise<void> {
  // Different filter = different cursor space; clear any stale "no more" from
  // the previous selection before fetching, or loadOlderThreads early-returns.
  threadHasMore.value = true;
  // Re-fetch the filter-scoped Archive badge total so it reflects the new
  // selection immediately (stable, server-sourced — not the loaded count).
  void refreshArchivedCount();
  // No stamp here, deliberately: `loadOlderThreads` stamps what it fetched.
  // Stamping the INTENT before the await records a lie whenever the call
  // declines. It declines on `threadLoadingMore` while a previous selection's
  // request is in flight. Toggling a second facet inside that round trip would
  // leave the stamp claiming the second while only the first was fetched.
  await loadOlderThreads();
}

/** The selection the loaded thread window was last FETCHED against. Set only
 *  where a fetch settles the question for a selection, which is what makes it
 *  safe to read as "nothing is owed". Those are `loadOlderThreads` after the
 *  server answers, each of its answered-without-asking branches, and the
 *  initial window load on a cold boot.
 *
 *  The *applied thread filter*, not the live signals. It is stamped and
 *  compared against the same selection the drawer displays and the cursor
 *  pages. Its identity changes exactly when the selection's CONTENTS change,
 *  `appliedThreadFilter` comparing contents before it replaces the object, so
 *  one identity comparison is the whole check. */
let loadedFilterSelection: ThreadFilterSelection | null = null;

/** Takes the selection RATHER than reading the signal, and callers pass the one
 *  their request was issued for. The settle sites are on the far side of an
 *  await, and the signal can have moved since. Stamping what it says there
 *  records the wrong thing, in the same shape as stamping an intent.
 *
 *  A page fetched for A that lands after the user picked B marks B as fetched.
 *  Nothing then owes B its reload, its archived count, or its first page. An
 *  empty response is worse, also writing `threadHasMore = false` onto B's
 *  untouched cursor space. */
function stampLoadedFilterSelection(selection: ThreadFilterSelection): void {
  loadedFilterSelection = selection;
}

/** True when the drawer filter has moved since the loaded window was fetched,
 *  so `reloadAfterFilterChange` still owes it a re-arm and a first page.
 *
 *  **A STORE fact, not a per-component one.** The drawer's list only mounts
 *  under the default `all` status, `ThreadDrawer` rendering the four other
 *  lists instead. The Filter panel can change the selection under any of them.
 *  A per-mount marker answers only whether the filter changed while that mount
 *  was watching. A change made under a status view would be seen by nobody, and
 *  the catch-up suppressed on the way back too.
 *
 *  It also answers from any moment, which a mount-independent marker still
 *  would not give. A claim taken when the reload is REQUESTED goes quiet even
 *  though `loadOlderThreads` can decline or fail. Nothing then owes a selection
 *  the server was never asked about. */
export function filterChangedSinceLoad(): boolean {
  return loadedFilterSelection !== appliedThreadFilter.value;
}

/** Test seam: forget which selection the window was fetched against. */
export function _clearLoadedFilterSelectionForTest(): void {
  loadedFilterSelection = null;
}

/** Channel/facet filter params for the older-threads + archived-count APIs,
 *  read from the *applied thread filter*: the selection the drawer list is
 *  actually showing, which holds still while the thread filter panel covers it
 *  (see `store/appliedThreadFilter.ts`). `sources` is undefined when every
 *  channel is selected (no narrowing); each facet array is undefined when its
 *  selection is empty. Shared by `loadOlderThreads` (pagination cursor space)
 *  and `refreshArchivedCount` (badge total) so both target the identical set. */
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

/** Refresh `archiveThreadCount`, the collapsed Archive badge total, so it
 *  reflects the ACTIVE drawer filter. It stays stable however many rows are
 *  paginated in: the badge reads this signal directly and must NOT change as
 *  the user scrolls or expands the section. Fetches the true server-side count
 *  of archived, unsaved threads matching the current selection.
 *
 *  Best-effort. On failure the badge keeps its previous value and the next
 *  filter change re-fetches. It is an informational count, not a blocking
 *  fetch, the rows themselves still loading via pagination. So a transient miss
 *  must not pop a toast (per the frontend telemetry carve-out). */
export async function refreshArchivedCount(): Promise<void> {
  // Empty channel filter = nothing shown by intent → the archive is empty.
  if (appliedThreadFilter.value.channels.size === 0) {
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
  // Neither of these settles anything for the current selection: a concurrent
  // call owns the round trip, or pagination is simply off. So no stamp, and
  // whatever owed a reload still owes it (see `filterChangedSinceLoad`).
  if (threadLoadingMore.value || !threadHasMore.value) return;
  const applied = appliedThreadFilter.value;
  // Empty filter = nothing visible by intent; never fetch. That IS the answer
  // for this selection, so it stamps like a landed page.
  if (applied.channels.size === 0) {
    threadHasMore.value = false;
    stampLoadedFilterSelection(applied);
    return;
  }
  threadLoadingMore.value = true;

  try {
    const map = threadMap.value;
    const { sources, triggerIds, repoIds, appIds } = currentThreadFilterParams();

    let oldestTime: string | null = null;
    for (const t of map.values()) {
      if (t.meta.saved) continue;
      // `t.meta.section` is the raw `archive_state`. The cursor tracks the
      // Archive pile, which `get_recent_threads` returns as one contiguous
      // `created_at DESC` window with no out-of-window injection. Every loaded
      // archived row is a contiguous member, so there are no outliers.
      if (t.meta.section !== 'archived') continue;
      // Family-extension threads are loaded eagerly so the drawer can nest
      // them under their parent, but their own `last_activity` can be
      // arbitrarily old. Letting one drive the cursor would jump natural
      // pagination over every intervening thread.
      if (familyExtensionIds.has(t.meta.id)) continue;
      // Same predicate AND the same *applied thread filter* the display reads.
      // The cursor is then the oldest loaded thread matching what is on screen,
      // and cannot drift from it.
      if (!threadPassesChannelFilter(t, applied.channels, applied.triggerIds, applied.repoIds, applied.appIds)) continue;
      // Cursor on created_at, the column the backend `get_older_threads` pages
      // by, so the cursor axis matches the Archive sort axis. Otherwise
      // pagination skips or repeats rows. Same fallback chain as `byCreated`.
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
        stampLoadedFilterSelection(applied);
        return;
      }
      oldestTime = new Date().toISOString();
    }

    const response = await fetchOlderThreads(oldestTime, 15, sources, triggerIds, repoIds, appIds);
    // The server has answered for THESE params. Every way out below is a
    // settled answer, so the stamp goes here rather than at each of them. It
    // stamps `applied`, captured before the await. The selection may have moved
    // on while this was in flight, and this answer says nothing about the new
    // one.
    stampLoadedFilterSelection(applied);
    // `threadHasMore` describes the CURRENT cursor space, so only an answer
    // still about the current selection may write it. Take a page fetched for A
    // that lands after the user moved to B. It would disarm pagination for a
    // cursor space the server was never asked about. Nothing re-arms it: the
    // reload B is owed has already run and declined on `threadLoadingMore`,
    // this very call, and the drawer's effect waits for the selection to move.
    // B is left short with its sentinel gone. The rows themselves are kept
    // either way, being threads in the map rather than a verdict about a query.
    const stillCurrent = appliedThreadFilter.value === applied;
    if (response.threads.length === 0) {
      if (stillCurrent) threadHasMore.value = false;
      return;
    }

    let added = 0;
    for (const info of response.threads) {
      if (!map.has(info.thread_id)) {
        upsertThread(map, info, false);
        added++;
      }
      // Promote. A thread loaded earlier as a family extension, now showing up
      // in natural pagination, is no longer family-only. It should contribute
      // to the cursor on subsequent calls.
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
      if (stillCurrent) threadHasMore.value = false;
      return;
    }

    if (stillCurrent) threadHasMore.value = response.has_more;
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
    // fetched before a live event this device already applied is stale.
    // Applying it would regress status, updatedAt and counts back in time, most
    // visibly as the dot stuck on "running" until reload. Skip the overlay
    // entirely: the fresher live SSE state stands. New event rows above are
    // folded in first and advance updatedAt, so a refresh that brought
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

  // Ring the per-thread bell once after the batch replay, so subscribers to
  // this thread's `events` and `streamingBuffer` recompute. Cheaper than
  // calling per-row. The caller separately writes `threadMap` to fire wide
  // meta subscribers.
  //
  // The aggregate-only branch deliberately does NOT bump. `computeExchanges`
  // reads only `meta.channel`, events and pendingUserMessages, and
  // `meta.channel` is the one `meta` field a bump subscriber would read.
  // `channel` only changes via the per-row `isChannelDefiningEvent` branch
  // above, which requires `rows.length > 0`.
  if (rows.length > 0) {
    bumpThreadEvents(threadId);
    // Replay is the ONLY path that learns about a submission made while this
    // device was asleep / disconnected, and it runs LAST: `resyncLoadedThreads`
    // fires `loadAllThreads` (whose empty-compose snapshot has no evidence yet)
    // before `refreshThreadEvents` brings the missed messages in. Without this
    // reconcile, a draft submitted from another device during the gap would
    // never be re-examined and would sit in the composer indefinitely.
    clearSupersededDraft(threadId);
  }
}
