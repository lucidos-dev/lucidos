import { activeInlineForm, panelOverlay, showToast, closeInlineFormIfActive } from '../store';
import type { PluginInstallForm } from '../store';
import {
  ApiError,
  confirmPluginInstall,
  cancelPluginInstall,
  proposePluginUpstream,
  stagePluginInstall,
  type PluginConfirmInstallResponse,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState, replaceNavState } from './navigation';
import { revealContentPane } from './pane';
import { focusSpawnedThread } from './threads';
import type { MarketplacePlugin, PluginInstallRequest } from '../types';
import { refreshPluginCatalogAfterMutation } from './plugin-marketplaces';
import { refreshApps } from './apps';

export function openPluginInstallRequest(request: PluginInstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
  pushNavState();
  revealContentPane();
}

export async function installMarketplacePlugin(plugin: MarketplacePlugin): Promise<void> {
  try {
    openPluginInstallRequest(await stagePluginInstall(plugin.source));
  } catch (e) {
    showToast(`Failed to stage plugin install: ${errorDetail(e)}`, 'error');
  }
}

export function markPluginInstalled(
  form: PluginInstallForm,
  result: PluginConfirmInstallResponse,
): boolean {
  if (activeInlineForm.value !== form) return false;
  if (form.installed) return false;
  panelOverlay.value = {
    type: 'form',
    form: {
      type: 'plugin-install',
      request: form.request,
      installed: {
        at: new Date().toISOString(),
        summary: result.summary,
        installed_files: result.installed_files,
        removed_files: result.removed_files,
        local_changes: result.local_changes,
        affected_triggers: result.affected_triggers,
      },
    },
  };
  replaceNavState();
  return true;
}

export async function confirmPluginInstallAction(
  form: PluginInstallForm,
  keepLocalChanges = true,
): Promise<void> {
  const {
    install_id: installId,
    plugin_name: pluginName,
    plugin_version: pluginVersion,
  } = form.request;
  let result: PluginConfirmInstallResponse;
  try {
    result = form.request.hook_confirmation_required
      ? await confirmPluginInstall(installId, keepLocalChanges, true)
      : await confirmPluginInstall(installId, keepLocalChanges);
  } catch (e) {
    showToast(`Install failed: ${errorDetail(e)}`, 'error');
    closeInlineFormIfActive(form);
    return;
  }
  void refreshPluginCatalogAfterMutation();
  void refreshApps();
  const receipted = markPluginInstalled(form, result);
  if (result.hooks_needing_review?.length) {
    showToast(`Review these hooks in Settings > Event Hooks to arm their installed content: ${result.hooks_needing_review.join(', ')}.`, 'warning');
  }
  if (result.setup_thread_id) {
    try {
      focusSpawnedThread(result.setup_thread_id);
    } catch (e) {
      showToast(
        `Installed ${pluginName} v${pluginVersion}, but couldn't open its setup thread: ${errorDetail(e)}`,
        'error',
      );
    }
  } else if (!receipted && !result.hooks_needing_review?.length) {
    showToast(`Installed ${pluginName} v${pluginVersion}`, 'success');
  }
}

export async function cancelPluginInstallAction(form: PluginInstallForm): Promise<void> {
  try {
    await cancelPluginInstall(form.request.install_id);
  } catch (e) {
    if (!(e instanceof ApiError) || (e.httpCode !== 404 && e.httpCode !== 410)) {
      showToast(`Cancel plugin install failed: ${errorDetail(e)}`, 'error');
    }
  }
  closeInlineFormIfActive(form);
}

export async function proposePluginUpstreamAction(
  pluginId: string,
  pluginName: string,
): Promise<void> {
  let result: Awaited<ReturnType<typeof proposePluginUpstream>>;
  try {
    result = await proposePluginUpstream(pluginId);
  } catch (e) {
    showToast(`Couldn't prepare a patch for ${pluginName}: ${errorDetail(e)}`, 'error');
    return;
  }
  try {
    focusSpawnedThread(result.thread_id);
  } catch (e) {
    showToast(
      `Saved your ${pluginName} patch to data/${result.patch_path}, but couldn't open the thread: ${errorDetail(e)}`,
      'error',
    );
  }
}
