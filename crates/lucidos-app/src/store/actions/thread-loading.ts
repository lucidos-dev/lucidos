import { threadMap, focusedThreadId, setFocusedThread, showToast, removeToast, connectionStatus, threadsLoaded, generatedTitleIds, threadHasMore, threadLoadingMore, archiveThreadCount, ALL_CHANNELS, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, filterFacets, codingAgentSessionVersion, engineRestarting, archivingThreadIds, CODING_AGENT_CHANNEL, toasts, THREAD_EVENTS_LOAD_TOAST_KEY, THREAD_EVENTS_REFRESH_TOAST_KEY, THREAD_EVENTS_FETCH_CONCURRENCY, threadChannelToFilterSource, type ThreadFilterSource } from '../store';
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
import { pendingComposePuts, composeEditedAt, composePutSettledAt, hasUnsentLocalDraft, clearSupersededDraft, noteServerDraft, noteComposeEpoch } from './compose';

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
      liveEventWaits: [],
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
/** True when a thread-summary snapshot carries no compose draft (no text, no
 *  images, no mode) — the "shared draft was sent/discarded (by anyone)" shape
 *  the backend leaves after clearing compose fields. One helper so the upsert
 *  focus-gate and stageDraftFromApi's clear decision can't drift apart (if they
 *  disagreed, a focused draft could pass the gate then get re-staged). */
function composeSnapshotIsEmpty(info: ThreadSummary): boolean {
  return (info.compose_text || '') === '' &&
    (info.compose_images || []).length === 0 &&
    (info.compose_mode ?? null) === null;
}

function stageDraftFromApi(info: ThreadSummary, batch?: DraftBatch): void {
  const text = info.compose_text || '';
  // The backend column is still named `compose_images` (Phase 5 cleanup
  // will rename it); post-migration the JSONB array contains hash strings.
  const image_hashes = info.compose_images || [];
  // The snapshot is the server telling us what it holds — record it before any
  // guard, since the guard below may keep a local draft the server no longer
  // has, and that difference is exactly what `serverDraft` exists to state.
  // Callers gate this whole function on the staleness guards, so a snapshot
  // known to predate a local write never lands here.
  noteServerDraft(info.thread_id, text, image_hashes);
  // The snapshot's *compose epoch*, recorded on the same "this is what the
  // engine holds" footing as the draft itself, and likewise before the guard
  // below can decide to keep a local draft instead.
  noteComposeEpoch(info.thread_id, info.compose_epoch);
  const mode = info.compose_mode ?? null;
  const isEmpty = composeSnapshotIsEmpty(info);
  // A bulk loadAllThreads/upsert snapshot must NEVER clear a non-empty draft
  // the user typed ON THIS DEVICE. The compose PUT is debounced and can fail or
  // time out under host contention; `composePutSettledAt` is then stamped (even
  // on failure) and a later resync's GET — fired AFTER that stamp, reading the
  // server before our text ever committed — returns an empty compose. None of
  // the timing guards in `upsertThread` (typing-here / pendingComposePuts /
  // composeEditedAt / composePutSettledAt) catch that post-settle stale-empty
  // read, so without this it blanks the just-typed draft (the value='' face of
  // mobile-webkit drafts.spec.ts:65 — and a real way a transient sync failure
  // silently drops an unsent draft). Gate on `hasUnsentLocalDraft` so this only
  // protects locally-authored drafts that are genuinely unsent: a
  // server-ORIGINATED draft (cross-device, never edited here) can still be
  // cleared by a snapshot, a draft the thread's history shows was already
  // submitted is cleared too, and a peer's clear still flows through the SSE
  // `ThreadComposeChanged` path. The kept draft is local-view only — it
  // schedules no PUT, so it never resurrects server-side unless the user
  // resumes editing it.
  if (isEmpty && hasUnsentLocalDraft(info.thread_id)) {
    return;
  }
  // Rehydrate the per-draft dropdown selection from the DB (the authoritative
  // store) so a reload restores the draft's picks. Past the local-edit guard
  // above, so it won't clobber a locally-edited draft; the caller
  // (`upsertThread`) already gates this whole call on the compose staleness
  // guards. `null`/absent = no stored selection → clears the local entry.
  setComposeSelectionFromServer(info.thread_id, info.compose_selection);
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
    existing.meta.liveEventWaitCount = info.live_event_wait_count || 0;
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
    //   1. User is mid-edit on this thread's textarea (focus + matching id) —
    //      but ONLY for a NON-empty snapshot (see below).
    //   2. A debounced/in-flight PUT covers the value the API would clobber.
    //   3. A local edit happened AFTER this GET went out — its response is
    //      stale wrt compose by definition. Without this, picker dismissal +
    //      slow loadAllThreads + fast PUT races to overwrite the optimistic
    //      image (preview appears, then disappears).
    existing.meta.state = info.state;
    const isFocusedThread = info.thread_id === focusedThreadId.value;
    // An EMPTY server snapshot is a genuine "the shared draft was sent/discarded
    // (by anyone)" signal — the backend clears compose_text on MessageReceived /
    // SessionStarted / ThreadDiscarded. Focus must NOT block that clear, or a
    // synced-from-peer draft stays as a ghost in a focused-but-untyped textarea
    // (the same authorship-vs-focus bug fixed in the SSE ThreadComposeChanged /
    // MessageReceived paths). Unsent locally-authored work is still protected:
    // the other staleness guards below AND stageDraftFromApi's own
    // hasUnsentLocalDraft empty-guard both keep it (a draft whose text has since
    // been submitted is NOT such work and clears). A NON-empty snapshot keeps
    // the focus guard so a background refresh can't move the cursor / overwrite
    // what the user sees.
    const snapshotIsEmpty = composeSnapshotIsEmpty(info);
    const userIsTypingHere = isFocusedThread && isComposeFocusedHere(info.thread_id) && !snapshotIsEmpty;
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
  // on focus. Not because the drawer needs them (it does not: its status dot,
  // sections, badges and counts are all `meta`, which the response above just
  // refreshed for every thread it returned), but because these are the threads
  // the user is most likely to open next, and pre-loading them while they read
  // the drawer is what makes that open instant. Both sets are small.
  //
  // Pooled rather than fired all at once (see `THREAD_EVENTS_FETCH_CONCURRENCY`);
  // the focused thread is first in the list so it claims a slot immediately.
  // Still awaited as a whole, so callers that order work after `loadAllThreads`
  // are unaffected.
  const loadIds: string[] = [];
  if (focused && focused !== ghostFocusedThread) loadIds.push(focused);
  for (const t of map.values()) {
    if (t.meta.id !== focused && (activeSet.has(t.meta.id) || t.meta.saved)) {
      loadIds.push(t.meta.id);
    }
  }
  await runWithConcurrency(loadIds, THREAD_EVENTS_FETCH_CONCURRENCY, loadThreadEvents);

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
 *  caller can surface the real error instead of a generic "not found".
 *
 *  Reads the BY-ID endpoint, not the grouped `GET /api/v1/threads`. The grouped
 *  one assembles saved + recent archive + active + composing + the family base,
 *  which is hundreds of milliseconds of server time on a large workspace, and
 *  this function needs exactly one row out of it. That cost sat directly on the
 *  notification-tap critical path: a tap that navigates to a thread outside the
 *  loaded window blocks here with nothing on screen, and on a cold push tap the
 *  map is always empty (the deep link dispatches while `loadAllThreads` is still
 *  in flight), so it fired on every single tap AND duplicated the grouped fetch
 *  the same boot had already issued. */
export async function ensureThreadByIdInMap(threadId: string): Promise<boolean> {
  if (threadMap.value.has(threadId)) return true;
  const requestStartedAt = Date.now();
  const info = await fetchThreadById(threadId);
  // null is the engine's 404, i.e. a real "no such thread" verdict. Anything
  // else threw, so the caller can tell "gone" from "could not ask" and retry
  // only the latter (see focusThreadOrBootstrapResult / landThreadHash).
  if (!info) return false;
  const map = threadMap.value;
  // `saved` rides on the summary itself here; the grouped endpoint conveys it
  // structurally instead (membership in its `saved` array), which a single row
  // cannot. Both read `thread_summaries.is_saved`. Absent only from a test mock
  // or an engine predating the field, where unsaved is the honest reading: the
  // Saved section is opt-in, and the next grouped load reconciles either way.
  upsertThread(map, info, info.saved === true, requestStartedAt);
  threadMap.value = new Map(map);
  return true;
}

/** Monotonic token claimed by each in-flight fetch attempt, load or refresh.
 *  Ownership is per ATTEMPT rather than per thread because the entry can be
 *  dropped out from under a live attempt in two ways: `clearThreadFetchGuards`
 *  on resume, and `forceRetryThreadEvents`' deliberate override. Either can put
 *  a second attempt on the same thread, and an attempt must not release, or act
 *  on, a slot that is no longer its own. */
let fetchAttemptSeq = 0;

/** Threads with an in-flight load, mapped to the attempt token that owns it.
 *  Prevents duplicate concurrent fetches, and lets a superseded attempt tell
 *  that its own outcome is stale. */
const loadingThreads = new Map<string, number>();

/** The newest refresh attempt claimed per thread, by token. EVERY attempt
 *  claims, and the claim does two jobs.
 *
 *  It decides whose OUTCOME counts. Several attempts can be live on one thread
 *  (see below), and they settle in any order, so the losing shape is an older
 *  one landing last: its failure would raise a card for a thread a newer attempt
 *  just refreshed cleanly, and its success would retract a card a newer failure
 *  had just earned. Only the newest claim reports. The fetched rows are applied
 *  either way, since they are append-only, gated on `lastDbSeq`, and covered by
 *  `applyEventRows`' own aggregate staleness guard.
 *
 *  And it lets a BACKGROUND caller decline a duplicate (`{ coalesce: true }`).
 *  Three of them can target one thread at once: a wake's `runResumeSync`, an SSE
 *  `Lagged` firing `resyncLoadedThreads`, and the user opening a thread the two
 *  of them just marked stale. Only those three decline, and the restriction is
 *  load-bearing rather than conservatism: several callers use a refresh as
 *  read-after-write PROOF and are broken by a call that resolves without
 *  fetching. `schedulePendingCleanup` treats a resolved refresh as evidence that
 *  a pending message is genuinely absent and force-drops it (losing a persisted
 *  message the user just sent), the empty-focused-thread recovery in
 *  `checkConnection` would spend one of three budgeted attempts per tick, and
 *  the cancel / queued-message heals need a request issued AFTER their own POST
 *  returned. None of those is a duplicate of the in-flight request, so none may
 *  be coalesced into it.
 *
 *  Cleared with the other guards on resume (`clearThreadFetchGuards`) so a fetch
 *  WebKit left hanging cannot block a thread forever. That reset is also how two
 *  coalescing attempts end up live at once: a second resume arriving while the
 *  first fan-out is still going (the 1s coalescing gate and `resumeInFlight`
 *  both expire well before a slow refresh settles) drops the claim and starts a
 *  fresh attempt. */
const refreshAttempts = new Map<string, number>();

/** Highest attempt token that has already REPORTED a refresh outcome for a
 *  thread. Distinct from the live claim above, which is released on settle
 *  whether or not the attempt reported anything, and so cannot answer "has a
 *  newer conclusion already landed?". Monotonic and never reset: tokens only
 *  increase, so a stale mark can never wrongly admit an older attempt. */
const lastRefreshReport = new Map<string, number>();

/** Tracks threads that have already been force-retried by the watchdog.
 *  Prevents infinite retry loops on persistent failures.
 *  Cleared on resume so threads can be retried after iOS suspend/resume. */
const forcedRetries = new Set<string>();

/** Threads whose events this device may have missed while it was not listening.
 *  See `docs/glossary.md` § "Stale thread events".
 *
 *  A wake, an SSE reopen or a `Lagged` used to answer that possibility by
 *  fetching: one incremental refresh per loaded thread, bounded to four at a
 *  time but still one request for every thread in the map. It marks instead, and
 *  only the thread the user OPENS is fetched. Nothing a background thread
 *  contributes to the drawer comes from its events: the status dot, the sections,
 *  the badges and the counts are all `meta`, which the single `loadAllThreads`
 *  request in the same sync point refreshes for every thread it returns. Its
 *  events are read only once it is on screen, which is exactly when the mark is
 *  consumed.
 *
 *  Deliberately NOT one of the guards `clearThreadFetchGuards` resets, and the
 *  distinction is easy to get wrong because both live here: that function is
 *  called at the TOP of `runResumeSync`, which is the very place the marks are
 *  set, so clearing them there would leave every stale thread reading as fresh
 *  and silently disable the whole mechanism.
 *
 *  Mutated without a paired `threadMap` signal write, unlike its neighbours
 *  `eventsLoaded` / `eventsLoadFailed`: nothing RENDERS off a mark. It is read
 *  imperatively at focus time to decide whether to issue a fetch. */
let staleThreadEvents = new Set<string>();

/** The value `fetchAttemptSeq` held when the marks were last raised, so that
 *  `staleMarkedAtToken < attemptToken` reads as "this fetch started AFTER the
 *  current gap opened". One number, because both rules that need it reduce to
 *  that same question and the attempt's own token is already the capture.
 *
 *  It matters because a fetch issued BEFORE a gap carries a snapshot that
 *  predates it, and that ordering is ordinary rather than a corner case: WebKit
 *  routinely leaves a fetch hanging across an iOS suspension, so "request out,
 *  device sleeps, events emitted, device wakes and marks, request finally lands"
 *  happens on any wake with a slow request outstanding. Such a fetch may not
 *  CLEAR the mark, since the gap it would be claiming to close is one it never
 *  covered; and, more sharply, it may not be COALESCED INTO either, because a
 *  caller acting on the current mark that is handed a pre-mark attempt gets no
 *  request at all, leaving the thread it just opened stale with nothing in
 *  flight to fix it. Same discipline as `lastRefreshReport`. */
let staleMarkedAtToken = 0;

/** Record that every loaded thread may be behind.
 *
 *  Rebuilt from `threadMap` rather than added to, so a thread that has left the
 *  map cannot leave an entry behind. Both optimistic-send rollbacks
 *  (`sendMessage` in chat.ts, `rollbackOptimistic` in compose.ts) delete a row
 *  carrying `eventsLoaded: true`, which is exactly the shape that would accumulate
 *  in a long-lived PWA session, and it is why the failure maps next door need a
 *  `dropDepartedThreads` reconcile at all.
 *
 *  Only `eventsLoaded` threads are marked. A thread with no events is not behind,
 *  it is unloaded, which is the LOAD path's business (`loadThreadEvents`, and the
 *  failed-load retry in `runResumeSync`); `refreshThreadEvents` would decline it
 *  anyway. The FOCUSED thread is marked with the rest even though its caller
 *  refreshes it immediately, so that a landed fetch stays the only thing that
 *  clears a mark: if that refresh fails, the mark survives and re-opening the
 *  thread retries it, instead of the thread waiting for the next sync point. */
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
 *  cannot answer the question the caller is asking: it will not clear the mark
 *  when it lands (`clearStaleMark` refuses it for the same reason), so treating
 *  it as a duplicate spends the caller's turn on nothing and leaves the thread
 *  stale with no request covering it. That is worst exactly where it is least
 *  visible, on a thread the user has just opened: `resyncLoadedThreads` marks
 *  without resetting the fetch guards, so a refresh outstanding from before the
 *  `Lagged` is still claiming the slot. (`runResumeSync` happens to be immune,
 *  since `clearThreadFetchGuards` drops every claim just before it marks, but
 *  relying on that would make this correct by accident.) */
function mayCoalesceIntoLiveRefresh(threadId: string): boolean {
  const live = refreshAttempts.get(threadId);
  return live !== undefined && live > staleMarkedAtToken;
}

/** The user opened this thread: catch it up if a sync point marked it behind.
 *
 *  The consuming half of the mark, called from `focusThread` beside the
 *  `loadThreadEvents` that covers a thread with no events yet. Deliberately NOT
 *  folded into `loadThreadEvents` itself, tempting though its `eventsLoaded`
 *  early-return is: that function is also what `loadAllThreadsInner` runs over
 *  every active and saved thread, so a refresh hidden inside it would rebuild the
 *  fan-out this replaced, through the back door.
 *
 *  Coalesced, because a focus can land while the sync point's own refresh of the
 *  same thread is still in flight, and because rapid navigation must not stack
 *  requests. That in-flight attempt clears the mark when it lands, but only if it
 *  started after the mark was raised, which is exactly the condition
 *  `mayCoalesceIntoLiveRefresh` declines on.
 *
 *  Fire-and-forget: `refreshThreadEvents` never rejects and owns its own
 *  reporting (one keyed card on a verdict, silence on anything transient). */
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
 *  Called on iOS PWA resume, where all three guards go stale the same way:
 *  a stale retry cap stops the watchdog retrying a thread whose transient
 *  failure has long since cleared, and WebKit can pause or drop the timers
 *  behind an in-flight fetch during suspension, leaving its promise unresolved
 *  forever and its thread permanently blocked by the in-flight guard. A
 *  duplicate fetch is safe either way: `applyEventRows` is idempotent (it
 *  checks `lastDbSeq`).
 *
 *  Named for the guards rather than for `forcedRetries` alone, which is only
 *  one of the three it clears. Dropping an entry here is exactly what can put a
 *  second attempt on a thread, which is what the per-attempt tokens are for. */
export function clearThreadFetchGuards(): void {
  forcedRetries.clear();
  loadingThreads.clear();
  refreshAttempts.clear();
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
  // `loadThreadEvents` bails at once while a Switch is in flight, so clearing
  // the flags below would leave the thread in neither resume collection AND
  // spend its one forced retry on nothing. The post-restart resume covers it.
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
  /** Threads currently failing this fetch with a VERDICT (the engine answered
   *  and refused), each mapped to the reason it gave. A Map rather than a Set so
   *  the card can be re-rendered from whatever is still failing, reason
   *  included, instead of freezing the count it was first raised with. A
   *  transient rejection never lands here. */
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
 *  failures over, which matters because `dropDepartedThreads` and the two
 *  outcome paths all run only when a later fetch settles, and each of these can
 *  leave none to settle. The card would then stand for the life of the page.
 *
 *  Two are removals, where nothing will ever fetch the thread again:
 *  `sendMessage`'s optimistic-send rollback and `rollbackOptimistic` in
 *  `compose.ts`. Two are recoveries, where the thread lives on but its failures
 *  are provably over: a full load succeeding (it carries no `after`, so it
 *  subsumes a refresh too), and the SSE handler clearing `eventsLoadFailed`,
 *  which is the thread's last exit from the failed-load retry set. */
export function forgetThreadEventsFailures(threadId: string): void {
  clearThreadEventsFailure(REFRESH_FAILURES, threadId);
  clearThreadEventsFailure(LOAD_FAILURES, threadId);
}

/** Drop entries whose thread has left `threadMap` entirely, which both
 *  optimistic-send rollbacks do (`sendMessage` in chat.ts, `rollbackOptimistic`
 *  in compose.ts). Nothing will ever fetch such a thread again, so its entry
 *  would hold the card open forever, counting a thread the user cannot even see.
 *
 *  The BACKSTOP half of that cleanup: reconciling where the map is read covers
 *  any future removal path that forgets to say so, while the removal sites call
 *  `forgetThreadEventsFailures` directly because this only runs when some later
 *  fetch settles, and a departed lone entry can leave none. */
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
 *  Names the thread when it is the only one failing (a title beats counting to
 *  one) and counts otherwise, including for a lone thread with no title yet,
 *  because a user-facing string must never fall back to a raw thread id. The
 *  reason shown is the most recently recorded one, which on a fan-out is the
 *  same cause for every thread in the card.
 *
 *  `raise` is FALSE on the recovery path, where this may only update a card that
 *  is already on screen and must never create one. Two ways it would otherwise
 *  emit a card at exactly the wrong moment, both of which invert the contract:
 *  `showToast` drops everything while `workspaceUnavailable()` holds, and a
 *  database outage is precisely that (the engine keeps answering `/health`, so
 *  every fetch 500s into this map while every card is suppressed); and the user
 *  may simply have dismissed the card. In both cases the FIRST success afterwards
 *  would raise a brand-new sticky error carrying the now-stale reason, counting
 *  down as the rest recover: a card emitted by the recovery and never by the
 *  failure. */
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

/** Record a VERDICT failure for one thread. Both fetches fan out one request per
 *  loaded thread, so the map is what makes "ten threads failed" one honest card
 *  instead of ten identical ones the user cannot act on.
 *
 *  Silent while the engine is unreachable, exactly as `loadUnreadNotifications`
 *  is: an outage is already reported, once, by the debounced connection dot, and
 *  a card per thread on top of it is the same fact told N more times. The dot's
 *  hysteresis keeps it green through a brief blip, so a verdict during one is
 *  still surfaced. `'connecting'` (the signal's value before the first health
 *  probe lands) is deliberately included: reachability is unconfirmed there, and
 *  the load path still flags the thread so the focused one paints its own failed
 *  state. */
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
        // Cleared here, not only at claim time: another attempt can have set it
        // while this one was in flight, and `eventsLoaded && eventsLoadFailed`
        // paints `ThreadView`'s failed empty state over a thread that loaded
        // (its `emptyReason` tests the failure flag first).
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
        // SUCCEEDING is terminal. It just set `eventsLoaded`, so "this device
        // never got the thread's history" is false no matter which attempt got
        // there, and a card still claiming it would sit over a thread that is
        // rendering. BOTH surfaces, because this fetch carries no `after` and so
        // returned the whole snapshot including the newest rows: it strictly
        // subsumes a refresh, which makes "did not get the newest events" false
        // too. (The focused-thread recovery and the ThreadView watchdog fire on
        // the same condition, so a refresh card and a full load really do race.)
        //
        // Claiming the refresh high-water mark is what makes that subsumption
        // hold over TIME rather than just at this instant: a refresh that
        // started before this load would otherwise still pass its report gate
        // and re-raise the card it just retracted, over a thread now holding the
        // whole snapshot. Monotone-safe, since a refresh starting after this
        // load draws a higher token and still reports. `Math.max`, never a bare
        // set: this load's token was drawn when it STARTED, so a refresh that
        // has already reported can hold a higher one, and lowering the mark
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
            // top already dropped `eventsLoadFailed`. Leaving it false here puts
            // the thread in NEITHER of `runResumeSync`'s collections and below
            // the SSE retraction hook, so nothing would fetch it again, and any
            // card it already has would have no retractor. Restore the honest
            // state so the post-restart resume picks it up.
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
          // case, same reasoning as `refreshThreadEvents` below: three attempts
          // that all died without an answer (browser-cancelled, the 10s client
          // deadline, or a stale-connection TypeError) say nothing about the
          // engine, and `loadAllThreads` fans this out across every active and
          // saved thread on boot and on every wake. The user is still told, by
          // the two surfaces that can act on it: `eventsLoadFailed` below paints
          // the focused thread's own failed empty state (which already knows
          // whether the connection dot is red), and the resume sync retries every
          // thread carrying the flag. A verdict is different: the engine answered
          // and refused, so it reaches the user through the single keyed card.
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
 *  Retries once on any `isTransientFetchError` rejection: a cancelled fetch, the
 *  client deadline, or a transport failure. The runResumeSync /
 *  resyncLoadedThreads paths call this for the FOCUSED thread on SSE reconnect,
 *  iOS PWA wake, or right after an engine restart (the other loaded threads are
 *  marked stale and reach this on focus instead), and iOS Safari both cancels
 *  in-flight fetches mid-flight (suspend/resume, lifecycle, network change) and
 *  fails the first request on a stale HTTP/2 connection. Both succeed on retry.
 *  Mirrors the retry pattern in refreshChangesState.
 *
 *  A failure that survives the retry is reported per the split in the catch
 *  below: silence for anything transient, one keyed card on a verdict.
 *
 *  Resolves to whether a snapshot actually LANDED. It never rejects (every path
 *  is caught, so a `.catch` on the call is dead code), and callers that use a
 *  refresh as read-after-write proof need to tell "the engine answered" from
 *  "we declined or gave up" before acting on it. `schedulePendingCleanup` is the
 *  one with teeth: it force-drops a pending message on the strength of this
 *  answer, so reading a give-up as a success loses a message the user just
 *  sent. */
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
   *  thread a newer attempt just refreshed cleanly. The high-water mark is what
   *  actually expresses the rule; it is self-correcting either way, since a
   *  newer attempt that reports later overwrites whatever this one concluded. */
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
    // background. It runs on a wake / SSE reconnect / engine restart, when the
    // user opens a thread a sync point marked stale, and behind a few
    // read-after-write heals. None of them is the user's click ITSELF: the two
    // that follow one (`handleCancelExchange`, `removeQueuedMessage`) each toast
    // their own failure on the very next line, the empty-focused-thread recovery
    // in `checkConnection` follows no click at all (the 5s health poll drives
    // it), and the focus case has already given the user what they asked for,
    // since the transcript paints from the map before this request is even
    // issued and it only tops up what arrived since. So the click is never the
    // line that fails silently here. Every rejection that carries no answer is
    // suppressed: WebKit aborts the in-flight fetch on the iOS suspend/resume
    // boundary (AbortError), the freshly-resumed page's stale HTTP/2 connection
    // fails at the transport layer (TypeError "Load failed"), and a dropped
    // tunnel drops packets rather than refusing, so the request hangs until the
    // 10s client deadline fires (TimeoutError).
    //
    // That last one used to toast, on the reasoning that waiting the full window
    // and getting nothing is a stronger signal than a cancel. It is not, on this
    // path: over a dropped tunnel the hang IS the outage's shape, so the deadline
    // firing is what an outage LOOKS like here rather than evidence about the
    // engine, which answers in single-digit milliseconds throughout. A sustained
    // outage is the debounced connection dot's to report, once, and the same
    // conclusion was reached for `loadUnreadNotifications` on the same grounds.
    // (The original wording rested on the fan-out multiplying one outage into a
    // deadline per loaded thread. That fan-out is gone, and the conclusion is
    // unchanged without it.) Self-recovery: a refresh that did not land leaves
    // the thread's stale mark set, so simply re-opening it retries; failing that,
    // the next SSE event (constant on any active thread) re-syncs via
    // handleThreadEvent, and the next sync point marks and refreshes again.
    //
    // A verdict (an ApiError, a parse error) means the engine answered and
    // refused, so none of the above applies and it reaches the user, through one
    // keyed card however many threads are failing.
    if (isTransientFetchError(err)) {
      console.warn(`[ThreadLoading] refresh failed transiently for ${threadId} (iOS PWA wake / engine restart); SSE will recover`, err);
      return false;
    }
    console.warn(`[ThreadLoading] Failed to refresh events for ${threadId}:`, err);
    // A newer attempt may have already refreshed this thread cleanly; raising a
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
