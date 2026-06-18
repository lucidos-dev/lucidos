import {
  activeMenuItem,
  panelOverlay,
  settingsSubview,
} from '../store';
import type { PanelOverlay } from '../store';
import { revealContentPane } from './pane';
import type { SettingsSubview } from '../store';
import type { MenuItem } from '../types';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadDevices } from './devices';
import { loadNotifications } from './notifications';
import { loadTriggers } from './triggers';
import { loadApps } from './apps';
import { loadPluginCatalog } from './plugin-marketplaces';
import { pushNavState } from './navigation';

/** Set the active menu item, without pushing nav state or loading data.
 *  `overlay` defaults to `null` — clears any open sub-panel (app UI, file
 *  preview, URL preview, etc.) so the menu's main content is shown. Pass an
 *  overlay to atomically land on a deep link (e.g. a trigger details panel)
 *  in the same render as the menu switch — avoids the empty-list flash that
 *  results from clear-then-set across an await.
 *
 *  Pure plumbing — does NOT manage panes. The user-intent callers
 *  (`switchMenuItem`, `openSettingsSubview`, `landOnAccountsWithOverlay`,
 *  the `thread-sync.ts` new-app / new-trigger branches) own the
 *  `revealContentPane()` call themselves. Earlier this function carried its
 *  own conditional `navigateToPane('content')` gated on `item !== prev &&
 *  mobileView === 'thread'`; that gate silently dropped the swipe when the
 *  user re-tapped the current item or wasn't on the chat pane. See
 *  `.claude/rules/frontend.md` — "Navigation that lands content must call
 *  revealContentPane()". */
export function setActiveMenu(item: MenuItem, overlay: PanelOverlay = null) {
  settingsSubview.value = 'main';

  panelOverlay.value = overlay;
  if (overlay?.type !== 'file-preview') localStorage.removeItem('file-preview-open');
  if (overlay?.type !== 'app-ui') localStorage.removeItem('app-window-open');

  activeMenuItem.value = item;
  localStorage.setItem('lucidos-active-menu-item', item);
}

export function switchMenuItem(item: MenuItem) {
  setActiveMenu(item);

  // Each loader sets its own Loadable failed state on error.
  if (item === 'apps') void loadApps();
  if (item === 'triggers') void loadTriggers();
  if (item === 'settings') void loadDevices();
  if (item === 'notifications') void loadNotifications();

  pushNavState();
  revealContentPane();
}

/** Navigate into a settings subview (from within the settings panel). */
export function openSettingsSubview(key: Exclude<SettingsSubview, 'main'>) {
  settingsSubview.value = key;
  if (key === 'accounts') void loadCredentials();
  if (key === 'environment-variables') void loadEnvironmentVariables();
  if (key === 'marketplaces') void loadPluginCatalog();
  panelOverlay.value = null;
  localStorage.removeItem('file-preview-open');
  localStorage.removeItem('app-window-open');
  pushNavState();
  revealContentPane();
}

/** Land on Settings > Accounts with `overlay` in a single render — caller
 *  pushes nav state so Back returns to where the user was, not to an empty
 *  Accounts intermediate. */
export function landOnAccountsWithOverlay(overlay: PanelOverlay): void {
  setActiveMenu('settings', overlay);
  settingsSubview.value = 'accounts';
  void loadCredentials();
  revealContentPane();
}
