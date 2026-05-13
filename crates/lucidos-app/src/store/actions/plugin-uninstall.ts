import { panelOverlay, showToast, closeInlineForm } from '../store';
import { confirmPluginUninstall, cancelPluginUninstall } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState } from './navigation';
import type { PluginUninstallRequest } from '../types';

/** Open the plugin uninstall panel for the staged uninstall in `request`.
 *  Mirrors `openPluginInstallRequest` — panel takes over the content pane,
 *  resolves via Confirm/Cancel POSTs to the engine. */
export function openPluginUninstallRequest(request: PluginUninstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-uninstall', request } };
  pushNavState();
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
  } catch (e) {
    showToast(`Uninstall failed: ${errorDetail(e)}`, 'error');
  } finally {
    closeInlineForm();
  }
}

/** User clicked Cancel — engine drops the pending entry + emits
 *  `PluginUninstallCanceled`. Closes the panel; failure is non-blocking
 *  (404 = entry already gone, which is harmless). */
export async function cancelPluginUninstallAction(uninstallId: string): Promise<void> {
  try {
    await cancelPluginUninstall(uninstallId);
  } catch (e) {
    console.error('cancel plugin uninstall failed:', e);
  }
  closeInlineForm();
}
