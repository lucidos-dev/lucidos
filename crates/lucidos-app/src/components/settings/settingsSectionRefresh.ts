import type { SettingsSubview } from '../../store/store';
import type { PanelRefreshAction } from '../../store/panelRefresh';
import { loadPreferences } from '../../store/actions/preferences';
import { loadChatModels } from '../../store/actions/models';
import { loadCredentials } from '../../store/actions/credentials';
import { loadResponseStyles } from '../../store/actions/responseStyles';
import { loadThemeGallery } from '../../store/actions/themes';
import { loadDevices } from '../../store/actions/devices';
import { loadPairedDevices } from '../../store/actions/pairedDevices';
import { loadOAuthAccounts } from '../../store/actions/oauth';
import { refreshRepositories } from '../../store/actions/repositoriesLoader';

/** The panel refresh for the settings sections that `SettingsView` draws with
 *  a function rather than a component of their own. A section that IS a
 *  component registers its own, and so does any component inside one of these
 *  (the coding-agent binaries, the two allowlist editors). The panel refresh
 *  runs all of them. */
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
