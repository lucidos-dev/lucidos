import { useState } from 'preact/hooks';
import { activeInlineForm } from '../../store/store';
import type { PluginInstallForm } from '../../store/store';
import type { PluginLocalChangeOutcome } from '../../store/types';
import {
  cancelPluginInstallAction,
  confirmPluginInstallAction,
} from '../../store/actions/plugin-install';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { NO_ENGINE_REQUIREMENT_SENTENCE } from './engineRequirement';
import { PluginFileList } from './PluginFileList';
import { PluginSection, pluginPanelHeader } from './PluginSection';
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
  conflict: 'Cannot merge, your version is saved aside',
  replaced: 'Replaced, your version is saved aside',
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
        class="action-btn action-btn-confirm"
        onClick={handleConfirm}
        disabled={busy}
      >
        {req.overwrites.length > 0 ? 'Install and replace' : 'Install'}
      </button>
    </div>
  );

  return (
    <div class="inline-form protected-surface">
      <div class="plugin-install-panel">
        {pluginPanelHeader({
          status: 'Install plugin',
          name: req.plugin_name,
          version: req.plugin_version,
          description,
          actions,
        })}

        <section class="plugin-install-section">
          <div class="plugin-install-source-row">
            <span class="plugin-install-label">Source</span>
            <span class="plugin-install-source-type">
              {req.source_type === 'git' ? 'Git' : 'Archive'}
            </span>
          </div>
          <code class="plugin-install-source-value" data-tooltip={req.source}>
            {sourceField ?? req.source}
          </code>
          {req.engine_requirement == null && (
            <p class="plugin-install-note" data-role="engine-undeclared">
              {NO_ENGINE_REQUIREMENT_SENTENCE}
            </p>
          )}
        </section>

        {localChanges.length > 0 && (
          <PluginSection
            label="Your edits"
            count={localChanges.length}
            defaultOpen
            note={
              <>
                You changed these files after you installed. Lucidos merges
                your edits into the new version where it can. Where it cannot,
                it saves your version and a patch under <code>data/artifacts/</code>.
              </>
            }
            footer={
              <label class="plugin-install-keep-toggle">
                <input
                  type="checkbox"
                  checked={keepLocal}
                  disabled={busy}
                  onChange={(e) => setKeepLocal((e.target as HTMLInputElement).checked)}
                />
                <span>
                  Keep my edits. Turn this off to take the new version exactly
                  as shipped.
                </span>
              </label>
            }
          >
            <ul class="plugin-install-files">
              {localChanges.map((change) => (
                <li
                  key={change.path}
                  class={`plugin-install-file plugin-install-file-${change.outcome}`}
                >
                  <span>{change.path}</span>
                  <span class="plugin-install-outcome">
                    {localChangeLabel(change.outcome, keepLocal)}
                  </span>
                </li>
              ))}
            </ul>
          </PluginSection>
        )}

        {replacedOutright.length > 0 && (
          <PluginFileList
            label="Files to replace"
            files={replacedOutright}
            tone="danger"
            note="These files already exist in your workspace. Installing replaces them."
          />
        )}

        {newFiles.length > 0 && (
          <PluginFileList label="New files" files={newFiles} />
        )}

        {req.setup && (
          <PluginSection label="Setup steps">
            <div
              class="plugin-install-setup-body markdown-content"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(req.setup) }}
            />
          </PluginSection>
        )}
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
 *  because a receipt is a page in the nav history. The header's back arrow is
 *  how you leave it, same as any other panel page.
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
    <div class="inline-form protected-surface">
      <div class="plugin-install-panel">
        {pluginPanelHeader({
          status: 'Installed',
          receiptAt: installed.at,
          name: req.plugin_name,
          version: req.plugin_version,
          description: installed.summary,
        })}

        {local && (
          <section class="plugin-install-section">
            <div class="plugin-install-label">Your edits</div>
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
          <PluginFileList label="Files installed" files={installed.installed_files} />
        )}

        {req.setup && (
          <PluginSection label="Setup steps" defaultOpen>
            <div
              class="plugin-install-setup-body markdown-content"
              dangerouslySetInnerHTML={{ __html: renderMarkdown(req.setup) }}
            />
          </PluginSection>
        )}
      </div>
    </div>
  );
}
