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
import { refreshPluginCatalogAfterMutation } from './plugin-marketplaces';

export function openPluginUninstallRequest(request: PluginUninstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-uninstall', request } };
  pushNavState();
  revealContentPane();
}

export async function uninstallPluginById(id: string): Promise<void> {
  try {
    const request = await stagePluginUninstall(id);
    openPluginUninstallRequest(request);
  } catch (e) {
    showToast(`Failed to stage plugin uninstall: ${errorDetail(e)}`, 'error');
  }
}

export function uninstallMarketplacePlugin(plugin: MarketplacePlugin): Promise<void> {
  return uninstallPluginById(plugin.id);
}

export function markPluginUninstalled(
  form: PluginUninstallForm,
  result: PluginConfirmUninstallResponse,
): boolean {
  if (activeInlineForm.value !== form) return false;
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
        affected_triggers: result.affected_triggers,
      },
    },
  };
  replaceNavState();
  return true;
}

export async function confirmPluginUninstallAction(form: PluginUninstallForm): Promise<void> {
  const { uninstall_id: uninstallId, plugin_name: pluginName } = form.request;
  let result: PluginConfirmUninstallResponse;
  try {
    result = await confirmPluginUninstall(uninstallId);
  } catch (e) {
    showToast(`Uninstall failed: ${errorDetail(e)}`, 'error');
    closeInlineFormIfActive(form);
    return;
  }
  void refreshPluginCatalogAfterMutation();
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
