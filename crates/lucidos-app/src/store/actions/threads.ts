import { focusedThreadId, focusedDraftId, showToast, threadMap, dismissingThreadIds, applyingNowThreadIds, discardingCCThreadIds, getReviewThreads, revealOnFocus, resetCCPendingPreferences } from '../store';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import { byReviewOrder } from '../thread-events';
import { pinThread, unpinThread, dismissThread } from '../../api/threads';
import { loadThreadEvents } from './thread-loading';
import { scrollToBottom } from '../../components/chat/scrollState';
import { pushThreadNavState } from './thread-navigation';
import { FOCUSED_THREAD_KEY } from '../../utils/draftStorage';
import { errorDetail } from '../../utils/errorDetail';

// Minimum time "Done..." is visible — long enough to register as feedback
const DISMISS_MIN_MS = 250;

// ---------------------------------------------------------------------------
// Thread CRUD
// ---------------------------------------------------------------------------

export function focusThread(threadId: string): void {
  focusedThreadId.value = threadId;
  localStorage.setItem(FOCUSED_THREAD_KEY, threadId);
  resetCCPendingPreferences();
  // Scroll to bottom and suppress ResizeObserver so content rendering
  // doesn't set scrolledUp=true before useAutoScroll can scroll down
  scrollToBottom();
  // notAtTop is NOT reset here — syncNotAtTop() in the scroll listener owns
  // it exclusively. Manual resets cause the chevron to vanish when no scroll
  // event fires (e.g. re-focusing the same thread where scrollTop is unchanged).

  // Lazy-load events for this thread if not already loaded
  loadThreadEvents(threadId);

  pushThreadNavState({ type: 'thread', id: threadId });

  // On mobile, navigate to the thread pane so the focused thread is visible.
  // Without this, callers like toast onClick and search would set the focused
  // thread but leave the user on whichever pane they were on.
  if (isMobile()) {
    navigateToPane('thread');
  }

  // No auto-read — user must explicitly click Done, Apply, or Discard.
}

export function unfocusThread(): void {
  focusedThreadId.value = null;
  revealOnFocus.value = false;
  resetCCPendingPreferences();
  localStorage.removeItem(FOCUSED_THREAD_KEY);
  pushThreadNavState({ type: 'draft', id: focusedDraftId.value });
}

// ---------------------------------------------------------------------------
// Pin/unpin
// ---------------------------------------------------------------------------

function updateThreadMeta(threadId: string, patch: Partial<{ pinned: boolean }>): void {
  const map = new Map(threadMap.value);
  const thread = map.get(threadId);
  if (thread) {
    map.set(threadId, { ...thread, meta: { ...thread.meta, ...patch } });
    threadMap.value = map;
  }
}

async function togglePin(threadId: string, pinned: boolean): Promise<void> {
  const thread = threadMap.value.get(threadId);
  if (!thread || thread.meta.pinned === pinned) return;

  updateThreadMeta(threadId, { pinned });
  try {
    await (pinned ? pinThread : unpinThread)(threadId);
  } catch (e) {
    updateThreadMeta(threadId, { pinned: !pinned });
    showToast(`Failed to ${pinned ? 'pin' : 'unpin'} thread: ${errorDetail(e)}`, 'error');
  }
}

export function handlePinThread(threadId: string): Promise<void> {
  return togglePin(threadId, true);
}

export function handleUnpinThread(threadId: string): Promise<void> {
  return togglePin(threadId, false);
}

// ---------------------------------------------------------------------------
// Dismiss (move waiting thread to history)
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

export async function handleDismissThread(threadId: string): Promise<void> {
  if (dismissingThreadIds.value.has(threadId)) return;
  if (discardingCCThreadIds.value.has(threadId)) return; // Can't dismiss while discarding
  // Pin to bottom and show header before banner re-renders
  scrollToBottom();

  // Clear stale apply state — applying and dismissing are mutually exclusive.
  // If the user is dismissing, any in-progress or stale apply is abandoned.
  if (applyingNowThreadIds.value.has(threadId)) {
    const next = new Map(applyingNowThreadIds.value);
    next.delete(threadId);
    applyingNowThreadIds.value = next;
  }

  // Pre-compute the next review thread for post-dismiss navigation.
  const nextId = findNextReviewThread(threadId);

  dismissingThreadIds.value = new Set([...dismissingThreadIds.value, threadId]);
  try {
    // Run API call alongside a minimum delay so "Done..." is visible
    await Promise.all([
      dismissThread(threadId),
      new Promise(r => setTimeout(r, DISMISS_MIN_MS)),
    ]);

    if (nextId) {
      revealOnFocus.value = true;
      focusThread(nextId);
    } else {
      unfocusThread();
      navigateToPane('thread');
    }
  } catch (e) {
    showToast(`Failed to dismiss thread: ${errorDetail(e)}`, 'error');
  } finally {
    const next = new Set(dismissingThreadIds.value);
    next.delete(threadId);
    dismissingThreadIds.value = next;
  }
}
