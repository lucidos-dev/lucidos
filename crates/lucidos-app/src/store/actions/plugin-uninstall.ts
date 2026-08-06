import { activeInlineForm, panelOverlay, showToast, closeInlineFormIfActive } from '../store';
import type { PluginUninstallForm } from '../store';
import {
  ApiError,
  confirmPluginUninstall,
  cancelPluginUninstall,
  stagePluginUninstall,
  type PluginConfirmUninstallResponse,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState, replaceNavState } from './navigation';
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

/** The uninstall succeeded: turn the open panel into a read-only receipt in
 *  place, instead of closing it and revealing whatever was underneath. That
 *  receipt is what makes the uninstall a real destination in the content nav
 *  history. Without it the row keeps pointing at a staged `uninstall_id` the
 *  engine has already popped, so walking back onto it lands on the bare menu
 *  item and a reload drops it entirely (`isTransientForm`).
 *
 *  The engine's own partition is baked into the marker, not recomputed from
 *  `form.request`: `files_present` was only what existed at prepare time, and
 *  the receipt has to survive a remount on the form alone.
 *
 *  `replaceNavState`, not `pushNavState`: the panel is already on screen and
 *  mutating in place, so one uninstall keeps one history row, whose label flips
 *  to "Uninstalled <plugin>" via `getFormTitle`. That also retires the stale
 *  pending entry, which a Forward walk would otherwise re-render with a live
 *  Confirm button for files that are already gone.
 *
 *  Returns false when `form` is no longer the active overlay: Escape still
 *  dismisses the panel mid-request, and a late success must not resurrect it
 *  over whatever the user opened since. The caller falls back to a toast.
 *  Identity comparison, not a type check, so a second staged uninstall cannot
 *  absorb this one's receipt. */
export function markPluginUninstalled(
  form: PluginUninstallForm,
  result: PluginConfirmUninstallResponse,
): boolean {
  if (activeInlineForm.value !== form) return false;
  // Already a receipt, and re-stamping would move the timestamp off the real
  // uninstall. Unreachable through the panel (the receipt has no Confirm), kept
  // so the marker can only ever be written once.
  if (form.removed) return false;
  panelOverlay.value = {
    type: 'form',
    form: {
      type: 'plugin-uninstall',
      request: form.request,
      removed: {
        at: new Date().toISOString(),
        summary: result.summary,
        files_deleted: result.files_deleted,
        files_missing: result.files_missing,
      },
    },
  };
  replaceNavState();
  return true;
}

/** User clicked Confirm. The engine deletes the recorded files from `data/`,
 *  prunes empty parent dirs, emits `PluginUninstalled`, and auto-reloads WASM
 *  signers if `auth-modules/` paths were touched.
 *
 *  On success the panel becomes a receipt and stays put; the toast is only the
 *  fallback for when it could not (the user dismissed it mid-request), since a
 *  receipt on screen already says everything the toast would.
 *
 *  A failure closes the panel: the engine pops the pending entry up-front, so a
 *  failed confirm has no second chance, and leaving the panel open just wedges
 *  the user with a Confirm button that can only 404. */
export async function confirmPluginUninstallAction(form: PluginUninstallForm): Promise<void> {
  const { uninstall_id: uninstallId, plugin_name: pluginName } = form.request;
  // The try covers the REQUEST only, same as `confirmPluginInstallAction`:
  // everything after it runs with the files already gone, so a throw there is
  // not an uninstall failure and must not be reported as one.
  let result: PluginConfirmUninstallResponse;
  try {
    result = await confirmPluginUninstall(uninstallId);
  } catch (e) {
    showToast(`Uninstall failed: ${errorDetail(e)}`, 'error');
    closeInlineFormIfActive(form);
    return;
  }
  void refreshPluginCatalog();
  if (!markPluginUninstalled(form, result)) {
    const missingNote = result.files_missing.length > 0
      ? ` (${result.files_missing.length} already gone)`
      : '';
    showToast(
      `${pluginName}: ${result.summary} (${result.files_deleted.length} files removed${missingNote})`,
      'success',
    );
  }
}

/** User clicked Cancel — engine drops the pending entry + emits
 *  `PluginUninstallCanceled`. Closes the panel; 404/410 (entry already
 *  gone — engine cleanup race or peer device acted first) is harmless and
 *  swallowed. Anything else means the pending entry likely persists and the
 *  user — who explicitly clicked Cancel — should know it didn't take. */
export async function cancelPluginUninstallAction(form: PluginUninstallForm): Promise<void> {
  try {
    await cancelPluginUninstall(form.request.uninstall_id);
  } catch (e) {
    if (!(e instanceof ApiError) || (e.httpCode !== 404 && e.httpCode !== 410)) {
      showToast(`Cancel plugin uninstall failed: ${errorDetail(e)}`, 'error');
    }
  }
  closeInlineFormIfActive(form);
}
