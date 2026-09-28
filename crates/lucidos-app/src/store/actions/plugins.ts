import { installedPlugins } from '../store';
import { setLoadingIfFresh, toFailed } from '../types';
import { fetchInstalledPlugins } from '../../api/client';

let loadSeq = 0;

/** Load the installed-plugins list (Plugins panel). Re-fetches on each
 *  call so the list reflects installs/uninstalls; `setLoadingIfFresh` keeps a
 *  revisit from flashing the spinner once the list is already loaded.
 *
 *  Only the newest call applies its result. Joining an in-flight call instead
 *  would let a reload after an install settle on a read from before it. */
export async function loadInstalledPlugins(): Promise<void> {
  const seq = ++loadSeq;
  setLoadingIfFresh(installedPlugins);
  try {
    const { plugins } = await fetchInstalledPlugins();
    if (seq !== loadSeq) return;
    installedPlugins.value = { status: 'loaded', data: plugins };
  } catch (e) {
    if (seq !== loadSeq) return;
    installedPlugins.value = toFailed(e);
  }
}
