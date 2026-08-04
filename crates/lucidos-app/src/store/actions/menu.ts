import {
  activeMenuItem,
  panelOverlay,
  settingsSubview,
  settingsScrollTarget,
} from '../store';
import type { PanelOverlay } from '../store';
import { revealContentPane } from './pane';
import type { SettingsSubview } from '../store';
import type { MenuItem } from '../types';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadDevices } from './devices';
import { refreshActiveNotificationsTab } from './notifications';
import { loadTriggers } from './triggers';
import { loadApps } from './apps';
import { loadInstalledPlugins } from './plugins';
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
  if (item === 'plugins') void loadInstalledPlugins();
  if (item === 'triggers') void loadTriggers();
  if (item === 'settings') void loadDevices();
  if (item === 'notifications') refreshActiveNotificationsTab();

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

/** Deep-link to Settings → Models → Providers in a single render and scroll to
 *  the Providers section. Used by the first-run provider onboarding (when no LLM
 *  provider is configured) so the "Set up your AI provider" button lands the
 *  user exactly where they add one. `settingsScrollTarget` drives the
 *  scroll-and-highlight effect in `SettingsView`. */
export function openProviderSettings(): void {
  setActiveMenu('settings');
  settingsSubview.value = 'models';
  settingsScrollTarget.value = 'models:providers';
  pushNavState();
  revealContentPane();
}

/** Deep-link to Settings → Backup in a single render. Used by the app-shell
 *  backup reminder's "Set up backup" button. No scroll target: the page is short
 *  and its health card, provider picker and schedule dropdown are all at the top,
 *  which is exactly what someone arriving from the reminder came for. */
export function openBackupSettings(): void {
  setActiveMenu('settings');
  settingsSubview.value = 'backup';
  pushNavState();
  revealContentPane();
}
