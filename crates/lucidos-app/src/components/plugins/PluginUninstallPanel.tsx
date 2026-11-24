import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import type { PluginUninstallForm } from '../../store/store';
import {
  cancelPluginUninstallAction,
  confirmPluginUninstallAction,
} from '../../store/actions/plugin-uninstall';
import { AFFECTED_TRIGGERS_NEXT_RUN, AffectedTriggerList } from './AffectedTriggerList';
import { PluginFileList } from './PluginFileList';
import { pluginPanelHeader } from './PluginSection';

export function PluginUninstallPanel() {
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-uninstall') return null;
  return form.removed
    ? <PluginUninstallReceiptPanel form={form} />
    : <PluginUninstallConfirm form={form} />;
}

function PluginUninstallConfirm({ form }: { form: PluginUninstallForm }) {
  const [busy, setBusy] = useState(false);
  const req = form.request;

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

        {(req.affected_triggers ?? []).length > 0 && (
          <AffectedTriggerList
            label="Triggers that use these files"
            triggers={req.affected_triggers!}
            tone="danger"
            note={`These triggers are not part of this plugin, so uninstalling leaves them as they are. ${AFFECTED_TRIGGERS_NEXT_RUN}`}
          />
        )}

        {req.files_missing.length > 0 && (
          <PluginFileList label="Already gone" files={req.files_missing} />
        )}
      </div>
    </div>
  );
}

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

        {(removed.affected_triggers ?? []).length > 0 && (
          <AffectedTriggerList
            label="Triggers to check"
            triggers={removed.affected_triggers!}
            note={AFFECTED_TRIGGERS_NEXT_RUN}
          />
        )}
      </div>
    </div>
  );
}
