import { effect } from '@preact/signals';
import { pageTitle, unreadCount, animationSpeed, stepsExpanded, detailsExpanded, expandedFolders, inputMode, threadDrawerOpen, selectedRepoId, notificationsFilter, collapsedExchanges } from './store';

// Sync page title with unread count
effect(() => {
  document.title = pageTitle.value;
});

// Persist unread count for instant title on next load
effect(() => {
  localStorage.setItem('cognos-unread-count', String(unreadCount.value));
});

// Clean up stale localStorage keys — model/effort are now per-thread, not persisted
localStorage.removeItem('cognos-model');
localStorage.removeItem('cognos-reasoning-effort');

// Persist animation speed
effect(() => {
  localStorage.setItem('cognos-animation-speed-slider', String(animationSpeed.value));
});

// Persist steps expanded state
effect(() => {
  localStorage.setItem('cognos-steps-expanded', String(stepsExpanded.value));
});

// Persist details (more/less) expanded state
effect(() => {
  localStorage.setItem('cognos-details-expanded', String(detailsExpanded.value));
});

// Persist expanded folders
effect(() => {
  localStorage.setItem('cognos-expanded-folders', JSON.stringify([...expandedFolders.value]));
});

// Persist input mode
effect(() => {
  localStorage.setItem('cognos-input-mode', JSON.stringify(inputMode.value));
});

// Persist thread drawer open state
effect(() => {
  localStorage.setItem('cognos-thread-drawer-open', String(threadDrawerOpen.value));
});

// Persist selected CC repo
effect(() => {
  localStorage.setItem('cognos-cc-last-repo', selectedRepoId.value);
});

// Persist notifications filter
effect(() => {
  localStorage.setItem('cognos-notifications-filter', notificationsFilter.value);
});

// Persist collapsed response panels
effect(() => {
  localStorage.setItem('cognos-collapsed-exchanges', JSON.stringify([...collapsedExchanges.value]));
});
