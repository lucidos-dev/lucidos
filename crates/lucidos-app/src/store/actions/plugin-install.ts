import { activeInlineForm, panelOverlay, showConfirm, showToast, closeInlineFormIfActive } from '../store';
import type { PluginInstallForm } from '../store';
import {
  ApiError,
  confirmPluginInstall,
  cancelPluginInstall,
  stagePluginInstall,
  type PluginConfirmInstallResponse,
} from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { pushNavState, replaceNavState } from './navigation';
import { revealContentPane } from './pane';
import { focusThread } from './threads';
import type { MarketplacePlugin, PluginInstallRequest } from '../types';
import { refreshPluginCatalog } from './plugin-marketplaces';

/** Open the plugin install panel for the staged install in `request`. The
 *  panel takes over the content pane (same surface as the credential
 *  request panel); user stays on whatever menu item they were on, and the
 *  panel resolves via Confirm/Cancel POSTs to the engine. Reveals the content
 *  pane — this fires from an engine SSE event, so without it a mobile user (or
 *  a desktop user with a collapsed split) never sees the panel that just
 *  appeared. Same shape as `openEmailConfirmRequest`. (NOT the credential-request
 *  path: `landOnAccountsWithOverlay` additionally switches to Settings →
 *  Accounts, which is right for a credential and wrong for everything else.) */
export function openPluginInstallRequest(request: PluginInstallRequest): void {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
  pushNavState();
  revealContentPane();
}

/** Stage an install (or an update) from a catalog row and open the confirm
 *  panel. Lives here rather than in `plugin-marketplaces.ts` for two reasons:
 *  it belongs beside the opener it routes through, mirroring
 *  `uninstallMarketplacePlugin` in `plugin-uninstall.ts`, and this module
 *  imports `refreshPluginCatalog` from there, so the call would otherwise close
 *  an import cycle. */
export async function installMarketplacePlugin(plugin: MarketplacePlugin): Promise<void> {
  // An update overwrites the plugin's shipped content. If the user has locally
  // modified that content (the "Modified" badge), warn before staging: the
  // update will discard their changes. A fresh install, or an update with no
  // local edits, proceeds straight through.
  if (plugin.status === 'update_available' && plugin.modified) {
    const paths = plugin.modified_paths ?? [];
    const changed = paths.length
      ? ` Changed: ${paths.slice(0, 6).join(', ')}${paths.length > 6 ? ', …' : ''}.`
      : '';
    const ok = await showConfirm(
      `You've locally modified "${plugin.name}". Updating to v${plugin.version} will overwrite your changes.${changed}`,
      'Update anyway',
      { title: 'Overwrite local changes?', variant: 'danger' },
    );
    if (!ok) return;
  }
  try {
    openPluginInstallRequest(await stagePluginInstall(plugin.source));
  } catch (e) {
    showToast(`Failed to stage plugin install: ${errorDetail(e)}`, 'error');
  }
}

/** The install succeeded: turn the open panel into a read-only receipt in
 *  place, instead of closing it and revealing whatever was underneath. Same
 *  role as `markPluginUninstalled`, and the same reasons. The receipt is what
 *  makes the install a real destination in the content nav history, and
 *  `replaceNavState` keeps one install to one history row (relabelled
 *  "Installed <plugin>") while retiring the pending entry a Forward walk would
 *  otherwise re-render with a live Install button.
 *
 *  `installed_files` comes off the engine response rather than
 *  `form.request.files`, which was only what the install *would* write.
 *
 *  Returns false when `form` is no longer the active overlay, so a confirm that
 *  lands after the user dismissed the panel cannot resurrect it. */
export function markPluginInstalled(
  form: PluginInstallForm,
  result: PluginConfirmInstallResponse,
): boolean {
  if (activeInlineForm.value !== form) return false;
  // Already a receipt, and re-stamping would move the timestamp off the real
  // install. Unreachable through the panel (the receipt has no Install button),
  // kept so the marker can only ever be written once.
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
      },
    },
  };
  replaceNavState();
  return true;
}

/** User clicked Confirm. The engine writes files into `data/`, emits
 *  `PluginInstalled`, and auto-reloads WASM modules if `auth-modules/` was
 *  touched.
 *
 *  A failure closes the panel: the engine pops the pending entry up-front, so a
 *  failed confirm has no second chance, and leaving the panel open just wedges
 *  the user with disabled buttons. */
export async function confirmPluginInstallAction(form: PluginInstallForm): Promise<void> {
  const {
    install_id: installId,
    plugin_name: pluginName,
    plugin_version: pluginVersion,
  } = form.request;
  // The try covers the REQUEST only. Everything after it runs with the files
  // already on disk, so a throw there is not an install failure: inside the try
  // it would toast "Install failed" over a stamped "Installed" receipt, and the
  // close would silently no-op because the active form is by then the receipt
  // rather than `form`. `focusThread` is the concrete hazard, not a theoretical
  // one: it loads events and scrolls, and `focusThreadOrBootstrap` in
  // `threads.ts` already documents that it can throw.
  let result: PluginConfirmInstallResponse;
  try {
    result = await confirmPluginInstall(installId);
  } catch (e) {
    showToast(`Install failed: ${errorDetail(e)}`, 'error');
    closeInlineFormIfActive(form);
    return;
  }
  void refreshPluginCatalog();
  const receipted = markPluginInstalled(form, result);
  // When the plugin shipped NEW `setup` instructions the engine spawns a
  // Lucidos Agent thread to walk the user through them. Drop the user straight
  // into it so setup happens in front of them: the thread IS the feedback, so
  // we skip the success toast in that case (the panel already showed the setup
  // instructions; dumping them into a toast as well was the noise we removed).
  // The engine spawns it as a SubThread, whose queue `prepare` step eager-emits
  // MessageReceived, so on the common immediate-admit path the thread_summaries
  // row exists before this response returns and the thread is real and already
  // running when we focus. Use focusThread (not …OrBootstrap): if the spawn is
  // briefly queued (no row yet) a bootstrap fetch would 404 → "Thread not
  // found"; focusThread just sets focus and lets the row + events stream in
  // over SSE.
  //
  // The setup thread and the receipt do NOT compete: `focusThread` reveals
  // the THREAD pane, while the receipt sits in the CONTENT pane, so both
  // land. Closing the panel is what used to make them look exclusive.
  if (result.setup_thread_id) {
    // Guarded on its own, and NOT by the request's catch: a throw here is a
    // failed navigation, not a failed install, so it must say so. Reporting it
    // is not optional either. This action is awaited by a click handler that
    // does not catch, so an escaping rejection would be a silent unhandled one
    // (`.claude/rules/frontend.md` § No Hidden Errors), and the plugin IS
    // installed, so the user is owed both halves of that sentence.
    try {
      focusThread(result.setup_thread_id);
    } catch (e) {
      showToast(
        `Installed ${pluginName} v${pluginVersion}, but couldn't open its setup thread: ${errorDetail(e)}`,
        'error',
      );
    }
  } else if (!receipted) {
    // No setup thread and no receipt on screen (the user dismissed the panel
    // mid-install), so the toast is the only place the success can land. With
    // the receipt up it would only restate what the panel already says.
    showToast(`Installed ${pluginName} v${pluginVersion}`, 'success');
  }
}

/** User clicked Cancel — engine drops the staged temp dir + emits
 *  `PluginInstallCanceled`. Closes the panel; the LLM's tool result already
 *  said "pending in panel" so no further confirmation is needed. */
export async function cancelPluginInstallAction(form: PluginInstallForm): Promise<void> {
  try {
    await cancelPluginInstall(form.request.install_id);
  } catch (e) {
    // 404/410 = entry already gone (harmless race with engine cleanup or a
    // peer device's confirm/cancel); anything else means the temp dir likely
    // wasn't dropped and the user — who explicitly clicked Cancel — should
    // know it didn't take.
    if (!(e instanceof ApiError) || (e.httpCode !== 404 && e.httpCode !== 410)) {
      showToast(`Cancel plugin install failed: ${errorDetail(e)}`, 'error');
    }
  }
  closeInlineFormIfActive(form);
}
