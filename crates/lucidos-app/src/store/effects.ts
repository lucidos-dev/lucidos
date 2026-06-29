import { effect, untracked } from '@preact/signals';
import { pageTitle, unreadCount, animationSpeed, stepsExpanded, detailsExpanded, expandedFolders, threadDrawerOpen, selectedScope, notificationsFilter, collapsedExchanges, collapsedInitiators, filePreviewSource, diffWholeFile, filePreviewEditing, previewFile, viewingNotification, repoSelectedChangeId, inputMode, showToast, dismissToast, applyAllInProgress, engineRestarting, SELECTED_CHANGE_KEY } from './store';
import { clientRefreshing } from '../hooks/sw-update';
import { cancelApplyAllBatch } from './actions/chat-changes';
import { handleRestartTimeout } from './actions/connection';
import { onNotificationDetailClosed } from './actions/notifications';
import { applyAppBadge } from './actions/app-badge';
import { IS_PICKER } from '../utils/basePath';
import { isTauri } from '../utils/platform';

// Sync page title with unread count
effect(() => {
  document.title = pageTitle.value;
});

// Per-workspace PWA app-icon badge: mirror THIS workspace's unread count onto
// the installed PWA icon (a workspace PWA badges its own workspace). Not on the
// gateway picker — that context sets the aggregate total across running
// workspaces itself — and not under Tauri, where the desktop process drives a
// native dock badge from the gateway total. Live while the app is open; the
// service worker keeps a CLOSED PWA fresh via the push payload's `app_badge`.
if (!IS_PICKER && !isTauri()) {
  effect(() => {
    applyAppBadge(unreadCount.value);
  });
}

// Clean up stale localStorage keys — model/effort are now per-thread, not persisted
localStorage.removeItem('lucidos-model');
localStorage.removeItem('lucidos-reasoning-effort');
// The unread count is now derived from the loaded unread set (store.ts
// `unreadCount` computed), not a cached number — drop the legacy persisted key
// so a stale value can't linger in storage.
localStorage.removeItem('lucidos-unread-count');
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

// Reset transient preview toggles whenever the previewed file changes (or the
// preview closes): inline edit mode AND the diff whole-file *override*. Clearing
// the override to `null` re-derives each new diff's default from file status (see
// `diffWholeFileEffective`: added → whole file, otherwise the hunks) instead of
// dragging the prior file's explicit toggle along. Restore-from-history
// (navigation.restoreState) sets panelOverlay directly without going through
// openFilePreview, so resetting here — keyed on the previewed path — covers every
// entry point, not just the click path. Both signals are non-persisted, so even
// if this effect doesn't fire on the initial hydration tick they start reset.
let lastPreviewFile: string | null = previewFile.value;
effect(() => {
  const path = previewFile.value;
  if (path !== lastPreviewFile) {
    lastPreviewFile = path;
    filePreviewEditing.value = false;
    diffWholeFile.value = null;
  }
});

// Reset the notification view-dedup guard (and refresh the inbox list when it's
// the active panel) whenever the notification detail closes. The overlay is
// cleared by panel Back nav / menu switch / restore — none of which run an
// explicit close action — so keying on the open notification's id here covers
// every close path, mirroring the `lastPreviewFile` reset above. Only the
// →null edge counts as a close; a prev/next walk (X→Y) must not reset the guard
// or reload mid-walk. `untracked` keeps the effect's sole dependency
// `viewingNotification` (onNotificationDetailClosed reads activeMenuItem and
// writes the notifications/toasts signals, which would otherwise be tracked).
let lastViewingNotificationId: string | null = viewingNotification.value?.id ?? null;
effect(() => {
  const id = viewingNotification.value?.id ?? null;
  if (id === lastViewingNotificationId) return;
  const closed = id === null && lastViewingNotificationId !== null;
  lastViewingNotificationId = id;
  if (closed) untracked(() => onNotificationDetailClosed());
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
//
// A refresh always supersedes the "New version available" prompt — that prompt's
// whole job is to start a refresh, which is now in flight — so dismiss it here
// rather than leaving it stacked above the spinner. This covers every refresh
// entry point (the toast's own Refresh button, the control panel, the
// applied-change / reconnect toasts), so the update prompt is replaced by the
// spinner regardless of how the refresh was triggered.
//
// `untracked` keeps the effect's only dependency `clientRefreshing` — showToast /
// dismissToast read AND write the `toasts` signal, so tracking them here would
// make the effect re-trigger itself (a signals "Cycle detected").
effect(() => {
  if (!clientRefreshing.value) return;
  untracked(() => {
    dismissToast('update-available');
    showToast('Refreshing...', 'info', { key: 'refreshing', spinning: true, dismissable: false, showDuringRestart: true });
  });
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

// Engine-restart safety timeout. The initiating tab no longer mounts the
// UiBlockingOverlay during a restart (the gateway boot splash + GET-gate + SSE
// reconnect handle recovery), but the GET-gate `awaitEngineReady` still blocks
// reads on `engineRestarting` — so if the engine never comes back, something must
// clear the flag or reads hang forever. This effect carries the timeout the
// overlay used to own: arm it on the false→true edge, cancel on the true→false
// edge (reconnect via started_at, or a restart spawn-failure that reverts the
// flag). handleRestartTimeout probes health before declaring a timeout, so a
// frozen timer firing on iOS PWA resume (engine already restarted) doesn't show a
// false error — see its doc comment in connection.ts. The effect's only
// dependency is `engineRestarting`; handleRestartTimeout runs async (out of the
// tracking scope) so flipping the flag inside it can't form a cycle.
const RESTART_TIMEOUT_MS = 300_000;
let restartTimer: ReturnType<typeof setTimeout> | null = null;
effect(() => {
  if (engineRestarting.value) {
    if (restartTimer === null) {
      restartTimer = setTimeout(() => {
        restartTimer = null;
        void handleRestartTimeout();
      }, RESTART_TIMEOUT_MS);
    }
  } else if (restartTimer !== null) {
    clearTimeout(restartTimer);
    restartTimer = null;
  }
});
