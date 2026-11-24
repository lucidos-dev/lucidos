import type { SettingsSubview } from '../../store/store';
import type { PanelRefreshAction } from '../../store/panelRefresh';
import { loadPreferences } from '../../store/actions/preferences';
import { loadChatModels } from '../../store/actions/models';
import { loadCredentials } from '../../store/actions/credentials';
import { loadResponseStyles } from '../../store/actions/responseStyles';
import { loadThemeGallery } from '../../store/actions/themes';
import { loadDevices } from '../../store/actions/devices';
import { loadPairedDevices } from '../../store/actions/pairedDevices';
import { loadGatewayUsers } from '../../store/actions/gatewayUsers';
import { reloadRemoteGatewayLists } from '../../store/actions/remoteGatewayManage';
import { loadOAuthAccounts } from '../../store/actions/oauth';
import { refreshRepositories } from '../../store/actions/repositoriesLoader';

export const SETTINGS_SECTION_REFRESH: Partial<Record<SettingsSubview, PanelRefreshAction>> = {
  models: () => Promise.all([
    loadPreferences(),
    loadChatModels(),
    loadCredentials(),
    loadResponseStyles(),
  ]),
  appearance: () => Promise.all([
    loadPreferences(),
    loadThemeGallery(),
  ]),
  devices: () => Promise.all([
    loadDevices(),
    loadPairedDevices(),
    loadGatewayUsers(),
    reloadRemoteGatewayLists(),
  ]),
  users: () => Promise.all([
    loadGatewayUsers(),
    reloadRemoteGatewayLists(),
  ]),
  accounts: () => Promise.all([
    loadOAuthAccounts(),
    loadCredentials(),
  ]),
  'coding-agents': () => Promise.all([
    loadPreferences(),
    refreshRepositories(),
  ]),
};
