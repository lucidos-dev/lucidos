import { effect } from '@preact/signals';
import { focusedThreadId, wipPreviewThreadId, threadMap, currentApp, threadsLoaded } from '../store';

// WIP app preview auto-revert. The toggle button (in PromptInput) sets
// `wipPreviewThreadId` to the thread's id. The button itself handles the
// "click again to revert" case. This module covers two other revert
// triggers from the design:
//
// 1. User navigates away from the thread — the WIP is conceptually tied to
//    the open thread's worktree, so showing it while the user is on an
//    unrelated thread is confusing.
// 2. User opens a different app in the panel-overlay — the WIP URL would
//    become `/app/<other-app>/?thread_id=<wip-thread>`, which the engine
//    rejects (the thread's recorded app doesn't match the requested app).
//    Drop the WIP before the iframe attempts the doomed request.
//
// Apply / Discard / Archive cleanup happens through `clearWipForThread()`,
// invoked by the thread-sync handlers for ChangeApplied / ChangeDiscarded /
// ThreadArchived, and by `refreshAppUI` when the engine emits
// AppUiRefreshRequested for the WIP'd app (which is exactly when Apply lands
// on an iframe-bundled file). Keeping the cleanup explicit avoids inferring
// "worktree gone" from less-reliable proxies (a diff transient flip, a
// status race) that historically caused the WIP toggle to immediately
// undo itself on click before any diff existed.

effect(() => {
  const tid = wipPreviewThreadId.value;
  if (!tid) return;
  // Reading the signals here is what makes the effect re-fire on change.
  const focused = focusedThreadId.value;
  const map = threadMap.value;
  const loaded = threadsLoaded.value;
  if (focused !== tid) {
    wipPreviewThreadId.value = null;
    return;
  }
  const t = map.get(tid);
  if (!t) {
    // During reload the nav stack restores WIP synchronously, but threadMap is
    // populated asynchronously by the SSE backfill. Treat an empty map as
    // "unknown" rather than "thread is gone" until threadsLoaded flips true —
    // otherwise the effect would clear restored WIP before the user sees it.
    if (loaded) wipPreviewThreadId.value = null;
    return;
  }
  // currentApp must match the WIP thread's target app — otherwise the WIP
  // URL would route the wrong app through the WIP-preview branch and 404.
  const folder = t.meta.codingAgentFolder;
  const wipAppId = folder ? folder.split('/').filter(Boolean).pop() : undefined;
  const openApp = currentApp.value;
  if (openApp && wipAppId && openApp.id !== wipAppId) {
    wipPreviewThreadId.value = null;
  }
});

/** Imperative API for the SSE handlers — clears WIP when a terminal change
 *  event fires for the thread currently being previewed. Cheaper than
 *  threading another signal through the effect and avoids re-firing on
 *  every projection update; thread-sync calls it directly on
 *  ChangeApplied / ChangeDiscarded / ThreadArchived. Also called by
 *  refreshAppUI when AppUiRefreshRequested fires for the WIP'd app. */
export function clearWipIfMatches(predicate: (currentWipThreadId: string) => boolean): void {
  const tid = wipPreviewThreadId.value;
  if (tid && predicate(tid)) {
    wipPreviewThreadId.value = null;
  }
}
