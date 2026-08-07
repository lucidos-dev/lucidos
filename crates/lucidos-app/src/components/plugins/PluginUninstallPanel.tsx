import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import type { PluginUninstallForm } from '../../store/store';
import {
  cancelPluginUninstallAction,
  confirmPluginUninstallAction,
} from '../../store/actions/plugin-uninstall';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { PluginFileList } from './PluginFileList';

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
          <PluginFileList
            label={`Will be deleted (${req.files_present.length})`}
            files={req.files_present}
            sectionClass="plugin-install-overwrites"
            fileClass="plugin-install-file-overwrite"
            note={<>Local edits to these files since install will be lost. Empty parent directories under <code>data/</code> are pruned.</>}
          />
        )}

        {req.files_missing.length > 0 && (
          <PluginFileList
            label={`Already gone (${req.files_missing.length})`}
            files={req.files_missing}
          />
        )}

        {renderActions()}
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
 *  because it broke the nav history the receipt exists to hold:
 *  `closeInlineForm()` blanks `panelOverlay` without touching the nav stack, so
 *  the cursor was left pointing at an entry describing a panel no longer on
 *  screen, and Back/Forward walked from that stale position. The header's back
 *  arrow is how you leave a receipt, same as any other panel page.
 *
 *  Exported for its unit test, which renders it directly: the suite's VNode walk
 *  stops at function components (the confirm branch's hooks would throw), so it
 *  cannot reach this one through the dispatcher. */
export function PluginUninstallReceiptPanel({ form }: { form: PluginUninstallForm }) {
  const req = form.request;
  const removed = form.removed!;
  return (
    <div class="inline-form">
      <div class="plugin-install-panel">
        <header class="plugin-install-header">
          <div class="panel-receipt-status">
            <span class="panel-receipt-badge">Uninstalled</span>
            <span class="panel-receipt-time">{formatMessageTimestamp(removed.at)}</span>
          </div>
          <div class="plugin-install-title-row">
            <span class="plugin-install-name">{req.plugin_name}</span>
            <span class="plugin-install-version">v{req.plugin_version}</span>
          </div>
          <p class="plugin-install-description">{removed.summary}</p>
        </header>

        {removed.files_deleted.length > 0 && (
          <PluginFileList
            label={`Deleted (${removed.files_deleted.length})`}
            files={removed.files_deleted}
          />
        )}

        {removed.files_missing.length > 0 && (
          <PluginFileList
            label={`Already gone (${removed.files_missing.length})`}
            files={removed.files_missing}
          />
        )}
      </div>
    </div>
  );
}
