import { removeWindowItem, writeWindowItem } from '../../utils/windowStorage';
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
import { ladderAnchor } from '../../components/settings/intelligenceLadder';
import type { LadderSurface } from '../../api/types';
import { resetContentScroll } from '../savedScroll';
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
import { providerAnchor } from '../modelProviders';

export function setActiveMenu(item: MenuItem, overlay: PanelOverlay = null) {
  settingsSubview.value = 'main';

  panelOverlay.value = overlay;
  if (overlay?.type !== 'file-preview') removeWindowItem('file-preview-open');
  if (overlay?.type !== 'app-ui') removeWindowItem('app-window-open');

  activeMenuItem.value = item;
  writeWindowItem('lucidos-active-menu-item', item);
}

export function switchMenuItem(item: MenuItem) {
  setActiveMenu(item);

  if (item === 'apps') void loadApps();
  if (item === 'plugins') void loadInstalledPlugins();
  if (item === 'triggers') void loadTriggers();
  if (item === 'notifications') void refreshActiveNotificationsTab();

  pushNavState();
  revealContentPane();
}

export function openSettingsSubview(key: Exclude<SettingsSubview, 'main'>) {
  setActiveMenu('settings');
  settingsSubview.value = key;
  if (key === 'accounts') void loadCredentials();
  if (key === 'devices') void loadDevices();
  if (key === 'environment-variables') void loadEnvironmentVariables();
  if (key === 'marketplaces') void loadPluginCatalog();
  pushNavState();
  revealContentPane();
}

export function landOnAccountsWithOverlay(overlay: PanelOverlay): void {
  setActiveMenu('settings', overlay);
  settingsSubview.value = 'accounts';
  void loadCredentials();
  revealContentPane();
}

export function openProviderSettings(): void {
  openSettingsSubview('providers');
}

export function openProviderBlock(provider: string): void {
  settingsScrollTarget.value = providerAnchor(provider);
  openSettingsSubview('providers');
}

export function openCodingAgentAutoAnswerSettings(): void {
  settingsScrollTarget.value = 'permissions:coding-agent-auto-answer';
  openSettingsSubview('permissions');
}

export function openMaintenanceSettings(): void {
  settingsScrollTarget.value = 'system:maintenance';
  openSettingsSubview('system-overview');
}

export function openCommandJudgeModelSettings(): void {
  settingsScrollTarget.value = 'models:command-judge';
  openSettingsSubview('models');
}

export function openAgentSandboxSettings(): void {
  settingsScrollTarget.value = 'agent-sandbox';
  openSettingsSubview('permissions');
}

export function openFreeProviderSettings(): void {
  openProviderBlock('opencode-free');
}

export function openCaptureContextSetting(): void {
  settingsScrollTarget.value = 'debugging:capture-context';
  openSettingsSubview('debugging');
}

export function openCacheCardsSetting(): void {
  settingsScrollTarget.value = 'models:cache-cards';
  openSettingsSubview('models');
}

export function openCacheKeepAliveSetting(): void {
  settingsScrollTarget.value = 'models:cache-keep-alive';
  openSettingsSubview('models');
}

export function openBackupSettings(): void {
  openSettingsSubview('backup');
}

export function openWebhookSettings(): void {
  openSettingsSubview('webhooks');
}

export function openWhatsNew(release?: string | null): void {
  whatsNewTargetRelease.value = release ?? null;
  if (release) resetContentScroll(settingsViewKey('whats-new'));
  openSettingsSubview('whats-new');
}

export function openConnectedAccountsSettings(provider?: string, scopes?: string): void {
  oauthConnectPrefill.value = provider ? { provider, scopes } : null;
  settingsScrollTarget.value = 'accounts:connected';
  openSettingsSubview('accounts');
}

export function openCredentialSettings(credentialId: string): void {
  settingsScrollTarget.value = credentialAnchor(credentialId);
  openSettingsSubview('accounts');
}

export function openIntelligenceLadder(surface: LadderSurface): void {
  settingsScrollTarget.value = ladderAnchor(surface);
  openSettingsSubview('intelligence');
}
