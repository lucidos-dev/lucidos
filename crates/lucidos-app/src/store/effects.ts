import { effect } from '@preact/signals';
import { pageTitle, unreadCount, animationSpeed, stepsExpanded, detailsExpanded, expandedFolders, inputMode, threadDrawerOpen, selectedRepoId, notificationsFilter, collapsedExchanges, collapsedInitiators, filePreviewSource } from './store';

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

// Persist input mode
effect(() => {
  localStorage.setItem('lucidos-input-mode', JSON.stringify(inputMode.value));
});

// Persist thread drawer open state
effect(() => {
  localStorage.setItem('lucidos-thread-drawer-open', String(threadDrawerOpen.value));
});

// Persist selected CC repo
effect(() => {
  localStorage.setItem('lucidos-cc-last-repo', selectedRepoId.value);
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
