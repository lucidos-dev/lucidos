import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import {
  cancelPluginInstallAction,
  confirmPluginInstallAction,
} from '../../store/actions/plugin-install';
import { renderMarkdown } from '../../utils/renderMarkdown';

export function PluginInstallPanel() {
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-install') return null;

  const req = form.request;
  const [busy, setBusy] = useState(false);

  const description = typeof req.manifest['description'] === 'string'
    ? (req.manifest['description'] as string)
    : '';
  const sourceField = typeof req.manifest['source'] === 'string'
    ? (req.manifest['source'] as string)
    : null;

  const overwriteSet = new Set(req.overwrites);
  const newFiles = req.files.filter((f) => !overwriteSet.has(f));

  // The action fns always closeInlineForm() in their own finally (unmounting
  // this panel), so busy normally never resets visibly — but reset in a finally
  // anyway so the buttons re-enable if a future action path returns without
  // closing the form. setBusy after unmount is a harmless no-op in Preact.
  async function handleConfirm() {
    setBusy(true);
    try {
      await confirmPluginInstallAction(req.install_id, req.plugin_name, req.plugin_version);
    } finally {
      setBusy(false);
    }
  }

  async function handleCancel() {
    setBusy(true);
    try {
      await cancelPluginInstallAction(req.install_id);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="inline-form">
      <div class="plugin-install-panel">
        <header class="plugin-install-header">
          <h2>Install plugin</h2>
          <div class="plugin-install-title-row">
            <span class="plugin-install-name">{req.plugin_name}</span>
            <span class="plugin-install-version">v{req.plugin_version}</span>
          </div>
          {description && <p class="plugin-install-description">{description}</p>}
        </header>

        <section class="plugin-install-section">
          <div class="plugin-install-source-row">
            <span class="plugin-install-label">Source</span>
            <span
              class={`plugin-install-source-type plugin-install-source-type-${req.source_type}`}
            >
              {req.source_type === 'git' ? 'GitHub / git' : 'Archive'}
            </span>
          </div>
          <code class="plugin-install-source-value" data-tooltip={req.source}>
            {sourceField ?? req.source}
          </code>
        </section>

        {req.overwrites.length > 0 && (
          <section class="plugin-install-section plugin-install-overwrites">
            <div class="plugin-install-label">
              Overwrites ({req.overwrites.length})
            </div>
            <ul class="plugin-install-files">
              {req.overwrites.map((f) => (
                <li class="plugin-install-file plugin-install-file-overwrite" key={f}>
                  {f}
                </li>
              ))}
            </ul>
            <p class="plugin-install-warning-text">
              These files already exist in your workspace and will be replaced.
            </p>
          </section>
        )}

        <section class="plugin-install-section">
          <div class="plugin-install-label">
            New files ({newFiles.length})
          </div>
          {newFiles.length === 0 ? (
            <p class="plugin-install-empty">No new files — every path overwrites an existing one.</p>
          ) : (
            <ul class="plugin-install-files">
              {newFiles.map((f) => (
                <li class="plugin-install-file" key={f}>{f}</li>
              ))}
            </ul>
          )}
        </section>

        {req.setup && (
          <section class="plugin-install-section plugin-install-setup">
            <div class="plugin-install-label">Setup instructions</div>
            <div
              class="plugin-install-setup-body markdown-content"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(req.setup) }}
            />
          </section>
        )}

        <div class="plugin-install-actions">
          <button
            type="button"
            class="action-btn action-btn-danger"
            onClick={handleCancel}
            disabled={busy}
          >
            Cancel
          </button>
          <button
            type="button"
            class="action-btn action-btn-confirm"
            onClick={handleConfirm}
            disabled={busy}
          >
            {req.overwrites.length > 0 ? 'Confirm and overwrite' : 'Install'}
          </button>
        </div>
      </div>
    </div>
  );
}
