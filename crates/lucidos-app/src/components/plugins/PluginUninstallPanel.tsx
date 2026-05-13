import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import {
  cancelPluginUninstallAction,
  confirmPluginUninstallAction,
} from '../../store/actions/plugin-uninstall';

export function PluginUninstallPanel() {
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-uninstall') return null;

  const req = form.request;
  const [busy, setBusy] = useState(false);

  async function handleConfirm() {
    setBusy(true);
    await confirmPluginUninstallAction(req.uninstall_id, req.plugin_name);
  }

  async function handleCancel() {
    setBusy(true);
    await cancelPluginUninstallAction(req.uninstall_id);
  }

  const totalKnown = req.files_present.length + req.files_missing.length;

  return (
    <div class="inline-form">
      <div class="plugin-install-panel">
        <header class="plugin-install-header">
          <h2>Uninstall plugin</h2>
          <div class="plugin-install-title-row">
            <span class="plugin-install-name">{req.plugin_name}</span>
            <span class="plugin-install-version">v{req.plugin_version}</span>
          </div>
          <p class="plugin-install-description">
            {req.files_present.length === 0
              ? `All ${totalKnown} recorded files are already gone — this just clears the install record.`
              : `Removes ${req.files_present.length} file${req.files_present.length === 1 ? '' : 's'} from your workspace.`}
          </p>
        </header>

        {req.files_present.length > 0 && (
          <section class="plugin-install-section plugin-install-overwrites">
            <div class="plugin-install-label">
              Will be deleted ({req.files_present.length})
            </div>
            <ul class="plugin-install-files">
              {req.files_present.map((f) => (
                <li class="plugin-install-file plugin-install-file-overwrite" key={f}>
                  {f}
                </li>
              ))}
            </ul>
            <p class="plugin-install-warning-text">
              Local edits to these files since install will be lost. Empty parent directories under <code>data/</code> are pruned.
            </p>
          </section>
        )}

        {req.files_missing.length > 0 && (
          <section class="plugin-install-section">
            <div class="plugin-install-label">
              Already gone ({req.files_missing.length})
            </div>
            <ul class="plugin-install-files">
              {req.files_missing.map((f) => (
                <li class="plugin-install-file" key={f}>{f}</li>
              ))}
            </ul>
          </section>
        )}

        <div class="plugin-install-actions">
          <button
            type="button"
            class="action-btn"
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
            {req.files_present.length === 0 ? 'Clear install record' : 'Confirm uninstall'}
          </button>
        </div>
      </div>
    </div>
  );
}
