import { showToast, showConfirm, threadMap, archivingThreadIds, applyingNowThreadIds, discardingCCThreadIds, getReviewThreads, revealOnFocus, resetCCPendingPreferences, setFocusedThread } from '../store';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import { byReviewOrder } from '../thread-events';
import { saveThread, unsaveThread, archiveThread } from '../../api/threads';
import { loadThreadEvents, ensureThreadByIdInMap } from './thread-loading';
import { scrollToBottom } from '../../components/chat/scrollState';
import { pushThreadNavState } from './thread-navigation';
import { hasSavedScroll, threadScrollKey } from '../../hooks/useScrollMemory';
import { errorDetail } from '../../utils/errorDetail';

// Minimum time the in-flight Archive feedback is visible — long enough to register
const ARCHIVE_MIN_MS = 250;

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

export interface FocusThreadOptions {
  /** Skip mobile pane navigation. Used by history chevrons in the threads
   *  list header so the user can preview prior threads without leaving the
   *  list view. */
  skipPaneNav?: boolean;
}

export function focusThread(threadId: string, options?: FocusThreadOptions): void {
  setFocusedThread(threadId);
  resetCCPendingPreferences();
  // Scroll to bottom and suppress ResizeObserver so content rendering
  // doesn't set scrolledUp=true before useAutoScroll can scroll down.
  // Skip when the target has a saved scroll — the 500ms pinning loop
  // would override useScrollMemory's restore.
  if (!hasSavedScroll(threadScrollKey(threadId))) scrollToBottom();
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

  // No auto-read — user must explicitly click Archive, Apply, or Discard.
}

/** Focus a thread by id, fetching its metadata first if it's not already in
 *  the loaded list (e.g. an old archived thread beyond the History per-source
 *  window, or a thread reached via cross-workspace deep link). */
export function focusThreadOrBootstrap(threadId: string): void {
  if (threadMap.value.has(threadId)) {
    focusThread(threadId);
    return;
  }
  ensureThreadByIdInMap(threadId).then(found => {
    if (found) focusThread(threadId);
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
// Archive (move waiting thread to history)
// ---------------------------------------------------------------------------

/** Find the next review thread below `excludeId` in the drawer sort order. */
function findNextReviewThread(excludeId: string): string | null {
  const threads = getReviewThreads();
  threads.sort(byReviewOrder);
  const idx = threads.findIndex(t => t.meta.id === excludeId);
  // Pick the thread immediately below; if at bottom, pick the one above
  if (idx >= 0 && idx + 1 < threads.length) return threads[idx + 1].meta.id;
  if (idx > 0) return threads[idx - 1].meta.id;
  return null;
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

  // Pre-compute the next review thread for post-archive navigation.
  const nextId = findNextReviewThread(threadId);

  archivingThreadIds.value = new Set([...archivingThreadIds.value, threadId]);
  try {
    // Run API call alongside a minimum delay so "Archive..." is visible
    await Promise.all([
      archiveThread(threadId),
      new Promise(r => setTimeout(r, ARCHIVE_MIN_MS)),
    ]);

    if (nextId) {
      revealOnFocus.value = true;
      focusThread(nextId);
    } else {
      unfocusThread();
      navigateToPane('thread');
    }
  } catch (e) {
    showToast(`Failed to archive thread: ${errorDetail(e)}`, 'error');
  } finally {
    const next = new Set(archivingThreadIds.value);
    next.delete(threadId);
    archivingThreadIds.value = next;
  }
}
