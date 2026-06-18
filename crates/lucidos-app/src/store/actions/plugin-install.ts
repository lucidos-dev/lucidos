import { panelOverlay, showToast, closeInlineForm } from '../store';
import { ApiError, confirmPluginInstall, cancelPluginInstall } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState } from './navigation';
import { revealContentPane } from './pane';
import { focusThread } from './threads';
import type { PluginInstallRequest } from '../types';
import { refreshPluginCatalog } from './plugin-marketplaces';

/** Open the plugin install panel for the staged install in `request`. The
 *  panel takes over the content pane (same surface as the credential
 *  request panel); user stays on whatever menu item they were on, and the
 *  panel resolves via Confirm/Cancel POSTs to the engine. Reveals the content
 *  pane — this fires from an engine SSE event, so without it a mobile user (or
 *  a desktop user with a collapsed split) never sees the panel that just
 *  appeared. Mirrors the credential-request path (landOnAccountsWithOverlay). */
export function openPluginInstallRequest(request: PluginInstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
  pushNavState();
  revealContentPane();
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
    void refreshPluginCatalog();
    // When the plugin shipped `setup` instructions the engine spawns a Lucidos
    // Agent thread to walk the user through them. Drop the user straight into it
    // so setup happens in front of them instead of silently in the background.
    // Use focusThread (not …OrBootstrap): the thread was spawned via the Thread
    // Queue and its summary row may not exist yet, so a bootstrap fetch would
    // 404 → "Thread not found". focusThread just sets focus and lets the row +
    // events stream in over SSE as the spawn runs.
    if (result.setup_thread_id) {
      focusThread(result.setup_thread_id);
    }
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
    // 404/410 = entry already gone (harmless race with engine cleanup or a
    // peer device's confirm/cancel); anything else means the temp dir likely
    // wasn't dropped and the user — who explicitly clicked Cancel — should
    // know it didn't take.
    if (!(e instanceof ApiError) || (e.httpCode !== 404 && e.httpCode !== 410)) {
      showToast(`Cancel plugin install failed: ${errorDetail(e)}`, 'error');
    }
  }
  closeInlineForm();
}
