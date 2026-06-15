import { effect, untracked } from '@preact/signals';
import { pageTitle, animationSpeed, stepsExpanded, detailsExpanded, expandedFolders, threadDrawerOpen, selectedScope, notificationsFilter, collapsedExchanges, collapsedInitiators, filePreviewSource, filePreviewEditing, previewFile, repoSelectedChangeId, inputMode, showToast, dismissToast, applyAllInProgress, SELECTED_CHANGE_KEY, MOBILE_VIEW_KEY } from './store';
import { clientRefreshing } from '../hooks/sw-update';
import { cancelApplyAllBatch } from './actions/chat-changes';

// Sync page title with unread count
effect(() => {
  document.title = pageTitle.value;
});

// Clean up stale localStorage keys — model/effort are now per-thread, not persisted
localStorage.removeItem('lucidos-model');
localStorage.removeItem('lucidos-reasoning-effort');
// The unread count is now derived from the loaded unread set (store.ts
// `unreadCount` computed), not a cached number — drop the legacy persisted key
// so a stale value can't linger in storage.
localStorage.removeItem('lucidos-unread-count');
// mobile-view moved from localStorage to sessionStorage so a cold PWA launch
// doesn't strand the user on a pane they last viewed days ago.
localStorage.removeItem(MOBILE_VIEW_KEY);
// Legacy key from when the toggle used a different shape — drop so it can't
// shadow the current 'lucidos-input-mode' payload.
localStorage.removeItem('lucidos-input-target');

// Persist the compose actor toggle (Lucidos / Claude). Restored on init in
// store.ts so a Claude pick survives reload.
effect(() => {
  localStorage.setItem('lucidos-input-mode', JSON.stringify(inputMode.value));
});

// Persist animation speed
effect(() => {
  localStorage.setItem('lucidos-animation-speed-slider', String(animationSpeed.value));
});

// Persist steps expanded state
effect(() => {
  localStorage.setItem('lucidos-steps-expanded', String(stepsExpanded.value));
});

// Persist details (more/less) expanded state
effect(() => {
  localStorage.setItem('lucidos-details-expanded', String(detailsExpanded.value));
});

// Persist expanded folders
effect(() => {
  localStorage.setItem('lucidos-expanded-folders', JSON.stringify([...expandedFolders.value]));
});

// Persist thread drawer open state
effect(() => {
  localStorage.setItem('lucidos-thread-drawer-open', String(threadDrawerOpen.value));
});

// Persist selected coding-agent scope (Lucidos / external repo / app). Legacy
// `lucidos-cc-last-repo` / `lucidos-cc-last-scope` are migrated once at
// signal-restore time inside store.ts; this effect only ever writes the new key.
effect(() => {
  localStorage.setItem('lucidos-coding-agent-last-scope', JSON.stringify(selectedScope.value));
});

// Persist notifications filter
effect(() => {
  localStorage.setItem('lucidos-notifications-filter', notificationsFilter.value);
});

effect(() => {
  localStorage.setItem('lucidos-collapsed-exchanges', JSON.stringify([...collapsedExchanges.value]));
});

effect(() => {
  localStorage.setItem('lucidos-collapsed-initiators', JSON.stringify([...collapsedInitiators.value]));
});

// Persist source-vs-rendered preview toggle (md/html/csv/svg + diff view)
effect(() => {
  localStorage.setItem('lucidos-file-preview-source', String(filePreviewSource.value));
});

// Drop inline edit mode whenever the previewed file changes (or the preview
// closes). Restore-from-history (navigation.restoreState) sets panelOverlay
// directly without going through openFilePreview, so resetting here — keyed on
// the previewed path — covers every entry point, not just the click path.
let lastPreviewFile: string | null = previewFile.value;
effect(() => {
  const path = previewFile.value;
  if (path !== lastPreviewFile) {
    lastPreviewFile = path;
    filePreviewEditing.value = false;
  }
});

// Persist selected change so the Diff view survives reload — without this,
// reloading on the Changes tab silently drops the selection and the toggle
// snaps back to All Files. Restored at startup via restoreRepoSelectionFromStorage.
effect(() => {
  const id = repoSelectedChangeId.value;
  if (id) {
    localStorage.setItem(SELECTED_CHANGE_KEY, id);
  } else {
    localStorage.removeItem(SELECTED_CHANGE_KEY);
  }
});

// Show a spinner toast the instant a client refresh starts, mirroring the
// "Restarting engine..." banner an engine restart raises. `refreshClient`
// (hooks/sw-update.ts) flips `clientRefreshing` true before its async SW swap +
// reload, and never clears it (the reload tears the page down), so this fires
// once per refresh and the spinner stays until the new page loads. Lives here as
// an effect rather than in `refreshClient` so showing it doesn't pull `showToast`
// into sw-update.ts — the store ↔ sw-update import cycle `clientRefreshing`'s
// home deliberately avoids. dismissable/showDuringRestart match the restart
// toast: it can't be closed mid-reload, and it survives the showToast
// engine-restart suppression in the rare refresh-during-restart overlap.
// `untracked` keeps the effect's only dependency `clientRefreshing` — showToast
// reads AND writes the `toasts` signal, so tracking it here would make the
// effect re-trigger itself (a signals "Cycle detected").
effect(() => {
  if (!clientRefreshing.value) return;
  untracked(() => showToast('Refreshing...', 'info', { key: 'refreshing', spinning: true, dismissable: false, showDuringRestart: true }));
});

// Sticky spinner toast for the lifetime of an Apply All batch. applyAllInProgress
// is the single source of truth (set optimistically on click + by the
// ApplyAllBatchStarted SSE, cleared by ApplyAllBatchCompleted or an HTTP error),
// so driving the toast from it here keeps one show/dismiss pair instead of
// scattering them across the four flip sites. Like the restart/refresh spinners
// it's non-dismissable — the batch (which can sit for minutes hardening a member)
// is genuinely in flight and clears its own toast when it finishes. The
// `shown` transition guard mirrors the `lastPreviewFile` pattern above: it acts
// only on the true↔false edges so the false init pass doesn't churn the `toasts`
// signal with a no-op dismiss. `untracked` keeps the effect's sole dependency
// `applyAllInProgress` — showToast/dismissToast read AND write `toasts`, so
// tracking them here would self-trigger the effect ("Cycle detected").
let applyAllToastShown = false;
effect(() => {
  const active = applyAllInProgress.value;
  if (active && !applyAllToastShown) {
    applyAllToastShown = true;
    // Cancel action stops the whole batch (aborts the in-flight hardening/merge,
    // leaves the rest pending). dismissable:false — the spinner clears itself
    // when ApplyAllBatchCompleted lands; the action is the deliberate way out.
    untracked(() => showToast('Applying changes...', 'info', {
      key: 'apply-all-batch',
      spinning: true,
      dismissable: false,
      action: { label: 'Cancel', onClick: () => void cancelApplyAllBatch(), variant: 'danger' },
    }));
  } else if (!active && applyAllToastShown) {
    applyAllToastShown = false;
    untracked(() => dismissToast('apply-all-batch'));
  }
});
