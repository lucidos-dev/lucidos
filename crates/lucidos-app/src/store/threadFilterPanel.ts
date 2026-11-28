import { signal } from '@preact/signals';
import { pushOverlay, removeOverlay } from './overlayStack';
import { drawerGrouping } from './store';

const OVERLAY_ID = 'thread-filter-panel';

const PANEL_OPEN_KEY = 'lucidos-thread-filter-panel-open';

export const threadFilterPanelOpen = signal(localStorage.getItem(PANEL_OPEN_KEY) === 'true');

function setPanelOpen(open: boolean): void {
  threadFilterPanelOpen.value = open;
  if (open) localStorage.setItem(PANEL_OPEN_KEY, 'true');
  else localStorage.removeItem(PANEL_OPEN_KEY);
  syncEscapeRegistration();
}

let paneVisible = true;

export function setThreadFilterPaneVisible(visible: boolean): void {
  paneVisible = visible;
  syncEscapeRegistration();
}

function syncEscapeRegistration(): void {
  if (threadFilterPanelOpen.value && paneVisible) {
    pushOverlay({ id: OVERLAY_ID, dismiss: closeThreadFilterPanel, hasPanel: false });
  } else {
    removeOverlay(OVERLAY_ID);
  }
}

syncEscapeRegistration();

function canOpen(): boolean {
  return drawerGrouping.value === 'folders';
}

export function openThreadFilterPanel(): void {
  if (canOpen()) setPanelOpen(true);
}

export function closeThreadFilterPanel(): void {
  setPanelOpen(false);
}

export function toggleThreadFilterPanel(): void {
  setPanelOpen(!threadFilterPanelOpen.value && canOpen());
}
