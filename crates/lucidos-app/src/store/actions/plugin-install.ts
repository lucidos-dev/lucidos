import { panelOverlay, showToast, closeInlineForm } from '../store';
import { confirmPluginInstall, cancelPluginInstall } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState } from './navigation';
import type { PluginInstallRequest } from '../types';

/** Open the plugin install panel for the staged install in `request`. The
 *  panel takes over the content pane (same surface as the credential
 *  request panel); user stays on whatever menu item they were on, and the
 *  panel resolves via Confirm/Cancel POSTs to the engine. */
export function openPluginInstallRequest(request: PluginInstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
  pushNavState();
}

/** User clicked Confirm — engine writes files into `data/`, emits
 *  `PluginInstalled`, auto-reloads WASM modules if `auth-modules/` was
 *  touched. Closes the panel either way (the engine pops the pending
 *  entry up-front, so a failed confirm has no second chance — leaving
 *  the panel open just wedges the user with disabled buttons). */
export async function confirmPluginInstallAction(installId: string, pluginName: string): Promise<void> {
  try {
    const result = await confirmPluginInstall(installId);
    showToast(
      `${pluginName}: ${result.summary} (${result.installed_files.length} files)`,
      'success',
    );
  } catch (e) {
    showToast(`Install failed: ${errorDetail(e)}`, 'error');
  } finally {
    closeInlineForm();
  }
}

/** User clicked Cancel — engine drops the staged temp dir + emits
 *  `PluginInstallCanceled`. Closes the panel; the LLM's tool result already
 *  said "pending in panel" so no further confirmation is needed. */
export async function cancelPluginInstallAction(installId: string): Promise<void> {
  try {
    await cancelPluginInstall(installId);
  } catch (e) {
    // Log but don't block panel close — the user already decided. A 404
    // (entry already gone) is the most likely failure and is harmless.
    console.error('cancel plugin install failed:', e);
  }
  closeInlineForm();
}
