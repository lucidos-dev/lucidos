import { installedPlugins } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { fetchInstalledPlugins } from '../../api/client';

/** Load the installed-plugins list (Plugins panel). Re-fetches on each
 *  call so the list reflects installs/uninstalls; `setLoadingIfFresh` keeps a
 *  revisit from flashing the spinner once the list is already loaded. */
export async function loadInstalledPlugins(): Promise<void> {
  setLoadingIfFresh(installedPlugins);
  try {
    const { plugins } = await fetchInstalledPlugins();
    installedPlugins.value = { status: 'loaded', data: plugins };
  } catch (e) {
    installedPlugins.value = toFailed(e);
  }
}
