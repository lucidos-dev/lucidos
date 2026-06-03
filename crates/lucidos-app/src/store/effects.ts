import { effect } from '@preact/signals';
import { pageTitle, unreadCount, animationSpeed, stepsExpanded, detailsExpanded, expandedFolders, threadDrawerOpen, selectedScope, notificationsFilter, collapsedExchanges, collapsedInitiators, filePreviewSource, repoSelectedChangeId, inputMode, SELECTED_CHANGE_KEY, MOBILE_VIEW_KEY } from './store';

// Sync page title with unread count
effect(() => {
  document.title = pageTitle.value;
});

// Persist unread count for instant title on next load
effect(() => {
  localStorage.setItem('lucidos-unread-count', String(unreadCount.value));
});

// Clean up stale localStorage keys — model/effort are now per-thread, not persisted
localStorage.removeItem('lucidos-model');
localStorage.removeItem('lucidos-reasoning-effort');
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

// Persist selected CC scope (Lucidos / external repo / app). Legacy
// `lucidos-cc-last-repo` is migrated once at signal-restore time inside
// store.ts; this effect only ever writes the new key.
effect(() => {
  localStorage.setItem('lucidos-cc-last-scope', JSON.stringify(selectedScope.value));
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
