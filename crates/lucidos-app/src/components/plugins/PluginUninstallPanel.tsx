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

  // The action fns always closeInlineForm() in their own finally (unmounting
  // this panel), so busy normally never resets visibly — but reset in a finally
  // anyway so the buttons re-enable if a future action path returns without
  // closing the form. setBusy after unmount is a harmless no-op in Preact.
  async function handleConfirm() {
    setBusy(true);
    try {
      await confirmPluginUninstallAction(req.uninstall_id, req.plugin_name);
    } finally {
      setBusy(false);
    }
  }

  async function handleCancel() {
    setBusy(true);
    try {
      await cancelPluginUninstallAction(req.uninstall_id);
    } finally {
      setBusy(false);
    }
  }

  const totalKnown = req.files_present.length + req.files_missing.length;

  // Rendered twice — once top-right in the header, once at the bottom — so the
  // Cancel/Confirm pair is reachable without scrolling past a long file list.
  // A function (not a shared vnode) so each mount is a fresh element.
  const renderActions = (extraClass = '') => (
    <div class={`plugin-install-actions${extraClass ? ` ${extraClass}` : ''}`}>
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
  );

  return (
    <div class="inline-form">
      <div class="plugin-install-panel">
        <header class="plugin-install-header plugin-install-header-row">
          <div class="plugin-install-header-text">
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
          </div>
          {renderActions('plugin-install-actions-top')}
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

        {renderActions()}
      </div>
    </div>
  );
}
