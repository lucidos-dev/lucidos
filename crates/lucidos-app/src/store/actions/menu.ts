import {
  activeMenuItem,
  oauthConnectPrefill,
  panelOverlay,
  settingsSubview,
  settingsScrollTarget,
  whatsNewTargetRelease,
} from '../store';
import { settingsViewKey } from '../../components/layout/contentViewKey';
import { credentialAnchor } from '../../components/credentials/credentialAnchor';
import { resetContentScroll } from '../../hooks/useScrollMemory';
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
 *  overlay to atomically land on a deep link (e.g. the new-trigger form)
 *  in the same render as the menu switch — avoids the empty-list flash that
 *  results from clear-then-set across an await.
 *
 *  Pure plumbing — does NOT manage panes. The user-intent callers
 *  (`switchMenuItem`, `openSettingsSubview`, `landOnAccountsWithOverlay`,
 *  `navigateToTrigger` in `triggers.ts`, and the new-app / new-trigger
 *  branches of `handleNavigationRequest` in `navigation-request.ts`) own the
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
  if (item === 'notifications') refreshActiveNotificationsTab();

  pushNavState();
  revealContentPane();
}

/** Land on a Settings sub-section, from anywhere, in a SINGLE nav push.
 *
 *  This sets the Settings menu item itself, so it is a complete destination
 *  rather than half of one. It used to set only `settingsSubview`, which forced
 *  every caller outside the Settings panel (the `navigate_ui` / notification-tap
 *  router, Search Everywhere, the Plugins → Marketplaces jump, the compose
 *  destination row) to pair it with `switchMenuItem('settings')`. Since both
 *  push, one tap then left TWO history entries: the sub-section, and a Settings
 *  home the user never saw. Back walked onto that phantom before returning where
 *  they came from. The pairing is gone from all four; a caller that wants the
 *  home list still calls `switchMenuItem('settings')` alone.
 *
 *  `setActiveMenu` does the overlay + localStorage clearing (an open app / file
 *  preview must not survive a settings deep link), so this owns only the
 *  sub-section, its data loader, the one push and the pane reveal. */
export function openSettingsSubview(key: Exclude<SettingsSubview, 'main'>) {
  setActiveMenu('settings');
  settingsSubview.value = key;
  // Each loader belongs to the sub-section that renders its data. `devices`
  // moved here from `switchMenuItem('settings')`, where it fetched on every
  // visit to the home list (which shows no device data) and not at all on a
  // deep link that skipped the home list.
  if (key === 'accounts') void loadCredentials();
  if (key === 'devices') void loadDevices();
  if (key === 'environment-variables') void loadEnvironmentVariables();
  if (key === 'marketplaces') void loadPluginCatalog();
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
  settingsScrollTarget.value = 'models:providers';
  openSettingsSubview('models');
}

/** Deep-link to the OpenCode Free switch, one row inside the same Providers
 *  section. It is where the onboarding surface sends a user with no key, whose
 *  one actionable option is the keyless tier.
 *
 *  It navigates and nothing else. The tier is opt-in and states its terms at
 *  the switch (ADR 0104). So the decision stays on the row the user lands on,
 *  never on the button that sent them there. */
export function openFreeProviderSettings(): void {
  settingsScrollTarget.value = 'models:opencode-free';
  openSettingsSubview('models');
}

/** Deep-link to Settings → System → Backup in a single render. Two callers: the
 *  app-shell backup reminder's "Set up backup" button, and the tap on the
 *  backup-failure toast. No scroll target: the page is short, and its health
 *  card, provider picker and schedule dropdown all sit at the top. That is
 *  exactly what either caller sends someone here for. */
export function openBackupSettings(): void {
  openSettingsSubview('backup');
}

/** Deep-link to Settings → Webhooks. One caller, the ingress bar's button.
 *
 *  It navigates and nothing else. The engine reports an ingress outage and never
 *  repairs one. So the page is where the user reads which hooks are affected,
 *  and when each last heard from its sender. No scroll target: the list is the
 *  whole page. */
export function openWebhookSettings(): void {
  openSettingsSubview('webhooks');
}

/** Deep-link to Settings → System → What's New, opened on the release `release`
 *  names. The single way in, so every caller says what it is announcing.
 *
 *  An update offer names one: its whole subject is that release, and the panel's
 *  ordinary rule (expand the release you are RUNNING) is the older one. Callers
 *  with nothing to announce pass nothing, which also CLEARS a target left by an
 *  offer the user walked away from.
 *
 *  A named release also drops the panel's remembered scroll. Landing on that
 *  release is the whole request, so restoring where the reader last parked would
 *  put them somewhere else entirely. An ordinary open still restores it. */
export function openWhatsNew(release?: string | null): void {
  whatsNewTargetRelease.value = release ?? null;
  if (release) resetContentScroll(settingsViewKey('whats-new'));
  openSettingsSubview('whats-new');
}

/** Deep-link to Settings → Accounts → Connected accounts and scroll to it.
 *
 *  The trip a backup user has to make and previously could not: the Backup page
 *  has no account UI, so a provider with nothing signed in shows a line pointing
 *  at Accounts. That line was static prose naming a path, which is a route the
 *  user has to walk themselves (and one a 2026-08-05 session got wrong by
 *  naming the Backup page instead). Now it is a link that lands them on the
 *  section. Mirrors `openProviderSettings`, including the
 *  `settingsScrollTarget` scroll-and-highlight.
 *
 *  `provider` prefills the Connect field, so arriving from Backup does not mean
 *  typing the name of the provider you were just looking at.
 *
 *  `scopes` carries what the connection is FOR, and is the difference between
 *  one consent screen and two. Without it Connect requests a bare sign-in, so a
 *  user arriving from Backup completed the provider's consent screen, returned,
 *  and faced *Grant access*: a second trip through the same screen for one
 *  intent. */
export function openConnectedAccountsSettings(provider?: string, scopes?: string): void {
  oauthConnectPrefill.value = provider ? { provider, scopes } : null;
  settingsScrollTarget.value = 'accounts:connected';
  openSettingsSubview('accounts');
}

/** Deep-link to one credential row in Settings > Accounts, and scroll to it.
 *
 *  The caller holds a credential id, never a name. A signed webhook stores the
 *  NAME of its credential, so the webhooks page resolves that name to a row
 *  first. A name it cannot resolve is the missing-credential state, which the
 *  row reports instead of linking into nothing. */
export function openCredentialSettings(credentialId: string): void {
  settingsScrollTarget.value = credentialAnchor(credentialId);
  openSettingsSubview('accounts');
}
