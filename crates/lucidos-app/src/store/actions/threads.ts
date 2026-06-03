import { showToast, showConfirm, threadMap, archivingThreadIds, applyingNowThreadIds, discardingCCThreadIds, getReviewThreads, revealOnFocus, resetCCPendingPreferences, setFocusedThread, focusedThreadId } from '../store';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import { byReviewOrder } from '../thread-events';
import type { ThreadSection } from '../thread-events';
import { saveThread, unsaveThread, archiveThread } from '../../api/threads';
import { ApiError } from '../../api/client';
import { loadThreadEvents, ensureThreadByIdInMap, sectionMutatedAt } from './thread-loading';
import { scrollToBottom, scrollToEventAndPulse } from '../../components/chat/scrollState';
import { pushThreadNavState } from './thread-navigation';
import { hasSavedScroll, threadScrollKey } from '../../hooks/useScrollMemory';
import { errorDetail } from '../../utils/errorDetail';

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

export interface FocusThreadOptions {
  /** Skip mobile pane navigation. Used by history chevrons in the threads
   *  list header so the user can preview prior threads without leaving the
   *  list view. */
  skipPaneNav?: boolean;
  /** When set, after the thread loads, scroll the matching event card into
   *  view and briefly pulse it. Used by notification deep-links so a push
   *  for a `UserQuestionAsked` lands on that exact question, not the bottom
   *  of the thread or the user's last saved scroll. Overrides the default
   *  scroll-to-bottom / restore-saved-scroll behavior. */
  targetEventId?: string | null;
}

export function focusThread(threadId: string, options?: FocusThreadOptions): void {
  setFocusedThread(threadId);
  resetCCPendingPreferences();
  // Scroll to bottom and suppress ResizeObserver so content rendering
  // doesn't set scrolledUp=true before useAutoScroll can scroll down.
  // Skip when the target has a saved scroll — the 500ms pinning loop
  // would override useScrollMemory's restore.
  // Skip too when a target event id is provided — scrollToEventAndPulse
  // owns the scroll target in that case (notification deep-link).
  const targetEventId = options?.targetEventId ?? null;
  if (!targetEventId && !hasSavedScroll(threadScrollKey(threadId))) scrollToBottom();
  // notAtTop is NOT reset here — syncNotAtTop() in the scroll listener owns
  // it exclusively. Manual resets cause the chevron to vanish when no scroll
  // event fires (e.g. re-focusing the same thread where scrollTop is unchanged).

  // Lazy-load events for this thread if not already loaded
  loadThreadEvents(threadId);

  pushThreadNavState({ type: 'thread', id: threadId });

  // On mobile, navigate to the thread pane so the focused thread is visible.
  // Without this, callers like toast onClick and search would set the focused
  // thread but leave the user on whichever pane they were on.
  if (isMobile() && !options?.skipPaneNav) {
    navigateToPane('thread');
  }

  if (targetEventId) {
    scrollToEventAndPulse(targetEventId);
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
  resetCCPendingPreferences();
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
    showToast(`Failed to save thread: ${errorDetail(e)}`, 'error');
  }
}

export async function handleUnsaveThread(threadId: string): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || !thread.meta.saved) return;

  if (!await showConfirm('Remove this thread from the Saved section?', 'Remove')) {
    return;
  }

  updateThreadMeta(threadId, { saved: false });
  try {
    await unsaveThread(threadId);
  } catch (e) {
    updateThreadMeta(threadId, { saved: true });
    showToast(`Failed to unsave thread: ${errorDetail(e)}`, 'error');
  }
}

// ---------------------------------------------------------------------------
// Archive (move waiting thread to archive)
// ---------------------------------------------------------------------------

/** Ordered list of review-thread ids to consider as the next focus when the
 *  user archives `aroundId` — closest below first, then closest above.
 *  Snapshotted BEFORE the optimistic flip so the position anchor survives the
 *  cascade dropping `aroundId` (and its descendants) out of review. */
function reviewCandidatesAround(aroundId: string): string[] {
  const threads = getReviewThreads();
  threads.sort(byReviewOrder);
  const idx = threads.findIndex(t => t.meta.id === aroundId);
  if (idx < 0) return [];
  const result: string[] = [];
  for (let i = idx + 1; i < threads.length; i++) result.push(threads[i].meta.id);
  for (let i = idx - 1; i >= 0; i--) result.push(threads[i].meta.id);
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
  // cascade leaves review, getReviewThreads() can't compute it.
  const candidates = reviewCandidatesAround(threadId);

  // Snapshot section + codingAgentProposed on every family member so we can
  // roll back if the API rejects (409 blocking, 500 mid-cascade). Both fields
  // are required to leave review: `displaySection` keeps any thread with
  // pending changes in review regardless of `section`.
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
