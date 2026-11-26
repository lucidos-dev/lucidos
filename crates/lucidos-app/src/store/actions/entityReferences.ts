import { loadSpendCapHolds } from './spendCapHolds';
import { removeWindowItem } from '../../utils/windowStorage';
import { refreshReusableWidgets } from './widgets';
import { onDashboardEvent, refreshDashboardsNaming } from './dashboards';
import type { Dashboard } from '../../generated/thread-event-wire';
import { forgetAppById } from './appsById';
import { reloadFilterFacetsIfLoaded } from './thread-loading';
import { panelOverlay, appsList, installedPlugins, marketplaceCatalog, marketplaceScanning, triggers, credentials, environmentVariables, chatModels, oauthAccounts, repositories, llmConfigured, configuredProviders, mcpServersVersion, webhooksVersion, eventHooksVersion, permissionGrantsVersion, handshakeScriptsVersion, memoryEntriesVersion, promptFootprintVersion } from '../store';
import { checkHealth } from '../../api/client';
import { loadApps } from './apps';
import { loadInstalledPlugins } from './plugins';
import { pluginCatalogScanned, refreshPluginCatalogAfterMutation } from './plugin-marketplaces';
import { loadChatModels } from './models';
import { loadTriggers, loadHistoricalTriggers } from './triggers';
import { loadThreadQueue } from './threadQueue';
import { loadTriggerGroups } from './triggerGroups';
import { loadArtifacts, refreshArtifacts, invalidateFilePreview } from './artifacts';
import { refreshThemesIfLoaded } from './themes';
import { isActiveThemePath, refreshActiveTheme } from './preferences';
import { refreshWorkspaceFontsIfLoaded } from './workspaceFonts';
import { refreshClaudeGatewayStatusIfLoaded } from './claudeGateway';
import { refreshChatGptPlanStatusIfLoaded } from './chatgptPlan';
import { removePinnedAppLocal, loadPinnedApps } from './pinnedApps';
import { loadCredentials } from './credentials';
import { loadEnvironmentVariables } from './environmentVariables';
import { loadOAuthAccounts, handleOAuthAccountConnected } from './oauth';
import { loadRepositories } from './repositoriesLoader';
import { loadDevices, devices, getDeviceId } from './devices';
import { loadWebhookIngress } from './webhookIngress';
import { loadWebhookRefusals } from './webhookRefusals';
import { pruneNavHistory, type NavEntry } from './navigation';
import { clearAppStorage } from './app-bridge';

import { RECENTS_KEY, pruneRecents } from './searchRecents';
export { RECENTS_KEY, pruneRecents } from './searchRecents';

function forgetAppLabels(appId: string): void {
  forgetAppById(appId);
  reloadFilterFacetsIfLoaded();
}

async function probeLlmConfigured(): Promise<void> {
  const result = await checkHealth();
  if (result.status === 'loaded') {
    llmConfigured.value = result.data.llm_configured !== false;
    if (result.data.configured_providers !== undefined) {
      configuredProviders.value = result.data.configured_providers;
    }
  }
}

function invalidateArtifactPreview(data: Record<string, unknown>): void {
  const artifactPath = data.artifact_path as string | undefined;
  if (artifactPath) invalidateFilePreview(`artifacts/${artifactPath}`);
}

function pluginIdOf(data: Record<string, unknown>): string | undefined {
  const top = data.id as string | undefined;
  if (top) return top;
  const record = data.manifest as Record<string, unknown> | undefined;
  const manifest = record?.manifest as Record<string, unknown> | undefined;
  return manifest?.id as string | undefined;
}

function closeResolvedPluginPanel(
  eventType: string,
  pluginId: string | undefined,
  actor: { kind?: string; device_id?: string } | null | undefined,
): void {
  if (!pluginId) return;
  if (actor?.kind === 'device' && actor.device_id === getDeviceId()) return;
  const overlay = panelOverlay.value;
  if (overlay?.type !== 'form') return;
  const form = overlay.form;
  const installing = eventType === 'PluginInstalled' || eventType === 'PluginInstallCanceled';
  if (installing) {
    if (form.type !== 'plugin-install' || form.installed) return;
  } else if (form.type !== 'plugin-uninstall' || form.removed) {
    return;
  }
  if (form.request.plugin_id !== pluginId) return;
  panelOverlay.value = null;
}

function actorOf(data: Record<string, unknown>): { kind?: string; device_id?: string } | null {
  return (data.actor as { kind?: string; device_id?: string } | null | undefined) ?? null;
}

export function refreshLlmConfigured(): void {
  void probeLlmConfigured();
  setTimeout(() => {
    void probeLlmConfigured();
  }, 600);
}

export const PROVIDER_PREFERENCE_KEYS: ReadonlySet<string> = new Set([
  'opencode_free_enabled',
  'local_base_url',
  'provider_enabled_vertex',
  'provider_enabled_anthropic',
  'provider_enabled_openai',
  'provider_enabled_openrouter',
  'provider_enabled_xai',
  'provider_enabled_local',
  'provider_enabled_claude_gateway',
  'provider_enabled_chatgpt_plan',
  'claude_gateway_url',
]);

export function processSSEForReferences(type: string, data: Record<string, unknown>, eventId?: string): void {
  switch (type) {
    case 'TriggerCreated':
    case 'TriggerUpdated':
      void loadTriggers();
      void loadTriggerGroups();
      break;
    case 'TriggerEnabled':
    case 'TriggerDisabled':
    case 'TriggerExecuted':
      void loadTriggers();
      break;
    case 'DraftSubmissionAccepted':
    case 'OrdinarySubmissionAccepted':
    case 'ThreadQueued':
    case 'ThreadQueueAdmitted':
    case 'ThreadQueueDropped':
    case 'ThreadQueueCompleted':
    case 'ThreadQueueChanged':
    case 'CapacityPolicyChanged':
      void loadThreadQueue();
      break;
    case 'TriggerDeleted': {
      const triggerId = data.trigger_id as string;
      if (triggerId) {
        pruneRecents(triggerId, 'triggers');
        pruneNavHistory((entry) => isNavEntryStale(entry, 'trigger', triggerId));
        closeOverlayIfStale('trigger', triggerId);
      }
      void loadTriggers();
      void loadTriggerGroups();
      void loadHistoricalTriggers();
      break;
    }
    case 'TriggerGroupCreated':
    case 'TriggerGroupRenamed':
    case 'TriggerGroupReordered':
    case 'TriggerGroupDeleted':
      void loadTriggerGroups();
      break;
    case 'AppCreated': {
      const appId = data.app_id as string;
      if (appId) forgetAppLabels(appId);
      void loadApps();
      promptFootprintVersion.value++;
      break;
    }
    case 'AppOpened':
    case 'KnowhowRead':
      promptFootprintVersion.value++;
      break;
    case 'AppUpdated': {
      const appId = data.app_id as string;
      const name = data.name as string | undefined;
      if (appId) forgetAppLabels(appId);
      if (appId && name) patchRecentsMetadata(appId, 'apps', name);
      void loadApps();
      refreshReusableWidgets();
      if (appId) refreshDashboardsNaming(appId);
      promptFootprintVersion.value++;
      break;
    }
    case 'AppDeleted': {
      const appId = data.app_id as string;
      if (appId) {
        pruneRecents(appId, 'apps');
        pruneNavHistory((entry) => isNavEntryStale(entry, 'app', appId));
        removePinnedAppLocal(appId);
        closeOverlayIfStale('app', appId);
        clearAppStorage(appId);
        forgetAppLabels(appId);
      }
      void loadApps();
      refreshReusableWidgets();
      if (appId) refreshDashboardsNaming(appId);
      promptFootprintVersion.value++;
      break;
    }
    case 'DashboardTileAdded':
    case 'DashboardTileRemoved':
    case 'DashboardArranged': {
      const dashboard = data.dashboard as Dashboard | undefined;
      if (dashboard) onDashboardEvent(dashboard);
      break;
    }
    case 'ArtifactImported':
      invalidateArtifactPreview(data);
      void loadArtifacts();
      break;
    case 'ArtifactCreated':
    case 'ArtifactUpdated':
    case 'ArtifactDeleted':
      invalidateArtifactPreview(data);
      refreshArtifacts();
      break;
    case 'RepositoryImported':
      refreshArtifacts();
      break;
    case 'DataFileWritten':
    case 'DataFileEdited':
    case 'DataFileDeleted': {
      const dataPath = data.path as string | undefined;
      if (dataPath) invalidateFilePreview(dataPath);
      if (dataPath?.startsWith('artifacts/')) refreshArtifacts();
      if (dataPath?.startsWith('themes/')) refreshThemesIfLoaded();
      if (dataPath?.startsWith('fonts/')) {
        refreshWorkspaceFontsIfLoaded();
        void refreshActiveTheme();
      }
      if (isActiveThemePath(dataPath)) void refreshActiveTheme();
      break;
    }
    case 'PluginInstalled':
    case 'PluginUninstalled':
      if ([data.files, data.files_removed].some((files) => Array.isArray(files)
        && files.some((path) => typeof path === 'string' && path.startsWith('hooks/')))) {
        eventHooksVersion.value++;
      }
      if (appsList.value.status === 'loaded') void loadApps();
      if (triggers.value.status === 'loaded') void loadTriggers();
      if (installedPlugins.value.status === 'loaded') void loadInstalledPlugins();
      refreshThemesIfLoaded();
      refreshWorkspaceFontsIfLoaded();
      void refreshActiveTheme();
      closeResolvedPluginPanel(type, pluginIdOf(data), actorOf(data));
      break;
    case 'PluginInstallCanceled':
    case 'PluginUninstallCanceled':
      closeResolvedPluginPanel(type, pluginIdOf(data), actorOf(data));
      break;
    case 'PluginMarketplaceRegistered':
    case 'PluginMarketplaceRemoved':
      if (marketplaceCatalog.value.status === 'loaded') void refreshPluginCatalogAfterMutation();
      break;
    case 'PluginCatalogScanStarted':
      marketplaceScanning.value = true;
      break;
    case 'PluginCatalogScanned':
      pluginCatalogScanned();
      break;
    case 'CredentialCreated':
    case 'CredentialUpdated':
    case 'CredentialDeleted':
    case 'CredentialScopeInferred':
      if (credentials.value.status === 'loaded') void loadCredentials();
      refreshLlmConfigured();
      break;
    case 'ModelSpendCapReached':
    case 'ModelSpendCapHoldEnded':
      void loadSpendCapHolds();
      break;
    case 'ClaudeGatewaySignedIn':
    case 'ClaudeGatewaySignedOut':
      refreshLlmConfigured();
      refreshClaudeGatewayStatusIfLoaded();
      break;
    case 'ChatGptPlanSignedIn':
    case 'ChatGptPlanSignedOut':
      refreshLlmConfigured();
      refreshChatGptPlanStatusIfLoaded();
      break;
    case 'ChatGptPlanSignInStarted':
    case 'ChatGptPlanSignInCanceled':
    case 'ChatGptPlanSignInFailed':
    case 'ChatGptPlanLimitReached':
      refreshChatGptPlanStatusIfLoaded();
      break;
    case 'ClaudeGatewaySignInStarted':
    case 'ClaudeGatewaySignInCanceled':
    case 'ClaudeGatewaySignInFailed':
    case 'ClaudeGatewayManagedSettingsChanged':
      refreshClaudeGatewayStatusIfLoaded();
      break;
    case 'EnvironmentVariableSet':
    case 'EnvironmentVariableDeleted':
      if (environmentVariables.value.status === 'loaded') void loadEnvironmentVariables();
      break;
    case 'ModelCreated':
    case 'ModelUpdated':
    case 'ModelDeleted':
      if (chatModels.value.status === 'loaded') void loadChatModels();
      break;
    case 'McpServerRegistered':
    case 'McpServerUpdated':
    case 'McpServerRemoved':
    case 'McpServerStopped':
    case 'McpLaunchApproved':
    case 'McpServerDisabledToolsChanged':
      mcpServersVersion.value++;
      break;
    case 'MemoryCorrected':
      memoryEntriesVersion.value++;
      break;
    case 'WebhookCreated':
    case 'WebhookUpdated':
    case 'WebhookDeleted':
      webhooksVersion.value++;
      void loadWebhookIngress();
      void loadWebhookRefusals();
      break;
    case 'EventHookArmed':
    case 'EventHookDisarmed':
    case 'EventHookRemoved':
      eventHooksVersion.value++;
      break;
    case 'WebhookIngressDegraded':
    case 'WebhookIngressRecovered':
      void loadWebhookIngress();
      break;
    case 'WebhookDeliveriesRefused':
    case 'WebhookDeliveriesRecovered':
      void loadWebhookRefusals();
      break;
    case 'PermissionGrantsChanged':
      permissionGrantsVersion.value++;
      break;
    case 'HandshakeScriptApproved':
      handshakeScriptsVersion.value++;
      break;
    case 'OAuthAccountConnected':
      handleOAuthAccountConnected(data as {
        provider?: string;
        email?: string | null;
        actor?: { kind?: string; device_id?: string } | null;
      }, eventId);
      break;
    case 'OAuthAccountDeleted':
      if (oauthAccounts.value.status === 'loaded') void loadOAuthAccounts();
      break;
    case 'RepositoryAdded':
    case 'RepositoryRemoved':
      if (repositories.value.status === 'loaded') void loadRepositories();
      break;
    case 'DeviceRegistered':
    case 'DeviceRenamed':
    case 'DevicePushChanged':
    case 'DeviceDeleted':
    case 'DeviceHandedOver':
      if (devices.value.status === 'loaded') void loadDevices();
      break;
    case 'PinnedAppPinned':
    case 'PinnedAppUnpinned':
      if ((data.device_id as string | undefined) === getDeviceId()) {
        void loadPinnedApps();
      }
      break;
    case 'ThreadEvent':
      handleThreadTitleEvent(data);
      break;
  }
}

function handleThreadTitleEvent(data: Record<string, unknown>): void {
  const threadId = data.thread_id as string | undefined;
  const event = data.event as Record<string, unknown> | undefined;
  if (!threadId || !event) return;

  const eventType = event.type as string | undefined;
  if (eventType !== 'ThreadTitleGenerated' && eventType !== 'ThreadTitleRenamed') return;

  const title = event.title as string | undefined;
  if (!title) return;

  patchRecentsMetadata(threadId, 'threads', title);
}

function patchRecentsMetadata(id: string, category: string, name: string): void {
  try {
    const raw = localStorage.getItem(RECENTS_KEY);
    if (!raw) return;
    const recents: Array<{ id: string; category: string; title: string }> = JSON.parse(raw);
    let changed = false;
    for (const r of recents) {
      if (r.id === id && r.category === category && r.title !== name) {
        r.title = name;
        changed = true;
      }
    }
    if (changed) localStorage.setItem(RECENTS_KEY, JSON.stringify(recents));
  } catch {  }
}

function isNavEntryStale(entry: NavEntry, entity: string, id: string): boolean {
  const overlay = entry.overlay;
  switch (entity) {
    case 'app':
      return (overlay?.type === 'app-ui' && overlay.app.id === id)
        || (overlay?.type === 'form' && overlay.form.type === 'app-edit' && overlay.form.appId === id);
    case 'trigger':
      return overlay?.type === 'form' && overlay.form.type === 'trigger' && overlay.form.triggerId === id;
    default:
      return false;
  }
}

function closeOverlayIfStale(entity: string, id: string): void {
  const overlay = panelOverlay.value;
  if (!overlay) return;

  switch (entity) {
    case 'app':
      if (overlay.type === 'app-ui' && overlay.app.id === id) {
        panelOverlay.value = null;
        removeWindowItem('app-window-open');
      }
      break;
    case 'trigger':
      if (overlay.type === 'form' && overlay.form.type === 'trigger' && 'triggerId' in overlay.form && overlay.form.triggerId === id) {
        panelOverlay.value = null;
      }
      break;
  }
}
