import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import type { PluginUninstallForm } from '../../store/store';
import {
  cancelPluginUninstallAction,
  confirmPluginUninstallAction,
} from '../../store/actions/plugin-uninstall';
import { PluginFileList } from './PluginFileList';
import { pluginPanelHeader } from './PluginSection';

export function PluginUninstallPanel() {
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-uninstall') return null;
  // Two components rather than one branching on `removed`, so the confirm
  // panel's hooks are never conditionally skipped: resolving the uninstall
  // unmounts the confirm panel and mounts the receipt. Same split as
  // `EmailConfirmModal`.
  return form.removed
    ? <PluginUninstallReceiptPanel form={form} />
    : <PluginUninstallConfirm form={form} />;
}

function PluginUninstallConfirm({ form }: { form: PluginUninstallForm }) {
  const [busy, setBusy] = useState(false);
  const req = form.request;

  // The action fns resolve the panel themselves (into a receipt on success,
  // closed on failure), so busy normally never resets visibly. Reset in a
  // finally anyway so the buttons re-enable if a future path returns with the
  // panel still up. setBusy after unmount is a harmless no-op in Preact.
  async function handleConfirm() {
    setBusy(true);
    try {
      await confirmPluginUninstallAction(form);
    } finally {
      setBusy(false);
    }
  }

  async function handleCancel() {
    setBusy(true);
    try {
      await cancelPluginUninstallAction(form);
    } finally {
      setBusy(false);
    }
  }

  const present = req.files_present.length;
  const description = present === 0
    ? 'Its files are already gone. Uninstalling only clears its install record.'
    : `This deletes ${present} file${present === 1 ? '' : 's'} from your workspace.`;

  const actions = (
    <div class="plugin-install-actions">
      <button
        type="button"
        class="action-btn action-btn-secondary"
        onClick={handleCancel}
        disabled={busy}
      >
        Cancel
      </button>
      <button
        type="button"
        class="action-btn action-btn-danger"
        onClick={handleConfirm}
        disabled={busy}
      >
        {present === 0 ? 'Clear record' : 'Uninstall'}
      </button>
    </div>
  );

  return (
    <div class="inline-form protected-surface">
      <div class="plugin-install-panel">
        {pluginPanelHeader({
          status: 'Uninstall plugin',
          name: req.plugin_name,
          version: req.plugin_version,
          description,
          actions,
        })}

        {present > 0 && (
          <PluginFileList
            label="Files to delete"
            files={req.files_present}
            tone="danger"
            note="Any edits you made to these files are lost. Folders left empty are removed too."
          />
        )}

        {req.files_missing.length > 0 && (
          <PluginFileList label="Already gone" files={req.files_missing} />
        )}
      </div>
    </div>
  );
}

/** The panel after a confirmed uninstall: a read-only record of what the engine
 *  actually removed, holding the nav-history slot the pending confirm had (see
 *  `markPluginUninstalled`). The lists come off the receipt marker, not off
 *  `request.files_present`, which was only what existed at prepare time.
 *
 *  Deliberately offers NO buttons at all. Confirm and Cancel are gone because
 *  the files are gone and the staged `uninstall_id` is popped. Close is gone
 *  because a receipt is a page in the nav history. The header's back arrow is
 *  how you leave it, same as any other panel page.
 *
 *  Exported for its unit test, which renders it directly: the suite's VNode walk
 *  stops at function components (the confirm branch's hooks would throw), so it
 *  cannot reach this one through the dispatcher. */
export function PluginUninstallReceiptPanel({ form }: { form: PluginUninstallForm }) {
  const req = form.request;
  const removed = form.removed!;
  return (
    <div class="inline-form protected-surface">
      <div class="plugin-install-panel">
        {pluginPanelHeader({
          status: 'Uninstalled',
          receiptAt: removed.at,
          name: req.plugin_name,
          version: req.plugin_version,
          description: removed.summary,
        })}

        {removed.files_deleted.length > 0 && (
          <PluginFileList label="Deleted" files={removed.files_deleted} />
        )}

        {removed.files_missing.length > 0 && (
          <PluginFileList label="Already gone" files={removed.files_missing} />
        )}
      </div>
    </div>
  );
}
