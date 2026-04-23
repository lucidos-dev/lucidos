import {
  activeMenuItem,
  panelOverlay,
  mobileView,
  settingsSubview,
} from '../store';
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

/** Set the active menu item and clear overlays, without pushing nav state or loading data. */
export function setActiveMenu(item: MenuItem) {
  const prev = activeMenuItem.value;
  settingsSubview.value = 'main';

  // Always clear sub-panel overlay state (app UI, file preview, URL preview, etc.)
  // so clicking a menu item always shows that section's main content.
  // Overlays can be open without changing activeMenuItem (e.g. pinned app UIs),
  // so this must run unconditionally — not just when item !== prev.
  panelOverlay.value = null;
  localStorage.removeItem('file-preview-open');
  localStorage.removeItem('app-window-open');

  activeMenuItem.value = item;
  localStorage.setItem('cognos-active-menu-item', item);

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
