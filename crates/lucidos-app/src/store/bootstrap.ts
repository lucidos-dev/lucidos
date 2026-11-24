import { createBootstrapRequester, sliceEntry, type BootstrapSliceTable } from './bootstrapRequester';
import { appsSlice, loadApps } from './actions/apps';
import { embeddingModelStatusSlice, loadEmbeddingModelStatus } from './actions/backgroundActivity';
import { lastSuccessfulBackupSlice, loadLastSuccessfulBackup } from './actions/backup';
import { changesStateSlice, refreshChangesState } from './actions/chat-changes';
import { devicesSlice, loadDevices } from './actions/devices';
import { checkEngineVersion, engineVersionStatusSlice } from './actions/engine-update';
import { chatLadderSlice, chatModelsSlice, loadChatLadder, loadChatModels } from './actions/models';
import { loadUnreadNotifications, unreadNotificationsSlice } from './actions/notifications';
import { loadPinnedApps, pinnedAppsSlice } from './actions/pinnedApps';
import { pluginCatalogSlice, refreshPluginCatalogAfterMutation } from './actions/plugin-marketplaces';
import { installedPluginsSlice, loadInstalledPlugins } from './actions/plugins';
import { loadPreferences, preferencesSlice } from './actions/preferences';
import { loadReaderDevices, readerDevicesSlice } from './actions/reader-devices';
import { loadReleaseNotices, releaseNoticesSlice } from './actions/releaseNotices';
import { loadRepositories, repositoriesSlice } from './actions/repositoriesLoader';
import { loadSpendCapHolds, spendCapHoldsSlice } from './actions/spendCapHolds';
import { filterFacetsSlice, loadFilterFacets } from './actions/thread-loading';
import { loadThreadQueue, threadQueueSlice } from './actions/threadQueue';
import { loadTriggerGroups, triggerGroupsSlice } from './actions/triggerGroups';
import { historicalTriggersSlice, loadHistoricalTriggers, loadTriggers, triggersSlice } from './actions/triggers';
import { loadUsers, usersSlice } from './actions/users';
import { loadWebhookIngress, webhookIngressSlice } from './actions/webhookIngress';
import { loadWebhookRefusals, webhookRefusalsSlice } from './actions/webhookRefusals';

export const BOOTSTRAP_SLICES: BootstrapSliceTable = {
  preferences: sliceEntry(preferencesSlice, loadPreferences),
  unread_notifications: sliceEntry(unreadNotificationsSlice, loadUnreadNotifications),
  reader_devices: sliceEntry(readerDevicesSlice, loadReaderDevices),
  users: sliceEntry(usersSlice, loadUsers),
  pinned_apps: sliceEntry(pinnedAppsSlice, loadPinnedApps),
  apps: sliceEntry(appsSlice, loadApps),
  chat_models: sliceEntry(chatModelsSlice, loadChatModels),
  intelligence_ladder: sliceEntry(chatLadderSlice, loadChatLadder),
  spend_cap_holds: sliceEntry(spendCapHoldsSlice, loadSpendCapHolds),
  last_successful_backup: sliceEntry(lastSuccessfulBackupSlice, loadLastSuccessfulBackup),
  release_notices: sliceEntry(releaseNoticesSlice, loadReleaseNotices),
  embedding_model_status: sliceEntry(embeddingModelStatusSlice, loadEmbeddingModelStatus),
  webhook_ingress: sliceEntry(webhookIngressSlice, loadWebhookIngress),
  webhook_refusals: sliceEntry(webhookRefusalsSlice, loadWebhookRefusals),
  installed_plugins: sliceEntry(installedPluginsSlice, loadInstalledPlugins),
  plugin_catalog: sliceEntry(pluginCatalogSlice, refreshPluginCatalogAfterMutation),
  triggers: sliceEntry(triggersSlice, loadTriggers),
  historical_triggers: sliceEntry(historicalTriggersSlice, loadHistoricalTriggers),
  trigger_groups: sliceEntry(triggerGroupsSlice, loadTriggerGroups),
  thread_queue: sliceEntry(threadQueueSlice, loadThreadQueue),
  repositories: sliceEntry(repositoriesSlice, loadRepositories),
  filter_facets: sliceEntry(filterFacetsSlice, loadFilterFacets),
  devices: sliceEntry(devicesSlice, loadDevices),
  changes_state: sliceEntry(changesStateSlice, refreshChangesState),
  engine_version_status: sliceEntry(engineVersionStatusSlice, checkEngineVersion, 'preferences'),
};

export const { requestBootstrap, holdGlobalFrame } = createBootstrapRequester(BOOTSTRAP_SLICES);
