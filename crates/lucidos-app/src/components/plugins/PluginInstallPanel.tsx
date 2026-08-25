import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import type { PluginInstallForm } from '../../store/store';
import type { PluginLocalChangeOutcome } from '../../store/types';
import {
  cancelPluginInstallAction,
  confirmPluginInstallAction,
} from '../../store/actions/plugin-install';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { formatMessageTimestamp } from '../../utils/formatTime';
import { PluginFileList } from './PluginFileList';
import { ProposeUpstreamButton } from './ProposeUpstreamButton';

export function PluginInstallPanel() {
  const form = activeInlineForm.value;
  if (form?.type !== 'plugin-install') return null;
  // Two components rather than one branching on `installed`, so the confirm
  // panel's hooks are never conditionally skipped: resolving the install
  // unmounts the confirm panel and mounts the receipt. Same split as
  // `EmailConfirmModal`.
  return form.installed
    ? <PluginInstallReceiptPanel form={form} />
    : <PluginInstallConfirm form={form} />;
}

/** Human label for one local-change outcome, in the second person, because the
 *  panel is telling the user what is about to happen to THEIR edit. */
const LOCAL_CHANGE_LABEL: Record<PluginLocalChangeOutcome, string> = {
  merged: 'Kept, merged into the new version',
  conflict: 'Cannot merge, your version saved aside',
  replaced: 'Replaced, your version saved aside',
  restored: 'You deleted this, the new version brings it back',
};

/** What the panel promises for one edited file, under the current keep control.
 *
 *  Clearing the control makes every row read as replaced, because that is then
 *  what confirming does. Leaving a merged row saying "kept" would promise the
 *  opposite of the request the button is about to send.
 *
 *  Exported for its unit test: the confirm panel holds the control in a hook,
 *  so the suite's VNode walk cannot reach inside it. */
export function localChangeLabel(
  outcome: PluginLocalChangeOutcome,
  keepLocal: boolean,
): string {
  // A restore is untouched by the keep control: the user deleted the file, so
  // there is no edit to keep or drop, and nothing gets saved aside either way.
  if (outcome === 'restored' || keepLocal) return LOCAL_CHANGE_LABEL[outcome];
  return LOCAL_CHANGE_LABEL.replaced;
}

/** Overwrites with no local edit of their own. An edited path gets its own row
 *  stating its own outcome. Listing it again under the blunt "will be replaced"
 *  heading would contradict that row. */
export function plainOverwrites(
  overwrites: string[],
  changes: { path: string }[],
): string[] {
  const edited = new Set(changes.map((c) => c.path));
  return overwrites.filter((f) => !edited.has(f));
}

function PluginInstallConfirm({ form }: { form: PluginInstallForm }) {
  const [busy, setBusy] = useState(false);
  const [keepLocal, setKeepLocal] = useState(true);
  const req = form.request;
  const localChanges = req.local_changes ?? [];

  const description = typeof req.manifest['description'] === 'string'
    ? (req.manifest['description'] as string)
    : '';
  const sourceField = typeof req.manifest['source'] === 'string'
    ? (req.manifest['source'] as string)
    : null;

  const overwriteSet = new Set(req.overwrites);
  const newFiles = req.files.filter((f) => !overwriteSet.has(f));
  const replacedOutright = plainOverwrites(req.overwrites, localChanges);

  // Rendered twice — once top-right in the header, once at the bottom — so the
  // Cancel/Install pair is reachable without scrolling past a long file list.
  // A function (not a shared vnode) so each mount is a fresh element.
  const renderActions = (extraClass = '') => (
    <div class={`plugin-install-actions${extraClass ? ` ${extraClass}` : ''}`}>
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
  );

  // The action fns resolve the panel themselves (into a receipt on success,
  // closed on failure), so busy normally never resets visibly. Reset in a
  // finally anyway so the buttons re-enable if a future path returns with the
  // panel still up. setBusy after unmount is a harmless no-op in Preact.
  async function handleConfirm() {
    setBusy(true);
    try {
      await confirmPluginInstallAction(form, keepLocal);
    } finally {
      setBusy(false);
    }
  }

  async function handleCancel() {
    setBusy(true);
    try {
      await cancelPluginInstallAction(form);
    } finally {
      setBusy(false);
    }
  }

  return (
    <div class="inline-form">
      <div class="plugin-install-panel">
        <header class="plugin-install-header plugin-install-header-row">
          <div class="plugin-install-header-text">
            <h2>Install plugin</h2>
            <div class="plugin-install-title-row">
              <span class="plugin-install-name">{req.plugin_name}</span>
              <span class="plugin-install-version">v{req.plugin_version}</span>
            </div>
            {description && <p class="plugin-install-description">{description}</p>}
          </div>
          {renderActions('plugin-install-actions-top')}
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

        {localChanges.length > 0 && (
          <section class="plugin-install-section plugin-install-local-changes">
            <div class="plugin-install-label">
              Your local changes ({localChanges.length})
            </div>
            <p class="plugin-install-note">
              You have edited these files since installing. Lucidos merges your
              changes into the new version where it can, and keeps a copy plus a
              patch under <code>data/artifacts/</code> wherever it cannot.
            </p>
            <ul class="plugin-install-files">
              {localChanges.map((change) => (
                <li
                  key={change.path}
                  class={`plugin-install-file plugin-install-file-${change.outcome}`}
                >
                  <code>{change.path}</code>
                  <span class="plugin-install-outcome">
                    {localChangeLabel(change.outcome, keepLocal)}
                  </span>
                </li>
              ))}
            </ul>
            <label class="plugin-install-keep-toggle">
              <input
                type="checkbox"
                checked={keepLocal}
                disabled={busy}
                onChange={(e) => setKeepLocal((e.target as HTMLInputElement).checked)}
              />
              <span>
                Keep my local changes. Clear this for a clean update that takes
                the new version as shipped.
              </span>
            </label>
          </section>
        )}

        {replacedOutright.length > 0 && (
          <PluginFileList
            label={`Overwrites (${replacedOutright.length})`}
            files={replacedOutright}
            sectionClass="plugin-install-overwrites"
            fileClass="plugin-install-file-overwrite"
            note="These files already exist in your workspace and will be replaced."
          />
        )}

        {newFiles.length === 0 ? (
          <section class="plugin-install-section">
            <div class="plugin-install-label">New files (0)</div>
            <p class="plugin-install-empty">No new files — every path overwrites an existing one.</p>
          </section>
        ) : (
          <PluginFileList label={`New files (${newFiles.length})`} files={newFiles} />
        )}

        {req.setup && (
          <section class="plugin-install-section plugin-install-setup">
            <div class="plugin-install-label">Setup instructions</div>
            <div
              class="plugin-install-setup-body markdown-content"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(req.setup) }}
            />
          </section>
        )}

        {renderActions()}
      </div>
    </div>
  );
}

/** The panel after a confirmed install: a read-only record of what the engine
 *  actually wrote, holding the nav-history slot the pending confirm had (see
 *  `markPluginInstalled`). The list comes off the receipt marker, not off
 *  `request.files`, which was only what the install *would* write.
 *
 *  Deliberately offers NO buttons at all. Install and Cancel are gone because
 *  the files have landed and the staged `install_id` is popped. Close is gone
 *  because it broke the nav history the receipt exists to hold:
 *  `closeInlineForm()` blanks `panelOverlay` without touching the nav stack, so
 *  the cursor was left pointing at an entry describing a panel no longer on
 *  screen, and Back/Forward walked from that stale position. The header's back
 *  arrow is how you leave a receipt, same as any other panel page.
 *
 *  The plugin's setup instructions stay on it, since they are the one thing the
 *  user may still need after the install and the setup thread is a pane away
 *  rather than in front of them.
 *
 *  Exported for its unit test, which renders it directly: the suite's VNode walk
 *  stops at function components (the confirm branch's hooks would throw), so it
 *  cannot reach this one through the dispatcher. */
export function PluginInstallReceiptPanel({ form }: { form: PluginInstallForm }) {
  const req = form.request;
  const installed = form.installed!;
  const local = installed.local_changes;
  return (
    <div class="inline-form">
      <div class="plugin-install-panel">
        <header class="plugin-install-header">
          <div class="panel-receipt-status">
            <span class="panel-receipt-badge">Installed</span>
            <span class="panel-receipt-time">{formatMessageTimestamp(installed.at)}</span>
          </div>
          <div class="plugin-install-title-row">
            <span class="plugin-install-name">{req.plugin_name}</span>
            <span class="plugin-install-version">v{req.plugin_version}</span>
          </div>
          <p class="plugin-install-description">{installed.summary}</p>
        </header>

        {local && (
          <section class="plugin-install-section plugin-install-local-changes">
            <div class="plugin-install-label">Your local changes</div>
            {local.merged.length > 0 && (
              <p class="plugin-install-note">
                Merged into the new version: {local.merged.join(', ')}.
              </p>
            )}
            {local.conflicted.length > 0 && (
              <p class="plugin-install-note">
                Could not merge: {local.conflicted.join(', ')}.
              </p>
            )}
            {local.replaced.length > 0 && (
              <p class="plugin-install-note">
                Replaced: {local.replaced.join(', ')}.
              </p>
            )}
            {local.restored.length > 0 && (
              <p class="plugin-install-note">
                You had deleted these, and the new version brings them back:
                {' '}{local.restored.join(', ')}.
              </p>
            )}
            {local.saved_paths.length > 0 && (
              <p class="plugin-install-note">
                Your versions are saved under <code>data/artifacts/</code>, each
                with a patch you can re-apply.
              </p>
            )}
            {local.merged.length > 0 && (
              <ProposeUpstreamButton pluginId={req.plugin_id} pluginName={req.plugin_name} />
            )}
          </section>
        )}

        {installed.installed_files.length > 0 && (
          <PluginFileList
            label={`Files written (${installed.installed_files.length})`}
            files={installed.installed_files}
          />
        )}

        {req.setup && (
          <section class="plugin-install-section plugin-install-setup">
            <div class="plugin-install-label">Setup instructions</div>
            <div
              class="plugin-install-setup-body markdown-content"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(req.setup) }}
            />
          </section>
        )}
      </div>
    </div>
  );
}
