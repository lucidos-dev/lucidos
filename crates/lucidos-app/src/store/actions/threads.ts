import { showToast, showConfirm, threadMap, archivingThreadIds, applyingNowThreadIds, discardingCCThreadIds, revealOnFocus, resetCodingAgentPendingPreferences, setFocusedThread, focusedThreadId, focusedPane, threadChannelFilter, selectedTriggerIds, selectedRepoIds, selectedAppIds, drawerView, threadSearchQuery, threadSearchResults } from '../store';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import type { ThreadSection, ThreadState } from '../thread-events';
import { threadPassesChannelFilter } from '../threadFilter';
import { computeFamilyGraph, filterByTopThread, orderedCurrentForReview, attentionThreads, reviewThreads, runningThreads, draftThreads } from '../../components/drawer/family-graph';
import type { FamilyGraph } from '../../components/drawer/family-graph';
import { saveThread, unsaveThread, archiveThread } from '../../api/threads';
import { ApiError } from '../../api/client';
import { loadThreadEvents, ensureThreadByIdInMap, sectionMutatedAt } from './thread-loading';
import { scrollToBottom, scrollToEventAndPulse, scrollToChangeAndPulse, clearPendingEventScroll } from '../../components/chat/scrollState';
import { pushThreadNavState } from './thread-navigation';
import { hasSavedScroll, threadScrollKey } from '../../hooks/useScrollMemory';
import { errorDetail } from '../../utils/errorDetail';

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

export interface FocusThreadOptions {
  /** When set, after the thread loads, scroll the matching event card into
   *  view and briefly pulse it. Used by notification deep-links so a push
   *  for a `UserQuestionAsked` lands on that exact question, not the bottom
   *  of the thread or the user's last saved scroll. Overrides the default
   *  scroll-to-bottom / restore-saved-scroll behavior. */
  targetEventId?: string | null;
  /** When set, after the thread loads, scroll to the turn that produced this
   *  change (stamped with `data-change-id` by `ChatExchange`) and pulse it.
   *  Used by the Changes panel so a row lands on its own diff event rather than
   *  the bottom of the thread — the change isn't necessarily the last turn.
   *  Same scroll/suppression contract as `targetEventId`; ignored when
   *  `targetEventId` is also set. */
  targetChangeId?: string | null;
}

export function focusThread(threadId: string, options?: FocusThreadOptions): void {
  setFocusedThread(threadId);
  resetCodingAgentPendingPreferences();
  // Scroll to bottom and suppress ResizeObserver so content rendering
  // doesn't set scrolledUp=true before useAutoScroll can scroll down.
  // Skip when the target has a saved scroll — the 500ms pinning loop
  // would override useScrollMemory's restore.
  // Skip too when a deep-link target (event or change) is provided — the
  // matching scrollTo*AndPulse owns the scroll target in that case.
  const targetEventId = options?.targetEventId ?? null;
  // targetEventId wins when both are set (notification deep-link is more
  // specific than a Changes-row landing on the originating turn).
  const targetChangeId = targetEventId ? null : (options?.targetChangeId ?? null);
  const hasTarget = !!targetEventId || !!targetChangeId;
  // A plain focus (no deep-link target) cancels any in-flight deep-link scroll
  // claim from a prior focus, so its suppression can't leak onto this thread's
  // load. A deep-link focus re-claims below via scrollTo*AndPulse.
  if (!hasTarget) clearPendingEventScroll();
  if (!hasTarget && !hasSavedScroll(threadScrollKey(threadId))) scrollToBottom();
  // notAtTop is NOT reset here — syncNotAtTop() in the scroll listener owns
  // it exclusively. Manual resets cause the chevron to vanish when no scroll
  // event fires (e.g. re-focusing the same thread where scrollTop is unchanged).

  // Lazy-load events for this thread if not already loaded
  loadThreadEvents(threadId);

  pushThreadNavState({ type: 'thread', id: threadId });

  // Surface the focused thread on the pane the user is actually working in.
  // Mobile: navigate to the thread pane so the thread is visible — without this,
  // callers like toast onClick and search would set the focused thread but leave
  // the user on whichever pane they were on. Desktop: when arriving from the
  // Content pane group (e.g. Search Everywhere → a thread while viewing an
  // app/Settings), re-activate the Threads pane group so keyboard tabbing lands
  // on the conversation. Signal-only — handlePaneTab pulls DOM focus in on the
  // first Tab. Only the cross-group case switches: an existing 'drawer'/'thread'
  // focus is left alone so drawer ↑/↓ browsing (and its accent) isn't disturbed.
  if (isMobile()) {
    navigateToPane('thread');
  } else if (focusedPane.value === 'content') {
    focusedPane.value = 'thread';
  }

  if (targetEventId) {
    scrollToEventAndPulse(targetEventId);
  } else if (targetChangeId) {
    scrollToChangeAndPulse(targetChangeId);
  }

  // No auto-read — user must explicitly click Archive, Apply, or Discard.
}

/** Focus a thread by id, fetching its metadata first if it's not already in
 *  the loaded list (e.g. an old archived thread beyond the Archive per-source
 *  window, or a thread reached via cross-workspace deep link). */
export function focusThreadOrBootstrap(threadId: string, options?: FocusThreadOptions): void {
  if (threadMap.value.has(threadId)) {
    focusThread(threadId, options);
    return;
  }
  ensureThreadByIdInMap(threadId).then(found => {
    if (found) focusThread(threadId, options);
    else showToast('Thread not found', 'error');
  }).catch(err => {
    showToast(`Failed to open thread: ${errorDetail(err)}`, 'error');
  });
}

export function unfocusThread(): void {
  setFocusedThread(null);
  revealOnFocus.value = false;
  resetCodingAgentPendingPreferences();
}

// ---------------------------------------------------------------------------
// Save / Unsave
// ---------------------------------------------------------------------------
// Save is offered on Review/Archive sections at idle. Unsave is offered on
// the Saved section mid-turn — the only way to drop a running thread out of
// Saved without canceling it. Confirm before unsave so a stray click doesn't
// cost the parking spot.

/** Translate a failed `archiveThread` call into a user-facing toast string.
 *  The engine returns a structured 409 body (`reason`, `parent_status`,
 *  `blocking`) for the cascade-gate rejections — without the formatter the
 *  toast falls back to `"409"` (empty `statusText`, no `body.error`), which
 *  tells the user nothing actionable. */
function formatArchiveErrorToast(err: unknown): string {
  if (err instanceof ApiError && err.httpCode === 409 && err.body && typeof err.body === 'object') {
    const body = err.body as Record<string, unknown>;
    if (body.reason === 'descendants_blocking') {
      const blocking = Array.isArray(body.blocking) ? body.blocking : [];
      const n = blocking.length;
      if (n === 1) return "Can't archive yet — a sub-thread is still busy";
      if (n > 1) return `Can't archive yet — ${n} sub-threads are still busy`;
      return "Can't archive yet — a sub-thread is still busy";
    }
    if (body.reason === 'parent_not_archivable') {
      // parent_status === 'running' is the live-work case; any other status
      // means the parent's archive_state was already 'archived' (the OR in
      // `classify_archive_decision` rejected on the second clause).
      const status = body.parent_status;
      if (status === 'running') return "Can't archive yet — this thread is still running";
      return 'This thread is already archived';
    }
    if (body.reason === 'parent_has_pending_changes') {
      return "Can't archive — apply or discard the pending change first";
    }
  }
  return `Failed to archive thread: ${errorDetail(err)}`;
}

function updateThreadMeta(threadId: string, patch: Partial<{ saved: boolean }>): void {
  const map = new Map(threadMap.value);
  const thread = map.get(threadId);
  if (thread) {
    map.set(threadId, { ...thread, meta: { ...thread.meta, ...patch } });
    threadMap.value = map;
  }
}

export async function handleSaveThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.saved) return;

  updateThreadMeta(threadId, { saved: true });
  try {
    await saveThread(threadId);
  } catch (e) {
    updateThreadMeta(threadId, { saved: false });
    showToast(`Failed to pin thread: ${errorDetail(e)}`, 'error');
  }
}

export async function handleUnsaveThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || !thread.meta.saved) return;

  if (!await showConfirm('Remove this thread from the Pinned section?', 'Remove')) {
    return;
  }

  updateThreadMeta(threadId, { saved: false });
  try {
    await unsaveThread(threadId);
  } catch (e) {
    updateThreadMeta(threadId, { saved: true });
    showToast(`Failed to unpin thread: ${errorDetail(e)}`, 'error');
  }
}

// ---------------------------------------------------------------------------
// Archive (move waiting thread to archive)
// ---------------------------------------------------------------------------

/** Threads the user can actually see, given the active channel/trigger/repo/app
 *  filter — the SAME family-scoped predicate the drawer uses (`ThreadDrawer`'s
 *  ThreadList). The post-archive focus must land on a thread the user can click
 *  on in the list, so a thread hidden by the active filter is never offered as
 *  the next focus. Returns the filtered set plus the family graph (built over
 *  the full thread map so parent walks resolve) for the caller to order. */
function visibleThreadsAndGraph(): { visible: ThreadState[]; graph: FamilyGraph } {
  const all = Array.from(threadMap.value.values());
  const graph = computeFamilyGraph(all);
  const filter = threadChannelFilter.value;
  const triggerSelection = selectedTriggerIds.value;
  const repoSelection = selectedRepoIds.value;
  const appSelection = selectedAppIds.value;
  // A trigger/repo/app sub-selection flips the gate to any-member matching,
  // mirroring the drawer so a coding-agent thread in the selected repo/app surfaces with
  // its family even when the root's channel is filtered out.
  const subSelectionActive =
    triggerSelection.size > 0 || repoSelection.size > 0 || appSelection.size > 0;
  const visible = filterByTopThread(all, graph,
    t => threadPassesChannelFilter(t, filter, triggerSelection, repoSelection, appSelection),
    subSelectionActive,
  );
  return { visible, graph };
}

/** The visible thread ids of the drawer view the user is *currently looking at*,
 *  in the same order the drawer renders them. The post-archive focus walks this
 *  so "next" is the next visible row in whatever view is active — the next
 *  Needs-attention row when the attention view is open, the next Review/Running
 *  row, the next filtered Current row in the default view — instead of always
 *  jumping into Current. Mirrors `ThreadDrawer`'s `activeView` resolution: a live
 *  search query overrides the selected view; otherwise `drawerView` decides.
 *
 *  The alternate views (attention/review/running/drafts) deliberately bypass the
 *  channel/trigger/repo/app filter — exactly as the drawer renders them — so the
 *  next focus matches what's on screen. Only the default `all` view is
 *  filter-aware, walking the visible Current section (`orderedCurrentForReview`)
 *  the same way the drawer does. */
function orderedVisibleThreadIds(): string[] {
  // Search overrides the selected view (mirrors ThreadDrawer's activeView).
  if (threadSearchQuery.value.trim().length > 0) {
    const results = threadSearchResults.value;
    return results.status === 'loaded' ? results.data.map(r => r.thread_id) : [];
  }
  const map = threadMap.value;
  switch (drawerView.value) {
    case 'attention': return attentionThreads(map).map(t => t.meta.id);
    case 'review':    return reviewThreads(map).map(t => t.meta.id);
    case 'running':   return runningThreads(map).map(t => t.meta.id);
    case 'drafts':    return draftThreads(map).map(t => t.meta.id);
    case 'all':
    default: {
      const { visible, graph } = visibleThreadsAndGraph();
      return orderedCurrentForReview(visible, graph).map(t => t.meta.id);
    }
  }
}

/** Ordered list of visible thread ids to consider as the next focus when the
 *  user archives `aroundId` — closest below first, then closest above — within
 *  the currently active drawer view (`orderedVisibleThreadIds`). Snapshotted
 *  BEFORE the optimistic flip so the position anchor survives the cascade
 *  dropping `aroundId` (and its descendants) out of the view. */
function visibleCandidatesAround(aroundId: string): string[] {
  const ordered = orderedVisibleThreadIds();
  const idx = ordered.indexOf(aroundId);
  if (idx < 0) return [];
  const result: string[] = [];
  for (let i = idx + 1; i < ordered.length; i++) result.push(ordered[i]);
  for (let i = idx - 1; i >= 0; i--) result.push(ordered[i]);
  return result;
}

/** Walk parentThreadId from every thread in the map to collect the target +
 *  every transitive descendant. Mirrors the backend cascade scope so the
 *  optimistic flip drops the whole family out of review in one stroke. */
function collectArchiveCascade(rootId: string): Set<string> {
  const childrenByParent = new Map<string, string[]>();
  for (const t of threadMap.value.values()) {
    const p = t.meta.parentThreadId;
    if (!p) continue;
    const bucket = childrenByParent.get(p);
    if (bucket) bucket.push(t.meta.id); else childrenByParent.set(p, [t.meta.id]);
  }
  const seen = new Set<string>();
  const stack: string[] = [rootId];
  while (stack.length > 0) {
    const id = stack.pop()!;
    if (seen.has(id)) continue;
    seen.add(id);
    const kids = childrenByParent.get(id);
    if (kids) stack.push(...kids);
  }
  return seen;
}

export async function handleArchiveThread(threadId: string): Promise<void> {
  if (archivingThreadIds.value.has(threadId)) return;
  if (discardingCCThreadIds.value.has(threadId)) return; // Can't archive while discarding

  // Archive is the only exit from Saved — confirm before dropping the row out
  // of its parking spot. The ThreadArchived projection clears is_saved.
  const thread = threadMap.value.get(threadId);
  if (thread?.meta.saved) {
    if (!await showConfirm(
      'Are you sure you want to move this thread to the archive?',
      'Archive',
    )) {
      return;
    }
  }

  // Pin to bottom and show header before banner re-renders
  scrollToBottom();

  // Clear stale apply state — applying and archiving are mutually exclusive.
  // If the user is archiving, any in-progress or stale apply is abandoned.
  if (applyingNowThreadIds.value.has(threadId)) {
    const next = new Map(applyingNowThreadIds.value);
    next.delete(threadId);
    applyingNowThreadIds.value = next;
  }

  // Snapshot the position anchor BEFORE the optimistic flip — once the
  // cascade leaves the active view, visibleCandidatesAround() can't compute it.
  const candidates = visibleCandidatesAround(threadId);

  // Snapshot section + codingAgentProposed on every family member so we can
  // roll back if the API rejects (409 blocking, 500 mid-cascade). Both fields
  // are required to leave Current: `displaySection` keeps any thread with
  // pending changes in Current regardless of `section`.
  const cascade = collectArchiveCascade(threadId);
  type Snap = { section: ThreadSection; codingAgentProposed: boolean };
  const snapshot = new Map<string, Snap>();
  const optimistic = new Map(threadMap.value);
  // Stamp BEFORE the flip so any in-flight GET issued before this moment is
  // considered stale wrt section/codingAgentProposed. See `sectionMutatedAt`
  // in thread-loading.ts for the iOS-PWA-resume race this prevents.
  const flippedAt = Date.now();
  for (const tid of cascade) {
    const t = optimistic.get(tid);
    if (!t) continue;
    sectionMutatedAt.set(tid, flippedAt);
    snapshot.set(tid, {
      section: t.meta.section,
      codingAgentProposed: t.meta.codingAgentProposed,
    });
    optimistic.set(tid, {
      ...t,
      meta: { ...t.meta, section: 'archived', codingAgentProposed: false },
    });
  }
  threadMap.value = optimistic;

  // Every cascade member gets the in-flight flag, not just the root: the
  // backend's stop_agent emits CodingAgentIdled for each descendant with the
  // PRE-archive aggregate (section='inbox'), so the SSE archive-race guard
  // in thread-sync.ts needs to recognise descendants as in-flight too.
  archivingThreadIds.value = new Set([...archivingThreadIds.value, ...cascade]);

  // The SSE `ThreadArchived` cascade arriving later just confirms what we
  // already did.
  const nextId = candidates.find(id => !cascade.has(id)) ?? null;
  if (nextId) {
    revealOnFocus.value = true;
    focusThread(nextId);
  } else {
    unfocusThread();
    navigateToPane('thread');
  }

  try {
    await archiveThread(threadId);
  } catch (e) {
    const restored = new Map(threadMap.value);
    for (const [tid, snap] of snapshot) {
      const t = restored.get(tid);
      if (!t) continue;
      restored.set(tid, { ...t, meta: { ...t.meta, ...snap } });
    }
    threadMap.value = restored;
    // Re-focus the rejected thread only if the user hasn't actively navigated
    // away during the in-flight API call — a user who picked a different
    // thread made a deliberate choice we shouldn't yank them out of.
    const stillOnAutoFocus = focusedThreadId.value === nextId;
    if (stillOnAutoFocus && restored.has(threadId)) focusThread(threadId);
    showToast(formatArchiveErrorToast(e), 'error');
  } finally {
    const next = new Set(archivingThreadIds.value);
    for (const tid of cascade) next.delete(tid);
    archivingThreadIds.value = next;
  }
}
