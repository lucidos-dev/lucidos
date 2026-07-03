import { panelOverlay, showToast, closeInlineForm } from '../store';
import {
  ApiError,
  confirmPluginUninstall,
  cancelPluginUninstall,
  stagePluginUninstall,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState } from './navigation';
import { revealContentPane } from './pane';
import type { MarketplacePlugin, PluginUninstallRequest } from '../types';
import { refreshPluginCatalog } from './plugin-marketplaces';

/** Open the plugin uninstall panel for the staged uninstall in `request`.
 *  Mirrors `openPluginInstallRequest` — panel takes over the content pane,
 *  resolves via Confirm/Cancel POSTs to the engine. Reveals the content pane
 *  so a mobile user (or a desktop user with a collapsed split) actually sees
 *  the SSE-triggered panel. */
export function openPluginUninstallRequest(request: PluginUninstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-uninstall', request } };
  pushNavState();
  revealContentPane();
}

/** Stage a plugin uninstall by id and open the confirm panel — the same
 *  confirm panel the `uninstall_plugin` LLM tool produces. The panel's Confirm
 *  deletes the recorded files (and auto-deletes the plugin's triggers); nothing
 *  is touched until then. Shared by the Store card and the Plugins → Installed
 *  row, which hold different shapes but only need the plugin id. */
export async function uninstallPluginById(id: string): Promise<void> {
  try {
    const request = await stagePluginUninstall(id);
    openPluginUninstallRequest(request);
  } catch (e) {
    showToast(`Failed to stage plugin uninstall: ${errorDetail(e)}`, 'error');
  }
}

/** Store "Uninstall" button. */
export function uninstallMarketplacePlugin(plugin: MarketplacePlugin): Promise<void> {
  return uninstallPluginById(plugin.id);
}

/** User clicked Confirm — engine deletes the recorded files from `data/`,
 *  prunes empty parent dirs, emits `PluginUninstalled`, auto-reloads WASM
 *  signers if `auth-modules/` paths were touched. Closes the panel either
 *  way (the engine pops the pending entry up-front, so a failed confirm
 *  has no second chance — leaving the panel open just wedges the user). */
export async function confirmPluginUninstallAction(uninstallId: string, pluginName: string): Promise<void> {
  try {
    const result = await confirmPluginUninstall(uninstallId);
    const missingNote = result.files_missing.length > 0
      ? ` (${result.files_missing.length} already gone)`
      : '';
    showToast(
      `${pluginName}: ${result.summary} (${result.files_deleted.length} files removed${missingNote})`,
      'success',
    );
    void refreshPluginCatalog();
  } catch (e) {
    showToast(`Uninstall failed: ${errorDetail(e)}`, 'error');
  } finally {
    closeInlineForm();
  }
}

/** User clicked Cancel — engine drops the pending entry + emits
 *  `PluginUninstallCanceled`. Closes the panel; 404/410 (entry already
 *  gone — engine cleanup race or peer device acted first) is harmless and
 *  swallowed. Anything else means the pending entry likely persists and the
 *  user — who explicitly clicked Cancel — should know it didn't take. */
export async function cancelPluginUninstallAction(uninstallId: string): Promise<void> {
  try {
    await cancelPluginUninstall(uninstallId);
  } catch (e) {
    if (!(e instanceof ApiError) || (e.httpCode !== 404 && e.httpCode !== 410)) {
      showToast(`Cancel plugin uninstall failed: ${errorDetail(e)}`, 'error');
    }
  }
  closeInlineForm();
}
