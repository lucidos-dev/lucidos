import {
  activeMenuItem,
  panelOverlay,
  mobileView,
  settingsSubview,
} from '../store';
import type { PanelOverlay } from '../store';
import { navigateToPane } from './pane';
import { isMobile } from '../../utils/viewport';
import type { SettingsSubview } from '../store';
import type { MenuItem } from '../types';
import { loadCredentials } from './credentials';
import { loadDevices } from './devices';
import { loadNotifications } from './notifications';
import { loadTriggers } from './triggers';
import { loadApps } from './apps';
import { pushNavState } from './navigation';

/** Set the active menu item, without pushing nav state or loading data.
 *  `overlay` defaults to `null` — clears any open sub-panel (app UI, file
 *  preview, URL preview, etc.) so the menu's main content is shown. Pass an
 *  overlay to atomically land on a deep link (e.g. a trigger details panel)
 *  in the same render as the menu switch — avoids the empty-list flash that
 *  results from clear-then-set across an await. */
export function setActiveMenu(item: MenuItem, overlay: PanelOverlay = null) {
  const prev = activeMenuItem.value;
  settingsSubview.value = 'main';

  panelOverlay.value = overlay;
  if (overlay?.type !== 'file-preview') localStorage.removeItem('file-preview-open');
  if (overlay?.type !== 'app-ui') localStorage.removeItem('app-window-open');

  activeMenuItem.value = item;
  localStorage.setItem('lucidos-active-menu-item', item);

  // Only on mobile — on desktop both layouts render simultaneously
  // so mobileView must not be mutated by desktop interactions.
  if (item !== prev && isMobile() && mobileView.value === 'thread') {
    navigateToPane('content');
  }
}

export function switchMenuItem(item: MenuItem) {
  setActiveMenu(item);

  if (item === 'apps') loadApps();
  if (item === 'triggers') loadTriggers();
  if (item === 'settings') loadDevices();
  if (item === 'notifications') loadNotifications();

  pushNavState();
}

/** Navigate into a settings subview (from within the settings panel). */
export function openSettingsSubview(key: Exclude<SettingsSubview, 'main'>) {
  settingsSubview.value = key;
  if (key === 'accounts') loadCredentials();
  pushNavState();
}

/** Navigate to Settings > Accounts (from outside settings — loads credentials). */
export function navigateToAccounts() {
  switchMenuItem('settings');
  openSettingsSubview('accounts');
}
